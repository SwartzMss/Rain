# Issue #184：Runtime Adaptive Engine 设计

状态：首期实现设计，已在本分支落地；动态热调和基准系数校准仍属于后续工作。

基线：2026-09-24 拉取的 `origin/main`，提交 `6a4f7df4aa8ddabaa329d24917e045f8c1032412`，已合入 #185 / #182 配置 UX。需求：[Issue #184](https://github.com/SwartzMss/Rain/issues/184)。

## 1. 设计结论

#184 建设后端底层能力：读取部署资源约束，把管理员的 Auto / Manual 意图解析为一组一致、可解释、可以真正应用的资源参数。管理员页面只是这个能力的消费者。

第一阶段采用启动时自适应：启动时探测和计算，构造 runtime 后发布实际 effective；运行中保存新的资源配置只生成 candidate，等待用户重启。不自动重启、不周期性改写管理员配置、不在本阶段动态缩放 semaphore 或重建 writer。

启动时自适应已经覆盖不同部署环境的默认性能目标。基于实时负载的闭环调整是第二阶段能力，必须有单独的指标验证与执行器设计，不能仅加一个定时计算器就声称已支持。

## 2. main 的事实与接入约束

以下路径均相对于上述基线提交。

| 当前实现 | 设计约束 |
| --- | --- |
| `backend/src/settings/model.rs` 已有 `SettingsSnapshot { revision, configured, effective }`，值均为具体类型 | 保留现有数字字段兼容性，通过独立 modes 表达 Auto；不要用 0 或 null 冒充 Auto |
| `settings/service.rs` 对 writer、upload processing 等使用 restart-required；保存有 revision 检查和审计 | 新引擎沿用保存事务及审计，不另设一套配置写入入口 |
| `main.rs` 先 bootstrap initialize、把 configured 应用到 config，构建 AppState 后再次 initialize | 启动必须改为一次解析、一次 runtime 构造、一次实际状态发布，避免第二次 initialize 覆盖有效值 |
| `lib.rs` 构建 `UploadRuntime`、`SearchRuntime` 和 semaphore | 资源计算必须发生在这些对象构造之前 |
| `search/resource.rs` 的 writer admission 限制同时运行的 writer 实例，查询并发常量为 4 | 明确 writer 实例数与 writer 内部线程数的区别；查询并发需要成为正式设置 |
| `search/parallel.rs` 单次 Issue 搜索最多并行 2 个 bundle | 保留每请求上限，同时受全局查询额度约束 |
| `search/tantivy/writer.rs` 使用 `index.writer(heap_size_bytes)` | 当前并未显式指定 writer 内部线程数；不能把 max_writers=4 描述成 4 个线程 |
| `search/tantivy/pipeline.rs` permit 随 blocking writer 持有；rebuild 也获取 writer 预算 | 保留任务真实生命周期的 admission，不让 HTTP 请求取消提前归还仍在使用的资源 |
| #182 metadata 已有 category、visibility、recommended range | 复用分类；静态推荐范围不等于本机算出的推荐值，更不等于硬校验 |

## 3. 首批范围

| 参数 | 含义 | v1 生效方式 |
| --- | --- | --- |
| `search_tantivy_max_writers` | 进程内同时活跃的 bundle writer 实例上限 | 重启 |
| `search_tantivy_writer_heap_size` | 每个 writer 的 heap budget，非全进程预算 | 重启 |
| `upload_concurrent_processing_tasks` | 上传后的处理任务并发，包含归档与索引处理链 | 重启 |
| 全局 Tantivy 查询并发（由 runtime plan 派生） | 全进程 Tantivy bundle 查询任务并发 | 重启 |

不新增“archive workers”线程池：先复用当前 processing admission，避免与上传处理并发相乘。上传接收并发、前端上传队列并发和服务端处理并发是不同参数，本期只调整最后一项。

