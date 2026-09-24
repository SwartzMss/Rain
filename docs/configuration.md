# Rain 配置说明

## 配置来源

Rain 将配置分成三类：

1. `DATABASE_URL`、数据/日志目录、监听地址、`RESET_DB` 和首次管理员配置是启动配置，只能通过环境变量提供。
2. 上传、搜索、预览、临时结果、认证策略等业务配置保存在 `system_settings`，管理员通过 `/api/admin/settings` 修改。
3. 搜索后端和索引单行上限是高级启动调优，保留 ENV 兼容入口，不出现在普通设置页。

旧业务 ENV 只在对应数据库列为 NULL/未初始化时导入一次。已有数据库值（包括 `false`、`0` 和空数组）不会被 ENV 覆盖。修改数据库配置后，继续保留旧 ENV 不会改变运行时值。

## 热更新与重启

管理接口返回 `configured`、`effective`、`revision` 和 `pending_restart_fields`。认证阈值、Issue 策略、上传/临时结果容量和查询边界等热字段影响后续操作；已开始的单次搜索、上传或扫描会继续使用开始时捕获的策略。下调共享容量不会删除存量，但运行中任务后续申请空间可能被拒绝。

上传/处理并发、全局行读取并发、Tantivy writer、Argon2 和临时结果物化并发属于重启生效字段。保存后页面会显示待重启字段；Rain 不会自动重启进程，运维按部署方式手动重启即可。重启时数据库中的 configured 值成为新的 effective 值。

Issue 内容搜索会在每个请求内最多并行查询 2 个 Tantivy bundle，进程内最多同时执行 4 个 Tantivy 查询；单 bundle 搜索也计入进程级额度。结果仍按统一排序键合并并分页，搜索取消或索引删除时会保留查询任务和 generation lease 的生命周期保护。

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

## 故障与回退

升级前备份数据库和启动 ENV。初始化校验失败不会自动清库，也不会静默裁剪数值。回退优先恢复升级前数据库备份和旧程序；旧二进制不保证能读取已执行新 migration 的数据库。
