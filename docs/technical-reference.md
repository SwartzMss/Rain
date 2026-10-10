# 技术说明与 API 摘要

[返回使用指南](../README.md) · [配置说明](configuration.md) · [开发与构建](development.md)

本文记录内部机制与接口入口；安装、日常使用和数据维护分别见使用指南及[部署与维护](operations.md)。

## 搜索与索引

默认后端为 Tantivy，SQLite 保存控制数据、行定位和生命周期元数据；日志 chunk 正文位于各 Bundle 的 Tantivy artifact。普通启动不需要外部数据库。

Issue / Bundle 关键词接口使用 trigram 子字符串候选查询，再读取正文做精确匹配，支持标识符、错误码和连续中文的部分匹配。关键词至少 3 个字符；结果包含命中附近摘要，默认 50 条、最多 100 条。Issue 查询按统一排序键合并 Bundle 结果，使用有界窗口以及进程级查询额度。

文件内容的详细搜索会生成临时结果，支持表达式与结果内搜索。源文件数和累计扫描字节数已不再作为截断上限，旧 `RAIN_TEMP_RESULT_MAX_SCAN_BYTES`、`RAIN_TEMP_RESULT_MAX_SOURCES` 不再生效；结果容量、扫描超时和并发保护仍生效。不能把关键词接口的最短关键词规则直接套用到整个表达式语法。

Tantivy 的单文件过滤已使用 indexed `file_id`；当前 `path`、`timeline` 是 stored 字段。候选仍需精确正文校验，有界分页解决的是命中保留内存，不代表查询只读取当前页的候选。索引背景见 [Tantivy 文档](performance/tantivy-v01.md)。

个人保存条件记录查询和稳定选项，不保存临时结果 ID，按用户隔离，复用时应用于当前 Issue。标签分享链接记录文件身份或查询链，打开时恢复文件或重新执行搜索，源数据变动会影响结果。

## 上传、文件存储与清理

上传完成后在后台进行解压、索引和发布，阶段状态包括 RECEIVING、EXTRACTING、INDEXING、PUBLISHING。接收阶段的并发与临时空间预算和最终 Issue 容量是不同的保护维度；递归解压共享条目数、深度、路径和压缩比等安全限制。

工作区位于数据根目录的 `.tmp/{task_id}/staging`。真实文件通过 BlobStore 接口访问，当前实现为 LocalCasBlobStore，按 SHA-256 存储在 `blobs/<hash前两位>/<完整hash>`；相同内容可被多个 Bundle 引用。任务完成或失败后清理 staging，未完成的文件树不应对外可见。

Bundle 采用逻辑删除；无引用 Blob 由后台 GC 按数据库实际引用检查，并经过 24 小时宽限期回收。文件内容分页使用行偏移索引，受字节预算及全局、单客户端并发约束；必要时返回带 `[response truncated]` 标记的有界前缀，并推进游标。

索引单行上限与页面展示单行上限是独立设置，不应混为同一默认值。当前高级索引单行上限默认为 256 KiB；配置入口见[配置说明](configuration.md)及 [config.rs](../backend/src/config.rs)。

## 临时结果生命周期

结果先登记为 STAGING，受活动 lease 保护，完成后发布为 ACTIVE。过期清理原子认领为 DELETING，再删除结果文件及记录；重启后继续清理遗留 DELETING 记录、陈旧 `.part`、`.ready-*` 和无数据库记录的孤儿文件。

默认单结果最多 64 MiB，总容量最多 1 GiB 或 1000 条记录，并发物化最多 2 个，按 IP 每分钟最多 10 次。没有工作区引用的 Preview 保留 30 分钟，完整结果保留 7 天，读取不续期；可调保护参数以管理员当前配置为准。Issue 页面会创建独立的工作会话，搜索请求显式携带发起页面捕获的会话 ID；复制标签页检测到会话冲突时会创建自己的会话。真实用户操作每 4 分钟以内最多同步一次；连续 3 小时没有用户操作时，页面关闭该 Issue 的所有内部查看标签并清理本地搜索状态。服务端过期时间额外留出最多 4 分钟同步余量；会话有效期间关联的结果不受原始 TTL 清理，会话结束或过期后恢复原 TTL 清理。工作会话不改变 Issue owner 的 7 天非活跃清理规则。

