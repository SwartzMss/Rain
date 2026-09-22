# #142：文件树异步、可恢复删除实施方案

状态：已按本方案实现，随 #142 实现 PR 一起提交；文档保留设计取舍、验收标准和后续优化项。

基准：2026-09-22，main `514795d`，#143 / PR #147 已合入。
需求：https://github.com/SwartzMss/Rain/issues/142

## 1. 结论与范围

统一将单文件、目录删除改为持久化任务：短事务接受请求并建立根节点墓碑，返回 202；后台使用持久化游标，先清索引再按后序删除节点，失败后从已提交进度继续。

核心选择：

- 新增 file_deletion_jobs 表，任务同时作为子树可见性的墓碑；不在请求中枚举或更新全部后代。
- 首版同一 Bundle 最多一个非终态文件删除任务；其他 Bundle 不受该限制。相同根节点重复请求返回同一任务，不重复执行。
- 通过 parent_id 判断祖先关系，不使用 path LIKE 推断目录归属。
- 对日志 offsets、segments 和实际 files 删除分别设定批次上限，禁止用删除父目录的外键级联代替分批。
- 新增任务状态查询，前端接受即关闭弹窗；恢复失败时显示重试状态，节点继续隐藏。
- 不改变上传并发、owner 权限、Blob GC 和 #143 自动清理豁免的适用范围。

“快速返回”表示请求不执行与后代数量成比例的清理，不承诺在 SQLite 正被其他事务占用时零等待。

## 2. 实施前现状与遗漏风险

当前入口在 backend/src/routes/files.rs::delete_file_node：load_bundle → owner 检查 → ensure_bundle_ready → fetch_file → delete_file_tree → activity touch → 204。

services/file_deletion.rs::delete_file_tree 已通过 db::write::run 执行，但事务内部直接 DELETE 根节点，触发 files.parent_id、offsets、segments 的级联删除和 FTS trigger，然后扫描剩余 files 重算容量。使用 writer admission 本身没有缩短事务。

repositories/files.rs 的 fetch_file/fetch_children 没有删除祖先过滤。routes/logs.rs、services/skill_tools.rs、routes/temp_results/service.rs 也直接读 files/log_segments；仅隐藏文件树 UI 或设置根节点 files.status 都不能阻止直接访问子节点、搜索和下载。

现有 db.rs Bundle 清理的 files 阶段使用 LIMIT 删除任意节点，选中父目录时仍可能级联大量后代。本次需要将该阶段改为叶节点批次，否则 Bundle/Issue 接管任务时可能重新触发大事务。其用户语义保持不变。

## 3. 数据模型与迁移

新增下一版本迁移（当前只有 0001，预计为 0002_file_deletion_jobs.sql，实施时按最新迁移编号确定），不修改 0001_initial.sql 或历史 fixture。

建议任务字段：

| 字段 | 用途 |
| --- | --- |
| id TEXT PRIMARY KEY | UUID，在 replay 闭包外生成 |
| bundle_id TEXT NOT NULL | 外键 bundles(id)，Bundle 物理删除后级联移除任务记录 |
| root_file_id INTEGER NOT NULL | 原始根节点 ID，任务历史标识，不加删除根时级联任务的外键 |
| root_name TEXT NOT NULL | 状态展示快照，限制长度 |
| requested_by_user_id | 发起者审计信息；授权始终检查当前 Issue owner |
| state | QUEUED / RUNNING / RETRY_WAIT / SUCCEEDED / SUPERSEDED |
| phase | WALK / OFFSETS / SEGMENTS / REMOVE_NODE / RECONCILE |
| cursor_file_id | 当前遍历/清理节点，不依赖内存栈，不设阻止节点删除的外键 |
| lease_token、lease_until | 防止重复 worker 和旧进程继续提交 |
| attempts、next_retry_at、last_error_code | 有界退避、可展示的失败类型，不返回原始 SQL/磁盘路径 |
| reconcile_after_id、reconcile_bytes | 分页重算容量进度 |
| deleted_files、deleted_offsets、deleted_segments | 已提交进度；不在接受请求时统计总量 |
| created_at、updated_at、finished_at | 调度、状态展示和历史清理 |

