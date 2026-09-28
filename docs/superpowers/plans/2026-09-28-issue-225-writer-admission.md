# Issue #225 SQLite Writer Admission Implementation Plan

> For agentic workers: REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox syntax for tracking.

**Goal:** Route every normal runtime SQLite writer through one FIFO admission and BEGIN IMMEDIATE, add concurrency regressions and an audit guard, and stabilize the two parallel-suite baseline timeouts.

**Architecture:** db::write owns admission, immediate transaction startup, commit/rollback, telemetry, and bounded BUSY retry. Runtime repositories and services use run for short writes and transaction for atomic multi-statement writes. Readers, migrations, startup initialization, and test fixtures retain direct pool access.

**Tech Stack:** Rust 2024, Tokio, SQLx 0.7 SQLite, Actix tests, shell/rg audit script, GitHub Actions.

---

## File map

- Modify backend/src/db/write.rs: writer-key calculation, BEGIN IMMEDIATE, transaction API, shared retry/telemetry, and focused admission tests.
- Modify runtime writers in backend/src/repositories/users.rs, saved_searches.rs, sessions.rs, routes/issues.rs, routes/admin.rs, routes/temp_results/service.rs, services/file_deletion.rs, search/publication.rs, and blob_store.rs.
- Modify backend/src/routes/health.rs: immediate-begin rollback-only probe and isolated cancellation-test database.
- Create scripts/audit-runtime-sqlite-writers.sh and modify .github/workflows/ci.yml.
- Add backend/tests/sqlite_writer_admission.rs for file-backed WAL concurrency coverage.

## Task 1: Add the immediate transaction core and correct admission keys

**Files:**
- Modify: backend/src/db/write.rs
- Test: backend/src/db/write.rs test module

- [ ] Step 1: Add a failing test proving run reserves the writer before its first read.

Use the existing fixture. Start run(&pool, "immediate-test", &(), closure); the closure selects counter, notifies the test, waits on a Notify, then updates counter. After the notification, execute UPDATE counter through external and assert it returns a SQLite database error. Release the notification, assert run succeeds, and assert the counter is exactly one.

    #[tokio::test]
    async fn run_uses_begin_immediate_before_closure_reads() {
        let (root, pool, external) = fixture().await;
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let entered_for_run = entered.clone();
        let release_for_run = release.clone();
        let task = tokio::spawn(async move {
            run(&pool, "immediate-test", &(), move |conn, _| {
                let entered = entered_for_run.clone();
                let release = release_for_run.clone();
                Box::pin(async move {
                    sqlx::query_scalar::<_, i64>("SELECT value FROM counter")
                        .fetch_one(&mut *conn).await?;
                    entered.notify_one();
                    release.notified().await;
                    sqlx::query("UPDATE counter SET value=value+1")
                        .execute(conn).await?;
                    Ok(())
                })
            }).await
        });
        entered.notified().await;
        let error = sqlx::query("UPDATE counter SET value=value+1")
            .execute(&external).await.expect_err("writer must be reserved");
        assert!(matches!(error, sqlx::Error::Database(_)));
        release.notify_one();
        task.await.unwrap().unwrap();
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT value FROM counter")
            .fetch_one(&external).await.unwrap(), 1);
        cleanup_fixture(root, pool, external).await;
    }

- [ ] Step 2: Run the focused test and verify it fails against deferred pool.begin().

Run: cargo test --locked --lib db::write::tests::run_uses_begin_immediate_before_closure_reads -- --exact --nocapture

Expected: FAIL because the external update commits while the deferred transaction only holds a read snapshot.

- [ ] Step 3: Add a failing admission-key test for private in-memory pools.

Create first and second with SqlitePool::connect_lazy("sqlite::memory:"), clone first, hold acquire(&first), and assert acquire(&first_clone) times out while acquire(&second) completes. This proves clones share a key and independent private memory pools do not.

- [ ] Step 4: Refactor WRITERS keys.

Replace HashMap<PathBuf, Weak<Mutex<()>>> with:

    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    enum WriterKey {
        File(PathBuf),
        PrivateMemory(usize),
    }

For :memory:, use Arc::as_ptr(&pool.connect_options()) as usize. For file-backed databases preserve canonicalization and the relative-path fallback. Keep weak-entry cleanup.

- [ ] Step 5: Add begin_immediate and make run use it.

The helper acquires only a connection so explicitly coordinated health/blob paths can hold admission across their coordination:

    pub async fn begin_immediate(
        pool: &SqlitePool,
    ) -> Result<sqlx::pool::PoolConnection<sqlx::Sqlite>, AppError> {
        let mut connection = pool.acquire().await.map_err(AppError::Database)?;
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *connection)
            .await.map_err(AppError::Database)?;
        Ok(connection)
    }

Refactor the retry loop to acquire the guard, call begin_immediate, run the existing closure against &mut SqliteConnection, then issue COMMIT or ROLLBACK. Preserve MAX_ATTEMPTS, BUSY classification, rollback-failure handling, queue timing, and all sqlite_write fields.

