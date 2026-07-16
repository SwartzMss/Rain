# Search Hit Source Navigation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let users click or right-click a detailed-search hit to open its original file in a retained viewer tab, center and highlight the source line, or copy the source path.

**Architecture:** Keep source resolution in `BundleView`, where file-tree loading and viewer-tab state already live. Make `SearchResultViewer` emit hit-level actions and own a focused context-menu component. Add small pure helpers for source validation and viewport-safe menu placement so behavior is covered by the existing Vite/Node test harness.

**Tech Stack:** React 18, TypeScript, Tailwind CSS, Vite SSR test harness, Node `assert`.

---

## File Structure

- Create `frontend/src/features/files/searchHitSource.ts`: pure source validation, compound node-ID creation, and context-menu placement.
- Create `frontend/src/features/files/components/SearchHitContextMenu.tsx`: accessible two-action context menu with focus and dismissal behavior.
- Create `frontend/tests/search-hit-source.mjs`: helper and server-rendered component coverage.
- Modify `frontend/src/features/files/components/SearchResultViewer.tsx`: render actionable hit rows and emit open/copy actions.
- Modify `frontend/src/features/files/components/CodeLinesPane.tsx`: accept a target source line and apply a centered-scroll marker/highlight.
- Modify `frontend/src/features/files/FilesView.tsx`: resolve hit sources, invoke existing file-tab navigation, report feedback, and center the selected line.
- Modify `frontend/package.json`: include the new focused test in `npm test`.

### Task 1: Source Navigation Helpers

**Files:**
- Create: `frontend/src/features/files/searchHitSource.ts`
- Create: `frontend/tests/search-hit-source.mjs`
- Modify: `frontend/package.json`

- [ ] **Step 1: Write the failing helper tests**

Create `frontend/tests/search-hit-source.mjs` with:

```js
import assert from 'node:assert/strict';
import { createServer } from 'vite';

const server = await createServer({
  appType: 'custom',
  logLevel: 'silent',
  server: { middlewareMode: true }
});

try {
  const { getSearchHitSource, placeContextMenu } = await server.ssrLoadModule(
    '/src/features/files/searchHitSource.ts'
  );

  assert.deepEqual(
    getSearchHitSource({ bundle_hash: 'bundle-a', file_id: 42, path: '/bundle-a/app.log', snippet: 'ERROR', line_number: 17 }),
    { bundleHash: 'bundle-a', fileId: '42', nodeId: 'bundle-a:42', path: '/bundle-a/app.log', line: 17 }
  );
  assert.equal(
    getSearchHitSource({ file_id: 42, path: 'app.log', snippet: 'ERROR', line_number: 17 }),
    null
  );
  assert.deepEqual(
    placeContextMenu({ x: 990, y: 790 }, { width: 180, height: 88 }, { width: 1000, height: 800 }),
    { left: 812, top: 704 }
  );
} finally {
  await server.close();
}

console.log('search hit source tests passed');
```

Append `node tests/search-hit-source.mjs` to the `test` script in `frontend/package.json`.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && node tests/search-hit-source.mjs`

Expected: FAIL because `/src/features/files/searchHitSource.ts` does not exist.

- [ ] **Step 3: Implement the pure helpers**

Create `frontend/src/features/files/searchHitSource.ts`:

```ts
import type { IssueLogSearchHit } from '../../api/types';

export type SearchHitSource = {
  bundleHash: string;
  fileId: string;
  nodeId: string;
  path: string;
  line: number | null;
};

export function getSearchHitSource(hit: IssueLogSearchHit): SearchHitSource | null {
  const bundleHash = hit.bundle_hash?.trim();
  const fileId = String(hit.file_id ?? '').trim();
  if (!bundleHash || !fileId) return null;
  return {
    bundleHash,
    fileId,
    nodeId: `${bundleHash}:${fileId}`,
    path: hit.path,
    line: typeof hit.line_number === 'number' && hit.line_number >= 0 ? hit.line_number : null
  };
}

