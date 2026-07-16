# Search Results Log View Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render search hits as continuous log lines and expose source navigation only through a one-item right-click menu.

**Architecture:** Keep source metadata and the existing `BundleView` navigation path, while changing only the search result presentation and context-menu surface. Static SSR regression tests assert the DOM contract, and source checks ensure obsolete clipboard wiring is removed.

**Tech Stack:** React 18, TypeScript, Vite SSR test harness, Node.js assertions, Tailwind CSS

---

### Task 1: Lock the log-style result contract with failing tests

**Files:**
- Modify: `frontend/tests/search-hit-source.mjs`

- [ ] **Step 1: Replace the card and copy-action assertions with the desired DOM contract**

Update the `SearchResultViewer` assertion block to require a continuous grid, original one-based line number, selectable log content, and no source-opening button:

```js
assert.match(markup, /data-search-results-log="true"/);
assert.match(markup, /data-source-line="17"/);
assert.match(markup, />18<\/span>/);
assert.match(markup, /select-text/);
assert.doesNotMatch(markup, /aria-label="打开原文件/);
assert.doesNotMatch(markup, /title="打开原文件"/);
```

Change the missing-source assertion to require a readable log line without disabled-button semantics. Render `SearchHitContextMenu` with only `x`, `y`, `onOpen`, and `onClose`, then assert it contains “在原文件中打开” and does not contain “复制文件路径”. Remove `onCopySourcePath` from viewer props and replace source checks with:

```js
assert.doesNotMatch(filesView, /navigator\.clipboard\.writeText\(hit\.path\)/);
assert.doesNotMatch(filesView, /onCopySourcePath=/);
```

- [ ] **Step 2: Run the focused test and verify the new contract fails**

Run: `node tests/search-hit-source.mjs` from `frontend/`

Expected: FAIL in `search-hit-source.mjs` because the log marker is absent and the copy action still exists.

- [ ] **Step 3: Commit the failing regression test**

```bash
git add frontend/tests/search-hit-source.mjs
git commit -m "test: require log-style search results"
```

### Task 2: Simplify the context menu to source opening only

**Files:**
- Modify: `frontend/src/features/files/components/SearchHitContextMenu.tsx`

- [ ] **Step 1: Remove copy-related props and the second menu item**

Use this prop surface:

```ts
type SearchHitContextMenuProps = {
  x: number;
  y: number;
  onOpen: () => void;
  onClose: () => void;
};
```

Keep the existing positioning, focus, keyboard, outside-click, and scroll-close effects. Render one enabled `role="menuitem"` button labeled `在原文件中打开`, with `onClick={() => run(onOpen)}`.

- [ ] **Step 2: Run the focused test to confirm the menu assertions progress**

Run: `node tests/search-hit-source.mjs` from `frontend/`

Expected: FAIL on the still-missing continuous log markup, not on menu props or copy text.

- [ ] **Step 3: Commit the menu simplification**

```bash
git add frontend/src/features/files/components/SearchHitContextMenu.tsx
git commit -m "refactor: simplify search result context menu"
```

### Task 3: Render search hits as continuous log lines

**Files:**
- Modify: `frontend/src/features/files/components/SearchResultViewer.tsx`
- Modify: `frontend/src/features/files/FilesView.tsx`

- [ ] **Step 1: Remove copy wiring and replace result cards with a log grid**

Delete `onCopySourcePath` from the props type and component parameters. Inside the scroll container render a grid marked `data-search-results-log="true"`. For each hit, render one row with `data-source-line={source?.line ?? undefined}` and a two-column layout:

```tsx
<div
  key={`${hit.bundle_hash ?? 'bundle'}:${hit.file_id}:${hit.line_number ?? index}:${index}`}
  data-source-line={source?.line ?? undefined}
  className="grid grid-cols-[auto_minmax(0,1fr)] gap-3 px-1 font-mono text-xs leading-5 hover:bg-sky-50/60"
  onContextMenu={(event) => {
    if (!source) return;
    event.preventDefault();
    setContextMenu({ x: event.clientX, y: event.clientY, hit });
  }}
>
  <span className="select-none text-right text-slate-500">
    {(source?.line ?? activeViewerTab.from + index) + 1}
  </span>
  <span className="select-text whitespace-pre text-slate-900">
    {renderHighlightedText(hit.snippet, highlightTerm)}
  </span>
</div>
```

Render `SearchHitContextMenu` only for the stored valid-source hit and pass only `x`, `y`, `onOpen`, and `onClose`.

- [ ] **Step 2: Remove obsolete clipboard logic from the container**

In `frontend/src/features/files/FilesView.tsx`, delete `copySearchHitPath` and remove `onCopySourcePath={copySearchHitPath}` from `SearchResultViewer`.

- [ ] **Step 3: Run the focused regression test**

Run: `node tests/search-hit-source.mjs` from `frontend/`

Expected: PASS and print `search hit source tests passed`.

- [ ] **Step 4: Commit the implementation**

```bash
git add frontend/src/features/files/components/SearchResultViewer.tsx frontend/src/features/files/FilesView.tsx
git commit -m "feat: show search results as log lines"
```

### Task 4: Verify the frontend

**Files:**
- Verify: `frontend/tests/search-hit-source.mjs`
- Verify: `frontend/src/features/files/components/SearchResultViewer.tsx`
- Verify: `frontend/src/features/files/components/SearchHitContextMenu.tsx`
- Verify: `frontend/src/features/files/FilesView.tsx`

- [ ] **Step 1: Run all frontend regression tests**

Run: `npm test` from `frontend/`

Expected: all five scripts pass with no assertion failures.

- [ ] **Step 2: Run TypeScript checking**

Run: `npm run lint` from `frontend/`

Expected: exit code 0 with no TypeScript errors.

- [ ] **Step 3: Build the production frontend**

Run: `npm run build` from `frontend/`

Expected: TypeScript and Vite complete with exit code 0.

- [ ] **Step 4: Inspect the final diff and commit any plan tracking updates**

Run: `git diff --check && git status --short` from the repository root.

Expected: no whitespace errors; only intentional plan tracking changes may remain.