- [ ] Step 6: Add transaction(pool, operation, execute) by reusing the same retry engine with () input. Use closure type:

    F: for<'c> FnMut(&'c mut SqliteConnection)
        -> BoxFuture<'c, Result<T, AppError>>

Keep run's input-bearing signature and do not add a second mutex or retry implementation.

- [ ] Step 7: Run cargo test --locked --lib db::write::tests -- --nocapture, expect all writer tests to pass, and commit:

    git add backend/src/db/write.rs
    git commit -m "fix: start admitted sqlite writes immediately"

## Task 2: Migrate repositories and issue/admin runtime writers

**Files:**
- Modify: backend/src/repositories/users.rs
- Modify: backend/src/repositories/saved_searches.rs
- Modify: backend/src/repositories/sessions.rs
- Modify: backend/src/routes/issues.rs
- Modify: backend/src/routes/admin.rs
- Test: backend/tests/auth.rs, backend/tests/admin.rs, and module tests

- [ ] Step 1: Add a failing file-backed WAL concurrency regression for users::create_user and issues::create_issue. Hold an external read-then-write transaction while invoking each public mutation; assert the mutation completes without BUSY_SNAPSHOT or database is locked.

- [ ] Step 2: Run cargo test --locked --test sqlite_writer_admission runtime_user_and_issue_writes_use_admission -- --nocapture and verify the direct writer path fails or exhibits the lock race.

- [ ] Step 3: Wrap single-statement user, saved-search, session, issue-activity, issue-create, admin-audit, and session-revocation writes with db::write::run. Use owned replayable input tuples; keep post-write SELECTs outside the write transaction. Preserve unique-violation and API error classification.

Example:

    let input = (id.clone(), username.to_owned(), normalized.clone(), password_hash.to_owned());
    let result = crate::db::write::run(pool, "create user", &input,
        |conn, (id, username, normalized, password_hash)| {
            Box::pin(async move {
                sqlx::query("INSERT INTO users (id, username, username_normalized, password_hash) VALUES (?, ?, ?, ?)")
                    .bind(id).bind(username).bind(normalized).bind(password_hash)
                    .execute(conn).await.map(|_| ()).map_err(AppError::Database)
            })
        }).await;

- [ ] Step 4: Move the existing multi-statement session and admin operations from pool.begin() into transaction(pool, operation, closure). Preserve early-return values by returning them from the closure; let the wrapper own commit and rollback. Keep read-only role lookups outside.

- [ ] Step 5: Run cargo test --locked --lib repositories::users::tests repositories::sessions::tests and cargo test --locked --test auth --test admin. Expect existing behavior and the new regression to pass.

- [ ] Step 6: Commit:

    git add backend/src/repositories backend/src/routes/issues.rs backend/src/routes/admin.rs backend/tests
    git commit -m "fix: route auth and issue writers through sqlite admission"

## Task 3: Migrate deletion, temp-result, and publication metadata writers

**Files:**
- Modify: backend/src/services/file_deletion.rs
- Modify: backend/src/routes/temp_results/service.rs
- Modify: backend/src/search/publication.rs
- Test: backend/tests/sqlite_writer_admission.rs and module tests

- [ ] Step 1: Add a failing file-backed WAL regression with a queued deletion job and concurrent session/activity/publication metadata writes. Synchronize overlap with a barrier, collect all results, and assert no error contains BUSY_SNAPSHOT or database is locked.

- [ ] Step 2: Run cargo test --locked --test sqlite_writer_admission deletion_can_overlap_runtime_metadata_writes -- --nocapture and verify the direct writer path fails or resolves the lock race incorrectly.

- [ ] Step 3: Wrap every short file-deletion state transition in process_file_deletion_batches, process_file_deletion_jobs, and retry/yield helpers with db::write::run. Keep discovery SELECTs outside, preserve batch sizes, and keep filesystem deletion, sleeps, and scheduling outside the closure.

- [ ] Step 4: Convert temp-result runtime INSERT/UPDATE/DELETE cleanup writes to db::write::run or the existing repository API. Leave test-fixture SQL direct and keep filesystem artifact removal outside SQL closures.

- [ ] Step 5: Convert search publication heartbeat, reset, claim, and cleanup metadata writes to run; use transaction for related atomic SQL changes. Keep generation lease operations and filesystem cleanup outside SQL transactions, preserving lifecycle lock order. Leave SELECTs direct.

- [ ] Step 6: Run focused deletion, publication, temp-result, and sqlite_writer_admission tests; expect zero lock errors; commit:

    git add backend/src/services/file_deletion.rs backend/src/routes/temp_results/service.rs backend/src/search/publication.rs backend/tests/sqlite_writer_admission.rs
    git commit -m "fix: serialize deletion and publication metadata writers"

## Task 4: Preserve and harden explicitly coordinated blob/health writes

**Files:**
- Modify: backend/src/blob_store.rs
- Modify: backend/src/routes/health.rs
- Test: module-local blob and health tests

