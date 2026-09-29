# Issue #254 Public API Error Semantics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the first set of user-actionable Issue, Bundle, file-deletion, and resumable-upload failures return stable public error codes and safe Chinese messages while keeping internal races, leases, and implementation details sanitized.

**Architecture:** Reuse the existing `AppError::api(...)` and `AppError::public(...)` variants instead of introducing another error hierarchy. Add one backend code catalog for this phase, migrate only errors whose next user action is clear, and leave raw `BadRequest`/`Conflict`/`NotFound` variants generic at the HTTP boundary. The frontend will continue to display server messages, but deletion-specific formatting will branch on stable codes rather than HTTP status alone.

**Tech Stack:** Rust, Actix Web, SQLx/SQLite, TypeScript, React, Vitest, Testing Library.

---

## Scope and public contract

The following codes are in this PR. Messages contain no Issue code, file path, database id, lease token, search generation, or other internal value.

| Code | HTTP | Public message | Migration point |
| --- | ---: | --- | --- |
| `ISSUE_ALREADY_EXISTS` | 409 | `该 Issue 已存在` | duplicate Issue creation |
| `BUNDLE_PROCESSING` | 409 | `文件仍在处理中，请稍后重试` | reading a non-ready processing Bundle |
| `BUNDLE_PROCESSING_FAILED` | 409 | `文件处理失败，请重新上传或删除` | reading a failed Bundle |
| `FILE_DELETE_BUNDLE_BUSY` | 409 | `当前文件仍在处理中，暂不可删除，请等待处理完成后重试` | file deletion while parent Bundle is not READY |
| `FILE_DELETE_ALREADY_RUNNING` | 409 | `当前 Bundle 已有删除任务，请稍后重试` | competing deletion job for the same Bundle |
| `UPLOAD_IDEMPOTENCY_CONFLICT` | 409 | `上传请求标识已用于其他文件，请重新开始上传` | resumable session idempotency key reused with different metadata |
| `UPLOAD_SESSION_NOT_OPEN` | 409 | `上传会话已结束，请重新开始上传` | chunk/complete request against a terminal session |
| `UPLOAD_CHUNK_CONFLICT` | 409 | `上传分片与已提交内容不一致，请重新开始上传` | replayed chunk does not match committed chunk history |
| `UPLOAD_CONTENT_REJECTED` | 422 | `压缩包内容不符合处理要求，请检查后重试` | user-actionable archive validation/extraction rejection |

Existing public codes such as `UPLOAD_OFFSET_CONFLICT`, authentication errors, settings errors, search window errors, and temporary-result errors remain unchanged. Search index readiness/version, Tantivy admission, publication generation, deletion/cleanup leases, upload storage races, and other invariant failures remain generic in this phase.

## File map

- Modify `backend/src/error.rs` — add the phase-1 stable code catalog and preserve generic sanitization tests.
- Modify `backend/src/routes/issues.rs` — expose duplicate Issue creation as `ISSUE_ALREADY_EXISTS`.
- Modify `backend/src/routes/helpers.rs` — expose only the two user-understandable Bundle states.
- Modify `backend/src/services/file_deletion.rs` — expose parent-Bundle busy and competing-job conflicts without leaking job ids.
- Modify `backend/src/routes/upload_sessions.rs` — expose idempotency, terminal-session, and committed-chunk conflicts.
- Modify `backend/src/ingest.rs` and `backend/src/upload/lifecycle.rs` — mark intentionally user-actionable upload content failures as public and stop persisting raw internal error strings as upload failure reasons.
- Create `frontend/src/api/errorCodes.ts` — frontend names for the phase-1 codes used by UI behavior.
- Modify `frontend/src/features/files/deleteFeedback.ts` — map known deletion codes, and leave unknown 409 responses untouched.
- Modify `frontend/tests/delete-feedback.behavior.test.ts` — prove deletion UX no longer guesses from HTTP 409 alone.
- Create `frontend/tests/api-errors.behavior.test.ts` — verify the API client preserves stable code/message, generic conflict sanitization, and network-error wording.
- Modify `backend/tests/smoke.rs` — assert duplicate Issue and processing-Bundle response contracts while retaining HTTP status coverage.
- Modify `backend/src/upload/lifecycle.rs` tests — assert public upload errors remain actionable and raw internal conflicts are sanitized.
- Modify `backend/tests/upload_sessions.rs` — assert the resumable-session contract codes.

## Execution tasks

### Task 1: Add the stable error-code catalog and contract-first tests

**Files:**
- Modify: `backend/src/error.rs`
- Modify: `backend/tests/smoke.rs`
- Modify: `backend/tests/upload_sessions.rs`
- Create: `frontend/src/api/errorCodes.ts`
- Create: `frontend/tests/api-errors.behavior.test.ts`
- Modify: `frontend/tests/delete-feedback.behavior.test.ts`

