# Issue #95 Upload and Bundle Polling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Decouple HTTP upload availability from backend Bundle processing and continuously refresh active Bundles.

**Architecture:** `useUploadTask` owns only the current browser request, while `useIssueBundles` owns one recursive polling chain for all server-side Bundles. Presentation components consume Bundle status instead of a single local task.

**Tech Stack:** React 18, TypeScript, Vitest, Testing Library, Vite

---

### Task 1: Lock upload and polling behavior with failing tests

**Files:**
- Create: `frontend/tests/upload-polling.behavior.test.tsx`

- [ ] Add a hook test that completes upload A with a `PROCESSING` response and asserts that upload B can start immediately.
- [ ] Run `npm test -- --run tests/upload-polling.behavior.test.tsx` and confirm the upload assertion fails because `activeTask` keeps the hook disabled.
- [ ] Add fake-timer Bundle hook tests for `PENDING`, repeated `PROCESSING`, multiple active Bundles, terminal stopping, transient failure retry, and Issue switching.
- [ ] Run the focused test and confirm the one-shot polling implementation fails the continuous-polling assertions.

### Task 2: Restrict `useUploadTask` to the HTTP request lifecycle

**Files:**
- Modify: `frontend/src/features/files/hooks/useUploadTask.ts`

- [ ] Remove task polling, `activeTask`, and the active-Bundle input from the hook.
- [ ] Define `uploadDisabled` as `!currentIssueCode || uploading`.
- [ ] Release `uploadingRef` and dispatch the success state as soon as `uploadLogs` resolves, before awaiting Bundle and Issue refreshes.
- [ ] Preserve request-error behavior and the in-flight guard.
- [ ] Run the focused upload test and confirm upload B starts after A is accepted.

### Task 3: Add one resilient Bundle polling chain

**Files:**
- Modify: `frontend/src/features/files/hooks/useIssueBundles.ts`

- [ ] Derive active state from both `PENDING` and `PROCESSING` Bundles.
- [ ] Preserve the last successful Bundle snapshot on ordinary refresh failures.
- [ ] Add a recursive three-second timer with one timer slot and cancellation on terminal state, Issue change, or unmount.
- [ ] Delay work while hidden and refresh immediately when visibility returns.
- [ ] Run the focused polling tests and confirm all timer, retry, and cancellation cases pass.

### Task 4: Remove single-task background presentation

**Files:**
- Modify: `frontend/src/features/files/HomeView.tsx`
- Modify: `frontend/src/features/files/components/UploadPanel.tsx`
- Modify: `frontend/src/features/files/homeRows.ts`

- [ ] Stop passing active Bundle state and task lookup callbacks to `useUploadTask`.
- [ ] Remove `activeTask` from `UploadPanel`; show processing text only for the current HTTP upload.
- [ ] Build backend rows only from Bundle list state and keep optimistic rows only for current/failed HTTP uploads.
- [ ] Remove deletion-reset coupling to the latest upload task.
- [ ] Run the focused tests and TypeScript check.

### Task 5: Verify and publish

**Files:**
- Review all changed files.

- [ ] Run `npm test` in `frontend` and confirm the complete suite passes.
- [ ] Run `npm run lint` and `npm run build` in `frontend` and confirm both pass.
- [ ] Run `git diff --check` and inspect the complete diff against `origin/main`.
- [ ] Commit the scoped changes, push `agent/issue-95-upload-polling`, and open a draft PR targeting `main` with `Closes #95` and validation results.
