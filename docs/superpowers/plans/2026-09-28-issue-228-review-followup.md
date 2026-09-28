# Issue 228 Search Cancellation Review Follow-up Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make interactive search terminal state and cancellation converge through one guarded lifecycle, and verify Tantivy cancellation is observed during long candidate scans.

**Architecture:** The worker guard owns terminalization. Normal completion or error calls an explicit result-aware finish method exactly once; `Drop` only cancels and marks an unfinished worker as failed or cancelled. Tantivy keeps the independent `spawn_blocking` worker but checks the execution context on every candidate iteration so cancellation is cooperative and bounded by the next checkpoint.

**Tech Stack:** Rust, Tokio, Actix Web, Tantivy, Rust unit tests.

---

### Task 1: Make the search guard the single terminal-state owner

**Files:**
- Modify: `backend/src/routes/temp_results/service.rs:49-95,600-630`
- Test: `backend/src/routes/temp_results/service.rs:1050-1085`

- [x] **Step 1: Add regression tests for explicit completion and fallback drop behavior.**

  Add one test that finishes a guard as `Completed`, drops it, and asserts the registry remains `Completed` and the shared token is not cancelled. Add one test that drops an unfinished guard and asserts the token is cancelled and the registry is terminal.

- [x] **Step 2: Run the focused service tests and observe the new test failure.**

  Run `cargo test --locked routes::temp_results::service::tests:: --lib`. The completion test must fail because the guard has no explicit finished state yet.

- [x] **Step 3: Implement an idempotent guard finish API and synchronize handler-drop cancellation.**

  Add `finished: bool` to `SearchExecutionGuard`, implement `finish(status)` and `finish_result(result)`, and make `Drop` return immediately after an explicit finish. For an unfinished guard, cancel the token first and call registry finish once with `Cancelled` when cancellation was already requested, otherwise `Failed`. Make `SearchExecutionHandlerGuard::drop` request the registry’s internal cancellation transition as well as cancelling the shared token, so a handler-drop race cannot finish a still-running entry as `Completed`.

- [x] **Step 4: Route worker success, cancellation, timeout, and errors through `finish_result`.**

  Replace the worker’s direct `registry.finish` call with the guard method so every normal exit path uses the same transition logic.

- [x] **Step 5: Run the focused tests and verify they pass.**

  Run `cargo test --locked routes::temp_results::service::tests:: --lib` and confirm all service tests pass.

### Task 2: Strengthen cooperative Tantivy cancellation

**Files:**
- Modify: `backend/src/search/tantivy/query.rs:180-220`
- Test: `backend/src/search/tantivy/mod.rs:329-590` test module

- [x] **Step 1: Add a cancellation regression test around the blocking search worker.**

  Build a sufficiently large Tantivy fixture, run `CandidateSearch::search_page_with_context` inside `tokio::task::spawn_blocking`, cancel the shared token while the scan is active, and assert the join completes within one second with `SEARCH_CANCELLED` rather than waiting for the safety deadline.

- [x] **Step 2: Run the focused Tantivy test and verify the expected failure or missing coverage.**

  Run `cargo test --locked search::tantivy::tests:: --lib`. The test must either expose the current delayed checkpoint behavior or fail to compile until the fixture is completed; correct test setup errors before changing production code.

- [x] **Step 3: Check the execution context on every candidate iteration.**

  Replace the modulo-32 checkpoint gate in the candidate loop with an unconditional `checkpoint(context)?` at the start of each iteration, retaining the existing segment-level checkpoint and final result checks.

- [x] **Step 4: Run the focused Tantivy tests and confirm cancellation completes promptly.**

  Run `cargo test --locked search::tantivy::tests:: --lib` and confirm the cancellation regression passes.

### Task 3: Full verification and handoff

**Files:**
- Modify: `docs/superpowers/plans/2026-09-28-issue-228-review-followup.md`

- [x] **Step 1: Format and run all backend tests.**

  Run `cargo fmt --all`, `cargo clippy --locked -- -D warnings`, `cargo test --locked`, and `cargo test --locked --no-default-features`.

- [x] **Step 2: Run frontend verification because the PR contains the interactive lifecycle.**

  Run `cd frontend && npm test` and `npm run build`.

- [x] **Step 3: Check the diff and worktree.**

  Run `git diff --check` and `git status --short`; commit only the implementation and plan changes after the checks pass.
