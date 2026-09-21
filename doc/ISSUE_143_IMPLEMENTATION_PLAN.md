# Issue #143：统一 Issue 生命周期与自动清理豁免

状态：已按本方案实现，随 #143 PR 提交评审。

基准：2026-09-22，main `b771363`；#56/#145 与 #141/#146 已合入。

需求：[Issue #143](https://github.com/SwartzMss/Rain/issues/143)。相关：[Issue #142](https://github.com/SwartzMss/Rain/issues/142)。

## 1. 目标与实施决策

Issue 是自动生命周期的最小单元。活跃 Issue 下的所有历史 Bundle 均保留，只有整个 Issue 满足非活跃条件时才能由系统发起删除。

增加启动配置 `RAIN_CLEANUP_EXEMPT_USERS=alice,bob`，以 Issue owner 的 `users.username_normalized` 判断豁免。手工删除不受此名单限制。

本方案采用以下明确决策：

- 完全移除 Bundle 按创建时间自动过期的执行路径；旧环境变量只输出一次停用提示。
- 白名单在进程启动时解析，不支持运行中热更新；变更后需要重启。
- 保留非活跃天数现有来源：数据库 `system_settings` 为运行时依据，`RAIN_ISSUE_INACTIVE_DAYS` 仅作为首次初始化默认值。管理员仍可修改数据库中的天数。
- 不修改 activity 的刷新定义、所有者权限、上传并发参数或 Blob GC 规则。
- 不新增 Schema，不修改已发布的 `0001_initial.sql`；使用已有 owner、状态、删除原因和租约字段。
- 对已进入 INACTIVE 删除且新近被豁免的 Issue，停止继续自动删除，保留当前 DELETING 状态；不能推断部分删除是否已经发生，也不能自动恢复 ACTIVE。
- 允许 owner 显式手工删除上述暂停的 Issue：租约无效后，原子转换为 MANUAL 并使用现有手工删除流程。这是删除入口的状态兼容，不增加权限。

“永不自动删除”指符合豁免策略时不再发起或继续执行 Issue 非活跃删除，不是备份或回收站承诺。加入名单之前已经删除的数据无法恢复。

## 2. 当前代码与需要移除的路径

当前两条独立生命周期链路：

1. `config.rs::AppConfig.retention_days` 解析 `RAIN_RETENTION_DAYS`，`main.rs` 启动阶段运行 `expired-bundle-cleanup`，调用 `db.rs::cleanup_expired_bundles()`。
2. `routes/mod.rs::spawn_inactive_issue_cleanup()` 启动后立即执行一次，以后每小时执行 `routes/issues.rs::cleanup_inactive_issues()`。该函数先恢复已有 INACTIVE 删除，再选择 ACTIVE 的非活跃 Issue。

第二条包含启动后的首次执行和周期恢复；没有必要再新增一条独立的启动 inactive 清理实现。

需要删除：

- `AppConfig.retention_days` 字段及旧值的数值解析、校验、构造代码。
- `main.rs` 对 `cleanup_expired_bundles` 的导入与 `expired-bundle-cleanup` 阶段。
- `db.rs::cleanup_expired_bundles()` 及仅服务于该函数的行模型。
- smoke 测试中“旧 Bundle 自动过期”的断言；有关分批删除、恢复、FTS/容量正确性的覆盖改用显式删除路径保留。
- README、`.env.example` 中仍鼓励配置 Bundle retention 的说明。

必须保留：

- `resume_deleting_bundles()`、`finish_bundle_deletion*()` 以及周期 Bundle 删除恢复；手工删除和 Issue 整体删除仍需要它们。
- 临时上传清理、临时结果过期、会话清理、Skill 历史清理、无引用 Blob GC。它们不是活跃 Issue 的 Bundle retention。
- `bundles.created_at` 与相关业务索引，不因删除策略移除而删除业务数据字段。

历史 #141 方案中保留的旧函数引用标记为“已由 #143 移除”，不将历史规划当作现行配置说明。

## 3. 配置与共享策略

建议新建 `backend/src/services/issue_cleanup_policy.rs`，定义不可变 `IssueCleanupPolicy`：

```rust
pub struct IssueCleanupPolicy {
    exempt_usernames: HashSet<String>,
    exempt_usernames_json: String,
}
```

提供 `from_names(...)`、`is_exempt_normalized(...)`、`exempt_usernames_json()`；默认空集合。字段私有，避免 HashSet 与 SQL 参数不一致。JSON 使用排序后的用户名列表生成，便于测试和日志稳定。

解析流程：未配置或空字符串 → 空集合；按逗号拆分 → trim → 丢弃空项 → 复用 `auth::password::normalize_username` → 去重。

示例 ` Alice, BOB,alice,, ` 得到 `alice`、`bob`。当前 username normalization 是 ASCII 小写，不另建一套 Unicode/大小写规则。格式不符合现有 username 规则的非空项可 warning 后忽略，不导致服务启动失败。

配置中有效但当前不存在的用户名必须保留在策略里，可在启动时 warning 一次。若 `alice` 尚未注册，之后注册并创建 Issue，应自动获得豁免；不能启动时只解析成 user ID 后永久丢弃未知用户名。

`AppConfig` 接收解析后的名单/策略；`AppState` 增加 `Arc<IssueCleanupPolicy>`，现有测试构造器默认使用空策略。`main.rs` 在启动任何 inactive worker 之前注入配置，正常初始化与测试注入使用同一构造方法。

使用 `env::var_os("RAIN_RETENTION_DAYS").is_some()` 检测旧配置，在日志系统就绪后 warning 一次：旧配置已停用，改用 Issue 非活跃策略。旧值即便是负数或任意字符串，也不再按 retention 数值校验，不影响启动，不触发删除。

## 4. SQL 过滤和原子 claim

候选筛选、恢复筛选以及 claim 统一复用同一豁免谓词。建议使用已有 SQLite JSON 能力，以单个绑定参数传入名单：

```sql
AND NOT EXISTS (
    SELECT 1
    FROM users AS cleanup_owner
    JOIN json_each(?) AS exempt
      ON cleanup_owner.username_normalized = exempt.value
    WHERE cleanup_owner.id = issues.owner_user_id
)
```

这是设计示例；具体 alias 随查询调整。参数使用 `.bind(policy.exempt_usernames_json())`，不得拼接用户名生成 SQL。空名单绑定 `[]`，无需生成空 `IN ()` 或为每个名字增加 bind 参数。

该判断按 Issue owner，不按 Bundle uploader，也不按当前发起请求的用户。owner 为空或无法关联时保持现有非豁免语义；用户 disabled 不取消其豁免。

`cleanup_inactive_issues_with_lease()` 的两个 SELECT 都在 `ORDER BY ... LIMIT 20` 之前加入谓词：

- ACTIVE 候选：保留当前非活跃阈值、无 PENDING/PROCESSING Bundle 条件。
- INACTIVE 恢复：保留 claimed days、retry 到期、租约过期条件。

不能先 LIMIT 20 再在 Rust 中跳过，否则前 20 个都是豁免 owner 时，后续普通用户可能永久得不到清理。

`claim_inactive_issue()`、`claim_inactive_recovery()` 接收策略引用，在 `db::write::run` 内使用相同谓词执行条件 UPDATE，并检查 `rows_affected == 1`。候选列表只是提示，执行权必须由事务中的当前 owner、当前状态及租约条件决定。

不能将查询/策略检查放到 writer admission 之前后就认为已获授权。排队期间数据可能变化，实际执行事务仍应重新检查。

## 5. 执行与租约边界

`finish_auto_issue_deletion()` 接收策略，上层只允许已经成功 claim 的调用进入执行。自动 Bundle 状态转换、每个删除批次、Bundle 最终状态、Issue 最终删除都需要使用当前 INACTIVE 租约和策略进行保护。

建议为现有租约上下文增加自动/手动来源，或增加带策略的自动清理上下文：

- 自动上下文：Issue code、token、lease seconds、策略引用；要求 reason=INACTIVE 且 owner 非豁免。
- 手动上下文：保留 reason=MANUAL 的租约保护，不执行白名单检查。
- 单独手工 Bundle 删除：继续使用原来的无 Issue 租约路径。

实现 connection 级内部校验/续租 helper，在同一 `write::run` 事务里完成“校验当前租约与策略 → 执行本批 SQL”。外部 pool 包装负责申请 writer admission，内部 helper 只能使用传入连接，禁止再次调用 `run/acquire`。

当前 main 的批次清理仍是“先续租，再进入独立批次事务”，只有部分最终步骤在事务内续租。#143 不能假定 #141 已替所有批次完成策略保护，应在本次自动清理上下文接入时补上必要的事务内校验。

如果 owner 被豁免，返回明确的策略跳过结果，不当作 SQLite 错误重试，也不增加 deletion_attempts。实际 DB/I/O 失败保留现有有界重试和删除恢复机制。

Issue 最终事务必须同时保证有效租约、非豁免 owner、正确 deletion_reason，并原子完成 saved searches 清理、Issue 删除和自动过期审计。条件失败需要回滚，不能在已经执行部分写入后以 `Ok(false)` 提交。

Blob GC 继续只按真实引用清理无引用数据。白名单不会恢复已移除的文件引用，也不应该阻止无引用 Blob 回收。

## 6. 状态和配置变更语义

| Issue 状态 | owner 是否豁免 | 系统动作 |
| --- | --- | --- |
| ACTIVE，尚未到期 | 任意 | 保留全部 Bundle |
| ACTIVE，已到期且无进行中上传 | 否 | 原子 claim 为 INACTIVE，整体删除 |
| ACTIVE，已到期 | 是 | 跳过，不写删除状态/审计 |
| DELETING / INACTIVE，等待恢复 | 否 | 依原有 retry/lease 条件恢复 |
| DELETING / INACTIVE，等待恢复 | 是 | 停止自动续删，保留现有残余状态 |
| DELETING / MANUAL | 任意 | 继续手工删除恢复 |

新增豁免通过重启生效，同一运行进程内策略不可变。启动首次 inactive worker 和后续周期 worker 使用相同 `AppState` 策略；旧 token 不能在重启后绕过豁免。

已部分删除的 Issue 不恢复 ACTIVE，也不把 Bundle 状态批量恢复 READY。它可能已经失去文件或索引，伪装为完整数据不可接受。

对暂停的 INACTIVE Issue：启动/周期汇总日志标明数量和 Issue 标识，说明剩余内容保留但普通列表仍按现有规则隐藏。owner 可使用现有 DELETE Issue 接口显式结束删除；不在本 Issue 引入回收站、恢复按钮或自动修复数据。

为支持上述手工操作，调整 `delete_issue()` 的 INACTIVE 分支，仅当“当前 owner 有权删除 + 当前 owner 豁免 + 原自动租约已过期或为空”时，在 `write::run` 中条件更新为 MANUAL，清除旧自动 token/retry/claimed days，再交给 `claim_manual_recovery()`。存在有效租约时仍返回 409，不能强行接管正在执行的 worker。非 owner 仍 403。

移出白名单并重启后，暂停的 INACTIVE Issue 可按已有恢复规则继续；已经手工转换为 MANUAL 的始终按手工逻辑完成。

非活跃天数设置为 0 保持现有含义：停止新的 ACTIVE 自动 claim，但先前已经 INACTIVE 且未豁免的删除仍可恢复。文档必须写清这个区别；不将天数变更扩展成删除撤销机制。

升级时已经是 DELETING 的独立 Bundle 仍由现有恢复流程完成。当前 Schema 没有记录该 Bundle 最初是旧 retention 还是用户手工触发，因此不能安全恢复或取消它。新版本不会再基于 Bundle 年龄产生新的删除，升级说明应披露这点。

## 7. 前端与 API 一致性

现有 Issue 详情的 `inactivity_expiry` 驱动到期警告。只改后台删除会导致豁免用户仍看到“即将删除”，因此需要同步处理。

在 `get_issue_bundles()` 查询中读取 owner 的 `username_normalized` 供内部策略判断，不额外暴露用户名列表。owner 豁免时返回 `inactivity_expiry: null`；现有 `IssueExpirationNotice` 已在 null 时隐藏，无需引入新的 API 字段或前端白名单。

保持现有 `touch_issue_activity_best_effort` 调用行为，不能因名单命中提前 return 而跳过活动刷新。游客、其他用户的详情访问与 owner-only 过期信息规则保持不变。

白名单在本次仅为部署配置，不新增管理员编辑白名单的 UI。管理员非活跃天数设置旁可增加一句说明：部署配置中的豁免用户不参与自动清理。

## 8. 修改文件清单

| 文件 | 改动 |
| --- | --- |
| `backend/src/config.rs` | 移除 retention 字段，增加名单解析与纯函数测试 |
| `backend/src/services/issue_cleanup_policy.rs`、`services/mod.rs` | 共享不可变策略、规范化和 SQL 参数表示 |
| `backend/src/lib.rs` | AppState 注入策略，默认空集合 |
| `backend/src/main.rs` | 删除旧 retention 阶段，配置注入及一次性升级 warning |
| `backend/src/routes/issues.rs` | 候选/claim/recovery/finalize 豁免，手工恢复兼容，到期信息 |
| `backend/src/db.rs` | 移除旧 retention 函数，自动删除批次接受并事务内验证策略上下文 |
| `backend/src/routes/mod.rs` | 核对所有 worker 使用统一状态，不新增重复清理入口 |
| `backend/tests/smoke.rs`、`ownership.rs`、Issue/DB 单元测试 | 改写旧 retention 测试并增加策略、恢复、权限回归 |
| `frontend/tests/issue-expiry.behavior.test.tsx` | 验证 null 到期信息隐藏；按实际 UI 修改范围补测 |
| `README.md`、`backend/.env.example`、`doc/DB.md` | 新配置、DB 天数优先级、重启生效和升级中的删除状态说明 |

不修改 migration 或历史 SQL fixture；本次策略用现有结构表达。若实施中出现必须新增持久状态的需求，应单独评审并新增后续 migration，不能改 baseline。

## 9. 回归测试和验收

### 配置与身份

- 未设置、空白、连续逗号、重复项、大小写、前后空格规范化。
- 非法名称 warning/忽略，未知合法名称不阻塞启动且保留；之后创建用户能够匹配。
- Issue owner=alice、Bundle uploader=bob 时仍按 alice 判定，反向组合也覆盖。
- owner 为空维持非豁免规则，disabled owner 仍豁免。
- 旧 retention=0/正数/非法字符串只产生一次 warning，不触发校验失败或删除。

### 生命周期与 UI

- 普通非活跃 Issue 整体删除，FTS、files、offsets、容量和审计保持一致。
- 活跃 Issue 内十年前的 Bundle 保留，包括运行一次启动清理与周期清理后；不能只验证白名单 helper。
- 白名单非活跃 Issue 及其全部 Bundle 保留，状态仍 ACTIVE，自动删除审计为 0。
- PENDING/PROCESSING 上传依然阻止新的 inactive claim，白名单不绕过上传状态规则。
- 超过一页（例如 25 个）豁免 Issue 排在普通候选之前时，普通到期 Issue 仍被处理；ACTIVE 和恢复列表都覆盖。
- 豁免 owner 的详情 `inactivity_expiry=null`，活动时间仍按现有定义刷新；普通 owner 原有过期提示继续工作。

### 删除恢复和权限

- 先持久化部分 INACTIVE 删除状态，关闭应用，再以相同 DB 和新豁免名单构建应用；首次/周期清理均不继续删除残余内容。
- 直接调用 inactive claim/recovery helper，验证白名单不能绕过候选筛选进入执行。
- 豁免 owner 已经 MANUAL/DELETING 的 Issue 在重启后完成删除。
- 豁免 owner 从 ACTIVE 手工删除 Bundle/Issue 成功；其他用户仍不能删除。
- 豁免的 INACTIVE Issue，租约过期后可由 owner 显式转换 MANUAL；有效租约期间返回冲突，非豁免不擅自接管。
- 移出名单后旧 INACTIVE 删除可恢复；days=0 阻止新 claim，但保持明确的旧删除恢复语义。

### 并发和失败

- 文件数据库、多连接与 writer admission 下，利用 Barrier/Notify 确定交错：候选读取后 owner/状态改变，claim 仍按当前 DB 判定。
- 等待写入资格时租约失效/被接管，批次不得继续删除，最终审计不得写入。
- 自动最终事务失败时不留下部分删除/重复审计；不在 retry 闭包里执行文件删除。
- 豁免检查不抢占 MANUAL 租约，不让 generic Bundle recovery 续删 INACTIVE Issue 的子 Bundle。

检查命令：`cargo test`、`cargo clippy -- -D warnings`、`cargo fmt --check`；涉及前端时运行 `npm test` 与 `npm run build`。测试失败应定位，不以单线程替代并发回归来满足验收。

全仓搜索旧标识，生产代码中只允许保留检测旧环境变量的 warning；历史文档和升级说明可明确提及旧名，但不能保留删除执行入口。

## 10. 实施顺序与发布

建议一个 #143 PR，分为五个可审查提交：

1. 删除旧 Bundle retention 执行链，改写旧测试；保留手工/状态恢复清理。
2. 配置解析、不可变策略和 AppState 注入；实现 owner 查询与 SQL 参数复用。
3. 候选筛选、原子 claim、恢复、自动批次和最终事务的策略保护；补手工接管暂停自动删除的状态分支。
4. 到期信息一致性和完整测试矩阵。
5. 配置/升级文档、完整检查及对 main 最新变更的整合。

发布时先确认 DB 中实际 `issue_inactive_days` 与豁免用户名，再停止旧实例、备份数据库和数据目录、启动新版本；名单必须在首次 inactive worker 前生效。仍按单实例运行，不通过双版本同时连接数据库完成滚动升级。

升级日志至少报告旧 retention 已停用、豁免名单条目数、首次检查发现的被豁免暂停删除数；不每小时为每个正常豁免 Issue 写 audit。实际自动删除继续写现有 `ISSUE_AUTO_EXPIRED`。

回退旧 binary 会重新启用其支持的 retention 逻辑且不认识白名单。若需回退，应在旧实例启动前禁用 `RAIN_RETENTION_DAYS`，并确认数据库里的 Issue 自动清理设置，否则无法保持新豁免承诺。

与 #142 可并行：本方案负责“是否允许自动删除整个 Issue”，#142 负责显式文件树删除的状态与执行效率。双方在 DB cleanup helper 上可能重叠，应共享 writer admission 和删除上下文，不给文件树任务新增 Bundle 年龄策略。
