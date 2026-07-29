# Refresh Failed Upload Deletion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove a failed upload row from the Home file list after its bundle is successfully deleted.

**Architecture:** Add a small pure decision helper that identifies whether a deleted bundle owns the retained upload task. The Home deletion flow will reset upload state only for that matching task, after the delete API succeeds and before refreshed lists are rendered.

**Tech Stack:** React 18, TypeScript, Vite SSR test harness, Node assertions

---

### Task 1: Test the upload-reset decision

**Files:**
- Create: `frontend/src/features/files/uploadDeletion.ts`
- Create: `frontend/tests/upload-deletion.mjs`
- Modify: `frontend/package.json`

- [ ] **Step 1: Write the failing regression test**

Create `frontend/tests/upload-deletion.mjs`:

```js
import assert from 'node:assert/strict';
import { createServer } from 'vite';

const server = await createServer({
  appType: 'custom',
  logLevel: 'silent',
  server: { middlewareMode: true }
});

try {
  const { shouldResetUploadAfterBundleDeletion } = await server.ssrLoadModule(
    '/src/features/files/uploadDeletion.ts'
  );

  assert.equal(shouldResetUploadAfterBundleDeletion('failed-bundle', 'failed-bundle'), true);
  assert.equal(shouldResetUploadAfterBundleDeletion('other-bundle', 'failed-bundle'), false);
  assert.equal(shouldResetUploadAfterBundleDeletion('failed-bundle', undefined), false);
} finally {
  await server.close();
}

console.log('upload deletion tests passed');
```

Append `node tests/upload-deletion.mjs` to the frontend `test` script in `frontend/package.json`.

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
cd frontend
node tests/upload-deletion.mjs
```

Expected: FAIL because `/src/features/files/uploadDeletion.ts` does not exist.

- [ ] **Step 3: Add the minimal decision helper**

Create `frontend/src/features/files/uploadDeletion.ts`:

```ts
export const shouldResetUploadAfterBundleDeletion = (
  deletedBundleHash: string,
  uploadTaskBundleHash?: string
): boolean => deletedBundleHash === uploadTaskBundleHash;
```

- [ ] **Step 4: Run the focused test and verify GREEN**

Run:

```bash
cd frontend
node tests/upload-deletion.mjs
```

Expected: PASS with `upload deletion tests passed`.

### Task 2: Clear matching failed-upload state after deletion

**Files:**
- Modify: `frontend/src/features/files/HomeView.tsx`
- Test: `frontend/tests/upload-deletion.mjs`

- [ ] **Step 1: Extend the regression test with source integration assertions**

Add these imports and assertions to `frontend/tests/upload-deletion.mjs`:

```js
import { readFile } from 'node:fs/promises';

const homeView = await readFile(
  new URL('../src/features/files/HomeView.tsx', import.meta.url),
  'utf8'
);
assert.match(homeView, /shouldResetUploadAfterBundleDeletion/);
assert.match(homeView, /upload\.resetSelection\(\)/);
```

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
cd frontend
node tests/upload-deletion.mjs
```

Expected: FAIL because `HomeView.tsx` does not yet use the decision helper or reset the upload state after bundle deletion.

- [ ] **Step 3: Integrate reset into successful bundle deletion**

Import the helper in `frontend/src/features/files/HomeView.tsx`:

```ts
import { shouldResetUploadAfterBundleDeletion } from './uploadDeletion';
```

After `rainApi.deleteBundle` succeeds, reset only the matching retained upload task:

```ts
await rainApi.deleteBundle(issues.currentIssueCode, row.bundleHash);
if (
  shouldResetUploadAfterBundleDeletion(
    row.bundleHash,
    upload.uploadTask?.bundle_hash
  )
) {
  upload.resetSelection();
}
await bundles.loadBundles(issues.currentIssueCode);
```

Add `upload` to the `deleteRow` callback dependency list.

- [ ] **Step 4: Run focused and full frontend verification**

Run:

```bash
cd frontend
node tests/upload-deletion.mjs
npm test
npm run lint
npm run build
```

Expected: all commands exit successfully; the focused test prints `upload deletion tests passed`.

- [ ] **Step 5: Check the patch and commit**

Run:

```bash
git diff --check
git status --short
```

Expected: no whitespace errors and only the planned implementation/test files are modified.

Commit:

```bash
git add frontend/package.json frontend/src/features/files/HomeView.tsx frontend/src/features/files/uploadDeletion.ts frontend/tests/upload-deletion.mjs docs/superpowers/plans/2026-07-29-refresh-failed-upload-deletion.md
git commit -m "fix: clear deleted failed upload state"
```
