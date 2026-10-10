# Open Search Result Retention Implementation Plan

> **For agentic workers:** Use superpowers:subagent-driven-development or superpowers:executing-plans to implement tasks with regression tests and review.

**Goal:** Keep open search results usable and provide explicit recovery after real expiry.

**Architecture:** A bounded public keep-alive endpoint atomically extends ACTIVE/unexpired records. A shared React hook renews all open result IDs; closing a tab stops renewal rather than deleting shared data. Query-plan replay handles expired snapshots.

**Tech Stack:** Rust, Actix, SQLite/sqlx, React, TypeScript, Vitest.

## Task 1: Backend renewal

Files: new `backend/src/routes/temp_results/retention.rs`; register in `temp_results.rs` and `routes/mod.rs`.

- [ ] Add regression tests using in-memory SQLite and Actix. Seed ACTIVE with expiry one minute away and assert renewal moves it at least 29 minutes out; seed expired/STAGING/DELETING and assert all reported unavailable. Keep seven-day expiry unchanged. Assert old expiry cleanup claim cannot remove a renewed row.
- [ ] Run `cargo test --locked retention` and observe expected failure before implementation.
- [ ] Implement request validation and transactional batch renewal:
  ```sql
  UPDATE temp_results
  SET expires_at = CASE WHEN datetime(expires_at) < datetime(?) THEN ? ELSE expires_at END
  WHERE id IN (...) AND status = 'ACTIVE' AND datetime(expires_at) >= datetime('now')
  RETURNING id
  ```
  Bind both expiry arguments to server-now + 30 minutes and every ID. Return requested IDs absent from RETURNING as unavailable. Register the literal keep-alive route before `{id}` routes.
- [ ] Verify targeted tests plus fmt/check/clippy; review backend diff.

## Task 2: Browser retention controller

Files: `frontend/src/api/client.ts`; new `features/files/hooks/useResultRetention.ts`; `hooks/useViewerTabs.ts`; `tests/search-result-cleanup.behavior.test.tsx`; new retention behavior tests.

- [ ] Write failing fake-timer tests for immediate/five-minute renewal, all IDs including inactive tabs, duplicate IDs, close/unmount, 100-ID chunks, transient failures, recovery events and stale responses.
- [ ] Add `rainApi.keepAliveTempResults(ids, signal)` calling `POST /api/temp-results/keep-alive` and returning `{ unavailable_ids: string[] }`.
- [ ] Implement `useResultRetention(resultIds)` returning `unavailableIds`, `markUnavailable` and `refresh`. Serialize bounded requests with AbortController; use latest IDs and prune stale expiry state. Do not create loops on every render.
- [ ] Remove automatic `deleteTempResult` from `useViewerTabs`; preserve its public tab operations. Update closure tests to assert no destructive requests.
- [ ] Run targeted Vitest tests and TypeScript lint.

## Task 3: Recovery UI

Files: `FilesView.tsx`, `TempResultView.tsx`, `components/SearchResultViewer.tsx`, viewer pagination/search error paths and behavior tests.

- [ ] Write failing UI tests for stale result notice, loaded-content preservation, disabled paging/filtering and explicit replay.
- [ ] Wire retention to all open FilesView results and the standalone TempResultView. Distinguish temporary-result 404 from transient network errors. Retain tabs when unavailable.
- [ ] Reuse `restoreSharedTab({ kind: 'search', plan })` to replay query plans on explicit action. Keep old snapshot; display replay loading/failure and current-data notice. Tabs without a query plan get source-search guidance.
- [ ] Run UI tests, lint, build and complete frontend tests.

## Task 4: Documentation, verification and PR

- [ ] Update README lifetime semantics and document suspension/explicit deletion limitations.
- [ ] Review requirements and final diff. Run backend tests/check/clippy/fmt and frontend tests/lint/build. Fix material review findings and rerun affected checks.
- [ ] Commit on `codex/search-result-retention`, push and create a PR targeting main, with behavior and validation evidence. Leave worktree available for review.
