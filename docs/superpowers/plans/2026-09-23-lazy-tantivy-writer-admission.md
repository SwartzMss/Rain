# Lazy Tantivy Writer Admission Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delay Tantivy writer resource admission until the first searchable batch while preserving publication, cancellation, backpressure, and empty-index behavior.

**Architecture:** `BundleBuildSession::start` will create only control-plane state and heartbeat. The first non-empty searchable batch will atomically acquire the writer permit and initialize the bounded pipeline; the permit is handed to the blocking writer task and remains held until that task has fully finished or aborted. The pipeline will expose explicit finish and abort paths so a blocking writer task cannot outlive resource cleanup.

**Tech Stack:** Rust, Tokio, SQLite publication state machine, Tantivy, bounded `mpsc` pipeline, existing `SearchResourceBudget`.

---

### Task 1: Add failing admission and lifecycle tests

**Files:**
- Modify: `backend/src/search/tantivy/publication.rs`
- Modify: `backend/src/search/tantivy/pipeline.rs`
- Modify: `backend/src/search/resource.rs`

- [x] **Step 1: Add tests proving session start does not acquire a writer permit, first searchable batch does, and an empty session finishes with a valid empty index.**
- [x] **Step 2: Add tests for cancellation before writer start and after writer start, asserting active and queued writer counters return to zero.**
- [x] **Step 3: Run focused tests and verify the new expectations fail against eager admission.**

### Task 2: Implement lazy BundleBuildSession initialization

**Files:**
- Modify: `backend/src/search/tantivy/publication.rs`

- [x] **Step 1: Replace eager pipeline/permit fields with an initialization state that can be entered once by `commit_ingest_batch`.**
- [x] **Step 2: Acquire the permit and create the pipeline only for the first non-empty indexed batch, preserving the bounded queue and expected document accounting.**
- [x] **Step 3: Make `finish` create and commit a valid empty index when no searchable batch arrived, then run the existing verification and publication transition.**
- [x] **Step 4: Make `abort` and Drop handle both not-started and started states without leaking permits or staging artifacts.**

### Task 3: Make pipeline shutdown explicit and awaitable

**Files:**
- Modify: `backend/src/search/tantivy/pipeline.rs`

- [x] **Step 1: Add an abort operation that closes the sender, awaits the blocking worker when possible, and distinguishes cancellation from a successful commit.**
- [x] **Step 2: Ensure normal finish waits for the writer task before returning so the session permit cannot be released while the worker is still writing.**
- [x] **Step 3: Preserve bounded producer backpressure and existing writer error propagation.**

### Task 4: Add observability and upload concurrency coverage

**Files:**
- Modify: `backend/src/upload/job.rs`
- Modify: `backend/tests/large_log_benchmark.rs`
- Modify: `backend/src/search/tantivy/publication.rs`

- [x] **Step 1: Emit separate processing queue, preflight, writer queue, active writer, and publish timing fields.**
- [x] **Step 2: Extend the opt-in benchmark's metric collection to report writer queue/active metrics for 1, 2, and 4 concurrency configurations.**
- [x] **Step 3: Add a multi-upload admission test with `max_writers=1` showing two sessions can preflight before any writer slot is occupied.**

### Task 5: Verify and commit

**Files:**
- No additional production files.

- [x] **Step 1: Run `cargo fmt --check`, `cargo test --locked --lib`, and focused integration tests.**
- [x] **Step 2: Run `cargo clippy --locked --lib -- -D warnings` and `git diff --check`.**
- [x] **Step 3: Review failure/cancellation/empty-index paths and commit the focused implementation.**
