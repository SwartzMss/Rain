# Issue #181 Phase 1 Upload Queue Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make multi-file uploads independent, globally limited to two in-flight file requests, and visible with per-file transfer and acceptance states without changing the backend upload protocol.

**Architecture:** Add a small application-level upload queue with a singleton instance in the frontend. `useUploadTask` subscribes to that queue and filters snapshots by the selected Issue, while each queued item invokes the existing multipart endpoint with exactly one file. Accepted responses release transport slots immediately; the existing Issue bundle polling remains responsible for validation, extraction, indexing, and final readiness.

**Tech Stack:** React 18, TypeScript, Vite/Vitest, existing `rainApi.uploadLogs` XHR multipart client, Tailwind CSS.

---

### Task 1: Add executable queue behavior tests

**Files:**
- Create: `frontend/tests/upload-queue.behavior.test.ts`
- Test target: `frontend/src/features/files/uploadQueue.ts`

- [ ] **Step 1: Write the failing tests**

Create tests for the public queue contract:

```ts
import { describe, expect, it } from 'vitest';
import { createUploadQueue } from '../src/features/files/uploadQueue';

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

function waitForQueue() {
  return new Promise<void>((resolve) => queueMicrotask(resolve));
}

it('runs at most two file uploads and starts the next file after acceptance', async () => {
  const requests = new Map<string, ReturnType<typeof deferred<{ bundle_hash: string }>>>();
  let active = 0;
  let peak = 0;
  const queue = createUploadQueue(async (_issueCode, file) => {
    active += 1;
    peak = Math.max(peak, active);
    const request = deferred<{ bundle_hash: string }>();
    requests.set(file.name, request);
    const result = await request.promise;
    active -= 1;
    return result;
  }, 2);

  queue.enqueue('ISSUE-1', [new File(['a'], 'a.log'), new File(['b'], 'b.log'), new File(['c'], 'c.log')]);
  await waitForQueue();
  expect([...requests.keys()]).toEqual(['a.log', 'b.log']);
  expect(peak).toBe(2);

  requests.get('a.log')!.resolve({ bundle_hash: 'bundle-a' });
  await waitForQueue();
  expect([...requests.keys()]).toEqual(['a.log', 'b.log', 'c.log']);
  expect(queue.getTasks('ISSUE-1').find((task) => task.name === 'a.log')?.status).toBe('ACCEPTED');
});

it('marks one file failed without blocking the remaining queue', async () => {
  const requests = new Map<string, ReturnType<typeof deferred<{ bundle_hash: string }>>>();
  const queue = createUploadQueue(async (_issueCode, file) => {
    const request = deferred<{ bundle_hash: string }>();
    requests.set(file.name, request);
    return request.promise;
  }, 2);

  queue.enqueue('ISSUE-1', [new File(['a'], 'a.log'), new File(['b'], 'b.log'), new File(['c'], 'c.log')]);
  await waitForQueue();
  requests.get('a.log')!.reject(new Error('bad upload'));
  await waitForQueue();

  expect(queue.getTasks('ISSUE-1').find((task) => task.name === 'a.log')?.status).toBe('FAILED');
  expect(requests.has('c.log')).toBe(true);
});

it('retries a 429 task after its server-provided delay and releases the slot while waiting', async () => {
  let attempts = 0;
  const queue = createUploadQueue(async () => {
    attempts += 1;
    if (attempts === 1) {
      throw Object.assign(new Error('busy'), { status: 429, retryAfterMs: 10 });
    }
    return { bundle_hash: 'bundle-a' };
  }, 1);

  queue.enqueue('ISSUE-1', [new File(['a'], 'a.log')]);
  await waitForQueue();
  expect(queue.getTasks('ISSUE-1')[0].status).toBe('RETRY_WAIT');

  await new Promise<void>((resolve) => setTimeout(resolve, 15));
  await waitForQueue();
  expect(attempts).toBe(2);
  expect(queue.getTasks('ISSUE-1')[0].status).toBe('ACCEPTED');
});
```

