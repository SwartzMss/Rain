# Database Migration Framework Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 Rain 当前完整 SQLite schema 固化为 SQLx `0001_initial` baseline，安全接管已有 legacy 数据库，并让后续启动、reset、测试和文档都使用同一套可校验的 migration chain。

**Architecture:** 保留 `db::prepare_schema(pool, reset)` 作为应用层唯一入口；在入口内部区分空库、无 metadata 的 legacy 库和已受 SQLx 管理的数据库。legacy 库先通过严格的只读 schema validator，再由 SQLx Migrator 记录真实 baseline checksum；已管理数据库直接交给 SQLx 检查 dirty/checksum/版本。迁移完成后才允许 recovery、后台任务和 HTTP 服务继续启动。

**Tech Stack:** Rust, Actix Web, SQLx 0.7 SQLite migrations, SQLite FTS5, Tokio tests, `sqlx::migrate!()`。

---

## Task 1: Enable SQLx migrations and add the baseline SQL

**Files:**

- Modify `backend/Cargo.toml`
- Create `backend/migrations/0001_initial.sql`

- [ ] **Step 1: Write the migration feature change and a compile-time migration smoke test first.**

  Add SQLx's `migrate` feature and add a small test in the migration test module that references `sqlx::migrate!()` so the first red state is a missing migration directory or feature rather than an unverified runtime path.

- [ ] **Step 2: Run the focused compile/test command and record the expected red result.**

  ```bash
  cargo test -p backend migration_smoke --no-default-features
  ```

  The command is expected to fail until the migration module and baseline fixture are present; do not bypass the failure by restoring the old `create_schema()` path.

- [ ] **Step 3: Create `0001_initial.sql` from the current schema snapshot.**

  Move the effective DDL currently assembled by `create_schema()` into one ordered migration. It must include all current tables, constraints, foreign keys, defaults, partial/unique indexes, `rain_ready_probe`, `skill_runs.analysis_*`, `log_segments.event_time_*`, the event-time index, the external-content trigram FTS5 table, and all three FTS synchronization triggers.

  Keep the SQL safe to execute against a validated legacy database (`IF NOT EXISTS` where SQLite requires it for idempotent baseline adoption), but do not put runtime-only behavior into the file: no event-time data scan, no saved-search data repair, and no unversioned backfill loop.

- [ ] **Step 4: Make the baseline migration compile and pass its focused smoke test.**

  ```bash
  cargo test -p backend migration_smoke
  ```

  Verify the migration file is discoverable at compile time and its checksum is stable.

## Task 2: Introduce migration state detection and strict legacy validation

**Files:**

- Create `backend/src/db/migrations.rs`
- Modify `backend/src/db.rs`
- Modify `backend/src/main.rs` only if startup error context needs to be improved

- [ ] **Step 1: Add failing tests for the three database states before changing startup behavior.**

  Add tests covering:

  1. an empty SQLite database;
  2. a complete legacy schema with a sentinel business row but no `_sqlx_migrations` table;
  3. a partial/incompatible legacy schema missing a required column or index.

  The tests should assert the intended result, so they initially fail because `prepare_schema()` still calls the old implicit schema path.

- [ ] **Step 2: Run only the new state-detection tests and confirm the red state.**

  ```bash
  cargo test -p backend db::tests::migration_state -- --nocapture
  ```

- [ ] **Step 3: Implement a compile-time `Migrator`.**

  Define one application-level static migrator using `sqlx::migrate!()` pointed at `backend/migrations`. Keep migration file loading and SQLx's metadata/checksum/dirty checks inside this module rather than duplicating `_sqlx_migrations` bookkeeping.

- [ ] **Step 4: Implement read-only database classification.**

  Detect whether `_sqlx_migrations` exists and whether any non-SQLite-internal user objects exist. Treat a database with migration metadata as managed, a non-empty database without metadata as legacy, and a database with no user objects as empty. Do not classify a partial schema as empty and do not silently create missing pieces before validation.