约束与索引：

- 对 bundle_id 建非终态部分唯一索引，涵盖 QUEUED、RUNNING、RETRY_WAIT，保证一个 Bundle 一个进行中任务。
- 建到期调度索引及 (bundle_id, root_file_id, created_at) 查询索引。
- 检查现有索引覆盖，必要时新增 files(bundle_id,parent_id,id)、log_segments(file_id,id)；offsets 已有 (file_id,line_number) 主键。
- 可见性使用统一 SQL view / repository 谓词，迁移中如引入 view，reset_schema 必须先 drop view，再 drop 新任务表和旧表。
- 旧数据库先按 v1 规则完成 legacy adoption，再运行 v2；不能把 v2 对象加进 v1 的历史兼容要求。
- migration 测试覆盖空库、已有 managed v1、legacy、重复启动、RESET_DB，以及 v2 checksum。

## 4. DELETE 与任务 API

现有 DELETE 路径保持不变，成功响应改为：

```json
{ "job_id": "uuid", "status": "QUEUED" }
```

返回 202，可附任务状态 URL 的 Location。新建 owner-only 状态接口 GET /api/file-deletion-jobs/{job_id}，以及按 Bundle 查询最近/进行中任务的接口或现有 owner 响应中的任务摘要，确保刷新页面后能够重新发现任务。后台失败不用客户端重发 DELETE 才恢复。

接受事务在 db::write::run 内重新检查：

1. Bundle 属于目标 Issue，Issue ACTIVE、Bundle READY 且 deleted_at 为空。
2. 当前用户仍为 Issue owner；文件属于该 Bundle，虚拟 root 不是物理文件，仍拒绝删除。
3. 先查已有任务，再做“可见文件”检查：同根进行中请求返回已有 job，避免墓碑导致重试变 404。
4. 同 Bundle 不同根已有进行中任务：返回 409 FILE_DELETION_BUSY 和已有任务标识。父删子、子删父和兄弟并发首版都遵循该规则，不做任务合并。
5. 插入 QUEUED 任务、游标指向根节点；短小的 activity 更新可并入事务，或保留提交后的 best-effort 更新。已提交任务不得因 activity 更新失败向前端报告删除失败。

提交之后发 Notify 唤醒 worker；Notify 丢失不影响持久化任务，周期扫描会找回。不能每个 HTTP 请求无界 spawn 一个完整任务。若响应丢失，重试由任务记录保证幂等；历史记录过期后对已物理删除节点返回 404。

任务状态只返回安全摘要：阶段、已删除数量、更新时间、是否等待重试。SUCCEEDED 与 SUPERSEDED 都为终态；后者表示 Bundle/Issue 整体删除已接管。状态接口允许 owner 查看处于 DELETING 的父 Issue，不能直接复用只允许 ACTIVE 的 load_bundle。父对象物理删除后返回 404，前端刷新父对象确认消失后结束跟踪。

## 5. 立即隐藏整棵子树

任务提交即为逻辑删除的生效点。活跃任务对应的 root 及其任意后代均不可见，RETRY_WAIT 时也不可见，不能因失败重现部分已删数据。

建议集中提供 visible_files view（或完全等价的共享 SQL），只控制文件删除可见性；Bundle/Issue 状态、权限仍由调用者保留。谓词通过当前文件的 parent_id 向上递归，发现同 Bundle 的非终态任务 root 即排除。祖先查找包含节点自身，以 ID 和 bundle_id 校验边界；使用可去重递归以避免损坏数据形成循环时无限执行。

没有活跃任务的 Bundle 应通过任务索引快速排除额外祖先计算。不要全库展开所有墓碑的后代，不引入首个 HTTP 请求扫描整棵树的成本，也不要使用未转义的 LIKE 或字符串目录前缀替代真实父子关系。

所有业务读取入口必须统一迁移：

