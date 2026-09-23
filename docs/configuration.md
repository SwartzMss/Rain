# Rain 配置说明

## 配置来源

Rain 将配置分成三类：

1. `DATABASE_URL`、数据/日志目录、监听地址、`RESET_DB`、首次管理员和 `RAIN_AI_MASTER_KEY` 是启动配置或密钥，只能通过环境变量提供。
2. 上传、搜索、预览、临时结果、认证策略等业务配置保存在 `system_settings`，管理员通过 `/api/admin/settings` 修改。
3. 搜索后端、索引单行上限和 AI 环境 fallback 是高级启动调优，保留 ENV 兼容入口，不出现在普通设置页。

旧业务 ENV 只在对应数据库列为 NULL/未初始化时导入一次。已有数据库值（包括 `false`、`0` 和空数组）不会被 ENV 覆盖。修改数据库配置后，继续保留旧 ENV 不会改变运行时值。

## 热更新与重启

管理接口返回 `configured`、`effective`、`revision` 和 `pending_restart_fields`。认证阈值、Issue 策略、上传/临时结果容量和查询边界等热字段影响后续操作；已开始的单次搜索、上传或扫描会继续使用开始时捕获的策略。下调共享容量不会删除存量，但运行中任务后续申请空间可能被拒绝。

上传/处理并发、全局行读取并发、Tantivy writer、Argon2 和临时结果物化并发属于重启生效字段。保存后页面会显示待重启字段；Rain 不会自动重启进程，运维按部署方式手动重启即可。重启时数据库中的 configured 值成为新的 effective 值。

## 管理接口并发

新 PATCH 形态为：

```json
{
  "expected_revision": "12",
  "changes": {
    "api_default_search_results": 30,
    "api_max_search_results": 60
  }
}
```

保存和审计在同一 SQLite 事务中提交。版本冲突返回 `SETTINGS_REVISION_CONFLICT`，客户端应重新 GET 后由管理员确认合并；不要自动覆盖。旧扁平字段仍作为兼容输入适配，但也使用同一保存事务；新版 `changes` 请求缺少版本会返回 428。

AI provider 保持独立存储和独立 `provider_revision`，API key 只保存加密值，结构化输出模式随数据库 provider 一起版本化；环境 provider 仍只在启动时读取。

## 故障与回退

升级前备份数据库、启动 ENV 和 AI master key。初始化校验失败不会自动清库，也不会静默裁剪数值。回退优先恢复升级前数据库备份和旧程序；旧二进制不保证能读取已执行新 migration 的数据库。
