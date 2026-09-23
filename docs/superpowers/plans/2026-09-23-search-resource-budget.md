# Search Resource Budget Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task with verification checkpoints.

**Goal:** Make Tantivy Bundle indexing use one explicit resource-budget object that bounds concurrent writers, exposes admission/build timing, and remains safe on success, failure, and cancellation.

**Architecture:** Add an unconditional `SearchResourceBudget` in the search module. It owns the Tantivy writer semaphore, per-writer heap setting, queued/active counters, and an owned permit wrapper. `SearchRuntime`, upload jobs, benchmark setup, and `BundleBuildSession` pass this object instead of separate semaphore and heap fields. Build-session tracing emits admission wait and build duration without changing the SQLite default or search results.

**Tech Stack:** Rust, Tokio `Semaphore`, `AtomicUsize`, `tracing`, Tantivy feature tests, existing ignored large-log benchmark.

---

### Task 1: Add failing resource-budget tests

**Files:**
- Create: `backend/src/search/resource.rs`
- Modify: `backend/src/search/mod.rs`

- [x] **Step 1: Write tests for permit capacity and release**

Add a `#[cfg(test)]` module in `resource.rs` with these tests:

```rust
#[tokio::test]
async fn second_writer_waits_until_the_first_is_dropped() {
    let budget = SearchResourceBudget::new(1, 64).unwrap();
    let first = budget.acquire().await.unwrap();
    assert_eq!(budget.active_writers(), 1);
    let blocked = tokio::time::timeout(Duration::from_millis(25), budget.acquire()).await;
    assert!(blocked.is_err());
    drop(first);
    let second = tokio::time::timeout(Duration::from_secs(1), budget.acquire())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(budget.active_writers(), 1);
    drop(second);
    assert_eq!(budget.active_writers(), 0);
}

#[tokio::test]
async fn cancelled_waiter_does_not_leave_queue_or_active_counts() {
    let budget = SearchResourceBudget::new(1, 64).unwrap();
    let first = budget.acquire().await.unwrap();
    let waiting = budget.clone();
    let task = tokio::spawn(async move { waiting.acquire().await });
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(budget.queued_writers(), 1);
    task.abort();
    let _ = task.await;
    assert_eq!(budget.queued_writers(), 0);
    assert_eq!(budget.active_writers(), 1);
    drop(first);
    assert_eq!(budget.active_writers(), 0);
}
```

Export the resource module from `backend/src/search/mod.rs` without enabling the Tantivy feature; the type must also be available to `AppState` in the default build.

- [x] **Step 2: Run the focused tests and confirm the expected RED state**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml --locked search::resource --lib
```

Expected: compilation fails because `SearchResourceBudget` and its methods do not exist yet.

### Task 2: Implement the resource budget and runtime wiring

**Files:**
- Modify: `backend/src/search/resource.rs`
- Modify: `backend/src/search/mod.rs`
- Modify: `backend/src/lib.rs:70-120, 490-505, 620-635`
- Modify: `backend/src/routes/uploads.rs:195-210`
- Modify: `backend/src/upload/job.rs:80-100, 350-370`

- [x] **Step 1: Implement the budget and owned permit**

Implement:

```rust
#[derive(Clone)]
pub struct SearchResourceBudget {
    writer_permits: Arc<Semaphore>,
    writer_heap_size_bytes: usize,
    queued_writers: Arc<AtomicUsize>,
    active_writers: Arc<AtomicUsize>,
}

pub struct SearchResourcePermit {
    permit: OwnedSemaphorePermit,
    active_writers: Arc<AtomicUsize>,
    queue_wait: Duration,
}
```

`new` must return `Result<Self, AppError>` and reject zero values with `AppError::Config`. `acquire` must increment the queued counter before awaiting, decrement it on both success and cancellation through a drop guard, increment active count after acquisition, and return the measured queue wait. Dropping `SearchResourcePermit` decrements active count and releases the owned semaphore permit. Expose `writer_heap_size_bytes`, `queue_wait`, `active_writers`, `queued_writers`, and `available_writers` for callers and tests.

- [x] **Step 2: Replace split runtime fields with the budget**

Change `SearchRuntime` to hold `tantivy_budget: SearchResourceBudget`. Construct it from the existing `SearchConfig` values in `SearchRuntime::new`. Update `AppState` construction, the runtime admission test, upload route job construction, and `UploadJob` to carry a cloned `SearchResourceBudget`. Preserve the existing environment variable names and default values.

- [x] **Step 3: Run the focused resource and state tests**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml --locked search::resource --lib
cargo test --manifest-path backend/Cargo.toml --locked state_uses_configured_tantivy_writer_admission --lib
```

Expected: all focused tests pass, including the cancellation count assertions.

