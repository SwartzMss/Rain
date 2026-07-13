# Shareable Temporary Log Results Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create shareable seven-day temporary log files from boolean expressions over the currently opened log file.

**Architecture:** Parse boolean expressions into a small AST and evaluate them while streaming source lines. Store metadata in SQLite and generated files under `temp-results`, expose CRUD/read/download endpoints, then add frontend creation and shared-result views without adding dependencies.

**Tech Stack:** Rust, Actix Web, SQLx/SQLite, Tokio, React 18, TypeScript, Vite, Tailwind CSS

---

### Task 1: Boolean expression parser

**Files:**
- Create: `backend/src/log_expression.rs`
- Modify: `backend/src/lib.rs`

- [ ] Write failing unit tests for precedence, parentheses, quoted phrases, NOT, case-insensitive substring matching, and syntax error offsets.
- [ ] Run `cargo test log_expression` and confirm failures are caused by missing parser behavior.
- [ ] Implement tokenizer, recursive-descent parser, AST, and line evaluator.
- [ ] Run `cargo test log_expression` and confirm all parser tests pass.

### Task 2: Temporary result persistence and API

**Files:**
- Modify: `backend/src/db.rs`
- Create: `backend/src/routes/temp_results.rs`
- Modify: `backend/src/routes/mod.rs`
- Modify: `backend/tests/smoke.rs`

- [ ] Add a failing smoke test that creates a result from a file, verifies complete matching lines and source marker, reads paginated lines, downloads it, and deletes it.
- [ ] Run the exact smoke test and confirm the create endpoint is missing.
- [ ] Add the `temp_results` schema and implement UUID file creation, seven-day expiry/renewal, metadata, line pagination, download, and deletion.
- [ ] Run the exact smoke test and confirm it passes.

### Task 3: Frontend API and temporary result view

**Files:**
- Modify: `frontend/src/api/types.ts`
- Modify: `frontend/src/api/client.ts`
- Modify: `frontend/src/App.tsx`
- Modify: `frontend/src/features/files/FilesView.tsx`
- Create: `frontend/src/features/files/TempResultView.tsx`

- [ ] Add typed client methods for create, metadata, lines, download, and delete.
- [ ] Add “生成临时文件” beside current-file search and submit the active bundle/file ID plus expression.
- [ ] Navigate to `/temp-results/:id` and render expression, source, expiry, share-link copy, paginated content, download, delete, and another expression input.
- [ ] Run `npm run build` and confirm TypeScript/Vite succeeds.

### Task 4: Full verification

**Files:**
- Verify all modified files.

- [ ] Run `cargo fmt --check`.
- [ ] Run `cargo test` and confirm zero failures.
- [ ] Run `npm run build` and confirm success.
- [ ] Run `git diff --check` and review the complete diff for scope and lifecycle correctness.
