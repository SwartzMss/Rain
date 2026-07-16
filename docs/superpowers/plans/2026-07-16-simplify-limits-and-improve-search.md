# Simplify Limits and Improve Search Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace overlapping upload/archive limits with one atomic 4 GiB Issue content quota, remove unused structured events, and provide UTF-8 literal substring search through SQLite FTS5 trigram indexing.

**Architecture:** Keep SQLite as the source of truth. Store Bundle content reservations in `bundles.content_size_bytes`, update them atomically while final files are admitted, and derive Issue usage from READY and PROCESSING Bundles. Keep archive safety and indexing tuning as internal constants, remove `log_events`, then migrate the derived FTS table to trigram with a bounded short-query fallback.

**Tech Stack:** Rust, Tokio, Actix Web, SQLx, SQLite/FTS5, tracing, existing smoke-test harness.

---

## File Structure

- `backend/src/config.rs`: only user-configurable Issue capacity, processing concurrency, line limit, and API settings.
- `backend/src/ingest/limits.rs`: fixed multipart, archive-safety, and indexing-performance constants.
- `backend/src/ingest/quota.rs`: database-backed Bundle reservation API and quota errors.
- `backend/src/ingest/archive/budget.rs`: archive counters and structural safeguards; delegates final-byte accounting to quota reservations.
- `backend/src/ingest.rs`: orchestrates classification, extraction, final-file admission, and text indexing without structured events.
- `backend/src/ingest/indexing/`: line reading only; remove event parsing.
- `backend/src/upload/multipart.rs`: streamed multipart collection using fixed transport guards.
- `backend/src/upload/job.rs`: creates one quota reservation context per Bundle.
- `backend/src/upload/finalizer.rs`: releases reservations on failure through Bundle reset and existing batched cleanup.
- `backend/src/db.rs`: schema evolution/backfill, event-schema removal, trigram FTS rebuild, cleanup, and diagnostics.
- `backend/src/routes/logs.rs`: literal trigram queries and bounded short-query fallback.
- `backend/src/routes/issues.rs`: deletion behavior remains the quota-release path through Bundle deletion.
- `backend/tests/smoke.rs`: cross-component quota, rollback, cleanup, migration, and search regression tests.
- `backend/.env.example`, `doc/DB.md`: deployment and schema documentation.

### Task 1: Collapse configuration and introduce internal limits

**Files:**
- Create: `backend/src/ingest/limits.rs`
- Modify: `backend/src/ingest.rs`
- Modify: `backend/src/config.rs`
- Modify: `backend/.env.example`
- Test: `backend/src/config.rs`

- [ ] **Step 1: Add failing configuration tests**

Add tests asserting the defaults and environment overrides:

```rust
assert_eq!(limits.issue_max_content_size, 4 * 1024_u64.pow(3));
assert_eq!(limits.upload.concurrent_processing_tasks, 4);
assert_eq!(limits.indexing.max_line_size, 8 * 1024_u64.pow(2));
```

Set `RAIN_ISSUE_MAX_CONTENT_SIZE=6 GiB` under the existing environment-test lock and assert the parsed value. Remove assertions and override tests for deleted fields.

- [ ] **Step 2: Run the focused tests and confirm failure**

Run: `cargo test --locked config::tests -- --nocapture`

Expected: compilation fails because `issue_max_content_size` does not exist and old config structures still require removed fields.

- [ ] **Step 3: Implement the minimal public configuration**

Make the relevant shape:

```rust
pub struct UploadConfig {
    pub concurrent_processing_tasks: usize,
}

pub struct IndexingConfig {
    pub max_line_size: u64,
}

pub struct AppLimits {
    pub issue_max_content_size: u64,
    pub upload: UploadConfig,
    pub indexing: IndexingConfig,
    pub api: ApiConfig,
}
```

Parse and validate `RAIN_ISSUE_MAX_CONTENT_SIZE`; retain only the three approved workflow variables. Create `ingest/limits.rs` with the prior safe defaults as internal constants:

```rust
pub const MAX_UPLOAD_FILES: usize = 100;
pub const MAX_MULTIPART_TEXT_FIELD_SIZE: u64 = 64 * 1024;
pub const MAX_ARCHIVE_ENTRIES: usize = 10_000;
pub const MAX_ARCHIVE_PATH_DEPTH: usize = 16;
pub const MAX_ARCHIVE_RECURSION_DEPTH: usize = 16;
pub const MAX_ARCHIVE_OUTPUT_PATH_CHARS: usize = 1024;
pub const MAX_ARCHIVE_COMPRESSION_RATIO: u64 = 100;
pub const INDEX_CHUNK_LINES: usize = 200;
pub const INDEX_COMMIT_LINES: i64 = 5_000;
pub const LINE_OFFSET_INTERVAL: i64 = 1_000;
```