- [ ] **Step 2: Run the focused test and verify it fails for the intended reason**

Run: `npm test -- --run tests/upload-queue.behavior.test.ts` from `frontend/`.

Expected: FAIL because `src/features/files/uploadQueue.ts` does not exist yet; do not proceed if the failure is a test syntax or environment error.

- [ ] **Step 3: Commit the failing test**

```bash
git add frontend/tests/upload-queue.behavior.test.ts
git commit -m "test: define concurrent upload queue behavior"
```

### Task 2: Implement the application-level upload queue

**Files:**
- Create: `frontend/src/features/files/uploadQueue.ts`
- Modify: `frontend/src/api/client.ts:33-77,103-126,294-338`
- Test: `frontend/tests/upload-queue.behavior.test.ts`

- [ ] **Step 1: Add queue types and the injected upload operation**

Define these exported types and factory signature in `uploadQueue.ts`:

```ts
export type UploadQueueTaskStatus =
  | 'QUEUED'
  | 'UPLOADING'
  | 'RETRY_WAIT'
  | 'ACCEPTED'
  | 'FAILED'
  | 'UNCONFIRMED';

export type UploadQueueTask<TResponse> = {
  id: string;
  issueCode: string;
  file: File;
  name: string;
  sizeBytes: number;
  status: UploadQueueTaskStatus;
  progressPercent: number;
  message: string | null;
  response: TResponse | null;
};

export type UploadFileOperation<TResponse> = (
  issueCode: string,
  file: File,
  onProgress: (percent: number) => void
) => Promise<TResponse>;

export declare function createUploadQueue<TResponse>(
  uploadFile: UploadFileOperation<TResponse>,
  concurrency = 2
): {
  enqueue(issueCode: string, files: File[]): string[];
  subscribe(listener: () => void): () => void;
  getSnapshot(): readonly UploadQueueTask<TResponse>[];
  getTasks(issueCode?: string): UploadQueueTask<TResponse>[];
  retry(taskId: string): void;
};
```

- [ ] **Step 2: Implement queue dispatch and observable snapshots**

Implement a FIFO queue with a `Map` of task snapshots, a pending ID list, an active counter, a listener set, and a cached array snapshot. Expose `enqueue(issueCode, files)`, `subscribe(listener)`, `getSnapshot()`, `getTasks(issueCode?)`, and `retry(taskId)`. `enqueue` must create a unique task ID for every selected file, notify subscribers, and dispatch while `active < concurrency`; `runTask` must set `UPLOADING`, forward progress, set `ACCEPTED` on a resolved response, and always decrement the active counter before dispatching the next task.

Classify failures as follows:

```ts
const isRateLimited = (error: unknown): error is { status: 429; retryAfterMs?: number } =>
  typeof error === 'object' && error !== null && 'status' in error && error.status === 429;

const isUnconfirmed = (error: unknown): boolean =>
  !(typeof error === 'object' && error !== null && 'status' in error && Number(error.status) >= 400 && Number(error.status) < 500);
```

For a 429, set `RETRY_WAIT`, release the active slot, and retry up to three attempts after `retryAfterMs` or a capped exponential delay with jitter. For network/timeout/5xx errors, set `UNCONFIRMED` with `接收结果未确认，请刷新文件列表核对；确认未接收后可重试`; for other errors, set `FAILED`. `retry(taskId)` only requeues terminal `FAILED` or `UNCONFIRMED` tasks and retains the original `File` reference.

- [ ] **Step 3: Make the API expose Retry-After and per-file uploads**

Extend `ApiError` with `retryAfterMs?: number`. Parse a numeric `Retry-After` seconds value (or an HTTP date) in both fetch and XHR error paths. Keep `rainApi.uploadLogs(issueCode, files, onProgress?)` backward compatible, but have the queue call it with `[file]`; add an `uploadFile(issueCode, file, onProgress)` helper only if it prevents duplicated FormData/XHR setup. Do not change the backend endpoint or multi-file compatibility.

