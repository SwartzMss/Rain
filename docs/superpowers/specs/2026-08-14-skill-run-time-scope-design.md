# Skill Run Time Scope Design

## Goal

为 Skill Run 增加可选的故障时间范围，使一次诊断可以持久化并安全地使用 Primary Incident Window，同时保持未填写时间范围时的现有行为。

## Scope

本次实现覆盖：

- 前端“不限制时间 / 指定故障时间 / 指定时间范围”交互；
- Create Skill Run API 的 `time_scope` 输入、RFC3339 校验、开始时间早于结束时间校验和最大窗口限制；
- Skill Run 持久化、查询和 SSE snapshot 返回时间范围；
- Runner 的 Trusted Run Scope 注入；
- 日志索引中的标准化事件时间范围和 `search_logs` 的服务端主窗口过滤；
- 受控的边界上下文扩展；
- 前后端回归测试和旧数据库启动兼容。

不修改 SKILL schema、EvidenceLedger、Final Result contract 或输出语言策略，也不实现其他 Run Scope。

## Decisions

### Time scope representation

客户端提交：

```json
{
  "skill_id": "skill-id",
  "time_scope": {
    "start": "2026-08-14T01:27:15.000Z",
    "end": "2026-08-14T01:37:15.000Z"
  }
}
```

`time_scope` 为 `null` 或省略时表示不限制时间。服务端只接受 RFC3339 时间，统一规范化为 UTC RFC3339 文本，同时保存 epoch milliseconds 供搜索过滤。单次窗口最大为 24 小时；超过、格式非法或 `start >= end` 返回稳定的 `INVALID_TIME_SCOPE` 400 错误。

Run 记录保存 `analysis_start_time`、`analysis_end_time` 以及内部的毫秒边界。时间范围属于创建时的 Run snapshot，后续前端状态改变不会影响已创建 Run。

### Log event time index

当前 `log_segments.timeline` 固定为 `all`，不能用于比较时间。因此在 `log_segments` 增加可空的 `event_time_start_ms` 和 `event_time_end_ms`。索引器在构建每个 segment 时从带有明确日期和时区的常见 RFC3339/ISO-8601 行首时间戳中提取最小和最大时间；无法解析的行不臆测时区，保持为空。一个 segment 可以包含多行，因此 segment 的时间范围是候选过滤范围，最终事实仍由 `read_file_lines` 验证。

旧数据库启动时通过幂等 schema ensure 补列，并对已有 segment 做一次 best-effort 回填；无法解析的历史内容保持空值。无时间范围的搜索完全不增加时间条件，兼容既有数据和行为。

### Server-bound search scope

`SkillRunContext` 持有不可由模型修改的时间范围。`search_logs` 不接受任意 start/end 参数；执行器自动把 Run 范围加入 SQL：segment 时间范围与主窗口相交才可作为候选。未设置范围时不追加条件。

模型可以请求一个有限的边界上下文扩展，但只能通过固定上限的 `context_expansion_minutes` 参数，服务端限制为最多 15 分钟，并同时向前后扩展主窗口。该参数不能覆盖或替换原始 Run scope。工具响应包含实际应用的范围，便于诊断和审计。

### Runner prompt

有时间范围时，在 Trusted system message 中注入：

```text
Primary incident time range: 2026-08-14T01:27:15.000Z through 2026-08-14T01:37:15.000Z.
Prioritize events inside this window. You may request only bounded context near its edges when needed for causality. Do not associate an identical message from another time solely by keyword.
```

时间范围不进入 `USER SKILL INSTRUCTIONS`。无范围时继续只注入当前 Issue 的 scope 文案，不引入时间语义。

### Frontend interaction

默认选择“不限制时间”。“指定故障时间”使用 `datetime-local`、故障前分钟数和故障后分钟数，前端转换为浏览器本地时间对应的 UTC RFC3339 范围。“指定时间范围”使用开始和结束 `datetime-local`。运行期间这些控件和 Skill 选择保持禁用。Run 状态和结果区域展示保存的分析范围；空范围显示“不限制时间”。

## Data flow

```text
UI time mode
  -> useSkillRun(start payload)
  -> POST /issues/:issue/skill-runs { skill_id, time_scope }
  -> validate + canonicalize + persist Run snapshot
  -> SkillRunner loads snapshot
  -> Trusted prompt + SkillRunContext(time scope)
  -> SkillToolExecutor.search_logs applies bounded SQL scope
  -> read_file_lines verifies original lines/evidence
  -> Run API/SSE returns immutable analysis range
```

## Error handling

- Invalid timestamp, missing endpoint, equal/reversed endpoints, or window over 24 hours returns `INVALID_TIME_SCOPE` and does not enqueue a Run.
- Indexing failures do not fail the upload solely because a line timestamp is unparseable; the event-time columns remain null.
- A scoped search excludes segments with no normalized time. The tool response reports the applied scope and whether no time-indexed candidates were available; the model can produce `INSUFFICIENT_EVIDENCE` rather than silently treating unrelated global hits as incident evidence.
- Database schema extension is idempotent and runs before normal request serving. Existing unscoped Runs remain valid with null scope.

## Testing strategy

- Unit tests for timestamp extraction, RFC3339 canonicalization, range validation, max-window enforcement, and bounded context expansion.
- Database tests for new columns, old-schema upgrade behavior, Run persistence/serialization, and scoped versus unscoped search results.
- Runner tests asserting trusted time scope placement, absence from user instructions, and server-owned tool scope.
- Frontend behavior tests for all three time modes, payload conversion, validation/error display, disabled controls, and Run scope display.
- Full backend and frontend suites plus frontend build before publishing.

## Success criteria

An unscoped Run behaves exactly as before. A scoped Run stores and returns an immutable range, the Runner receives it as trusted context, searches are server-filtered to the primary window with bounded optional expansion, and the UI can create and display both incident-window and direct-range Runs.
