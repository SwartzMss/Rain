# Tantivy deletion consistency Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove stale Tantivy search results immediately after file deletion is enqueued and compact immutable generations in the background.

**Architecture:** Search routes obtain one `visible_files` snapshot and pass it into Tantivy filtering. Deletion enqueue marks the Bundle dirty with a monotonic revision while preserving the active generation. A periodic rebuild worker creates and verifies a new generation from visible documents, then publishes it conditionally; failed or superseded work leaves the active generation available.

**Tech Stack:** Rust, Actix Web, Tokio, SQLx SQLite migrations, Tantivy immutable generation artifacts.

---

### Task 1: Add visibility snapshot and filtered Tantivy search

**Files:**
- Create: `backend/src/search/visibility.rs`
- Modify: `backend/src/search/mod.rs`
- Modify: `backend/src/search/tantivy/mod.rs`
- Modify: `backend/src/routes/logs.rs`
- Test: `backend/src/search/tantivy/mod.rs`

- [ ] Add a failing unit test where a matching document's file ID is absent from a visibility set and assert it is excluded while a later visible document fills `size`.
- [ ] Run `cargo test --manifest-path backend/Cargo.toml --features tantivy-search tantivy::tests::visibility_filter_fills_page -- --exact`; expect failure because the filter does not exist.
- [ ] Implement one batched `SELECT id FROM visible_files WHERE bundle_id=?` snapshot helper and pass the resulting set into Bundle and Issue Tantivy searches.
- [ ] Filter before total and pagination, retaining all candidates needed for exact total and only `from + size` rows.
- [ ] Run the focused test and existing Tantivy parity tests; expect pass.

### Task 2: Mark publication dirty at deletion enqueue

**Files:**
- Create: `backend/migrations/0005_search_visibility_revisions.sql`
- Modify: `backend/src/services/file_deletion.rs`
- Modify: `backend/src/db/migrations.rs`
- Test: `backend/src/services/file_deletion.rs`

- [ ] Add a failing integration assertion that enqueueing deletion changes `bundle_search_indexes.state` to `NEEDS_REBUILD` and increments its revision while leaving the active generation queryable.
- [ ] Add migration columns/defaults for `visibility_revision`, `compacted_revision`, and an active generation representation compatible with existing rows.
- [ ] Update enqueue SQL in the same write transaction as job insertion; make repeated enqueue idempotent.
- [ ] Run the focused deletion tests and verify `visible_files` hides the subtree before the worker runs.

### Task 3: Rebuild and publish generations safely

**Files:**
- Create: `backend/src/search/rebuild.rs`
- Modify: `backend/src/search/publication.rs`
- Modify: `backend/src/routes/mod.rs`
- Modify: `backend/src/main.rs`
- Modify: `backend/src/search/tantivy/writer.rs`
- Test: `backend/tests/search_publication.rs`

- [ ] Add failing tests for copying only visible documents, conditional publication after a second deletion, and failed rebuild preserving the old generation.
- [ ] Implement a bounded rebuild worker that reads stored documents from the active artifact, copies visible documents into staging, commits/verifies, and publishes only when the Bundle and target revision still match.
- [ ] Keep active generation/state queryable during BUILDING; record covered revision separately from active generation.
- [ ] Add periodic scheduling and startup cleanup for staging, failed, retired, and deleted Bundle artifacts; make cleanup retryable and lease-safe.
- [ ] Run publication/rebuild tests, then full locked test, clippy, and fmt checks.

### Task 4: Share the visibility contract with Skill Search

**Files:**
- Modify: `backend/src/services/skill_tools.rs`
- Modify: `backend/src/search/tantivy/mod.rs`
- Test: `backend/tests/search_backend_parity.rs`

- [ ] Add a regression test proving a hidden file cannot be returned through the Skill Tantivy path.
- [ ] Route Skill Tantivy queries through the same snapshot/filter helper and preserve existing short-literal and time coverage behavior.
- [ ] Run the parity and Skill tests.
