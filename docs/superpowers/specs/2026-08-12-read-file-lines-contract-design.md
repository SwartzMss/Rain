# `read_file_lines` Contract and Iteration Error Budget Design

## Goal

修复 Skill Runner 在读取文件行范围时难以遵循跨字段约束、以及同一模型 iteration 内多个失败 Tool Call 过快耗尽连续错误预算的问题。

## Scope

只修改 Skill Runner 的 `read_file_lines` 外部 contract、服务端参数转换、recoverable Tool error 的安全提示和连续错误预算统计。EvidenceLedger 的最终证据结构、单次最多 200 行限制、Tool Call 总预算、Skill 诊断策略和 AI Provider retry 保持不变。

## Architecture and data flow

公开给模型的 `read_file_lines` schema 使用 `file_id/start/limit`。`file_id` 的最小值为 1，`start` 的最小值为 0，`limit` 的范围为 1 到 200；这些边界由 schema 直接表达。

解析器只接受这三个字段，并把它们转换成内部 `SkillToolCall::ReadFileLines { file_id, start, limit }`。执行器在真正读取前校验相同的边界，计算 `end = start + limit - 1`，然后继续调用现有的文件读取逻辑。读取结果的 EvidenceLedger 仍从实际返回行号生成 `start_line/end_line`，所以 final evidence 的语义不变。

每个模型 iteration 维护两个局部状态：是否出现 recoverable Tool error、是否至少有一个 Tool Call 成功。iteration 内的所有 Tool Call 都完成记录并返回给模型后，才统一更新连续错误计数：存在 recoverable error 且没有成功调用时计数加一；否则计数归零。这样同一 iteration 的多个失败最多消耗一次预算，而含有成功调用的 iteration 被视为已经恢复。达到阈值后，在 iteration 结束时进入最终结果阶段，不跳过该 iteration 中尚未处理的 Tool response。

## Error handling

非法 `read_file_lines` 参数仍返回 recoverable 的安全 JSON，不包含原始日志、文件内容或敏感参数。缺失/类型错误使用明确的参数名；范围错误明确指出 `limit` 必须在 1 到 200 之间，服务端 bad request 的安全原因也说明 `file_id`、`start` 和 `limit` 的边界。

计算结束行时使用 checked arithmetic；无法表示 `start + limit - 1` 时将请求作为 recoverable 参数错误返回，而不是让范围溢出或静默截断。

## Testing strategy

- 更新所有直接调用执行器的测试，使它们使用 `limit` 语义。
- 增加 parser/schema contract 测试，覆盖 `limit=1`、`limit=200`、`limit=0`、`limit=201`、负 `file_id`、负 `start`、缺失/额外字段和结束行溢出。
- 保留并更新 evidence 测试，确认 200 行边界可读、201 行不可构造/被拒绝，且 EvidenceLedger 仍保存实际返回的行区间。
- 增加 Runner 集成测试：同一 iteration 的三个失败调用只计一次并继续处理全部 response；跨三个 iteration 的失败仍在第三个 iteration 后进入最终结果阶段；成功调用会清零连续错误计数。
- 运行定向后端测试、完整 `cargo test`，并运行前端构建以确认嵌入资源仍可生成。

## Alternatives considered

1. 只增强 schema，保留 `start/end`：无法消除模型的跨字段算术负担。
2. 公开新参数但长期兼容旧参数：会维护两套模型可见/内部 contract，容易让迁移状态永久化。
3. 只提高连续错误阈值或放宽行数：不能修复 contract 表达能力，且会削弱现有安全边界。

采用本设计的一次性迁移，因为它同时修复 contract 根因和 iteration 级预算语义，并保留现有证据与资源限制。
