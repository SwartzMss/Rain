# Skill Run Wall-Clock Time Scope Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an optional, persisted Skill Run incident-time scope that is trusted by the runner and automatically bounds log search while preserving unscoped behavior.

**Architecture:** Treat the user-entered time as log wall-clock time. Accept local date/time text with either a space or `T` separator, optional fraction, and `datetime-local` minute precision; retain the `start < end` and 24-hour checks. Persist the wall-clock text and a sortable comparison key on each Run. The existing `*_ms` database names remain for compatibility, but their values are only wall-clock comparison keys, never Unix epoch or UTC values. Index each log segment with the minimum and maximum dated event time found in its lines; scoped `search_logs` applies a server-owned overlap predicate and permits only a bounded 15-minute edge expansion.

**Tech Stack:** Rust 2024, Actix Web, SQLite/SQLx, Chrono, React 18, TypeScript, Vitest.

## File map

- Create `backend/src/services/skill_time_scope.rs`: local wall-clock parsing, comparison-key encoding, validation, and bounded expansion.
- Modify `backend/src/models/skill_runs.rs`, `backend/src/repositories/skill_runs.rs`, and `backend/src/db.rs`: persist the optional Run scope and retain the existing `*_ms` columns.
- Modify `backend/src/ingest.rs`: parse dated wall-clock timestamps and persist per-segment event-time bounds.
- Modify `backend/src/routes/skill_runs.rs`: accept and validate `time_scope`, map validation errors to the API contract, and create the Run snapshot.
- Modify `backend/src/services/skill_tools.rs` and `backend/src/services/skill_runner.rs`: carry the immutable Run scope, report coverage, and apply server-bound search filtering.
- Create `frontend/src/features/skill-runs/timeScope.ts` and modify the related API/UI files: preserve `datetime-local` wall-clock semantics without UTC conversion.
- Keep the existing backend/frontend test coverage aligned with the wall-clock contract.

### Task 1: Add the wall-clock time-scope value object and validation

Parse `YYYY-MM-DD HH:mm:ss[.fraction]`, the equivalent `T` form, and `datetime-local` minute precision. Preserve the local clock fields in the canonical Run text. Reject missing/invalid values, `start >= end`, and windows above 24 hours. Use a deterministic packed comparison key for ordering and SQL; document and test that the key has no Unix epoch or timezone meaning. Keep `expanded(minutes)` bounded to `0..=15` and perform calendar arithmetic before re-encoding the key.

Focused checks: `cargo test skill_time_scope --lib`.

### Task 2: Persist the immutable Run scope and upgrade existing databases

Retain the nullable `analysis_start_time`, `analysis_end_time`, `analysis_start_ms`, and `analysis_end_ms` columns. Store the wall-clock text and matching comparison keys. Preserve the unscoped creation helper and old-schema startup compatibility. Existing Runs with null scope remain unscoped.

Focused checks: `cargo test --test skill_runs -- --nocapture` and the relevant database tests.

### Task 3: Index dated wall-clock event times during ingestion and backfill

Support these line-start forms without requiring timezone data:

- `2026-08-14 09:32:15 ...`;
- `[2026-08-14 09:32:15] ...`;
- `[E][2026-08-14 09:32:15][...] ...`;
- the same forms with `T` separators and fractions.

Do not infer a date for `HH:mm:ss`. Keep `event_time_start_ms` and `event_time_end_ms` as compatibility column names whose values are wall-clock comparison keys. Preserve `event_time_indexed`: successful or unparseable backfill attempts become indexed, while pending rows remain distinguishable. Keep keyset batching, per-batch transactions, and `COALESCE` behavior.

Focused checks: `cargo test ingest::tests --lib && cargo test db::tests --lib`.

### Task 4: Accept `time_scope` in the API and expose the persisted Run snapshot

Accept the optional object with no timezone requirement. Validate the local wall-clock range before model/provider work, return `INVALID_TIME_SCOPE` for invalid input, and expose the persisted wall-clock text through the create response, GET, active-run lookup, and SSE snapshot. Do not let a later form change affect an existing Run.

Focused checks: `cargo test --test skill_runs -- --nocapture`.

### Task 5: Bind scoped search to the server-owned Run context

Keep `time_scope` on `SkillRunContext` and do not expose arbitrary start/end parameters to the model. Apply the saved comparison-key range in both short-literal and FTS search SQL. Segments with either null event-time boundary are excluded only when a Run scope is active. Add the optional `context_expansion_minutes` argument with a hard `0..=15` limit, expanding the saved wall-clock range on both edges.

Return the applied range and time-index coverage. In particular, distinguish no matching logs from matching logs excluded because their event time is not indexed. Keep the Issue binding, Trusted Run Scope, bounded expansion, and unscoped SQL behavior unchanged.

Focused checks: `cargo test --test skill_tools -- --nocapture && cargo test --test skill_runner -- --nocapture`.

### Task 6: Preserve wall-clock semantics in frontend request plumbing

Keep `datetime-local` values in their entered local-clock meaning. Generate the incident window using local calendar fields and explicit formatting; do not call `toISOString()` or convert to UTC. Send the resulting no-timezone text through `createSkillRun`, and display the saved Run values rather than transient form state.

Focused checks: the time-scope helper and Issue Skill Runner Vitest suites, plus frontend lint.

### Task 7: Build the three-mode UI and display the saved scope

Retain “不限制时间”, “指定故障时间”, and “指定时间范围”. Disable the scope controls while a Run is active. Validate equal/reversed endpoints and display the persisted wall-clock range for scoped Runs; show “不限制时间” for unscoped Runs.

Focused checks: `npm test -- --run tests/issue-skill-runner.behavior.test.tsx` from `frontend`.

### Task 8: Update documentation and perform focused verification

Document the wall-clock contract in `README.md`, `doc/DB.md`, this plan, and the design spec. Explicitly state that:

- API input is no-timezone wall-clock text with space/`T` forms, optional fraction, and minute precision;
- `start < end` and the 24-hour maximum remain enforced;
- `*_ms` columns are compatibility names for wall-clock comparison keys, not Unix epoch/UTC;
- ordinary, bracketed, and `[E][time][...]` log prefixes are supported, while time-only logs do not get an inferred date;
- Run binding, Trusted Run Scope, server-bound SQL, `event_time_indexed`, coverage, 15-minute expansion, persistence, and unscoped behavior remain in place.

Pure documentation changes do not require TDD. Verify the allowed file set, inspect the diff, run `git diff --check`, and search these documents for stale absolute-time wording before committing.

## Final checklist

- [ ] Wall-clock text is preserved; no frontend UTC conversion remains in this flow.
- [ ] No API validation requires timezone-bearing input.
- [ ] Dated common log prefixes are indexed; `HH:mm:ss` does not invent a date.
- [ ] Existing `*_ms` columns are retained and documented as comparison-key storage.
- [ ] Run binding, persistence, Trusted Run Scope, server-bound SQL, coverage, bounded expansion, and unscoped behavior remain intact.
- [ ] Only the requested documentation files changed for the documentation task.
- [ ] `git diff --check` passes and stale references are reported separately from unrelated temp-result/session UTC usage.