改变来源枚举、查询或写文件方式时，必须同时保留结果配额、读 lease、取消、超时和原子发布语义。

## SQLite 写入与就绪探测

SQLite 使用 WAL 和 30 秒 busy timeout。运行期写入通过共享 writer admission 排队，使用短事务；默认 Bundle 清理每批 100 行，每批提交后重新排队。该队列只协调一个进程内的写入，同一数据库不能由多个 Rain 实例共享。

Tantivy writer 独立于 SQLite writer admission，当前每个进程固定一个 Tantivy writer。管理员支持 Auto / Manual 的自适应项为上传处理并发与单个 writer heap，启动时计算生效值；准确的配置与重启规则见[配置说明](configuration.md)。

`/readyz` 保留写入后回滚的 SQLite 探测，结果缓存 5 秒，并发请求共享一次探测。数据库 schema、migration 与旧数据兼容规则见[数据库说明](../doc/DB.md)。

## 认证与权限实现

密码使用 Argon2id；Session 通过 HttpOnly Cookie 传递，数据库只保存 token 的 SHA-256 哈希。浏览器访问遵循同源策略，服务端不发送跨域许可响应头。后台每小时清理过期或已撤销 Session。

默认登录限流为每 IP 每分钟 20 次尝试、每用户名每 5 分钟 10 次失败，注册为每 IP 每小时 10 次。成功登录不累计用户名失败次数；Argon2 并发另有内部保护。实际策略以管理员配置为准。

管理员初始化与审计原子提交，数据库必须恰好存在一个活跃管理员。管理员是独立运营角色，不自动拥有普通用户业务写入权限；Issue 写操作需要校验普通用户身份和所有权。

## API 摘要

### Issues / Bundles

- `GET /api/issues`
- `POST /api/issues`
- `GET /api/issues/{issueCode}`
- `DELETE /api/issues/{issueCode}`
- `DELETE /api/issues/{issueCode}/bundles/{bundleHash}`

### Upload

- `POST /api/issues/{issueCode}/uploads`：返回 `202 Accepted`，响应包含 `task_id`、`bundle_hash` 和初始 `PROCESSING` 状态。
- `GET /api/uploads/{taskId}`：查询后台解压/索引任务状态。

Multipart 字段：

- `files`

### Files

- `GET /api/files/v1/{bundleId}/files/root`
- `GET /api/files/v1/{bundleId}/files/{fileId}`
- `GET /api/files/v1/{bundleId}/files/{fileId}/content`
- `GET /api/files/v1/{bundleId}/files/{fileId}/lines?start=0&limit=200`
- `GET /api/files/v1/{bundleId}/files/{fileId}/download`（需要登录；访客不可下载）
- `DELETE /api/files/v1/{bundleId}/files/{fileId}`

文件节点包含 `preview_kind`（`directory`、`text`、`binary` 或 `archive`），前端据此决定展开目录、显示文字查看器或显示二进制文件信息页。

### Search

- `GET /api/log/v2/{bundleId}/search?q=keyword`
- `GET /api/issues/{issueCode}/search?q=keyword`

搜索接口支持 `from`、`size` 分页，服务端限制 `from + size <= RAIN_API_MAX_SEARCH_WINDOW`；超出 offset 返回 `SEARCH_OFFSET_TOO_LARGE`，窗口溢出返回 `SEARCH_WINDOW_TOO_LARGE`（HTTP 400）。响应中的 `max_search_window` 是当前生效上限，Issue 内容搜索会在所有 Bundle 间做全局有界排序。

以上保留常用接口摘要，不是完整 API 契约；还包括认证、保存条件、上传会话、临时结果和后台管理等路由，入口见 [routes/mod.rs](../backend/src/routes/mod.rs)。示例中的 `bundleId` 按公开路由约定传日志包 hash，不能与数据库内部 id 混用；配置中的分页窗口以当前生效设置为准。

## 性能与后续工作

性能实验的方法、缓存条件和测量范围见[大日志基准](performance/large-log-baseline.md)。优化前后必须在同一机器、相同数据与配置下比较，短小样本不能代表 GiB 数据或并发负载。

当前高优先级搜索优化方案见[Issue 来源内存与 Tantivy 查询计划](superpowers/plans/2026-10-07-search-performance.md)。任务细粒度进度与取消、结构化事件查询、更完整的日志解析和多行异常合并属于独立后续方向。