- [ ] Step 1: Add a failing health probe test using a unique file-backed temporary database. Hold admission on a second pool, poll check_database, and assert it cannot write until the gate is released.

- [ ] Step 2: Replace health's direct pool.begin() with db::write::begin_immediate. Keep the explicit writer guard, execute the probe insert through the returned connection, always ROLLBACK, and release the guard after rollback. Use a unique file fixture in the cancellation test.

- [ ] Step 3: Replace blob-store coordinated pool.begin() calls with begin_immediate while retaining the explicit guard and rollback-on-filesystem-error behavior. Add comments documenting these as audit exceptions because filesystem deletion must remain in the coordination window and cannot be replayed.

- [ ] Step 4: Run cargo test --locked --lib routes::health::tests blob_store::tests; expect all cancellation and filesystem rollback tests to pass; commit:

    git add backend/src/blob_store.rs backend/src/routes/health.rs
    git commit -m "fix: use immediate begin for coordinated sqlite writes"

## Task 5: Add the runtime writer audit and CI gate

**Files:**
- Create: scripts/audit-runtime-sqlite-writers.sh
- Modify: .github/workflows/ci.yml
- Test: shell self-check fixtures

- [ ] Step 1: Write shell self-checks with a temporary source tree containing one direct .execute(pool) and one db::write::run call. Assert the script rejects the direct writer and accepts the wrapped call. Add explicit exact file:line exceptions for migrations, health rollback-only probe, and blob coordinated transactions only when comments document them.

- [ ] Step 2: Run bash scripts/audit-runtime-sqlite-writers.sh before migration is complete. Expected: FAIL with every remaining runtime direct writer and its line.

- [ ] Step 3: Implement the audit with rg over backend/src only. Exclude migrations and test files, reject:

    rg -n --glob '*.rs' '\.execute\((pool|&state\.db\.pool|state\.db\.pool)\)|\b(pool|state\.db\.pool)\.begin\(\)' backend/src

Normalize matches through an exact file:line allowlist so new matches cannot silently pass. Exit zero only when no unallowlisted match remains.

- [ ] Step 4: Add this CI step before Rust compilation:

    - name: Audit runtime SQLite writers
      run: bash scripts/audit-runtime-sqlite-writers.sh

- [ ] Step 5: Run the script expecting exit 0 and commit:

    git add scripts/audit-runtime-sqlite-writers.sh .github/workflows/ci.yml
    git commit -m "ci: audit runtime sqlite writer admission"

## Task 6: Stabilize the two confirmed parallel-suite failures

**Files:**
- Modify: backend/src/routes/health.rs
- Modify: backend/src/search/tantivy/pipeline.rs
- Test: corresponding module tests

- [ ] Step 1: Add a test-only condition-based helper for dropped pipeline cleanup:

    async fn wait_for_dropped_pipeline_cleanup(
        budget: &SearchResourceBudget, path: &Path,
    ) {
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                if budget.active_writers() == 0 && !path.exists() {
                    return;
                }
                tokio::task::yield_now().await;
            }
        }).await.expect("pipeline cleanup must release permit and staging path");
    }

Retain the drop assertion; only replace the brittle short polling guard.

- [ ] Step 2: Run the two formerly failing tests by name, then cargo test --locked --lib -- --test-threads=1. Expect focused tests and serial suite to pass.

- [ ] Step 3: Run the default parallel suite once after all migrations. If the scheduler-sensitive tests still fail, return to evidence from their full stack traces rather than raising the guard again. After green, commit:

    git add backend/src/routes/health.rs backend/src/search/tantivy/pipeline.rs
    git commit -m "test: stabilize parallel sqlite and pipeline cleanup checks"

## Task 7: Full verification, review, and PR

- [ ] Step 1: Run:

    cargo fmt --check
    cargo check --locked
    cargo clippy --locked -- -D warnings
    bash scripts/audit-runtime-sqlite-writers.sh
    git diff --check

Expected: all commands exit 0.

- [ ] Step 2: Run cargo test --locked. Expected: every unit and integration test passes under the default parallel scheduler with no lock-related timeout failures.

- [ ] Step 3: Check Issue #225 acceptance item by item: normal writers use admission; write transactions begin immediate; readers remain concurrent; retry remains bounded; deletion plus metadata writes succeed; audit runs in CI; replayable transactions contain no filesystem/network work; and both baseline failures are resolved.

- [ ] Step 4: Request final code review against main. Include the design requirements and fresh verification output; fix every Critical or Important finding before pushing.

- [ ] Step 5: Create /tmp/issue-225-pr-body.md with Closes #225, implementation summary, audit/test summary, and exact verification results. Then run:

    git status --short --branch
    git log --oneline main..HEAD
    git push -u origin fix/issue-225-writer-admission
    gh pr create --repo SwartzMss/Rain --base main --head fix/issue-225-writer-admission --title "fix: unify SQLite writer admission" --body-file /tmp/issue-225-pr-body.md