- [ ] **Step 4: Run focused queue tests and the TypeScript build**

Run: `npm test -- --run tests/upload-queue.behavior.test.ts` from `frontend/`.

Expected: all three queue tests PASS.

Run: `npm run build` from `frontend/`.

Expected: TypeScript and Vite build PASS.

- [ ] **Step 5: Commit the queue implementation**

```bash
git add frontend/src/features/files/uploadQueue.ts frontend/src/api/client.ts frontend/tests/upload-queue.behavior.test.ts
git commit -m "feat: add controlled concurrent upload queue"
```

### Task 3: Integrate queue state into the upload hook

**Files:**
- Modify: `frontend/src/features/files/hooks/useUploadTask.ts`
- Modify: `frontend/src/features/files/uploadRows.ts`
- Modify: `frontend/src/features/files/homeRows.ts`
- Modify: `frontend/src/features/files/HomeView.tsx`
- Test: `frontend/tests/upload-polling.behavior.test.tsx`

- [ ] **Step 1: Replace batch state assertions with per-file queue assertions**

Update the upload hook tests so that a three-file selection invokes `rainApi.uploadLogs` three times with one-file arrays, no more than two promises are active, a resolved request becomes accepted without waiting for bundle polling, and one rejected request produces a failed local task while later files still upload. Add a test that switching the selected Issue filters old tasks instead of changing their `issueCode`.

- [ ] **Step 2: Run the updated hook tests and verify the new expectations fail**

Run: `npm test -- --run tests/upload-polling.behavior.test.tsx` from `frontend/`.

Expected: FAIL because `useUploadTask` still owns one batch state and does not subscribe to the new queue.

- [ ] **Step 3: Implement `useUploadTask` as a queue subscriber**

Create one module-level queue instance using:

```ts
const uploadQueue = createUploadQueue((issueCode, file, onProgress) =>
  rainApi.uploadLogs(issueCode, [file], onProgress)
);
```

Use `useSyncExternalStore` or an equivalent subscription-safe hook to read queue snapshots. `performUpload(files)` must validate the selected Issue and non-empty input, enqueue all files with the current Issue code, and return without serializing the files into one request. Expose the selected Issue’s tasks as `uploadTasks`; derive `uploading` from `QUEUED`, `UPLOADING`, or `RETRY_WAIT` tasks, `uploadFailed` from `FAILED`/`UNCONFIRMED`, `uploadError` from the first terminal task message, and keep `uploadingRef.current` synchronized for existing callers. Keep `resetSelection` as a display reset only; it must not cancel or reassign in-flight tasks.

After an item reaches `ACCEPTED`, refresh its captured Issue with `loadBundles(task.issueCode)` and `loadIssues()`. Guard the callbacks with the task’s Issue code so an old Issue response cannot overwrite the current Issue’s state. Keep the queue alive if the hook unmounts.

- [ ] **Step 4: Convert optimistic rows to task-based rows**

Replace the global `uploadSelection`, `uploadProgress`, and `uploadFailed` inputs with `uploadTasks` in `homeRows.ts`. Give every local row its queue task ID as `key`, render `QUEUED`, `UPLOADING`, `RETRY_WAIT`, `ACCEPTED`, and terminal states, and omit a local accepted row once its response `bundle_hash` already appears in the backend bundle snapshot. This prevents duplicate rows after the first refresh while preserving the local row during the acceptance-to-polling gap.

- [ ] **Step 5: Update `HomeView` and upload controls**

Pass `upload.uploadTasks` to `buildFileRows`. Do not disable the file picker merely because another file is uploading; only lack of an Issue or write permission disables it. Remove the `uploadingRef.current` guards from `UploadPanel` input/drop/click handlers, while retaining the visible “上传中” label when any task is active. Keep Issue deletion blocked while that Issue has queued or in-flight transport tasks.

