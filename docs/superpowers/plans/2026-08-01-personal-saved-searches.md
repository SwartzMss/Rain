# Personal Saved Searches Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make saved searches personal to each user, remove Issue scope and manual sorting, and preserve pinning with recent-update ordering.

**Architecture:** Keep the existing saved-search table shape for compatibility, but normalize all new and existing records to `GLOBAL` with a null scope key. Remove scope and sort controls from the frontend and make list APIs return all of the current user's saved searches.

**Tech Stack:** React, TypeScript, Vite tests, Rust, Actix Web, SQLx SQLite.

---

### Task 1: Lock the new API and UI behavior with failing tests

**Files:**
- Modify: `frontend/tests/auth-state.mjs`
- Modify: `frontend/tests/pending-saved-search.mjs`
- Modify: `backend/tests/auth.rs`

- [ ] Assert saved-search payloads no longer expose scope or sort fields, pending saved searches no longer depend on an Issue, and backend accepts legacy scope input while returning global scope.
- [ ] Run focused frontend and backend tests and verify they fail against the current scope-aware behavior.

### Task 2: Simplify frontend saved-search state and dialogs

**Files:**
- Modify: `frontend/src/api/types.ts`
- Modify: `frontend/src/api/client.ts`
- Modify: `frontend/src/features/files/pendingSavedSearch.ts`
- Modify: `frontend/src/features/files/FilesView.tsx`

- [ ] Remove scope and sort fields from frontend payload construction, pending restoration, create dialog, and edit dialog.
- [ ] Fetch the current user's complete saved-search list without an Issue query.
- [ ] Order the list by pinned first and updated time from the backend; preserve pin toggling/editing/deleting.
- [ ] Run focused frontend tests and verify they pass.

### Task 3: Make backend saved searches personal and migrate legacy rows

**Files:**
- Modify: `backend/src/models/saved_searches.rs`
- Modify: `backend/src/routes/saved_searches.rs`
- Modify: `backend/src/repositories/saved_searches.rs`
- Modify: `backend/src/db.rs`
- Modify: `backend/tests/auth.rs`

- [ ] Ignore legacy scope selection on create/update and persist `GLOBAL` plus null scope key.
- [ ] Remove Issue filtering from list queries while retaining backward-compatible request parsing.
- [ ] Normalize existing `ISSUE` rows during schema preparation so Issue deletion cannot hide saved searches.
- [ ] Verify create, update, list, and legacy-row migration behavior with integration tests.

### Task 4: Verify and publish

- [ ] Run frontend tests, lint, build, backend format check, backend tests, and diff checks.
- [ ] Commit the implementation and push the current branch.
