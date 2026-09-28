# Issue #224: Temp-result preview Tantivy fast path Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reuse READY Tantivy Bundle indexes for safe temporary-preview keyword searches while preserving exact raw-line matching and fallback behavior.

**Architecture:** Classify only simple ASCII `Expression::Term` values as index-compatible. For eligible Bundle/File sources, query the existing Tantivy publication with its query permit, visibility snapshot, and generation lease to obtain candidate chunk line ranges. Materialize results by seeking near those ranges and verifying each raw line with the existing matcher; unsupported expressions and unavailable indexes use the existing full raw scan.

**Tech Stack:** Rust, Tokio, Actix Web, SQLx/SQLite, Tantivy, tracing, Cargo integration tests.

---

## File map

- Create `backend/src/routes/temp_results/search_plan.rs`: pure expression classification plus per-source Tantivy publication lookup, candidate search, sparse offset lookup, and structured plan statistics.
- Modify `backend/src/routes/temp_results.rs`: register the search-plan module and expose the planner/materializer types needed by the route service.
- Modify `backend/src/routes/temp_results/service.rs`: retain Bundle/File identity during source resolution, request a preview search plan, invoke planned materialization, and emit preview observability fields.
- Modify `backend/src/services/temp_results.rs`: add candidate line-range plans and a bounded raw verifier while keeping `materialize_preview` as the raw-scan compatibility wrapper.
- Modify `backend/tests/smoke.rs`: extend the existing preview scenario to cover a supported indexed keyword and an unsupported boolean fallback with identical response semantics.
- Modify `backend/tests/search_publication.rs`: add a publication fixture proving a READY generation supplies file-scoped preview candidates and preserves visibility filtering.
- Modify `backend/tests/large_log_benchmark.rs`: add an opt-in candidate-preview benchmark fixture that reports raw-scan versus candidate verification work without requiring a gigabyte test file by default.

### Task 1: Add the expression classifier and scan-plan types

**Files:**
- Create: `backend/src/routes/temp_results/search_plan.rs`
- Modify: `backend/src/routes/temp_results.rs`

- [ ] **Step 1: Write failing classifier tests.** Add a pure `classify_expression` function contract and tests in `search_plan.rs`:

```rust
#[test]
fn only_safe_ascii_terms_use_tantivy_candidates() {
    assert_eq!(classify_expression(&parse("ERROR").unwrap()), SearchPlanKind::IndexedTerm);
    assert_eq!(classify_expression(&parse(r#""ERROR smoke""#).unwrap()), SearchPlanKind::IndexedTerm);
    assert_eq!(classify_expression(&parse("ab").unwrap()), SearchPlanKind::RawFallback("term_too_short"));
    assert_eq!(classify_expression(&parse("ERROR AND timeout").unwrap()), SearchPlanKind::RawFallback("expression_not_a_term"));
    assert_eq!(classify_expression(&parse("错误标记").unwrap()), SearchPlanKind::RawFallback("term_not_ascii"));
}
```

The classifier must inspect the normalized `Expression::Term` value, require at least three characters, reject NUL bytes and leading/trailing whitespace, and return a stable fallback reason for every rejected category.

- [ ] **Step 2: Run the focused test to verify it fails for the missing API.**

Run: `cargo test --manifest-path backend/Cargo.toml routes::temp_results::search_plan::tests::only_safe_ascii_terms_use_tantivy_candidates -- --exact`

Expected: FAIL because `classify_expression` and `SearchPlanKind` do not exist yet.

