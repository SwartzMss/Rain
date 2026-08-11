# Issue #97 实施计划

## Task 1：结构化 Tool Call 校验错误

- [x] 将 `parse_tool_call()` 的单位错误改为分类错误。
- [x] 区分 envelope、未知 Tool、JSON、额外字段、缺失字段和参数范围。
- [x] 对已知 Tool 生成白名单参数摘要，未知 Tool 不记录名称或参数正文。
- [x] 增加 `search_logs` 额外字段、`read_file_lines` 范围、未知 Tool 和无效 envelope 单元测试。

## Task 2：按调用恢复并保持协议完整

- [x] 删除整轮 Tool Calls 的 fail-fast `collect()`。
- [x] 为每个合法 `tool_call_id` 返回结构化 parse/execute 错误。
- [x] parse 错误记录 `REJECTED`，execute 可恢复错误记录 `FAILED`。
- [x] 前端监听 `tool.rejected` / `tool.failed` SSE 事件并刷新 Run 状态。
- [x] 同轮包含合法和非法调用时补齐全部 Tool responses。
- [x] 非法调用继续计入总调用预算。

## Task 3：限制无效重试

- [x] 连续 3 次可恢复 Tool 错误后停止 Tool 使用。
- [x] 为同轮剩余 Tool Calls 补齐 retry-limit response。
- [x] 进入无 Tool 的强制总结，不增加 iteration、Tool Call 或 retrieval budget。
- [x] 增加重复未知 Tool 的回归测试。

## Task 4：执行错误与平台故障

- [x] 将参数/资源/文件类型错误安全返回模型。
- [x] 保留 retrieval limit 的强制总结语义。
- [x] 数据库和 I/O 失败映射为 `SKILL_TOOL_STORAGE_ERROR`。
- [x] 其他不可恢复平台错误映射为 `SKILL_TOOL_EXECUTION_ERROR`。
- [x] 启动 Manifest 复用平台错误分类和安全日志，且不计入 Tool Call 预算。
- [x] 日志包含 parse/execute 阶段、分类、调用位置和安全参数摘要。

## Task 5：最终结果可观测性

- [x] 区分 `parse_json`、`schema` 和 `evidence`。
- [x] 首次失败和 repair 后失败均记录安全结构化日志。
- [x] 保持现有一次 repair 和公开错误码语义。

## Task 6：验证

- [x] `npm run build`（frontend）
- [x] `npm test`（frontend，12 个 Vitest 文件 / 61 个测试及附加脚本测试通过）
- [x] `cargo test --locked --test skill_runner`
- [x] `cargo clippy --locked -- -D warnings`
- [x] `cargo fmt --check`
- [x] `cargo check --locked`
- [x] `cargo test --locked`（243 个自动化测试通过，1 个手工 benchmark 按设计忽略）
- [x] `git diff --check`
