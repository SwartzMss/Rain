# #228：交互式搜索执行状态、取消与安全时限

状态：设计提案，供 PR 评审；本 PR 不修改运行时代码。

关联：[#228](https://github.com/SwartzMss/Rain/issues/228)。代码基线：`618e0f5`，已包含 #224 的 Tantivy preview 优化和 #226 的 upload handoff。

## 1. 目标与边界

所有 `previewTempResult` 内容搜索入口统一提供搜索中状态、真实 elapsed time、无百分比的进度提示和用户取消。取消必须传到后端实际执行线程，并在清理后释放资源。新搜索运行、取消或失败时保留上一次成功结果；只有当前执行成功才替换结果或打开新 viewer tab。

第一阶段覆盖 Issue 详细搜索、单文件搜索、结果内二次搜索、Saved Search，以及独立 TempResult 页面的继续搜索。文件名搜索和读取已生成结果的分页不创建 materialization execution；它们仍需独立的 loading、AbortSignal 和 stale-response 防护，不能共用内容搜索状态覆盖彼此。`POST /temp-results` 的显式完整结果生成不增加取消交互；共享 executor 的签名变化必须保持该入口兼容。

不增加真实百分比、SSE/WebSocket、任务历史中心或多节点取消服务。本方案保证协作式停止，不承诺能强制中断任意内核 I/O 或第三方同步函数。

## 2. 已核对的代码现状

| 位置 | 现状与设计影响 |
| --- | --- |
| `frontend/src/api/client.ts` 的 `request` | 已通过 `...init` 透传 RequestInit；缺的是 preview 的 signal 参数，以及 fetch catch 将 AbortError 转为普通 Error。还需处理 `response.text()` 期间的取消。 |
| `frontend/src/features/files/FilesView.tsx` | 四处调用 preview；普通搜索有 generation 检查，Saved Search、单文件与 nested 路径不统一；部分开始/失败分支清空其他结果状态。 |
| `frontend/src/features/files/TempResultView.tsx` | `createFromResult` 单独维护 creating，成功后导航，没有取消语义。 |
| `backend/src/routes/temp_results/routes.rs` | preview 没有 RequireUser，当前允许访客搜索；直接给取消路由加登录限制会让访客无法真正取消。 |
| `backend/src/routes/temp_results/service.rs` | resolve 和 materialize 分别使用 timeout；planning、permit 等待、publish 和首屏读取没有统一 deadline。 |
| `backend/src/routes/temp_results/search_plan.rs` | 等待 query permit 后执行 Tantivy；索引错误统一降级 raw。取消/超时必须从该降级路径排除。 |
| `backend/src/search/tantivy/mod.rs`、`query.rs` | 查询用 spawn_blocking；query permit 和 generation lease 归阻塞闭包持有；`CandidateSearch::search_page` 有 segment/doc 循环可插入检查点。 |
| `backend/src/services/temp_results.rs` | raw/candidate 扫描共用 executor，按 source、range、line 执行；超长逻辑行使用 chunk callback，不能只在换行时检查取消。 |
| `backend/src/routes/temp_results/lifecycle.rs` | `abort_staging_result` 先 claim STAGING 为 DELETING，再清文件和记录；失败保留 DELETING 供恢复。当前每 peer-IP 最多一个 materialization。 |
| `backend/src/config.rs`、`settings/metadata.rs` | timeout 默认都是 30 秒；已初始化 DB 设置优先于 ENV，调整代码默认不会更新存量设置。 |

## 3. 方案选择

1. **采用：同步 preview 响应 + 预注册执行身份 + 显式取消 + 协作式 executor。** 保留现有结果响应格式，多一次很小的控制请求，解决注册与取消乱序，兼容访客。
2. **仅在 preview POST 注册，再给未知 ID 存取消 tombstone。** 少一次请求，但 tombstone 到期后迟到 POST 可能重新启动；需要额外签名期限协议才能完整关闭竞态，且未知 ID 写入易耗尽 registry。
3. **全面改为异步任务 API。** 可以自然提供状态查询，但需要任务结果交付和轮询协议，第一阶段收益不足以抵消改造面。

这里的预注册只返回启动凭据，不创建持久任务、不占 query/materialization permit。执行完成仍由 preview HTTP 响应交付。

## 4. 身份、协议与鉴权

每次真正执行前，前端生成新的 UUID `search_id`；绝不复用 viewer tab ID 或 `result_id`。执行记录的身份为 search_id，加独立、不可从 ID 推导的 256-bit 随机 capability。服务端仅保存 capability 的摘要，凭据不进入 URL、普通日志或持久存储。

### 4.1 预注册

`POST /api/search-requests`，JSON `{ "search_id": "<uuid>" }`。

成功返回 201：`{ "search_id": "...", "cancel_token": "...", "expires_in_ms": 60000 }`，标记 `Cache-Control: no-store`。该 token 通过 `X-Search-Cancel-Token` 同时用于 preview 启动和取消。鉴权使用当前 OptionalUser：有登录身份的 reservation 绑定 user ID；访客由 capability 授权，不能使用 IP 作为所有权依据。

对于绑定用户的记录，所有操作同时校验当前登录 user ID 和 capability；管理员不能借角色取消别人。访客记录只校验 capability，不因后续登录升级为别人的用户记录。账户失效后取消无法通过用户校验的工作仍由 deadline 收敛。客户端在登出前尽力取消自己的执行。

重复 search_id 返回 409 `SEARCH_REQUEST_CONFLICT`，不覆盖旧记录。非法 UUID 返回 400。预注册纳入现有每 IP 10 次/分钟控制；后续 preview 不再对同一预注册执行重复扣速率额度。取消使用独立限流：有效 capability 每分钟最多 120 次，未知/错误凭据按 peer IP 每分钟最多 120 次，限流桶同样有界且到期回收，不能因创建额度耗尽而阻止正常取消。

### 4.2 启动 preview

`POST /api/temp-results/preview` 在现有 payload 增加 `search_id`，携带上述 header。服务端仅允许原子 `RESERVED → RUNNING`，并在 RESERVED 的 60 秒有效期内启动。取消、过期、丢失或已消费的 reservation 均不能启动；新 ID 未预注册时不能隐式创建 execution。

已取消记录返回 409 `SEARCH_CANCELLED`；其他不可启动 reservation 返回 409 `SEARCH_REQUEST_UNAVAILABLE`。凭据错误与记录不存在采用相同公开响应，不暴露 owner 或执行状态。

兼容没有 search_id 的旧客户端：继续允许 preview，由服务端生成内部身份并受统一 deadline 控制，但不承诺客户端主动取消。带 search_id 却没有合法 reservation 的请求不能退回旧客户端路径。

### 4.3 取消与确认

`DELETE /api/search-requests/{search_id}` 使用独立请求及 header，不能复用已 abort 的 signal。

| 条件 | 返回与含义 |
| --- | --- |
| RESERVED | 转为 CANCELLED，200 `{ "status": "cancelled" }`，后续启动被拒绝。 |
| RUNNING | 原子记录取消原因并通知 token；完成资源回收后返回 200 cancelled。最多等待 1 秒，尚未完成则返回 202 `{ "status": "cancelling" }`。 |
| CANCELLING | 同样等待/返回，重复 DELETE 幂等。 |
| TIMING_OUT | 等待已有超时收尾，202 cancelling 或 200 timeout；不改写已确定的停止原因。 |
| COMMITTING | 不撤销发布；最多等待 1 秒，未结束返回 202 `{ "status": "finishing" }`；终态返回 200 completed 或 failed。 |
| 已终止且凭据有效 | 200，status 为 cancelled、completed、timeout、failed 之一。不返回 result_id。 |
| 不存在、已过期或凭据/用户不匹配 | 一律 204，无状态信息，无 registry 写入，无取消效果。 |

前端收到 202 后可用相同 DELETE 每秒确认一次；前 5 秒显示 CANCELLING，随后显示中性提示“已停止等待，服务器仍在结束本次搜索”，继续有限频率确认，直到终态或上下文离开。网络失败显示“取消请求未确认，可重试；服务器安全时限仍生效”，保留重试入口，不虚报后端已经停止。

204 只说明无可访问记录，不证明某个跨重启任务被该请求中断。对成功预注册且凭据、登录上下文保持不变的当前执行，可结束本地等待并显示“搜索已取消”；记录已过期或进程重启后迟到 preview 必然无法启动。登录上下文变化时只显示“已停止等待”，不把 204 当作资源释放证明；后续新请求仍受正常 admission 检查。

### 4.4 注册乱序与上限

前端等待预注册完成后才发送 preview。如果在预注册返回前点击取消，立即标记本地取消；待 token 返回，仅发送 DELETE，绝不启动 preview。预注册请求本身设置 5 秒控制超时；响应丢失的 RESERVED 由 60 秒 TTL 回收，无搜索工作。

active/reserved 总容量初始上限 1024；每登录 user ID 最多一个未终止 reservation/execution，访客按 peer IP 最多一个，另保留现有每 IP materialization admission。这里 IP 只用于容量限制，不用于取消授权。terminal 回执用单独最多 1024 项、TTL 60 秒的缓存，容量满可淘汰最旧终态，不淘汰活跃执行。未知取消不创建 tombstone。真正启动始终要求仍存在的 RESERVED，因此终态缓存淘汰也不能使迟到请求复活。

registry 由 AppState 共享给全部 Actix workers，仅短暂持有锁；锁内不 await。定时清理 reservation/terminal 过期项。活跃 registry 直到 worker 完成/清理后才注销；内存和准入均有界。

## 5. 执行所有权与取消传播

新增 `backend/src/services/search_execution.rs`，统一封装 registry、终态通知、deadline、首个停止原因及 `SearchExecutionContext`。context 包含 `search_id`、CancellationToken、单调时钟 deadline；`checkpoint()` 返回类型化 `Cancelled` 或 `TimedOut`，供 async 与 blocking 路径共同使用。

合法启动后由受跟踪的 worker 持有 materialization permit/client lease、resolved source/read lease、staging lease 及结果清理责任；HTTP handler 只等待 worker 结果。handler 被 drop 时通过 guard 发出取消信号，不能 abort worker。Actix HttpRequest 不跨线程移动：先提取 principal、peer key、payload 和拥有所有权的数据，再启动 worker。应用关闭通知所有 worker 并在 shutdown grace 内等待清理，异常终止交给现有 startup recovery。

worker 无论成功、取消、timeout、错误都走同一收尾路径，发终态通知并注销 registry。panic/JoinError 转为失败，释放 RAII 资源；不能异步清理的遗留 STAGING 由恢复机制处理并记录异常。不得将取消实现为 handler 外层 `select!` 丢掉整个 materialization future，绕过 `abort_staging_result`。

| 阶段 | 必须加入的检查与资源规则 |
| --- | --- |
| source resolve | DB 查询等待、逐 source/文件解析前后检查；取消等待可用 select，但不能跳过已取得资源的收尾。 |
| planning/query admission | 每 source 前后检查；query semaphore 等待可取消；尚未获取 permit 就取消不得泄漏配额。 |
| Tantivy blocking | context 移入阻塞闭包；index open/weight/scorer 前后、每 segment、doc 迭代与结果转换检查。必须等待闭包实际退出，之后才能报告资源已释放。 |
| candidate verification/raw scan | 每 source/range/line 检查；每 64 KiB 读取块检查并定期 yield，覆盖无换行超长输入和全内存缓存扫描。 |
| materialization | 写 log/meta/idx 前后检查；关闭所有 writer 后统一执行 abort_staging_result。 |
| publish/首屏交付 | 按第 6 节的原子提交边界处理，不在持久化操作中间任意丢弃 future。 |

现有 line reader callback 返回 `()`，不能假定它能传播错误。新增可失败的受控读取版本供 temp executor 使用，旧接口作为无取消 wrapper 保留，避免影响 ingestion；受控版本在填充 buffer/chunk matcher 前后执行 checkpoint。

`search_plan.rs` 改为区分 `Fallback(reason)` 与 `Stopped(reason)`：取消、timeout 一律向上传播，绝不能落入当前 `map_err(|_| "tantivy_query_failed")` 后继续 raw scan。非 preview 调用通过无取消 context/default wrapper 保持兼容，并验证 Tantivy feature 开启和关闭的构建。

Tantivy index open、单次 scorer 调用、一次 OS 文件操作不能保证立即抢占。取消延迟需实测并披露；不得提前释放仍被 blocking worker 使用的 permit/lease 来伪造停止。

## 6. 取消、发布与清理的竞态

后端状态：`RESERVED → RUNNING → COMMITTING → COMPLETED`；RUNNING 可转 CANCELLING 或 TIMING_OUT，经清理到 CANCELLED/TIMED_OUT；任意工作错误经收尾到 FAILED。

发布前完成扫描、关闭 writer、校验 staging 大小，并从 staging 读取首屏数据。这样 publish 后不再新增可能失败的首屏 I/O。新增 staging page read helper，不绕过现有 ACTIVE 结果读取校验。

worker 在开始最终 rename/DB 发布前，以同一原子状态/短锁执行 `RUNNING → COMMITTING`；cancel 与 timeout 同样竞争 RUNNING。此时也必须检查 deadline，已超时不能获准提交。

- cancel/timeout 先赢：不允许发布。关闭句柄后调用 `abort_staging_result`，清理 .part 及已经 rename 的 artifacts，释放资源再通知终态。
- commit 先赢：取消返回 finishing/completed，不能删除已提交结果。完成现有 rename 与 conditional publish；数据库 ACTIVE 转换成功后为 COMPLETED。提交期间不再用取消 select 丢弃数据库 future。
- rename/发布失败：复用 staging cleanup；若 DB 提交状态因错误不明确，查询持久状态。ACTIVE 视作已经发布，不能作为 staging 清除；STAGING/DELETING 走现有清理。服务恢复负责中断后的收敛。

取消 API 从不直接删除结果文件，也不调用 ACTIVE 删除接口。`abort_staging_result` 已有 claim 状态保护，继续沿用。清理失败允许保留 DELETING 供 retry/recovery，但此时所有活动句柄、permit、read/staging lease 仍必须释放；telemetry 增加 cleanup_pending，不能声称磁盘已清空。

COMMITTING 是不可撤销的短收尾区，沿用数据库/文件操作自己的边界和超时；安全 deadline 阻止新工作与新提交，不能承诺在时限一到就杀死同步 I/O。记录 commit/cleanup 耗时，慢收尾报警。

前端已点击取消但后端完成优先时，仍保留旧结果，不打开新 tab。这个未交付 preview 按现有 30 分钟 retention 回收，不新增“取消已发布结果”的权限路径；将此种情况计为后端 success 和 cancel_after_commit，而非 cancelled。

## 7. 前端执行层与展示

新增 `hooks/useSearchExecution.ts`、`components/SearchExecutionStatus.tsx`。使用状态：IDLE、RUNNING、CANCELLING、CANCELLED、SUCCEEDED、FAILED；FAILED 区分 timeout/network/server，取消确认状态单独保存。elapsed 从 `performance.now()` 的起点计算，每 250ms 刷新，终态冻结；隐藏页面恢复时从时间差计算，不能靠计数器累加。

一个执行快照固定 `{searchId, scopeKey, expression, source, generation, startedAt}`。hook 管理预注册、AbortController、取消确认与卸载清理；调用方通过带身份校验的成功回调更新结果。**Abort 前先作废 generation**，所有 promise 的 then/catch/finally 都验证身份，防止旧请求清除新 loading、覆盖结果或导航。

API client：preview 增加可选 signal 和 search_id/capability；request 在 fetch 和 body read 两阶段识别 abort，抛 `RequestCancelledError`，不经 normalizeApiError 包装。DELETE 有独立 controller；控制请求超时不能当作用户取消。应用级信号取消不导致登录重验证事件。

| 入口 | scope 与成功更新 |
| --- | --- |
| Issue 详细搜索、Saved Search | 共用 `issue:<code>` 执行槽；成功才替换 issue hits/创建 tab，Saved Search used 标记只在有效成功后发送。 |
| 单文件搜索 | `file:<bundleHash>:<fileId>`；成功时一起更新 hits/total/from/tab，切换选中文件取消该槽。 |
| SearchResultViewer 二次搜索 | `viewer:<tabId>:<resultId>`；每个来源 tab 独立状态，关闭或离开该 tab 取消对应执行。 |
| TempResultView 继续搜索 | `temp:<resultId>`；有效成功才导航，取消保留当前结果。 |

保留当前每 IP 仅一个 materialization 的后端限制。前端页面级 coordinator 只允许一个实际 preview 正在执行；各 scope 状态独立，不能复用一个全局 loading。新搜索替换旧执行时先使旧 generation 失效并取消，拿到终态/无记录确认后才启动新的 reservation；取消未确认时不偷偷并发启动，显示中性等待。跨浏览器 tab 竞争仍可能得到 TEMP_RESULT_BUSY，按服务器繁忙提示处理。

RUNNING 显示“搜索中…”按钮、“取消搜索”、scope 名称、`mm:ss` elapsed 和不带 aria-valuenow 的 indeterminate progress；执行表达式和来源相关控件只读。10 秒后显示“大范围日志仍在搜索，可以继续等待或取消”。CANCELLING 禁用重复取消按钮，但确认失败后提供重试。状态用 aria-live polite，避免每 250ms 播报时间；减少动态效果偏好下保留文字。

成功显示“搜索完成 · N 条 · X.X 秒”；正常取消用中性色“搜索已取消 · 运行 X.X 秒”。408/TEMP_RESULT_SCAN_TIMEOUT 显示“搜索达到系统安全时限，已停止。建议缩小搜索范围或调整搜索条件”。取消确认尚未到达时显示等待/未确认文字，不提前宣称服务器停止。

旧结果连同其表达式、total、分页和 viewer 归属作为一个快照保留，并标注“上次搜索结果”；新草稿不能成为旧结果标题。失败/取消不清空旧快照。Issue/Bundle/route change 必须取消并隐藏不属于新上下文的旧快照；该规则优先于“保留旧结果”。unmount 先作废身份，再 best-effort DELETE（可用 keepalive），页面关闭/断网仍依赖安全 deadline。

## 8. timeout 与设置发布策略

一个执行在 RESERVED 转 RUNNING 时读取设置并确定唯一 monotonic deadline；包含 source resolve、query permit 等待、planning、Tantivy、verify/materialization 和获准提交前检查。预注册有效期独立，不计入搜索工作时间；UI elapsed 包括控制请求时间。取消与 timeout 的首个有效停止原因固定，不互相覆盖。

建议新安装默认 **300 秒**，保留管理员配置与现有 setting key `temp_results_max_scan_duration_seconds`，改 UI 标签为“搜索安全时限（秒）”。这是候选默认值，尚无本 PR benchmark 数据；实际调整必须随实现 PR 提交下述基准证据，未达标时保持原默认，不能只先拉长时限。

基准矩阵：Issue 多文件/单文件/temp-source；Tantivy 稀疏与密集命中、exact verify、AND/OR raw fallback、Unicode fallback、超长无换行行；冷/热缓存；1/4 个不同客户端并发；100 MiB、1 GiB、10 GiB 数据（记录硬件、数据生成方式、命令和样本数）。报告完成耗时、cancel-to-worker-exit p50/p95/max、峰值 RSS、磁盘与 permit 恢复情况。健康本地存储条件下，受控取消回归目标 ≤1 秒；不可抢占 I/O 单独列出，不混入保证。

更新点包括 config 默认、settings metadata 默认、管理员提示、README/配置示例及对应测试。DB 已初始化的值必须保留，尤其不能把所有值为 30 的记录当成默认值自动覆盖；部署说明指导管理员在完整取消能力上线后自行调高。现有显式完整结果生成也读取这个设置，需说明默认调整的连带影响。

## 9. Telemetry

worker 收尾处统一记录一次 `metric=interactive_search`：search_id、scope（issue/file/temp_result）、outcome（success/cancelled/timeout/error）、elapsed_ms、search_backend（tantivy/mixed/raw_scan，尚未规划用 unknown）、candidate_count、verified_match_count。部分执行的计数只记已完成工作，不能伪造为 0 或最终 total。

另记录 cancellation_requested、cancel_to_exit_ms、cancel_after_commit、cleanup_pending、active/reserved 数与 admission rejection。纯用户取消不计入业务失败率；HTTP 可返回 409/SEARCH_CANCELLED，不要求使用非标准 499。access logger 对已标记的正常取消响应使用 info，保留真正错误级别；不能只因 HTTP 4xx 就把取消统计为失败。

不记录完整 expression、capability 或带凭据 header；search_id 为随机标识，不使用其作为高基数 metrics label（仅结构化日志/trace）。

## 10. 实现分段与评审门槛

| 顺序 | 主要文件/单元 | 可独立核验的产出 |
| --- | --- | --- |
| 1 | 新 `services/search_execution.rs`、`routes/search_requests.rs`；`lib.rs`、`routes/mod.rs` | 有界 registry、reservation/capability、幂等取消、终态确认、shutdown 所有权。 |
| 2 | `routes/temp_results/{service,search_plan,storage,lifecycle}.rs`、`services/temp_results.rs`、`ingest/indexing/line_reader.rs`、`search/tantivy/{mod,query}.rs` | 全链路 context、blocking 检查、类型化停止、提交边界与 staging 清理。 |
| 3 | `frontend/src/api/{client,types}.ts`、新 hook/status、FilesView/TempResultView/SearchResultViewer | 全入口 UI、保留结果、scope coordinator、signal 与 stale-response。 |
| 4 | config/settings/README、测试与基准记录 | 跑取消/发布竞态与回收测试后，依据基准确定默认；完整功能同时发布。 |

第 1、2 段可先合入保持旧客户端兼容；只有后端取消链完整后才启用前端取消交互，最后才调整默认 timeout。此设计 PR 使用 `Refs #228`，不关闭功能 Issue。

## 11. 验收与测试

测试用 deferred promise、barrier 和注入时钟控制交错，不用固定 sleep 猜测竞态发生。

- **前端 hook/API：** reserve 未返回就取消；fetch 与 body read abort；DELETE 使用独立 signal；202→200、网络错误重试；elapsed 递增和冻结；A 取消→B 开始→A 晚到不修改任何 B 状态。
- **前端真实页面：** 对五类 preview 入口分别执行 start/cancel/success/error；断言状态/禁用条件/无假进度/旧结果快照保留/不打开取消任务 tab。Saved Search 与普通 Issue 搜索替换；文件/tab/route/Issue 切换及 unmount；已生成结果分页不覆盖执行状态。
- **身份与 registry：** 登录 A 不能取消 B（即使知道 ID）；错误 token 无副作用；访客凭据只控制自己的执行；相同 IP 不获得取消权限；未知与无权响应一致；重复 ID、重复 DELETE、TTL、进程重启、容量/速率限制、取消不消耗创建额度。
- **启动乱序：** 先 reserve→cancel→迟到 preview 永不执行；terminal 淘汰后迟到 preview 仍拒绝；reservation 响应丢失无 materialization；并发两个 POST 只一个进入 RUNNING。
- **后端真实执行：** 等待 query permit、source stream、Tantivy segment/doc 循环、candidate verify、raw 扫描、超长无换行输入、temp source 和三类输出写入时触发取消，确认实际 worker 退出而非 handler 提前返回。
- **资源断言：** 正常清理后 STAGING/DELETING 行及 .part/.log/.meta/.idx 不残留；materialization/query permit 回到基线；staging/read/generation lease 与 active registry 回到基线；文件可重新打开/删除。故障注入清理失败则断言 DELETING 被恢复最终收敛。
- **发布竞态：** barrier 放在 COMMITTING CAS 前/后、rename 之间、DB 发布后、响应发送前；验证取消先赢不 ACTIVE，提交先赢不误删 ACTIVE，只有一个终态且无 double cleanup/panic。DB 返回不明确与 worker panic 走恢复路径。
- **timeout：** 注入短 deadline，覆盖 resolve/planning/blocking/raw/提交前；必须返回 TEMP_RESULT_SCAN_TIMEOUT 而非 SEARCH_CANCELLED；取消先赢时不被后续 deadline 改写。无 search_id 的旧客户端同样有安全 deadline。
- **验证命令：** `cd frontend && npm test`、`npm run build`；生成 dist 后在 backend 执行 `cargo test` 和 `cargo test --no-default-features`，以及 `cargo fmt --check`。实现时先确认 Cargo features 与 CI 矩阵，补充仓库要求的检查。此文档 PR 只做 Markdown/引用与差异检查，不声称执行了运行时回归或 benchmark。

设计完成的评审重点是：访客 capability 与预注册额外请求、提交优先时的取消反馈、保留现有单 IP 并发限制、300 秒候选默认及存量配置不自动迁移。实现验收以上述行为和竞态测试为准。
