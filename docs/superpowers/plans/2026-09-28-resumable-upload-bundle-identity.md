# Resumable Upload Bundle Identity Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Return the public `bundles.hash` in resumable upload-session responses and make the frontend use it for upload-task lookup, keeping it distinct from the internal `bundles.id`.

**Architecture:** Extend the existing `UploadSession` database projection with an optional joined `bundle_hash` field from `bundles.hash`; expose that field in every session response while retaining `bundle_id` for compatibility. Change both resumable-upload delivery paths to require and pass `bundle_hash` to `GET /api/uploads/{task_id}`.

**Tech Stack:** Rust 2024, Actix Web, SQLite/SQLx, React/TypeScript, Vitest.

---

### Task 1: Add a backend public-hash projection and regression test

**Files:**
- Modify: `backend/src/upload/session.rs`
- Modify: `backend/src/routes/upload_sessions.rs`
- Modify: `backend/tests/upload_sessions.rs`

- [ ] **Step 1: Write the failing backend contract assertion**

In `complete_handoff_creates_one_bundle_and_startup_reconciles_tail_bytes`, preserve the auth token for a second request by passing `token.clone()` to the complete request. After `finalize_one` has marked the session delivered, load the bundle hash and request the session endpoint:

```rust
let bundle_hash: String = sqlx::query_scalar("SELECT hash FROM bundles WHERE id=?")
    .bind(&delivered.1)
    .fetch_one(&pool)
    .await
    .unwrap();
assert_ne!(delivered.1, bundle_hash);

let delivered_response = actix_test::call_service(
    &app,
    actix_test::TestRequest::get()
        .uri(&format!("/api/upload-sessions/{session_id}"))
        .cookie(Cookie::new(SESSION_COOKIE_NAME, token))
        .to_request(),
)
.await;
assert_eq!(
    delivered_response.status(),
    actix_web::http::StatusCode::OK
);
let delivered_body: serde_json::Value = actix_test::read_body_json(delivered_response).await;
assert_eq!(delivered_body["bundle_id"], delivered.1);
assert_eq!(delivered_body["bundle_hash"], bundle_hash);
```

- [ ] **Step 2: Run the focused backend test and confirm it fails**

Run from `backend/`:

```bash
cargo test --test upload_sessions complete_handoff_creates_one_bundle_and_startup_reconciles_tail_bytes -- --nocapture
```

Expected: FAIL because the response does not contain `bundle_hash`.

- [ ] **Step 3: Extend the upload-session row model**

Add `bundle_hash: Option<String>` to both `UploadSession` and the private `UploadSessionRow`, and copy it in `TryFrom<UploadSessionRow> for UploadSession` immediately after `bundle_id`.

Update every session-row SELECT in `backend/src/upload/session.rs` to use this projection shape:

```sql
SELECT upload_sessions.id,
       upload_sessions.issue_code,
       upload_sessions.owner_user_id,
       upload_sessions.idempotency_key,
       upload_sessions.file_name,
       upload_sessions.file_size_bytes,
       upload_sessions.last_modified_ms,
       upload_sessions.chunk_size_bytes,
       upload_sessions.committed_offset,
       upload_sessions.next_chunk_index,
       upload_sessions.status,
       upload_sessions.input_path,
       upload_sessions.bundle_id,
       bundles.hash AS bundle_hash,
       upload_sessions.failure_code,
       upload_sessions.failure_reason,
       upload_sessions.created_at,
       upload_sessions.updated_at,
       upload_sessions.expires_at
FROM upload_sessions
LEFT JOIN bundles ON bundles.id = upload_sessions.bundle_id
```

Preserve each existing WHERE/ORDER BY clause, prefixing session columns where the join could make a name ambiguous. Apply this to `get_session`, `find_by_idempotency`, `list_sessions`, `list_issue_sessions`, `list_finalizing`, `list_recoverable`, `list_expired`, and the private transactional `load_by_id`.

- [ ] **Step 4: Expose `bundle_hash` in HTTP responses**

Add `bundle_hash: Option<String>` to `UploadSessionResponse` and populate it in both `session_response` and `From<UploadSession>`. Leave `bundle_id` unchanged so existing clients remain compatible.

- [ ] **Step 5: Run the backend regression test and formatting**

Run from `backend/`:

```bash
cargo fmt --check
cargo test --test upload_sessions complete_handoff_creates_one_bundle_and_startup_reconciles_tail_bytes -- --nocapture
```