单次 Issue 的并行度派生为 `min(2, effective_query_limit)`，不是独立用户配置。一个 bundle 查询只获取一次全局 permit，单 bundle 与多 bundle 路径共用同一预算。SQLite FTS 后端不套用 Tantivy 查询预算；API 标注 Tantivy 参数 `applicable=false`，处理并发仍适用。

归档防爆、Issue 配额、行大小、搜索结果窗口、临时文件磁盘额度、认证限制均不参加 Auto。`archive_max_working_size` 不能当作实际 RSS 或可分配内存。

## 4. 分层与数据流

建议新建 `backend/src/runtime_tuning/`：

| 模块 | 职责 |
| --- | --- |
| `model.rs` | ResourceSnapshot、PolicyVersion、ResolvedPlan、AppliedRuntimeSnapshot、DecisionReason |
| `probe.rs` | 平台资源探测；以 trait 支持测试注入 |
| `policy.rs` | 纯函数：配置意图 + 资源快照 + 后端类型 → 完整候选计划 |
| `validation.rs` | 类型、跨字段约束、预算估算、建议警告 |
| `service.rs` | 协调候选解析与实际状态发布，不负责持久化用户配置 |

现有 `settings` 继续拥有持久化、revision、审计、字段 metadata。各业务 runtime 继续拥有任务执行和 permit。引擎不依赖 HTTP，也不直接操作 SQL 或业务任务。

数据流：

```text
DB 配置意图 + 资源探测快照 + policy_version
                    ↓
            纯函数 resolve + validate
                    ↓
               candidate plan
                    ↓ 启动时构造各 runtime 成功
               applied snapshot
                    ↓
             API effective + 原因
```

不可把 resolve 成功等同于 applied 成功。

## 5. 配置意图、候选值、实际值

保留现有 `configured` 数值映射，新增稀疏的 `modes` 映射，仅允许首批四个 key，取值 `auto | manual`。

- `configured[key]`：管理员保存的数值，Auto 时仅为保留的手工值/兼容值。
- `modes[key]`：真正决定如何解释 configured。
- `candidate[key]`：当前配置在本进程启动资源快照下解析出的目标值。
- `effective[key]`：当前 runtime 真正使用的数值。
- `decisions[key]`：candidate 与 effective 各自的来源、原因、policy_version 和资源快照 ID。
- `pending_restart_fields`：本期资源字段中 candidate 与 effective 不同的集合。

逻辑上的 configured 是 `(mode, stored_value)`，不能再只比较 configured 数字与 effective 来推导是否待重启。Auto → Manual 切换若目标数字不变，可以无待重启字段，但保存 revision 和当前决策原因必须更新，实际资源数值仍保持一致。

引入 `runtime_generation` 和进程实例 ID，独立于管理员配置 `revision`。资源探测、计算推荐值、读取状态都不能递增配置 revision，也不能修改 updated_by。effective 的解释保留它实际应用时的输入，不能被最新 candidate 的理由覆盖。

## 6. 资源探测契约

v1 决策输入只使用 CPU 容量与可用给部署的内存上限。其他信号预留接口，不影响首期计算。

- CPU：考虑系统可用逻辑 CPU、进程 affinity/cpuset，以及容器 CPU quota。取有效约束中的最小值；保留小数 quota，计算任务数时向下取整且至少为 1。
- Memory：取物理内存和实际进程/容器限制中的最小有限值。容器限制要定位当前进程所属层级，考虑祖先约束；不能固定读取假定的根 cgroup 路径。
- Linux：覆盖 cgroup v1/v2、无限制标记、缺失/权限失败、非标准挂载。Windows：读取物理内存、进程 affinity 和可获得的 Job 限制；未知限制显式标记。
- 每个探测值含 `source`、`confidence`、`observed_at`、`warnings`；unknown 不等于无限资源。
- `memory_available` 属于波动观测，不直接作为固定 heap 的基数。宿主机空闲内存也不保证容器内可用。
- 探测失败使用保守 fallback，并显示降级原因。已知任一资源约束仍必须参与计算，不能因另一项未知而丢弃它。

