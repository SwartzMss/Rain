# Optimize Database Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bound structured-event storage and replace multi-million-row cleanup transactions with observable batches and best-effort WAL truncation.

**Architecture:** Introduce a bundle-scoped event budget alongside the existing archive budget, leaving full-text segment indexing complete after structured-event capacity is reached. Centralize dependent-row deletion in `db.rs` as autocommitted 10,000-row batches, then let each caller delete its bundle record and request WAL maintenance after large cleanups.

**Tech Stack:** Rust, Tokio, SQLx, SQLite, Actix Web, tracing.

---

### Task 1: Configure a bundle event limit

**Files:**
- Modify: `backend/src/config.rs`
- Modify: `backend/.env.example`
- Test: `backend/src/config.rs`

- [ ] Add failing tests asserting the default `indexing.max_events_per_bundle` is `250_000`, `RAIN_INDEXING_MAX_EVENTS_PER_BUNDLE` overrides it, and zero is rejected.
- [ ] Run `cargo test --locked config::tests -- --nocapture` and confirm compilation/assertions fail before the field exists.
- [ ] Add the positive `usize` configuration field, environment parsing, validation, and Chinese `.env.example` comments explaining that full-text indexing continues after the structured-event cap.
- [ ] Run the focused configuration tests and commit with `git commit -m "feat: configure bundle event indexing limit"`.

### Task 2: Stop structured-event growth at the cap

**Files:**
- Modify: `backend/src/ingest.rs`
- Modify: `backend/src/routes/uploads.rs`
- Test: `backend/src/ingest.rs`

- [ ] Add failing tests for an `EventBudget` shared across clones that permits exactly the configured number of reservations and reports the cap only once.
- [ ] Run `cargo test --locked ingest::tests::event_budget -- --nocapture` and confirm failure before the type exists.
- [ ] Implement an atomic bundle-scoped `EventBudget`; create one per upload task and pass it through `ProcessFileOptions` to every file and nested archive operation.
- [ ] Reserve before inserting `log_events`; after capacity is reached skip only structured-event rows, retain `log_segments` and FTS insertion, and emit one warning containing bundle ID and limit.
- [ ] Run ingest tests and the upload smoke test, then commit with `git commit -m "feat: bound structured event indexing"`.

### Task 3: Delete bundle data in bounded batches

**Files:**
- Modify: `backend/src/db.rs`
- Modify: `backend/src/routes/uploads.rs`
- Modify: `backend/src/routes/issues.rs`
- Test: `backend/tests/smoke.rs`

- [ ] Add a failing smoke test inserting more than one small test batch of events, offsets, segments, FTS rows, and files, then assert `cleanup_bundle_content_batched` removes all dependent content while preserving the bundle row and reports multiple batches.
- [ ] Run the focused smoke test and confirm the helper is absent.
- [ ] Implement a generic `DELETE ... WHERE rowid IN (SELECT rowid ... LIMIT ?)` loop and `BundleCleanupStats`; remove events, line offsets, FTS rows, segments, and files in dependency order with per-phase affected/batch/elapsed logs.
- [ ] Replace failed-upload cleanup, bundle deletion, issue deletion, and retention cleanup duplicate transactions with the shared helper, keeping terminal FAILED state and existing filesystem best-effort behavior.
- [ ] Run backend smoke tests and commit with `git commit -m "fix: batch large bundle cleanup"`.

### Task 4: Checkpoint WAL and expose disk sizes

**Files:**
- Modify: `backend/src/db.rs`
- Modify: `backend/src/main.rs`
- Modify: `backend/src/routes/uploads.rs`
- Modify: `backend/src/routes/issues.rs`
- Test: `backend/src/db.rs`

- [ ] Add tests for parsing SQLite checkpoint rows and resolving main/WAL/SHM diagnostic paths without requiring the files to exist.
- [ ] Run focused DB tests and confirm the new API is absent.
- [ ] Add best-effort `PRAGMA wal_checkpoint(TRUNCATE)` returning busy/log/checkpointed counts and log its result after large cleanup; failures remain non-fatal.
- [ ] Log startup file sizes for the SQLite main file and existing `-wal`/`-shm` sidecars; explicitly do not run automatic `VACUUM`.
- [ ] Run focused tests and commit with `git commit -m "feat: maintain and diagnose sqlite wal"`.

### Task 5: Verify and publish

**Files:**
- Verify all modified files.

- [ ] Run `cargo fmt --check`, `cargo clippy --locked -- -D warnings`, and `cargo test --locked`.
- [ ] Run `git diff --check` and inspect `git diff origin/main...HEAD` for unrelated changes.
- [ ] Push `agent/optimize-database-cleanup` and open a PR targeting `main` with the observed 3.2-million-row/1.14-GiB-WAL root cause and validation evidence.
