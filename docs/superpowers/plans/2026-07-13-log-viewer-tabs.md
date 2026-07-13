# Log Viewer Tabs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace upload summary placeholders with per-file rows, simplify the file footer, and turn the right content pane into a pinned multi-tab log viewer with virtual search-result documents.

**Architecture:** Keep backend APIs unchanged. Extract pure frontend state helpers for upload rows and viewer tabs so behavior can be checked independently with TypeScript compilation, then integrate those helpers into `HomeView` and `BundleView`. A tab owns either a source-file identity or an immutable snapshot of search hits; only the active file tab drives paginated file fetching.

**Tech Stack:** React 18, TypeScript 5.6, Tailwind CSS, Vite, Rust/Actix backend unchanged.

---

### Task 1: Per-file optimistic upload rows

**Files:**
- Create: `frontend/src/features/files/uploadRows.ts`
- Modify: `frontend/src/features/files/HomeView.tsx`

- [x] Define `UploadSelectionItem` and `createOptimisticUploadRows(items, progressPercent, bundleHash)` as a pure helper. It returns one stable row per selected file with keys `active-upload:<index>:<name>`.
- [x] Run `npm run lint`; expect failure until `HomeView` adopts the new selection type.
- [x] Change `uploadSelection` from one summary object to `UploadSelectionItem[]`, populate it with `Array.from(files, file => ({ name: file.name, sizeBytes: file.size }))`, and prepend all optimistic rows while the backend bundle is not visible.
- [x] Run `npm run lint`; expect success and no “等 N 个文件” construction remaining in `HomeView.tsx`.

### Task 2: Viewer tab state model

**Files:**
- Create: `frontend/src/features/files/viewerTabs.ts`
- Modify: `frontend/src/features/files/FilesView.tsx`

- [x] Define discriminated `ViewerTab` types for `file` and `search`, including `id`, `title`, `pinned`, and file pagination or search-hit snapshot fields.
- [x] Define pure operations `openPreviewTab`, `togglePinnedTab`, and `closeViewerTab`; preview opening replaces the existing unpinned tab while preserving pinned tabs.
- [x] Run `npm run lint`; expect success for the isolated type/helper module.
- [x] Add `viewerTabs` and `activeViewerTabId` state to `BundleView`, open a file preview from tree clicks, and reset tabs when the Issue changes.
- [x] Run `npm run lint`; expect success.

### Task 3: Tab bar and active file document

**Files:**
- Modify: `frontend/src/features/files/FilesView.tsx`

- [x] Render a compact tab bar above the right pane with active styling, a Pin/unpin button, and a close button.
- [x] Make the active file tab drive `selectedNodeId`, `lineStart`, and `linePageSize`; store updated pagination back into that tab before switching.
- [x] Preserve and restore each tab content scroller’s `scrollTop` on tab changes.
- [x] Remove the top “行 X - Y / Z” text while retaining “下载原文件”, line gutters, and bottom pagination.
- [x] Run `npm run lint`; expect success.

### Task 4: Virtual search-result documents

**Files:**
- Modify: `frontend/src/features/files/FilesView.tsx`

- [x] On a successful log-content search, snapshot the filtered hits into a `search` preview tab; reuse an existing unpinned preview unless it was pinned.
- [x] Render the active search tab as one continuous monospaced document using only `hit.snippet`; do not render `hit.path` or `hit.line_number` in the document.
- [x] Retain the complete hits in tab state so file ID, bundle hash, path, and source line remain available internally.
- [x] Keep the result-in-search input filtering the latest backend result set without another HTTP request, and update the virtual preview as the local filter changes.
- [x] Keep “生成临时文件” using the existing backend API and route.
- [x] Run `npm run lint`; expect success.

### Task 4.1: Complete result pagination and pinned temporary files

**Files:**
- Modify: `backend/src/routes/temp_results.rs`
- Modify: `backend/tests/smoke.rs`
- Modify: `frontend/src/features/files/viewerTabs.ts`
- Modify: `frontend/src/features/files/FilesView.tsx`

- [x] Raise preview page limits to the existing viewer sizes of 1000 and 3000, with a unit test for clamping.
- [x] Store result totals, offsets, and page sizes in search tabs and load previous/next pages on demand.
- [x] Add a pinned temporary-result tab type backed by `/temp-results/{id}/lines`.
- [x] Open generated temporary files in the right pane with download and share actions instead of navigating away.

### Task 5: Verification

**Files:**
- Modify only if verification exposes a defect.

- [x] Run `rg -n "等 .*个文件|行 \{fileLines.start" frontend/src/features/files`; expect no obsolete upload summary or top line-range label.
- [x] Run `npm run build` in `frontend`; expect TypeScript and Vite production build success.
- [x] Run `cargo fmt --check && cargo test` in `backend`; expect all existing backend tests to pass.
- [x] Run `git diff --check`; expect no whitespace errors, then review `git status --short` to confirm only scoped files changed.

### Task 6: Preserve multiple tabs and close the last tab

**Files:**
- Modify: `frontend/src/features/files/viewerTabs.ts`
- Modify: `frontend/src/features/files/FilesView.tsx`

- [x] Append different file, search, and temporary-result tabs instead of replacing an unpinned tab.
- [x] Activate an existing tab when the same document is opened again.
- [x] Gate automatic initial-file opening so closing the final tab does not recreate it.
