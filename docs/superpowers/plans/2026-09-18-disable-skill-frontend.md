# Frontend Skill Feature Disablement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Hide all AI Skill-related frontend entry points while preserving the backend APIs, data, and background services.

**Architecture:** Keep Skill feature components, API methods, and types intact for future restoration. Remove only the three mounted UI entry points and the imports/state/callbacks that exist solely to support those mounts: account Skill management, Issue Skill diagnosis, and admin AI Provider settings.

**Tech Stack:** React, TypeScript, Vitest, Testing Library, Vite.

---

## Files and responsibilities

- Modify `frontend/src/features/auth/AccountPage.tsx`: keep account security and remove the Skills tab/page.
- Modify `frontend/src/features/files/FilesView.tsx`: keep file browsing and remove the Issue Skill runner and evidence reveal bridge.
- Modify `frontend/src/features/admin/AdminPage.tsx`: keep all other admin settings and remove the AI Provider panel.
- Modify `frontend/tests/account-skills.behavior.test.tsx`: assert the account page no longer exposes Skill management.
- Keep `frontend/tests/issue-skill-runner.behavior.test.tsx` unchanged: it verifies the retained component still works for future restoration.
- Modify `frontend/tests/admin-guard.behavior.test.tsx`: assert the admin settings page no longer displays the AI Provider panel.

The following files remain unchanged: `frontend/src/features/skills/**`, `frontend/src/features/skill-runs/**`, `frontend/src/api/client.ts`, `frontend/src/api/types.ts`, all backend files, and database schema code.

### Task 1: Update frontend behavior tests first

**Files:**
- Modify: `frontend/tests/account-skills.behavior.test.tsx`
- Modify: `frontend/tests/admin-guard.behavior.test.tsx`

- [ ] **Step 1: Change the account test expectation**

Replace the existing account Skill-management assertion with an assertion that the account page does not render the `我的 Skills` tab or the mocked `skill management` content, while the `账户安全` tab remains visible.

- [ ] **Step 2: Update the admin provider expectation**

Add an assertion to the existing authenticated admin settings test that `AI Provider` is absent while unrelated admin settings remain available. Keep `ai-provider-settings.behavior.test.tsx` unchanged because it verifies the retained panel implementation for future restoration.

- [ ] **Step 3: Run the focused tests and verify the expected RED state**

Run:

```bash
cd frontend && npx vitest run tests/account-skills.behavior.test.tsx tests/admin-guard.behavior.test.tsx
```

Expected: the updated account and admin visibility assertions fail because those two entry points are still mounted. The Issue entry point will be verified by source diff and the full build because its existing test file exercises the retained runner component directly, not the larger file-view container.

### Task 2: Hide the three frontend entry points

**Files:**
- Modify: `frontend/src/features/auth/AccountPage.tsx:1-50`
- Modify: `frontend/src/features/files/FilesView.tsx:1-43,1290-1312`
- Modify: `frontend/src/features/admin/AdminPage.tsx:1-14,630-632`

- [ ] **Step 1: Remove account Skill UI dependencies**

In `AccountPage.tsx`, remove the `SkillsPage` import, the `section` state, the Skills tab button, the conditional `max-w-5xl` layout branch, and the conditional `<SkillsPage />` branch. Render the existing account-security content directly in the account card.

- [ ] **Step 2: Remove Issue Skill UI dependencies**

In `FilesView.tsx`, remove the `IssueSkillRunner` and `SkillEvidence` imports, remove `revealSkillEvidence`, and remove the top-level conditional block that renders `IssueSkillRunner` or the guest Skill message. Leave the main file browser section as the first content in the view.

- [ ] **Step 3: Remove the admin AI Provider mount**

In `AdminPage.tsx`, remove the `AiProviderSettingsPanel` import and the `<AiProviderSettingsPanel />` element. Leave the surrounding settings sections and `AdminGuard` unchanged.

- [ ] **Step 4: Run the focused tests and verify GREEN**

Run:

```bash
cd frontend && npx vitest run tests/account-skills.behavior.test.tsx tests/admin-guard.behavior.test.tsx
```

Expected: PASS, with account Skill management and admin AI Provider hidden. The retained Skill runner and AI Provider component tests continue to pass in the full suite.

### Task 3: Verify no backend or shared frontend contract was changed

**Files:**
- No production files beyond Task 2.

- [ ] **Step 1: Search the diff for accidental backend/API changes**

Run:

```bash
git diff -- backend frontend/src/api frontend/src/features/skills frontend/src/features/skill-runs
```

Expected: only the three page files and the two focused parent-page test files are changed; Skill API methods/types and Skill feature components remain intact.

- [ ] **Step 2: Run the full frontend test suite**

Run:

```bash
cd frontend && npm test -- --run
```

Expected: all frontend tests pass.

- [ ] **Step 3: Run frontend type checking and production build**

Run:

```bash
cd frontend && npm run lint && npm run build
```

Expected: TypeScript linting and the production build both complete successfully.

- [ ] **Step 4: Review final status**

Run:

```bash
git status --short
git diff --check
```

Expected: only the intended frontend files and the implementation plan/design documentation are present, with no whitespace errors.
