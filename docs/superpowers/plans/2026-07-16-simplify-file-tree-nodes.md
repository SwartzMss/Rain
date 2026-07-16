# Simplify File Tree Nodes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce every file-tree row to its disclosure arrow, type icon, and filename while exposing the complete filename on hover.

**Architecture:** Keep the existing recursive `FileTreeNode` data and expansion behavior unchanged, but remove presentation-only metadata from its row. Add a focused Vite SSR regression test that renders representative directory, archive, and file nodes and asserts the visible DOM contract.

**Tech Stack:** React 18, TypeScript, Vite SSR test harness, Node.js assertions, Tailwind CSS

---

### Task 1: Add a failing file-tree presentation test

**Files:**
- Create: `frontend/tests/file-tree-node.mjs`
- Modify: `frontend/package.json`

- [ ] **Step 1: Create the SSR regression test**

Create `frontend/tests/file-tree-node.mjs` using Vite's SSR loader and `renderToStaticMarkup`. Render `FileTreeNode` with a root archive named `very-long-diagnostic-bundle.zip` and a text child named `application-production.log`. Give the archive one child and the text file `mime_type: 'text/plain'`, `size_bytes: 4096`, and `preview_kind: 'text'`.

Assert the markup contains the full names, `title="very-long-diagnostic-bundle.zip"`, `aria-label="very-long-diagnostic-bundle.zip"`, `ZIP`, `TXT`, and the disclosure marker. Assert it does not contain `1 子节点`, `text/plain`, `4 KB`, `展开`, `收起`, or `暂无子节点`.

- [ ] **Step 2: Add the focused test to the frontend test script**

Change the script to:

```json
"test": "node tests/binary-file-info.mjs && node tests/search-tokens.mjs && node tests/filename-search.mjs && node tests/upload-failure.mjs && node tests/search-hit-source.mjs && node tests/file-tree-node.mjs"
```

- [ ] **Step 3: Run the focused test and verify it fails**

Run: `node tests/file-tree-node.mjs` from `frontend/`

Expected: FAIL because the existing node markup still includes child counts, MIME type, size, and expansion text, and lacks the filename title.

- [ ] **Step 4: Commit the failing test**

```bash
git add frontend/tests/file-tree-node.mjs frontend/package.json
git commit -m "test: define compact file tree rows"
```

### Task 2: Simplify file-tree row rendering

**Files:**
- Modify: `frontend/src/features/files/components/FileTreeNode.tsx`

- [ ] **Step 1: Remove obsolete metadata dependencies and calculations**

Replace the tree-model import with:

```ts
import { isExtractionFolder, type TreeNode } from '../treeModel';
```

Delete `rowMeta` and stop calling `formatSize`.

- [ ] **Step 2: Render only the disclosure arrow, icon, and titled filename**

Add `aria-label={node.name}` to the node button. Replace the filename wrapper and trailing metadata with:

```tsx
<span className="min-w-0 flex-1 truncate text-[13px] font-medium leading-4" title={node.name}>
  {node.name}
</span>
```

Keep the existing disclosure marker, type icon, selection styles, indentation, click handler, recursive child rendering, and border. When an expanded node has no children, render nothing instead of `暂无子节点`.

- [ ] **Step 3: Run the focused test and verify it passes**

Run: `node tests/file-tree-node.mjs` from `frontend/`

Expected: PASS and print `file tree node tests passed`.

- [ ] **Step 4: Commit the implementation**

```bash
git add frontend/src/features/files/components/FileTreeNode.tsx
git commit -m "feat: simplify file tree rows"
```

### Task 3: Verify the frontend

**Files:**
- Verify: `frontend/tests/file-tree-node.mjs`
- Verify: `frontend/src/features/files/components/FileTreeNode.tsx`

- [ ] **Step 1: Run all frontend tests**

Run: `npm test` from `frontend/`

Expected: all six test scripts pass.

- [ ] **Step 2: Run TypeScript checking**

Run: `npm run lint` from `frontend/`

Expected: exit code 0 with no TypeScript errors.

- [ ] **Step 3: Build the production frontend**

Run: `npm run build` from `frontend/`

Expected: Vite finishes successfully with production assets in `frontend/dist/`.

- [ ] **Step 4: Check repository cleanliness**

Run: `git diff --check && git status --short` from the repository root.

Expected: no whitespace errors and no uncommitted implementation changes.
