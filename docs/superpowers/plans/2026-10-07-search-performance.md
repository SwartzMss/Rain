# Issue 来源内存与 Tantivy 查询优化计划

日期：2026-10-07  
基线：main `2eb0033e13bf8c4973a1097675f49705cc84d98e`（v0.1.10）  
状态：首版已实施；本文保留剩余基线、压测与可选索引升级工作。

## 目标与范围

优先处理扫描报告中的两项搜索风险：

1. Issue 详细搜索一次性保留全部来源元数据，内存随文件数增长。
2. Tantivy 对候选读取完整 stored document，分页命中重复读取，部分过滤发生在文档读取之后。

批量删除、前端 Bundle 加载、逐行输出缓冲、依赖升级留给独立任务。按命中文件批量查行偏移是来源分批化的配套改动，可在同一个 PR 完成。

本轮不改变对外 HTTP API、关键词/表达式语义、结果排序、精确总数、临时结果有效期或账户权限，不重新引入“超过 N 个文件便不搜索”的产品限制。实现使用任务内的有界 keyset 批次，不引入磁盘 manifest；因此来源枚举不是跨全任务的数据库快照，消费时仍依赖可见性与存储状态复核。

## 已确认的事实与测量边界

- [resolve_sources_inner](../../../backend/src/routes/temp_results/service.rs) 从 SQL cursor 逐行读取，但同时填充 sources、indexed_sources、deferred_files 三个集合；读取 SQL 是流式的，消费路径仍是全量物化。
- [search_plan.rs](../../../backend/src/routes/temp_results/search_plan.rs) 还构造 pending、Bundle 分组、source_plans；Bundle 内为每个命中文件串行查询最近行偏移。只删除一个 Vec 不能解决整体增长。
- Preview 与 Full 共用来源解析；Full 当前走 raw 扫描。单文件和 source_temp_id 搜索也使用现有 executor，改造不能遗漏这些入口。
- [query.rs](../../../backend/src/search/tantivy/query.rs) 已将 file_id 条件推入查询；visibility、timeline、path 在读取 document 后判断，正文小写化与精确匹配在这些过滤通过后执行。不能把所有过滤都描述为“完全没有下推”。
- 当前 [schema.rs](../../../backend/src/search/tantivy/schema.rs) 的 file_id 已 indexed；path、timeline 仅 stored。直接用后两者构造 TermQuery 不可行。
- include_content=false 只控制输出正文，不取消正确性所需的正文验证。
- 2026-10-07 Windows release 冒烟：16 MiB、199,714 行、1 个 Bundle，入库至 READY 约 350 ms；常见词搜索约 4.8～5.0 ms、稀有词预览约 43～45 ms。每查询只有 2 个计时样本且缓存未清空，不能据此认定生产性能、尾延迟或 OOM 已复现。
- 当前基准在 Windows 没有 RSS 数据，且原资源采样主要覆盖 ingest；必须补充搜索阶段测量后才能评估来源内存风险。

## 不可退让的正确性约束

- ngram 候选可能是假阳性，精确子字符串验证必须保留；Unicode 大小写行为必须与现有实现一致。
- 总数仍为完整匹配计数，不能在当前页收集完成后提前结束。
- 保持有界排序堆，不为所有候选或深分页窗口缓存正文。
- 结果保留现有主排序规则：来源按 Bundle 创建时间、文件路径，文件内部按行。相同主键的顺序此前未定义，改造时明确增加 Bundle id、file id 作稳定平局键，并记录这项收敛行为。
- 不持有 SQLite writer admission 执行文件扫描、索引搜索或磁盘 manifest 写入。
- 可见性校验、generation lease、临时结果读 lease、配额、取消、超时、失败清理和 STAGING → ACTIVE 发布必须贯穿全链路；失败不能发布不完整成功结果。
- 多请求共享的 reader cache 或进程缓存不能成为隐藏的无界内存集合。

## 工作包 A：建立可比较的基线

涉及：现有 large_log_benchmark、搜索 metrics、补充多文件 fixture 与资源采样。