export function placeContextMenu(
  point: { x: number; y: number },
  menu: { width: number; height: number },
  viewport: { width: number; height: number },
  margin = 8
) {
  return {
    left: Math.max(margin, Math.min(point.x, viewport.width - menu.width - margin)),
    top: Math.max(margin, Math.min(point.y, viewport.height - menu.height - margin))
  };
}
```

- [ ] **Step 4: Run the focused test**

Run: `cd frontend && node tests/search-hit-source.mjs`

Expected: PASS with `search hit source tests passed`.

- [ ] **Step 5: Commit the helpers**

```bash
git add frontend/src/features/files/searchHitSource.ts frontend/tests/search-hit-source.mjs frontend/package.json
git commit -m "test: cover search hit source navigation"
```

### Task 2: Accessible Search-Hit Actions

**Files:**
- Create: `frontend/src/features/files/components/SearchHitContextMenu.tsx`
- Modify: `frontend/src/features/files/components/SearchResultViewer.tsx`
- Modify: `frontend/tests/search-hit-source.mjs`

- [ ] **Step 1: Add failing render assertions**

Extend `frontend/tests/search-hit-source.mjs` to SSR-render `SearchResultViewer` with one sourced hit and assert:

```js
assert.match(markup, /aria-label="打开原文件：app\.log，第 18 行"/);
assert.match(markup, /title="打开原文件"/);
assert.match(markup, /app\.log/);
assert.match(markup, /第 18 行/);
```

Also SSR-render `SearchHitContextMenu` and assert:

```js
assert.match(menuMarkup, /role="menu"/);
assert.match(menuMarkup, /在原文件中打开/);
assert.match(menuMarkup, /复制文件路径/);
```

- [ ] **Step 2: Run the focused test to verify it fails**

Run: `cd frontend && node tests/search-hit-source.mjs`

Expected: FAIL because the menu component and actionable-row props do not exist.

- [ ] **Step 3: Implement the context menu**

Create `SearchHitContextMenu.tsx` with props:

```ts
type SearchHitContextMenuProps = {
  x: number;
  y: number;
  canOpen: boolean;
  onOpen: () => void;
  onCopyPath: () => void;
  onClose: () => void;
};
```

Render a fixed-position `role="menu"` panel with two `role="menuitem"` buttons. Measure the panel in `useLayoutEffect`, call `placeContextMenu`, focus the first enabled item, and install `pointerdown`, `scroll`, and `keydown` listeners. Escape calls `onClose`; ArrowUp/ArrowDown cycles enabled items; either action calls its callback and then closes.

- [ ] **Step 4: Make search results actionable**

Add these props to `SearchResultViewerProps`:

```ts
onOpenSource: (hit: IssueLogSearchHit) => void;
onCopySourcePath: (hit: IssueLogSearchHit) => void;
```

Keep `CodeLinesPane` for layout, but extend it with an optional `renderRow` callback so each result can render as a full-width button containing path, original one-based line number, and highlighted snippet. The button uses `getSearchHitSource(hit)` to determine whether opening is available, invokes `onOpenSource(hit)` on click, and opens `SearchHitContextMenu` from `onContextMenu`. Missing source metadata disables only the open action; copying remains enabled when `hit.path` is non-empty.

- [ ] **Step 5: Run focused tests and type checking**

Run: `cd frontend && node tests/search-hit-source.mjs && npm run lint`

Expected: both commands PASS with no TypeScript errors.

- [ ] **Step 6: Commit the result actions**

```bash
git add frontend/src/features/files/components/SearchHitContextMenu.tsx frontend/src/features/files/components/SearchResultViewer.tsx frontend/src/features/files/components/CodeLinesPane.tsx frontend/tests/search-hit-source.mjs
git commit -m "feat: add source actions to search hits"
```

### Task 3: Open the Original File and Highlight Its Line

**Files:**
- Modify: `frontend/src/features/files/FilesView.tsx`
- Modify: `frontend/src/features/files/components/CodeLinesPane.tsx`
- Modify: `frontend/tests/search-hit-source.mjs`

- [ ] **Step 1: Add failing integration-source assertions**

Read `FilesView.tsx` and `CodeLinesPane.tsx` in `frontend/tests/search-hit-source.mjs`, then assert the required wiring:

```js
assert.match(filesView, /const openSearchHitSource = useCallback/);
assert.match(filesView, /getSearchHitSource\(hit\)/);
assert.match(filesView, /handleNodeClick\(source\.nodeId, source\.line, \{ preserveSearch: true \}\)/);
assert.match(filesView, /navigator\.clipboard\.writeText\(hit\.path\)/);
assert.match(filesView, /onOpenSource=\{openSearchHitSource\}/);
assert.match(filesView, /onCopySourcePath=\{copySearchHitPath\}/);
assert.match(codeLinesPane, /targetLine/);
assert.match(codeLinesPane, /data-source-line/);
assert.match(codeLinesPane, /bg-amber-100/);
```

- [ ] **Step 2: Run the focused test to verify it fails**

Run: `cd frontend && node tests/search-hit-source.mjs`

Expected: FAIL on missing `openSearchHitSource` wiring.

- [ ] **Step 3: Wire source opening and feedback**

In `FilesView.tsx`, import `getSearchHitSource`, add a lightweight `sourceActionMessage` state, and implement:

```ts
const openSearchHitSource = useCallback(async (hit: IssueLogSearchHit) => {
  const source = getSearchHitSource(hit);
  if (!source) {
    setSourceActionMessage('来源文件信息不可用');
    return;
  }
  await handleNodeClick(source.nodeId, source.line, { preserveSearch: true });
  if (source.line === null) setSourceActionMessage('已打开文件，原始行号不可用');
}, [handleNodeClick]);

