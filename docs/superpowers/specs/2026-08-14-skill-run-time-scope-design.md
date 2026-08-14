# Skill Run Wall-Clock Time Scope Design

## Goal

为 Skill Run 增加可选的故障时间范围，使一次诊断可以在同一批日志中定位事故时间附近的内容，同时保持未填写时间范围时的现有行为。

本功能比较的是日志中的 wall-clock time，不尝试在设备、服务器或时区之间建立绝对时间关系。用户输入的是日志上看到的时间；前端和后端都保留这个本地时间语义。

## Scope

本次实现覆盖：

- 前端“不限制时间 / 指定故障时间 / 指定时间范围”交互；
- Create Skill Run API 的 `time_scope` 输入、wall-clock 文本解析、开始时间早于结束时间校验和最大窗口限制；
- Skill Run 持久化、查询和 SSE snapshot 返回时间范围；
- Runner 的 Trusted Run Scope 注入；
- 日志索引中的 wall-clock 事件时间范围和 `search_logs` 的服务端主窗口过滤；
- 受控的边界上下文扩展和 time-index coverage 反馈；
- 前后端回归测试和旧数据库启动兼容。

不修改 SKILL schema、EvidenceLedger、Final Result contract 或输出语言策略，也不实现其他 Run Scope。

## Decisions

### Time scope representation

客户端提交无时区的日志 wall-clock 文本：

```json
{
  "skill_id": "skill-id",
  "time_scope": {
    "start": "2026-08-14 09:27:15.123",
    "end": "2026-08-14T09:37:15.456"
  }
}
```

`time_scope` 为 `null` 或省略时表示不限制时间。服务端接受空格或 `T` 分隔的本地日期时间、可选小数秒，以及 `datetime-local` 的分钟精度（例如 `2026-08-14T09:27`）；不要求也不解释 timezone。服务端以稳定的无时区 wall-clock 文本保存 Run 快照，并要求 `start < end`、窗口不超过 24 小时。

内部整数值只用于同一 wall-clock 编码之间的排序和范围比较。数据库继续保留 `analysis_*_ms` 和 `event_time_*_ms` 这些兼容性列名，但其值不是 Unix epoch、UTC、真实经过的毫秒数或跨时区绝对时间；不能把它们交给需要绝对时间语义的调用方。

### Log event time index

当前 `log_segments.timeline` 固定为 `all`，不能用于比较时间。因此 `log_segments` 保存可空的 `event_time_start_ms` 和 `event_time_end_ms` wall-clock 比较键。索引器支持常见的行首日志格式：

- `2026-08-14 09:32:15 ...`；
- `[2026-08-14 09:32:15] ...`；
- `[E][2026-08-14 09:32:15][...] ...`；
- 上述格式带 `T` 分隔符或 fraction。

日志不需要携带 timezone。只有 `HH:mm:ss` 而没有日期时不推断日期，相关 segment 保持没有可比较的事件时间。一个 segment 可以包含多行，因此 segment 的时间范围是候选过滤范围，最终事实仍由 `read_file_lines` 验证。

旧数据库启动时通过幂等 schema ensure 补列，并为 `event_time_indexed` 使用 `0` 默认值。历史 segment 按 `id` keyset 分批回填，每批使用独立事务；成功处理后即使无法解析也将 `event_time_indexed` 置为 `1`，从而区分“已尝试但没有 wall-clock 时间”和“尚未处理”。事务失败会回滚该批，后续启动可从状态为 `0` 的记录继续。回填使用 `COALESCE` 保留已有部分边界，不更新正文或 FTS；无时间范围的搜索完全不增加时间条件，兼容既有数据和行为。

### Server-bound search scope

`SkillRunContext` 持有不可由模型修改的时间范围。`search_logs` 不接受任意 start/end 参数；执行器自动把 Run 范围加入 SQL：segment 两个事件时间边界均已知且与主窗口相交，才可作为候选。未设置范围时不追加条件。

模型可以请求有限的边界上下文扩展，但只能通过固定上限的 `context_expansion_minutes` 参数，服务端限制为最多 15 分钟，并同时向前后扩展主窗口。该参数不能覆盖或替换原始 Run scope。工具响应包含实际应用的范围以及 time-index coverage；当日志命中但因缺少事件时间索引被排除时，响应会显式报告 excluded-unindexed 信息，而不是把它伪装成普通的 0 hits。

### Runner prompt

有时间范围时，在 Trusted system message 中注入日志 wall-clock 范围：

```text
Primary incident wall-clock range: 2026-08-14 09:27:15 through 2026-08-14 09:37:15.
Prioritize events inside this window. You may request only bounded context near its edges when needed for causality. Do not associate an identical message from another time solely by keyword.
```

时间范围不进入 `USER SKILL INSTRUCTIONS`。无范围时继续只注入当前 Issue 的 scope 文案，不引入时间语义。

### Frontend interaction

默认选择“不限制时间”。“指定故障时间”使用 `datetime-local`、故障前分钟数和故障后分钟数，前端按浏览器输入的 wall-clock 语义生成无时区范围，不调用 `toISOString()` 或进行 UTC 转换。“指定时间范围”使用开始和结束 `datetime-local`。运行期间这些控件和 Skill 选择保持禁用。Run 状态和结果区域展示保存的分析范围；空范围显示“不限制时间”。

## Data flow

```text
UI wall-clock time mode
  -> useSkillRun(start payload)
  -> POST /issues/:issue/skill-runs { skill_id, time_scope }
  -> validate local wall-clock range + persist Run snapshot
  -> SkillRunner loads snapshot
  -> Trusted prompt + SkillRunContext(time scope)
  -> SkillToolExecutor.search_logs applies server-bound comparison-key SQL
  -> read_file_lines verifies original lines/evidence
  -> Run API/SSE returns immutable analysis range
```

## Error handling

- Invalid or missing endpoint, equal/reversed endpoints, or window over 24 hours returns `INVALID_TIME_SCOPE` and does not enqueue a Run.
- Indexing failures do not fail the upload solely because a line timestamp is unparseable; the event-time columns remain null and `event_time_indexed` records whether the segment has been attempted.
- A scoped search excludes segments with no indexed wall-clock time. The tool response reports the applied scope and coverage, including excluded-unindexed matches; the model can produce `INSUFFICIENT_EVIDENCE` rather than silently treating unrelated global hits as incident evidence.
- Database schema extension is idempotent and runs before normal request serving. Existing unscoped Runs remain valid with null scope.

## Testing strategy

- Unit tests for wall-clock parsing, local text preservation, comparison-key ordering, range validation, max-window enforcement, and bounded context expansion.
- Database tests for new columns, old-schema upgrade behavior, Run persistence/serialization, event-time backfill, and scoped versus unscoped search results.
- Runner tests asserting Trusted Run Scope placement, absence from user instructions, and server-owned tool scope.
- Frontend behavior tests for all three time modes, wall-clock payload conversion without UTC conversion, validation/error display, disabled controls, and Run scope display.
- Full backend and frontend suites plus frontend build before publishing.

## Success criteria

An unscoped Run behaves exactly as before. A scoped Run stores and returns an immutable wall-clock range, the Runner receives it as Trusted Run Scope, searches are server-filtered to the primary window with bounded optional expansion, coverage reports excluded unindexed matches, and the UI can create and display both incident-window and direct-range Runs.
