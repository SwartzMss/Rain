# Administrator and User Domain Separation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Align Issue #43 so ADMIN is restricted to the management backend while USER owns normal log/Issue write workflows.

**Architecture:** Add a backend extractor that accepts only active USER accounts for business writes, while retaining `RequireAdmin` for `/api/admin/**`. Split the React admin surface into `/admin/users` and `/admin/audit-logs`, and derive business write controls from `USER` rather than `ADMIN`.

**Tech Stack:** Rust, Actix Web, SQLx/SQLite, React, TypeScript, Node test runner.

---

### Task 1: Lock down the role boundary with regression tests

**Files:**
- Modify: `backend/src/auth/extractor.rs`
- Modify: `backend/tests/admin.rs`
- Modify: `frontend/tests/admin-permissions.mjs`

- [x] Add an Actix extractor test proving an active ADMIN receives `403 BUSINESS_USER_REQUIRED` from a business-user-only route.
- [x] Add a frontend static test proving business write controls are based on `isUser`, while admin navigation points to `/admin/users`.
- [x] Run the focused backend and frontend tests and confirm they fail for the missing extractor/helper.

### Task 2: Enforce USER-only business writes

**Files:**
- Modify: `backend/src/auth/extractor.rs`
- Modify: `backend/src/routes/issues.rs`
- Modify: `backend/src/routes/uploads.rs`
- Modify: `backend/src/routes/files.rs`
- Modify: `backend/src/routes/temp_results.rs`
- Modify: `frontend/src/auth/permissions.ts`
- Modify: `frontend/src/features/files/HomeView.tsx`
- Modify: `frontend/src/features/files/TempResultView.tsx`

- [x] Implement `RequireBusinessUser`, preserving `401 AUTHENTICATION_REQUIRED`, `403 ACCOUNT_DISABLED`, and `403 BUSINESS_USER_REQUIRED` responses.
- [x] Replace ADMIN guards on Issue creation/deletion, uploads, file deletion, saved-search mutation, and temp-result deletion with `RequireBusinessUser`.
- [x] Add `isUser` and use it for all normal business write controls; keep guest read-only behavior and admin-only management controls.
- [x] Run backend tests, frontend tests, typecheck, and formatting.

### Task 3: Split the administrator information architecture

**Files:**
- Modify: `frontend/src/App.tsx`
- Modify: `frontend/src/features/admin/AdminPage.tsx`
- Modify: `frontend/src/api/client.ts`
- Modify: `frontend/src/api/types.ts`
- Modify: `frontend/tests/admin-permissions.mjs`

- [x] Add independent admin users and audit-log views at `/admin/users` and `/admin/audit-logs`, with `/admin` redirecting to `/admin/users`.
- [x] Keep user and audit pagination state independent; add localized status/action labels, timestamps, empty/loading/error states, active-session disabling, and explicit operation buttons.
- [x] Remove business navigation from the admin landing path and ensure ordinary users cannot render admin pages.
- [x] Run the complete frontend suite and backend suite, then inspect `git diff --check`.

### Task 4: Verify the Issue #43 acceptance boundary

**Files:**
- Modify: `backend/tests/admin.rs`
- Modify: `backend/tests/auth.rs`
- Modify: `README.md` (only if behavior documentation is stale)

- [x] Verify ADMIN can use `/api/admin/**` but cannot perform Issue/upload/delete writes.
- [x] Verify active USER can perform the normal business writes but receives `403 ADMIN_REQUIRED` from `/api/admin/**`.
- [x] Run `cargo test`, `cargo fmt --check`, `npm test`, `npm run lint`, and the frontend production build.
- [x] Report any remaining Issue #43 acceptance items that are outside this focused patch.