const copySearchHitPath = useCallback(async (hit: IssueLogSearchHit) => {
  if (!hit.path) {
    setSourceActionMessage('文件路径不可用');
    return;
  }
  try {
    await navigator.clipboard.writeText(hit.path);
    setSourceActionMessage('已复制文件路径');
  } catch {
    setSourceActionMessage('复制文件路径失败');
  }
}, []);
```

Pass both callbacks to `SearchResultViewer` and render `sourceActionMessage` in an `aria-live="polite"` status element near the viewer toolbar. Clear stale success messages when the active tab changes; preserve errors until the next source action.

- [ ] **Step 4: Center and highlight the source line**

Add `targetLine?: number | null` to `CodeLinesPaneProps`. For each content row, compare `line.line_number` with `targetLine`, add `data-source-line={line.line_number}`, and apply `bg-amber-100 ring-1 ring-inset ring-amber-300` to the target row.

Pass `targetLine` from the file-view branch in `FilesView.tsx`. Replace the fixed-height scroll calculation with a DOM lookup after lines render:

```ts
const target = contentRef.current.querySelector<HTMLElement>(
  `[data-source-line="${targetLine}"]`
);
target?.scrollIntoView({ block: 'center' });
```

Keep the existing saved `scrollTop` restoration only when `targetLine` is null.

- [ ] **Step 5: Handle navigation failures without losing results**

Wrap `handleNodeClick` inside `openSearchHitSource` with `try/catch`, normalize the error into `sourceActionMessage`, and leave the active search tab and its hits unchanged. `handleNodeClick` must not clear search state because it is called with `{ preserveSearch: true }`.

- [ ] **Step 6: Run focused and full frontend verification**

Run: `cd frontend && node tests/search-hit-source.mjs && npm test && npm run lint && npm run build`

Expected: all tests PASS, TypeScript emits no errors, and Vite reports a successful production build.

- [ ] **Step 7: Commit the integration**

```bash
git add frontend/src/features/files/FilesView.tsx frontend/src/features/files/components/CodeLinesPane.tsx frontend/tests/search-hit-source.mjs
git commit -m "feat: open original files from search results"
```

### Task 4: Final Regression Review

**Files:**
- Verify only; modify files above only if verification exposes a defect.

- [ ] **Step 1: Review the diff against the design**

Run: `git diff HEAD~3 -- frontend/src frontend/tests frontend/package.json`

Expected: changes are limited to source navigation, the context menu, target-line presentation, tests, and test-script registration.

- [ ] **Step 2: Run complete project verification**

Run: `cd frontend && npm test && npm run lint && npm run build`

Run: `cd backend && cargo test`

Expected: frontend tests/type-check/build and backend tests all PASS.

- [ ] **Step 3: Confirm repository state**

Run: `git status --short`

Expected: no uncommitted feature changes. Any pre-existing unrelated user changes remain untouched.