- [ ] **Step 1: Add failing backend assertions for public codes.**

In the existing duplicate-Issue smoke flow, after the second `POST /api/issues`, assert the response JSON contains `code == "ISSUE_ALREADY_EXISTS"` and `message == "该 Issue 已存在"`. In the existing processing-Bundle flow, assert the file-tree response contains `code == "BUNDLE_PROCESSING"` and the processing message. In the resumable session HTTP conflict case, assert `code == "UPLOAD_IDEMPOTENCY_CONFLICT"` and its safe message.

Run:

```bash
cargo test --test smoke --test upload_sessions
```

Expected: FAIL because the current responses still contain generic `CONFLICT` or the old sanitized message.

- [ ] **Step 2: Add frontend contract tests before implementation.**

Create `frontend/tests/api-errors.behavior.test.ts` with real `rainApi` parsing behavior:

```ts
it('preserves a stable business code and public message from a 409 response', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(
    JSON.stringify({ code: 'ISSUE_ALREADY_EXISTS', message: '该 Issue 已存在' }),
    { status: 409, headers: { 'Content-Type': 'application/json' } }
  )));

  await expect(rainApi.createIssue({ code: 'DUPLICATE' })).rejects.toMatchObject({
    status: 409,
    code: 'ISSUE_ALREADY_EXISTS',
    message: '该 Issue 已存在'
  });
});

it('keeps generic conflict and network errors safe', async () => {
  expect(normalizeApiError(new ApiError('请求冲突', 409, 'CONFLICT'))).toBe('请求冲突');
  expect(normalizeApiError(new Error('Failed to fetch'))).toBe('无法连接 Rain 后端，请确认服务已启动');
});
```

Update `frontend/tests/delete-feedback.behavior.test.ts` so a known `FILE_DELETE_BUNDLE_BUSY` code gets the deletion guidance, while an unknown `ApiError('请求冲突', 409, 'CONFLICT')` returns `请求冲突` instead of being guessed as a processing conflict.

Run:

```bash
npm test -- --run tests/api-errors.behavior.test.ts tests/delete-feedback.behavior.test.ts
```

Expected: FAIL for the new response assertions and code-aware deletion expectation.

- [ ] **Step 3: Add the backend code catalog and frontend code constants.**

Add a `pub mod codes` to `backend/src/error.rs`:

```rust
pub mod codes {
    pub const ISSUE_ALREADY_EXISTS: &str = "ISSUE_ALREADY_EXISTS";
    pub const BUNDLE_PROCESSING: &str = "BUNDLE_PROCESSING";
    pub const BUNDLE_PROCESSING_FAILED: &str = "BUNDLE_PROCESSING_FAILED";
    pub const FILE_DELETE_BUNDLE_BUSY: &str = "FILE_DELETE_BUNDLE_BUSY";
    pub const FILE_DELETE_ALREADY_RUNNING: &str = "FILE_DELETE_ALREADY_RUNNING";
    pub const UPLOAD_IDEMPOTENCY_CONFLICT: &str = "UPLOAD_IDEMPOTENCY_CONFLICT";
    pub const UPLOAD_SESSION_NOT_OPEN: &str = "UPLOAD_SESSION_NOT_OPEN";
    pub const UPLOAD_CHUNK_CONFLICT: &str = "UPLOAD_CHUNK_CONFLICT";
}
```

Create the matching frontend object:

```ts
export const API_ERROR_CODES = {
  bundleProcessing: 'BUNDLE_PROCESSING',
  fileDeleteBundleBusy: 'FILE_DELETE_BUNDLE_BUSY',
  fileDeleteAlreadyRunning: 'FILE_DELETE_ALREADY_RUNNING'
} as const;
```

Keep frontend constants limited to codes that affect current frontend branching; the API client must continue accepting any server code as a string.

- [ ] **Step 4: Run the focused tests and commit the contract scaffolding.**

Run the two focused test commands again. Expected: the frontend parsing test passes once the backend fixture response is represented by the existing parser, while backend assertions remain red until migration tasks are complete. Commit the scaffolding and tests only if the repository workflow permits intermediate commits; otherwise leave the plan checkpoint staged for the next task.

### Task 2: Migrate Issue and Bundle user-actionable errors

**Files:**
- Modify: `backend/src/routes/issues.rs`
- Modify: `backend/src/routes/helpers.rs`
- Modify: `backend/tests/smoke.rs`

- [ ] **Step 1: Replace duplicate Issue conflict with a stable public error.**

Change the `rows_affected() == 0` branch in `create_issue` to:

```rust
return Err(AppError::api(
    StatusCode::CONFLICT,
    codes::ISSUE_ALREADY_EXISTS,
    "该 Issue 已存在",
));
```

