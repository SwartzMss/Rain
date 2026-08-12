# `read_file_lines` Contract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 Skill Runner 的 `read_file_lines` 迁移到可由 JSON Schema 直接约束的 `start/limit` contract，并按模型 iteration 统计 recoverable Tool error。

**Architecture:** `skill_runner.rs` 负责模型可见 schema、参数解析、安全错误输出和 iteration 生命周期；`skill_tools.rs` 负责内部读取参数、200 行边界、范围计算和 evidence 记录。公开层和执行层都做防御性校验，最终证据继续使用真实返回的行号。

**Tech Stack:** Rust 2024、Tokio、Serde/serde_json、SQLx SQLite、现有 `cargo test` 集成测试。

---

## Files and responsibilities

- Modify `backend/src/services/skill_runner.rs`: `SkillToolCall` 参数解析、tool schema、参数摘要、安全错误提示、iteration 级错误状态和阈值处理。
- Modify `backend/src/services/skill_tools.rs`: `SkillToolCall::ReadFileLines` 的字段、执行器读取方法的 `limit` contract、范围校验和 checked `end` 计算。
- Modify `backend/tests/skill_tools.rs`: 更新读取测试调用，增加 200 行边界与 evidence 语义覆盖。
- Modify `backend/tests/skill_runner.rs`: 更新旧 payload，并增加 schema/parser 错误提示与 iteration 级连续错误集成覆盖。
- Create `docs/superpowers/specs/2026-08-12-read-file-lines-contract-design.md`: 已批准的设计记录。
- Create `docs/superpowers/plans/2026-08-12-read-file-lines-contract.md`: 本实施计划。

### Task 1: Define the failing contract tests

**Files:**
- Modify: `backend/tests/skill_tools.rs`
- Modify: `backend/tests/skill_runner.rs`

- [ ] **Step 1: Update direct executor tests to express ranges as `start, limit`**

把现有 `read_file_lines(file_id, start, end)` 调用改为 `read_file_lines(file_id, start, limit)`，并把原来的 `(16, 215)` 改为 `(16, 200)`；保留断言读取到 line 16，以明确新 API 的第三个参数是数量而不是结束行。

- [ ] **Step 2: Add a failing test for the public `limit` contract**

在 `backend/tests/skill_runner.rs` 的工具恢复测试附近增加一组 model payload，至少覆盖：

```text
{"file_id":123,"start":100,"limit":200}
{"file_id":123,"start":100,"limit":0}
{"file_id":123,"start":100,"limit":201}
{"file_id":0,"start":100,"limit":1}
{"file_id":123,"start":-1,"limit":1}
{"file_id":123,"start":i64::MAX,"limit":2}
```

断言合法 payload 可被执行，非法 payload 的 Tool response 含有 `INVALID_TOOL_CALL`、`INVALID_ARGUMENT`，并且范围错误包含 `limit` 的可执行修正信息。该测试在实现迁移前应因旧 `end` contract 而失败。

- [ ] **Step 3: Add a failing test for iteration-level error accounting**

构造第一轮包含三个非法 `read_file_lines` 调用的 model response，第二轮返回 `insufficient_evidence_response()`；断言三个 tool response 都被加入第二个 model request，且 run 不会因为同一轮的三个失败直接进入无工具最终化。再构造跨三个 model iteration 各一个非法调用的场景，断言第三次失败后才强制最终化。

- [ ] **Step 4: Run the focused tests and confirm the expected failures**

Run:

```bash
cargo test --test skill_tools read_file_lines -- --nocapture
cargo test --test skill_runner runner_preserves_all_tool_responses -- --nocapture
cargo test --test skill_runner runner_forces_summary -- --nocapture
```

Expected: the new `limit` payload and iteration-level assertions fail against the old `start/end` parser and per-call counter; existing unrelated tests remain green.

### Task 2: Migrate the internal read contract

**Files:**
- Modify: `backend/src/services/skill_tools.rs`

- [ ] **Step 1: Change the enum field from `end` to `limit`**

Use:

```rust
SkillToolCall::ReadFileLines {
    file_id: i64,
    start: i64,
    limit: i64,
}
```

Update `SkillToolExecutor::execute` to pass `limit` to `read_file_lines`.

- [ ] **Step 2: Validate and calculate the internal end line**

At the start of `SkillToolExecutor::read_file_lines`, reject `file_id <= 0`, `start < 0`, `limit < 1`, or `limit > MAX_READ_LINES`. Calculate:

```rust
let end = start
    .checked_add(limit - 1)
    .ok_or_else(|| AppError::BadRequest("invalid file line range".into()))?;
```

Use this `end` for ledger duplicate/unseen range calculations. Keep the existing response truncation and `EvidenceRange` creation unchanged so only returned line numbers enter the ledger.

- [ ] **Step 3: Run the focused executor test**

Run:

```bash
cargo test --test skill_tools read_file_lines_exposes_bounded_long_lines_and_records_only_returned_evidence -- --nocapture
```

Expected: PASS, including legal 200-line reads, rejected zero/201-line requests, duplicate detection, continuation, and evidence assertions.

### Task 3: Migrate schema, parser, and safe error output