- [ ] **Step 3: Implement the minimal classifier and shared plan types.** Define:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchPlanKind {
    IndexedTerm,
    RawFallback(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineRange {
    pub start: i64,
    pub end: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CandidateScanPlan {
    pub ranges: Vec<LineRange>,
    pub seek_line: i64,
    pub seek_offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceSearchPlan {
    Raw { reason: &'static str },
    Tantivy(CandidateScanPlan),
}
```

Register `mod search_plan;` in `backend/src/routes/temp_results.rs` and re-export the types needed by `service.rs` and `services::temp_results`.

- [ ] **Step 4: Run the focused tests and the existing log-expression tests.**

Run: `cargo test --manifest-path backend/Cargo.toml routes::temp_results::search_plan::tests log_expression::tests`

Expected: PASS with the new classifier cases and all existing expression behavior unchanged.

- [ ] **Step 5: Commit the classifier boundary.**

```bash
git add backend/src/routes/temp_results.rs backend/src/routes/temp_results/search_plan.rs
git commit -m "feat: classify temp preview search plans"
```

### Task 2: Add candidate-range raw verification to the executor

**Files:**
- Modify: `backend/src/services/temp_results.rs`

- [ ] **Step 1: Write failing candidate-materialization tests.** Add tests beside the existing executor tests for:

```rust
#[tokio::test]
async fn materializes_only_exact_matches_inside_candidate_ranges() {
    // Write lines 0..=20 with ERROR at lines 3 and 17, then provide ranges 0..=5 and 15..=20.
    // Assert total == 2, metadata line numbers are [3, 17], and both raw lines are retained.
}

#[tokio::test]
async fn empty_candidate_ranges_do_not_open_or_scan_the_source() {
    // Point at a missing source path with SourceSearchPlan::Tantivy and an empty range vector.
    // Assert the result is an empty artifact rather than an I/O error.
}

#[test]
fn candidate_ranges_are_sorted_and_merged_without_overlap() {
    // Assert LineRange values [5..=9, 0..=4, 8..=12] become one range [0..=12].
}
```

- [ ] **Step 2: Run the focused tests to verify the expected failures.**

Run: `cargo test --manifest-path backend/Cargo.toml services::temp_results::tests::materializes_only_exact_matches_inside_candidate_ranges services::temp_results::tests::empty_candidate_ranges_do_not_open_or_scan_the_source services::temp_results::tests::candidate_ranges_are_sorted_and_merged_without_overlap`

Expected: FAIL because the executor has no candidate-plan API.

- [ ] **Step 3: Implement planned materialization without changing the raw API.** Add:

```rust
pub async fn materialize_preview_with_plans(
    sources: &[TempSource],
    plans: &[SourceSearchPlan],
    expression: &Expression,
    from: i64,
    size: i64,
    max_output_bytes: u64,
    output: &mut File,
    metadata_output: &mut File,
    index_output: &mut File,
) -> Result<MaterializedPreview, AppError>
```

Keep `materialize_preview` as a wrapper that creates one `SourceSearchPlan::Raw { reason: "legacy_raw_scan" }` per source and delegates to the new function. In the planned loop, `Raw` preserves the current sequential scan. `Tantivy` seeks to `seek_offset`, initializes the source line counter to `seek_line`, advances through only the merged candidate ranges, and feeds each selected raw line to the existing matcher and artifact-writing code. Candidate ranges with no selected line return no matches without opening the source. Keep metadata-sidecar handling, truncation markers, output budgets, checkpoints, and pagination identical to the current path.

- [ ] **Step 4: Run the focused tests and the full executor test module.**

Run: `cargo test --manifest-path backend/Cargo.toml services::temp_results::tests`

Expected: PASS for all existing raw-scan tests plus the new candidate tests.

- [ ] **Step 5: Commit the verifier.**

```bash
git add backend/src/services/temp_results.rs
git commit -m "feat: verify temp preview candidates against raw lines"
```

### Task 3: Preserve source identity and build Tantivy candidate plans

**Files:**
- Modify: `backend/src/routes/temp_results/service.rs`
- Modify: `backend/src/routes/temp_results/search_plan.rs`

- [ ] **Step 1: Write failing source-plan integration tests.** Add tests that seed an in-memory schema and a published Tantivy fixture, then assert:

```rust
#[tokio::test]
async fn ready_tantivy_publication_returns_file_scoped_candidate_ranges() {
    // Seed bundle/file identity, publication generation, one indexed chunk covering lines 10..=12,
    // and a source file. Build the plan for Expression::Term("marker"), then assert it is Tantivy,
    // contains exactly that range, and uses a sparse offset at or before line 10.
}

#[tokio::test]
async fn unsupported_expression_and_missing_index_use_raw_fallback() {
    // Assert boolean expressions and a source without a READY-compatible publication both produce Raw plans
    // with stable reasons and never return a Tantivy error.
}
```

- [ ] **Step 2: Run the focused tests to verify they fail.**

Run: `cargo test --manifest-path backend/Cargo.toml routes::temp_results::search_plan::tests::ready_tantivy_publication_returns_file_scoped_candidate_ranges routes::temp_results::search_plan::tests::unsupported_expression_and_missing_index_use_raw_fallback -- --exact`

Expected: FAIL because resolved sources do not retain Bundle/File identity and no planner performs publication lookup.

- [ ] **Step 3: Extend `ResolvedSources` with optional index identity.** Add an internal identity record:

```rust
pub(crate) struct IndexedSource {
    pub bundle_id: String,
    pub file_id: i64,
}
```

Store `Vec<Option<IndexedSource>>` alongside `sources`. Set it to `None` for `source_temp_id`; set it to `Some` for direct Bundle/File resolution; and include `b.id` in the Issue source query so each Issue file also receives its internal Bundle ID. Keep the existing source ordering and metadata fields unchanged.

- [ ] **Step 4: Implement the planner's publication and candidate query.** Add `build_source_search_plans` that:

1. returns raw plans immediately when `classify_expression` rejects the expression or the source has no identity;
2. loads `backend`, `state`, `generation`, `schema_version`, `tokenizer_version`, `visibility_revision`, and `compacted_revision` from `bundle_search_indexes`;
3. accepts only Tantivy `READY`/`NEEDS_REBUILD` rows with current schema/tokenizer versions and a positive generation;
4. builds a file-scoped `ContentSearchRequest` using the normalized term and `HARD_MAX_SEARCH_WINDOW` as the candidate size;
5. acquires `state.search.query_permits`, a generation lease, and either a visibility snapshot or the complete-visibility path using the existing publication helpers;
6. calls `search_tantivy_bundle_*_with_lease_and_permit`; and
7. falls back with a reason if the publication is absent/incompatible, Tantivy is unavailable, or `result.total > HARD_MAX_SEARCH_WINDOW`.

Convert each returned row's `offset`/`line_end` into an inclusive `LineRange`, discard malformed ranges, sort and merge them, and query `nearest_line_offset` for the first range. If all rows are malformed, use a raw fallback rather than silently returning an empty indexed result.

- [ ] **Step 5: Run the focused planner tests and existing publication tests.**

Run: `cargo test --manifest-path backend/Cargo.toml routes::temp_results::search_plan::tests search::publication::tests --features tantivy-search`

Expected: PASS, including lease-safe visibility behavior and raw fallback cases.

- [ ] **Step 6: Commit source identity and candidate planning.**

```bash
git add backend/src/routes/temp_results/service.rs backend/src/routes/temp_results/search_plan.rs
git commit -m "feat: plan Tantivy candidates for temp previews"
```

### Task 4: Wire planned materialization and observability into the preview route

**Files:**
- Modify: `backend/src/routes/temp_results/service.rs`
- Modify: `backend/src/routes/temp_results.rs`

- [ ] **Step 1: Write a failing route-level behavior test.** Extend the existing preview test fixture so a supported keyword is served through a READY Tantivy publication and a boolean expression still returns the same exact raw result. Add a test assertion around the planner stats helper that the indexed case reports `tantivy`, while the boolean case reports `raw_scan`.

- [ ] **Step 2: Run the focused route/smoke test to verify it fails.**

Run: `cargo test --manifest-path backend/Cargo.toml --test smoke upload_search_tree_and_delete_issue -- --exact`

Expected: The current response remains correct but the new backend-stat assertion fails because preview still always invokes `TempResultExecutor::write_matches` through the raw path.

- [ ] **Step 3: Integrate the plan into `materialize_result_with_timeout`.** Pass the vector of `SourceSearchPlan` values to `materialize_preview_with_plans` for previews and to the raw wrapper for full-result creation. Keep the existing staging/publish/cleanup transaction unchanged. The timeout remains around the materialization operation, so raw fallback continues to honor `temp_results_max_scan_duration_seconds`.

- [ ] **Step 4: Emit one structured preview event.** Record the planner's elapsed milliseconds, candidate count, verified match count, and backend classification after materialization:

```rust
tracing::info!(
    metric = "temp_result_preview",
    search_backend = plan.backend_label(),
    candidate_count = plan.candidate_count(),
    verified_match_count = total,
    query_elapsed_ms = plan.query_elapsed_ms(),
    fallback_reason = ?plan.fallback_reason(),
    "completed temporary result preview"
);
```

Use `raw_scan`, `tantivy`, and `mixed` labels exactly, and do not add fields to the public JSON response.

- [ ] **Step 5: Run route, smoke, and executor tests.**

Run: `cargo test --manifest-path backend/Cargo.toml routes::temp_results::service::tests services::temp_results::tests --features tantivy-search` and `cargo test --manifest-path backend/Cargo.toml --test smoke upload_search_tree_and_delete_issue -- --exact`

Expected: PASS with identical preview response lines, result IDs, metadata, pagination, deletion, and boolean fallback behavior.

- [ ] **Step 6: Commit route integration.**

```bash
git add backend/src/routes/temp_results.rs backend/src/routes/temp_results/service.rs
git commit -m "feat: use Tantivy candidates in temp preview"
```

### Task 5: Add publication parity and opt-in large-log coverage

**Files:**
- Modify: `backend/tests/search_publication.rs`
- Modify: `backend/tests/large_log_benchmark.rs`
- Modify: `backend/tests/smoke.rs`

- [ ] **Step 1: Add a publication-level preview candidate test.** Reuse the existing publication fixture helpers to create two files in one Bundle, index a marker only in one file, mark the other file invisible, and assert the planner returns only visible candidate ranges for the requested file. Use the existing `snapshot_file_ids` path for `NEEDS_REBUILD` and the complete-visibility path for compacted `READY`.

- [ ] **Step 2: Add an opt-in benchmark measurement.** In `large_log_benchmark.rs`, generate a deterministic fixture large enough to contain multiple index chunks, build the Tantivy generation, run both the raw executor and candidate executor with the same term, and print:

```text
temp_preview_backend=raw_scan raw_lines_read={raw_lines_read} verified_matches={raw_matches} elapsed_ms={raw_elapsed_ms}
temp_preview_backend=tantivy candidates={candidate_count} verified_matches={candidate_matches} elapsed_ms={candidate_elapsed_ms}
```

Keep the benchmark ignored by default and avoid making the regular test suite depend on a 1 GB file.

- [ ] **Step 3: Run the focused integration and benchmark fixture tests.**

Run: `cargo test --manifest-path backend/Cargo.toml --test search_publication --features tantivy-search` and `cargo test --manifest-path backend/Cargo.toml --test large_log_benchmark fixture -- --nocapture`

Expected: PASS; the opt-in benchmark fixture reports fewer verified raw lines for the candidate path while returning the same match set.

- [ ] **Step 4: Commit coverage.**

```bash
git add backend/tests/search_publication.rs backend/tests/large_log_benchmark.rs backend/tests/smoke.rs
git commit -m "test: cover Tantivy-backed temp preview"
```

### Task 6: Final verification and PR preparation

**Files:**
- No new production files; inspect all commits and the final diff.

- [ ] **Step 1: Format and check the complete backend.**

Run: `cargo fmt --manifest-path backend/Cargo.toml --check` and `cargo clippy --manifest-path backend/Cargo.toml --all-targets --all-features -- -D warnings`

Expected: both commands exit 0 with no formatting changes, warnings, or errors.

- [ ] **Step 2: Run the complete frontend build and backend suite.**

Run: `npm run build` from `frontend/`, then `cargo test --manifest-path backend/Cargo.toml --all-features`

Expected: frontend build exits 0; backend reports 0 failures, with only the repository's existing ignored benchmarks.

- [ ] **Step 3: Run diff and requirement checks.**

Run: `git diff --check`, `git status --short`, and `git diff --stat origin/main HEAD`

Expected: no whitespace errors, only Issue #224 implementation/spec/plan files are changed, and no pre-existing main-worktree files are included.

- [ ] **Step 4: Request a code review before pushing.** Use the requesting-code-review workflow with base `3d1d061`, the final branch HEAD, the Issue #224 acceptance criteria, and the focused test evidence. Fix all Critical/Important findings and rerun the affected tests.

- [ ] **Step 5: Push and create the PR.**

```bash
git push -u origin fix/issue-224-temp-preview-search
gh pr create --base main --head fix/issue-224-temp-preview-search \
  --title "perf: reuse Tantivy index for temp-result preview" \
  --body-file /tmp/issue-224-pr-body.md
```

The PR body must link `Closes #224`, summarize the candidate/verification/fallback architecture, list observability fields, and include the exact frontend build, clippy, and backend test commands run.

## Plan self-review

- **Spec coverage:** safe expression classification is Task 1; exact candidate verification is Task 2; publication/visibility/lease reuse is Task 3; route observability and fallback are Task 4; large-file and parity coverage are Task 5; final verification and PR creation are Task 6.
- **Placeholder scan:** every step contains concrete paths, APIs, commands, and expected results; no unresolved placeholder instruction remains.
- **Type consistency:** `SearchPlanKind`, `LineRange`, `CandidateScanPlan`, and `SourceSearchPlan` are introduced in Task 1; Task 2 consumes `SourceSearchPlan`; Task 3 produces it; Task 4 passes it into materialization and consumes its statistics.
- **Scope check:** the plan keeps the first fast path limited to safe ASCII terms and leaves full boolean planning for a later issue, matching the approved design.
