# Skill Run Wall-Clock Time Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task with spec and quality reviews.

**Goal:** Remove UTC/RFC3339/timezone semantics from the Skill Run time scope while preserving server-bound filtering, indexing coverage, persistence, and bounded expansion.

**Architecture:** Parse API and log timestamps through one shared `chrono::NaiveDateTime` wall-clock module, encode them as deterministic comparison keys, and keep the existing nullable `*_ms` database columns only as storage names. Incident window arithmetic stays in Rust; the frontend only sends incident/range input. The Run still owns the immutable scope and `search_logs` remains the sole place that applies it.

**Tech Stack:** Rust, Axum, SQLx/SQLite, Chrono `NaiveDateTime`, React/TypeScript, Vitest.

---

### Task 1: Replace absolute time scope parsing with wall-clock parsing

**Files:** `backend/src/services/skill_time_scope.rs`, its unit tests, `backend/src/routes/skill_runs.rs`, `backend/src/models/skill_runs.rs`, `backend/src/repositories/skill_runs.rs`, `backend/src/services/skill_runner.rs`, related backend tests.

- [ ] Add failing tests for space/T-separated values without timezone, fractional seconds, frontend minute precision, `start < end`, 24-hour maximum, and expansion across calendar boundaries.
- [ ] Replace `DateTime<Utc>`/`timestamp_millis()` with one shared `NaiveDateTime` wall-clock parser/formatter/key encoder. Use checked duration arithmetic for incident validation and expansion.
- [ ] Preserve persisted strings, nullable Run binding, Trusted Run Scope, and existing API error code; change messages and test fixtures away from RFC3339/UTC.
- [ ] Run focused backend tests, then commit.

### Task 2: Parse real no-timezone log timestamps

**Files:** `backend/src/ingest.rs`, `backend/src/db.rs`, ingest/db tests.

- [ ] Add failing parser/indexing tests for plain, bracketed, severity-prefixed, fractional, and time-only log lines.
- [ ] Implement conservative prefix parsing into the same wall-clock comparison key; do not infer a date from `HH:mm:ss`.
- [ ] Keep the existing `event_time_indexed` lifecycle and batched backfill; update comments and SQL bind names without changing the existing `*_ms` schema columns.
- [ ] Run ingest and database tests, then commit.

### Task 3: Preserve scoped search behavior with wall-clock keys

**Files:** `backend/src/services/skill_tools.rs`, search tests, runner integration tests.

- [ ] Update scoped predicates and response serialization to use wall-clock terminology while keeping server-owned bounds, coverage/excluded-unindexed metadata, and `LIMIT max_hits + 1` truncation.
- [ ] Add regression coverage proving common no-timezone log lines are included and only genuinely unindexed matches are reported as excluded.
- [ ] Run focused search/runner tests, then commit.

### Task 4: Remove frontend UTC conversion

**Files:** `frontend/src/features/skill-runs/timeScope.ts`, `frontend/src/api/types.ts`, `frontend/src/api/client.ts`, `frontend/src/features/skill-runs/useSkillRun.ts`, frontend behavior tests.

- [ ] Add failing tests asserting incident/range request passthrough without `Z`, offsets, `toISOString()`, or frontend date arithmetic.
- [ ] Implement the small request adapter and let the backend own date ordering and 24-hour validation.
- [ ] Run focused frontend tests and lint, then commit.

### Task 5: Align documentation and full verification

**Files:** `README.md`, `doc/DB.md`, approved design/plan references and any remaining time-scope docs/tests.

- [ ] Remove claims that require RFC3339, explicit timezone, UTC normalization, or Unix epoch event times; document the wall-clock comparison-key meaning and conservative time-only behavior.
- [ ] Search for stale `toISOString`, RFC3339/timezone requirements, UTC canonicalization, and misleading scope field descriptions.
- [ ] Run backend format/build/tests, frontend tests/lint/build, diff checks, and PR CI checks; push the resulting commits to PR #128.
