# Issue #269：FilesView 搜索异步状态拆分设计

日期：2026-09-29
状态：设计提案，尚未实现
基线：已 fetch 的 `origin/main@18a0c1c90de8071b9497eb79810132a103d28ab7`
需求：https://github.com/SwartzMss/Rain/issues/269

## 1. 结论与范围

按 Issue 搜索、File 搜索、Viewer 二次搜索、结果分页、Saved Search 五个领域拆 controller。保留 `useSearchExecution` 作为搜索传输及服务端取消协议层；每个搜索 controller 拥有自己的用户意图、编辑器、状态、错误和结果提交权限。

本次不改后端接口、Boolean parser/planner、临时结果格式、文件树、source reveal、原文件内容缓存或页面布局。`FilesView` 的搜索部分收敛为组合和事件连接，其他领域的现有编排不强行一起拆除。不引入全局状态库或全局大 reducer。

最新 main 已包含 #260 统一编辑器和 #268 Boolean candidate planner，并合入 #271 单文件/结果筛选仅关键词输入。Issue 搜索及 Saved Search 继续使用完整表达式；File 和 Viewer 筛选继续使用 `allowOperators=false` 的关键词编辑器。

## 2. 基线核查

| 位置 | 当前实现 | 设计影响 |
| --- | --- | --- |
| `FilesView.tsx` | 三个 `useSearchExecution()`，但 Issue、Viewer、`loadViewerPage` 共用 `searchLoading/searchError` | 删除共享 UI 状态和通过终态 effect 修改它们的逻辑 |
| `runSearch` / `useSavedSearch` | 两套 validation、generation、结果转换逻辑 | 合并为 Issue controller 的同一条运行通道 |
| 编辑器 onChange | 手动增加 `searchRequestGenerationRef` | editor 更新也进入 controller，失效 pending validation |
| `runSearch` | 直接清 File 搜索状态和 Viewer 筛选草稿 | 拆分时必须明确跨域交互，不能照搬其他 controller 的 setter |
| `loadViewerPage` | 只按 tab id/kind 回写，没有 latest-request guard | 按上下文、tab、result identity、请求序号保护成功、失败和 finally |
| `useSearchExecution` | 独立 generation 和替换请求 sequence，取消确认后才允许新执行 | 保留协议层；补齐等待替换期间的失效与卸载约束 |
| `api/client.ts` | validation 和 temp-result lines 暂无 signal 参数 | 可选增加 AbortSignal，不改变 HTTP API |
| `useViewerTabs` | 管理结果 Tab 与临时结果清理 | 继续作为 Tab 数据和已接收结果资源的唯一 owner |
| `files-view-search.behavior.test.tsx` | 三个 execution 共用 mock execute/cancel，snapshot 恒为 IDLE | 无法覆盖不同实例的终态干扰；补真实 hook/独立实例测试 |

这些结论来自源代码审查；本次未运行竞态复现或功能测试。

## 3. 所有权和依赖

| Controller | 拥有 | 输入/输出边界 |
| --- | --- | --- |
| `useIssueSearchController` | 表达式编辑器、validation、intent、execution、状态/错误 | 输入 Issue/Bundle 上下文；输出已确认有效的结果 Tab 描述 |
| `useFileSearchController` | 文件关键词编辑器、file scope、execution、空结果/命中状态 | 输入选中文件身份；输出结果 Tab 描述 |
| `useViewerSearchController` | 筛选编辑器、source result scope、execution、状态/错误 | 输入 active tab/result identity；使用 `source_temp_id` 搜索 |
| `useViewerPaginationController` | 每个结果 Tab 的请求序号、AbortController、分页状态/错误 | 接收分页命令，通过带身份校验的 Tab 更新接口提交页内容 |
| `useSavedSearchController` | 列表、增删改、保存/编辑对话框、登录恢复、使用记录 | 通过注入的 `issueSearch.applySavedSearch` 执行；不能直接调用 execution |

每个搜索 controller 内部各有一个 `useSearchExecution`。控制器之间不读写内部 ref/setter。可以抽小型通用 intent 生命周期工具和纯函数结果映射，但不做一个配置项繁多的万能搜索 controller。

建议对外接口：

```ts
// 示意接口；不是本次已实现的代码。
issueSearch = {
  editor, status, busy, error, executionSnapshot,
  setTokens, setDraft, run, applySavedSearch, cancel, clear
};
fileSearch = { editor, status, busy, error, run, cancel, clear };
viewerSearch = { editor, status, busy, error, run, cancel, clear };
viewerPages = { getState, loadPage, disposeTab, reset };
```

scope 变化由 hook 的输入驱动，所有失效最终进入内部同一个 `invalidate(reason)`。页面不持有 generation。

## 4. 意图、状态和取消协议

### 4.1 两层职责，不能只删掉一个 generation

controller 的 intent version 覆盖「编辑/校验 → 等待旧执行退出 → 执行 → 提交结果」整个用户操作。底层 execution generation 保护 reservation、preview 和 cancellation 的网络协议。两者用途不同，保留两层，但每层只能有一个 owner。

