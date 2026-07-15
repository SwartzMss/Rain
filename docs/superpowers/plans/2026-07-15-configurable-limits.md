# Configurable Limits Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace hard-coded upload, archive, indexing, preview, and pagination limits with validated typed `.env` configuration while preserving existing defaults and fixing misleading gzip budget errors.

**Architecture:** `backend/src/config.rs` owns immutable nested limit types, parsing, defaults, and validation. `AppState` carries those limits and a configured processing semaphore; routes consume state directly while ingestion receives explicit archive/indexing values so tests can inject small budgets. Process environment values retain precedence over the executable-adjacent or working-directory `.env` file.

**Tech Stack:** Rust 2024, Actix Web, Tokio, serde/config support already present, dotenvy, SQLx, flate2/zip/tar.

---

### Task 1: Typed limit configuration and size parsing

**Files:**
- Modify: `backend/src/config.rs`
- Modify: `backend/src/error.rs`

- [ ] Add failing unit tests for `parse_byte_size` covering `64 KiB`, `4 GiB`, `20 GiB`, plain bytes, whitespace/case handling, unknown units, overflow, and zero.
- [ ] Run `cargo test config::tests --lib` from `backend`; expect the new tests to fail because the parser and limit types do not exist.
- [ ] Add `UploadConfig`, `ArchiveConfig`, `IndexingConfig`, and `ApiConfig`, with `Default` implementations matching every current module constant. Implement checked binary-size parsing into `u64`, plus helpers that parse named environment values with actionable `AppError::Config` messages.
- [ ] Add failing tests for defaults, one representative environment override per group, zero rejection, `max_entry_size <= max_extracted_size`, `max_file_size <= max_total_size`, and both default-page/max-page relationships. Serialize environment-mutating tests with a test mutex and restore prior values.
- [ ] Extend `AppConfig::from_env` to populate and validate all groups, then rerun `cargo test config::tests --lib`; expect all configuration tests to pass.
- [ ] Commit `backend/src/config.rs` and any error support with message `Add typed limit configuration`.

### Task 2: Put immutable limits and concurrency in application state

**Files:**
- Modify: `backend/src/lib.rs`
- Modify: `backend/src/main.rs`
- Modify: `backend/tests/smoke.rs`

- [ ] Add a failing state-construction test or compile the smoke tests after changing a fixture to expect `AppState::new(pool, data_root, limits)` and `processing_permits.available_permits()` to equal the configured upload concurrency.
- [ ] Run `cargo test --test smoke --no-run`; expect a compile failure until the new state constructor exists.
- [ ] Add a cloneable aggregate limits value to `AppState`, store `Arc<Semaphore>`, and implement a constructor that builds the semaphore from `upload.concurrent_processing_tasks`.
- [ ] Update `main.rs` to construct state from validated config and log every effective nested limit once after tracing initialization. Update all smoke-test state fixtures to use default or deliberately supplied limits.
- [ ] Run `cargo test --test smoke --no-run`; expect successful compilation.
- [ ] Commit with message `Share configured limits through app state`.

### Task 3: Configurable upload enforcement

**Files:**
- Modify: `backend/src/routes/uploads.rs`
- Test: `backend/src/routes/uploads.rs`

- [ ] Add route/helper tests using tiny `UploadConfig` values to prove per-file bytes, total bytes, file count, text-field bytes, and concurrent processing permits use injected state rather than constants.
- [ ] Run `cargo test routes::uploads::tests --lib`; expect failures against hard-coded constants/global semaphore.
- [ ] Remove the five upload constants and global `Lazy<Semaphore>`. Read checked `u64` limits from `state.limits.upload`, pass them into collectors, and acquire the permit from `state.processing_permits.clone().acquire_owned()` for spawned processing.
- [ ] Ensure size errors use a binary-size formatter that represents bytes/KiB/MiB/GiB without reporting small nonzero values as zero.
- [ ] Rerun `cargo test routes::uploads::tests --lib`; expect all upload tests to pass.
- [ ] Commit with message `Apply configured upload limits`.