**Files:**
- Modify: `backend/src/services/skill_runner.rs`

- [ ] **Step 1: Expose the new JSON Schema**

Replace the `read_file_lines` definition with properties and required fields:

```rust
"file_id": {"type":"integer","minimum":1},
"start": {"type":"integer","minimum":0},
"limit": {"type":"integer","minimum":1,"maximum":200}
```

Keep `additionalProperties: false`.

- [ ] **Step 2: Parse only `file_id`, `start`, and `limit`**

Update unexpected/missing field checks, integer extraction, argument summaries, and the returned `SkillToolCall`. Use explicit reasons:

```text
read_file_lines requires file_id, start, and limit
read_file_lines file_id must be positive
read_file_lines start must be non-negative
read_file_lines limit must be between 1 and 200
```

Reject an overflowing `start + limit - 1` before execution with a recoverable `INVALID_ARGUMENT` result.

- [ ] **Step 3: Include actionable field metadata in invalid results**

Extend the safe error output path so range failures return a bounded object such as:

```json
{
  "error": "INVALID_TOOL_CALL",
  "category": "INVALID_ARGUMENT",
  "tool": "read_file_lines",
  "field": "limit",
  "message": "read_file_lines limit must be between 1 and 200"
}
```

Do not include raw arguments or file/log contents. Preserve the existing sanitized behavior for other tools.

- [ ] **Step 4: Run parser and runner contract tests**

Run:

```bash
cargo test services::skill_runner::tests::tool_validation_errors_are_classified_and_sanitized --lib -- --nocapture
cargo test --test skill_runner runner_returns_parse_errors_and_allows_a_corrected_call -- --nocapture
cargo test --test skill_runner runner_preserves_all_tool_responses -- --nocapture
```

Expected: PASS with the new schema and error messages, while unknown-field sanitization remains intact.

### Task 4: Count recoverable errors once per model iteration

**Files:**
- Modify: `backend/src/services/skill_runner.rs`
- Modify: `backend/tests/skill_runner.rs`

- [ ] **Step 1: Track iteration-local error and success flags**

Before iterating `tool_calls`, initialize:

```rust
let mut iteration_had_recoverable_error = false;
let mut iteration_had_successful_call = false;
```

Set the corresponding flag for each processed call. Do not increment `consecutive_tool_errors` inside the per-call branch.

- [ ] **Step 2: Move threshold evaluation after all calls in the iteration**

After the per-call loop, update:

```rust
if iteration_had_recoverable_error && !iteration_had_successful_call {
    consecutive_tool_errors += 1;
} else {
    consecutive_tool_errors = 0;
}
```

Log and emit the iteration-level count once. If the threshold is reached, set `finalization_reason`, emit `iteration.completed` with `tool_error_limit_reached: true`, and break only after every tool response from that model response has been recorded. Keep the existing retrieval-limit and total-call-limit behavior unchanged.

- [ ] **Step 3: Add the mixed-success regression test**

Use one model response containing one invalid call followed by one valid `list_files` call. Assert both tool responses are present and the next invalid iteration starts from a single error, not two errors. This confirms a successful call resets the iteration-level consecutive budget.

- [ ] **Step 4: Run the iteration-focused tests**

Run:

```bash
cargo test --test skill_runner runner_preserves_all_tool_responses -- --nocapture
cargo test --test skill_runner runner_forces_summary_after_three_consecutive_invalid_tool_calls -- --nocapture
```

Expected: same-iteration multi-failure, mixed-success, and cross-iteration behavior all pass.

### Task 5: Refactor, format, and verify the complete change

**Files:**
- Modify: `backend/src/services/skill_runner.rs`
- Modify: `backend/src/services/skill_tools.rs`
- Modify: `backend/tests/skill_runner.rs`
- Modify: `backend/tests/skill_tools.rs`
- Create: `docs/superpowers/specs/2026-08-12-read-file-lines-contract-design.md`
- Create: `docs/superpowers/plans/2026-08-12-read-file-lines-contract.md`

- [ ] **Step 1: Review the diff for stale `start/end` contracts**

Run:

```bash
rg -n 'read_file_lines|start.*end|end.*start' backend/src backend/tests docs/superpowers
```

Confirm every public tool definition and model payload uses `limit`; only evidence fields and internal file-reader APIs that intentionally use actual `end_line` retain `end` terminology.

- [ ] **Step 2: Format and run the complete verification suite**

Run:

```bash
cargo fmt --all -- --check
cargo test
npm run build
git diff --check
```

Expected: formatting check passes, all backend tests pass with one pre-existing ignored benchmark, frontend build succeeds, and `git diff --check` emits no whitespace errors.

- [ ] **Step 3: Commit the implementation**

```bash
git add backend/src/services/skill_runner.rs backend/src/services/skill_tools.rs backend/tests/skill_runner.rs backend/tests/skill_tools.rs docs/superpowers/specs/2026-08-12-read-file-lines-contract-design.md docs/superpowers/plans/2026-08-12-read-file-lines-contract.md
git commit -m "fix: make skill file reads iteration-safe"
```

After committing, inspect `git status --short --branch` and `git show --stat --oneline HEAD` before reporting the result.
