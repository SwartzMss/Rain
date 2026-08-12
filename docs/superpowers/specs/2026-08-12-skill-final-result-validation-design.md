# Skill Final Result Validation Design

## Goal

让 Skill final result 的校验失败能安全、稳定地定位到可行动的原因和字段，并在明确配置 Provider 支持时使用 strict JSON Schema structured output，同时保留现有 EvidenceLedger 与公开错误语义。

## Scope

本设计只覆盖 `SkillRunner` 的 final result / result repair：校验原因、allow-listed `validation_field`、定向 repair prompt、strict JSON Schema 请求和兼容 fallback。Tool contract、Provider retry/backoff、EvidenceLedger 证据合法性和数据库公开错误码不变。

## Architecture

`validate_result` 先解析 JSON，再由显式 shape/type validator 检查顶层对象、嵌套对象、字段类型、枚举和未知字段。只有通过 shape/type 检查后才调用 `serde_json::from_value` 构造 `SkillRunResult`；因此 serde 解析失败不会再统一映射成 `missing_field`。

校验失败由一个安全错误值表示：

```text
reason: ResultValidationReason
field: Option<ValidationField>
```

`ResultValidationReason` 至少包含 `invalid_json`、`missing_top_level_field`、`missing_nested_field`、`unknown_field`、`invalid_field_type`、`invalid_summary_status`、`invalid_confidence`、`empty_required_text`、`invalid_array_size`、`invalid_missing_context`、`invalid_evidence_reference`、`unsupported_claim` 和 `result_too_large`。`ValidationField` 使用静态 allow-list，不接受模型提供的任意 JSON key/path；无法安全定位时为 `None`。

校验顺序为：JSON 语法 → 顶层/嵌套 shape → 字段类型/枚举 → 业务约束 → EvidenceLedger 引用。业务约束失败仍保持 `SKILL_RESULT_INVALID` 或 `SKILL_EVIDENCE_INVALID` 的现有映射；日志额外记录 `validation_reason` 和可选 `validation_field`，不记录原始结果、serde 错误或日志正文。

## Provider response format

新增 `RAIN_AI_STRUCTURED_OUTPUT` 配置，取值为 `json_schema` 或 `json_object`，默认 `json_object`。该能力模式随 resolved provider 传入 `OpenAiChatClient`，由客户端暴露给 Skill Runner。工具调用请求继续使用 `response_format: null`。

当模式为 `json_schema` 时，final model request 和 result repair request 使用固定的 `skill_run_result` strict schema：顶层和嵌套对象均使用 `additionalProperties: false`，声明所有 required 字段，限制 summary status、inference confidence、数组数量及可表达的字符串长度。当模式为 `json_object` 时，两类请求继续发送当前兼容格式。不会因为普通 HTTP 400 自动尝试另一种格式，避免掩盖真实请求错误。

## Repair behavior

repair prompt 根据安全校验错误生成定向指令，例如：

```text
The previous result omitted the required top-level field `evidence`.
Return all required top-level fields exactly once.
```

或：

```text
`summary.evidence_ids` must be an array of strings.
```

只有固定字段名和固定提示文本会进入 prompt。repair 后仍失败时保留现有公开错误码和用户提示。

## Testing

- 单元测试覆盖每类 validation reason、field 映射、unknown field 脱敏、类型/长度/语义约束和过大结果。
- Runner 集成测试覆盖首次缺字段后定向 repair 成功、repair 仍失败的错误码、strict schema 与 json_object 请求格式，以及两条路径都执行服务端 evidence 校验。
- 配置和客户端测试覆盖默认 fallback、合法/非法能力配置和 strict response format 结构。
- 运行 `cargo fmt --all -- --check`、`cargo test`、`npm run build`、`git diff --check`；并单独复跑已有并发敏感 smoke 测试，避免并行执行造成误报。

## Security invariants

不得放宽 `deny_unknown_fields`、EvidenceLedger 引用校验或 unsupported-claim 检查；不得记录完整模型输出、Issue 日志正文、原始 serde 错误或未经 allow-list 的字段名。
