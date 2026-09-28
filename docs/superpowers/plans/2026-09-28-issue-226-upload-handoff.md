# Issue #226 Upload Handoff Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent an `ACCEPTED` local upload task from reappearing as an optimistic ghost row after its server Bundle is observed and later removed.

**Architecture:** Add a one-way `ACCEPTED → HANDED_OFF` transition to the singleton upload queue. `HomeView` acknowledges accepted tasks when the current Bundle snapshot contains their response hash, and both the view-level task filter and row builder suppress handed-off tasks independently of the current snapshot. Server Bundle rows remain derived only from the server snapshot.

**Tech Stack:** React 18, TypeScript, Vitest, Testing Library, Vite.

---

## File map

- Modify `frontend/src/features/files/uploadQueue.ts`: add the `HANDED_OFF` queue status and an idempotent `markHandedOff(taskId)` transition.
- Modify `frontend/src/features/files/uploadRows.ts`: centralize the optimistic-task visibility predicate and suppress handed-off tasks.
- Modify `frontend/src/features/files/hooks/useUploadTask.ts`: expose `acknowledgeAcceptedTasks(bundleHashes)` and exclude handed-off tasks from the retry-selection summary.
- Modify `frontend/src/features/files/HomeView.tsx`: derive the current Bundle hash set, acknowledge matching accepted tasks in an effect, and reuse the shared visibility predicate.
- Modify `frontend/tests/upload-queue.behavior.test.ts`: verify the queue transition contract.
- Modify `frontend/tests/upload-polling.behavior.test.tsx`: verify the hook-level hash acknowledgement and idempotency.
- Create `frontend/tests/upload-handoff.behavior.test.ts`: verify row behavior when a failed/ready Bundle is present or has disappeared.

## Task 1: Add the queue handoff transition

**Files:**
- Modify: `frontend/tests/upload-queue.behavior.test.ts`
- Modify: `frontend/src/features/files/uploadQueue.ts`

- [ ] **Step 1: Write the failing queue test**

Extend the existing `runs at most two file uploads...` test immediately after the current `ACCEPTED` assertion:

```ts
    const acceptedTask = queue.getTasks('ISSUE-1').find((task) => task.name === 'a.log')!;
    expect(queue.markHandedOff(acceptedTask.id)).toBe(true);
    expect(queue.getTasks('ISSUE-1').find((task) => task.name === 'a.log')?.status).toBe('HANDED_OFF');
    expect(queue.markHandedOff(acceptedTask.id)).toBe(false);
    expect(queue.getTasks('ISSUE-1').find((task) => task.name === 'a.log')?.response).toEqual({
      bundle_hash: 'bundle-a'
    });
```

Add a separate no-op assertion for a missing task:

```ts
  it('only hands off an accepted task with a response', () => {
    const queue = createUploadQueue(async () => ({ bundle_hash: 'unused' }));

    expect(queue.markHandedOff('missing-task')).toBe(false);
  });
```

- [ ] **Step 2: Run the focused test and verify it fails for the missing API**

Run:

```bash
cd frontend
npx vitest run tests/upload-queue.behavior.test.ts
```

Expected: the test fails at runtime because `queue.markHandedOff` is not yet returned by `createUploadQueue`.

- [ ] **Step 3: Implement the minimal queue transition**

In `frontend/src/features/files/uploadQueue.ts`, add the status literal:

```ts
export type UploadQueueTaskStatus =
  | 'QUEUED'
  | 'UPLOADING'
  | 'RETRY_WAIT'
  | 'ACCEPTED'
  | 'HANDED_OFF'
  | 'FAILED'
  | 'UNCONFIRMED';
```

Before the returned object, add:

```ts
  const markHandedOff = (id: string) => {
    const task = tasks.get(id);
    if (!task || task.status !== 'ACCEPTED' || !task.response) return false;
    update(id, { status: 'HANDED_OFF' });
    return true;
  };
```

Return it with the existing queue methods:

```ts
  return {
    enqueue,
    markHandedOff,
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    getSnapshot() {
      return snapshot;
    },
    getTasks(issueCode?: string) {
      return issueCode ? snapshot.filter((task) => task.issueCode === issueCode) : [...snapshot];
    },
    retry
  };
```

The method must not clear `response`, and calling it for a non-`ACCEPTED` task must return `false` without changing the task.

- [ ] **Step 4: Run the focused test and verify it passes**

Run:

```bash
cd frontend
npx vitest run tests/upload-queue.behavior.test.ts
```

Expected: the test file passes, including the existing queue and row tests.

- [ ] **Step 5: Commit the queue change**

```bash
git add frontend/src/features/files/uploadQueue.ts frontend/tests/upload-queue.behavior.test.ts
git commit -m "feat: track upload bundle handoff"
```

## Task 2: Centralize optimistic-row visibility

**Files:**
- Create: `frontend/tests/upload-handoff.behavior.test.ts`
- Modify: `frontend/src/features/files/uploadRows.ts`

- [ ] **Step 1: Write failing row visibility tests**

Create `frontend/tests/upload-handoff.behavior.test.ts` with these fixtures and tests:

```ts
import { describe, expect, it } from 'vitest';
import type { UploadResponse, UploadSummary } from '../src/api/types';
import { buildFileRows } from '../src/features/files/homeRows';
import { createOptimisticUploadRows, type UploadTaskSnapshot } from '../src/features/files/uploadRows';

const response: UploadResponse = {
  task_id: 'task-226',
  issue_code: 'ISSUE-226',
  bundle_hash: 'bundle-226',
  status: 'FAILED',
  stage: 'FAILED',
  file_count: 0,
  total_bytes: 1
};

const handedOffTask: UploadTaskSnapshot = {
  id: 'task-226',
  issueCode: 'ISSUE-226',
  file: new File(['broken'], 'broken.zip'),
  name: 'broken.zip',
  sizeBytes: 7,
  status: 'HANDED_OFF',
  progressPercent: 100,
  message: null,
  response
};

const readyBundle: UploadSummary = {
  hash: 'bundle-226',
  name: 'broken.zip',
  status: { upload_status: 'READY' },
  stage: 'READY',
  size_bytes: 7
};

it('does not create an optimistic row for a handed-off task when its Bundle is gone', () => {
  expect(createOptimisticUploadRows([handedOffTask], new Set())).toEqual([]);
  expect(buildFileRows({ bundles: [], bundleFiles: {}, uploadTasks: [handedOffTask] })).toEqual([]);
});

it('keeps a handed-off task out of rows while the server READY Bundle remains visible', () => {
  const rows = buildFileRows({
    bundles: [readyBundle],
    bundleFiles: {
      'bundle-226': {
        files: [{
          id: 1,
          name: 'app.log',
          path: 'app.log',
          is_dir: false,
          preview_kind: 'text',
          size_bytes: 1
        }],
        loading: false,
        loaded: true,
        error: null
      }
    },
    uploadTasks: [handedOffTask]
  });

  expect(rows).toHaveLength(1);
  expect(rows[0]).toMatchObject({ bundleHash: 'bundle-226', name: 'app.log', stage: 'READY' });
});
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```bash
cd frontend
npx vitest run tests/upload-handoff.behavior.test.ts
```

Expected: the first test fails because the current row filter only considers the current Bundle hash and allows the `HANDED_OFF` task when the hash set is empty.

- [ ] **Step 3: Implement the shared visibility predicate**

In `frontend/src/features/files/uploadRows.ts`, add:

```ts
export const shouldShowOptimisticUploadTask = (
  task: UploadTaskSnapshot,
  existingBundleHashes: ReadonlySet<string>
) =>
  task.status !== 'HANDED_OFF' &&
  (!task.response || !existingBundleHashes.has(task.response.bundle_hash));
```

Use it in `createOptimisticUploadRows`:

```ts
export const createOptimisticUploadRows = (
  tasks: readonly UploadTaskSnapshot[],
  existingBundleHashes: ReadonlySet<string>
) =>
  tasks
    .filter((task) => shouldShowOptimisticUploadTask(task, existingBundleHashes))
    .map((task) => ({
      key: task.id,
      bundleHash: task.response?.bundle_hash ?? '',
      bundleName: task.name,
      name: task.name,
      status: task.status === 'FAILED' || task.status === 'UNCONFIRMED'
        ? ('FAILED' as const)
        : (task.response?.status ?? 'PENDING'),
      stage: task.status as LocalUploadStage,
      progressPercent: task.progressPercent,
      sizeBytes: task.sizeBytes,
      failureReason: task.message,
      uploadTaskId: task.id
    }));
```

- [ ] **Step 4: Run the focused test and verify it passes**

Run:

```bash
cd frontend
npx vitest run tests/upload-handoff.behavior.test.ts
```

Expected: both row tests pass, proving a disappeared failed Bundle does not recreate a local row and an existing READY Bundle is still rendered from server data.

- [ ] **Step 5: Commit the row behavior**

```bash
git add frontend/src/features/files/uploadRows.ts frontend/tests/upload-handoff.behavior.test.ts
git commit -m "fix: suppress handed-off upload rows"
```

## Task 3: Expose and test hash-based acknowledgement in the upload hook

**Files:**
- Modify: `frontend/tests/upload-polling.behavior.test.tsx`
- Modify: `frontend/src/features/files/hooks/useUploadTask.ts`

- [ ] **Step 1: Write the failing hook test**

Add this test to `frontend/tests/upload-polling.behavior.test.tsx` using a unique Issue code:

```tsx
  it('acknowledges an accepted task only when its Bundle hash is observed', async () => {
    vi.mocked(rainApi.uploadLogs).mockResolvedValueOnce(uploadResponse('handoff-226'));
    const loadBundles = vi.fn().mockResolvedValue(undefined);
    const loadIssues = vi.fn().mockResolvedValue(undefined);
    const { result, unmount } = renderHook(() => useUploadTask({
      currentIssueCode: 'ISSUE-HANDOFF-226',
      loadBundles,
      loadIssues
    }));

    await act(async () => {
      await result.current.performUpload([new File(['a'], 'handoff.log')]);
      await Promise.resolve();
    });
    expect(result.current.uploadTasks[0].status).toBe('ACCEPTED');

    act(() => result.current.acknowledgeAcceptedTasks(new Set(['other-bundle'])));
    expect(result.current.uploadTasks[0].status).toBe('ACCEPTED');

    act(() => result.current.acknowledgeAcceptedTasks(new Set(['bundle-handoff-226'])));
    expect(result.current.uploadTasks[0].status).toBe('HANDED_OFF');

    act(() => result.current.acknowledgeAcceptedTasks(new Set(['bundle-handoff-226'])));
    expect(result.current.uploadTasks[0].status).toBe('HANDED_OFF');
    expect(result.current.uploadSelection).toEqual([]);
    unmount();
  });
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```bash
cd frontend
npx vitest run tests/upload-polling.behavior.test.tsx
```

