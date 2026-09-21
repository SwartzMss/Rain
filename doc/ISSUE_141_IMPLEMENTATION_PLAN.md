# Issue #141：SQLite writer admission 覆盖方案

状态：实现设计，尚未修改业务代码。

依据：2026-09-21 的 main `74c6e86`、Issue #141，以及 #56 的开放 PR #145。

目标是在现有 `db::write::{run, acquire}` 上补齐后台、批量、恢复、删除路径的写入协调，保持 Schema、部署配置和上传并发配置不变。不新增任务队列、不扩大 busy timeout、不支持多实例共写数据库。

## 1. 现状与边界

- `db/write.rs` 已按数据库文件共享进程内 FIFO 写入资格；先拿资格，再从连接池取连接。
- `run` 包含事务、rollback 和有界 SQLITE_BUSY 重试；只适用于可安全重放的纯数据库操作。
- 索引、Quota、上传状态、Blob 大部分写入、Bundle 分批删除、Skill 历史清理已经接入。
- `db.rs` 中删除前后的状态更新、Issue 租约和删除事务、部分恢复及定期清理仍直接写数据库。
- Blob GC 有明确的 DB + 文件删除耦合，已用 `acquire` 和手工事务；这里必须保留现有互斥范围。
- readiness 探针必须回滚，保留 `acquire` + 手工事务，不改成自动提交的 `run`。

保留部分短前台直写意味着本方案减少并统一高风险写竞争，不能承诺任意负载和外部 writer 下永不出现 SQLITE_BUSY。

## 2. 覆盖矩阵

| 路径 | 实施方式 | 边界 |
| --- | --- | --- |
| `db.rs::cleanup_expired_bundles` | 每个 Bundle 的 DELETING claim 使用 `run` | claim 内重新检查状态及过期条件，成功后才启动清理；不锁整个循环 |
| `finish_bundle_deletion` | 最后的 DELETED / quota 清零更新使用 `run` | 保留现有分批删除和 cleanup semaphore |
| `finish_bundle_deletion_with_inactive_lease` | 带租约的每批删除及最终状态更新，在同一事务内验证/续租 | 防止等待 admission 后继续使用过期 token |
| `renew_inactive_issue_lease` | 对外 pool 包装使用 `run`；新增接收 connection 的内部 helper | 内部 helper 不再次申请 admission |
| `fail_stale_processing_bundles` / `_before` | 使用 `run` | 保留状态条件和原启动 cutoff，不重算 cutoff |
| `routes/issues.rs` manual/inactive claim、legacy manual 状态归一、retry 调度 | 每次状态转换使用 `run` | token、状态、有效期条件保留在 SQL 中；失败不能静默吞掉 |
| `routes/issues.rs` Bundle 删除入口及 Issue 删除入口/撤销 | 使用 `run` | 包含与清理相关的前台状态转换，但不把后台清理包进请求事务 |
| `finish_auto_issue_deletion` / `finish_manual_issue_deletion` | 最终删除事务整体使用 `run` | saved searches、Issue、自动删除审计日志保持原子性 |
| `services/file_deletion.rs::delete_file_tree` | 现有纯 DB 事务整体使用 `run` | 删除文件树和重算容量原子提交；异步化及拆批属于 #142 |
| `repositories/sessions.rs::cleanup_expired_or_revoked` | 使用 `run`，按有限批次删除 | 每批释放资格，保留总删除行数返回值 |
| `repositories/skill_runs.rs` 后台运行状态/进度/步骤/完成/失败、两种 recovery | 使用 `run` | 模型调用、工具执行、结果计算保持在事务外；已有过期清理直接复用 |
| `routes/temp_results/repository.rs` staging/publish/删除 claim/final delete | 纯 DB 状态转换使用 `run` | 特别包含 `UPDATE ... RETURNING`，不能只搜索 `.execute()` |
| `blob_store.rs` / upload / ingest / quota / health 已接入路径 | 审计调用关系并保留现有覆盖 | 不重复套锁；文件副作用不得移入 `run` |
| Schema 初始化、migration、bootstrap settings | 交由 #56 及启动顺序约束 | migration 完成后才启动 recovery / 后台任务 |
| 账户、普通配置、收藏搜索、活动时间等短前台操作 | 本次可保留直接写 | 审计表记录具体函数与理由；发现批量或后台调用时重新分类 |

以上是确定的改造范围。实施前补齐全仓生产写路径清单，排除测试 fixture；SQL 审计同时覆盖 execute、fetch_optional、fetch_one、query builder 和触发器引发的级联写入。审计结果放入 `doc/DB.md`，每个直写保留项需要说明理由。

## 3. 事务与锁的约束

通常的锁顺序：

```text
业务任务 / 必要的 cleanup semaphore
  -> writer admission
    -> pool connection / transaction
      -> DB 状态检查和写入
    -> commit 或 rollback
  -> 释放 writer admission
-> 下一批 / 文件处理 / 其他计算
```

- 禁止取得 connection/transaction 后再等待 admission。
- 禁止在 `write::run` 闭包中调用另一个接收 pool 的 `run/acquire` helper，否则会嵌套等待自身。
- 复用同一事务的 helper 接收 `&mut SqliteConnection`，只能使用调用方提供的连接。
- 禁止持有 admission 等待 heavy cleanup semaphore；现有排队续租完成后应释放 admission，再继续等待 cleanup semaphore。
- 每个删除批次独立拿资格并提交，不能用一个资格包住整个大 Bundle。
- retry backoff 在释放 admission 后进行，继续沿用现有次数与时长策略。
- UUID、审计内容、序列化、解析在进入闭包前生成；闭包只执行 DB 操作并返回值，提交后再更新内存统计或触发任务。
- 不对完整业务函数添加统一 retry。可重放的是本次 DB 事务，不是已经发生过文件操作或网络调用的流程。

