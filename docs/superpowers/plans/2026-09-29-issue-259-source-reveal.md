# Issue #259 Source Reveal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make “在原文件中打开” materialize and render the complete file-tree path, including targets outside the initially loaded root page.

**Architecture:** Keep reveal in `FilesView`, but centralize child attachment and ordering in `treeModel`. Reveal will attach fetched nodes to already-known parents, explicitly attach the final top-level node to the synthetic bundle root, and preserve pagination metadata. Existing archive flattening remains the visible-path rule.

**Tech Stack:** React 18, TypeScript, Vitest, Testing Library.

---

### Task 1: Add a failing tree-path attachment test

**Files:**
- Modify: `frontend/tests/file-tree-loading.behavior.test.ts`
- Modify: `frontend/src/features/files/treeModel.ts`

- [ ] **Step 1: Write the failing test**

Add a test that constructs a synthetic root with a loaded first-page child and an unseen top-level target, then asserts that attaching the target:

```ts
it('attaches a revealed top-level node to the synthetic root without closing pagination', () => {
  const root = toTreeNode('bundle', {
    id: 'root', parent_id: null, name: 'bundle_root', path: '/', is_dir: true
  }, null);
  const firstPageChild = toTreeNode('bundle', {
    id: 1, parent_id: null, name: 'first.log', path: '/first.log', is_dir: false
  }, root.id);
  const target = toTreeNode('bundle', {
    id: 101, parent_id: null, name: 'target.log', path: '/target.log', is_dir: false
  }, null);
  const rootWithPage = {
    ...root,
    childrenIds: [firstPageChild.id],
    hasLoadedChildren: true,
    hasMoreChildren: true,
    childrenCursor: 'cursor-1'
  };

  const result = attachTreeChild(rootWithPage, target, [firstPageChild]);

  expect(result.child.parentId).toBe(root.id);
  expect(result.parent.childrenIds).toEqual([firstPageChild.id, target.id]);
  expect(result.parent.hasMoreChildren).toBe(true);
  expect(result.parent.childrenCursor).toBe('cursor-1');
});
```

- [ ] **Step 2: Run the focused test to verify it fails**

Run: `npm exec vitest run tests/file-tree-loading.behavior.test.ts`

Expected: FAIL because `attachTreeChild` does not exist yet.

### Task 2: Implement ordered child attachment

**Files:**
- Modify: `frontend/src/features/files/treeModel.ts`
- Test: `frontend/tests/file-tree-loading.behavior.test.ts`

- [ ] **Step 1: Implement the minimal helper**

Add `attachTreeChild(parent, child, knownChildren)` that normalizes a synthetic-root child’s `parentId`, inserts the child according to the existing directory/name/id ordering, deduplicates IDs, and leaves `hasMoreChildren` and `childrenCursor` unchanged.

- [ ] **Step 2: Run the focused test**

Run: `npm exec vitest run tests/file-tree-loading.behavior.test.ts`

Expected: PASS.

### Task 3: Use materialized attachment in source reveal

**Files:**
- Modify: `frontend/src/features/files/FilesView.tsx:1107-1188`
- Test: `frontend/tests/search-hit-source.mjs`

- [ ] **Step 1: Replace reveal-time sibling pagination with explicit attachment**

Use the helper whenever a loaded parent does not list the active node. Keep the existing archive flatten branch. After ancestor traversal reaches a node whose `parentId` is null, load or reuse `${bundleHash}:root` and attach that node to the synthetic root. Update both `knownNodes` and React state for every attached parent/child pair.

- [ ] **Step 2: Update structural expectations**

Change the source-reveal static test to require synthetic-root attachment and no longer require `loadMoreNode(parent)` for reveal.

- [ ] **Step 3: Run the focused tests**

Run: `npm exec vitest run tests/file-tree-loading.behavior.test.ts tests/files-view-search.behavior.test.tsx`

Expected: PASS.

### Task 4: Verify the complete frontend

**Files:**
- No additional files.

- [ ] **Step 1: Run the full frontend test suite**

Run: `npm test`

Expected: 0 failures, including Vitest and all standalone behavior tests.

- [ ] **Step 2: Run the frontend type check**

Run: `npm run lint`

Expected: TypeScript exits successfully with no errors.

- [ ] **Step 3: Review the final diff**

Run: `git diff --check && git diff --stat`

Expected: no whitespace errors; only the Issue #259 spec/plan, tree model test/helper, reveal integration, and updated structural test are changed.
