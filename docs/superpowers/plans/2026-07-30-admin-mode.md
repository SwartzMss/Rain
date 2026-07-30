# Administrator Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement issue #40's two-level USER/ADMIN authorization, bootstrap administrator, global write protection, user administration, audit trail, and administrator UI.

**Architecture:** Keep role and status as shared Rust domain enums loaded on every authenticated request. Bootstrap and all administrator mutations are transactional repository operations; Actix extractors enforce server-side access, while React derives presentation from the authenticated user's role. SQLite keyset pagination and `BEGIN IMMEDIATE` protect bounded queries and the last-active-admin invariant.

**Tech Stack:** Rust, Actix Web, SQLx/SQLite, Argon2, React, TypeScript, Vite, Node test runner.

---

### Task 1: Schema, domain types, and bootstrap administrator

**Files:**
- Create: `backend/src/auth/role.rs`
- Create: `backend/src/auth/status.rs`
- Create: `backend/src/repositories/admin_audit.rs`
- Create: `backend/src/repositories/bootstrap_admin.rs`
- Modify: `backend/src/auth/mod.rs`
- Modify: `backend/src/config.rs`
- Modify: `backend/src/db.rs`
- Modify: `backend/src/main.rs`
- Modify: `backend/src/repositories/mod.rs`
- Modify: `backend/Cargo.toml`
- Test: `backend/tests/admin.rs`

- [ ] Write bootstrap tests proving valid configuration creates one ACTIVE ADMIN and one `ADMIN_BOOTSTRAPPED` audit row, empty/weak/conflicting credentials fail without leaking the password, existing ACTIVE ADMIN is untouched, and reset recreates from current configuration.
- [ ] Run `cargo test --test admin bootstrap -- --nocapture` from `backend`; verify the new tests fail because bootstrap/domain support is absent.
- [ ] Add `UserRole`/`UserStatus` SQLx+Serde enums, CHECK-constrained `users` columns, `admin_audit_logs` and indexes, secret bootstrap configuration, and transactional `bootstrap_admin` using the existing username/password validation and Argon2 service.
- [ ] Invoke bootstrap after `prepare_schema` and before recovery/server startup; run the focused tests until green.
- [ ] Run `cargo fmt --check` and `cargo test --test admin bootstrap`; commit as `feat: bootstrap administrator accounts`.

### Task 2: Live authorization and global write protection

**Files:**
- Modify: `backend/src/auth/extractor.rs`
- Modify: `backend/src/auth/mod.rs`
- Modify: `backend/src/models/auth.rs`
- Modify: `backend/src/repositories/sessions.rs`
- Modify: `backend/src/routes/issues.rs`
- Modify: `backend/src/routes/uploads.rs`
- Modify: `backend/src/routes/files.rs`
- Modify: `backend/src/routes/temp_results.rs`
- Modify: `backend/src/error.rs`
- Test: `backend/tests/admin.rs`
- Test: `backend/tests/auth.rs`

- [ ] Add failing tests for `RequireAdmin` ordering (missing session 401, disabled ADMIN 403 `ACCOUNT_DISABLED`, ACTIVE USER 403 `ADMIN_REQUIRED`, ACTIVE ADMIN accepted) and for every issue/upload/delete write endpoint across GUEST/USER/ADMIN.
- [ ] Run `cargo test --test admin authorization -- --nocapture`; verify failures are caused by the missing extractor/write guards.
- [ ] Extend `AuthenticatedUser` and public auth responses with `role`, decode role/status strictly during every session resolution, implement `RequireAdmin`, and replace `RequireUser`/unguarded write handlers with `RequireAdmin` while leaving temp-result creation/preview public.
- [ ] Run focused admin/auth tests until green, then `cargo test`; commit as `feat: enforce administrator write access`.

### Task 3: Administrator APIs, audit, and concurrency invariant

**Files:**
- Create: `backend/src/models/admin.rs`
- Create: `backend/src/repositories/admin_users.rs`
- Create: `backend/src/routes/admin.rs`
- Modify: `backend/src/repositories/admin_audit.rs`
- Modify: `backend/src/repositories/mod.rs`
- Modify: `backend/src/routes/mod.rs`
- Modify: `backend/src/error.rs`
- Test: `backend/tests/admin.rs`

- [ ] Add failing integration tests for bounded/filterable cursor pagination, promote/demote, disable/enable, atomic session revocation, self-protection, last ACTIVE ADMIN protection, concurrent mutual demotion/disable, audit filtering/pagination, non-admin rejection, and rollback when audit insertion fails.
- [ ] Run `cargo test --test admin api -- --nocapture`; verify failures identify the missing `/api/admin/*` services.
- [ ] Implement typed request/response models and keyset cursors; implement list, role, status, revoke-sessions, and audit endpoints behind `RequireAdmin`.
- [ ] Use explicit `BEGIN IMMEDIATE` repository transactions to reload targets, count ACTIVE ADMIN users, mutate state/revoke sessions, and append audit records atomically; map the issue's stable error codes and statuses.
- [ ] Run focused tests, `cargo test`, `cargo fmt --check`, and Clippy with warnings denied; commit as `feat: add audited administrator user management`.

### Task 4: Frontend permissions and administrator page

**Files:**
- Create: `frontend/src/auth/RequireAdminRoute.tsx`
- Create: `frontend/src/features/admin/AdminPage.tsx`
- Create: `frontend/src/features/admin/UserTable.tsx`
- Create: `frontend/src/features/admin/AuditLogTable.tsx`
- Create: `frontend/src/features/admin/adminApi.ts`
- Create: `frontend/tests/admin-permissions.mjs`
- Modify: `frontend/src/api/types.ts`
- Modify: `frontend/src/api/client.ts`
- Modify: `frontend/src/auth/AuthContext.tsx`
- Modify: `frontend/src/App.tsx`
- Modify: `frontend/src/features/files/FilesView.tsx`
- Modify: `frontend/src/features/files/HomeView.tsx`
- Modify: `frontend/src/features/files/TempResultView.tsx`
- Modify: `README.md`
- Modify: `backend/.env.example`
- Modify: `doc/DB.md`

- [ ] Add failing frontend tests proving only ADMIN sees global write actions, USER is labeled read-only and receives an admin 403 view, admin mutations refresh authoritative state after success/failure, and user/audit cursor filters work.
- [ ] Run `npm test -- admin-permissions`; verify the tests fail for missing role helpers and admin UI.
- [ ] Add `UserRole`, centralized `isAdmin`, admin API client/types, guarded `/admin` route, searchable/filterable cursor tables, confirmations, and explicit mutation feedback; gate every existing global write control through `isAdmin`.
- [ ] Document bootstrap environment variables, fresh-install deployment, final schema, permissions, and audit behavior.
- [ ] Run `npm test`, `npm run check`, backend tests/Clippy, `git diff --check`, and verify the issue #40 Definition of Done item-by-item; commit as `feat: add administrator management interface`.