新意图开始时同步失效前一个意图，abort 旧 validation，并请求停止旧执行。任何异步 continuation 在启动下一阶段、更新状态或发布结果前，都检查 intent version、scope 和 mounted 状态。不能只在成功分支做检查。

尤其是 A 正在取消、B 已进入 `execute()` 等待取消、此时用户 clear/context change/unmount：B 不得在 A 完成后继续 reserve。建议给底层 `execute` 增加可选 intent signal，检查入口及所有等待之后的有效性；同步失效排队 sequence。普通 `cancel()` 也必须使待启动的替换请求失效。

新意图不得复用旧 snapshot 的成功、失败或 busy。可用调用身份关联 snapshot；旧取消仍未确认时，显式显示「等待旧搜索停止」及重试入口，而不是把旧状态冒充新请求状态。

### 4.2 状态模型

```ts
type SearchStatus =
  | 'idle' | 'validating' | 'waitingPrevious'
  | 'running' | 'cancelling'
  | 'succeeded' | 'failed' | 'cancelled';
```

`busy` 从状态派生，不单独存 boolean。`validating`、`waitingPrevious`、`running`、`cancelling` 都属于 busy。只有与当前操作匹配的 execution snapshot 才能参与派生。

预执行错误由 controller 保存；执行错误直接投影当前 execution 的错误，不用 effect 再拷贝一份。对外可提供带 `phase: 'input' | 'validation' | 'execution'` 的统一错误视图，但 UI 每条错误只渲染一次。取消未确认是取消状态，不包装成普通搜索失败。

校验阶段也提供取消入口。运行阶段继续保留已有取消确认、失败重试和服务端 60 秒安全时限。不因 clear 清空编辑器而假装服务端已停止；草稿可以清空，尚未完成的停止过程仍保留状态/重试提示。

### 4.3 事件规则

| 事件 | 行为 |
| --- | --- |
| Run / 使用 Saved Search | 创建新 intent；替换同领域旧操作；统一校验和执行路径 |
| 编辑器修改 | 失效旧 validation；清本领域输入错误；通过 owner API 更新草稿 |
| Cancel | 失效 validation 和排队执行；只取消本领域；保留草稿和已打开结果 |
| Clear | 失效并停止本领域操作；清草稿与输入错误；不删除已打开结果 |
| Issue/Bundle context 改变 | 立即失效相关搜索和分页；随后完成网络清理，禁止旧结果发布 |
| 文件身份改变 | 失效 File 搜索；身份包含 context、bundle hash、raw file id |
| active result tab 改变 | 保留现有 Viewer 搜索自动取消语义；禁止旧 tab 的搜索迟到后打开新结果 |
| Unmount | 阻止状态写入/新请求启动；abort 校验、分页和 preview；清 UI 定时器 |

scope 改变的提交防护必须覆盖 React effect 清理执行前的窗口，不能只依赖被动 effect 中递增一个 ref。可在 commit 阶段同步切换 scope owner，并让所有提交比较捕获 scope 和最新有效 scope。

卸载取消需要区分 UI 清理和服务端释放：reservation 尚未返回时，可能仍需一个有界收尾任务取得 cancel token 后发送 DELETE。它不得更新 React 状态、启动新搜索或留下无限轮询；不能无条件 abort reservation 然后丢掉释放服务端请求的能力。现有取消超时仍作为收尾上限。

## 5. 结果 Tab 与分页

搜索成功后先在 controller 内验证 intent/scope，再同步调用注入的 `onResult`。`FilesView` 仅把结果交给 `openViewerTab`；Tab 的内容、pin、关闭及清理仍由 `useViewerTabs` 管理。Tab 标识建议用执行 searchId 或 UUID，避免并发成功使用同一毫秒的 `Date.now()` 碰撞。

底层返回成功但已失效、没有被 Tab 接收的临时结果，应按现有权限规则做尽力清理；Guest 不调用需要权限的删除接口，继续依赖过期回收。不要让 controller 删除已交给 Tab 管理的结果。

分页仅覆盖 `search`/`temp` Tab；原文件分页继续使用 `useFileContent` 与 file content cache。

每个分页请求的身份为 `(context epoch, tabId, resultId, requestSequence)`。controller 内用 Map 保存每个 Tab 的 request state，不把 AbortController 放进可展示的 Tab 模型。调用新页请求时 abort 同 Tab 旧请求、递增 sequence；不同 Tab 可以并行。

提交时同时检查请求仍为最新、上下文仍有效、目标 Tab 仍存在且 resultId 一致。页内容、from、pageSize、pageHistory 原子提交；失败保留原页与 history，并只更新该 Tab 的 error。旧 finally 不能清新请求的 busy。

切换 Tab 允许旧 Tab 的分页完成并缓存到原 Tab；其提示不显示在当前 Tab。关闭 Tab、同 id 换 resultId 或 context reset 则取消并清掉对应 owner。重开同 id 必须使用新 epoch，旧请求不能借复用 id 回写。

## 6. Saved Search 与 UI 连接