- 文件树、指定节点、内容预览、行读取、下载。
- Bundle / Issue 日志搜索的结果和 COUNT、文件名搜索、指定 file_id 快捷路径、FTS 与 LIKE 两种路径。
- Skill manifest、list_files、search_logs、read_file_lines 及相关统计。
- 临时结果的 Issue 源文件解析和指定文件解析。

过滤应在 COUNT、排序分页、LIMIT 之前执行；LEFT JOIN 查询要保持没有可见文件时的原本语义。物理清理、摄取和容量核算继续读真实 files，不能误用 visible_files。

明确并发边界：202 提交后新开始读取必须看不到目标。已经取得读快照或已打开下载流的请求可按既有语义结束，不实现强制撤销网络流；客户端要丢弃删除前发出的陈旧树/搜索响应。已有物化临时结果是独立快照，不在本任务中级联删除；新建任务重新解析源时必须过滤墓碑。

祖先递归可能增加搜索成本，实施必须用 EXPLAIN QUERY PLAN 和大树/深树/FTS 分页数据验证：无任务常用查询不发生全库递归，有任务时结果与总数一致。不能以移除搜索过滤来换取性能。若性能不满足，先调整执行计划及索引，再决定是否需要额外持久化层级索引。

## 6. 后台游标和真正有界的删除

READY Bundle 不再接收文件结构写入，同 Bundle 单任务保证遍历稳定；整体 Bundle/Issue 删除通过第 8 节的接管规则协调。

任务不必预先收集全部 file IDs：使用一个持久化当前节点即可实现后序遍历。

1. WALK：查询当前节点的第一个未删除直接子节点（parent 索引，LIMIT 1）；有子节点就推进 cursor。每个事务最多执行固定数量的下降步骤，例如 100 步，深树也要提交后让出 writer。
2. 无子节点：进入 OFFSETS。每批按 (file_id,line_number) 最多删除 100 条 offsets。
3. OFFSETS 清空后进入 SEGMENTS。每批按 (file_id,id) 最多删除 100 条 segments；现有 trigger 同事务维护 FTS，不手工双删 FTS。
4. REMOVE_NODE：再次确认没有子节点、offsets、segments；删除这个叶节点，保存原 parent_id，进度和 cursor 更新与 DELETE 同事务提交。不是根就回到 parent，继续查询其剩余子节点；是根则进入 RECONCILE。
5. 目录同样按叶节点处理，不假设目录绝对没有关联日志。不允许删一个含百万行索引的叶文件来绕过日志批次。

每个批次在取得 writer admission 后，使用同一连接检查任务 token、租约未过期、任务非终态、Bundle/Issue 状态，再续租、执行 SQL、更新进度、提交。失去 token / 父级接管则停止，不能在事务外校验后排队执行无保护的 DELETE。内部 helper 不嵌套 run/acquire。

批次 row limit 是初始工程参数而非延迟保证：记录实际耗时和 FTS 工作量；按基准调整。每次提交释放 writer，必要时短暂 yield；调度器每个任务最多处理约 20 批就轮转，让多个删除任务也有机会推进。

持久化进度和数据删除原子提交，使 replay 不重复计数，崩溃不丢失游标。不把文件系统删除、Blob 删除或 Notify 放进 replay 闭包。

## 7. 容量核算

首版采取保守释放策略：接受任务时不减少 content_size_bytes，清理中也不提前释放整棵树容量。完成物理删除后，再分页核算剩余实际文件，最后一次事务更新 Bundle 容量并标记任务 SUCCEEDED。

重算通过 id keyset 每批读取最多 100 条 files 的大小/类型信息，累计值和 after_id 同事务持久化，避免最后用一次全 Bundle SUM 长时间持有 writer。同 Bundle 的非终态唯一约束保持到核算完成，防止另一个文件删除任务使累计结果失效。READY Bundle 不会继续摄取，父级删除则中止本次核算并接管。

计数沿用现有语义：目录不计，meta.preview_kind=archive 不计，其他普通文件按 size_bytes（空值按 0）。不要按压缩包原始大小释放容量，或把 Blob 共享引用误当作逻辑内容大小。

