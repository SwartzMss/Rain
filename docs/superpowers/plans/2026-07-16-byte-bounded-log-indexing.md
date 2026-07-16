# Byte-Bounded Log Indexing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bound log-indexing batches by content bytes as well as line counts and split indexed-line limits from API preview-line limits.

**Architecture:** Introduce distinct indexing and API configuration fields, then make `LogChunk` build and measure its segment content incrementally. A small `IndexBatchBudget` owns transaction counters so both thresholds are testable without multi-gigabyte fixtures; `ingest_text_file` remains responsible for transaction lifecycle and line offsets.

**Tech Stack:** Rust, Tokio, SQLx/SQLite FTS5, Actix Web, Cargo tests

---

### Task 1: Split indexed-line and preview-line configuration

**Files:**
- Modify: `backend/src/config.rs`
- Modify: `backend/src/services/file_reader.rs`
- Modify: `backend/src/routes/files.rs`
- Modify: `backend/tests/smoke.rs`
- Modify: `backend/.env.example`
- Modify: `README.md`

- [ ] **Step 1: Write failing configuration tests**

In `backend/src/config.rs`, change `defaults_expose_only_meaningful_workflow_limits` to assert:

```rust
assert_eq!(limits.indexing.max_indexed_line_size, 256 * 1024);
assert_eq!(limits.api.max_preview_line_size, 8 * 1024_u64.pow(2));
```

Add an environment test that sets `RAIN_INDEXING_MAX_INDEXED_LINE_SIZE=512 KiB` and `RAIN_API_MAX_PREVIEW_LINE_SIZE=4 MiB`, calls `AppLimits::from_env()`, restores both variables, and asserts both values. Add zero-value validation assertions whose errors contain the respective new variable names.

In `backend/tests/smoke.rs`, configure the app with an indexed-line limit of 64 bytes and a preview-line limit of 256 bytes. Upload a first line longer than 256 bytes with a prefix token before byte 64 and a suffix token after byte 64, followed by a normal second line. Assert prefix search succeeds, suffix search has zero hits, the lines endpoint returns a truncated first-line preview longer than 64 bytes but no longer than the configured preview limit plus the existing marker, and the second line remains complete at source line 1.

- [ ] **Step 2: Run the config test and verify it fails**

Run: `cargo test config::tests --locked` from `backend/`

Expected: compilation failure because the new config fields do not exist.

- [ ] **Step 3: Implement the new configuration fields**

Change the structs to:

```rust
pub struct IndexingConfig {
    pub max_indexed_line_size: u64,
}

pub struct ApiConfig {
    pub file_preview_size: u64,
    pub max_preview_line_size: u64,
    pub default_line_page_size: i64,
    pub max_line_page_size: i64,
    pub default_search_results: i64,
    pub max_search_results: i64,
}
```

Use defaults of 256 KiB and 8 MiB. Read only `RAIN_INDEXING_MAX_INDEXED_LINE_SIZE` and `RAIN_API_MAX_PREVIEW_LINE_SIZE`; delete all runtime references to `RAIN_INDEXING_MAX_LINE_SIZE`. Validate positivity and `usize` conversion with errors naming the new variables.

Update `read_file_lines` to accept `&ApiConfig` and use `api.max_preview_line_size`. Update `routes/files.rs` to pass `&state.limits.api`. In the smoke fixture set both `limits.indexing.max_indexed_line_size` and `limits.api.max_preview_line_size` to 64 KiB where the existing long-line expectations require it.

- [ ] **Step 4: Update current configuration documentation**

Replace the old variable in `backend/.env.example` and the README configuration table with:

```text
RAIN_INDEXING_MAX_INDEXED_LINE_SIZE=256 KiB
RAIN_API_MAX_PREVIEW_LINE_SIZE=8 MiB
```

Explain that the first limits searchable prefixes and the second limits a line returned by pagination. Do not rewrite historical design/plan documents.

- [ ] **Step 5: Run configuration and smoke compilation checks**

Run: `cargo test config::tests --locked && cargo test --test smoke --no-run --locked` from `backend/`

Expected: config tests pass and the smoke test target compiles.

- [ ] **Step 6: Commit the configuration split**

```bash
git add backend/src/config.rs backend/src/services/file_reader.rs backend/src/routes/files.rs backend/tests/smoke.rs backend/.env.example README.md
git commit -m "feat: split indexing and preview line limits"
```

### Task 2: Make chunks byte-aware and reuse constructed content

**Files:**
- Modify: `backend/src/ingest/limits.rs`
- Modify: `backend/src/ingest.rs`

- [ ] **Step 1: Write failing `LogChunk` unit tests**

Add tests that push `"alpha"` and `"世界"` and assert:

```rust
assert_eq!(chunk.len(), 2);
assert_eq!(chunk.byte_len(), "alpha\n世界".len());
assert_eq!(chunk.content(), "alpha\n世界");
```

Add a test that creates a chunk with a 256 KiB string and asserts `chunk.reached_target(INDEX_CHUNK_MAX_LINES, INDEX_CHUNK_TARGET_BYTES)`. Add a separate 200-short-line test proving the line threshold independently returns true.

