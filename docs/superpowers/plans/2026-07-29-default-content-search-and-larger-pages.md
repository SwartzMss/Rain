# Default Content Search and Larger Pages Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Default the file view to log-content search and use 5,000/10,000-line page choices consistently across frontend viewers and backend limits.

**Architecture:** Centralize frontend line-page choices in one typed constant consumed by both file and temporary-result views. Update backend configuration defaults and the temporary-preview clamp so every line-oriented endpoint accepts the same values, then synchronize operator-facing documentation.

**Tech Stack:** React 18, TypeScript, Vite SSR test harness, Rust, Actix Web

---

### Task 1: Default to content search and centralize frontend page sizes

**Files:**
- Create: `frontend/src/features/files/linePageSizes.ts`
- Create: `frontend/tests/viewer-defaults.mjs`
- Modify: `frontend/src/features/files/FilesView.tsx`
- Modify: `frontend/src/features/files/TempResultView.tsx`
- Modify: `frontend/package.json`

- [ ] **Step 1: Write the failing frontend defaults test**

Create `frontend/tests/viewer-defaults.mjs`:

```js
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createServer } from 'vite';

const server = await createServer({
  appType: 'custom',
  logLevel: 'silent',
  server: { middlewareMode: true }
});

try {
  const { LINE_PAGE_SIZE_OPTIONS } = await server.ssrLoadModule(
    '/src/features/files/linePageSizes.ts'
  );
  assert.deepEqual([...LINE_PAGE_SIZE_OPTIONS], [5000, 10000]);

  const filesView = await readFile(
    new URL('../src/features/files/FilesView.tsx', import.meta.url),
    'utf8'
  );
  const tempResultView = await readFile(
    new URL('../src/features/files/TempResultView.tsx', import.meta.url),
    'utf8'
  );
  assert.match(
    filesView,
    /useState<'log' \| 'detailed'>\('detailed'\)/
  );
  assert.match(filesView, /import \{ LINE_PAGE_SIZE_OPTIONS \} from '\.\/linePageSizes';/);
  assert.match(tempResultView, /import \{ LINE_PAGE_SIZE_OPTIONS \} from '\.\/linePageSizes';/);
  assert.doesNotMatch(filesView, /const LINE_PAGE_SIZE_OPTIONS =/);
  assert.doesNotMatch(tempResultView, /const PAGE_SIZES =/);
} finally {
  await server.close();
}

console.log('viewer default tests passed');
```

Append `node tests/viewer-defaults.mjs` to the `test` script in `frontend/package.json`.

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
cd frontend
node tests/viewer-defaults.mjs
```

Expected: FAIL because `linePageSizes.ts` does not exist and the file view still initializes in filename mode.

- [ ] **Step 3: Add the shared page-size constant**

Create `frontend/src/features/files/linePageSizes.ts`:

```ts
export const LINE_PAGE_SIZE_OPTIONS = [5000, 10000] as const;
```

- [ ] **Step 4: Integrate the new frontend defaults**

In `frontend/src/features/files/FilesView.tsx`, replace the local constant with:

```ts
import { LINE_PAGE_SIZE_OPTIONS } from './linePageSizes';
```

Change the initial mode to:

```ts
const [searchMode, setSearchMode] = useState<'log' | 'detailed'>('detailed');
```

In `frontend/src/features/files/TempResultView.tsx`, import the same constant, remove `PAGE_SIZES`, initialize from `LINE_PAGE_SIZE_OPTIONS[0]`, and render options from `LINE_PAGE_SIZE_OPTIONS`.

- [ ] **Step 5: Run the focused test and verify GREEN**

Run:

```bash
cd frontend
node tests/viewer-defaults.mjs
```

Expected: PASS with `viewer default tests passed`.

### Task 2: Increase backend line-page defaults and preview clamp

**Files:**
- Modify: `backend/src/config.rs`
- Modify: `backend/src/routes/temp_results.rs`

- [ ] **Step 1: Write failing backend assertions**

In `backend/src/config.rs`, extend `defaults_expose_only_meaningful_workflow_limits`:

```rust
assert_eq!(limits.api.default_line_page_size, 5_000);
assert_eq!(limits.api.max_line_page_size, 10_000);
```

Update `preview_supports_log_viewer_page_sizes` in `backend/src/routes/temp_results.rs`:

```rust
assert_eq!(preview_page_size(None), 5_000);
assert_eq!(preview_page_size(Some(5_000)), 5_000);
assert_eq!(preview_page_size(Some(10_000)), 10_000);
assert_eq!(preview_page_size(Some(20_000)), 10_000);
```

- [ ] **Step 2: Run focused backend tests and verify RED**

Run:

```bash
cd backend
cargo test config::tests::defaults_expose_only_meaningful_workflow_limits
cargo test routes::temp_results::tests::preview_supports_log_viewer_page_sizes
```

Expected: both tests FAIL against the old 1,000/3,000 defaults.

- [ ] **Step 3: Update backend defaults**

In `ApiConfig::default` in `backend/src/config.rs`, set:

```rust
default_line_page_size: 5_000,
max_line_page_size: 10_000,
```

Update `preview_page_size` in `backend/src/routes/temp_results.rs`:

```rust
fn preview_page_size(requested: Option<i64>) -> i64 {
    requested.unwrap_or(5_000).clamp(1, 10_000)
}
```

- [ ] **Step 4: Run focused backend tests and verify GREEN**

Run:

```bash
cd backend
cargo test config::tests::defaults_expose_only_meaningful_workflow_limits
cargo test routes::temp_results::tests::preview_supports_log_viewer_page_sizes
```

Expected: both focused tests PASS.

### Task 3: Synchronize configuration documentation

**Files:**
- Modify: `backend/.env.example`
- Modify: `README.md`

- [ ] **Step 1: Update documented defaults**

Set the example environment values to:

```dotenv
RAIN_API_DEFAULT_LINE_PAGE_SIZE=5000
RAIN_API_MAX_LINE_PAGE_SIZE=10000
```

Set the README configuration-table defaults to `5000` and `10000`, keeping the existing descriptions.

- [ ] **Step 2: Verify old documented values are gone**

Run:

```bash
rg -n 'RAIN_API_(DEFAULT|MAX)_LINE_PAGE_SIZE=(1000|3000)|RAIN_API_(DEFAULT|MAX)_LINE_PAGE_SIZE.*`(1000|3000)`' backend/.env.example README.md
```

Expected: no matches.

### Task 4: Full verification and commit

**Files:**
- Test all files changed by Tasks 1–3.

- [ ] **Step 1: Run frontend verification**

Run:

```bash
cd frontend
npm test
npm run lint
npm run build
```

Expected: all commands exit successfully.

- [ ] **Step 2: Run backend verification**

Run:

```bash
cd backend
cargo fmt --check
cargo test config::tests
cargo test routes::temp_results::tests
cargo test routes::files::tests
```

Expected: all commands exit successfully.

- [ ] **Step 3: Review the patch**

Run:

```bash
git diff --check
git status --short
```

Expected: no whitespace errors and only the planned source, test, configuration, and documentation files are changed.

- [ ] **Step 4: Commit**

Run:

```bash
git add README.md backend/.env.example backend/src/config.rs backend/src/routes/temp_results.rs frontend/package.json frontend/src/features/files/FilesView.tsx frontend/src/features/files/TempResultView.tsx frontend/src/features/files/linePageSizes.ts frontend/tests/viewer-defaults.mjs docs/superpowers/plans/2026-07-29-default-content-search-and-larger-pages.md
git commit -m "feat: default to content search with larger pages"
```