- [ ] **Step 4: Update `.env.example` and run focused tests**

Leave exactly the approved three workflow settings with Chinese comments. Run: `cargo test --locked config::tests -- --nocapture`

Expected: all config tests pass.

- [ ] **Step 5: Commit**

```bash
git add backend/src/config.rs backend/src/ingest.rs backend/src/ingest/limits.rs backend/.env.example
git commit -m "refactor: simplify upload and indexing limits"
```

### Task 2: Add Bundle content accounting and backfill

**Files:**
- Modify: `backend/src/db.rs`
- Modify: `doc/DB.md`
- Test: `backend/src/db.rs`
- Test: `backend/tests/smoke.rs`

- [ ] **Step 1: Add a failing schema/backfill smoke test**

Create a legacy-like READY Bundle containing a direct text file, an archive record with `meta.preview_kind = "archive"`, an extracted directory, and an extracted leaf. Re-run database initialization and assert:

```rust
let content_size: i64 = sqlx::query_scalar(
    "SELECT content_size_bytes FROM bundles WHERE id = ?",
)
.bind(bundle_id)
.fetch_one(&pool)
.await?;
assert_eq!(content_size, direct_size + extracted_leaf_size);
```

The archive record and directory must not contribute.

- [ ] **Step 2: Run the test and confirm failure**

Run: `cargo test --locked --test smoke bundle_content_size_backfill -- --nocapture`

Expected: FAIL because `bundles.content_size_bytes` is absent.

- [ ] **Step 3: Add schema evolution and idempotent backfill**

Add a non-negative column through the existing column-migration helper:

```sql
ALTER TABLE bundles ADD COLUMN content_size_bytes INTEGER NOT NULL DEFAULT 0;
```

Follow the existing `pragma_table_info` pattern. Only when the column is absent, add it and immediately backfill READY Bundles by summing non-directory files whose `meta.preview_kind` is not `archive`. Use `COALESCE(size_bytes, 0)` and guard against negative values; keep FAILED Bundle usage zero. Because backfill runs in the same one-time branch that adds the column, legitimately empty Bundles are not repeatedly scanned at startup.

- [ ] **Step 4: Run DB and smoke tests**

Run: `cargo test --locked db::tests -- --nocapture`

Run: `cargo test --locked --test smoke bundle_content_size_backfill -- --nocapture`

Expected: PASS and a second initialization leaves the same value.

- [ ] **Step 5: Commit**

```bash
git add backend/src/db.rs backend/tests/smoke.rs doc/DB.md
git commit -m "feat: track bundle content size"
```

### Task 3: Implement atomic Issue quota reservations

**Files:**
- Create: `backend/src/ingest/quota.rs`
- Modify: `backend/src/ingest.rs`
- Test: `backend/tests/smoke.rs`

- [ ] **Step 1: Add failing exact-limit, overflow, and concurrency tests**

Exercise a public reservation function with two PROCESSING Bundles under one Issue. Assert exact-limit success, one-byte overflow, and that two concurrent reservations cannot both claim the same remaining bytes:

```rust
let outcomes = tokio::join!(
    reserve_bundle_content(&pool, issue, bundle_a, 60, 100),
    reserve_bundle_content(&pool, issue, bundle_b, 60, 100),
);
assert_eq!([outcomes.0.is_ok(), outcomes.1.is_ok()].into_iter().filter(|v| *v).count(), 1);
```

- [ ] **Step 2: Run the test and confirm failure**

Run: `cargo test --locked --test smoke issue_quota -- --nocapture`

Expected: compilation fails because the reservation API is absent.

- [ ] **Step 3: Implement a transactionally serialized reservation**

Create:

```rust
pub struct IssueQuota {
    pool: SqlitePool,
    issue_code: String,
    bundle_id: String,
    limit: u64,
}

impl IssueQuota {
    pub async fn reserve(&self, bytes: u64) -> Result<(), AppError>;
}
```

Within one SQLite write transaction, atomically increment only when this predicate holds:

```sql
UPDATE bundles
SET content_size_bytes = content_size_bytes + ?
WHERE id = ?
  AND issue_code = ?
  AND status = 'PROCESSING'
  AND ? >= (
    SELECT COALESCE(SUM(content_size_bytes), 0) + ?
    FROM bundles
    WHERE issue_code = ? AND status IN ('READY', 'PROCESSING')
  );
```

