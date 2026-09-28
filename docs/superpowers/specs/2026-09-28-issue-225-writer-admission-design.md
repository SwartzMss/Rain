# Issue #225 SQLite Writer Admission Design

## Goal

Eliminate normal single-process SQLite writer bypasses that can produce `SQLITE_BUSY_SNAPSHOT` during long-running file deletion, while preserving concurrent readers, short deletion batches, and the existing bounded BUSY retry fallback.

This change also stabilizes two existing unit tests that fail only under the default parallel test scheduler: readiness tests must not serialize unrelated private in-memory databases, and the Tantivy drop-cleanup test must wait for its blocking worker without treating normal scheduler contention as a functional failure.

## Context and root causes

`backend/src/db/write.rs` already provides a per-database FIFO writer admission, rollback, telemetry, and bounded retry. `run` currently acquires that admission but starts a normal deferred SQL transaction with `pool.begin()`. Several runtime paths still call `.execute(pool)` or `pool.begin()` directly, so they bypass the admission entirely.

The resulting read-then-write upgrade can fail with `SQLITE_BUSY_SNAPSHOT`: an admitted transaction reads an old snapshot, an unadmitted writer commits, and the first transaction later attempts to become a writer. Large deletion jobs amplify the timing window because they execute many short batches.

The baseline readiness timeout is scheduler-sensitive: the focused test and the serial full suite pass, while the initial default-parallel run timed out during cancellation recovery. SQLx gives independently-created private in-memory pools distinct internal filenames, so the writer-key implementation must preserve its existing file-key behavior rather than inventing a separate memory-pool identity scheme. The test will use a unique file-backed fixture to remove unrelated scheduler and pool setup variables.

The baseline Tantivy timeout is scheduler-sensitive: a dropped pipeline signals a `spawn_blocking` worker and the worker owns the resource permit until it exits. Under the full parallel suite, the worker can wait behind other blocking jobs longer than the test's fixed ten-second guard. Focused and serial runs pass, and the production lifetime rule must remain unchanged.

## Design

### 1. One admission API for runtime writes

Extend `backend/src/db/write.rs` with two transaction entry points:

- `run(pool, operation, input, closure)` remains the short replayable write API.
- `transaction(pool, operation, closure)` supports closures that need multiple SQL statements with one atomic commit.

Both APIs acquire the existing per-database FIFO guard before borrowing a pool connection. Both use `BEGIN IMMEDIATE` so SQLite reserves the writer slot before any transaction read can establish a deferred snapshot. Both retain the current rollback, telemetry, retry classification, and release-between-attempts behavior. A transaction closure may only use its supplied transaction/connection and replayable input; it must not perform filesystem or network work, sleep, external side effects, nested admission, or nested transaction creation.

The implementation will acquire a connection, execute `BEGIN IMMEDIATE`, and use the resulting connection-backed transaction for the closure. The admission guard and transaction are held only for the SQL work. The existing health probe remains an explicit exceptional path because it must always roll back; it will use the same admission and immediate-begin helper without committing.

### 2. Migrate runtime writers

Migrate all normal runtime INSERT, UPDATE, DELETE, and multi-statement write paths to the two APIs. The focused migration set includes:

- `services/file_deletion.rs`
- `routes/issues.rs`, `routes/admin.rs`, and `routes/temp_results/service.rs`
- `repositories/users.rs`, `repositories/sessions.rs`, and `repositories/saved_searches.rs`
- `search/publication.rs`
- `blob_store.rs`
- any additional runtime writer found by the audit during implementation

Pure SELECT paths continue to use the pool directly. Migrations, startup initialization, test fixtures, and explicitly documented rollback-only probes remain exceptions. Existing filesystem and Tantivy operations stay outside SQL write transactions; deletion remains batch-based.

### 3. Admission key correctness

File-backed SQLite pools continue to share a writer mutex by canonical database path. SQLx assigns independently-created private in-memory pools distinct internal filenames, while clones of one pool retain the same connection options; a regression test will preserve and document those semantics. No separate memory-only keying rule is needed.

### 4. Runtime writer audit

Add a repository script that scans runtime Rust sources for direct `.execute(pool)`, `.execute(&state.db.pool)`, and `pool.begin()` patterns, excluding migrations, tests, and the documented rollback-only/initialization exceptions. The script will fail with the file and line for a new unallowlisted match. Add the script as a CI step so adding a direct runtime writer requires an explicit review and allowlist entry.

The audit is a guardrail, not the implementation mechanism: all existing matches will be reviewed and either migrated or documented as an exception.

### 5. Existing scheduler-sensitive tests

Change the readiness test fixture to use a unique file-backed temporary SQLite database (or an explicitly unique SQLite URI) so its writer gate cannot contend with unrelated private in-memory pools. Keep the assertion that a pending readiness refresh releases its cache lock after cancellation.

For the Tantivy drop-cleanup test, retain the assertion that dropping the pipeline eventually releases the permit and staging directory. Replace the brittle fixed short wait with a condition-based polling helper whose upper guard reflects the full parallel test environment; the helper must still fail if the worker never releases the permit. No production resource ownership change is part of this stabilization.

## Data flow

```text
runtime SQL write
      |
      v
db::write::run / db::write::transaction
      |
      +--> per-database FIFO admission
      |
      +--> BEGIN IMMEDIATE
      |
      +--> short SQL closure
      |
      +--> COMMIT or ROLLBACK
      |
      +--> bounded BUSY retry only for exceptional contention
```

Readers continue to borrow the pool directly. WAL mode preserves reader concurrency; only runtime writers are serialized per database.

## Error handling

- Any SQL, begin, commit, or rollback error is returned as `AppError::Database`.
- SQLite BUSY-family errors retain the existing maximum of three replay attempts and telemetry.
- A failed closure is rolled back before its error is returned.
- A rollback failure suppresses retry, as today, because the transaction state is no longer known to be safe to replay.
- Transactions never include non-replayable external side effects.

## Testing strategy

Add or update tests before implementation for:

1. `BEGIN IMMEDIATE` is used by the unified write path.
2. Concurrent writers through one pool and through two pools for the same file are serialized in FIFO order.
3. Independent private in-memory pools do not block each other's admission; clones of one pool still do.
4. Reader queries can execute while another task is waiting for writer admission.
5. A large-file deletion batch can commit concurrently with session, admin, issue activity, and publication metadata writes without direct-writer lock failures.
6. The audit script rejects a new runtime direct writer and accepts documented exceptions.
7. Readiness cancellation releases its cache lock under the parallel test suite.
8. Dropping a Tantivy pipeline eventually releases its permit and removes staging output under the parallel test suite.

Verification will include focused red/green tests, `cargo fmt --check`, `cargo check --locked`, `cargo clippy --locked -- -D warnings`, `cargo test --locked`, the audit script, and `git diff --check`.

## Scope and non-goals

In scope: complete single-process runtime writer admission, immediate write transactions, runtime writer audit, concurrency regressions, and the two confirmed baseline test stabilizations.

Out of scope: issue quota timing, temp-result preview search strategy, Tantivy indexing throughput, changing SQLite busy timeout/retry counts as a workaround, and isolation from external processes that modify the database.
