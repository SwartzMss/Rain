# Implementation Plan: #228 Interactive Search Execution State and Cancellation

> **For Codex execution:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` to implement this plan task-by-task with review checkpoints.

**Goal:** Add bounded, capability-authorized interactive search executions with real frontend execution state, cooperative backend cancellation, one monotonic safety deadline, and staging-safe cleanup while preserving guest preview compatibility and legacy clients.

**Architecture:** A pre-registration endpoint creates a short-lived in-memory reservation identified by a client UUID and a hashed capability. Preview consumes the reservation atomically and runs through a shared `SearchExecutionContext` containing a cancellation token and monotonic deadline. A separate idempotent DELETE endpoint requests cancellation and waits briefly for terminal confirmation. Existing staging transitions remain the cleanup boundary. The frontend uses a per-scope execution hook and preserves the last successful result until a new execution succeeds.

**Verification:** `cargo fmt --check`; focused Rust tests; `cargo test`; `cargo test --no-default-features`; `cd frontend && npm test`; `npm run build`.

## Task 1: Establish the bounded execution registry and control API

Files:
- Create `backend/src/services/search_execution.rs`.
- Create `backend/src/routes/search_requests.rs`.
- Modify `backend/src/services/mod.rs`, `backend/src/routes/mod.rs`, and `backend/src/lib.rs`.
- Add unit tests beside the registry and Actix route tests where the existing test harness permits.

Steps:
1. Write failing tests for capability hashing, reservation ownership, duplicate IDs, `RESERVED -> RUNNING`, idempotent cancellation, terminal retention, and bounded active/terminal capacity.
2. Run the focused tests and confirm they fail for the missing registry/API.
3. Implement an in-memory registry with short-lived reservations, terminal receipts, `CancellationToken`, notification, and monotonic deadline; never hold a mutex across an await.
4. Add `POST /api/search-requests` using `OptionalUser` and peer IP only for bounded admission/rate limiting. Return `Cache-Control: no-store` and never persist/log the capability.
5. Add `DELETE /api/search-requests/{search_id}` with capability/user checks, unknown/mismatched 204 behavior, idempotent state responses, and a bounded wait for running workers.
6. Run the focused registry/route tests and `cargo fmt --check`.

## Task 2: Attach preview requests to an execution context

Files:
- Modify `backend/src/routes/temp_results.rs`, `backend/src/routes/temp_results/routes.rs`, and `backend/src/routes/temp_results/service.rs`.
- Modify `backend/src/routes/temp_results/lifecycle.rs` and `backend/src/routes/temp_results/search_plan.rs`.

Steps:
1. Add failing tests for pre-register-then-cancel-before-preview, invalid capability, legacy preview without `search_id`, and cancellation/timeout outcome precedence.
2. Run those tests to verify the current preview path does not enforce reservations or distinguish cancellation from timeout.
3. Consume a reservation atomically before starting preview, extract request ownership data before spawning work, and keep the existing no-auth guest path for legacy requests.
4. Thread `SearchExecutionContext` through source resolution, planning, materialization, and first-page delivery. Set one deadline at execution start and map controlled stop reasons to stable API errors.
5. Ensure handler drop signals cancellation but does not drop the worker future before staging cleanup completes.
6. Add structured interactive-search completion telemetry without expression/capability contents.

## Task 3: Make all backend work cooperatively cancellable and publish-safe

Files:
- Modify `backend/src/services/temp_results.rs` and the controlled line-reader implementation under `backend/src/ingest/`.
- Modify `backend/src/search/tantivy/mod.rs`, `backend/src/search/tantivy/query.rs`, and `backend/src/routes/temp_results/service.rs`.
- Modify `backend/src/routes/temp_results/repository.rs`, `storage.rs`, and `lifecycle.rs` only where needed for the commit race.

Steps:
1. Write failing tests using barriers/deferred signals for query-permit waits, Tantivy segment/doc loops, raw scans, candidate verification, long lines, output writes, timeout, and commit/cancel ordering.
2. Run the focused tests and confirm cancellation currently returns too late or falls back to raw scanning.
3. Add a fallible controlled line-reader wrapper while preserving the existing ingestion API, checking the execution context while reading chunks.
4. Add checkpoints/yields through source/range/line loops, Tantivy blocking loops, query-permit admission, and output writes; cancellation and timeout must propagate instead of becoming a raw fallback.
5. Refactor materialization to close writers, acquire a short `COMMITTING` boundary, then rename/publish or reuse `abort_staging_result`; never delete an ACTIVE result from the cancellation path.
6. Run resource, cleanup, timeout, and publish-race tests, including `cargo test --no-default-features`.

## Task 4: Add frontend execution primitives and API cancellation support

Files:
- Modify `frontend/src/api/client.ts` and `frontend/src/api/types.ts`.
- Create `frontend/src/hooks/useSearchExecution.ts` and `frontend/src/components/SearchExecutionStatus.tsx`.
- Add focused tests under `frontend/tests/` for abort during fetch/body read, reservation ordering, stale generations, 202 confirmation, and elapsed-time freezing.

Steps:
1. Write failing Vitest/Node tests for abort normalization, independent cancellation signals, and A-cancel/B-start stale-response protection.
2. Run them and confirm the current client wraps aborts as ordinary API errors and preview has no execution identity.
3. Add typed reserve/preview/cancel methods and `RequestCancelledError`; preserve abort identity across fetch and response-body reads.
4. Implement the hook with `IDLE/RUNNING/CANCELLING/CANCELLED/SUCCEEDED/FAILED`, `performance.now()` elapsed updates, independent cancel controller, generation invalidation before abort, and best-effort unmount cleanup.
5. Implement an accessible indeterminate status component with no fake percentage and stable timeout/cancel messaging.
6. Run frontend focused tests and `npm run build`.

## Task 5: Integrate all interactive preview entry points

Files:
- Modify `frontend/src/features/files/FilesView.tsx`, `frontend/src/features/files/TempResultView.tsx`, and `frontend/src/features/files/SearchResultViewer.tsx` (or the current viewer module path).
- Add/extend page-level tests for issue, saved-search, file, nested viewer, and temp-result scopes.

Steps:
1. Write failing UI tests for loading/status isolation, cancellation retaining the previous result, route/file/tab changes, and no new viewer tab after cancellation.
2. Run the tests to capture current global-loading and stale-response behavior.
3. Replace content-search loading flags with per-scope execution snapshots while leaving filename search and result pagination independently cancellable.
4. Preserve previous successful result snapshots on cancellation/failure; only successful current-generation executions update hits, totals, saved-search usage, or navigation.
5. Ensure all route/unmount transitions invalidate generation before aborting and do not let stale promises clear a newer execution.
6. Run the full frontend suite and build.

## Task 6: Final safety settings, documentation, and verification

Files:
- Modify `backend/src/config.rs`, `backend/src/settings/metadata.rs`, README/config examples only if benchmark evidence supports a default change.
- Add focused regression tests and a short benchmark/verification note if required by repository conventions.

Steps:
1. Keep the existing persisted timeout default unless measured evidence justifies changing it; update labels to describe a safety deadline and preserve existing database values.
2. Run `cargo fmt --check`, `cargo test`, `cargo test --no-default-features`, `cd frontend && npm test`, and `npm run build`.
3. Run `git diff --check`, inspect the diff for secret/capability logging, and verify worktree status.
4. Commit coherent implementation changes, push `fix/issue-228-search-cancellation`, and create an implementation PR referencing #228 and design PR #230.
