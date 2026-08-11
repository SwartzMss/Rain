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

## 设计

在 `backend/src/routes/skills.rs` 的 `SKILL_REVIEW_SYSTEM_PROMPT` 中增加四组明确约束：简体中文输出、Skill/Runner 能力边界、证据不足处理和客观停止条件。

`parse_review()` 同时承担确定性的入站校验：每条用户可见反馈至少包含一个汉字，但允许 Bluetooth、模块名和错误码等英文技术片段；suggestions 不得推荐未授权能力，不得在日志不完整时把未验证推断作为根因结论，也不得使用循环定义的停止条件。英文禁用词按独立词或短语匹配，避免误伤 `BT_PARSER_TIMEOUT` 一类标识符；明确要求删除、避免或禁止某工具的建议不视为推荐该工具。

增加聚焦单元测试，分别锁定 System Prompt、非法反馈拒绝和合法中英技术片段放行。测试先以失败状态加入，再补充最小实现使其通过。

## 错误处理与兼容性

本次变更不改变 API schema。模型首次返回不符合反馈契约的文本时，`parse_review()` 返回错误并进入现有一次 repair；repair 后仍不合规则返回 `SKILL_REVIEW_FAILED`，不会保存违规结果。无需迁移历史数据或客户端协议。

## 验证

- 运行新增的 Reviewer Prompt 单元测试。
- 运行反馈语言、能力边界、证据边界和停止条件的 parser 单元测试。
- 运行 `backend` 完整测试套件。
- 运行格式检查和 `git diff --check`。
- 检查变更只涉及设计文档、Reviewer Prompt 和对应测试。