- [ ] **Step 5: Implement strict baseline compatibility validation for legacy databases.**

  Validate the required tables and columns, type/nullability/default properties, required constraints, indexes/partial indexes, FTS5 external-content configuration, and the three FTS triggers. Validation may tolerate unrelated extra objects, but every missing or mismatched required object must produce an error naming the object and the reason. The validator must not update data or repair schema.

- [ ] **Step 6: Route all three states through SQLx migration execution.**

  Empty databases call the migrator directly. Legacy databases validate first and then call the same migrator so SQLx records the actual `0001` version/checksum. Managed databases call the migrator directly so dirty rows, checksum changes, missing migration files, and version conflicts fail fast.

- [ ] **Step 7: Make the state-detection tests green.**

  ```bash
  cargo test -p backend db::tests::migration_state -- --nocapture
  ```

  Confirm that legacy data remains untouched and that an incompatible legacy database fails before any schema repair occurs.

## Task 3: Replace `create_schema()` and reset with the migration-aware entry point

**Files:**

- Modify `backend/src/db.rs`
- Modify `backend/src/main.rs` if needed for startup ordering/error propagation
- Modify or remove obsolete schema helpers in `backend/src/db.rs`

- [ ] **Step 1: Add failing tests for idempotent startup, metadata integrity, reset, and migration failure ordering.**

  Add tests that:

  - call `prepare_schema()` twice and assert exactly one successful version-1 row;
  - preserve a sentinel row across legacy adoption and restart;
  - set a migration row dirty or alter its checksum and assert startup fails;
  - reset a test database and assert the same migration chain recreates the schema and metadata;
  - return an error from schema preparation so a recovery marker/background task cannot be reached.

- [ ] **Step 2: Run the focused tests and confirm failures come from the old schema path.**

  ```bash
  cargo test -p backend db::tests::prepare_schema -- --nocapture
  ```

- [ ] **Step 3: Rewrite `prepare_schema()` to run migrations before any recovery work.**

  Preserve its public signature for existing callers. Its sequence must be reset (if explicitly requested), classify, validate/adopt if necessary, then run the SQLx migrator and return. `main.rs` must continue to await this function before settings loading that can start recovery/background work or bind HTTP.

- [ ] **Step 4: Implement destructive reset through the same chain.**

  Drop FTS triggers before the FTS virtual table, drop all known application tables, and remove `_sqlx_migrations` only for an explicit `reset=true` test/development path. Then invoke the same migrator used by normal startup. Do not leave a second hand-written schema creation path.

- [ ] **Step 5: Remove long-term `ensure_*` startup upgrades.**

  Delete or make unreachable the old optional-column/index creation and saved-search repair logic. If event-time rows need reconciliation for an adopted current legacy database, expose it as an explicit, bounded, resumable adoption step with a clear log boundary; it must not scan on every managed startup and must not masquerade as schema migration.

- [ ] **Step 6: Make the focused lifecycle tests green.**

  ```bash
  cargo test -p backend db::tests::prepare_schema -- --nocapture
  cargo test -p backend db::tests::reset -- --nocapture
  ```

## Task 4: Add migration-chain integration coverage

**Files:**

- Modify `backend/src/db.rs` tests or create `backend/tests/db_migrations.rs`
- Create `backend/tests/fixtures/migrations/0001_initial.sql` only if a dynamic test migrator needs an isolated fixture directory
- Create `backend/tests/fixtures/migrations/0002_test_marker.sql` for future-migration ordering coverage if the test cannot safely use the production directory

- [ ] **Step 1: Add a failing empty-database schema inventory test.**

  Initialize a fresh SQLite file through `prepare_schema(false)`, then assert `_sqlx_migrations` contains version 1 with `success=1`, and query `sqlite_master` for representative tables, indexes, FTS table, and FTS triggers. Include assertions for the event-time and analysis columns.

- [ ] **Step 2: Add a failing legacy adoption preservation test.**

  Build a complete pre-metadata legacy schema, insert a sentinel issue/bundle/log row, call `prepare_schema(false)`, and assert all sentinel values remain, migration metadata is recorded, and no second baseline application occurs on restart.