Import `error::codes` alongside `AppError`. Do not include the normalized Issue code in the response message.

- [ ] **Step 2: Replace only the understandable Bundle status branches.**

Change `ensure_bundle_ready` so `PROCESSING`/`PENDING` and active processing stages return `BUNDLE_PROCESSING`, `FAILED` returns `BUNDLE_PROCESSING_FAILED`, and the default branch remains raw `AppError::Conflict` so an unknown state is still sanitized.

Use the exact public messages from the contract table. Do not migrate search-index states here; this helper describes Bundle processing state, not index publication state.

- [ ] **Step 3: Run the backend contract tests.**

Run:

```bash
cargo test --test smoke duplicate -- --nocapture
cargo test --lib routes::helpers
```

Expected: the duplicate Issue and processing-Bundle response assertions pass, and an unknown Bundle state remains generic.

### Task 3: Migrate file-deletion conflicts without leaking identifiers

**Files:**
- Modify: `backend/src/services/file_deletion.rs`
- Modify: `frontend/src/features/files/deleteFeedback.ts`
- Modify: `frontend/tests/delete-feedback.behavior.test.ts`
- Modify: `backend/tests/smoke.rs`

- [ ] **Step 1: Write the deletion behavior tests.**

Assert that a deletion request whose parent Bundle is not READY returns `FILE_DELETE_BUNDLE_BUSY` and the safe waiting message. Assert that a second deletion request for a different root in the same active Bundle returns `FILE_DELETE_ALREADY_RUNNING`, and that the response body does not contain the internal job id.

On the frontend, assert these mappings:

```ts
expect(normalizeDeletionError(new ApiError('busy', 409, 'FILE_DELETE_BUNDLE_BUSY')))
  .toBe('当前文件仍在处理中，暂不可删除，请等待处理完成后重试');
expect(normalizeDeletionError(new ApiError('running', 409, 'FILE_DELETE_ALREADY_RUNNING')))
  .toBe('当前 Bundle 已有删除任务，请稍后重试');
expect(normalizeDeletionError(new ApiError('请求冲突', 409, 'CONFLICT')))
  .toBe('请求冲突');
```

Run the focused backend/frontend tests and verify they fail before editing production code.

- [ ] **Step 2: Migrate the two service branches.**

In `enqueue_file_deletion`, return `AppError::api(StatusCode::CONFLICT, codes::FILE_DELETE_BUNDLE_BUSY, ...)` when `parent_ready` is false. When another active deletion job exists for a different root, return `FILE_DELETE_ALREADY_RUNNING` and omit `existing.id` from the public message. Keep missing-file `NotFound` generic in this phase.

- [ ] **Step 3: Make the frontend formatter code-aware.**

Switch `normalizeDeletionError` from `status === 409` to a `switch (error.code)` for the two known deletion codes. Fall back to `normalizeApiError(error)` for unknown codes, non-API errors, and generic conflicts.

- [ ] **Step 4: Run focused tests and the existing deletion regression suite.**

Run:

```bash
cargo test --lib services::file_deletion
cargo test --test smoke rejected_issue_delete_preserves_open_resumable_session
npm test -- --run tests/delete-feedback.behavior.test.ts tests/write-permissions.behavior.test.tsx
```

Expected: all pass, including the existing UI behavior that generic errors are still shown safely.

### Task 4: Migrate resumable-upload user conflicts and sanitize persisted failure reasons

**Files:**
- Modify: `backend/src/routes/upload_sessions.rs`
- Modify: `backend/src/ingest.rs`
- Modify: `backend/src/upload/lifecycle.rs`
- Modify: `backend/tests/upload_sessions.rs`
- Modify: `backend/src/upload/lifecycle.rs` tests

- [ ] **Step 1: Add failing upload contract assertions.**

Extend the existing resumable session HTTP test to assert `UPLOAD_IDEMPOTENCY_CONFLICT` for metadata mismatch. Add requests for a terminal session and a mismatched already-committed chunk, asserting `UPLOAD_SESSION_NOT_OPEN` and `UPLOAD_CHUNK_CONFLICT` respectively. Add a lifecycle unit test proving a raw `AppError::Conflict("secret internal state")` produces the generic upload failure reason and does not leak the string.

For intentionally user-actionable content rejection, replace the existing test that passes a raw `BadRequest` with a test using a public error:

```rust
let error = AppError::api(
    StatusCode::UNPROCESSABLE_ENTITY,
    "UPLOAD_CONTENT_REJECTED",
    "压缩包内容不符合处理要求，请检查后重试",
);
assert_eq!(user_facing_failure_reason(&error), "压缩包内容不符合处理要求，请检查后重试");
```

Run the focused tests and verify the new assertions fail before production edits.

