# Batched Ingest SQL Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce SQLite statement and commit overhead during archive ingestion and text indexing without extending write-lock duration across filesystem work.

**Architecture:** Split directory ingestion into discovery/classification, a short direct-child insert transaction, and post-commit recursive/index work. Buffer log chunks per existing 5000-line transaction and use `sqlx::QueryBuilder` for segment, FTS, and offset multi-row inserts with bounded batch sizes.

**Tech Stack:** Rust 2024, Tokio, sqlx 0.7 SQLite, QueryBuilder, FTS5.

---

### Task 1: Batch line-offset writes

**Files:**
- Modify: `backend/src/ingest.rs`

- [ ] **Step 1: Write a failing offset batch test**

Add a test that prepares an in-memory schema, creates one file, passes 1201 `(line_number, byte_offset)` pairs to a desired `insert_line_offsets` helper, and asserts all rows and boundary values are stored.

- [ ] **Step 2: Verify RED**

Run: `cargo test ingest::tests::inserts_line_offsets_in_complete_batches --lib`

Expected: compilation fails because `insert_line_offsets` does not exist.

- [ ] **Step 3: Implement bounded QueryBuilder insertion**

Add `LINE_OFFSET_BATCH_SIZE = 500` and implement:

```rust
async fn insert_line_offsets(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    file_id: i64,
    offsets: &[(i64, i64)],
) -> Result<(), AppError>
```

For each `offsets.chunks(500)`, build one multi-row `INSERT INTO log_line_offsets (file_id, line_number, byte_offset)` using three binds per row. Replace the current per-offset loop after the delete statement.

- [ ] **Step 4: Verify GREEN**

Run: `cargo test ingest::tests::inserts_line_offsets_in_complete_batches --lib`

Expected: test passes with 1201 persisted rows.

### Task 2: Batch segment and FTS writes per commit window

**Files:**
- Modify: `backend/src/ingest.rs`

- [ ] **Step 1: Write failing segment-batch tests**

Add a test creating 125 `LogChunk` values, call a desired `flush_log_chunks`, and assert 125 segment rows and 125 FTS rows exist with matching `chunk_index`/content. Add a second test with a deliberate FTS constraint/schema failure inside a transaction and assert no segment rows remain after rollback.

- [ ] **Step 2: Verify RED**

Run: `cargo test ingest::tests::batch --lib`

Expected: compilation fails because the batch helper is absent.

- [ ] **Step 3: Implement segment batch insertion**

Add `SEGMENT_BATCH_SIZE = 100`. Implement a helper that chunks pending `LogChunk` references, uses `QueryBuilder<Sqlite>` to insert segment rows with `RETURNING id, chunk_index`, builds a `HashMap<chunk_index, segment_id>`, and issues a bounded multi-row insert into `log_segments_fts`. Reject missing or duplicate returned chunk indexes as invalid database state.

- [ ] **Step 4: Buffer chunks per existing transaction window**

Replace immediate `flush_log_chunk` calls with a `pending_chunks: Vec<LogChunk>`. Continue closing chunks every 200 effective lines, but call `flush_log_chunks` only before each 5000-line commit and at EOF. Keep segment and FTS writes in the same transaction.

- [ ] **Step 5: Verify GREEN**

Run: `cargo test ingest::tests::batch --lib`

Expected: batch mapping and rollback tests pass.

### Task 3: Use short directory-level file insert transactions

**Files:**
- Modify: `backend/src/ingest.rs`

- [ ] **Step 1: Write failing transaction tests**

Extract a direct-child insertion helper and add a test with three prepared child records proving all are inserted with parent IDs in one transaction. Add a failure case containing an invalid row and assert the directory has zero new direct children after rollback.

- [ ] **Step 2: Verify RED**

Run: `cargo test ingest::tests::directory_child --lib`

Expected: compilation fails because prepared-child and batch insertion helpers do not exist.

- [ ] **Step 3: Make file insertion executor-aware**

Change `insert_file_record` to execute through `&mut Transaction<Sqlite>` for directory children. Keep a small pool wrapper for the top-level uploaded file and archive root records where no directory batch exists.

- [ ] **Step 4: Split directory discovery from processing**

Create a `PreparedDirectoryEntry` containing disk path, DB path, name, type, size, MIME, preview kind, and metadata. Discover/classify/reserve quota for all direct children before opening the transaction. Insert all prepared entries in one transaction, commit, then zip IDs back to prepared entries and perform recursion, text indexing, and nested archive extraction.

- [ ] **Step 5: Verify GREEN**

Run: `cargo test ingest::tests::directory_child --lib`

Expected: success and rollback tests pass without holding a transaction during post-insert work.

### Task 4: Full verification and review

**Files:**
- Modify if necessary: `backend/tests/smoke.rs`

- [ ] **Step 1: Run focused upload smoke**

Run: `cargo test --test smoke upload_search_tree_and_delete_issue -- --nocapture`

Expected: upload, nested archive, search, tree, preview, and delete checks pass.

- [ ] **Step 2: Run full backend verification**

Run: `cargo fmt --check && cargo test`

Expected: all tests pass with zero formatting errors.

- [ ] **Step 3: Run frontend integration verification**

Run: `npm test && npm run build`

Expected: all script tests and production build pass.

- [ ] **Step 4: Inspect SQL shape**

Confirm no per-offset INSERT loop remains, chunks are flushed only in bounded groups, segment and FTS inserts share a transaction, directory transactions contain only file INSERT statements, and no structured-event code is introduced.
