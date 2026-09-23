# v0.1 Tantivy-first Release Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task with verification checkpoints.

**Goal:** Make the v0.1.x release Tantivy-first for fresh installations without building a legacy Bundle migration path. Existing SQLite-backed data must fail fast with an actionable fresh-data-directory error instead of silently losing search results.

**Architecture:** Enable the `tantivy-search` feature by default, make Tantivy the default backend when that feature is compiled, and validate the database before startup when Tantivy is selected. The existing SQLite adapter remains in the source tree for tests and future migration work, but v0.1 does not promise to open old SQLite-search databases.

**Scope:** No background migration, no FTS removal migration, and no changes to Skill search in this release. The release documentation will state the fresh-install requirement and Tantivy build command.

### Task 1: Add failing tests for v0.1 defaults and the fresh-data guard

- [x] Test that a feature build parses an unset `RAIN_SEARCH_BACKEND` as Tantivy while a no-feature build keeps SQLite as the compile-only fallback.
- [x] Test that a database containing a SQLite-backed Bundle is rejected for Tantivy startup.
- [x] Test that an empty database and a database containing only Tantivy publications pass the guard.
- [x] Run the focused tests and confirm the expected RED state.

### Task 2: Implement Tantivy-first defaults and startup validation

- [x] Make `tantivy-search` a default Cargo feature.
- [x] Make `SearchBackendKind::parse(None)` select Tantivy in feature builds.
- [x] Add a publication/database guard that rejects persisted `sqlite_fts` Bundle search rows when Tantivy is selected, with a clear v0.1 fresh-data message.
- [x] Invoke the guard immediately after schema preparation and before normal recovery/bootstrap work.
- [x] Keep explicit `RAIN_SEARCH_BACKEND=sqlite_fts` available for tests and no-feature tooling.

### Task 3: Update release configuration and documentation

- [x] Update `.env.example`, README, Tantivy prototype notes, and benchmark docs for the v0.1 fresh-install contract.
- [x] Document `cargo build --release` as Tantivy-enabled through the default feature and explain that old data directories require a fresh `DATABASE_URL`/`RAIN_DATA_ROOT`.
- [x] Add a release-oriented config test for the default backend and guard error wording.

### Task 4: Verify and prepare PR

- [x] Run format, default/feature checks, Clippy, library/integration tests, and the benchmark compile path.
- [x] Review `git diff --check`, remove temporary build symlinks, update this plan, commit, push, create a PR, and monitor CI.