- [ ] 在独立临时数据库中构建 1 千 / 1 万 / 10 万个小文件的 Issue；分别覆盖单大 Bundle 与多 Bundle。复用 blob 字节可以隔离元数据成本，但还要用不同内容 fixture 复验，避免 dedup 掩盖实际开销。
- [ ] 测量 Preview / Full、低命中 / 高命中 / 无命中、raw fallback 和嵌套临时结果；单请求与并发 2 请求分别采样。
- [ ] 补充 search 阶段峰值内存、来源/计划活跃数量与估算字节、数据库查询数、各阶段时间、manifest 字节和 WAL 增长；Windows 用进程内存采样，Linux 同时观察 RSS/PSS。
- [ ] Tantivy 记录 candidate_docs、stored_doc_reads、exact_hits、max_retained_hits，并增加输出正文读取、过滤拒绝数量与归一化正文分配量的可观察指标。区分逻辑 document 读取次数和真实磁盘 I/O。
- [ ] 在 release 模式、固定数据/机器/配置下保存基线 JSONL；至少 3 次独立运行、每类查询 100 个计时样本，预热与冷启动分开报告。编译和大规模 fixture 生成不混入查询测量。
- [ ] 给结果正确性保存可比较摘要：总数、命中顺序、源身份、行号、正文与截断标记。

产出：可复现的基线命令、原始报告和待优化成本分布。没有基线收益证据的分支不作为默认路径落地。

## 工作包 B：让 Issue 搜索分批消费来源

涉及：routes/temp_results/service.rs、search_plan.rs、services/temp_results.rs；必要时新增来源枚举模块和 repository 查询。

### B1. 有界来源快照

采用稳定排序的 keyset 分页 + 有界读取批次，避免 OFFSET 越深越慢，也避免一次性保留全量 FileRow。磁盘 manifest 仍是后续需要一致性快照时的备选方案。

- [x] 按 Bundle 创建时间、文件路径、文件 id 做稳定 keyset 分页，每批最多 256 个来源；只保留当前批次的搜索/路径元数据。
- [x] 来源数通过 count 查询获得，label 与每批来源按需构造；不再隐式 collect 全量 FileRow、IndexedSource 或 DeferredSourcePath。
- [x] 每批消费前重新解析当前批次路径，并沿用 blob 状态、可见性、lease、取消、超时与失败清理约束。
- [ ] 若后续需要跨全任务的一致来源快照，再单独引入有界磁盘 manifest，并补充空间预算、WAL 与重启回收验证。

### B2. 增量计划和物化

- [x] 将 executor 拆成“初始化输出与累计状态 → 消费一批来源 → 完成/发布”；总匹配数、页面位置、log/meta 偏移、稀疏 checkpoint 和配额累计跨批次保持连续。
- [x] Preview 只对当前有界批次构建 source plans；Full 继续使用 raw 路径但分批枚举；直接文件与 source_temp_id 保持原路径。
- [x] 单个 Bundle 也按当前批次的 file_id 集合约束 Tantivy 候选查询，不以 Bundle 分组替代有界处理。
- [x] 候选窗口溢出仍安全回退 raw；批次结果通过共享物化状态累计，未重置总数、分页位置或输出索引。
- [x] 当前批内合并行偏移查找为单次 SQLite 查询，消除命中文件逐个串行查询。
- [x] 来源枚举、计划、路径解析和物化都运行在同一可取消/超时生命周期内。
- [ ] 对 SQL 查询做 EXPLAIN 检查，确认所需排序不会变成新增全表重复扫描；只有必要时新增索引和 migration。

B 的验收：来源相关活跃内存受批次预算限制；10 万文件仍完整处理，不能因文件数越界拒绝或静默漏搜；物化输出与基线一致，所有失败注入点都能回收资源。

## 工作包 C：降低 Tantivy 无效与重复读取

涉及：search/tantivy/query.rs、mod.rs、对应单元测试与 metrics；第一版保持索引格式 v1。

### C1. 无格式变更的低风险改动

