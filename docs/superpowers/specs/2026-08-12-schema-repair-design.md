# Skill 最终结果 schema repair 设计

## 目标

降低 Skill 最终结果首次输出触发 repair 的概率，同时保留现有 JSON schema、证据引用和 repair fallback 的安全约束。日志只记录可枚举的校验原因，不记录模型响应正文、技能提示词或日志内容。

## 方案

1. 将最终结果校验失败归类为稳定的 `validation_reason`：`invalid_json`、`missing_field`、`invalid_summary_status`、`invalid_evidence_reference`、`unsupported_claim`、`invalid_confidence`。
2. 首次校验成功记录 `final_result_validation=succeeded repair_used=false`；首次失败记录 `final_result_validation=failed validation_reason=... repair_attempt=1`。repair 成功或失败分别记录对应的安全字段。
3. 强化停止工具后的最终结果提示，明确固定顶层结构、状态和置信度枚举、EvidenceLedger 引用规则，以及只输出 JSON、不得 Markdown/额外文本/工具调用。
4. 保持 `json_object` response format 和现有 repair 流程，不放宽证据验证，也不改变 #106 的 provider retry 策略。

## 验证

- 单元测试覆盖 JSON、必填字段、状态、置信度、证据引用和 unsupported claim 的原因映射。
- 集成测试覆盖合法结果首次通过的日志和 repair 失败原因日志，并断言日志不包含模型响应正文。
