# Skill Reviewer 中文输出与诊断边界设计

## 背景

Rain 的 Skill Reviewer 已使用固定六维评分协议，但用户可见的 `warnings` 和 `suggestions` 可能返回英文，或建议 Skill 绑定 Runner 未提供的工具能力、弱化证据规则、使用不可客观判断的停止条件。

## 目标

- 要求所有 `warnings` 和 `suggestions` 使用简体中文。
- 建议只描述诊断意图与策略，不推荐 shell、grep、外部解析器、脚本、SQL、网络访问或其他未授权能力。
- 日志不完整时，要求明确缺失证据并把无法验证的判断保留为待验证假设，不得将其作为确定结论。
- 停止条件必须可客观判断，例如证据已足够、诊断问题已回答，或可用日志已耗尽但证据仍不足。

## 非目标

- 不修改 Reviewer JSON schema。
- 不修改英文 `grade` 枚举或六个英文 `dimensions` key。
- 不修改六维权重、总分计算或 grade 派生逻辑。
- 不增加运行时翻译或新的模型调用；仅复用现有的一次 repair。
- 不在 Rust 中实现通用自然语言理解，也不声称确定性 validator 能识别任意未授权工具、否定作用域或推理语义。

## 设计

在 `backend/src/routes/skills.rs` 的 `SKILL_REVIEW_SYSTEM_PROMPT` 中增加四组明确约束：简体中文输出、Skill/Runner 能力边界、证据不足处理和客观停止条件。

`parse_review()` 只承担确定性结构和文本契约校验。结构部分继续检查 JSON schema、固定 dimensions、分数、总分、反馈数量和长度。文本部分要求每条反馈至少包含 Han 字符，并使用 `zhconv` 内置的 OpenCC 数据转换到 `ZhHans`；转换前后不一致时进入 repair。

英文正文检测不再使用可被前置中文稀释的全局比例。validator 按 Han 字符切分 ASCII 语言片段，并统计每个片段中的普通英文词；带数字、下划线、点、斜杠、冒号的 identifier-like token 和全大写 acronym 不计为英文正文。任一片段出现两个或以上普通英文词时拒绝，因此 `Bluetooth`、`HCI_TIMEOUT`、`com.android.bluetooth` 等孤立技术标识可以保留，而 `Clarify the Bluetooth failure scope.` 会稳定进入 repair。单个无法区分是术语还是正文的 ASCII token 不作语义猜测。

suggestion 只保留一层无歧义的字面限制：出现 `grep`、`shell`、`parser`、`script`、`SQL`、`curl`、`network access`、`network request`、`解析器`、`脚本`、`网络访问` 或 `网络请求` 等具体且稳定的禁用能力名时直接拒绝，不判断该字面量处于推荐还是否定语境。需要表达“删除 grep”的模型输出可在 repair 时改写为“删除未授权能力”。`外部工具`、`第三方工具`、`工具` 和 `命令` 属于普通关系词，不作为 context-free denylist；“保持建议与具体工具无关”这类合规表述可以通过。这条规则只保证列出的具体字面量不会落库，不扩展成任意工具识别器。

删除 invocation/object 切片、否定词作用域、跨句 inference-to-conclusion 和循环停止条件等自然语言规则。任意未知工具（例如 `awk`）是否被推荐、日志补齐和验证是否足以支持后续结论、停止条件是否客观可判断，都属于开放式语义，由 System Prompt 约束并明确视为 best-effort。这样避免 parser 在 false negative 与 false positive 之间不断增加词表和例外，也允许 `日志不完整时先保留待验证假设；补齐缺失日志并验证后再形成根因结论。` 这类安全建议通过确定性契约。

聚焦单元测试锁定责任边界：混入多词英文正文、繁体文本和具体禁用能力字面量必须拒绝；孤立技术标识、安全 Evidence Policy 以及泛化的工具无关表述必须接受；未知 `awk` 语义不由 `parse_review()` 分类。System Prompt 测试继续确保能力、证据和停止条件要求不会从模型契约中消失。

## 错误处理与兼容性

本次变更不改变 API schema。模型首次返回不符合确定性结构或文本契约时，`parse_review()` 返回错误并进入现有一次 repair；repair 提示会重申简体中文和禁用字面量要求。repair 后仍不合规则返回 `SKILL_REVIEW_FAILED`，不会保存违规结果。无法由确定性规则判断的开放式语义只受 System Prompt 约束，可能被保存，因此不再把它描述为强保证。无需迁移历史数据或客户端协议。

## 验证

- 运行新增的 Reviewer Prompt 单元测试。
- 运行简体文本、英文正文片段、禁用字面量和 parser 责任边界单元测试。
- 运行 `backend` 完整测试套件。
- 运行格式检查和 `git diff --check`。
- 检查旧的 invocation、否定和证据自然语言规则已删除，没有新增同类关键词语法引擎。