存储类型在 RAID、虚拟机、网络卷下难以可靠判定，本期不做启动 I/O benchmark、不递归扫描索引。索引规模和运行负载放到后续阶段，避免启动代价和未经验证的系数进入生产策略。

## 7. 初始策略 v1：确定性、联合计算、可解释

下面是待基准验证的初始策略，不是已验证的最优参数。采用 MiB/GiB 二进制单位；计算使用检查溢出的整数运算。

令 C 为有效 CPU 任务容量（至少 1），M 为有效内存上限：

```text
P0 = clamp(ceil(C / 2), 1, 8)       // 上传处理任务
W0 = clamp(floor(C / 4), 1, 4)      // writer 实例
H0 = clamp(floor_to_16MiB(M / 64), 16MiB, 256MiB)
Q0 = clamp(C, 1, 16)               // 全局 Tantivy 查询任务
B  = M / 4                        // 本期可调资源的规划预算
E  = P × 32MiB + W × (H + 32MiB) + Q × 32MiB
```

E 中的 32MiB 是首轮容量规划占位估算，需要混合负载测量后校准。H 是 writer budget，E 不是 RSS 硬上限；merge、mmap/page cache、数据库、解压缓冲、HTTP 和其他任务仍会消耗资源，余下 75% 不承诺一定足够。Auto 是保守默认值能力，不提供 OOM 保证。

算法顺序：

1. 先固定所有 Manual 值；只为 Auto 字段生成初值。
2. Tantivy 未启用时，预算排除 writer/query 项。
3. 若 E > B，依次降低 Auto 的 H（每次 16MiB，最低 16MiB）、W（最低 1）、P（最低 1）、Q（最低 1），每步重新检查，首次满足即停止。
4. Manual 字段从不被静默裁剪。Auto 上限只约束 Auto，不替代原有硬校验范围。
5. 降到最小值仍超预算：保留合法最小计划并返回 `estimated_budget_exceeded`，明确资源不足；不把零并发作为合法降级。
6. 内存未知时 H 从 16MiB 起步，其余 Auto 并发保守取 1；CPU 未知时 C=1。完全未知使用 P=1、W=1、H=16MiB、Q=1，并标记 fallback。不能显示“已验证内存安全”。

Manual 超出估算预算采用建议警告，不增加隐式的硬件相关拒绝规则，避免旧部署升级后无法启动；类型、现有硬上下界和平台表示范围仍是硬校验。32 位转换、乘法总预算、semaphore 可表示上限必须显式检查。

全 Auto 示例，假设 Tantivy 启用且探测完整：

| C / M | P | W | H / writer | Q | E / B |
| --- | ---: | ---: | ---: | ---: | --- |
| 1 / 512MiB | 1 | 1 | 16MiB | 1 | 112 / 128MiB |
| 2 / 2GiB | 1 | 1 | 32MiB | 2 | 160 / 512MiB |
| 8 / 16GiB | 4 | 2 | 256MiB | 8 | 960 / 4096MiB |
| 16 / 64GiB | 8 | 4 | 256MiB | 16 | 1920 / 16384MiB |

内存压力示例：C=16、M=1GiB，初值 E=960MiB、B=256MiB；按规则降低 W 至 1、P 至 1、Q 至 5 后 E=240MiB。这个退让顺序优先保留在线查询容量，需要通过上传与搜索混合负载验证。

writer 的内部线程数和 merge 线程是后续性能验证必查项；本期不新增内部线程的 Auto 参数，因此 P+W+Q 不是 CPU 线程总数上限。若基准发现内部并行导致明显超订阅，必须在策略定版前降低实例上限或另行设计显式线程预算。

## 8. 启动、保存与读取

### 启动