If no row changes, query current usage and return a Chinese `BadRequest` containing limit, usage, and observed addition. Convert sizes with checked `i64` conversions.

- [ ] **Step 4: Run quota tests**

Run: `cargo test --locked --test smoke issue_quota -- --nocapture`

Expected: exact limit passes, one-byte overflow fails, and concurrency never exceeds the limit.

- [ ] **Step 5: Commit**

```bash
git add backend/src/ingest.rs backend/src/ingest/quota.rs backend/tests/smoke.rs
git commit -m "feat: enforce atomic issue content quota"
```

### Task 4: Apply quota to uploads and recursive archives

**Files:**
- Modify: `backend/src/upload/multipart.rs`
- Modify: `backend/src/upload/job.rs`
- Modify: `backend/src/upload/finalizer.rs`
- Modify: `backend/src/ingest.rs`
- Modify: `backend/src/ingest/archive/budget.rs`
- Modify: `backend/src/ingest/archive/gzip.rs`
- Modify: `backend/src/ingest/archive/tar_gz.rs`
- Modify: `backend/src/ingest/archive/zip.rs`
- Modify: `backend/src/ingest/archive/path_policy.rs`
- Test: `backend/src/ingest.rs`
- Test: `backend/tests/smoke.rs`

- [ ] **Step 1: Add failing accounting tests for each input shape**

Add fixtures for a direct file, ZIP, tar.gz, gzip, and nested ZIP containing gzip. Assert final Bundle sizes equal only terminal non-archive descendants. Add overflow tests asserting the Bundle becomes FAILED, `content_size_bytes = 0`, dependent rows are removed, and the previous READY Bundle remains intact.

- [ ] **Step 2: Run focused tests and confirm failure**

Run: `cargo test --locked --test smoke upload_issue_quota -- --nocapture`

Expected: old upload/archive limits reject fixtures or Bundle content accounting remains zero/incorrect.

- [ ] **Step 3: Replace multipart configuration with fixed transport guards**

Change multipart collection to use `MAX_UPLOAD_FILES` and `MAX_MULTIPART_TEXT_FIELD_SIZE`. Stream file fields to disk without a second configurable content ceiling. Preserve checked byte counters for diagnostics and reject only counter overflow or fixed pathological transport violations.

- [ ] **Step 4: Thread one `IssueQuota` through the upload job**

Construct it in `process_upload_job` from `job.issue_code`, `job.bundle_id`, and `job.issue_max_content_size`. Pass clones through `ProcessFileOptions` and recursive ingestion.

Before admitting a direct non-archive file, call `quota.reserve(size_bytes)`. Do not reserve uploaded archive records. During extracted-tree traversal, classify first; reserve only non-directory, non-archive terminal files before inserting/indexing them.

- [ ] **Step 5: Retain structural archive protection with internal constants**

Refactor `ArchiveBudget` to count entries only and expose fixed safety values from `ingest::limits`. ZIP and tar.gz validate declared sizes and compression ratios; gzip enforces streaming ratio and quota reservation without a separate extracted-byte cap. Preserve path traversal, collision, path-depth, recursive-depth, and path-length failures.

- [ ] **Step 6: Reset quota on failure**

In `finalize_bundle_failed`, run batched content cleanup and then set:

```sql
UPDATE bundles
SET status = 'FAILED', process_stage = 'FAILED', failure_reason = ?, content_size_bytes = 0
WHERE id = ?;
```

Do not reset READY Bundle sizes during normal deletion; deleting their rows releases usage naturally.

- [ ] **Step 7: Run focused archive and smoke tests**

Run: `cargo test --locked ingest::tests -- --nocapture`

Run: `cargo test --locked --test smoke upload_issue_quota -- --nocapture`

Expected: all input shapes account correctly; overflow rolls back atomically from the user's perspective.

- [ ] **Step 8: Commit**

```bash
git add backend/src/upload backend/src/ingest.rs backend/src/ingest/archive backend/tests/smoke.rs
git commit -m "feat: apply issue quota to uploads and archives"
```

### Task 5: Remove structured-event indexing and storage

**Files:**
- Delete: `backend/src/ingest/indexing/event_parser.rs`
- Modify: `backend/src/ingest/indexing/mod.rs`
- Modify: `backend/src/ingest.rs`
- Modify: `backend/src/upload/job.rs`
- Modify: `backend/src/db.rs`
- Modify: `backend/src/repositories/files.rs`
- Modify: `doc/DB.md`
- Test: `backend/src/ingest.rs`
- Test: `backend/tests/smoke.rs`

