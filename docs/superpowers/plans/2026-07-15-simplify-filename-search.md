# Simplified Filename Search Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give Issue-level filename search a plain single-query input with direct Enter/Search execution and a complete accessible Clear action, without changing content-search tokens.

**Architecture:** `BundleView` owns separate `filenameQuery` and content-token state and conditionally renders the correct editor. A small pure helper in `filenameSearch.ts` decides Clear visibility for direct unit testing, while a request-generation ref prevents cleared or mode-switched searches from restoring stale results.

**Tech Stack:** React 18, TypeScript, Vite SSR test runner, Node assertions, Tailwind CSS.

---

### Task 1: Define and test filename-search state behavior

**Files:**
- Create: `frontend/src/features/files/filenameSearch.ts`
- Create: `frontend/tests/filename-search.mjs`
- Modify: `frontend/package.json`

- [ ] Write a failing Node test that imports `shouldShowFilenameClear` and asserts false for an empty idle state and true for query text, executed state, results, loading, or error. The test also reads `FilesView.tsx` and asserts filename mode contains `aria-label="文件名搜索"`, a Clear label, and a submit handler while the token editor remains present for content mode.
- [ ] Add `node tests/filename-search.mjs` to the frontend `test` script and run `npm test`; expect failure because the helper and filename-only markup do not exist.
- [ ] Implement:

```ts
export type FilenameSearchState = {
  query: string;
  executed: boolean;
  resultCount: number;
  loading: boolean;
  error: string | null;
};

export function shouldShowFilenameClear(state: FilenameSearchState): boolean {
  return Boolean(
    state.query.trim() || state.executed || state.resultCount > 0 || state.loading || state.error
  );
}
```

- [ ] Run `npm test`; helper assertions pass while any still-unimplemented markup assertions remain red for Task 2.
- [ ] Commit the helper/test setup with message `Test filename search controls`.

### Task 2: Separate filename and content controls

**Files:**
- Modify: `frontend/src/features/files/FilesView.tsx`
- Test: `frontend/tests/filename-search.mjs`
- Test: `frontend/tests/search-tokens.mjs`

- [ ] Replace filename-mode token state usage with `filenameQuery`, add `filenameInputRef` and `searchRequestGenerationRef`, and keep `searchTokens`/`searchDraft` exclusive to content mode.
- [ ] In filename mode render a controlled native input with `aria-label="文件名搜索"`; wrap the controls in a form whose submit prevents navigation and calls `runSearch`, so Enter and the existing submit button share one path. Render `SearchTokenEditor` only in content mode.
- [ ] Implement `clearFilenameSearch`: increment request generation, clear query/results/executed/loading/error and result-filter state, then focus `filenameInputRef` on the next animation frame.
- [ ] Show an accessible text Clear button only when `shouldShowFilenameClear` returns true. Label Search according to the active mode and keep content token controls unchanged.
- [ ] Capture the current generation before each request. In success, error, and finally blocks, update state only when it still equals `searchRequestGenerationRef.current`. Increment the generation when changing modes and reset active result/filter/loading/error state without copying editor values between modes.
- [ ] Update highlight and `canRunSearch` derivations to use trimmed filename text in filename mode and token validation in content mode.
- [ ] Run `npm test`; expect filename-control and existing token-editor tests to pass.
- [ ] Run `npm run lint` and `npm run build`; expect TypeScript and production build success.
- [ ] Commit with message `Simplify filename search controls`.

### Task 3: Review and publish

**Files:**
- Review all files changed since `origin/main`.

- [ ] Run `git diff --check`, `npm test`, `npm run lint`, and `npm run build` with fresh output.
- [ ] Review `git diff origin/main...HEAD` against every issue #16 acceptance criterion, including no filename `+`, Clear reset/focus, stale-request invalidation, separate mode state, and unchanged boolean controls.
- [ ] Push `agent/simplify-filename-search` and create a draft PR targeting `main` with `Fixes #16`, behavior summary, accessibility notes, and exact validation commands.