1. DB migration；识别新安装/已有配置；初始化数值与 modes。
2. 加载持久化 configured 和 modes，不提前认定它们就是 effective。
3. 探测一次资源并冻结为本进程启动快照。
4. resolve 得到完整计划，并做现有字段与新字段校验。
5. 用计划中的具体数值构造 UploadRuntime / SearchRuntime 等对象。
6. 统一发布 applied snapshot，再开放相关服务；AppState 使用同一个 SettingsService 实例。

删除原有“第二次 initialize 把 DB 值当 effective”的路径；这里描述的是未来实现要求，本次不改代码。部分构造失败时不发布部分成功的 effective，不启动对外处理。

### 保存

沿用 `PATCH /api/admin/settings` 的 expected_revision 和同一个保存锁。校验数值和 modes → 完整解析 candidate → 数据库事务原子保存 configured、modes、审计和 revision → 发布新的配置视图。

首批资源字段全部等待重启，旧 effective 原样保留。其他现有 Hot 字段继续既有行为，并按最终组合验证，避免两套快照冲突。保存不能临时再用宿主机的另一次探测结果制造不一致。

计算 `pending_restart_fields` 由一个公共函数负责，替换 service 与 admin route 中重复的字段比较逻辑；metadata 的 ApplyMode 也从同一注册表取得。

### 读取

GET 只读状态，不重新计算或应用资源。当前 `settings.load()` 需要确保不会通过 reload 把 candidate 当作 effective。不得因为打开管理员页面改变实际并发或配置 revision。

## 9. API 与 UI

保留 endpoint 和现有数值映射，响应提升为 schema_version=3，增加 modes、candidate、runtime、decisions。metadata 增加 supports_auto；既有 category/visibility 保留。

示意响应片段（省略其他字段）：

```json
{
  "schema_version": 3,
  "revision": "12",
  "configured": { "search_tantivy_max_writers": 1 },
  "modes": { "search_tantivy_max_writers": "auto" },
  "candidate": { "search_tantivy_max_writers": 4 },
  "effective": { "search_tantivy_max_writers": 1 },
  "pending_restart_fields": ["search_tantivy_max_writers"],
  "runtime": { "generation": "1", "policy_version": "v1" }
}
```

保存形态：`{ expected_revision, changes, modes }`。只改 modes 也是合法请求；同事务应用，revision 冲突仍返回 409。未知 key、非 adaptive key 的 mode、未知 mode 和非法数值返回 422。

兼容规则：旧客户端显式提交 adaptive 数字且未带对应 mode 时，视为选择 Manual；只更新其他字段不改变 modes。新客户端切 Auto 发送 mode，保留手工数字；切 Manual 未提供新数字时使用存储值，UI 必须先显示该值，不能让用户误以为会沿用 effective。仅更新 mode 的请求需要调整现有“changes 不得为空”校验。

页面延续 #182 的分类：每个支持 Auto 的字段显示模式；Auto 下显示当前有效值、待重启目标值（如有）、简短原因，手工输入隐藏或禁用。原因来自后端稳定 reason code + 参数，前端负责中文文案。Manual 显示输入值与预算警告。Auto 卡片不能把保留的 configured 数字当作当前使用值。

v1 不新增独立运行监控页面。资源快照只向管理员暴露，与现有 settings 权限一致，保持 private/no-store。

## 10. 持久化与升级

新增 migration，编号取实现时 main 的下一个空闲号，不能写死为当前假定编号。

