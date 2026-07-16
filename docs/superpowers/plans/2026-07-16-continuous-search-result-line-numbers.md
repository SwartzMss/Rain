# Continuous Search Result Line Numbers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Display search-result positions as continuous paginated line numbers while preserving original source lines for right-click navigation.

**Architecture:** Change only the visible line-number expression in `SearchResultViewer`; source metadata and navigation remain untouched. Extend the existing SSR regression test to distinguish first-page and later-page display numbers from `data-source-line`.

**Tech Stack:** React 18, TypeScript, Vite SSR test harness, Node.js assertions

---

### Task 1: Add failing continuous-number regression coverage

**Files:**
- Modify: `frontend/tests/search-hit-source.mjs`

- [ ] **Step 1: Change the first-page assertion to require result position 1**

Keep the existing hit with `line_number: 17` and `data-source-line="17"`, but replace the visible `18` assertion with:

```js
assert.match(markup, />1<\/span>/);
assert.doesNotMatch(markup, />18<\/span>/);
```

- [ ] **Step 2: Add a later-page rendering assertion**

Render the same `SearchResultViewer` props with `activeViewerTab.from: 1000` and assert:

```js
assert.match(secondPageMarkup, /data-source-line="17"/);
assert.match(secondPageMarkup, />1001<\/span>/);
assert.doesNotMatch(secondPageMarkup, />18<\/span>/);
```

- [ ] **Step 3: Run the focused test and verify it fails**

Run: `node tests/search-hit-source.mjs` from `frontend/`

Expected: FAIL because the visible number is still the source line plus one.

- [ ] **Step 4: Commit the failing regression test**

```bash
git add frontend/tests/search-hit-source.mjs
git commit -m "test: require continuous search result line numbers"
```

### Task 2: Separate visible and source line numbers

**Files:**
- Modify: `frontend/src/features/files/components/SearchResultViewer.tsx`

- [ ] **Step 1: Change only the visible number expression**

Replace:

```tsx
{(source?.line ?? activeViewerTab.from + index) + 1}
```

with:

```tsx
{activeViewerTab.from + index + 1}
```

Keep `data-source-line={source?.line ?? undefined}`, the context-menu hit, and `onOpenSource` unchanged.

- [ ] **Step 2: Run the focused test and verify it passes**

Run: `node tests/search-hit-source.mjs` from `frontend/`

Expected: PASS and print `search hit source tests passed`.

- [ ] **Step 3: Commit the implementation**

```bash
git add frontend/src/features/files/components/SearchResultViewer.tsx
git commit -m "fix: number search results continuously"
```

### Task 3: Verify the frontend

**Files:**
- Verify: `frontend/tests/search-hit-source.mjs`
- Verify: `frontend/src/features/files/components/SearchResultViewer.tsx`

- [ ] **Step 1: Run all frontend tests**

Run: `npm test` from `frontend/`

Expected: all six test scripts pass.

- [ ] **Step 2: Run TypeScript checking**

Run: `npm run lint` from `frontend/`

Expected: exit code 0 with no TypeScript errors.

- [ ] **Step 3: Build production assets**

Run: `npm run build` from `frontend/`

Expected: Vite completes successfully.

- [ ] **Step 4: Check the final repository state**

Run: `git diff --check && git status --short` from the repository root.

Expected: no whitespace errors and no uncommitted implementation changes.