- [x] 区分候选验证需要的 metadata 与响应需要的 content；include_content=false 复用已读取的 metadata，不再为当前页重复 load_hit。
- [x] include_content=true 仍只加载最终页正文，不把深分页窗口正文常驻在堆中。
- [x] 对有限批次 visible_file_ids 使用 indexed file_id 查询约束，并保留可见性复核；批次上限避免无界 BooleanQuery。
- [x] 精确匹配增加 ASCII 专用无分配忽略大小写路径；非 ASCII 保留现有 Unicode to_lowercase 语义。
- [x] 候选扫描与输出读取阶段继续检查取消/超时，并补充 metadata-only stored read 回归测试。

C1 的验收：include_content=false 路径不因输出当前页再读 document；ASCII 正文验证不再为每个候选创建完整小写副本；所有过滤/总数/排序/摘要回归一致；分页保留内存不退回与全部候选数成比例。

### C2. 有证据才实施的索引格式升级

path / timeline 目前没有索引或 fast fields，不能承诺无重建即可下推。若 C1 后的测量确认 stored document 解码仍是主要成本，再单独提出格式升级 PR：

- [ ] 选择必要 metadata fast fields / 精确过滤字段，估算额外索引空间、构建时间和 writer 内存。
- [ ] 明确 path substring 过滤语义，不能用精确 term 匹配替换现有 contains。
- [ ] 增加格式版本、旧 artifact 兼容读取或明确重建路径；重建用现有 generation 发布机制，完成前旧索引仍可读。
- [ ] 覆盖重建失败、重启、查询与删除并发、混合版本及回退。禁止原地覆盖正在被 lease 使用的索引。
- [ ] 若收益不足以抵消索引体积和迁移成本，保留 C1，记录 C2 不采用的实验依据。

## 功能回归与性能门槛

| 场景 | 必须验证 |
| --- | --- |
| 中文、ASCII、大小写、重复 gram、假阳性 | 精确命中与旧实现一致 |
| file / Issue / 临时结果内搜索 | 来源、总数、分页、排序和摘要一致 |
| 大量小文件、单个超大 Bundle、同时间/同路径 | 无漏项、无重复，稳定平局排序，受控批次内存 |
| 空候选、窗口上界、深分页、候选溢出 | 保留精确 total 与 raw fallback 正确性 |
| 并发上传/删除/索引重建 | 可见性、generation lease 和源文件保护有效 |
| 取消/超时/磁盘满/重启 | manifest、输出、permit、lease 均可回收，无部分 ACTIVE 结果 |
| 默认与 no-default-features | 构建及适用测试通过 |

结构性门槛：来源/计划驻留集合有明确条数与字节边界，metadata-only 输出不重复 load_hit，候选扫描不增加无界缓存。

建议的性能验收目标（实施前基线校准，不是已取得的收益）：10 万文件来源阶段峰值分配较基线减少至少 50%；针对性候选查询 p95 改善至少 20%；小 Issue p95 回退不超过 10%。若未达目标，先分析原始样本和主要成本，不通过减少搜索范围、取消精确计数或放宽一致性凑指标。总进程 RSS 还受 reader cache / mmap 影响，须与来源分配指标分开解释。

## 提交顺序、验证与交付

1. **PR 1：基线与指标。** 多文件 workload、查询阶段资源采样、正确性对照；无需业务行为变化。
2. **PR 2：Tantivy C1。** 依赖 PR 1；范围小，可单独对照收益与回退。
3. **PR 3：Issue 来源分批。** 依赖 PR 1；B1/B2 同时闭环，配套生命周期、批次边界及并发回归，不只替换容器。
4. **可选 PR 4：索引格式优化。** 必须有 C2 收益和迁移证据，独立评审。

每个业务改动运行对应 Rust 测试、fmt/check/clippy，最终运行全量默认与 no-default-features 测试、SQLite writer 审计和前端现有检查。端到端抽查 Issue 搜索、详细搜索、结果内搜索及分享查询链恢复。

交付每个 PR 的改动范围、功能对照、同机性能原始报告、内存/延迟对比和已知限制。无格式变更的改动可回退代码；涉及格式升级时严格使用预先验证的兼容/恢复路径，不假设旧程序可直接读取新 artifact。
