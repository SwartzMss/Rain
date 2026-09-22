# Large Log Search Evolution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. 每个 PR 独立验证；不要把整个路线图合并成一个实现批次。

**Goal:** 缩短大日志从上传完成到 READY 的等待时间，降低并发索引对查询和元数据操作的干扰，同时保持旧数据可用和现有搜索语义。

**Architecture:** SQLite 保留控制元数据与短事务，CAS 保留原文，通过 SearchIndex 接口接入 SQLite FTS5 与按 Bundle 发布的 Tantivy 索引。单个大文件使用有界生产消费流水线；新索引完成发布后才允许 Bundle READY。先观测和保持行为的抽象，再上线可选后端，最后进行渐进迁移。

**Tech Stack:** Rust / Actix Web / Tokio / SQLx SQLite migrations / SQLite FTS5 / Tantivy（在原型 PR 验证兼容版本并锁定 Cargo.lock）。

---

## 计划状态与边界

- 来源：[Issue #153](https://github.com/SwartzMss/Rain/issues/153)。代码核对基于本地 `f55c154`；开始执行前核对 main 与当前迁移编号。
- 本文按独立提交执行；进度记录见文末。性能数字是建议验收门槛，不是已有测量或收益承诺。
- 用户已明确大文件处理是优先问题。暂按“上传结束后 PROCESSING / INDEXING 等待久”设计，真实文件规模、机器配置和卡顿阶段未确认；基准默认覆盖 100 MiB、1 GiB、5 GiB。
- 第一阶段交付 PR0、PR1；第二阶段 PR2、PR3 形成可选的新文件完整处理链路；PR4 验证并调整并发，达标后再切默认。PR5、PR6 是后续独立交付。
- 本轮不实现分布式、多实例共享索引、PostgreSQL、后台索引合并、CAS snippet 去正文副本。已有归档安全限制、Issue 配额、删除规则继续有效。

## 方案选择

| 方案 | 适用与代价 | 结论 |
| --- | --- | --- |
| 继续调整 SQLite 批次与参数 | 变更小，可能改善单文件耗时，但全文索引仍争用同一个 writer | 作为 PR0 后有证据的小优化，不作为长期方案 |
| SQLite 控制面 + Bundle Tantivy + 有界流水线 | 保持本地部署，隔离索引写入；需要发布、恢复和混合查询 | 推荐，符合 #153 |
| 直接按 Issue 构建可变索引 | 减少跨 Bundle 查询次数，但并发发布、文件删除和重建更复杂 | 留待 100 Bundle 基准证明必要后单独评估 |

## 已核实的约束

1. `backend/src/ingest.rs::ingest_text_file` 逐批读取、清洗、构建 chunk，然后等待 `commit_index_batch`；同一文件解析与提交当前不会重叠。只加 Bundle 并发不能保证一个大文件提速。
2. `backend/src/ingest/limits.rs` 当前 chunk 目标 256 KiB / 200 行，提交目标 1 MiB / 5,000 行，原文行偏移每 1,000 行记录一次。第一轮保留这些边界，避免搜索迁移与 chunk 语义变化混杂。
3. `backend/src/db/write.rs::run` 已记录 admission `queue_ms` 和事务包装段 `elapsed_ms`；后者包括取得连接和开始事务的成本，不能直接当纯 SQL 时间。
4. `backend/src/routes/logs.rs` HTTP 搜索要求至少 3 字符，返回 total 与分页，并按行偏移、segment ID 排序；`backend/src/services/skill_tools.rs` 允许带 file_id 的 2 字符查询，FTS 按 rank 排序。不要在 PR1 统一排序或短词策略。
5. 时间列名称虽含 `_ms`，实际为 wall-clock 比较键，不是 Unix UTC 时间戳；保留未索引时间的 coverage 行为。
6. `clean_log_line` 会 trim、清除 NUL、解码并标记截断。索引文本与 CAS 原始字节并非一一对应。行偏移仍指原文。
7. 文件允许异步删除；`visible_files` 决定可见性。Bundle 索引不可变不等于所有文件永久可见。
8. `repositories/files.rs` 使用 `log_line_offsets` 读取行；`routes/temp_results/service.rs` 使用 `EXISTS(log_segments)` 判断日志内容。这两个依赖必须迁移或明确保留。
9. `upload/finalizer.rs` 当前更新 Bundle READY；`main.rs` 启动时会恢复/标记旧 PROCESSING 任务。新 publication 恢复必须与此顺序协调，不能先把可恢复的新索引当失败任务清掉。

## 性能基线与切换门槛

基准数据必须流式生成，固定 seed，记录原始字节和清洗后字节。所有运行使用 release build、相同机器、相同输入、独立 data_root。默认不覆盖真实数据目录。

| 维度 | 必须覆盖 |
| --- | --- |
| 单个大文件 | 100 MiB、1 GiB、5 GiB 原文；超长单行；非 UTF-8、CRLF、NUL、空行 |
| 归档 | 相同解压内容的 zip / tar.gz，遵守现有配额与压缩比限制 |
| 上传并发 | 1、2、4 Bundle；2 / 4 / 8 核资源限制，受机器实际能力约束 |
| 查询 | 单 Bundle；10 / 100 Bundle Issue；常见词、罕见词、UUID、中文、符号、短词、时间范围 |
| 共存负载 | 上传期间持续搜索、轻量元数据读写、文件删除 |
| 存储 | 本机 SSD；NVMe 有硬件时独立运行，不用 SSD 结果冒充 NVMe |

每轮记录 commit、配置、CPU、RAM、磁盘、OS、缓存状态、输入 hash；预热 1 次，计时至少 5 次上传，查询至少 1,000 次。输出每次原始结果与中位数，查询给出 p50/p95/p99；不从 5 次上传样本推导可信 p99。

指标：receive、processing queue、extract/CAS、parse active、index queue blocked、index build/commit/publish、upload complete → READY；SQLite admission、connection/begin、transaction/commit；原文 MiB/s、lines/s、RSS peak、CPU、I/O bytes、DB/WAL peak、search artifact bytes。嵌套计时不相加冒充总时间；并行阶段同时记录 wall time 和累计工作时间。

默认后端切换建议门槛：

- correctness、删除、恢复、混合后端测试全部通过；不接受静默漏结果。
- 如果 PR0 证明索引是主要成本，1 GiB 和 5 GiB 单文件 processing 中位耗时目标降低至少 30%；4 Bundle 总吞吐目标至少 1.5 倍。未达标就保留 opt-in，依据阶段数据调整，不能调小数据集掩盖回归。
- 同负载常用查询 p95 不超过 SQLite 基线 1.2 倍；100 Bundle、常见词和冷缓存分别报告。
- writer 内存、队列和并发数有硬限制；RSS 不随文件大小线性增长，RSS 总上限依据部署内存设置。writer memory 不代表整个进程 RSS。
- 索引+DB（排除两边相同的 CAS）目标不超过基线 1.2 倍；超出时维持 opt-in 并记录空间与速度取舍。
- 若 PR0 发现解压/CAS 占主要耗时，在 PR0 报告后先追加针对实测瓶颈的小 PR；保留本路线图，但不把 Tantivy 当作已证实的单文件解法。

## PR0：可重复大文件基线与观测

**修改：** `backend/src/db/write.rs`、`backend/src/ingest.rs`、`backend/src/upload/job.rs`、`backend/src/upload/finalizer.rs`、`backend/src/routes/logs.rs`、`backend/src/services/skill_tools.rs`。

**新增：** `backend/src/ingest/metrics.rs`、`backend/tests/indexing_metrics.rs`、`backend/tests/large_log_benchmark.rs`、`backend/tests/support/large_log_fixture.rs`、`docs/performance/large-log-baseline.md`。

- [ ] 从现有 `backend/tests/smoke.rs` 复用上传、搜索、清理 fixture 方式，为 benchmark 使用独立临时根目录；大测试标记 ignored，CI 不自动生成 GiB 数据。
- [ ] 编写 metrics 测试：文件字节与行数对应输入；失败/取消也记录终态；同一次重试不重复累计逻辑 indexed bytes；queue 与实际写入耗时分开。
- [ ] 将 parse active、等待提交、CAS/解压阶段计时分开，保持现有事务结构和 limits。逐批只累加计数，文件结束输出汇总，避免每行日志开销。
- [ ] writer 使用独立计时覆盖 admission、pool/begin、execute/commit；保留已有 operation、attempt 和错误字段。不得将 bundle/file ID 作为无限增长的常驻指标维度。
- [ ] 实现 ignored 基准：环境变量 `RAIN_BENCH_BYTES`、`RAIN_BENCH_CONCURRENCY`、`RAIN_BENCH_REPORT`，默认 100 MiB / 1；输出 JSONL 原始样本，归档解压大小单独记录。
- [ ] 先执行 100 MiB 验证工具，再执行 1 GiB / 5 GiB 与并发矩阵；超出当前配额时显式记录基准配置变更，不改生产默认。
- [ ] baseline 文档写入阶段占比、最慢阶段、RSS、DB/WAL 与查询延迟；根据数据确定后续优先级。

验证（仓库根目录）：

```bash
cargo test --manifest-path backend/Cargo.toml --test indexing_metrics
RAIN_BENCH_BYTES=104857600 RAIN_BENCH_CONCURRENCY=1 RAIN_BENCH_REPORT=/tmp/rain-baseline.jsonl cargo test --manifest-path backend/Cargo.toml --release --test large_log_benchmark -- --ignored --nocapture --test-threads=1
```

预期：计数测试通过，benchmark 生成可重复报告；本 PR 不要求业务性能提升。提交主题：`perf: establish large log indexing baseline`。

## PR1：搜索与索引写入边界，保留 SQLite 行为

**新增：** `backend/src/search/mod.rs`、`backend/src/search/contract.rs`、`backend/src/search/sqlite.rs`、`backend/src/search/router.rs`、`backend/src/ingest/indexing/chunk.rs`、`backend/tests/search_contract.rs`。

**修改：** `backend/src/lib.rs`、`backend/src/ingest.rs`、`backend/src/routes/logs.rs`、`backend/src/services/skill_tools.rs`。

- [ ] 先记录 HTTP Bundle、HTTP Issue、skill 三种入口的实际 JSON 契约；fixture 覆盖 total、from/size、snippet、truncated、time_index_coverage、timeline、path_like 与 literal path_prefix 的区别。
- [ ] 把现有 LogChunk 及 batch 数据移到 indexing/chunk.rs，保持 chunk 边界和排序 ID；仅搬移，不重写清洗器。
- [ ] 建立 owned 请求/结果类型，分别表示 scope、caller policy、filters、pagination、排序、时间 coverage；显式区分“精确总数”与“截断候选”，不能复用同一个无说明的 count。
- [ ] 建立对象安全 async 查询接口和每个 Bundle 的 build session：有界 batch 写入、完成文件、完成 Bundle、放弃构建；batch 同时携带 chunk、原文 offsets 和最终 line count。生命周期由协调层驱动，backend 不自行设 READY。
- [ ] Sqlite 实现复用现有 SQL 和 `db::write::run`；route 保留鉴权、入参验证及 activity touch，skill 保留调用去重/ledger；FTS SQL 收口到 search/sqlite.rs。
- [ ] 增加兼容测试：两字符 HTTP 拒绝；两字符 skill 无 file_id 拒绝；同文件短词可用；时间未索引提示；删除文件不出现；分页、大小写、中文、引号和通配符行为保持。
- [ ] 在相同 fixture 上比较抽象前后完整响应，并重跑 PR0，确认没有把整个文件加载到内存或扩大 writer 临界区。

```bash
cargo test --manifest-path backend/Cargo.toml --test search_contract
cargo test --manifest-path backend/Cargo.toml --test skill_tools
cargo test --manifest-path backend/Cargo.toml --test smoke
rg -n 'log_segments_fts' backend/src/routes backend/src/services/skill_tools.rs
```

预期：测试通过，最后的 rg 无生产搜索 SQL 命中（无匹配 exit 1 正常）。不增加新 schema，不改变默认行为。提交主题：`refactor: encapsulate SQLite search and indexing contracts`。

## PR2：Tantivy 原型与单文件有界流水线

**新增：** `backend/src/search/tantivy/{mod,schema,tokenizer,writer,query}.rs`、`backend/src/search/admission.rs`、`backend/src/ingest/indexing/pipeline.rs`、`backend/tests/search_backend_parity.rs`、`backend/tests/indexing_pipeline.rs`。

**修改：** `backend/Cargo.toml`、`backend/Cargo.lock`、`backend/src/search/mod.rs`、`backend/src/ingest.rs`、`backend/src/config.rs`、`backend/.env.example`。

- [ ] 依照官方 API 核对 Rust/平台支持，添加可选 `tantivy-search` feature，更新 lockfile；先只从测试/benchmark 构建新索引，不把不完整原型接入真实上传。
- [ ] 固定 schema/format/tokenizer 三个版本号。索引字段：searchable cleaned text；file_id、chunk_index、行范围、原文 byte range、wall-clock 时间范围与 indexed 标记；path/timeline 按过滤需求建立索引；bounded cleaned content 存储用于 snippet 和候选复核。
- [ ] Ngram 仅作为候选产生器，不能用 trigram AND 直接认定连续 substring 命中。加入 `abc ... bcd` 不匹配 `abcd`、重复 gram、Unicode 大小写与跨 chunk 测试；统一字符规范化必须以 SQLite parity fixture 为依据。
- [ ] 候选精确复核前不得截取最终 top-N；HTTP 精确 total 必须遍历并核实全部候选。超时明确返回错误，不能伪造 total 或假装完整成功。长查询使用去重后有界 gram 子集生成候选，再复核完整字符串，避免 Boolean clause 无限制增长。
- [ ] 2 字符 skill 路径保持 file scope。现有 file scope 不等于扫描成本有界；为新旧 backend 同时加入显式 candidate/bytes/time budget，沿用 truncated 字段并增加可解释原因（作为独立可审阅行为变化）。一字符继续拒绝，不做全库 fallback。
- [ ] 同一文件仅一个顺序 reader，维护准确行号/字节数；通过字节预算与有界 batch channel 连接索引 worker。同步 Tantivy 工作放 blocking worker，不能阻塞 Tokio executor。
- [ ] 流水线初始候选配置：2 个 pending batch、每 batch 约 1 MiB；硬上限包含超长单行与 chunk 元数据。writer 初始预算 64 MiB、2 worker threads；这些是实验起点，须通过库限制与 RSS 基准校验后才进入配置默认值。
- [ ] 入队前获得 byte permits，消费或取消时释放；后台 writer 报错关闭 channel，reader 停止；reader 报错取消 writer。失败不 publish，不悬挂上传任务，所有 permits 回收。
- [ ] 初始保留原文稀疏 offsets 在 SQLite，并单独计量该成本；Tantivy 路径不写 `log_segments.content`。文件检索能力 metadata 在 PR3 落地后才接入业务。
- [ ] 比较 SQLite 与 Tantivy 的单文件 1 GiB / 5 GiB 和峰值 RSS；若解析主导，再用数据决定是否并行清洗，不在本 PR 将文件按字节任意切分。

```bash
cargo test --manifest-path backend/Cargo.toml --features tantivy-search --test search_backend_parity
cargo test --manifest-path backend/Cargo.toml --features tantivy-search --test indexing_pipeline
```

预期：包含早停、错误、取消、队列饱和的测试全部通过；单文件内存受配置约束。性能仍由报告证明。提交主题：`feat: prototype bounded Tantivy bundle indexing`。

## PR3：新 Bundle 可选后端、持久发布与恢复

**新增：** `backend/migrations/0003_search_indexes.sql`（执行前确认编号未占用）、`backend/src/search/{publication,recovery,visibility}.rs`、`backend/tests/search_publication.rs`、`backend/tests/search_mixed_backends.rs`。

**修改：** `backend/src/db/migrations.rs`、`backend/src/lib.rs`、`backend/src/main.rs`、`backend/src/config.rs`、`backend/src/upload/{job,finalizer,lifecycle}.rs`、`backend/src/routes/temp_results/service.rs`、`backend/src/services/file_deletion.rs`、`backend/src/db.rs`、`backend/.env.example`。

- [ ] migration 持久化 backend、三个版本号、generation、artifact key、index state、built_at；旧 Bundle 默认为 sqlite_fts。文件增加独立日志检索能力标记，回填只依据旧 log_segments 是否存在，替代 temp_results 的 SQL 存在性依赖。
- [ ] backend 在创建 Bundle 时固定，环境配置只决定新 Bundle。可选依赖关闭时拒绝选择 Tantivy；读到已有 Tantivy 数据也必须明确报不支持，不能悄悄查空 SQLite。
- [ ] 使用内部不可变 ID 构造路径 `search/<bundle-id>/<generation>/`；staging 与最终目录在同一文件系统。不要依赖可变 Issue code 或用户文件名构造 artifact 路径。
- [ ] build 完成 → commit → 关闭 writer → 验证可重新打开/文档数量 → 写 manifest 并执行平台支持的 durability 操作 → 发布 generation → SQLite 短事务绑定 generation 并 READY。目录 rename 的可见性与断电持久性分别测试/说明。
- [ ] 发布前、标记 READY 时再次验证 Issue ACTIVE、Bundle PROCESSING、generation 所属任务仍有效；与 Issue/Bundle 删除竞态通过条件更新胜负决定，不允许复活已删除任务。
- [ ] 恢复流程先检查新 publication 状态，再运行 legacy stale-processing 清理；逐个恢复，不能启动时同步全量重建。
- [ ] 完整验证发布成功但 DB 未绑定的 generation 后才继续 READY；无法验证则 FAILED 并排队清理。READY 缺失/损坏/未知版本索引标记 NEEDS_REBUILD，不返回空命中冒充成功。旧 SQLite 数据仍存在时可显式回退，否则报告暂不可搜。
- [ ] 文件删除先更新 metadata 可见性；查询取得受控可见性视图，过滤必须进入命中计数及分页前。删除等待已有查询 lease 结束后再物理释放 artifact；新请求必须排除已删除文件。不可变 artifact 留到 Bundle 删除或重建时回收，文档说明磁盘不立即下降。
- [ ] Issue 混合查询统一合并、排序、分页。HTTP 保留各 legacy Bundle 的原 segment 排序，在混合后端定义稳定 tie-break `(line_start, bundle_id, file_id, chunk_index)` 并单独记录语义变化；不得假设旧 SQLite rank 与独立 Tantivy score 可比较。
- [ ] skill 排序跨后端采用明确的 rank fusion 规则并建立 golden fixture；这是显式新排序版本，PR1 保留旧行为，PR3 对混合/新后端生效。time coverage 汇总包含所有可见 Bundle。
- [ ] 建立 reader 缓存最大项数和查询 lease；淘汰关闭句柄。Windows mmap/文件句柄占用时延迟幂等删除，进程重启后继续，不使用一次 rename 成功作为删除完成凭证。

恢复矩阵至少测试：temp+PROCESSING、published+PROCESSING、published+READY、无 artifact+READY、未知版本、DELETING+正在 publish、崩溃后多余 generation、migration 失败仍可搜旧数据。

```bash
cargo test --manifest-path backend/Cargo.toml --features tantivy-search --test search_publication
cargo test --manifest-path backend/Cargo.toml --features tantivy-search --test search_mixed_backends
```

预期：混合后端真实上传→READY→搜索→删除全链路可用；默认仍 sqlite。提交主题：`feat: publish and recover per-bundle search backends`。

## PR4：资源 admission 与性能验收

**修改：** `backend/src/search/admission.rs`、`backend/src/ingest/indexing/pipeline.rs`、`backend/src/upload/job.rs`、`backend/src/config.rs`、`backend/tests/large_log_benchmark.rs`。

**新增：** `backend/tests/indexing_resource_budget.rs`、`docs/performance/tantivy-comparison.md`。

- [ ] PR2/PR3 上线前已有最低并发和内存限制；本 PR 根据测量调整，不允许前几阶段无界运行。
- [ ] admission 同时约束 active Bundle writers、总 writer memory、总待处理 batch bytes、blocking worker 数；统一获取顺序，取消安全。分别控制前台 query fan-out、后台 build/GC I/O 并发，禁止一次启动 100 个查询 task。
- [ ] writer threads 与 Bundle 并发共同计入 CPU 预算；线程数不按每个 Bundle 都取机器总核数。避免 query 长期被后台任务占满。
- [ ] 报告是 CPU、磁盘、解析还是 writer queue 限制吞吐；对 1/2/4 上传并发完整比较，给低内存与多核机器独立配置示例。
- [ ] 所有验收达标后单独修改默认配置并补 release notes；不达标保留 opt-in，记录下一项由数据支持的优化。

预期：资源饱和时受控排队，不 OOM、不饥饿、不静默跳过文件；得到可复现的性能与空间对照。提交主题：`perf: budget parallel indexing and validate large log gains`。

## PR5：旧 Bundle 渐进 rebuild

**新增：** `backend/src/search/rebuild.rs`、`backend/tests/search_rebuild.rs`；通过下一独立 migration 持久化 rebuild job、lease、attempt、progress、错误、目标 generation。

- [ ] 从 CAS 和 files metadata 读取可见日志；复用同一 parser/schema，不依赖旧索引正文作为唯一来源。
- [ ] 默认后台并发 1，前台上传/查询优先；按 Bundle 恢复，进程中断可重新构建单个 Bundle，不能要求整批重跑。
- [ ] build 期间旧 backend 一直有效；校验文件集合版本/删除状态后条件切换 generation。冲突则重试，不能把删除文件重建为可见。
- [ ] CAS 缺失/损坏、磁盘满、未知格式等只使当前 rebuild 失败并保留旧查询；可观察 progress、错误和 retry。
- [ ] 切换后保留旧 FTS 兼容期；删除旧记录用 bounded batch，通过已有 writer admission。配置改回 sqlite 仅影响新 Bundle，不能充当已有 Tantivy Bundle 的回滚。
- [ ] 发布运维步骤：停止新任务、保留/备份 generation、重试、恢复旧 backend 的必要条件；backup 同时包含 DB、CAS 与引用的搜索 generation，或明确从 CAS 重建流程。

预期：无须重新上传；失败可重试；已切换/未切换 Bundle 混合可搜。提交主题：`feat: rebuild legacy bundle indexes incrementally`。

## PR6：独立退役 SQLite FTS

- [ ] 前置：新后端已默认、兼容期结束、legacy Bundle 为零或有明确处理清单、rebuild 和恢复演练通过。
- [ ] 再次扫描所有 log_segments/log_line_offsets 读者，包括 preview、临时结果、skill、清理和备份；只删除确认无依赖的数据。offsets 保留在 SQLite，除非另一份基准证明迁移 sidecar 必要。
- [ ] 独立 migration 移除 FTS triggers/table 与纯搜索正文；不修改历史 migration checksum。
- [ ] 大数据库回收空间作为单独维护步骤，明确磁盘余量和耗时；不在启动路径自动 VACUUM。
- [ ] 更新 README、目录格式、最低兼容版本、备份/恢复与升级说明。

## 每个代码 PR 的验证与交付

先写行为测试，确认新能力尚不存在时失败，再实现、跑针对性测试。搬移现有逻辑用 characterization 测试验证行为一致，不要求为纯搬移制造假失败。每个 PR 单独提交，提交中包含代码、测试、对应文档和实际执行结果。

现有基础检查：

```bash
cargo fmt --manifest-path backend/Cargo.toml --check
cargo check --manifest-path backend/Cargo.toml --locked
cargo clippy --manifest-path backend/Cargo.toml --locked -- -D warnings
cargo test --manifest-path backend/Cargo.toml --locked
```

引入 feature 后同样对 `--features tantivy-search` 执行 check/clippy/test，CI 保留 SQLite-only 与新后端两种配置。目录发布、mmap、删除和 recovery 增加 Windows 验证，不能仅依赖 Linux 测试。需要前端嵌入产物时按现有 CI 先执行 frontend 的 npm ci / npm run build。

PR0/PR1 是当前可先执行的工作包；后续 PR 开始前，以前一阶段报告和接口为基础补齐该 PR 的具体类型签名及测试实现，不把本文的建议配置直接当作已验证生产参数。

## 参考与自检

- [Issue #153](https://github.com/SwartzMss/Rain/issues/153)：整体架构与阶段要求。
- [Tantivy NgramTokenizer 官方文档](https://docs.rs/tantivy/latest/tantivy/tokenizer/struct.NgramTokenizer.html)：检索于 2026-09-22，页面版本 0.26.2；内置 ngram token position 均为 0，不能直接假定它提供与 SQLite 等价的 substring phrase 语义。依赖版本在 PR2 经验证后锁定。
- 覆盖核对：抽象 PR1；并行/内存 PR2+PR4；版本/发布/恢复/删除/混合查询 PR3；迁移 PR5；旧 FTS 退役 PR6；每个性能结论由 PR0/PR4 报告支持。
- 已知条件仍未确定：用户实际卡顿阶段、文件类型/大小、CPU/RAM/磁盘。这些影响基准选择和参数，不阻塞先补观测与契约测试。

## 执行记录

### 2026-09-22：PR0 实施中

- 工作分支 `feat/issue-153-search`，隔离目录 `.worktrees/issue-153-search`；基线 298 个库测试通过。
- 已接入按文件汇总的原文字节/行数、提交正文/批次数、读取解析与等待提交时间；writer 单独计量 admission、begin、execute、commit/rollback。
- CAS persist 与 verify/publish、归档解压、Bundle publish、HTTP/skill 搜索均使用终态计时；取消与错误分别记录。
- receipt→READY 从 multipart 完成开始，包含 reservation 写等待，在 READY 后清理前结束。
- 成功/失败计数集成测试通过；真实 SQLITE_BUSY 重试和等待 writer 时取消的回归测试通过。
- 完成独立需求审查与代码质量审查；剩余完整测试及 release 压测完成后记录实际结论。
- 扩展 `clippy --all-targets` 发现现有 `skill_runs.rs` 单元素循环、`services/file_reader.rs` 测试默认值赋值告警；保持本次范围，标准 CI 的 clippy 命令通过。

### 2026-09-22：PR1 搜索边界抽离

- 新增 `SearchIndex` owned 请求/结果契约和 SQLite FTS 实现；Bundle 内容、Issue 内容、文件名和 skill 搜索均通过适配器执行。
- 保留三类入口的原有策略：HTTP 内容搜索仍要求至少 3 个字符并返回精确分页总数，文件名搜索继续做转义后的大小写不敏感子串匹配，skill 搜索继续区分 FTS 与带 `file_id` 的两字符 literal，并保留时间范围 coverage。
- `commit_batch` 已实现为原子 SQLite 适配器并覆盖批次边界、完整 `RETURNING` 校验和部分插入回滚；当前 ingest 仍使用原有 `LogChunk` 写路径，避免在本 PR 同时改变生产写入内存所有权和临界区。LogChunk 搬移及生产切换留给有界流水线 PR。
- 适配器单元测试、305 个库测试（两个并发敏感测试串行重跑通过）、`skill_tools` 10 个、`smoke` 9 个和 `indexing_metrics` 2 个测试通过；`cargo fmt --check`、`cargo check --locked`、生产库 `clippy -D warnings` 通过。
- broad smoke 首次并发编译期间曾在 BINARY 小上传处超过固定 2 秒轮询窗口；随后独立完整 smoke 通过，未观察到搜索相关失败。

### 2026-09-22：PR2 Tantivy 原型与有界写入

- 添加可选 `tantivy-search` feature，锁定 Tantivy 0.26.2；默认构建和生产路由仍使用 SQLite FTS。
- 新增 per-Bundle schema、固定三元组 n-gram tokenizer、stored cleaned content 和文件/chunk/行/事件时间字段。
- 新增有界生产者/消费者管道：默认最多缓存 2 个 batch，writer 在 blocking pool 中运行并限制 64 MiB heap；发送端停止或 writer 失败时不会继续无限生产。
- 查询只用有限 n-gram 生成候选，再对 stored chunk 做连续不区分大小写复核；覆盖 `abcd` 不匹配 `abc ... bcd`、重复 n-gram 和中文。
- 新增 `search_backend_parity` feature 集成测试和原型文档；生产 ingest 接入、发布、恢复、删除可见性和混合查询在后续阶段补齐。
- 已通过 feature 下的原型测试与 `clippy --features tantivy-search --lib -D warnings`；尚未用真实大文件宣称性能收益。

### 2026-09-22：PR3 发布元数据基础

- 新增 `0003_search_indexes.sql`，为每个 Bundle 持久化 backend、schema/tokenizer 版本、generation、artifact key 和 publication state；迁移会给旧 Bundle 显式回填 `sqlite_fts/LEGACY`。
- 新 Bundle 创建时写入 `sqlite_fts/BUILDING`，legacy SQLite 完成后在同一 writer 事务把索引元数据置为 `READY`，Bundle 更新增加 publication-ready 条件，避免未来 backend 未发布时静默进入 READY。
- 新增只使用内部 Bundle id 与 generation 的 artifact 路径校验；Tantivy 选择、artifact 验证、崩溃恢复、删除 lease 和混合后端查询仍未接入。
- 串行 smoke 9 passed/1 ignored；迁移、READY 元数据、publication 路径和现有 skill/indexing 集成测试通过。组合并发 smoke 曾重现既有固定轮询/reader 争用抖动，单独串行重跑通过。

### 2026-09-22：PR3 Tantivy 单 Bundle 发布闭环

- 新增 `RAIN_SEARCH_BACKEND` 配置，默认 `sqlite_fts`；只有启用 `tantivy-search` feature 的构建才能选择 `tantivy`。
- 上传任务在处理开始时 claim 新 generation，从规范化 `log_segments` 以 64 chunk 批次送入有界 Tantivy pipeline；writer 完成后执行目录 rename、重新打开和文档数校验，再把 publication 标为 `READY`。
- Bundle finalizer 现在只接受 SQLite legacy/ready 或已发布的 Tantivy `READY` 元数据；Tantivy publication 未完成时不会把 Bundle 置为 `READY`。
- Bundle 内容搜索按 publication backend 路由到 Tantivy；Issue-wide 内容搜索已支持 SQLite/Tantivy 混合合并，已有 Bundle 继续走 SQLite。
- 新增 `search_publication` 集成测试，覆盖 generation claim、artifact publish/reopen、READY 元数据和 Bundle 查询；默认与 feature 构建均通过 check，Tantivy feature 库 clippy 通过。

### 2026-09-22：PR4 writer admission 第一阶段

- 新增 `RAIN_SEARCH_TANTIVY_MAX_WRITERS` 与 `RAIN_SEARCH_TANTIVY_WRITER_HEAP`，默认只允许一个 64 MiB writer；所有 Tantivy Bundle publication 共用 admission semaphore。
- Upload worker 在启动 Tantivy pipeline 前获取 writer permit，permit 覆盖 batch 消费、commit、reopen 校验和 publication，避免单任务有界但并发任务把 writer heap 线性叠加。
- writer 资源预算已覆盖真实上传；查询 fan-out 目前串行且磁盘 I/O admission、1/2/4 并发矩阵仍留在 PR4 后续验收。
- 新增 migration `0004_tantivy_skip_sqlite_fts`：Tantivy-owned Bundle 的 log segment 不再写 SQLite FTS5 shadow table，保留 legacy/SQLite Bundle 的 trigger 语义，消除双重索引写入的主要重复成本。

### 2026-09-22：PR4 流式接入与首轮性能对照

- 上传解析器通过 `IngestIndex` 把清洗后的 chunk 直接送入 Tantivy `BundleBuildSession`；不再先写完整 SQLite 正文再回读构建索引。
- SQLite 仅保留空正文兼容行、稀疏 offsets、行范围、事件时间和 line count；元数据按 512 chunk 批量提交，避免每个 chunk 争抢 writer。
- tokenizer 固定为 `rain_ngram_v2` 三元组，候选结果仍逐 chunk 做连续子串复核；旧的 2–20 配置已废弃并递增 tokenizer 版本。
- Issue 内容搜索已支持 SQLite/Tantivy 混合合并、稳定排序和全局分页；Bundle 路由校验 publication、schema 和 tokenizer 版本。
- 增加启动/定时清理 unpublished generation 和已删除 Bundle artifact，并补充真实 streaming publication 与“正文不落 SQLite”的集成测试。
- 一次 100 MiB release 对照：SQLite ingest→READY 约 11.874 s，Tantivy streaming 约 2.026 s；Tantivy RSS 采样峰值约 55.3 MiB。数字只代表单主机单次运行，默认后端仍不切换。
