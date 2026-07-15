# Recover Failed Uploads Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make failed background uploads reliably reach `FAILED`, show an actionable reason, and keep startup recovery bounded so the service cannot hang before binding HTTP.

**Architecture:** Persist `failure_reason` with each bundle and centralize terminal failure finalization before best-effort artifact cleanup. Extract startup recovery into time-bounded, independently logged stages; propagate the persisted reason through API models to frontend polling and bundle rows.

**Tech Stack:** Rust, Tokio, Actix Web, SQLx/SQLite, React, TypeScript, Node test scripts.

---

### Task 1: Persist and expose failure reasons

**Files:**
- Modify: `backend/src/db.rs`
- Modify: `backend/src/models/issues.rs`
- Modify: `backend/src/routes/issues.rs`
- Modify: `backend/src/routes/uploads.rs`
- Test: `backend/tests/smoke.rs`

- [ ] Add a smoke assertion that `PRAGMA table_info(bundles)` contains nullable `failure_reason`, new rows return `null`, and a failed upload task and issue bundle return the stored reason.
- [ ] Run `cargo test --locked --test smoke upload_search_tree_and_delete_issue -- --nocapture` and confirm the new assertion fails because the column/JSON field is absent.
- [ ] Extend the idempotent schema migration with `failure_reason TEXT`; add `failure_reason: Option<String>` to `UploadSummary`, `BundleRow`, `UploadTaskRow`, and `UploadTaskResponse`; select and serialize it from both APIs.
- [ ] Change READY finalization to clear `failure_reason` and stale startup recovery to set `status`, `process_stage`, and a restart-specific reason atomically.
- [ ] Run the focused smoke tests and confirm the migration and response assertions pass.
- [ ] Commit with `git commit -m "feat: persist upload failure reasons"`.

### Task 2: Reliably finalize runtime failures

**Files:**
- Modify: `backend/src/routes/uploads.rs`
- Test: `backend/src/routes/uploads.rs`
- Test: `backend/tests/smoke.rs`

- [ ] Add tests for a terminal-update helper that retries transient failures, stores `FAILED/FAILED` plus a normalized reason, and still reports cleanup errors without reverting the terminal state.
- [ ] Run the focused upload route tests and confirm they fail before the helper exists.
- [ ] Implement `finalize_bundle_failed_with_retry`: attempt the atomic terminal update three times with bounded backoff, emit structured errors containing the bundle identifier, then perform database and filesystem cleanup as independently logged best-effort operations.
- [ ] Route processing errors and permit-acquisition errors through the helper; preserve actionable input-limit messages while replacing internal details with a stable user-facing fallback.
- [ ] Run upload route tests and the failed gzip/depth smoke scenarios until all pass.
- [ ] Commit with `git commit -m "fix: reliably finalize failed uploads"`.

### Task 3: Add bounded startup self-recovery

**Files:**
- Modify: `backend/src/main.rs`
- Modify: `backend/src/db.rs`
- Test: `backend/src/main.rs`
- Test: `backend/tests/smoke.rs`

- [ ] Add tests for a recovery-stage runner proving success, error, and timeout all return control, plus temp cleanup continuing after an injected per-entry failure.
- [ ] Run the focused backend tests and confirm timeout/error cases fail with the current startup implementation.
- [ ] Extract a 15-second `run_optional_recovery_stage` wrapper that logs stage start, completion/error/timeout, elapsed milliseconds, and affected counts without aborting subsequent stages.
- [ ] Initialize tracing first, log effective database/data/log paths, keep schema preparation fatal, and run stale-status recovery, temp cleanup, and retention cleanup sequentially through the wrapper.
- [ ] Refactor temp cleanup to treat missing `.tmp` as success and return removed/failed counts while logging and continuing past individual path failures.
- [ ] Run focused recovery tests and backend smoke tests.
- [ ] Commit with `git commit -m "feat: bound startup recovery stages"`.

### Task 4: Surface failure recovery in the frontend

**Files:**
- Modify: `frontend/src/api/types.ts`
- Create: `frontend/src/features/files/uploadFailure.ts`
- Modify: `frontend/src/features/files/HomeView.tsx`
- Modify: `frontend/src/features/files/FilesView.tsx`
- Create: `frontend/tests/upload-failure.mjs`
- Modify: `frontend/package.json`

- [ ] Add a Node test importing the compiled helper and asserting FAILED tasks return their persisted reason, older records receive a retry/delete fallback, and processing states return no terminal error.
- [ ] Run `npm test` and confirm it fails because `uploadFailure` does not exist.
- [ ] Add optional `failure_reason` to upload/bundle types and implement a pure `uploadFailureMessage` helper.
- [ ] When polling receives FAILED, set the visible error before refreshing data; render the same reason on failed upload rows and keep failed rows deletable while the selector becomes enabled.
- [ ] Add the new test script to `npm test`, then run `npm test`, `npm run lint`, and `npm run build`.
- [ ] Commit with `git commit -m "feat: show upload failure reasons"`.

### Task 5: Verify the complete branch and publish

**Files:**
- Verify all modified files.

- [ ] Run `cargo fmt --check`.
- [ ] Run `cargo clippy --locked -- -D warnings` and confirm the recursive-ingest CI fix remains clean.
- [ ] Run `cargo test --locked` and repeat the upload smoke test enough times to detect timing-sensitive `PROCESSING/INDEXING` stalls.
- [ ] Run `npm test`, `npm run lint`, and `npm run build` in `frontend`.
- [ ] Run `git diff --check` and inspect the branch diff against `main` for unrelated changes.
- [ ] Commit any verification-only corrections, push `agent/recover-failed-uploads`, and open the Issue #17 pull request.
