# Skill Runner Tool Call 错误恢复与可观测性设计

## 背景

Skill Runner 对四个只读 Tool 使用严格 schema，但此前会先批量解析整轮 Tool Calls；任一调用解析失败都会让整个 Run 以 `SKILL_TOOL_FAILED` 结束。执行阶段除 retrieval limit 外也会把不同 `AppError` 折叠成同一失败，且缺少 iteration、调用序号、错误阶段和安全参数摘要。

## 目标

- 模型可修正的 parse 或 execute 错误通过对应 `tool_call_id` 返回结构化 Tool Result，不立即终止 Run。
- 所有模型请求的调用继续计入既有 24 次 Tool Call 预算。
- 连续错误有小上限，避免模型无限重试。
- parse、execute 和最终结果校验日志可定位失败阶段，但不记录查询正文、路径、凭据或原始参数。
- 数据库、Blob、I/O 等平台故障仍立即终止，并使用明确的公开错误码。

## Tool Call 校验

`parse_tool_call()` 返回带分类的 `ToolCallValidationError`：

- `INVALID_ENVELOPE`
- `UNKNOWN_TOOL`
- `INVALID_JSON`
- `UNEXPECTED_ARGUMENT`
- `MISSING_ARGUMENT`
- `INVALID_ARGUMENT`

Tool 名只记录四个允许名称；其他名称统一记为 `unknown`。参数摘要按 Tool 白名单生成：查询和路径只记录字符数，`read_file_lines` 只记录数值字段，未知 Tool 或无效 JSON 只记录参数字节数。原始参数不会写入日志或步骤。

合法 `tool_call_id` 的校验错误作为以下受控结果返回模型：

```json
{
  "error": "INVALID_TOOL_CALL",
  "category": "INVALID_ARGUMENT",
  "tool": "read_file_lines",
  "message": "read_file_lines requires a positive file_id, 0 <= start <= end, and at most 200 lines"
}
```

无效或缺失的 `tool_call_id` 无法安全满足 assistant/tool 消息配对协议，因此归为不可恢复的 `SKILL_TOOL_PROTOCOL_INVALID`。

## 执行错误

执行阶段按 `AppError` 分类：

- 普通 `BadRequest`、当前 Run 内不可用资源、目录和非文本文件：返回 `TOOL_EXECUTION_ERROR`，步骤状态为 `FAILED`，允许模型修正。
- retrieval byte/range/output limit：保持 `RETRIEVAL_LIMIT`，进入既有强制总结流程。
- Database、I/O：终止为 `SKILL_TOOL_STORAGE_ERROR`。
- 其他平台异常：终止为 `SKILL_TOOL_EXECUTION_ERROR`。
- 启动阶段自动获取的 Issue Manifest 复用同一分类与脱敏日志，但仍不计入模型的 24 次 Tool Call 预算。

对模型和日志只使用固定安全原因，不转发 `AppError` 内部文本、SQL、堆栈或物理路径。

## 迭代与预算

Runner 不再对整轮调用执行 fail-fast `collect()`，而是按 assistant 返回顺序逐个处理，并为每个调用追加匹配的 Tool response。parse 拒绝步骤记录为 `REJECTED`，execute 可恢复错误记录为 `FAILED`；成功调用会重置连续错误计数。

运行事件分别使用 `tool.rejected` 和 `tool.failed`，前端与既有 `tool.started` / `tool.completed` 一样监听并刷新 Run 状态。

连续 3 个可恢复 Tool 错误后停止 Tool 使用，为同轮剩余 `tool_call_id` 补齐 `INVALID_TOOL_CALL_LIMIT` 响应，并进入无 Tool 的强制总结请求。该机制不修改 8 iterations、24 Tool Calls 或 retrieval budget。

## 结构化日志

Tool 错误日志包含：

- `run_id`
- `iteration`
- `tool_call_index`
- 全局 `tool_call` 序号
- 安全 `tool`
- `error_stage=parse|execute`
- `error_category`
- `arguments_summary`
- 固定安全 `reason`
- 连续错误计数（可恢复错误）

最终结果先区分 JSON 语法、schema/约束和 Evidence 校验，并以 `result_validation_stage=parse_json|schema|evidence` 记录首次失败与 repair 后失败。模型正文和证据内容不进入日志。

## 持久化与兼容性

本次不修改数据库 schema 或 API。`skill_run_steps.status` 已是开放文本字段，直接新增 `REJECTED` 和 `FAILED` 状态；`arguments_summary` 继续保存有界、非敏感元数据。已有成功、预算耗尽和取消行为保持兼容。
