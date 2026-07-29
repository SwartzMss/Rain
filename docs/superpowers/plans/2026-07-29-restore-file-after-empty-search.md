# Restore File Content After Empty Search Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore the active file's original content whenever its search tokens and draft text have both been removed.

**Architecture:** Add a small pure predicate beside the file-search feature so the empty-condition rule can be tested independently. `FilesView` will observe the two editor state values and call the existing `clearFileSearch` state reset when a search-result state is active and that predicate reports a fully empty condition.

**Tech Stack:** React 18, TypeScript, Vite SSR test loader, Node.js assertions

---

### Task 1: Define and Test the Empty File-Search Condition

**Files:**
- Create: `frontend/src/features/files/fileSearchState.ts`
- Create: `frontend/tests/file-search-state.mjs`
- Modify: `frontend/package.json`

- [ ] **Step 1: Write the failing predicate test**

Create `frontend/tests/file-search-state.mjs`:

```js
import assert from 'node:assert/strict';
import { createServer } from 'vite';

const server = await createServer({
  appType: 'custom',
  logLevel: 'silent',
  server: { middlewareMode: true }
});

try {
  const { isFileSearchConditionEmpty } = await server.ssrLoadModule(
    '/src/features/files/fileSearchState.ts'
  );

  assert.equal(isFileSearchConditionEmpty([], ''), true);
  assert.equal(
    isFileSearchConditionEmpty([{ kind: 'term', value: 'ERROR' }], ''),
    false
  );
  assert.equal(isFileSearchConditionEmpty([], 'ERROR'), false);
} finally {
  await server.close();
}

console.log('file search state tests passed');
```

Append `node tests/file-search-state.mjs` to the frontend `test` script in
`frontend/package.json`.

- [ ] **Step 2: Run the test to verify it fails**

Run:

```bash
cd frontend
node tests/file-search-state.mjs
```

Expected: FAIL because `/src/features/files/fileSearchState.ts` does not exist.

- [ ] **Step 3: Implement the predicate**

Create `frontend/src/features/files/fileSearchState.ts`:

```ts
import type { SearchToken } from './searchTokens';

export function isFileSearchConditionEmpty(tokens: SearchToken[], draft: string) {
  return tokens.length === 0 && draft.length === 0;
}
```

- [ ] **Step 4: Run the focused test to verify it passes**

Run:

```bash
cd frontend
node tests/file-search-state.mjs
```

Expected: PASS with `file search state tests passed`.

### Task 2: Reset Search Results When the Condition Becomes Empty

**Files:**
- Modify: `frontend/src/features/files/FilesView.tsx`
- Modify: `frontend/tests/file-search-state.mjs`

- [ ] **Step 1: Add a failing integration guard**

Extend `frontend/tests/file-search-state.mjs` to read `FilesView.tsx` and assert
that the active-file search uses the predicate in an effect:

```js
import { readFile } from 'node:fs/promises';

const filesView = await readFile(
  new URL('../src/features/files/FilesView.tsx', import.meta.url),
  'utf8'
);
assert.match(
  filesView,
  /useEffect\(\(\) => \{\s*if \(fileSearchExecuted && isFileSearchConditionEmpty\(fileSearchTokens, fileSearchDraft\)\) \{\s*clearFileSearch\(\);\s*\}\s*\}, \[clearFileSearch, fileSearchDraft, fileSearchExecuted, fileSearchTokens\]\);/
);
```

- [ ] **Step 2: Run the test to verify it fails**

Run:

```bash
cd frontend
node tests/file-search-state.mjs
```

Expected: FAIL because `FilesView` does not yet reset result state when the
editor condition becomes empty.

- [ ] **Step 3: Add the state synchronization effect**

Import the predicate in `frontend/src/features/files/FilesView.tsx`:

```ts
import { isFileSearchConditionEmpty } from './fileSearchState';
```

After the `clearFileSearch` callback, add:

```ts
useEffect(() => {
  if (fileSearchExecuted && isFileSearchConditionEmpty(fileSearchTokens, fileSearchDraft)) {
    clearFileSearch();
  }
}, [clearFileSearch, fileSearchDraft, fileSearchExecuted, fileSearchTokens]);
```

This leaves searches intact while either tokens or draft text remain and routes
an active search whose condition becomes completely empty through the existing
reset operation. The `fileSearchExecuted` guard prevents the idle initial state
from repeatedly resetting itself.

- [ ] **Step 4: Run the focused test to verify it passes**

Run:

```bash
cd frontend
node tests/file-search-state.mjs
```

Expected: PASS with `file search state tests passed`.

### Task 3: Verify the Frontend

**Files:**
- Verify: `frontend/src/features/files/fileSearchState.ts`
- Verify: `frontend/src/features/files/FilesView.tsx`
- Verify: `frontend/tests/file-search-state.mjs`
- Verify: `frontend/package.json`

- [ ] **Step 1: Run all frontend tests**

Run:

```bash
cd frontend
npm test
```

Expected: all test scripts pass, including `file search state tests passed`.

- [ ] **Step 2: Run TypeScript validation**

Run:

```bash
cd frontend
npm run lint
```

Expected: exit code 0 with no TypeScript errors.

- [ ] **Step 3: Build the production frontend**

Run:

```bash
cd frontend
npm run build
```

Expected: TypeScript compilation and Vite production build both complete
successfully.

- [ ] **Step 4: Check the final diff**

Run:

```bash
git diff --check
git status --short
```

Expected: no whitespace errors; only the planned implementation, test, package,
and plan files are changed.