Expected: formatting succeeds and the contract assertion passes with different `bundle_id` and `bundle_hash` values.

- [ ] **Step 6: Commit the backend contract change**

```bash
git add backend/src/upload/session.rs backend/src/routes/upload_sessions.rs backend/tests/upload_sessions.rs
git commit -m "fix: expose public hash for resumable upload sessions"
```

### Task 2: Make the frontend use only the public hash

**Files:**
- Modify: `frontend/src/api/types.ts`
- Modify: `frontend/src/features/files/resumableUpload.ts`
- Modify: `frontend/tests/resumable-upload.behavior.test.ts`

- [ ] **Step 1: Change the frontend regression fixture to distinguish identifiers**

In the existing large-file test, define two values:

```ts
const internalBundleId = 'internal-bundle-1';
const publicBundleHash = 'public-hash-1';
```

Return both values from the delivered session mock, return `publicBundleHash` as `task_id` and `bundle_hash`, and add:

```ts
expect(api.fetchUploadTask).toHaveBeenCalledWith(publicBundleHash);
expect(api.fetchUploadTask).not.toHaveBeenCalledWith(internalBundleId);
```

- [ ] **Step 2: Run the focused frontend test and confirm it fails**

Run from `frontend/`:

```bash
npx vitest run tests/resumable-upload.behavior.test.ts
```

Expected: FAIL because the current implementation passes the internal `bundle_id` and the API type does not yet declare `bundle_hash`.

- [ ] **Step 3: Add the optional response field**

Add `bundle_hash?: string | null;` to `UploadSessionResponse` next to `bundle_id`, matching the backend’s `Option<String>` JSON representation. Do not remove `bundle_id`.

- [ ] **Step 4: Require `bundle_hash` in both delivery paths**

Add a small local helper in `resumableUpload.ts`:

```ts
function deliveredBundleHash(session: UploadSessionResponse): string {
  const bundleHash = session.bundle_hash?.trim();
  if (!bundleHash) throw new Error('服务器未返回已交付的上传任务');
  return bundleHash;
}
```

Use it in the early `DELIVERED` branch and after `waitForDelivery`; call `rainApi.fetchUploadTask(deliveredBundleHash(session))`. Remove the `bundle_id` fallback and retain the existing error text for missing identifiers.

- [ ] **Step 5: Run focused frontend tests and type checking**

Run from `frontend/`:

```bash
npx vitest run tests/resumable-upload.behavior.test.ts
npm run lint
```

Expected: the regression test passes and TypeScript reports no errors.

- [ ] **Step 6: Commit the frontend contract change**

```bash
git add frontend/src/api/types.ts frontend/src/features/files/resumableUpload.ts frontend/tests/resumable-upload.behavior.test.ts
git commit -m "fix: query resumable tasks by public bundle hash"
```

### Task 3: Verify the complete change and prepare the pull request

**Files:**
- No additional source files expected.

- [ ] **Step 1: Run the full relevant frontend suite**

Run from `frontend/`:

```bash
npm test
npm run build
```

Expected: both commands exit 0.

- [ ] **Step 2: Run the full backend test suite and formatting**

Run from `backend/`:

```bash
cargo fmt --check
cargo test
```

Expected: both commands exit 0. If the repository’s integration tests require environment setup, report the exact failing command and output rather than masking it.

- [ ] **Step 3: Inspect the final diff and verify scope**

Run from the repository root:

```bash
git diff --check origin/main...HEAD
git diff --stat origin/main...HEAD
git status --short
```

Confirm that the two pre-existing untracked plan documents remain untouched, no `WHERE hash = ? OR id = ?` fallback was added, and the frontend contains no `fetchUploadTask(session.bundle_id)` calls.

- [ ] **Step 4: Request review and create the PR**

Use the code-review workflow against `origin/main...HEAD`, address any Critical or Important findings, then push and create the PR:

```bash
git push -u origin codex/fix-resumable-upload-identifiers
gh pr create \
  --title "fix: use public bundle hash for resumable uploads" \
  --body "$(cat <<'EOF'
## Summary
- expose `bundles.hash` as `bundle_hash` on upload-session responses
- query resumable upload tasks with the public hash instead of internal `bundle_id`
- add regression coverage with distinct internal and public identifiers

## Test plan
- `npm test`
- `npm run build`
- `cargo fmt --check`
- `cargo test`
EOF
)"
```