### Task 4: Configurable archive and indexing enforcement

**Files:**
- Modify: `backend/src/ingest.rs`
- Modify: `backend/src/routes/uploads.rs`
- Modify: `backend/src/routes/files.rs`

- [ ] Add failing ingestion tests with `ArchiveConfig { max_extracted_size: 8, max_entry_size: 6, .. }` that separately exceed one entry and exhaust the shared bundle, asserting messages contain `max entry size` and `max bundle size` respectively. Add small indexing-config tests for line truncation/chunk/commit/offset behavior.
- [ ] Run `cargo test ingest::tests --lib`; expect the tests to fail because ingestion still reads constants.
- [ ] Make `ArchiveBudget` own an `ArchiveConfig`; add archive and indexing references to `ProcessFileOptions`; thread them through recursive archive and text-ingestion functions. Replace every archive/indexing constant with the corresponding typed value, using checked `u64` to `usize` conversion only at buffer or collection boundaries.
- [ ] In gzip copying, check whether the bundle remaining budget or entry ceiling selected the copy limit and return the matching configured error when the extra-byte probe proves overflow.
- [ ] Update uploads to create the configured budget and pass both config groups. Update file-line reading to use `indexing.max_line_size` from state.
- [ ] Rerun `cargo test ingest::tests --lib` and `cargo test routes::uploads::tests --lib`; expect all tests to pass.
- [ ] Commit with message `Apply configured archive and indexing limits`.

### Task 5: Configurable API pagination and previews

**Files:**
- Modify: `backend/src/routes/files.rs`
- Modify: `backend/src/routes/logs.rs`
- Modify: `backend/src/routes/temp_results.rs`
- Test: `backend/tests/smoke.rs`

- [ ] Add smoke tests with a small `ApiConfig` proving preview truncation, default/max file-line pagination, default/max temporary-result pagination, and default/max search results.
- [ ] Run the named smoke tests; expect failures while handlers use constants and numeric literals.
- [ ] Replace file preview size, line-page defaults/maxima, and all three log-search default/max pairs with `state.limits.api` values. Apply the same line-page values to temporary-result lines.
- [ ] Rerun `cargo test --test smoke`; expect all smoke tests to pass.
- [ ] Commit with message `Apply configured API limits`.

### Task 6: Environment example and README

**Files:**
- Modify: `backend/.env.example`
- Modify: `README.md`

- [ ] Add every `RAIN_UPLOAD_*`, `RAIN_ARCHIVE_*`, `RAIN_INDEXING_*`, and `RAIN_API_*` variable to `.env.example` using the preserved defaults and human-readable binary sizes.
- [ ] Put a concise Chinese comment immediately above every `.env.example` setting. State its purpose and, where relevant, accepted binary units, whether zero disables it, or the default/max and entry/bundle relationship.
- [ ] Document `.env` discovery, process-environment precedence, accepted byte syntax, validation relationships, and a table of every configurable limit/default in README.
- [ ] Update existing fixed-limit prose to point to defaults rather than imply values are immutable.
- [ ] Run `rg -n "512 MB|2 GB|500 MB|100 MB|10000|3000|1 MB|64 KB" README.md backend/.env.example` and inspect every match for consistency.
- [ ] Commit with message `Document configurable limits`.

### Task 7: Full verification and publication

**Files:**
- Review all changed files.

- [ ] Run `cargo fmt --check`, `cargo check`, and `cargo test` in `backend`; expect zero failures.
- [ ] Run `npm run build` in `frontend`; expect a successful Vite production build.
- [ ] Run `git diff --check`, inspect `git status -sb`, and review `git diff origin/main...HEAD` for unrelated changes, stale constants, unsafe casts, and missing acceptance criteria.
- [ ] Use the verification-before-completion and requesting-code-review skills, address any concrete findings, and rerun affected checks.
- [ ] Push `agent/configurable-limits` to `origin` and create a draft PR targeting `main` with `Fixes #15`, root cause, behavior changes, compatibility, and exact validation commands.