- [ ] **Step 1: Add a failing no-events schema/ingestion test**

Initialize a database and assert `sqlite_master` has no `log_events`, `idx_events_bundle_level`, or `idx_events_file_line`. Upload parseable ERROR/WARN lines and assert segments and FTS rows exist without any structured-event pipeline dependency.

- [ ] **Step 2: Run the test and confirm failure**

Run: `cargo test --locked --test smoke structured_events_removed -- --nocapture`

Expected: FAIL because `log_events` and its indexes still exist.

- [ ] **Step 3: Remove event code and schema**

Delete `EventBudget`, `EventReservation`, parser calls, event inserts, module export, upload-job construction, file-delete event SQL, Bundle cleanup event phase, table creation, and event indexes. During initialization execute idempotent cleanup:

```sql
DROP TABLE IF EXISTS log_events;
```

Remove event fields from `BundleCleanupStats` and update total-row calculations and tests.

- [ ] **Step 4: Run focused tests**

Run: `cargo test --locked ingest::tests -- --nocapture`

Run: `cargo test --locked --test smoke structured_events_removed -- --nocapture`

Expected: segments, FTS, offsets, and paging pass with no event schema.

- [ ] **Step 5: Commit**

```bash
git add backend/src/ingest.rs backend/src/ingest/indexing backend/src/upload/job.rs backend/src/db.rs backend/src/repositories/files.rs backend/tests/smoke.rs doc/DB.md
git commit -m "refactor: remove unused structured event index"
```

### Task 6: Add trigram FTS schema migration and rebuild

**Files:**
- Modify: `backend/src/db.rs`
- Test: `backend/src/db.rs`
- Test: `backend/tests/smoke.rs`

- [ ] **Step 1: Add failing migration tests**

Create the legacy `log_segments_fts` with the default tokenizer, insert source segments, run initialization, and assert its SQL contains `tokenize='trigram'` and its row count/content matches `log_segments`. Run initialization twice to assert idempotence.

- [ ] **Step 2: Run tests and confirm failure**

Run: `cargo test --locked db::tests::trigram -- --nocapture`

Expected: FAIL because the current virtual table has no trigram tokenizer or schema detection.

- [ ] **Step 3: Implement schema detection and safe rebuild**

Read the table SQL from `sqlite_master`. If it does not contain the normalized trigram declaration, rebuild in one SQLite transaction so rollback restores the previous virtual table on any failure:

```sql
DROP TABLE log_segments_fts;
CREATE VIRTUAL TABLE log_segments_fts USING fts5(
  content,
  segment_id UNINDEXED,
  bundle_id UNINDEXED,
  file_id UNINDEXED,
  timeline UNINDEXED,
  tokenize='trigram'
);
INSERT INTO log_segments_fts(content, segment_id, bundle_id, file_id, timeline)
SELECT content, id, bundle_id, file_id, timeline FROM log_segments;
```

Emit start/completion logs with segment count and elapsed time; propagate failure so startup does not serve incomplete results.

- [ ] **Step 4: Run migration tests**

Run: `cargo test --locked db::tests::trigram -- --nocapture`

Run: `cargo test --locked --test smoke fts_trigram_migration -- --nocapture`

Expected: old schema rebuilds, content remains searchable, second initialization performs no rebuild.

- [ ] **Step 5: Commit**

```bash
git add backend/src/db.rs backend/tests/smoke.rs
git commit -m "feat: migrate log search to trigram fts"
```

### Task 7: Implement literal substring search and short-query fallback

**Files:**
- Modify: `backend/src/routes/logs.rs`
- Test: `backend/src/routes/logs.rs`
- Test: `backend/tests/smoke.rs`

- [ ] **Step 1: Add failing search regressions**

Index content containing `requestId=abcdef123456`, a UUID, punctuation, quotes, and contiguous Chinese text. Assert Bundle and Issue searches find internal substrings of at least three characters. Assert `ER` uses fallback, pagination remains bounded, and PROCESSING/FAILED Bundles are excluded from Issue search.

- [ ] **Step 2: Run tests and confirm failure**

Run: `cargo test --locked --test smoke substring_search -- --nocapture`

Expected: current quoted-token AND query fails partial identifier and Chinese substring cases.

- [ ] **Step 3: Replace token-AND query construction**