- [ ] **Step 6: Run hook tests and build**

Run: `npm test -- --run tests/upload-polling.behavior.test.tsx` from `frontend/`.

Expected: PASS, including old Issue isolation and polling tests.

Run: `npm run build` from `frontend/`.

Expected: PASS with no TypeScript errors.

- [ ] **Step 7: Commit hook integration**

```bash
git add frontend/src/features/files/hooks/useUploadTask.ts frontend/src/features/files/uploadRows.ts frontend/src/features/files/homeRows.ts frontend/src/features/files/HomeView.tsx frontend/tests/upload-polling.behavior.test.tsx
git commit -m "feat: show independent upload task states"
```

### Task 4: Add user-visible retry and queue summary behavior

**Files:**
- Modify: `frontend/src/features/files/components/UploadPanel.tsx`
- Modify: `frontend/src/features/files/components/UploadFileTable.tsx`
- Modify: `frontend/src/features/files/hooks/useUploadTask.ts`
- Modify: `frontend/src/features/files/homeRows.ts`
- Modify: `frontend/tests/upload-queue.behavior.test.ts`

- [ ] **Step 1: Write failing UI-state tests**

Add assertions that queued tasks display “等待上传”, active tasks display an individual percentage, accepted tasks display “已接收，等待处理”, retry-wait tasks display “等待重试”, and failed/unconfirmed tasks expose a retry action without retrying automatically.

- [ ] **Step 2: Run the focused UI test and verify it fails**

Run: `npm test -- --run tests/upload-queue.behavior.test.ts` from `frontend/`.

Expected: FAIL because row labels and retry callbacks are not wired yet.

- [ ] **Step 3: Implement labels, counts, and retry action**

Extend `stageLabel`/`stageClass` for local queue stages. Show a compact summary in `UploadPanel`: `上传中 N · 等待 N · 等待处理 N`; do not include backend processing in the transport count. Add a `重试` button to failed/unconfirmed local rows that calls the queue task retry callback. Do not add pause/resume controls in Phase 1 because the multipart endpoint cannot resume bytes safely.

- [ ] **Step 4: Run all frontend tests**

Run: `npm test` from `frontend/`.

Expected: all Vitest suites and existing Node behavior tests PASS.

- [ ] **Step 5: Commit the UI behavior**

```bash
git add frontend/src/features/files/components/UploadPanel.tsx frontend/src/features/files/components/UploadFileTable.tsx frontend/src/features/files/hooks/useUploadTask.ts frontend/src/features/files/homeRows.ts frontend/tests/upload-queue.behavior.test.ts
git commit -m "feat: expose per-file upload progress and retry"
```

### Task 5: Verify the Phase 1 acceptance boundary

**Files:**
- Verify only: `frontend/`
- Reference: `docs/superpowers/specs/2026-09-24-issue-181-upload-design.md`

- [ ] **Step 1: Run formatting and frontend checks**

Run:

```bash
npm run lint
npm run build
npm test
```

Expected: all commands exit successfully.

- [ ] **Step 2: Inspect the diff for protocol and scope regressions**

Run: `git diff --check` and `git diff --stat`.

Confirm that the backend endpoint, archive limits, Issue quota accounting, Tantivy indexing, and Release metadata are unchanged; confirm that multi-file selection now creates one request and one Bundle per selected top-level file.

- [ ] **Step 3: Record the implementation boundary**

Update the design/spec status from “待评审设计；未实现” to “Phase 1 implemented locally; Phase 2 resumable sessions not implemented” only after all checks pass. Do not create a release or push to `main` as part of this implementation unless explicitly requested.

- [ ] **Step 4: Commit the verified Phase 1 change**

```bash
git add docs/superpowers/specs/2026-09-24-issue-181-upload-design.md
git commit -m "docs: mark issue 181 upload queue phase one implemented"
```
