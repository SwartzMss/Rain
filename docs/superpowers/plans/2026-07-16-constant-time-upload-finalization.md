# Constant-Time Upload Finalization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate per-file metadata rewrites after an uploaded Bundle directory is moved, reducing finalization from O(file count) to O(1).

**Architecture:** Stop persisting staging absolute paths for newly ingested files and resolve finalized files from the already-stable `files.path`. Keep legacy `meta.storage_path` support for reads and deletion, then simplify the ready finalizer to one Bundle update with its existing retry behavior.

**Tech Stack:** Rust 2024, Tokio, sqlx/SQLite, Serde JSON, Actix Web.

---

### Task 1: Remove staging absolute paths from new metadata

**Files:**
- Modify: `backend/src/ingest.rs`
- Test: `backend/src/ingest.rs`

- [ ] **Step 1: Write failing metadata tests**

Add tests around a small metadata-construction helper and assert uploaded files, extracted directories, extracted files, and nested extracted directories contain classification fields but no `storage_path`.

- [ ] **Step 2: Verify RED**

Run: `cargo test ingest::tests --lib`

Expected: compilation fails because the metadata helper does not exist or assertions observe `storage_path`.

- [ ] **Step 3: Centralize stable metadata construction**

Create focused helpers that produce metadata without filesystem locations, for example:

```rust
fn file_meta(kind: &str, preview_kind: PreviewKind) -> serde_json::Value {
    serde_json::json!({
        "kind": kind,
        "preview_kind": preview_kind.as_str()
    })
}
```

Preserve `original_name`, `display_name`, `storage_name`, and archive `source` fields where currently present. Replace all four `storage_path` writes in upload and recursive archive ingestion.

- [ ] **Step 4: Verify GREEN**

Run: `cargo test ingest::tests --lib`

Expected: all ingest unit tests pass.

### Task 2: Resolve and delete files through stable paths

**Files:**
- Modify: `backend/src/repositories/files.rs`
- Modify: `backend/src/services/file_deletion.rs`
- Modify: `backend/src/routes/files.rs`
- Test: `backend/src/repositories/files.rs`
- Test: `backend/tests/smoke.rs`

- [ ] **Step 1: Write failing path-resolution tests**

Add tests proving a record without `storage_path` resolves to `data_root + files.path`, while a legacy record with a valid absolute `storage_path` remains supported. Add a test proving paths outside `data_root` are rejected.

- [ ] **Step 2: Verify RED**

Run: `cargo test repositories::files::tests --lib`

Expected: new shared storage-candidate helper is absent or the deletion path API lacks `data_root`.

- [ ] **Step 3: Implement compatible candidate resolution**

Extract a helper that selects legacy metadata when present and otherwise joins `data_root` with `FileRow.path`. Use it from `resolve_file_path`. Change `fetch_storage_paths_for_ids` to accept `data_root` and use the same fallback for each row.

- [ ] **Step 4: Thread data root into deletion**

Change the deletion interface to:

```rust
pub async fn delete_file_tree(
    pool: &sqlx::SqlitePool,
    data_root: &Path,
    bundle_id: &str,
    root_file_id: i64,
) -> Result<(), AppError>
```

Pass the configured root from the files route. Preserve deepest-first path sorting and deduplication.

- [ ] **Step 5: Add deletion regression coverage**

Extend smoke coverage so a ready file whose metadata has no `storage_path` is deleted from both SQLite and disk.

- [ ] **Step 6: Verify GREEN**

Run: `cargo test repositories::files::tests --lib && cargo test --test smoke upload_search_tree_and_delete_issue -- --nocapture`

Expected: focused repository and deletion scenarios pass.

### Task 3: Make ready finalization constant time

**Files:**
- Modify: `backend/src/upload/finalizer.rs`
- Modify: `backend/src/upload/job.rs`
- Test: `backend/src/upload/finalizer.rs`

- [ ] **Step 1: Write a failing finalizer test**

Create an in-memory/temp SQLite Bundle with many `files` rows containing deliberately invalid metadata JSON. Call the desired finalizer and assert the Bundle reaches `READY` without reading or modifying file rows. Capture file metadata before and after and assert equality.

- [ ] **Step 2: Verify RED**

Run: `cargo test upload::finalizer::tests --lib`

Expected: current finalizer fails while parsing invalid metadata or its old path arguments remain required.

- [ ] **Step 3: Simplify finalizer**

Remove `FileMetaRow`, the `SELECT files`, JSON parsing loop, and per-file `UPDATE`. Change ready finalization and retry to accept only `pool` and `bundle_id`, execute the single existing Bundle update, and update the job call site after the directory move.

- [ ] **Step 4: Verify GREEN**

Run: `cargo test upload::finalizer::tests --lib`

Expected: finalizer tests pass and file metadata remains unchanged.

### Task 4: Full verification and integration

**Files:**
- Review: all files modified above

- [ ] **Step 1: Format and inspect**

Run: `cargo fmt --check`

Run: `git diff --check`

Expected: both exit with status 0.

- [ ] **Step 2: Run backend verification**

Run: `cargo test` in `backend/`.

Expected: every unit, smoke, and doc test passes.

- [ ] **Step 3: Run frontend integration verification**

Run: `npm test && npm run build` in `frontend/`.

Expected: script tests pass and the production build completes.

- [ ] **Step 4: Review final behavior**

Confirm new metadata contains no staging path, legacy metadata remains readable/deletable, ready finalization issues no file query/update, and directory rename still precedes the Bundle status update.
