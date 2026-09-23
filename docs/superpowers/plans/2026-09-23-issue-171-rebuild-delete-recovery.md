# Tantivy Rebuild Delete Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task with verification checkpoints.

**Goal:** Make Tantivy rebuild cleanup recover deleted Bundles after crashes or restarts without leaving pending metadata, staging directories, or unpublished final generations, while preserving leased active/retired readers.

**Architecture:** Treat each Bundle's rebuild and artifact cleanup as one in-process lifecycle critical section, with a durable owner token for cross-process claims. Each worker builds in an owner-specific staging directory and refreshes a heartbeat; publication first advances the claim to `PUBLISHING`, which prevents stale takeover while the shared final generation is replaced. Cleanup claims rows as `CLEANING`, removes both `search/<bundle>/<generation>` and all matching `.search-rebuild` staging locations idempotently, and only then clears pending metadata. Deleted-Bundle cleanup enumerates current, retired, and pending generations and leaves leased active/retired artifacts for a later pass. Successful startup cleanup also removes orphaned owner staging left by a crashed worker.

**Tech Stack:** Rust, Tokio, SQLx SQLite, Tantivy, existing `bundle_search_indexes`/`bundle_search_artifacts` metadata and async filesystem APIs.

---

### Task 1: Add failing recovery and lifecycle tests

**Files:**
- Modify: `backend/src/search/publication.rs` test module
- Modify: `backend/src/search/rebuild.rs` tests if needed for claim revalidation

- [x] **Step 1: Add a test for deleted Bundle pending BUILDING cleanup.** Seed a DELETED Bundle with a READY current generation, a `pending_generation` in BUILDING, active/retired artifact rows, and both staging/final pending directories. Assert cleanup removes pending paths, clears pending fields, removes unleased artifacts, and resets the index generation metadata.
- [x] **Step 2: Add a test for crash-after-rename recovery and staging cleanup.** Seed only a pending final generation and a staging directory, call cleanup, then assert both paths are absent and a second cleanup reports no filesystem removal.
- [x] **Step 3: Add a reader-safety test.** Seed a deleted Bundle with an active/retired artifact carrying a reader lease plus an unleased pending generation. Assert pending is removed immediately while the leased artifact remains; release the lease and assert a later cleanup removes the old artifact.
- [x] **Step 4: Run the focused tests and verify they fail for the missing behavior.**

Run: `cargo test --features tantivy-search search::publication::tests::deleted_bundle -- --nocapture`

Expected: failures show pending metadata/directories are not reclaimed by the current implementation.

### Task 2: Add idempotent dual-location artifact cleanup and Bundle lifecycle locking

**Files:**
- Modify: `backend/src/search/publication.rs`
- Modify: `backend/src/search/rebuild.rs`

- [x] **Step 1: Implement a per-Bundle async lifecycle lock shared by rebuild and cleanup.** Store weak references in a process-local map so idle Bundle locks do not grow without bound; expose an internal guard helper for both modules.
- [x] **Step 2: Replace single-location cleanup with a helper that removes the final generation and matching `.search-rebuild` staging directory, treating missing paths as success and cleaning empty parent directories opportunistically.**
- [x] **Step 3: Update unpublished cleanup to exclude deleted Bundles, claim `CLEANING` rows under the lifecycle lock, remove both locations, and clear pending fields only after filesystem success.
- [x] **Step 4: Update deleted-Bundle cleanup to enumerate pending generations as well as current and artifact-table generations, coordinate through the lifecycle lock, remove pending generations without reader leases, and only remove active/retired artifacts when `active_readers=0`. Reset current index metadata after all unleased deleted artifacts are handled.
- [x] **Step 5: Re-run the focused publication tests until green.**

Run: `cargo test --features tantivy-search search::publication::tests -- --nocapture`

### Task 3: Make rebuild cancellation and publication race-safe

**Files:**
- Modify: `backend/src/search/rebuild.rs`
- Modify: `backend/src/search/publication.rs`

- [x] **Step 1: Add an internal claim-validity query requiring the Bundle and Issue to remain READY/ACTIVE and the exact pending BUILDING generation to remain owned by the claim.
- [x] **Step 2: Acquire the Bundle lifecycle lock after claiming and revalidate before snapshot/build; return without publishing if cleanup already reclaimed the claim.
- [x] **Step 3: Revalidate before entering `PUBLISHING` and before database publication; stale workers remove only their owner-specific staging while shared final paths are removed by durable `CLEANING` recovery.
- [x] **Step 4: Keep the active-generation reader lease while reading the source and building, then release it on every success/error path; do not create a lease for pending generations.
- [x] **Step 5: Add regression tests for stale/deleted claims, owner heartbeats, and orphan staging recovery, then run rebuild/publication tests.

### Task 4: Full verification and handoff

**Files:**
- No additional source files unless verification exposes a required fix.

- [x] **Step 1: Run `cargo fmt --check` and `cargo clippy --features tantivy-search --lib -- -D warnings`.
- [x] **Step 2: Run the complete backend suite with `cargo test --features tantivy-search`.
- [x] **Step 3: Run `git diff --check` and inspect the diff for unrelated changes.
- [x] **Step 4: Commit the implementation on `fix/issue-171-rebuild-delete-recovery` with a focused message.**
