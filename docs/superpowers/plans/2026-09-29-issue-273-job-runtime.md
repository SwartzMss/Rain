# Issue #273 Unified Job Runtime Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox syntax for tracking. 本文是本分支实现的设计依据；实现与验证结果以代码和 PR 为准。

**Goal:** 在单实例 Rain 中统一 Upload、Index、Search、Materialize、Cleanup 的注册、取消、超时、指标及退出管理，保留现有业务状态机和资源限制。

**Architecture:** 新增进程级 `JobRuntime`，负责内存任务登记、执行监督、周期调度和 shutdown；现有业务服务继续负责持久化、权限、publication、清理及恢复。采用合作式取消和受监督的子任务；沿用现有 admission semaphore，避免增加互相等待的资源队列。

**Tech Stack:** Rust 2024、Tokio、Actix Web、SQLx/SQLite、tracing；新增直接依赖 `tokio-util` 的 cancellation primitive，不引入 Redis、MQ 或持久化任务调度表。

---

## 依据与范围

- 需求：[GitHub #273](https://github.com/SwartzMss/Rain/issues/273)，当前没有补充评论；本文按整个 issue 的实现计划理解。
- 代码基线：本地 `7ef6095`。执行前重新核对 HEAD 和相关模块；不覆盖工作区已有计划文件。
- 本次计划只新增本文；业务代码、依赖和数据库均不修改。
- 不包含前端任务中心、公开通用取消 API、任务优先级、自动通用重试、跨进程任务恢复或分布式调度。
- 保留 SQLite/Tantivy 两种构建模式。Runtime 内存状态不替代 bundle、upload session、temp result、search publication 的数据库状态。

## 当前接入点与缺口

| 接入点 | 当前行为 | 计划改动 |
| --- | --- | --- |
| `backend/src/lib.rs::spawn_periodic_job` | 定时执行并记录错误；返回普通 JoinHandle | 由 runtime 注册 scheduler，按每次执行记录结果 |
| `backend/src/main.rs` | 保存部分后台 handle；HTTP server 返回后逐个 abort，不等待 | 同一个退出协调器关闭 admission、通知取消、等待和升级 abort |
| `backend/src/upload/job.rs::spawn_upload_job` | fire-and-forget；等待 processing semaphore；独立做 finalization | 注册 Upload，等待阶段可取消，完成 finalization 后才结束记录 |
| `backend/src/upload/session_finalizer.rs` | 每 5 秒恢复/交付 upload job | scheduler 停止后不再交付；交付失败保持可恢复语义 |
| `backend/src/search/rebuild.rs` | 周期启动 rebuild；包含 heartbeat 和 blocking build | Index 子任务及 heartbeat 受监督，blocking worker 有独立存活记录 |
| `backend/src/services/search_execution.rs` | 独立 token、deadline、capability、Committing 和 terminal receipt | 保留鉴权/receipt；底层执行控制接入 runtime |
| `backend/src/routes/temp_results/service.rs` | 有 search_id 的 preview spawn worker；legacy preview/full 走不同路径 | 所有搜索/物化入口均登记，保留原 HTTP 行为 |
| `backend/src/routes/issues.rs` | 删除 bundle/issue 后直接 spawn 清理 | 一次性 Cleanup；停止接收时保留持久化恢复入口 |
| `backend/src/routes/mod.rs` / `backend/src/blob_store.rs` | 多个定时/通知型清理 worker | 保留各自间隔、初始延时、通知和重试规则 |

## 设计决策

### 1. 职责边界和状态

Runtime 只描述执行，不把 `READY`、`STAGING`、`DELETING`、`Committing` 等业务状态收编进通用状态机。

建议在 issue 示例基础上增加明确终态 `Cancelled` 和 `TimedOut`，避免正常取消都被统计为业务失败。`Cancelling` 表示已经请求停止、执行/清理尚未完成。强制中止记 `Failed` 并附 `Aborted` 原因；panic 附 `Panicked` 原因。终态只能写一次。

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JobType { Upload, Index, Search, Materialize, Cleanup }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobStatus {
    Pending, Running, Cancelling,
    Completed, Failed, Cancelled, TimedOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason { Cancelled, TimedOut, Shutdown }
```

允许 `Pending -> Cancelled/TimedOut`，不运行任务主体；运行中的 stop 先进入 Cancelling，待业务清理及子任务退出后写入终态。完成与取消的竞态通过同一记录锁裁决；业务已经进入不可取消提交阶段时，以实际提交结果为准。

任务使用内部 UUID；`search_id`、`bundle_id` 作为关联字段。禁止把 capability、搜索表达式、原始日志、敏感路径写入日志或指标标签。

### 2. 运行模型与资源准入

- `AppState` 持有 cloneable `jobs: JobRuntime`，在 HTTP worker factory 外创建一次。构造 AppState 不自动启动后台循环，保持测试可控。
- `submit` 同步原子登记，再启动执行；返回可等待的结果 handle。拒绝提交必须返还未被消费的业务输入，上传方才能处理接收预算和临时目录。
- 登记与关闭 admission 使用同一同步边界，保证 shutdown 快照不会漏掉刚登记的任务。
- Upload 继续等待现有 `processing_permits`；Search 继续使用 `query_permits`；Tantivy writer 继续使用 `SearchResourceBudget`；Materialize 保留当前 try-acquire 和 `TEMP_RESULT_BUSY`。
- 不新增统一串行 semaphore，不把现有立即返回 busy 改成排队。Pending 反映最初准入等待；嵌套阶段等待以阶段/资源指标反映。
- 父任务等待子任务时，子任务不重复获取父任务已经持有的同类 permit。Upload -> Index、Search -> Materialize 为嵌套执行记录，不增加并发槽。
- scheduler 与一次业务执行分开登记：空闲 scheduler 不计 active jobs；同一周期任务不重叠；保留当前 interval/missed-tick 语义。
- 活跃记录随实际工作存活；终态记录采用 TTL 5 分钟和最多 1024 条的双重淘汰。成功/失败累计指标独立保存，不受记录淘汰影响。
- 保留现有业务 admission 上限，不在本 issue 任意增加拒绝业务请求的新硬限制；周期调度每个定义最多一个执行，子任务数量随父任务有界，完成 handle 及时回收。

### 3. 取消、timeout、提交和 blocking 的边界

- 使用 `tokio_util::sync::CancellationToken` 提供通知；stop 原因由 runtime 状态保存，不能仅根据 token 推断 timeout 或 shutdown。
- `JobContext` 提供 `checkpoint()`、停止通知、单调 deadline、关联 ID 和子任务控制。一次任务使用一个绝对 deadline，嵌套任务继承最早 deadline，不能在每个阶段重置超时。
- Search 沿用当前 effective scan timeout 和错误码；Upload/Index/Cleanup 默认不新增业务 timeout，shutdown deadline 独立控制。
- 超时首先请求合作式停止，不能直接 drop 包含数据库/文件发布操作的 future。业务适配器执行现有 abort/finalizer/retry 后才完成任务。
- 保留 Search `mark_committing` 和 publication 的短提交边界。cancel 与进入提交必须原子协调；开始提交后允许它完成，禁止 runtime 的通用包装器事后把成功发布改报取消。
- 每个 blocking closure 自己持有任务 guard、writer permit 和必要 generation lease。外层 async handle 被 abort 时，不能提前释放这些资源或报告全部工作已结束。
- 解压、扫描、索引批次、rebuild 文档循环加入 checkpoint；channel sender 关闭/取消时唤醒 writer，不能让 `blocking_recv` 永远等待。
- heartbeat 是父任务的辅助任务，不作为独立业务成功计数；父任务正常结束或 drop 都通知停止并回收，禁止遗留刷新 claim 的 heartbeat。

Tokio 已开始的 `spawn_blocking` 无法由 abort 停止。因此 shutdown 报告必须区分已退出 async 任务和仍存活 blocking 工作，后者保持资源占用并记录 outstanding；不承诺任意阻塞第三方调用能在固定秒数内终止。[Tokio 官方说明](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html)

### 4. Shutdown 顺序

1. 收到 SIGINT/SIGTERM（Windows 使用对应 Ctrl-C 分支）或 server 异常：原子关闭新根任务/周期触发准入。
2. 停止 HTTP 接收，同时请求运行中任务合作式取消。已经接受的任务仍可执行清理/finalization；必要子任务必须附着在现存父记录上，不能绕过关闭再创建根任务。
3. HTTP drain 与 job drain 并行，共享退出起点；推荐内部默认 grace 30 秒，由测试注入更短时长。本期不新增完整可配置设置项。
4. 到 grace deadline 后 abort 剩余 async handles，再给 join 收尾一个有界窗口（建议 1 秒）。报告 still-running blocking jobs；不能用一个无期限 join 抵消 grace deadline。
5. 输出 shutdown summary：completed、cancelled、failed、aborted、outstanding_blocking、elapsed。确保停止 task admission 在 HTTP server await 之前发生。
6. 所有退出路径都调用幂等 shutdown，包括 bind 失败、server error 和重复 signal；尽可能先 bind 成功再启动 worker。

Actix 默认信号处理需关闭，由应用统一协调；实现选用 locked 版本支持的 `disable_signals`、server handle stop API，不要求升级 Actix。[Actix HttpServer 文档](https://docs.rs/actix-web/latest/actix_web/struct.HttpServer.html)

### 5. 指标出口

沿用 tracing，增加结构化完成事件；在已有管理员 settings metadata 响应顶层增加只读 `jobs` 字段，不依赖 `runtime_plan` 是否存在。本期不新增页面或公开 job 查询接口。

按固定五种 JobType 提供 `active`、`queued`、`failed_total`、`cancelled_total`、`timed_out_total`、`completed_total`、`duration_count`、`duration_sum_ms`、`last_duration_ms`、`last_success_at`。active 包含 Running 和 Cancelling；queued 是 Pending；duration 从 Running 起算，另记录 queue duration。never-succeeded 的 last_success_at 为 null；进程重启累计值清零。

父子任务时长允许重叠，不能把各类型 duration 相加当作进程 CPU 时间。scheduler 循环的成功不是业务成功；每次 execution 的错误和 panic 都要被计入。权限继承管理员接口，不泄露用户级关联详情。

## 实施任务

### Task 1：Runtime 数据模型、控制与监督

**Create:** `backend/src/job_runtime/mod.rs`、`types.rs`、`context.rs`、`registry.rs`、`metrics.rs`、`tests.rs`。
**Modify:** `backend/src/lib.rs`、`backend/Cargo.toml`、`backend/Cargo.lock`。

- [ ] 在 Cargo 中加入直接依赖 `tokio-util = { version = "0.7", features = ["rt"] }`；显式启用 Tokio `time`、`signal`；dev dependency 启用 `test-util` 用于暂停时钟测试。沿用 lock 可兼容版本。
- [ ] 先写状态测试：Pending 到 Running 到 Completed；Pending 取消不调用主体；Running 取消等待清理；deadline 只触发一次；重复 finish 不增加计数；submit/close 竞态无遗漏。
- [ ] 实现上述状态与 `JobContext`，registry mutex 不跨 await；使用 watch 或正确注册后复查的 Notify 等待终态，避免检查/等待之间丢失通知。
- [ ] 实现内部 submit/await-result 入口，结果类型保持泛型，业务错误保持原类型。JobHandle 被业务 handler drop 不使任务脱离监督；取消必须走显式策略。
- [ ] supervisor 始终观察 worker 的 Result/JoinError；panic、外部 abort、正常错误只落一次终态。关闭后禁止新根任务；既有父任务收尾子工作仍纳入同一次 drain。
- [ ] 实现有界终态保留、固定类型聚合指标；active job 不因 TTL 淘汰。`AppState::new` 等构造入口统一注入 runtime。
- [ ] 运行 `cd backend && cargo test job_runtime`，预期上述测试通过；再 `cargo check --no-default-features`，避免 Runtime 意外依赖 Tantivy。
- [ ] 独立提交建议：`feat: add supervised in-process job runtime`。

推荐用例命名和断言：

```text
pending_cancel_does_not_invoke_body: called == false, status == Cancelled
finish_is_exactly_once: completed_total == 1, active == 0
shutdown_racing_submit_tracks_or_rejects: accepted == terminal + outstanding
timeout_waits_for_cleanup: cleanup_done precedes terminal observation
terminal_retention_is_bounded: receipts <= 1024, completed_total unchanged
```

### Task 2：周期调度及 Cleanup 全量接入

**Create:** `backend/src/job_runtime/scheduler.rs`、`backend/tests/job_runtime_cleanup.rs`。
**Modify:** `backend/src/lib.rs`、`backend/src/routes/mod.rs`、`backend/src/routes/issues.rs`、`backend/src/blob_store.rs`、`backend/src/upload/job.rs`、`backend/src/upload/session_finalizer.rs`、`backend/src/main.rs`。

- [ ] 用暂停时间/Notify 写测试：初始延时与间隔不变；一次 Err 后下一周期仍执行；panic 可观测且下一周期按原定间隔继续；同一 job 不重叠；shutdown 后无新 tick。
- [ ] 将 `spawn_periodic_job` 替换为 runtime scheduler 注册；迁移 blob GC/audit/recovery、deleting bundle、session cleanup、temp upload/result cleanup、inactive/manual issue cleanup、search artifact cleanup。
- [ ] session finalizer 的每次处理按 Upload 分类，search rebuild 按 Index 分类；空闲检查不制造无意义的业务成功记录，真正执行的任务才更新业务 success 指标。
- [ ] 保留 file deletion 的 Notify + 30 秒 timer + 每轮 100 次处理/yield；每轮执行登记 Cleanup，并在批次边界检查停止信号。
- [ ] 将 issues 中两处一次性删除 spawn 纳入 runtime。已持久化 DELETING 后若 admission 关闭，保持 Accepted/后续恢复语义；手动删除 lease 按现有 retry/过期规则释放，不丢失任务。
- [ ] invariant recovery supervisor 接入 Cleanup，保留 recovery readiness gate、超时和退避；不能因 scheduler 停止将 readiness 标记成功。
- [ ] 检查每个闭包返回 Err 的链路，避免业务层 log 后 `Ok(())` 导致 runtime 统计成功。
- [ ] 运行 `cd backend && cargo test --test job_runtime_cleanup` 和 `cargo test --test ownership --test smoke`，预期定时/通知行为及恢复断言通过。
- [ ] 独立提交建议：`refactor: supervise cleanup and maintenance jobs`。

### Task 3：Upload 与 Index 生命周期及阻塞任务

**Create:** `backend/tests/job_runtime_upload.rs`。
**Modify:** `backend/src/upload/job.rs`、`backend/src/upload/session_finalizer.rs`、`backend/src/routes/uploads.rs`、`backend/src/ingest.rs`、`backend/src/ingest/archive/budget.rs`、`backend/src/ingest/archive/zip.rs`、`backend/src/ingest/archive/tar_gz.rs`、`backend/src/ingest/archive/gzip.rs`、`backend/src/ingest/archive/seven_z.rs`、`backend/src/ingest/indexing/line_reader.rs`、`backend/src/search/rebuild.rs`、`backend/src/search/tantivy/mod.rs`、`backend/src/search/tantivy/pipeline.rs`、`backend/src/search/tantivy/publication.rs`。

- [ ] 先写可控 barrier 测试：排队取消释放 receive budget；运行中取消先终止 writer 再清目录；已交付 session 不重复创建 bundle；runtime 拒绝提交仍能收回 UploadJob 输入。
- [ ] 改造 `spawn_upload_job` 返回提交结果，业务输入仅在 admission 成功后移入 worker。multipart 路径拒绝时走既有 failed finalizer/temp cleanup；session 路径按交付点保留重试或既有失败策略，不能误记 delivered。
- [ ] 在 processing semaphore 等待中选择 permit/stop/deadline；获得 permit 后标 Running。将 process_result 的失败传播给 supervisor，finalize/cleanup 完成后再结束 Upload。
- [ ] 将 context 贯穿 preflight、archive 和 ingest，检查每个读块/entry/索引批次；使用 reader wrapper 或库提供的 callback 传播停止，不在同步解压代码中执行 async。
- [ ] 为上传内部索引阶段和独立 rebuild 登记 Index；SQLite 索引也登记。父任务与子任务不重复抢 processing/writer permit。
- [ ] 改造 blocking worker ownership：task guard、permit、lease 移入 closure；channel close 使 writer 退出；父任务 abort 不会伪造 Index 已结束。rebuild heartbeat 用受监督生命周期收尾。
- [ ] 保留 publication claim、PUBLISHING、READY/FAILED 和 generation fencing；取消发生在提交前走现有 abort/失败恢复，提交开始后完成短发布阶段。
- [ ] 写“阻塞 worker 尚未退出”的测试：async supervisor abort 后 active Index/permit 仍保留，放开 barrier 后才归零；不测试强制杀死线程。
- [ ] 运行 `cd backend && cargo test --test job_runtime_upload --test upload_sessions --test search_publication --test indexing_metrics`；再运行 `cargo test --no-default-features --test job_runtime_upload --test upload_sessions`，预期预算/恢复/READY 行为不变。
- [ ] 独立提交建议：`refactor: supervise upload and indexing lifecycles`。

### Task 4：Search 与 Materialize 接入并保留外部协议

**Create:** `backend/tests/job_runtime_search.rs`。
**Modify:** `backend/src/services/search_execution.rs`、`backend/src/routes/search_requests.rs`、`backend/src/routes/temp_results/service.rs`、`backend/src/routes/temp_results/search_plan.rs`、`backend/src/routes/temp_results/lifecycle.rs`、`backend/src/services/temp_results.rs`、`backend/src/routes/logs.rs`、`backend/src/search/mod.rs`、`backend/src/search/sqlite.rs`、`backend/src/search/tantivy/mod.rs`、`backend/src/search/tantivy/query.rs`。

- [ ] 先扩展现有 `backend/tests/search_execution.rs`：capability/owner 校验不变、Reserved 取消不启动任务、duplicate cancel 幂等、Committing 返回 finishing、timeout 错误码不变。
- [ ] Search registry 继续维护预约容量、60 秒 reservation TTL、鉴权和 terminal receipt；只有真正 start 才登记 Search。底层 token/deadline 改为 runtime context，删除第二套执行计时和取消真源。
- [ ] runtime 取消适配到 Search registry 的状态转换；handler drop/cancel API/shutdown 共用一个请求停止路径。Committing 的互斥边界必须覆盖 runtime cancellation 的裁决，不能只是复制 token。
- [ ] 将 preview worker 改用 supervised submit；先从 HttpRequest 提取可 Send 数据，不把 Actix response/request 放进 Tokio worker。保持 handler drop 触发取消而 worker 完成 staging cleanup。
- [ ] 无 search_id 的 legacy preview 也创建内部 Search job，不新增 capability 要求；full-result 创建 Materialize job；搜索里的物化是继承同一 deadline 的 Materialize 子任务。
- [ ] 接入 logs 中实际执行内容查询的入口，保留分页/line-read 等短请求的原资源策略；不把所有 HTTP 请求都包装成后台 job。
- [ ] 保留 materialization try-acquire、per-client lease 和 temp budget。取消不会变成 raw-scan fallback；保留 `abort_staging_result`、ACTIVE publication 和首屏失败后的清理分支。
- [ ] 使用 barrier 测试 cancel-before-commit 与 commit-before-cancel；legacy preview/full result 在 shutdown 中可见；查询 permit 和 blocking worker 均退出后才终态。
- [ ] 运行 `cd backend && cargo test --test job_runtime_search --test search_execution --test search_backend_parity` 以及 `cargo test --no-default-features --test job_runtime_search --test search_execution`，预期外部状态码和业务结果不变。
- [ ] 独立提交建议：`refactor: unify search and materialization execution control`。

### Task 5：统一 Shutdown 与服务退出

**Create:** `backend/src/job_runtime/shutdown.rs`、`backend/tests/job_runtime_shutdown.rs`。
**Modify:** `backend/src/main.rs`、`backend/src/job_runtime/mod.rs`。

- [ ] 先写确定性测试：空 runtime、Pending、合作式任务、拒绝停止的 async future、活跃 blocking closure、重复 shutdown、提交竞态和 server bind 失败。
- [ ] 实现 close/cancel/drain/abort/join 的有界流程，返回显式 shutdown report；不持锁等待，不以 drop JoinHandle 代替取消，不把 abort 请求当作已退出。
- [ ] main 关闭 Actix 默认信号处理，统一处理 Unix SIGINT/SIGTERM 与 Windows Ctrl-C；保留日志 guard 至 shutdown summary 输出之后。
- [ ] server 启动前保存可用于 shutdown 的 AppState/runtime clone；server error 和信号路径调用同一个协调器。禁止等待 `server.await` 后才开始取消任务。
- [ ] 在 server stop 与 job drain 中共用退出 deadline；停止周期任务后允许已接受 Upload/Index/Search 的必要清理继续，处理 session finalizer 交付与关闭的竞态。
- [ ] 移除 `background_tasks` vector 和末尾直接 abort 循环；完成常规退出后确认 registry/worker 数归零，异常 blocking 场景明确列出 outstanding。
- [ ] 运行 `cd backend && cargo test --test job_runtime_shutdown` 和 `cargo test --bin backend`；补充真实子进程 SIGTERM 验证，检查退出日志和重启 recovery。Windows signal 分支至少编译检查；无 Windows 环境时记录未验证。
- [ ] 独立提交建议：`feat: coordinate graceful job and HTTP shutdown`。

### Task 6：可观测性、文档及最终回归

**Create:** `backend/tests/job_runtime_metrics.rs`。
**Modify:** `backend/src/job_runtime/metrics.rs`、`backend/src/routes/admin.rs`、`backend/tests/admin.rs`、`docs/configuration.md`。

- [ ] 用可控时钟验证 active/queued 变化、成功和失败计数、panic、cancel/timeout 分开计数、last_success_at、终态淘汰后累计不丢失。
- [ ] 复用 `backend/tests/indexing_metrics.rs` 的 tracing capture 方法验证 completed/failed 事件，不输出 capability、表达式或原文；长耗时使用单调时钟，last_success 使用 UTC。
- [ ] 管理员响应加入聚合 `jobs`，验证非管理员无法读取、runtime_plan 缺失时仍可读取、旧字段保持原值。不新增前端依赖。
- [ ] 文档说明状态、指标定义、进程重启归零、默认 shutdown grace、blocking 限制和业务恢复归属。
- [ ] 审计 `rg -n 'tokio::spawn|spawn_blocking|spawn_periodic_job' backend/src`：每个长期根任务有 runtime owner；剩余 scoped parallel/query helper 有明确父任务和 join/drop 回收；测试用 spawn 不计入迁移。
- [ ] 执行以下命令，记录实际结果；本计划编写阶段不运行测试、不声称实现已验证。

```bash
cd backend
cargo fmt --check
cargo test
cargo test --no-default-features
cargo clippy --all-targets --all-features -- -D warnings
```

- [ ] 预期两种 feature 模式测试通过，格式与 lint 通过；若发现基线已有失败，记录复现和差异，不能把既有失败误报为本次通过。
- [ ] 独立提交建议：`feat: expose job runtime metrics and document lifecycle`。

## 验收映射与执行顺序

| Issue 验收 | 对应任务 | 必要证据 |
| --- | --- | --- |
| Upload/Index/Search/Cleanup 可注册 | 1–4 | 各适配器测试实际入口，含 SQLite 与 Tantivy |
| Materialize 生命周期覆盖 | 4 | legacy preview/full 和 Search 子物化测试 |
| shutdown 统一管理 | 2、3、5 | 关闭竞态、grace 超时、signal、blocking outstanding |
| cancellation 一致 | 1、3、4 | 相同停止机制、资源真实释放、提交竞态、清理完成后终态 |
| task failure 可观测 | 1、2、6 | Err/panic/abort 指标与日志，周期下一轮仍可执行 |
| 不改变业务逻辑 | 2–6 | 原有权限、busy、timeout、publication、upload/session、recovery 回归 |

按 Task 1 → 2 → 3 → 4 → 5 → 6 顺序实施，每个任务独立评审；中间可提交但只有全部验收完成后才视为 #273 完成。Task 3 和 Task 4 风险最高，应分别检查资源所有权及持久化提交边界。

实现过程中如果某个第三方阻塞调用没有合作取消点，保留明确 outstanding 和恢复语义，不引入线程强杀或放宽资源计数；是否增加进程级强退策略另开需求讨论。