Saved Search 的使用调用 `issueSearch.applySavedSearch(item)`，手工搜索与保存的搜索共享 intent、validation、scope 和提交规则。建议返回明确 outcome（成功且已发布、失效、取消、失败），避免把 `undefined` 当作笼统结束信号。

只有成功且已发布后才记录使用次数。使用记录写入失败属于 Saved Search 提示，不能把成功搜索改成 failed，也不能删除结果 Tab。列表请求、保存/编辑对话框请求有各自版本和取消处理；关闭对话框、退出登录后，旧成功和 catch 都不得恢复 UI。登录后恢复草稿只消费一次，继续支持现有 sessionStorage pending payload。

`SearchResultViewer` 将 `searchLoading` 拆为 `searchBusy` 和 `pageBusy`，各自有错误展示位置。Issue、File、Viewer 分别连接自己的状态和取消按钮；分页错误显示在当前结果的分页区域。搜索执行失败继续复用 `SearchExecutionStatus`，避免同时渲染两条相同错误。

当前 `runSearch()` 清 File/Viewer 状态的隐式副作用要显式裁定：允许通过 owner API 重置未执行的辅助草稿以保持操作习惯，但不得重置另一个正在执行的领域的状态、错误或取消它。Issue 搜索完成打开新 Tab 导致 Viewer scope 改变时，仍可按既有导航规则取消旧 Viewer 搜索；这是明确导航事件，不能由 Issue terminal effect 暗中执行。

兼容目标是搜索结果、表达式语义、Tab/权限/API 行为不变；校验期间显示忙碌、独立取消/错误，以及不再跨域禁用按钮，是本次有意修正的可见行为。

## 7. 文件和实施顺序

新增五个 controller hook 到 `frontend/src/features/files/hooks/`。视重复程度新增小型 intent 生命周期工具与纯结果映射模块；不预先创建大框架。

修改 `FilesView.tsx`、`SearchResultViewer.tsx`、`useSearchExecution.ts`、`api/client.ts`；必要时为 `useViewerTabs` 增加带 identity 的分页提交接口。`SearchExecutionStatus` 可由 controller 适配展示校验/等待阶段，不修改后端 status 类型。

1. **建立回归边界**：用 deferred promises 补齐真实状态转换、替换等待与分页乱序用例，保留当前已有 validation/clear/saved-search 回归。
2. **最小加强 execution 生命周期**：可选 intent signal、取消排队替换、scope/unmount 提交防护。确保使用同一 hook 的 `TempResultView` 兼容。
3. **独立 Viewer 分页**：解除与搜索 loading/error 的共用，落实每 Tab 请求身份和原子回写。
4. **Issue controller + Saved Search 执行桥**：两者同一阶段迁移，避免一段时间同时存在两个 generation owner。
5. **File / Viewer controller**：各自拥有编辑器和执行状态，明确跨域草稿操作和导航事件。
6. **Saved Search CRUD/dialog controller**：移走列表、保存、编辑、恢复流程；删除页面剩余搜索 ref、共享状态及跨域终态 effects。

各阶段保持可运行；不等到最后才统一接线。

## 8. 验证和验收

controller 测试用 deferred promises 精确控制成功和失败顺序；用真实 `useSearchExecution` 搭配网络 mock 验证关键生命周期。少量 FilesView 集成测试覆盖页面接线，不依赖三个实例共享静态 mock。

必须覆盖：

- A validation → B run / Saved Search；A 成功或失败都不再修改 B。
- validation pending → edit / clear / cancel / context change / unmount。
- A 正在取消 → B 等待替换 → clear/context change/unmount；B 不得 reserve/preview。
- 旧取消失败或超时：保留重试，禁止绕过停止确认启动新执行。
- Issue 与 Viewer 并行，双向交换成功、失败、取消顺序；各自状态和错误不被对方终态修改。
- 若成功打开 Tab 改变 active scope，按导航策略取消 Viewer；另设不触发导航的 controller 级并行测试验证隔离。
- File 搜索时切换文件，旧回调不打开结果、不写错误。
- 分页与搜索并行；同 Tab A/B 页乱序；不同 Tab 并行；关闭重开同 id；resultId 改变；context reset。
- 分页失败保留旧内容和 history；迟到 success/error/finally 都被丢弃。
- Saved Search 使用记录失败不改变搜索成功；对话框关闭后旧 validation/create/update/list 错误不回写。
- 卸载后无 React 状态写入、新搜索启动或残留 UI 定时器；有界服务端取消收尾单独验证。
- Guest、60 秒超时、括号/NOT Saved Search 往返、File/Viewer 关键词限制、Tab pin/关闭、source reveal、`TempResultView` 保持兼容。

实施后执行 `npm run lint`、`npm test`、`npm run build`（frontend 目录），再检查仓库 CI。本次仅设计，未执行上述测试，也未声称 CI 通过。

验收核心：没有跨领域可写的 loading/error；页面不拥有搜索 generation；同一 Tab 只有最新分页能提交；每个异步提交可指出唯一 owner、intent 和 scope；取消一个领域不改变其他领域状态。