其他 Bundle 的并发上传在核算完成之前可能暂时仍看到旧占用，但不会因提前释放出现超配。最终更新必须要求 Bundle 仍 READY、Issue ACTIVE 和任务 token 有效；不得覆盖整体删除设置的 0。测试以实际剩余文件独立 SUM 验证完成值。

## 8. 与 Bundle / Issue 删除、#143 的协调

显式 file/tree 删除不受 RAIN_CLEANUP_EXEMPT_USERS 限制。白名单仅限制自动 Issue 非活跃清理，不能成为文件删除任务的授权条件。

Bundle/Issue 整体删除优先：其状态改变后读取已有父级状态过滤会隐藏所有文件；file worker 的下一批检查发现父级正在删除，停止局部清理，由原整体恢复链路完成。

接管事务将对应非终态 file jobs 置为 SUPERSEDED、清除旧 token。Issue 很大时不在 HTTP 中遍历全部 Bundle 的任务：先提交 Issue DELETING，由每个 file worker 和父级后台 cleanup 按 Bundle 有界接管。所有旧 worker 每批都检查父状态，不能越过该窗口继续写。

Bundle 真正开始物理清理前必须使该 Bundle 的 file job 终止；同 Bundle 至多一个活跃任务，因此更新有界。generic Bundle recovery、手动 Issue recovery、INACTIVE recovery 均需覆盖这一协作点。

同时把整体 Bundle 的 files 阶段改为仅删除无子节点且无日志/offsets 的叶节点批次，避免父级接管重新引入级联大事务。索引支持必须同步验证；不修改用户层面的 Bundle/Issue 删除状态机。

新增豁免后已经暂停的 INACTIVE Issue 保持 #143 的 DELETING 语义：不借 file job 恢复自动续删。file job 让位给父状态，owner 仍可按 #143 的规则显式结束整个 Issue 删除。

## 9. Worker、恢复与 GC

- 启动一个受控 worker，启动后即扫描，再按固定间隔（建议 30 秒）扫描；Notify 提供接受后的即时唤醒。
- 每轮选有限数量到期任务；使用 next_retry_at、updated_at 排序和租约条件，避免失败任务占满首屏饿死后续任务。
- 首次运行与周期运行共用实现；不等待全部恢复完成才开放 HTTP。
- 队列由数据库承担，进程退出后无需内存任务表。租约过期后可接管 RUNNING；旧 token 不能提交。
- 暂时性失败进入 RETRY_WAIT，指数退避并封顶（例如 1、2、4…60 分钟），记录安全错误码；持续失败仍隐藏节点并向 owner 显示，不能伪报完成。
- 确定的数据损坏单独报告并保留任务/墓碑供修复，不以回滚到可见状态作为错误处理。
- 终态任务保留例如 7 天便于刷新/请求重试；后台按固定批次清理历史记录，墓碑失效与任务终态原子处理。
- 真实 files 行删除前保留 blob_id 引用，删除后交给现有 Blob GC 判断无引用。任务不直接删除共享 blob，也不改变现有下载与 GC 的读保护边界。

重型 cleanup 并发限制复用/提取现有 HEAVY_CLEANUP_WRITER，file worker 按调度片段释放重型许可；不要拿着 SQLite writer admission 等待另一个重型任务完成。统一锁序并覆盖死锁测试。

## 10. 前端交互

修改 api/client.ts 的 deleteFile 返回类型，收到 202 后：

1. 关闭确认框、清除按钮 pending；后续树/Issue 刷新不再阻塞确认框。
2. 移除目标和缓存子树，关闭或失效当前选中的后代预览；失效相关搜索结果。
3. 刷新树时由服务端墓碑兜底，删除前发出的旧响应不能把节点加回来。
4. owner 视图显示“后台删除中”；失败显示“清理暂未完成，系统将重试”，不永久占用删除按钮。
5. 同 Bundle 再删其他节点提示已有清理任务，并展示状态；其他 Bundle 正常操作。
6. 仅在页面有进行中任务时做有界轮询，例如 2–5 秒并退避，卸载时停止；刷新后通过 Bundle 任务摘要恢复跟踪。
7. 完成后刷新容量；父级整体删除接管后停止 file job 轮询并刷新父级。

