# Materialized Search Preview Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Scan original logs once per detailed search and serve every later page from an indexed temporary result while preserving source navigation metadata.

**Architecture:** Extend the temp-result executor so preview scans write a `.log`, newline-delimited `.meta`, and sparse `.idx` sidecar in one pass. Register the materialized preview in the existing `temp_results` table, enhance the lines endpoint to seek through the sparse index, and make search tabs page by returned `result_id` instead of rerunning the expression.

**Tech Stack:** Rust 2024, Tokio async file I/O, Actix Web, SQLite/sqlx, Serde JSON, React 18, TypeScript.

---

### Task 1: Materialized result writer and sparse index

**Files:**
- Modify: `backend/src/services/temp_results.rs`

- [ ] **Step 1: Write failing unit tests**

Add Tokio tests that create two source files, call a new `materialize_preview` API, and assert that it returns the requested first page, writes all matches, preserves source metadata, and records checkpoints at result lines `0` and `1000`. Add a test for selecting the greatest checkpoint whose result line is not greater than `start`.

- [ ] **Step 2: Run tests and verify RED**

Run: `cargo test services::temp_results::tests --lib`

Expected: compilation fails because `materialize_preview`, `MaterializedPreview`, `MatchMetadata`, and sparse-index helpers do not exist.

- [ ] **Step 3: Implement the streaming writer**

Introduce serializable sidecar records with these stable fields:

```rust
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct MatchMetadata {
    pub bundle_hash: Option<String>,
    pub file_id: Option<String>,
    pub path: String,
    pub line_number: i64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct SparseCheckpoint {
    pub result_line: i64,
    pub log_offset: u64,
    pub meta_offset: u64,
}

pub struct MaterializedPreview {
    pub total: i64,
    pub lines: Vec<PreviewLine>,
}
```

Add `TempResultExecutor::materialize_preview(sources, expression, size, log, meta, index)`. For every match, write a checkpoint before every 1000th result, append the full line to `.log`, append one JSON metadata record plus newline to `.meta`, and collect only the first `size` `PreviewLine` values. Track byte offsets from the exact bytes written rather than querying file position after every line.

- [ ] **Step 4: Run tests and verify GREEN**

Run: `cargo test services::temp_results::tests --lib`

Expected: all temp-result service tests pass.

### Task 2: Preview API persistence and indexed pagination

**Files:**
- Modify: `backend/src/routes/temp_results.rs`
- Modify: `backend/tests/smoke.rs`

- [ ] **Step 1: Write failing HTTP integration tests**

Extend the smoke test to assert:

```rust
assert!(temporary_preview["result_id"].as_str().is_some());
```

Use that ID with `/api/temp-results/{id}/lines`, assert source fields and original line numbers are returned, and assert `.log`, `.meta`, and `.idx` are removed by DELETE. Add enough generated matches to request a page after line 1000 and validate it starts at the requested result without rescanning a source path.

- [ ] **Step 2: Run the focused smoke test and verify RED**

Run: `cargo test --test smoke smoke -- --nocapture`

Expected: assertion fails because preview has no `result_id` and does not register files.

- [ ] **Step 3: Persist preview results**

Change the response shape to:

```rust
#[derive(Serialize)]
pub struct TempPreview {
    pub result_id: String,
    pub total: i64,
    pub lines: Vec<PreviewLine>,
}
```

In `preview_temp_result`, allocate the ID and three paths, run `materialize_preview`, insert the existing temp-result metadata row, and return the ID with the collected first page. Centralize insertion and cleanup so failed scans and failed inserts remove every produced file.

- [ ] **Step 4: Add indexed result reading**

Enhance `TempLine` with optional `bundle_hash`, `file_id`, and `path`. If `.meta` and `.idx` exist, load the last checkpoint at or before `start`, seek both readers with `AsyncSeekExt`, skip at most 999 result records, and zip content with decoded metadata. Return an internal-server error for malformed or prematurely truncated sidecars. Fall back to current sequential `.log` reading when sidecars are absent.

- [ ] **Step 5: Clean up all sidecars**

Replace single-file removal in explicit deletion, expired cleanup, and insert-error cleanup with a helper that removes `.log`, `.meta`, and `.idx`, ignoring only `NotFound`.

- [ ] **Step 6: Run backend tests**

Run: `cargo test`

Expected: all backend tests pass.

### Task 3: Frontend paging by materialized result ID

**Files:**
- Modify: `frontend/src/api/types.ts`
- Modify: `frontend/src/features/files/viewerTabs.ts`
- Modify: `frontend/src/features/files/FilesView.tsx`

- [ ] **Step 1: Create a failing type/build check through desired types**

Make `TempResultPreviewResponse.result_id` required and add optional `bundle_hash`, `file_id`, and `path` fields to temporary result lines. Add required `resultId: string` to `SearchViewerTab`. Run the build before updating call sites.

- [ ] **Step 2: Run build and verify RED**

Run: `npm run build`

Expected: TypeScript errors at search-tab construction sites because `resultId` is missing.

- [ ] **Step 3: Store and use the result ID**

At every preview response, store `response.result_id` in the new search tab. Replace the `tab.kind === 'search'` paging payload reconstruction and `previewTempResult` call with:

```ts
const response = await rainApi.fetchTempResultLines(tab.resultId, {
  start: from,
  limit: pageSize
});
```

Map the returned source fields back into `IssueLogSearchHit`. When filtering an existing search tab, always use `{ source_temp_id: activeViewerTab.resultId }`, so nested searches consume the already materialized content.

- [ ] **Step 4: Run frontend verification**

Run: `npm test && npm run build`

Expected: all script tests pass and the production build completes.

### Task 4: Full verification and documentation consistency

**Files:**
- Modify if needed: `docs/superpowers/specs/2026-07-16-materialized-search-preview-design.md`

- [ ] **Step 1: Format and inspect changes**

Run: `cargo fmt --check`

Run: `git diff --check`

Expected: both commands exit successfully.

- [ ] **Step 2: Run full verification**

Run: `cargo test` in `backend/`.

Run: `npm test && npm run build` in `frontend/`.

Expected: every command exits with status 0.

- [ ] **Step 3: Review final diff**

Confirm preview scans original sources once, search paging calls only the result-lines endpoint, source metadata survives pagination, old `.log`-only results retain fallback behavior, and all sidecars share the existing lifecycle.
