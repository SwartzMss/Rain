# Skill Run Time Scope Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an optional, persisted Skill Run incident-time scope that is trusted by the runner and automatically bounds log search while preserving unscoped behavior.

**Architecture:** Normalize user-supplied RFC3339 ranges into canonical UTC text plus epoch milliseconds. Persist both values on each Run and pass the immutable millisecond range into `SkillRunContext`. Index each log segment with the minimum and maximum explicit event timestamp found in its lines; scoped `search_logs` applies a server-owned overlap predicate and permits only a bounded 15-minute edge expansion.

**Tech Stack:** Rust 2024, Actix Web, SQLite/SQLx, Chrono, React 18, TypeScript, Vitest, Testing Library.

---

## File map

- Create `backend/src/services/skill_time_scope.rs`: RFC3339 parsing, canonicalization, validation, and bounded expansion.
- Modify `backend/src/services/mod.rs`: register the time-scope service module.
- Modify `backend/src/models/skill_runs.rs`: expose persisted analysis start/end values and internal epoch bounds.
- Modify `backend/src/repositories/skill_runs.rs`: select and insert the new Run fields while retaining the existing unscoped helper.
- Modify `backend/src/db.rs`: create/upgrade Run and log segment columns, indexes, and one-time backfill.
- Modify `backend/src/ingest.rs`: parse timestamped lines and persist per-segment event-time bounds.
- Modify `backend/src/routes/skill_runs.rs`: accept and validate `time_scope`, map validation errors to the API contract, and create the Run snapshot.
- Modify `backend/src/services/skill_tools.rs`: carry the immutable Run scope, validate bounded expansion, and apply it to `search_logs`.
- Modify `backend/src/services/skill_runner.rs`: inject the trusted time window and pass it to the tool executor.
- Create `frontend/src/features/skill-runs/timeScope.ts`: pure browser-side mode conversion and validation.
- Modify `frontend/src/api/types.ts`, `frontend/src/api/client.ts`, `frontend/src/features/skill-runs/useSkillRun.ts`, and `frontend/src/features/skill-runs/IssueSkillRunner.tsx`: request payload, controls, Run display, and state flow.
- Modify `backend/tests/skill_runs.rs`, `backend/tests/skill_tools.rs`, `backend/tests/skill_runner.rs`, `backend/src/db.rs` tests, `backend/src/ingest.rs` tests, and `frontend/tests/issue-skill-runner.behavior.test.tsx`; keep the pure frontend helper covered by the existing Vitest suite.

### Task 1: Add the time-scope value object and validation

**Files:**
- Create: `backend/src/services/skill_time_scope.rs`
- Modify: `backend/src/services/mod.rs`

- [ ] **Step 1: Write failing Rust unit tests**

Add tests for the public parser and expansion behavior:

```rust
#[test]
fn canonicalizes_offsets_to_utc_and_milliseconds() {
    let scope = parse_time_scope(Some(TimeScopeInput {
        start: "2026-08-14T09:27:15+08:00".into(),
        end: "2026-08-14T09:37:15+08:00".into(),
    }))
    .unwrap()
    .unwrap();

    assert_eq!(scope.start, "2026-08-14T01:27:15.000Z");
    assert_eq!(scope.end, "2026-08-14T01:37:15.000Z");
    assert_eq!(scope.end_ms - scope.start_ms, 10 * 60 * 1000);
}

#[test]
fn rejects_invalid_order_and_windows_over_24_hours() {
    let reversed = parse_time_scope(Some(TimeScopeInput {
        start: "2026-08-14T02:00:00Z".into(),
        end: "2026-08-14T01:00:00Z".into(),
    }));
    assert!(matches!(reversed, Err(TimeScopeError::InvalidRange)));

    let too_large = parse_time_scope(Some(TimeScopeInput {
        start: "2026-08-14T00:00:00Z".into(),
        end: "2026-08-15T00:00:01Z".into(),
    }));
    assert!(matches!(too_large, Err(TimeScopeError::TooLarge)));
}

#[test]
fn none_means_unscoped_and_expansion_is_limited() {
    assert_eq!(parse_time_scope(None).unwrap(), None);
    let scope = parse_time_scope(Some(valid_input())).unwrap().unwrap();
    let expanded = scope.expanded(15).unwrap();
    assert_eq!(expanded.start_ms, scope.start_ms - 15 * 60 * 1000);
    assert!(scope.expanded(16).is_err());
}
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run: `cargo test skill_time_scope --lib`

Expected: FAIL because the module, input type, parser, and tests do not exist yet.

- [ ] **Step 3: Implement the minimal value object**

Define:

```rust
pub const MAX_SCOPE_MILLIS: i64 = 24 * 60 * 60 * 1000;
pub const MAX_CONTEXT_EXPANSION_MINUTES: i64 = 15;