Use one safely quoted FTS literal for queries of at least three Unicode scalar values:

```rust
fn build_trigram_query(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}
```

Keep Bundle/Issue/path/status predicates unchanged. Validate behavior for whitespace as part of the literal search term rather than converting it to independent AND tokens.

- [ ] **Step 4: Add bounded fallback for shorter queries**

For fewer than three scalar values, query `log_segments.content LIKE ? ESCAPE '\\'` with escaped `%`, `_`, and `\\`; retain scope filters, `LIMIT`, `OFFSET`, and maximum result size. Count and row queries must use the same predicate. Build snippets from bounded segment content without returning an unbounded payload.

- [ ] **Step 5: Run route and smoke tests**

Run: `cargo test --locked routes::logs -- --nocapture`

Run: `cargo test --locked --test smoke substring_search -- --nocapture`

Expected: literal substring cases pass in Bundle and Issue scope; short queries work within existing result limits.

- [ ] **Step 6: Commit**

```bash
git add backend/src/routes/logs.rs backend/tests/smoke.rs
git commit -m "feat: support literal substring log search"
```

### Task 8: Verify deletion, rollback, and capacity reuse

**Files:**
- Modify: `backend/src/upload/finalizer.rs`
- Modify: `backend/src/routes/issues.rs`
- Modify: `backend/src/db.rs`
- Test: `backend/tests/smoke.rs`

- [ ] **Step 1: Add lifecycle regression tests**

Fill an Issue to quota, delete one Bundle, and assert a replacement upload can reserve the released bytes. Repeat with an overflowed FAILED Bundle and assert its zero reservation does not block a later upload. Verify expired-Bundle cleanup and Issue deletion remove all dependent rows without referencing `log_events`.

- [ ] **Step 2: Run tests and confirm any lifecycle gaps**

Run: `cargo test --locked --test smoke issue_quota_lifecycle -- --nocapture`

Expected before final fixes: at least one assertion exposes stale content size, cleanup stats, or release behavior.

- [ ] **Step 3: Make lifecycle state transitions consistent**

Ensure failed finalization sets content size to zero only after dependent cleanup, READY finalization preserves the accumulated value, and deletion relies on row removal. Update cleanup logs/stats to report offsets, FTS segments, segments, and files only.

- [ ] **Step 4: Run lifecycle and full smoke tests**

Run: `cargo test --locked --test smoke issue_quota_lifecycle -- --nocapture`

Run: `cargo test --locked --test smoke -- --nocapture`

Expected: all lifecycle and smoke tests pass.

- [ ] **Step 5: Commit**

```bash
git add backend/src/upload/finalizer.rs backend/src/routes/issues.rs backend/src/db.rs backend/tests/smoke.rs
git commit -m "fix: release issue quota across bundle lifecycle"
```

### Task 9: Documentation and full verification

**Files:**
- Modify: `backend/.env.example`
- Modify: `doc/DB.md`
- Modify: other repository documentation only where it names removed settings or token-oriented search.

- [ ] **Step 1: Find and update stale documentation**

Run: `rg -n "RAIN_UPLOAD_MAX_|RAIN_ARCHIVE_MAX_|RAIN_INDEXING_MAX_EVENTS|log_events|250_000|250000|unicode61" backend doc docs README*`

Expected: only historical design/plan documents may retain old names; active docs and examples must describe the Issue quota, internal safeguards, removed events, and trigram search.

- [ ] **Step 2: Run formatting and whitespace validation**

Run: `cargo fmt --check`

Run: `git diff --check`

Expected: both exit successfully with no output from `git diff --check`.

- [ ] **Step 3: Run static analysis**

Run: `cargo clippy --locked -- -D warnings`

Expected: exit code 0 with no warnings.

- [ ] **Step 4: Run the complete backend suite**

Run: `cargo test --locked`

Expected: all unit, integration, and documentation tests pass.

- [ ] **Step 5: Run frontend validation**

Run from `frontend`: `npm test`

Run from `frontend`: `npm run build`

Expected: tests and production build pass.

- [ ] **Step 6: Inspect scope and commit documentation**

Run: `git status --short`

Run: `git diff --stat HEAD~8..HEAD`

Run: `git diff origin/main...HEAD -- backend/src backend/tests backend/.env.example doc/DB.md`

Expected: changes are limited to the approved quota, limits, structured-event removal, search, tests, and documentation.

```bash
git add backend/.env.example doc/DB.md
git commit -m "docs: document issue quota and substring search"
```