不展示没有可靠分母的百分比，可展示阶段与已清理数量。兼容现有 204 空响应的客户端封装行为，新增 202 JSON 不影响 Bundle 删除 API。

## 11. 验收与验证

功能/恢复测试：

- DELETE 在 worker 被测试 barrier 阻塞时已经返回 202；数据库中根、日志仍存在，但树、直接子 ID、下载、搜索结果及总数都不可见。
- 10,000 子文件目录、单文件大量 offsets/segments、深层目录、空目录、archive 包装节点逐批清理，断言每批实际受影响记录有界。
- 每个阶段和最终核算前强制中止 worker，关闭数据库、重新打开同一磁盘库，恢复后 FTS/offsets/files/容量正确。
- 租约失效、双 worker 接管、旧 token 等待 writer 期间过期时，旧批次无写入；注入事务失败后进度不重复。
- 同根重复 DELETE、响应丢失、不同根并发、父子重叠、非 owner、跨 Bundle ID、PROCESSING Bundle、无权查看任务。
- Bundle/Issue 手动删除与 file job 在每个阶段交错；INACTIVE + 豁免暂停恢复不被局部任务绕过。
- 容量核算期间并发上传其他 Bundle，不超配；相同 Blob 被其他文件引用时不删除共享数据。
- 用户名豁免不影响显式 file/tree 删除。
- 前端确认框收到 202 即结束等待、刷新重新发现任务、失败提示、终态停止轮询和丢弃旧树响应。

性能验收：

- 文件型 SQLite、多连接环境下并发运行实际上传/索引与大树删除，记录 HTTP 接受时间、writer 排队与持有时间、上传吞吐、前台写延迟 p50/p95/p99。
- 固定硬件、数据规模和无删除基线，对比不存在与存在任务时的 FTS、文件名搜索和深树查询计划。不能仅以 sleep/yield 存在推断公平性。
- 建议无额外 writer 阻塞时接受请求 p95 < 500ms 作为本地性能目标；最终阈值按项目基准环境固定，不写入容易抖动的普通单测。
- 测试显式证明父节点不会触发超批次级联。全量测试遇到环境/基线失败需在原 main 复现才能标记“已有问题”，不能仅凭改动文件判断无关。

已执行检查：后端 `cargo fmt --all`、`cargo check`、`cargo clippy -- -D warnings`、文件删除服务测试和迁移测试；前端 `npm run lint`、`npm test`、`npm run build`。大规模重启、租约抢占和端到端性能项仍建议在部署前按本节清单补充。

## 12. 实施拆分

建议一个功能 PR，以以下提交顺序审查，所有读取过滤和 API 切换必须一起上线：

1. v2 migration、任务 repository、可见性共享查询和升级/reset 测试。
2. 原子接受、幂等处理、状态 API、所有业务读取过滤。
3. 有界遍历与日志清理、事务内租约校验、分页容量核算、重启与周期恢复。
4. 父级删除接管、Bundle 叶节点清理、Blob 引用与并发回归。
5. 前端异步状态、陈旧响应处理、文档、端到端与性能验收。

主要改动位置：services/file_deletion.rs、新增 repositories/file_deletions.rs、repositories/files.rs、routes/files.rs/logs.rs/temp_results/service.rs、services/skill_tools.rs、db.rs、db/migrations.rs、main.rs/lib.rs/routes/mod.rs、frontend/api 与文件树 hooks/HomeView；补充 doc/DB.md 和迁移发布说明。

新增迁移后不直接回滚旧 binary 访问带活跃任务的新库：旧版本不认识墓碑，会重新暴露删除中的残余节点。回退需恢复升级前备份，或先完成任务并验证目标旧版本的迁移兼容策略。
