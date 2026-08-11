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
- 不增加运行时翻译、输出过滤或新的模型调用。

## 设计

在 `backend/src/routes/skills.rs` 的 `SKILL_REVIEW_SYSTEM_PROMPT` 中增加四组明确约束：简体中文输出、Skill/Runner 能力边界、证据不足处理和客观停止条件。现有请求、解析和持久化数据流保持不变。

增加一个聚焦的单元测试，逐项断言 System Prompt 包含上述约束。保留已有 rubric 测试，以共同证明评分维度映射和新增输出边界同时存在。测试先以失败状态加入，再补充 Prompt 使其通过。

## 错误处理与兼容性

本次变更只调整模型系统提示，不改变 API 或本地解析规则。模型若仍返回不符合语义的文本，现有 JSON 解析与修复流程保持原样；本 Issue 不引入新的运行时拒绝策略，因此不存在对历史数据或客户端协议的迁移。

## 验证

- 运行新增的 Reviewer Prompt 单元测试。
- 运行 `backend` 完整测试套件。
- 运行格式检查和 `git diff --check`。
- 检查变更只涉及设计文档、Reviewer Prompt 和对应测试。