## 4. 租约必须在执行时有效

现有路径多次采用“续租 -> 等待/执行删除 -> 再续租”。接入 admission 后，仅在排队前续租不够：等待期间 token 可能失效或被其他 worker 接管。

建议增加 `renew_inactive_issue_lease_on(conn, ...) -> Result<bool, AppError>`，保留现有对外 pool 接口供无事务调用方使用。

带租约的每批删除、Bundle 完成更新及 Issue 最终删除，采用以下事务边界：

```text
write::run
  -> 验证并续租：code + DELETING + reason + token + 尚未过期
  -> 如 rows_affected != 1，返回非重试的 lease-lost 错误
  -> 执行本批写入或最终状态转换
  -> commit
```

旧 token 不能通过续租复活；租约丢失时终止当前清理，交给现有 claim/recovery 接管。事务内检查发生在真正得到执行机会之后；不保证任意长时间外部阻塞下租约永不失效。

最终 Issue 删除中，删除 saved searches、删除 Issue、写自动过期审计应处于同一事务。必须防止把原有“rollback 后返回 false”机械改成闭包 `Ok(false)`，从而提交前面的部分写入。需要回滚的分支使用内部错误，待 `run` 完成回滚后再映射成原接口语义。

## 5. 文件副作用的两种处理方式

临时结果已有持久 DELETING 状态，继续采用：

```text
run: claim DELETING 并提交
  -> 释放 writer，删除临时文件
  -> run: 删除对应 DELETING 记录
```

重试最终 DB 删除不会自动重放文件删除；如果文件已删但 DB 操作失败，保留 DELETING 状态供后续清理恢复。已有 read/staging lease 规则保持不变。

Blob GC 的引用检查与文件删除目前必须在同一互斥范围内，继续采用 `acquire + 手工事务`。不能为了缩短占锁时间直接把 `store.delete` 移到锁外，否则会与文件引用发布竞争。若要消除此处文件 I/O 占锁，需要后续 Blob 状态机设计，属于 #135。

## 6. 测试设计

并发测试使用独立临时文件数据库、WAL、多连接或同文件多 pool；不依赖单连接内存库来证明并发安全。使用 Barrier/Notify/oneshot 建立确定的交错，timeout 只作失败保护。

| 测试 | 必须证明 |
| --- | --- |
| 实际上传/索引 + Bundle 删除/过期清理 + recovery | 存在执行重叠；上传达到 READY，目标清理完成，FTS/引用/容量一致 |
| recovery cutoff 隔离 | cutoff 前的残留任务被处理，之后的新上传不被误标失败；固定跨秒 fixture，避免时间精度偶然性 |
| admission 等待不占连接 | 持有资格期间启动真实 cleanup/recovery 写函数，另一请求仍可借用唯一空闲连接执行 SELECT |
| 事务中途 SQLITE_BUSY | 在首条写成功后注入真实 BUSY 错误，确认完整 rollback 后重放；审计只一条、retry 次数只增一次、容量不重复修改 |
| 非 BUSY / 约束 / lease lost | 不自动重放，不留下部分提交；资格与连接释放 |
| 等待期间租约过期/被接管 | 确认旧任务已经等待；让 token 失效后放行；不得继续删除或写审计，新 worker 能恢复 |
| waiting/事务取消 | 取消等待或执行任务不会泄漏资格或连接，不留下部分事务；后续任务可完成 |
| 文件副作用边界 | DB-only 重试不再调用文件删除；文件删除成功但最终 DB 失败可由现有状态恢复 |
| file-tree 删除 + 索引 | 保留级联删除/FTS/容量断言；不会因此次接入破坏行为，但不宣称解决长事务延迟 |
| 临时结果、会话和 Skill 清理并发 | 状态转换和删除计数正确；每批释放 admission，活跃数据不被误删 |

复用 `db/write.rs` 已有跨 pool、BUSY rollback、约束错误、重试耗尽测试；新增真实业务路径测试，不能只再次证明 mutex 会互斥。优先使用现有 fixture；确需故障注入时只添加 `cfg(test)` 窄入口。

已有并发测试不能通过改为串行运行来掩盖失败。最终运行 `cargo test`、`cargo clippy -- -D warnings` 和 `cargo fmt --check`。

## 7. 可观测性与交付顺序

沿用 `run` 的 operation、queue_ms、elapsed_ms、attempt、retry 日志，给新增事务稳定且可区分的 operation 名称。外层错误日志附带 Issue/Bundle/任务 ID。特别补齐 `schedule_manual_retry` 当前静默吞错误的情况。不新增部署配置或监控系统。

建议在一个 #141 PR 内按可审查提交拆分：

1. DB/Bundle recovery 与 cleanup 收口，加入 connection 级租约 helper。
2. Issue 删除、claim/renew/retry、最终审计事务接入并补租约测试。
3. 临时结果、会话、Skill 后台写入、同步 file-tree 删除接入。
4. 并发和故障回归测试、`doc/DB.md` 覆盖清单、完整检查。

与 #56 并行时保留 `prepare_schema(pool, reset)` 接口；#141 不改 baseline SQL、migration 校验或 Schema 初始化块。#145 合入后同步 main，在 migration 创建的数据库上运行完整测试。两者没有 Schema 依赖，但 `db.rs` 和测试区域可能需要手工整合。

完成标准：上述覆盖矩阵已逐项落实或明确解释保留原因；单个事务获得一致的 writer admission；业务原子性、租约归属和文件副作用边界均有测试；全部检查通过。未改造短前台直写、外部 writer、Blob 必要文件 I/O 和同步大树删除仍是明确的剩余约束。