- [ ] **Step 2: Run the focused tests and verify they fail**

Run: `cargo test ingest::tests::log_chunk --locked` from `backend/`

Expected: compilation failure because `byte_len` and `reached_target` do not exist.

- [ ] **Step 3: Implement incremental chunk content**

Rename constants in `backend/src/ingest/limits.rs` and add byte targets:

```rust
pub const INDEX_CHUNK_MAX_LINES: usize = 200;
pub const INDEX_CHUNK_TARGET_BYTES: usize = 256 * 1024;
pub const INDEX_COMMIT_MAX_LINES: i64 = 5_000;
pub const INDEX_COMMIT_TARGET_BYTES: usize = 16 * 1024 * 1024;
```

Replace `LogChunk.lines` with `line_count: usize` and `content: String`. In `push`, append `\n` before every line after the first, update line bounds, and append the content. Implement `byte_len`, `len`, `is_empty`, `content(&self) -> &str`, and `reached_target(max_lines, target_bytes)`.

Update `flush_log_chunks` to bind `chunk.content()` to both SQL builders without reconstructing it. Preserve segment batching and chunk-index mapping checks.

- [ ] **Step 4: Run chunk and database write tests**

Run: `cargo test ingest::tests::log_chunk --locked && cargo test ingest::tests::batches_segments_and_fts_with_chunk_index_mapping --locked && cargo test ingest::tests::batch_segment_failure_rolls_back_segments_and_fts_together --locked` from `backend/`

Expected: all focused tests pass.

- [ ] **Step 5: Commit byte-aware chunks**

```bash
git add backend/src/ingest/limits.rs backend/src/ingest.rs
git commit -m "refactor: bound log chunks by content bytes"
```

### Task 3: Add byte-aware transaction budgets

**Files:**
- Modify: `backend/src/ingest.rs`

- [ ] **Step 1: Write failing budget unit tests**

Introduce tests for the desired `IndexBatchBudget` API:

```rust
let mut budget = IndexBatchBudget::default();
budget.record_line();
budget.record_chunk(INDEX_COMMIT_TARGET_BYTES);
assert!(budget.should_commit());
budget.reset();
assert!(!budget.should_commit());
```

Add a line-only test that calls `record_line()` `INDEX_COMMIT_MAX_LINES` times and asserts commit without recording bytes. Add a `usize::MAX` byte test followed by another byte record to prove saturating accumulation.

- [ ] **Step 2: Run the budget tests and verify they fail**

Run: `cargo test ingest::tests::index_batch_budget --locked` from `backend/`

Expected: compilation failure because `IndexBatchBudget` does not exist.

- [ ] **Step 3: Implement and integrate the budget**

Add a private `IndexBatchBudget { lines_since_commit: i64, pending_bytes: usize }` with `record_line`, `record_chunk`, `should_commit`, and `reset` using the constants from `ingest/limits.rs` and saturating addition.

In `ingest_text_file`:

- read lines with `indexing.max_indexed_line_size`;
- call `budget.record_line()` for every raw input line;
- close the current chunk when `reached_target` is true, record its bytes, and push it;
- when `budget.should_commit()` is true, close and record any nonempty current chunk, flush pending chunks, commit, clear pending chunks, reset the budget, and begin a new transaction;
- preserve final flush, offset insertion, line count, and chunk index ordering.

- [ ] **Step 4: Run all ingest tests**

Run: `cargo test ingest::tests --locked` from `backend/`

Expected: all ingest tests pass.

- [ ] **Step 5: Commit byte-aware transaction batching**

```bash
git add backend/src/ingest.rs
git commit -m "perf: bound indexing transactions by bytes"
```

### Task 4: Full verification

**Files:**
- Verify: `backend/src/config.rs`
- Verify: `backend/src/ingest.rs`
- Verify: `backend/src/ingest/limits.rs`
- Verify: `backend/src/services/file_reader.rs`
- Verify: `backend/src/routes/files.rs`
- Verify: `backend/tests/smoke.rs`
- Verify: `backend/.env.example`
- Verify: `README.md`

- [ ] **Step 1: Format and check formatting**

Run: `cargo fmt --check` from `backend/`

Expected: exit code 0.

- [ ] **Step 2: Run backend checks and tests**

Run: `cargo check --locked && cargo clippy --locked -- -D warnings && cargo test --locked` from `backend/`

Expected: all commands pass with no warnings or test failures.

- [ ] **Step 3: Build the frontend because it is embedded in release binaries**

Run: `npm run build` from `frontend/`

Expected: TypeScript and Vite complete successfully.

- [ ] **Step 4: Verify obsolete runtime configuration is gone**

Run from repository root:

```bash
rg -n "RAIN_INDEXING_MAX_LINE_SIZE|max_line_size" backend/src backend/tests backend/.env.example README.md
```

Expected: no matches.

- [ ] **Step 5: Inspect final state**

Run: `git diff --check && git status --short` from the repository root.

Expected: no whitespace errors and no uncommitted implementation changes.
