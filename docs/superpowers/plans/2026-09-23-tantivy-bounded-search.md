# Tantivy bounded search Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Bundle Tantivy search verify every exact match without materializing the complete hit set, while preserving totals, ordering, filters, and pagination.

**Architecture:** Build the n-gram query directly as a Tantivy scorer query, optionally adding the indexed `file_id` term. Scan matching document IDs segment-by-segment, apply visibility/timeline/path filters before exact substring verification, count every accepted match, and retain only the smallest `from + size` ordered window in a max heap. Return that window as the page and emit bounded-scan metrics.

**Tech Stack:** Rust, Tantivy 0.26, `BinaryHeap`, existing Bundle schema and Actix search result types.

---

### Task 1: Define the bounded query API and regression tests

**Files:**
- Modify: `backend/src/search/tantivy/query.rs`
- Modify: `backend/src/search/tantivy/mod.rs`

- [x] **Step 1: Write failing tests** for exact verification, file-id prefilter metrics, deep pagination ordering, and high-hit page bounding using `SearchOptions`/`search_page`.
- [x] **Step 2: Run the focused Tantivy tests** and confirm they fail because the bounded API is absent.

### Task 2: Implement bounded Tantivy candidate scanning

**Files:**
- Modify: `backend/src/search/tantivy/query.rs`

- [x] **Step 1: Add search options, page/metric result types, and a max-heap rank wrapper.**
- [x] **Step 2: Build n-gram plus optional indexed `file_id` BooleanQuery and iterate segment scorers instead of `TopDocs`.**
- [x] **Step 3: Apply stored-field filters before lowercasing content, verify exact contiguous substring matches, count totals, and retain only the requested ordered window.**
- [x] **Step 4: Emit `tracing::debug!` candidate/verification/read/return metrics and keep `search` as a compatibility wrapper.**
- [x] **Step 5: Run focused tests and refactor only after green.**

### Task 3: Route Bundle search through the bounded API

**Files:**
- Modify: `backend/src/search/tantivy/mod.rs`

- [x] **Step 1: Pass request pagination and all filters into `CandidateSearch::search_page`.**
- [x] **Step 2: Convert the bounded hits to `ContentSearchRow` without a second full-result materialization, preserving total and sort order.**
- [x] **Step 3: Remove the obsolete full-vector visibility filter and update its unit test to cover bounded visibility pagination.**

### Task 4: Verify repository behavior

**Files:**
- No additional production files.

- [x] **Step 1: Run `cargo fmt --check` and `cargo test --lib`.
- [x] **Step 2: Run `cargo clippy --lib --all-features -- -D warnings` and `git diff --check`; full-target clippy is blocked by pre-existing frontend/test issues.**
- [x] **Step 3: Review the diff for unchanged total semantics, exact substring correctness, bounded retained hits, and no `usize::MAX`/`num_docs` candidate limit in the search path.**
- [x] **Step 4: Commit the implementation with a focused message.**