- [ ] **Step 3: Add failing rejection tests for partial and structurally incorrect schemas.**

  Cover at least a missing `skill_runs.analysis_start_time`, a missing event-time index, and a mismatched FTS definition/trigger. Assert errors include the affected object and that no `_sqlx_migrations` success row is created.

- [ ] **Step 4: Add future migration and idempotent restart coverage.**

  Use a test-only migrator fixture containing the production baseline plus `0002_test_marker.sql`, or another isolated SQLx migrator arrangement that first reaches version 1 and then applies version 2. Assert version 2 runs once, the marker exists, and a second run does not duplicate it. Also cover dirty/checksum rejection using the real `_sqlx_migrations` metadata contract.

- [ ] **Step 5: Run the integration tests.**

  ```bash
  cargo test -p backend --test db_migrations -- --nocapture
  ```

## Task 5: Migrate existing test setup to the new contract

**Files:**

- Search and modify tests under `backend/src/` and `backend/tests/` that call schema helpers directly
- Modify `backend/src/db.rs` unit tests that currently assert `ensure_skill_run_optional_columns`, `ensure_log_segment_optional_columns`, or unconditional event-time backfill

- [ ] **Step 1: Inventory all direct schema setup and ensure-helper calls.**

  ```bash
  rg -n "create_schema|ensure_skill_run|ensure_log_segment|backfill_log_segment|prepare_schema" backend/src backend/tests
  ```

- [ ] **Step 2: Convert tests to call `prepare_schema()` or a dedicated migration test helper.**

  Tests must exercise the same migration chain as production. Remove assertions that expect every startup to add columns or rescan historical rows; replace them with baseline/adoption or explicit data-reconciliation tests.

- [ ] **Step 3: Preserve test isolation and close SQLite pools before temporary-directory cleanup.**

  Keep the prior release-CI fixes for private in-memory databases and Windows cleanup intact. Migration tests using file-backed SQLite must drop/close pools before removing the file or directory.

- [ ] **Step 4: Run the complete backend test suite.**

  ```bash
  cargo test -p backend
  ```

## Task 6: Document migration and legacy adoption behavior

**Files:**

- Modify `doc/DB.md`
- Modify `README.md`

- [ ] **Step 1: Add documentation assertions/checklist before editing prose.**

  Ensure the docs explicitly cover automatic pending migrations, the baseline compatibility check, fail-fast behavior for unknown/partial schemas, checksum/dirty failures, reset being development/test-only, and the fact that data is not deleted or guessed into compatibility.

- [ ] **Step 2: Update `doc/DB.md` with the operational contract.**

  Explain the migration directory, `_sqlx_migrations`, the empty/legacy/managed branches, safe restart behavior, and how to recover from an incompatible database (backup and deliberate operator action rather than automatic repair).

- [ ] **Step 3: Add a concise README startup note.**

  Link to `doc/DB.md` and state that normal startup runs migrations before recovery and HTTP service startup.

- [ ] **Step 4: Check documentation links and formatting.**

  ```bash
  git diff --check
  ```

## Task 7: Full verification and PR handoff

**Files:**

- No new files; verify all changed files and generated migration contents

- [ ] **Step 1: Format and run focused migration tests.**

  ```bash
  cargo fmt --all -- --check
  cargo test -p backend migration_smoke -- --nocapture
  cargo test -p backend --test db_migrations -- --nocapture
  ```

- [ ] **Step 2: Run the full backend test suite and build.**

  ```bash
  cargo test -p backend
  cargo check -p backend
  ```

- [ ] **Step 3: Inspect the final diff for safety invariants.**

  Confirm there is one production migration entry point, no startup call to the old unversioned ensure path, no destructive reset outside `reset=true`, no migration that logs business data, and no test-only fixture accidentally included in production migrations.

- [ ] **Step 4: Commit the implementation and prepare the PR.**

  ```bash
  git diff --check
  git status --short
  git add backend/Cargo.toml backend/migrations backend/src backend/tests doc/DB.md README.md
  git commit -m "feat: add sqlite migration framework"
  ```

  Before claiming completion, run the verification commands again after the commit and use the resulting commit for the new pull request linked to issue #56.
