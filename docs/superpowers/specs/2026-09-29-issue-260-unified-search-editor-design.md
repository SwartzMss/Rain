# #260：统一详细搜索编辑器设计

状态：设计提案；本次仅提交设计文档，不修改运行时代码。

关联：[#260](https://github.com/SwartzMss/Rain/issues/260)，承接 #253 的完整表达式能力。

代码基线：2026-09-29 拉取的 `origin/main`，`524a602e3e4fef1d836a6ac9230a49d685c7e9c2`（`test: expect split saved search actions`）。下述“现状”均以此提交为准。

## 1. 目标与范围

将 Issue 详细搜索和保存条件编辑弹窗统一为支持 `AND / OR / NOT / ( / )` 的 token editor，删除简单/高级模式及两套表达式状态。保留 term chip 编辑、删除、键盘操作、“我的搜索条件”“保存条件”两个独立入口，以及访客搜索和登录后恢复保存流程。

表达式由 `tokens + draft` 唯一表示；`query_text` 是派生的提交值和持久化来源。后端 `log_expression` 继续决定最终语义、复杂度和资源限制。不修改 DB schema、搜索执行协议、索引实现或布尔运算优先级。

共享组件的其他使用点必须回归验证，但本 issue 不要求为单文件搜索、结果内搜索等入口新增工具栏或改变搜索范围。不得把“文件名搜索/详细搜索”的业务选择误删成简单/高级表达式模式。

## 2. 代码现状与影响

| 文件 / 入口 | 已核对现状 | 设计影响 |
| --- | --- | --- |
| `frontend/src/features/files/searchTokens.ts` | 只有 term/operator；反序列化拒绝括号；简单模式只接受 term 起始和 AND NOT；删除逻辑按平铺 token 推断 | 增加 paren、统一词法与前缀检查，重写删除规则 |
| `SearchTokenEditor.tsx` | draft 整体作为关键词；连续关键词补 AND；运算符按钮只在 term 后出现；AND NOT 是组合按钮 | 保留关键词输入习惯，新增独立 NOT 和括号，统一 mutation |
| `SearchExpressionEditor.tsx` | mode tabs + textarea/token editor 分支 | 保留组件作为单一编辑器及提示的外壳，删除 mode/raw props |
| `FilesView.tsx` | `detailRawExpression` / `editingRawExpression` 用 null 区分模式；手动搜索和保存仅 raw 分支先调用校验；使用保存条件直接执行 | 所有详细搜索提交共用 finalize/serialize/validate 管线 |
| `pendingSavedSearch.ts` | 要求 options 非空，且可选 options.tokens 必须合法 | 取消冗余 tokens 对恢复的决定权，接受空 options |
| `frontend/src/api/types.ts` | SavedSearchPayload.options 是必需的通用对象 | 保留 API 字段，不为本 issue 改协议 |
| `backend/src/log_expression.rs` | NOT > AND > OR，支持递归 NOT、括号、短语、转义；没有隐式 AND | 前端自动 AND 仅用于交互，不能在导入旧 DSL 时擅自补齐 |
| `backend/src/routes/search_expressions.rs` | OptionalUser；失败返回带位置的 `SEARCH_EXPRESSION_INVALID` | 访客可用；保留原始服务端错误信息 |
| `backend/src/routes/saved_searches.rs` | create/update 已 parse query_text；仅要求 options 是对象 | 无需 DB migration；前端预校验不取代后端写入校验 |
| `useSearchExecution.ts` / FilesView generation | hook 负责执行替换与取消；页面 generation 保护部分异步回调 | 新增统一 validation 后必须把保护延伸至执行前阶段 |

## 3. 数据模型与职责

```ts
type SearchToken =
  | { kind: 'term'; value: string }
  | { kind: 'operator'; value: 'AND' | 'OR' | 'NOT' }
  | { kind: 'paren'; value: '(' | ')' };

type SearchEditorState = { tokens: SearchToken[]; draft: string };
```

每个编辑器实例拥有独立 state。主搜索与“编辑保存条件”弹窗独立是必要的编辑事务，不是同一表达式的两套表示。焦点、chip 局部编辑值、错误和请求编号属于交互状态，不是另一份已提交表达式。

在 `searchTokens.ts` 集中提供以下能力，具体导出名可随实现调整：

- `deserializeSearchTokens(query)`：与后端匹配的 lexer + 完整结构检查；不构造语义 AST。
- `analyzeSearchTokens(tokens)`：一次扫描，返回合法前缀/完整表达式、期待 operand/operator、括号栈及首个结构错误。
- `applySearchEdit(state, action)`：添加 term/语法、替换、删除的纯函数；组件不重复拼 token。
- `finalizeSearchTokens`：提交 draft 后要求完整表达式，不自动补右括号或删尾部操作符。
- `serializeSearchTokens`：仅接受完整表达式；term 统一引用并转义，syntax 直接输出。

不持久化 AST、括号 balance 或按钮状态；全部由 tokens 派生。删除 `SearchExpressionMode`、两组 mode change handler、两组 raw state 和所有 simple representability helper。

## 4. 编辑状态、输入与按钮

### 4.1 合法前缀与可执行表达式

允许 `A AND (`、`NOT`、`(A OR` 等尚未完成的合法前缀，展示“请添加关键词”或“还需闭合 N 个括号”，搜索/保存不可提交。禁止 UI 添加无左括号的 `)`、空 `()`、起始 AND/OR 等结构错误。

扫描器维护 `expectOperand` 和左括号栈：term 完成 operand；NOT 保持期待 operand；左括号入栈；右括号仅在已有 operand 且栈非空时出栈；AND/OR 仅在 operand 后转入期待 operand。最终非空、非期待 operand、栈为空才完整。

后端允许 `NOT NOT A`，前端同样允许，避免旧数据能力回退。这是对 issue 建议中“NOT 后只允许 term/(”的明确调整；仍受后端 nesting limit 限制。

### 4.2 工具栏规则

五个按钮始终显示，不能执行时 disabled，并有对应可访问名称。

| 当前尾部 | 添加 term | AND / OR | NOT | `(` | `)` |
| --- | --- | --- | --- | --- | --- |
| 空 / `(` / AND / OR / NOT | 直接添加 | 禁止；尾部 AND/OR 可替换 | 直接添加 | 直接添加 | 禁止 |
| term / `)` | 先补 AND | 直接添加 | 先补 AND | 先补 AND | 仅栈非空时允许 |

自动 AND 只用于“添加 operand”的显式交互，统一覆盖 term、NOT、左括号。例如 `A` 后点 NOT 得到 `A AND NOT`，`A OR` 后点 NOT 得到 `A OR NOT`。已存在 AND/OR chip 可相互替换，包括 AND NOT 中的 AND；NOT/paren 不响应运算符切换。

点击语法按钮时若 draft 非空，先把 draft 作为一个 term 提交，再基于提交后的状态判定按钮并原子提交两步；不允许先清空 draft 再发现语法操作失败。按钮状态按这一候选状态计算。

### 4.3 关键词输入与表达式导入

默认输入框仍是关键词输入：`disk full`、`func(x)`、`AND` 都作为单个 term，避免改变已有日志关键词的含义。输入框 Enter/Tab 或加号提交一个 term；空 draft 的 Enter 可提交搜索；中文输入法 composition 期间 Enter 不提交。

为承接原 textarea 的复制粘贴能力，同一输入框增加明确的“按表达式添加”动作；它把当前 draft 作为完整 DSL 解析并转换为 chips，没有模式切换、独立 textarea 或长期 raw state。成功后清空 draft；失败保留原文和原 tokens，显示错误。空编辑器直接导入；非空编辑器将导入表达式包为一个括号组，再按 operand 规则追加，避免 OR 优先级改变原有表达式。该导入动作要求完整表达式；逐步搭建使用五个按钮。

term chip 双击/点击沿用现有编辑行为；Enter/失焦提交，Escape 取消；空值提示并保留原 term，删除走明确删除动作。提交搜索或保存前必须先同步活动 chip 的编辑值，避免点击按钮引发 blur 后读到旧 React state。建议让局部编辑提交通过统一 action 返回新的快照，不依赖连续 setState 后立即读取。

保留左右键移动 chip 焦点、Delete/Backspace 删除、空 draft Backspace 删除最后一项；删除后焦点移到前一个仍存在的 chip，否则回到输入框。括号分别提供“左括号”“右括号”名称。

## 5. 删除规则：结构化删除，不猜测用户意图

不能直接复用当前按相邻 operator 删除的算法。利用括号配对表和同层 token 边界定位 operand 区间，无需计算布尔语义 AST。operand 区间包含其前缀 NOT 链以及 term 或完整括号组。

1. 删除 NOT：仅删选中的 NOT；允许从 `NOT NOT A` 变为 `NOT A`。
2. 删除 term：连同它直属的 NOT 链删除；优先删除同层左侧 AND/OR，没有左侧则删除同层右侧 AND/OR。不得跨越括号边界吞掉另一层 operand。
3. 删除 AND/OR：沿用现有“删除连接符及右侧 operand”的行为，但右侧必须识别整个 NOT 链/括号组；尾部连接符只删除自身。工具提示明确“删除运算符及右侧条件”。
4. 删除已配对的任一括号：同时移除该对括号，保留内部 token，提示“移除这对括号（保留内容）”。这是用户主动改变分组，可能改变优先级；不宣称语义不变。例如 `A AND (B OR C)` 变为 `A AND B OR C`。
5. 删除尚未配对的左括号：移除该符号；对受影响的空尾部 NOT/连接符按下述清理规则处理。
6. 删除导致空组：递归删除空组及直属 NOT 链，再按规则 2 移除同层连接符。只清理本次删除制造的空结构，不自动修复任意导入错误。
7. 合法前缀中的尾部不完整 operand（例如 `A AND NOT (`）：定位右侧区间时可取至流尾。删除末尾左括号可保留 `A AND NOT`，它仍可继续输入；不必为了完整性误删 A。

每次 mutation 后必须通过合法前缀检查；若实现无法产出合法前缀，原子拒绝操作并保留原值，不能静默丢掉后续 token。完整输入的常规删除应按上述规则得到完整或空表达式；不完整输入允许仍是可继续编辑的前缀。

| 操作 | 结果 |
| --- | --- |
| `A AND (B OR C)` 删除 B | `A AND (C)` |
| `A AND (B)` 删除 B | `A` |
| `(A OR B) AND NOT C` 删除 C | `(A OR B)` |
| `A OR NOT (B AND C)` 删除 OR | `A` |
| `NOT (A OR B)` 删除任一括号 | `NOT A OR B`（明确取消分组） |
| `A AND (` 删除 `(` | `A AND`（可继续输入） |

## 6. 词法、无损与后端一致性

“无损”指搜索语义和 term 内容保持一致，不要求空格、操作符大小写或是否加引号逐字相同。序列化不消除括号，不折叠 NOT，不在前端执行 case folding。

- 未引用 token 以空白或括号结束，支持 `ping AND(error OR timeout)`。仅 ASCII 大小写不敏感的独立 AND/OR/NOT 为操作符；引号中的 `"AND"` 是 term。
- 引用短语解码 `\"` 和 `\\`；未知 escape 保留反斜杠，不能把 `\n` 变成换行。未闭合引号或尾部转义报错。
- 与后端保持短语首尾 trim、空短语拒绝；Unicode、emoji、路径、内部空格保留。空白识别需对齐 Rust `char::is_whitespace`，不要假设 JS `\s` 的字符集合完全相同。
- 后端未引用词中途出现的引号属于词内容；前端 lexer 要按后端实际边界处理，不能只用通用正则替代。
- DSL 导入/加载时 `A B` 必须报错；自动 AND 不属于后端 grammar。
- 序列化所有 term 时转义反斜杠和双引号；确保 `deserialize(serialize(tokens))` 相同，原始 DSL 与规范化 DSL 在后端匹配结果一致。

后端当前限制为 4096 Unicode 字符、16 KiB、128 tokens、32 层 nesting、128 AST nodes，NOT 也占 nesting。前端结构检查不复制整套限额算法；最终服务端校验拒绝超限。若添加输入长度提示，应针对最终序列化值计算，不能使用 JS string.length 代替 Unicode 字符数或把所有限制当作括号深度。

服务端现有错误是包含人类可读位置的消息，并未提供结构化 offset；本次原样展示，不从中文消息反解析 chip 下标。前端结构错误可直接关联本地 token。

## 7. 统一提交与异步生命周期

```text
编辑器当前快照（含活动 chip 编辑）
  → finalize(tokens, draft)
  → serialize 得到 immutable query_text
  → validateSearchExpression(query_text)
  → 搜索 execute / createSavedSearch / updateSavedSearch
```

手动详细搜索、“使用”保存条件、创建保存条件、修改保存条件全部走后端校验。对同一次操作，只生成一次提交字符串；校验和后续 API 使用完全相同的值，不能 await 后重新从可变 state 组装。

### 7.1 搜索

复用 `searchRequestGenerationRef` 作为页面搜索意图编号：在 validation 前递增，捕获 issueCode 和提交快照。每次 await 后、错误处理、finally、打开结果 tab、标记保存条件已使用前均检查 generation 与页面仍挂载。

- 新搜索/使用另一保存条件、清空、切换 issue 或业务搜索类型、取消、卸载必须使旧 validation 失效。
- validation 期间修改 tokens/draft 或开始编辑 chip 也使该待提交请求失效，防止用户看到新条件却执行旧条件。无需因为修改草稿而取消一个已正式启动的快照搜索。
- validation 期间显示“校验中”及取消入口；取消首先失效页面 generation，再调用现有执行 hook 的 cancel（如已有运行）。目前只调用 hook.cancel 的按钮不足以取消尚未 execute 的 validation。
- 校验成功且仍当前才调用 `issueSearchExecution.execute`；保留 hook 对旧执行的取消和替换协议，不能绕过 hook 直接调用 preview。
- 校验 pending 状态与 hook RUNNING 状态分开派生，旧请求不能清掉新请求 loading。校验失败或取消保留现有结果。
- 校验前发起新意图时，旧执行回调必须立即失效；旧执行的实际停止继续由现有 cancel/execute 协议负责。不得让旧成功回调在新校验期间打开 tab。

validation 可以通过扩展 client 的可选 AbortSignal 节省无用请求，但 generation 检查是正确性必需条件，不能以 AbortController 代替。无需改服务端 validation API。

### 7.2 保存

保存拥有独立的提交编号和弹窗会话身份，不复用搜索 generation。一次保存捕获名称、query_text、options、记录 ID 和 is_pinned；validation 前锁定当前保存表单、禁用重复保存，关闭弹窗使尚未写入的请求失效。校验返回后确认会话仍有效才发起写请求。

写请求发出后不保证关闭弹窗能撤销服务端写入；完成回调只更新对应会话，不能关闭后来打开的另一条记录。失败保留编辑内容。列表刷新不得把旧错误写入新弹窗。

## 8. Saved Search 与登录前暂存

新建记录仍使用现有 payload，例如：

```json
{
  "name": "网络错误",
  "search_type": "DETAIL",
  "query_text": "\"ping\" AND ( \"error\" OR \"timeout\" )",
  "options": { "version": 1 }
}
```

不再写 `editor_mode` 或 `tokens`。options/version 保留以兼容现有 API，不代表恢复表达式需要它们。更新旧记录时去掉这两个旧字段，保留其他未知 options；不批量改数据库记录。

加载只从 query_text 反序列化；忽略旧 editor_mode 和旧 tokens，即使冗余 tokens 不合法或与文本冲突，也不能覆盖有效 query_text。加载后“使用”走同一提交管线，成功且当前时再调用 markSavedSearchUsed。

`takePendingSavedSearch` 保留 DETAIL、非空字符串、options 对象等 payload 安全检查，但接受 `{}`，删除 optional tokens 的语法校验及“对象不能为空”约束。非登录状态不消费暂存；登录后只消费一次，恢复 tokens 并打开保存弹窗，不自动创建记录；确认保存时再后端校验。

若异常旧 query_text 不能解析，展示“无法加载该搜索条件”及原文本只读诊断，保留已有编辑器内容与数据库记录，不退回 raw 模式，不以空 tokens 覆盖旧记录。登录恢复失败也展示诊断，允许重新输入；不悄悄丢弃失败原因。诊断文本不是可执行表达式的第二份编辑 state。

## 9. 实施顺序与文件范围

1. `searchTokens.ts`：paren 类型、词法、前缀检查、mutation、round-trip；先用边界测试锁定协议一致性。
2. `SearchTokenEditor.tsx`：固定工具栏、NOT/括号、统一原子 action、表达式导入、焦点与活动 chip 提交。检查其他调用方的 allowOperators 行为。
3. `SearchExpressionEditor.tsx`：删除模式 UI/props 和所有表达式能力说明文案，只保留统一 token editor。
4. `FilesView.tsx`：删除模式状态和 helper；收敛四类提交；完善 validation 阶段取消、乱序和保存会话保护。
5. `pendingSavedSearch.ts`：以 query_text 恢复；清理旧元数据读写；`api/types.ts` 通用 options 可保持不变。若采用 validation AbortSignal，仅扩展 `api/client.ts` 可选参数。
6. 更新现有测试中 simple/advanced 的旧断言，补充统一编辑器行为；保留“我的搜索条件/保存条件”分离入口断言。后端生产代码原则上无需修改，仅补必要的一致性用例。

拒绝只增加括号按钮的局部实现：它不能解决存量表达式、删除和异步提交问题。也不采用长期 raw 字符串与 token 双向同步，避免重建两套 source of truth。

## 10. 验证与验收

| 层次 | 必须验证 |
| --- | --- |
| grammar | issue 列出的全部合法/非法表达式；嵌套括号；NOT NOT A；NOT (A OR B)；A OR NOT B；前缀可编辑但不可提交 |
| mutation | 每个状态下五按钮；draft + 按钮原子提交；term/NOT/paren 自动 AND；AND NOT 中切换 AND/OR；上述全部删除示例；嵌套空组递归删除；连续删除至空 |
| 词法与 round-trip | 紧邻括号、短语、引号/反斜杠/未知转义、未引用词内引号、Unicode/emoji/空白边界、字面 AND、带括号的日志关键词、A B 拒绝 |
| UI | 无简单/高级 tabs；同一编辑器完整构造两条验收表达式；输入法、键盘焦点、Escape、失焦提交、活动 chip 后直接搜索/保存、失败导入保留原状态 |
| Saved Search | 旧 simple/advanced/无 mode 记录；旧冗余 tokens 无效或冲突；options={}；新建重载编辑；旧记录重存去掉 mode/tokens；未知 options 保留；异常文本诊断 |
| Guest | 未登录可校验和 preview；暂存不提前消费；登录后恢复括号表达式并再次保存；不误自动执行/自动保存 |
| async | 校验乱序；校验失败；校验时修改/清空/切 issue/取消/卸载；手动与 saved 使用相互替换；旧 finally 不清新 loading；旧错误不盖新状态；保存关闭重开/重复点击/网络失败 |
| 后端一致性 | 同一 DSL 与其序列化值在代表性日志上的匹配一致；128 token 与 nesting 等限额仍由后端拒绝；访客校验及 saved create/update parse 不回归 |

主要测试位置：`frontend/tests/search-tokens.mjs`、`search-ui.mjs`、`pending-saved-search.mjs`、`files-view-search.behavior.test.tsx`，按需要新增 SearchTokenEditor 的真实交互测试；执行生命周期还需回归 `search-execution.behavior.test.tsx`。不要只用源代码字符串断言替代用户操作测试。

实现完成后执行：

```text
cd frontend
npm ci
npm test
npm run build
npm run lint

cd ../backend
cargo test log_expression
cargo test routes::search_expressions
cargo test routes::saved_searches
```

验收以 UI 可完成 `ping AND (error OR timeout)` 与 `NOT (error OR timeout)`、旧条件恢复后语义不变、统一服务端校验和取消竞态测试通过为准。本次文档交付仅核对源码与设计自洽性；上述是后续实现的验证要求，不代表已经执行或通过。