#[derive(Debug, Clone, Deserialize)]
pub struct TimeScopeInput {
    pub start: String,
    pub end: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillTimeScope {
    pub start: String,
    pub end: String,
    pub start_ms: i64,
    pub end_ms: i64,
}
```

Parse with `DateTime::parse_from_rfc3339`, convert to `Utc`, serialize with millisecond precision and `Z`, reject missing/invalid values, `start >= end`, and duration above `MAX_SCOPE_MILLIS`. Add `expanded(minutes)` that accepts only `0..=15` and uses checked millisecond arithmetic.

- [ ] **Step 4: Run the focused test and verify it passes**

Run: `cargo test skill_time_scope --lib`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add backend/src/services/mod.rs backend/src/services/skill_time_scope.rs
git commit -m "feat: validate skill run time scopes"
```

### Task 2: Persist the immutable Run scope and upgrade existing databases

**Files:**
- Modify: `backend/src/models/skill_runs.rs`
- Modify: `backend/src/repositories/skill_runs.rs`
- Modify: `backend/src/db.rs`
- Test: `backend/tests/skill_runs.rs`, `backend/src/db.rs`

- [ ] **Step 1: Add persistence tests before implementation**

Extend the existing Run fixture with a scoped creation case that asserts:

```rust
let run = skill_runs::create_with_scope(&pool, &new_run, Some(&scope)).await.unwrap();
assert_eq!(run.analysis_start_time.as_deref(), Some("2026-08-14T01:27:15.000Z"));
assert_eq!(run.analysis_end_time.as_deref(), Some("2026-08-14T01:37:15.000Z"));
assert_eq!(run.analysis_start_ms, Some(scope.start_ms));
assert_eq!(run.analysis_end_ms, Some(scope.end_ms));
```

Add a database test that creates the legacy `skill_runs` and `log_segments` shapes, calls `prepare_schema(&pool, false)`, and verifies `PRAGMA table_info` contains the new nullable columns. Also verify a normal unscoped `create` returns all four scope fields as `None`.

- [ ] **Step 2: Run the focused tests and verify they fail**

Run: `cargo test --test skill_runs -- --nocapture` and `cargo test db::tests --lib`

Expected: FAIL to compile or assert because the model, SQL columns, and schema upgrade do not exist.

- [ ] **Step 3: Add nullable Run fields and repository SQL**

Add `analysis_start_time`, `analysis_end_time`, `analysis_start_ms`, and `analysis_end_ms` to `SkillRunRecord`; mark the millisecond fields `#[serde(skip_serializing)]`. Keep the current `create(pool, value)` as a compatibility wrapper that calls `create_with_scope(pool, value, None)`. The new function inserts the nullable text and integer values and keeps the existing unique active-run error behavior.

- [ ] **Step 4: Extend and upgrade the SQLite schema idempotently**

Add the four nullable columns to the `CREATE TABLE IF NOT EXISTS skill_runs` definition. Add an internal `ensure_optional_columns` helper that checks `pragma_table_info` and runs fixed, allowlisted `ALTER TABLE ... ADD COLUMN` statements only when a column is absent. Invoke it after the create statements and before indexes. This must work for both a fresh database and an existing database without failing on repeated startup.

- [ ] **Step 5: Run the focused tests and verify they pass**

Run: `cargo test --test skill_runs -- --nocapture` and `cargo test db::tests --lib`

Expected: PASS, including legacy schema upgrade and unscoped compatibility.

- [ ] **Step 6: Commit**

```bash
git add backend/src/models/skill_runs.rs backend/src/repositories/skill_runs.rs backend/src/db.rs backend/tests/skill_runs.rs
git commit -m "feat: persist skill run analysis windows"
```

### Task 3: Normalize log event times during indexing and backfill old segments

**Files:**
- Modify: `backend/src/ingest.rs`
- Modify: `backend/src/db.rs`
- Test: `backend/src/ingest.rs` tests and `backend/src/db.rs` tests

- [ ] **Step 1: Write timestamp parser and indexing tests first**

Add tests for explicit timezone formats and unparseable content:

```rust
assert_eq!(parse_event_time_ms("2026-08-14T09:32:15.123+08:00"), Some(1_786_671_135_123));
assert_eq!(parse_event_time_ms("[2026-08-14 09:32:15Z] error"), Some(1_786_699_935_000));
assert_eq!(parse_event_time_ms("09:32:15 error"), None);

let (start, end) = event_time_range(
    "2026-08-14T09:32:15Z first\nnoise\n2026-08-14T09:33:15Z second",
);
assert!(start.unwrap() < end.unwrap());
```

Use the exact computed epoch value `1_786_699_935_000` for the bracketed UTC timestamp. Add an indexing fixture that flushes one segment and asserts its `event_time_start_ms` and `event_time_end_ms`.

- [ ] **Step 2: Run the focused tests and verify they fail**

Run: `cargo test ingest::tests --lib`

Expected: FAIL because timestamp extraction and segment columns are absent.

- [ ] **Step 3: Implement explicit timestamp extraction**

Add a small parser in `ingest.rs` (or a private helper module) that searches only for a dated ISO/RFC3339 timestamp at the beginning of a cleaned log line, accepts `T` or a space separator and `.` or `,` fractional separators, requires `Z` or an explicit numeric offset, and returns epoch milliseconds. Do not infer a timezone from a naive time-only string. Add `event_time_start_ms` and `event_time_end_ms` to `LogChunk`; update them with the min/max parsed value in `push`.

- [ ] **Step 4: Persist event-time bounds and add the database index**

Add nullable `event_time_start_ms` and `event_time_end_ms` to `log_segments`, add an index on `(file_id, event_time_start_ms, event_time_end_ms)`, and bind both values in `flush_log_chunks`. Extend the same idempotent schema helper from Task 2 for existing databases.

- [ ] **Step 5: Backfill existing segments safely**

After adding the columns, select existing segment `id,content` rows whose event-time bounds are null, compute `event_time_range(content)`, and update only those rows. Keep unparseable rows null and do not fail schema initialization. The operation is one-time in effect because subsequent runs skip rows with populated bounds.

- [ ] **Step 6: Run the focused tests and verify they pass**

Run: `cargo test ingest::tests --lib && cargo test db::tests --lib`

Expected: PASS, including fresh indexing, idempotent schema creation, and best-effort backfill.

- [ ] **Step 7: Commit**

```bash
git add backend/src/ingest.rs backend/src/db.rs
git commit -m "feat: index normalized log event times"
```

### Task 4: Accept `time_scope` in the API and expose it in Run responses

**Files:**
- Modify: `backend/src/routes/skill_runs.rs`
- Modify: `backend/src/repositories/skill_runs.rs`
- Test: `backend/tests/skill_runs.rs`

- [ ] **Step 1: Add API contract tests**

Extend the existing create-route integration tests to submit:

```json
{"skill_id":"skill-id","time_scope":{"start":"2026-08-14T09:27:15+08:00","end":"2026-08-14T09:37:15+08:00"}}
```

Assert `202 Accepted`, canonical `analysis_start_time`/`analysis_end_time` in the response, and the same values from GET, active-run lookup, and SSE snapshot. Add invalid-format, reversed-range, and over-24-hours requests asserting HTTP 400, code `INVALID_TIME_SCOPE`, and no created run.

- [ ] **Step 2: Run the focused tests and verify they fail**

Run: `cargo test --test skill_runs -- --nocapture`

Expected: FAIL because `CreateSkillRun` ignores the field and the persistence path has not been wired.

- [ ] **Step 3: Wire validation before model/provider work**

Add `time_scope: Option<TimeScopeInput>` to `CreateSkillRun`. Parse it immediately after issue normalization and before skill/provider lookup. Convert `TimeScopeError` to a 400 `AppError::api(StatusCode::BAD_REQUEST, "INVALID_TIME_SCOPE", ...)`. Pass the canonical value to `skill_runs::create_with_scope`; preserve `create` compatibility for existing test helpers.

- [ ] **Step 4: Run the focused tests and verify they pass**

Run: `cargo test --test skill_runs -- --nocapture`

Expected: PASS with scoped response persistence and stable validation errors.

- [ ] **Step 5: Commit**

```bash
git add backend/src/routes/skill_runs.rs backend/tests/skill_runs.rs
git commit -m "feat: accept skill run time scope"
```

### Task 5: Bind scoped search to the server-owned Run context

**Files:**
- Modify: `backend/src/services/skill_tools.rs`
- Modify: `backend/src/services/skill_runner.rs`
- Test: `backend/tests/skill_tools.rs`, `backend/tests/skill_runner.rs`

- [ ] **Step 1: Write failing tool and prompt tests**

Add a search fixture with two segments containing the same query at different event times and assert a scoped executor returns only the overlapping segment, while an unscoped executor returns both. Add an expansion case where a hit 10 minutes outside an edge appears for `context_expansion_minutes=10` but not for `16`.

Add a runner test that inspects recorded model messages and asserts the trusted message contains `Primary incident time range`, while the user skill message contains only the skill body and no time-range text.

- [ ] **Step 2: Run focused tests and verify they fail**

Run: `cargo test --test skill_tools -- --nocapture && cargo test --test skill_runner -- --nocapture`

Expected: FAIL because `SkillRunContext` has no time scope and `search_logs` has no time predicate.

- [ ] **Step 3: Extend the immutable executor context and tool schema**

Add `time_scope: Option<SkillTimeScope>` to `SkillRunContext`. Add optional `context_expansion_minutes` to `SkillToolCall::SearchLogs`, the JSON tool definition, argument validation, argument summaries, and duplicate-search key. Reject values outside `0..=15` before any SQL. The model must not receive start/end parameters.

- [ ] **Step 4: Apply the effective overlap predicate in both search modes**

Compute the effective range from the stored Run scope plus the validated expansion. In both short-literal and FTS queries add the equivalent of:

```sql
AND (
  ? IS NULL
  OR (
    ls.event_time_start_ms IS NOT NULL
    AND ls.event_time_end_ms IS NOT NULL
    AND ls.event_time_end_ms >= ?
    AND ls.event_time_start_ms <= ?
  )
)
```

Bind the same optional scope consistently in count/fetch queries, include the applied range in the JSON response, and leave the SQL unchanged in semantics when the scope is null. Segments with no normalized time are excluded only when a scope is active.

- [ ] **Step 5: Pass the persisted scope into the runner**

When `SkillRunner::execute` loads a Run, construct `SkillTimeScope` from the stored millisecond/text fields, add it to `SkillRunContext`, and keep the issue binding unchanged. Update the initial trusted message with the primary window and bounded expansion rules only when scope exists.

- [ ] **Step 6: Run focused tests and verify they pass**

Run: `cargo test --test skill_tools -- --nocapture && cargo test --test skill_runner -- --nocapture`

Expected: PASS, including unscoped compatibility, primary-window filtering, bounded expansion, and trusted prompt placement.

- [ ] **Step 7: Commit**

```bash
git add backend/src/services/skill_tools.rs backend/src/services/skill_runner.rs backend/tests/skill_tools.rs backend/tests/skill_runner.rs
git commit -m "feat: enforce skill run search time scope"
```

### Task 6: Add frontend time-scope conversion and request plumbing

**Files:**
- Create: `frontend/src/features/skill-runs/timeScope.ts`
- Modify: `frontend/src/api/types.ts`
- Modify: `frontend/src/api/client.ts`
- Modify: `frontend/src/features/skill-runs/useSkillRun.ts`
- Test: `frontend/tests/issue-skill-runner.behavior.test.tsx`

- [ ] **Step 1: Write pure conversion tests**

Test that no-limit returns `null`, incident mode subtracts/adds the configured minutes, range mode converts both values, and equal/reversed or missing values return a user-facing validation error rather than a request.

- [ ] **Step 2: Run the focused frontend tests and verify they fail**

Run: `npm test -- --run tests/issue-skill-runner.behavior.test.tsx` from `frontend`.

Expected: FAIL because the helper, payload type, and API signature do not exist.

- [ ] **Step 3: Implement the payload helper and API types**

Add:

```ts
export interface SkillRunTimeScope {
  start: string;
  end: string;
}
```

Change `rainApi.createSkillRun(issueCode, skillId, timeScope = null)` to send `{ skill_id: skillId, time_scope: timeScope }`. Add optional `analysis_start_time` and `analysis_end_time` to `SkillRun` and change `useSkillRun.start` to accept an optional scope and forward it unchanged.

- [ ] **Step 4: Run focused frontend tests and verify they pass**

Run: `npm test -- --run tests/issue-skill-runner.behavior.test.tsx` from `frontend`.

Expected: PASS for payload conversion and existing run behavior.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/api/types.ts frontend/src/api/client.ts frontend/src/features/skill-runs/useSkillRun.ts frontend/src/features/skill-runs/timeScope.ts frontend/tests/issue-skill-runner.behavior.test.tsx
git commit -m "feat: send skill run time scope from frontend"
```

### Task 7: Build the three-mode UI and display the saved scope

**Files:**
- Modify: `frontend/src/features/skill-runs/IssueSkillRunner.tsx`
- Test: `frontend/tests/issue-skill-runner.behavior.test.tsx`

- [ ] **Step 1: Add behavior tests before changing the component**

Extend the existing test to select “指定故障时间”, fill `故障时间`, `故障前分钟`, and `故障后分钟`, run the Skill, and assert `createSkillRun` received a UTC range. Add a direct-range case, a validation-error case for equal endpoints, and a display assertion for `analysis_start_time`/`analysis_end_time` on the returned Run.

- [ ] **Step 2: Run the focused test and verify it fails**

Run: `npm test -- --run tests/issue-skill-runner.behavior.test.tsx` from `frontend`.

Expected: FAIL because the controls, payload wiring, and display do not exist.

- [ ] **Step 3: Implement the controls and state**

Add a radio group with values `none`, `incident`, and `range`; default to `none`. Render the incident datetime plus numeric before/after controls only in incident mode, and start/end datetime controls only in range mode. Disable all scope controls while `state.active`. On run, call the pure helper, show its validation error without sending a request, and otherwise call `state.start(selected, scope)`.

- [ ] **Step 4: Display the immutable Run scope**

Under the existing status line, render “分析时间：不限制时间” for null scope or the saved canonical start/end values for a scoped Run. Use the Run response rather than current form values so the displayed range remains the persisted snapshot.

- [ ] **Step 5: Run the focused frontend test and verify it passes**

Run: `npm test -- --run tests/issue-skill-runner.behavior.test.tsx` from `frontend`.

Expected: PASS for all three modes, validation, disabled controls, existing evidence display, and saved-scope display.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/features/skill-runs/IssueSkillRunner.tsx frontend/tests/issue-skill-runner.behavior.test.tsx
git commit -m "feat: configure skill run analysis window"
```

### Task 8: Update documentation and perform full verification

**Files:**
- Modify: `README.md`
- Modify: `doc/DB.md`
- Test/build: `backend`, `frontend`

- [ ] **Step 1: Document the new compatibility contract**

Update the existing timeline note to state that `timeline` remains the legacy display value while normalized segment event-time bounds power scoped Skill Run search. Document `time_scope` as optional, the 24-hour maximum, the 15-minute bounded expansion, and that unparseable historical lines remain unscoped candidates only when no Run time scope is supplied.

- [ ] **Step 2: Run formatting and static checks**

Run from the worktree root:

```bash
cargo fmt --all -- --check
npm run lint --prefix frontend
npm run build --prefix frontend
```

Expected: all commands exit 0.

- [ ] **Step 3: Run the complete test suites**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml
npm test --prefix frontend
git diff --check
```

Expected: backend and frontend report zero failures, and `git diff --check` is clean.

- [ ] **Step 4: Review the final diff against the issue checklist**

Confirm: optional UI modes; API validation; immutable persistence and response; trusted prompt; primary-window search filtering; bounded expansion; no schema change; unscoped compatibility; and no unrelated files.

- [ ] **Step 5: Commit documentation and verification-only changes**

```bash
git add README.md doc/DB.md
git commit -m "docs: document skill run time scopes"
```
