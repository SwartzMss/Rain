# Rain 配置说明

## 配置来源

Rain 将配置分成三类：

1. **启动配置**：`DATABASE_URL`、数据/日志目录、监听地址、`SERVER_PORT`、`RESET_DB` 和首次管理员配置只能通过环境变量提供。默认端口是 `8078`。
2. **DB 热配置**：上传、搜索、预览、临时结果、认证策略和 Issue 清理策略保存在 `system_settings`，管理员通过 `/api/admin/settings` 修改。旧业务 ENV 只在对应数据库字段尚未初始化时导入一次，已有数据库值始终优先。
3. **高级启动调优**：`RAIN_SEARCH_BACKEND` 和索引单行上限保留 ENV 兼容入口；普通部署使用默认值即可，`sqlite_fts` 只用于 legacy/诊断场景。

Issue 非活跃自动过期默认启用，阈值为 7 天；设置为 0 可以关闭。已有数据库中的显式配置不会被升级过程覆盖。

旧业务 ENV 只在对应数据库列为 NULL/未初始化时导入一次。已有数据库值（包括 `false`、`0` 和空数组）不会被 ENV 覆盖。修改数据库配置后，继续保留旧 ENV 不会改变运行时值。

## 热更新与重启

管理接口返回 `configured`、`effective`、`revision` 和 `pending_restart_fields`。认证阈值、Issue 策略、上传/临时结果容量和查询边界等热字段影响后续操作；已开始的单次搜索、上传或扫描会继续使用开始时捕获的策略。下调共享容量不会删除存量，但运行中任务后续申请空间可能被拒绝。

上传/处理并发、全局行读取并发、Tantivy writer、Argon2 和临时结果物化并发属于重启生效字段。保存后页面会显示待重启字段；Rain 不会自动重启进程，运维按部署方式手动重启即可。重启时数据库中的 configured 值成为新的 effective 值。

## 资源并发模式

管理员页面的运行时资源卡片展示启动探测到的 CPU、内存，以及当前生效的 3 项自适应参数：上传处理并发、Tantivy writer 并发和单个 writer heap。三项参数支持 `Manual`/`Auto`；在 `Auto` 下，runtime adaptive engine 只在进程启动时依据 CPU 与内存快照计算 effective 值，保存配置后需要重启才会应用新的计划。

上传接收、行读取和临时结果物化等其他并发字段仍属于持久化保护参数，不会出现在当前 3 项 RuntimeDecision 中。查询并发由系统内部保护逻辑派生，不开放配置，也不出现在管理员 resource modes 或 RuntimeDecision 中。Argon2id 同样由系统安全策略管理，管理员只能看到“Argon2id 已启用”状态。

管理设置会同时返回 `configured`、`effective`、`revision` 和 `pending_restart_fields`；页面只在资源探测失败时显示对应的 fallback 原因。Rain 不会在线调整已经创建的 semaphore 或 writer，也不会自动重启进程。

Issue 内容搜索会按统一排序键合并多个 Bundle 的结果，并受每请求 fan-out 和进程级查询额度保护；具体并发值属于内部 runtime 实现，不是管理员配置项。搜索取消或索引删除时会保留查询任务和 generation lease 的生命周期保护。

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