### Task 3: Integrate budget acquisition and build metrics

**Files:**
- Modify: `backend/src/search/tantivy/publication.rs:1-180`
- Modify: `backend/src/upload/job.rs:340-370`

- [x] **Step 1: Update BundleBuildSession to acquire the budget**

Change `BundleBuildSession::start` to accept `SearchResourceBudget`, acquire a `SearchResourcePermit` before starting `BoundedBundlePipeline`, and store the permit on the session. Pass `budget.writer_heap_size_bytes()` to `PipelineConfig`. Keep the permit alive through publication verification and `mark_publication_ready` so no second writer can start while the first is still publishing.

- [x] **Step 2: Add admission and build tracing**

Record `queue_wait_ms` from the permit and `build_started = Instant::now()` in the session. Wrap `finish` so success and every error outcome emit one event:

```rust
tracing::info!(
    metric = "tantivy_index_build",
    bundle_id = %self.bundle_id,
    outcome,
    admission_wait_ms,
    build_elapsed_ms = build_started.elapsed().as_millis() as u64,
    active_writers = budget.active_writers(),
    writer_heap_size_bytes = budget.writer_heap_size_bytes(),
    "Tantivy Bundle index build completed"
);
```

Use an inner `finish_inner` helper so errors do not skip the metric. `abort` must keep the current artifact cleanup behavior and allow the session drop to release the permit.

- [x] **Step 3: Update publication tests and add release coverage**

Update `backend/tests/search_publication.rs` and any in-module callers to pass a `SearchResourceBudget`. Add a test that starts a build with one permit, drops/aborts it, then starts a second build within one second; this proves an aborted build cannot permanently hold admission.

- [x] **Step 4: Run Tantivy publication tests**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml --locked --features tantivy-search --test search_publication
cargo test --manifest-path backend/Cargo.toml --locked --features tantivy-search search::tantivy --lib
```

Expected: publication, artifact cleanup, pipeline, and search tests pass.

### Task 4: Extend benchmark collection and documentation

**Files:**
- Modify: `backend/tests/large_log_benchmark.rs:35-85, 315-385`
- Modify: `docs/performance/large-log-baseline.md`
- Modify: `docs/performance/tantivy-prototype.md`

- [x] **Step 1: Capture Tantivy build metrics in the benchmark subscriber**

Include `tantivy_index_build` in the accepted metrics and aggregate `admission_wait_ms`, `build_elapsed_ms`, `active_writers`, and `writer_heap_size_bytes`. Keep the existing 1/2/4 `RAIN_BENCH_CONCURRENCY` matrix and include the selected writer budget in the emitted JSON `config` object.

- [x] **Step 2: Document the benchmark commands and interpretation**

Document commands for `RAIN_BENCH_CONCURRENCY=1`, `2`, and `4` with `RAIN_SEARCH_BACKEND=tantivy`, and explain that higher concurrency is useful only when throughput rises without unbounded RSS or p95 query latency. Do not record unmeasured numbers as results.

- [x] **Step 3: Run the benchmark compile/test path**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml --locked --features tantivy-search --test large_log_benchmark --no-run
```

Expected: the ignored benchmark compiles with the new metrics and budget API.

### Task 5: Full verification and PR preparation

**Files:**
- Modify: `docs/superpowers/plans/2026-09-23-search-resource-budget.md` (check off completed steps)

- [x] **Step 1: Run formatting, checks, and tests**

Run:

```bash
cargo fmt --manifest-path backend/Cargo.toml -- --check
cargo check --manifest-path backend/Cargo.toml --locked
cargo check --manifest-path backend/Cargo.toml --locked --features tantivy-search
cargo clippy --manifest-path backend/Cargo.toml --locked -- -D warnings
cargo clippy --manifest-path backend/Cargo.toml --locked --features tantivy-search -- -D warnings
cargo test --manifest-path backend/Cargo.toml --locked --lib
cargo test --manifest-path backend/Cargo.toml --locked --features tantivy-search --lib
cargo test --manifest-path backend/Cargo.toml --locked --test smoke
```

- [x] **Step 2: Review the diff and commit**

Run `git diff --check`, verify only the planned files changed, then commit with:

```bash
git add backend/src/search backend/src/lib.rs backend/src/routes/uploads.rs backend/src/upload/job.rs backend/tests/search_publication.rs backend/tests/large_log_benchmark.rs docs/performance docs/superpowers/plans/2026-09-23-search-resource-budget.md
git commit -m "feat: add bounded Tantivy resource budget"
```

- [ ] **Step 3: Push and open a PR against `main`**

Push `feat/pr4-resource-budget`, create a PR describing the budget, permit-release guarantees, and benchmark fields, then wait for GitHub CI before claiming completion.