- 给 system_settings 增加 `adaptive_modes_json`；查询并发是 runtime plan 的派生值，不增加持久化列。
- JSON 只存允许的 modes，数值继续使用原有 typed columns；不把整个 settings 改成无约束 JSON。
- migration 中已有行：现存三个目标字段按 Manual 解释；新增 query 字段为 Manual=4，保持旧常量行为。即便现存值恰好等于旧默认，也不能据此猜测用户没有手工配置。
- 新安装：四个目标字段默认 Auto；首次初始化显式提供的旧 ENV 对应字段转 Manual。配置解析层需要保留“是否显式设置”的 provenance，不能只看最终数值判断。
- 安装状态依据初始化前的持久化状态，在旧 bootstrap 插入行前识别；不能因先执行 INSERT 而把新安装误判为升级。
- 数字列与 modes、revision、审计在同一事务更新。Auto 计算结果不写回数字列。
- 老实例可由管理员一次将四字段切 Auto；升级不能自动替换用户意图。
- 降级到旧二进制会忽略 modes 并使用存储数字，可能与当前 Auto 结果不同；回退流程先把期望数值保存为 Manual 并备份数据库，不承诺无感降级。

## 11. 可观测性与后续闭环边界

启动记录一次资源来源、policy_version、每字段决策理由和估算预算。保存记录管理员意图变更与待重启字段。探测失败不输出含主机敏感路径的细节给普通用户。

第二阶段可采集 queue wait、active/queued writer、上传处理耗时、查询延迟、RSS 与资源压力，以影子模式比较建议值。真正热调前必须满足：

- 降并发只阻止新任务进入，让持有 permit 的任务完成；不能简单换一个 semaphore 留下双重额度。
- heap 变更仅对新 writer 生效；旧、新 writer 同时存在时仍按实际预算核算。
- 定义采样窗口、迟滞、冷却和恢复规则，防止震荡；负载反馈不能改变 Manual 意图。
- 锁顺序及 admission 顺序固定，上传等待 writer 不得与全局预算形成循环等待。
- 任务取消时额度跟随实际 blocking 工作结束，不能跟随请求结束。

这些是未来热调的进入条件，本期不实现控制回路。

## 12. 验证与交付拆分

建议按依赖顺序实现：

1. 引擎纯函数、资源探测契约、固定输入测试；此阶段不改变实际默认值。
2. 配置 modes、迁移、revision/审计、candidate/effective 模型与统一 apply 注册表。
3. 启动一次性接线，writer/upload/query 三条消费路径改用 resolved plan。
4. API 与 #182 页面接入；完成混合负载基准后定版 policy v1。

必须覆盖的行为测试：

| 类别 | 验证点 |
| --- | --- |
| 纯函数 | 相同输入相同结果；上表样例；低内存退让；单位/溢出；Auto 范围；Manual 不被覆盖 |
| 探测 | cgroup v1/v2、祖先限制、quota<1、Windows 受限进程、读取失败、未知来源 |
| 升级 | 老值等于默认仍保留 Manual；新安装 Auto；显式 ENV Manual；初始化重入不改变 modes |
| 保存 | modes-only、混合修改、非法模式、旧客户端数字写入、revision 冲突、事务失败不发布候选状态 |
| 生命周期 | GET 无副作用；保存后 effective 不变；重启后才应用；再次 initialize 不覆盖实际快照 |
| 并发 | query 全局与每请求同时受限；上传与重建共享 writer admission；取消任务不提前释放额度 |
| 前端 | Auto/Manual、候选/实际值、待重启、fallback 原因、#182 分组与保存权限回归 |

性能验证用相同输入集比较 main 固定默认值与 Auto：受限 1CPU/512MiB、2CPU/2GiB、8CPU/16GiB、16CPU/64GiB，覆盖纯上传、纯搜索、混合负载、并发重建。采集吞吐、查询 p95/p99、峰值 RSS、writer 排队、失败率。资源受限环境不得在代表性负载出现新增 OOM；大机器混合负载若搜索明显恶化，收紧并发策略，不以吞吐单指标验收。具体量化回归阈值应由先测得的 main 基线确定并在实现 PR 固定。

验收映射：Auto 支持由 modes + resolver 完成；configured/effective 由 API/UI 展示；默认无需理解底层参数由新安装 Auto 完成；不同硬件默认值由资源探测、确定性策略及基准验证完成；对应测试覆盖纯函数、升级和实际执行路径。实时 workload/storage/index-size 调参明确属于后续阶段。