Expected: TypeScript/test failure because `acknowledgeAcceptedTasks` is not yet exposed.

- [ ] **Step 3: Implement the hook acknowledgement API**

In `frontend/src/features/files/hooks/useUploadTask.ts`, add this callback after `uploadTasks` is derived:

```ts
  const acknowledgeAcceptedTasks = useCallback((bundleHashes: ReadonlySet<string>) => {
    uploadTasks.forEach((task) => {
      if (
        task.status === 'ACCEPTED' &&
        task.response &&
        bundleHashes.has(task.response.bundle_hash)
      ) {
        uploadQueue.markHandedOff(task.id);
      }
    });
  }, [uploadTasks]);
```

Change the upload selection filter so a handed-off task cannot be treated as retryable selection state:

```ts
  const uploadSelection: UploadSelectionItem[] = uploadTasks
    .filter((task) => task.status !== 'ACCEPTED' && task.status !== 'HANDED_OFF')
    .map((task) => ({
      name: task.name,
      sizeBytes: task.sizeBytes
    }));
```

Return the callback next to `retryUpload`:

```ts
    retryUpload: uploadQueue.retry,
    acknowledgeAcceptedTasks,
```

- [ ] **Step 4: Run the focused test and verify it passes**

Run:

```bash
cd frontend
npx vitest run tests/upload-polling.behavior.test.tsx
```

Expected: all polling and upload-hook tests pass, including the new hash acknowledgement test.

- [ ] **Step 5: Commit the hook behavior**

```bash
git add frontend/src/features/files/hooks/useUploadTask.ts frontend/tests/upload-polling.behavior.test.tsx
git commit -m "feat: acknowledge observed upload bundles"
```

## Task 4: Connect Bundle snapshots to handoff and run the full regression suite

**Files:**
- Modify: `frontend/src/features/files/HomeView.tsx`

- [ ] **Step 1: Wire the effect in `HomeView`**

Add this import to `frontend/src/features/files/HomeView.tsx`:

```ts
import { shouldShowOptimisticUploadTask } from './uploadRows';
```

Immediately after `useUploadTask(...)`, derive and acknowledge the current server hashes:

```tsx
  const existingBundleHashes = useMemo(
    () => new Set(bundles.bundles.map((bundle) => bundle.hash)),
    [bundles.bundles]
  );

  useEffect(() => {
    upload.acknowledgeAcceptedTasks(existingBundleHashes);
  }, [existingBundleHashes, upload.acknowledgeAcceptedTasks]);

  const visibleUploadTasks = useMemo(
    () => upload.tasks.filter((task) => shouldShowOptimisticUploadTask(task, existingBundleHashes)),
    [existingBundleHashes, upload.tasks]
  );
```

Remove the old inline hash filter so there is one shared rule for the upload panel and file rows. Keep `buildFileRows` unchanged; it continues to receive `visibleUploadTasks` and server Bundle rows.

- [ ] **Step 2: Run the full frontend verification**

Run:

```bash
cd frontend
npm test
npm run build
```

Expected: all Vitest files and standalone Node behavior tests pass; TypeScript and Vite build exit with code 0. Existing warnings about stale browser data may remain and are unrelated to this change.

- [ ] **Step 3: Run the backend verification after frontend assets exist**

Run:

```bash
cd backend
cargo build
```

Expected: the backend compiles with the generated `frontend/dist` assets embedded.

- [ ] **Step 4: Inspect the final diff and commit the integration**

Run:

```bash
git diff --check
git status --short
git diff HEAD~1 --stat
```

Confirm only the planned frontend source/test files and the committed plan/spec documents are present, then commit:

```bash
git add frontend/src/features/files/HomeView.tsx frontend/tests/upload-handoff.behavior.test.ts
git commit -m "fix: prevent upload ghost rows after bundle deletion"
```

## Final verification checklist

- [ ] `ACCEPTED → HANDED_OFF` is the only handoff transition.
- [ ] Repeated acknowledgement is a no-op.
- [ ] A handed-off task creates no optimistic row with an empty Bundle snapshot.
- [ ] A failed Bundle deletion leaves no “已接收，等待处理” or “处理中，暂不可删除” row.
- [ ] A READY server Bundle still renders normally after handoff.
- [ ] Issue switching and retry behavior remain covered by the existing hook tests.
- [ ] `npm test`, `npm run build`, and `cargo build` all pass from fresh commands.