- [ ] **Step 2: Migrate route-level resumable conflicts.**

Use the catalog codes in `create_upload_session` for idempotency metadata mismatch, in `upload_session_chunk` for non-OPEN sessions and committed-chunk mismatch, and in `complete_upload_session` for terminal states. Preserve the existing `UPLOAD_OFFSET_CONFLICT` helper for authoritative offset recovery. Leave storage-behind, finalization races, missing issue, and database uniqueness fallback generic.

- [ ] **Step 3: Stop leaking raw internal errors through upload status.**

Change `user_facing_failure_reason` to return messages only for `AppError::Api` and `AppError::PublicApi`; map raw `BadRequest` and `Conflict` to `上传处理失败，请删除后重试`, just like other internal failures. Convert the deliberately user-actionable archive/content rejection branches in `backend/src/ingest.rs` to `AppError::api` or `AppError::public` with a safe stable code/message before they reach this function. Do not include archive paths, configured internals, or filesystem details in those messages.

- [ ] **Step 4: Run upload tests and inspect failure payloads.**

Run:

```bash
cargo test --test upload_sessions
cargo test --lib upload::lifecycle
cargo test --test smoke failed_upload -- --nocapture
```

Expected: public upload failures keep their stable code/message, raw internal failures persist only generic code/reason, and resumable offset recovery remains unchanged.

### Task 5: Complete frontend contract coverage and run the full verification matrix

**Files:**
- Modify: `frontend/src/api/errorCodes.ts`
- Modify: `frontend/tests/api-errors.behavior.test.ts`
- Modify: `frontend/tests/delete-feedback.behavior.test.ts`
- Modify: `backend/src/error.rs` tests if any contract assertion is still missing

- [ ] **Step 1: Add explicit sanitization and no-status-guessing tests.**

Cover these cases in tests:

```ts
expect(normalizeDeletionError(new ApiError('请求冲突', 409, 'CONFLICT'))).toBe('请求冲突');
expect(normalizeApiError(new ApiError('该 Issue 已存在', 409, 'ISSUE_ALREADY_EXISTS'))).toBe('该 Issue 已存在');
expect(normalizeApiError(new Error('networkerror'))).toBe('无法连接 Rain 后端，请确认服务已启动');
```

Keep the backend `AppError::Conflict("secret internal state")` response assertion and add the same guarantee to upload failure persistence.

- [ ] **Step 2: Run the complete frontend and backend verification matrix.**

Run from the worktree root:

```bash
npm --prefix frontend test
npm --prefix frontend run lint
npm --prefix frontend run build
cargo test --lib
cargo test --test smoke --test upload_sessions --test search_execution
cargo fmt --all -- --check
git diff --check
```

Expected: all commands exit 0. `cargo build` requires `npm --prefix frontend run build` first because the backend embeds `frontend/dist`.

- [ ] **Step 3: Review the final diff against the scope table.**

Confirm that every new public code is stable, every public message is free of internal identifiers, unknown `BadRequest`/`Conflict`/`NotFound` variants remain sanitized, frontend branching never maps all 409 responses to one business meaning, and search-index/internal concurrency migrations are absent from this PR.

- [ ] **Step 4: Commit the implementation.**

```bash
git add backend/src/error.rs backend/src/routes/issues.rs backend/src/routes/helpers.rs backend/src/services/file_deletion.rs backend/src/routes/upload_sessions.rs backend/src/ingest.rs backend/src/upload/lifecycle.rs backend/tests/smoke.rs backend/tests/upload_sessions.rs frontend/src/api/errorCodes.ts frontend/src/features/files/deleteFeedback.ts frontend/tests/api-errors.behavior.test.ts frontend/tests/delete-feedback.behavior.test.ts docs/superpowers/plans/2026-09-29-issue-254-public-api-errors.md
git commit -m "fix: expose actionable business error codes"
```

- [ ] **Step 5: Push and create the PR from latest main.**

```bash
git push -u origin feat/issue-254-public-api-errors
gh pr create --title "fix: expose actionable business error codes" --body "Closes #254 ..."
```

The PR body must list the stable codes, the sanitized internal-error boundary, the test commands, and the explicit phase-2 follow-up for search-index and internal concurrency states.

## Self-review checklist

- [ ] No raw internal error string is added to an HTTP response or persisted upload failure reason.
- [ ] Public errors use the existing `AppError::api/public` contract and stable code catalog.
- [ ] Frontend special handling uses `ApiError.code`, never only HTTP status.
- [ ] Duplicate Issue, Bundle processing, deletion, upload session, generic sanitization, and network-error behavior are covered by tests.
- [ ] The implementation stays within phase 1; search-index and lease/race details remain generic.
- [ ] The plan contains no unresolved placeholders or unspecified code paths.
