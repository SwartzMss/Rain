# Issue #181 Phase 2 Resumable Upload Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add durable, sequential 8 MiB upload sessions so large files can resume after interrupted requests, browser reloads, and server restarts without changing archive validation or Bundle processing semantics.

**Architecture:** Files below 64 MiB continue using the Phase 1 multipart request. Larger files create a database-backed upload session and write chunks to `data_root/.uploads/{session_id}/input.part`; the session owns transfer metadata and capacity reservation until a durable finalization handoff creates one normal Bundle. A per-session async mutex serializes append, complete, cancel, and cleanup operations; the existing upload worker remains responsible for archive preflight, extraction, indexing, and publication.

**Tech Stack:** Rust 2024, Actix Web, Tokio, SQLite/SQLx migrations, SHA-256, existing upload lifecycle/job code, React/TypeScript, IndexedDB and Web Crypto.

---

### Task 1: Add the persistent session schema and pure session models

**Files:**
- Create: `backend/migrations/0010_upload_sessions.sql`
- Create: `backend/src/upload/session.rs`
- Modify: `backend/src/upload/mod.rs`
- Modify: `backend/src/lib.rs:104-141`
- Test: `backend/src/upload/session.rs` unit tests

- [x] **Step 1: Write failing database and model tests**

Add tests that run against a reset SQLite schema and assert:

```rust
#[tokio::test]
async fn session_schema_tracks_committed_offset_and_idempotency() {
    let pool = test_pool().await;
    let session = create_session_row(&pool, CreateSessionRow {
        id: "session-1".into(),
        issue_code: "SESSION".into(),
        owner_user_id: "user-1".into(),
        idempotency_key: "key".into(),
        file_name: "large.log".into(),
        file_size_bytes: 64 * 1024 * 1024,
        last_modified_ms: Some(1),
        chunk_size_bytes: 8 * 1024 * 1024,
        input_path: ".uploads/session-1/input.part".into(),
        expires_at: "2099-01-01 00:00:00".into(),
    }).await.unwrap();
    assert_eq!(session.committed_offset, 0);
    let same = find_by_idempotency(&pool, &session.owner_user_id, &session.issue_code, "key").await.unwrap();
    assert_eq!(same.id, session.id);
}

#[test]
fn expected_chunk_size_rejects_overflow_and_accepts_short_final_chunk() {
    assert_eq!(expected_chunk_size(17 * 1024 * 1024, 8 * 1024 * 1024, 0).unwrap(), 8 * 1024 * 1024);
    assert_eq!(expected_chunk_size(17 * 1024 * 1024, 8 * 1024 * 1024, 2).unwrap(), 1 * 1024 * 1024);
    assert!(expected_chunk_size(17 * 1024 * 1024, 8 * 1024 * 1024, 3).is_err());
}
```

The test fixture must create Issue `SESSION` and user `user-1` through the existing repository helpers; the test must also assert that reusing `key` with `file_size_bytes = 64 * 1024 * 1024 + 1` returns a conflict rather than the old session.

- [x] **Step 2: Run the focused backend tests and verify the intended failure**

Run: `cargo test -p backend upload::session::tests -- --nocapture` from `backend/`.

Expected: FAIL because migration `0010_upload_sessions.sql` and `upload::session` do not exist.

- [x] **Step 3: Create migration 0010**

Create these tables and indexes:

```sql
CREATE TABLE IF NOT EXISTS upload_sessions (
    id TEXT PRIMARY KEY,
    issue_code TEXT NOT NULL REFERENCES issues(code) ON DELETE CASCADE,
    owner_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    idempotency_key TEXT NOT NULL,
    file_name TEXT NOT NULL,
    file_size_bytes INTEGER NOT NULL CHECK (file_size_bytes >= 0),
    last_modified_ms INTEGER,
    chunk_size_bytes INTEGER NOT NULL CHECK (chunk_size_bytes > 0),
    committed_offset INTEGER NOT NULL DEFAULT 0 CHECK (committed_offset >= 0),
    next_chunk_index INTEGER NOT NULL DEFAULT 0 CHECK (next_chunk_index >= 0),
    status TEXT NOT NULL CHECK (status IN ('OPEN','FINALIZING','DELIVERED','CANCELLED','EXPIRED','FAILED')),
    input_path TEXT NOT NULL UNIQUE,
    bundle_id TEXT UNIQUE REFERENCES bundles(id) ON DELETE SET NULL,
    failure_code TEXT,
    failure_reason TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at TEXT NOT NULL,
    UNIQUE(owner_user_id, issue_code, idempotency_key)
);

CREATE TABLE IF NOT EXISTS upload_session_chunks (
    session_id TEXT NOT NULL REFERENCES upload_sessions(id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
    offset_bytes INTEGER NOT NULL CHECK (offset_bytes >= 0),
    size_bytes INTEGER NOT NULL CHECK (size_bytes > 0),
    sha256 TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY(session_id, chunk_index)
);

CREATE INDEX IF NOT EXISTS idx_upload_sessions_owner_issue
    ON upload_sessions(owner_user_id, issue_code, status, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_upload_sessions_finalizing
    ON upload_sessions(status, updated_at);
```

The input path is always generated from the session UUID; never accept a client path. Keep the session table outside the existing Bundle status machine.

- [x] **Step 4: Implement session rows, validation, locks, and capacity helpers**

Define `UploadSession`, `UploadSessionChunk`, `SessionStatus`, `CHUNK_SIZE_BYTES = 8 * 1024 * 1024`, `SESSION_MIN_FILE_SIZE_BYTES = 64 * 1024 * 1024`, `SESSION_MAX_AGE_SECONDS = 7 * 24 * 60 * 60`, and `SESSION_IDLE_AGE_SECONDS = 24 * 60 * 60`. Add:

```rust
pub fn expected_chunk_size(file_size: u64, chunk_size: u64, index: u64) -> Result<u64, AppError>;
pub async fn create_session(...);
pub async fn get_session(...);
pub async fn list_sessions(...);
pub async fn record_chunk(...);
pub async fn mark_finalizing(...);
pub async fn mark_delivered(...);
pub async fn cancel_session(...);
```

Add `UploadRuntime.session_locks: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>` and a helper returning a stable lock per session. Add persistent temp-budget methods beside `TempBudget` so session creation reserves declared bytes, startup can rebuild the atomic total from active session rows, and delivery/cancel/expiry releases the reservation exactly once. Capacity checks must run in the existing `db::write::run` admission path and include active old multipart bytes plus nonterminal session declarations.

- [x] **Step 5: Run focused tests and format**

Run: `cargo fmt --check` and `cargo test -p backend upload::session::tests -- --nocapture` from `backend/`.

Expected: formatting passes and all schema/model tests pass.

- [x] **Step 6: Commit the schema/model layer**

```bash
git add backend/migrations/0010_upload_sessions.sql backend/src/upload/session.rs backend/src/upload/mod.rs backend/src/lib.rs
git commit -m "feat: add durable upload session storage"
```

### Task 2: Add authenticated session lifecycle endpoints

**Files:**
- Create: `backend/src/routes/upload_sessions.rs`
- Modify: `backend/src/routes/mod.rs:14-18,190-241`
- Modify: `backend/src/upload/session.rs`
- Test: `backend/tests/upload_sessions.rs`

- [x] **Step 1: Write failing HTTP tests**

Cover these concrete cases with the existing Actix test fixture:

1. `POST /api/issues/SESSION/upload-sessions` returns `201` with `session_id`, `chunk_size_bytes`, `committed_offset: 0`, `status: OPEN`, `expires_at`, and an idempotency replay returns the same `session_id`.
2. The same idempotency key with a different file size returns `409`.
3. A different logged-in user receives `404`/`403` and cannot list, inspect, complete, or delete the session.
4. `GET /api/issues/{code}/upload-sessions` only lists the owner’s nonterminal sessions; `GET /api/upload-sessions/{id}` returns the exact authoritative offset.
5. `DELETE` is idempotent for OPEN sessions and returns conflict with the delivered `task_id` after delivery.

- [x] **Step 2: Run the focused tests and verify they fail**

Run: `cargo test --test upload_sessions -- --nocapture` from `backend/`.

Expected: FAIL because the routes are not registered.

- [x] **Step 3: Implement create/list/get/delete handlers**

Use `RequireBusinessUser`, `normalize_issue_code`, and `require_issue_owner`. The create JSON body is:

```rust
#[derive(Deserialize)]
struct CreateUploadSessionRequest {
    file_name: String,
    file_size_bytes: u64,
    last_modified_ms: Option<i64>,
    idempotency_key: String,
}
```

Reject empty/oversized names, negative/overflow values, file sizes above the existing per-request upload maximum, and idempotency keys outside a bounded 1–128 byte range. Create the `.uploads/{session_id}` directory and empty input file using server-generated names. On a database or capacity failure, remove the just-created directory and release the reservation. Return `Cache-Control: no-store, private` for all session responses.

- [x] **Step 4: Register routes and verify lifecycle tests**

Register `upload_sessions` in the `/api` scope. Run: `cargo test --test upload_sessions -- --nocapture`.

Expected: all lifecycle and ownership tests pass.

- [x] **Step 5: Commit the lifecycle API**

```bash
git add backend/src/routes/upload_sessions.rs backend/src/routes/mod.rs backend/src/upload/session.rs backend/tests/upload_sessions.rs
git commit -m "feat: expose resumable upload session lifecycle"
```

### Task 3: Implement sequential chunk writes and durable completion handoff

**Files:**
- Modify: `backend/src/routes/upload_sessions.rs`
- Modify: `backend/src/upload/session.rs`
- Modify: `backend/src/upload/multipart.rs`
- Modify: `backend/src/upload/job.rs`
- Modify: `backend/src/upload/lifecycle.rs`
- Modify: `backend/src/main.rs`
- Test: `backend/tests/upload_sessions.rs`

- [x] **Step 1: Write failing chunk and recovery tests**

Add three tests. The first sends the first 8 MiB with the correct offset and SHA-256, repeats the same chunk and expects the same confirmed offset, sends a conflicting duplicate and expects 409, sends a future offset and expects 409 with the authoritative offset, sends a bad SHA-256 and verifies the file length is unchanged, then sends the short final chunk. The second completes the session, repeats complete after discarding the first response, and asserts one DELIVERED session and one Bundle row. The third writes 1 MiB beyond the database offset, runs startup reconciliation, and asserts the input is truncated to the committed offset.

- [x] **Step 2: Run tests and verify the new behavior fails**

Run: `cargo test --test upload_sessions -- --nocapture` from `backend/`.

Expected: FAIL because chunk and complete endpoints are not present.

- [x] **Step 3: Implement raw sequential chunk handling**

Add `PUT /api/upload-sessions/{session_id}/chunks/{chunk_index}` with:

- `X-Upload-Offset` and `X-Chunk-SHA256` required headers;
- exact `Content-Length` equal to the server-computed chunk size;
- session lock held across authority check, file length reconciliation, stream write, `sync_data`, and the short database transaction;
- SHA-256 calculated while streaming without collecting the chunk in memory;
- duplicate `(session_id, chunk_index)` with matching offset/length/hash returns the prior authoritative offset without appending;
- conflicting duplicate returns `409`, a future offset/index returns `409` with authoritative offset;
- failed hash or database commit truncates back to the previous committed offset;
- no SQLite write transaction held during filesystem I/O.

Return `{ session_id, status, committed_offset, next_chunk_index, expires_at }` and do not extend expiry on GET or duplicate old-chunk requests.

- [x] **Step 4: Implement complete and the persistent finalizer**

Add `POST /api/upload-sessions/{session_id}/complete`. Under the session lock, require all bytes committed, transition OPEN to FINALIZING idempotently, and return `202` with the session status. A background periodic worker claims FINALIZING sessions and:

1. verifies the final file length and SHA-256;
2. creates exactly one normal Bundle row and a unique session-to-bundle association in one SQLite transaction;
3. moves the input into an isolated `.tmp/{upload_id}` processing directory and creates the existing `UploadedFile` metadata;
4. transfers the persistent temp reservation to an existing `ReceiveReservation` and calls `spawn_upload_job`;
5. marks the session DELIVERED with the task/bundle identifiers only after the handoff record is durable.

Add startup recovery for FINALIZING sessions and handoff records before accepting requests. Change stale Bundle recovery so it does not mark a not-yet-delivered session’s processing record as an unrelated interrupted Bundle. If the Issue is deleted during finalization, cancel the session and remove its input safely.

- [ ] **Step 5: Run backend formatting, focused tests, and existing upload tests**

Run:

```bash
cargo fmt --check
cargo test --test upload_sessions -- --nocapture
cargo test upload:: -- --nocapture
```

Expected: all commands pass, including the existing multipart upload and archive-limit tests.

- [ ] **Step 6: Commit chunking and handoff**

```bash
git add backend/src/routes/upload_sessions.rs backend/src/upload/session.rs backend/src/upload/multipart.rs backend/src/upload/job.rs backend/src/upload/lifecycle.rs backend/src/main.rs backend/tests/upload_sessions.rs
git commit -m "feat: support durable resumable upload chunks"
```

### Task 4: Add frontend resumable transport and browser metadata recovery

**Files:**
- Modify: `frontend/src/api/types.ts`
- Modify: `frontend/src/api/client.ts`
- Create: `frontend/src/features/files/resumableUpload.ts`
- Modify: `frontend/src/features/files/uploadQueue.ts`
- Modify: `frontend/src/features/files/hooks/useUploadTask.ts`
- Test: `frontend/tests/resumable-upload.behavior.test.ts`

- [ ] **Step 1: Write failing transport tests**

Test the real resumable operation with mocked `rainApi` responses:

Add four tests: a 17 MiB `File` must produce two 8 MiB requests and one 1 MiB request while progress uses the confirmed server offset; a rejected chunk response must trigger GET and continue from the returned offset; a same-name/same-size file with a different confirmed-prefix hash must stop with a reselect error; and the IndexedDB record must contain session metadata and chunk hashes but no file bytes.

- [ ] **Step 2: Run the focused frontend test and verify it fails**

Run: `npx vitest run tests/resumable-upload.behavior.test.ts` from `frontend/`.

Expected: FAIL because session API types, IndexedDB metadata, and transport do not exist.

- [ ] **Step 3: Add API types and client methods**

Add `UploadSessionResponse`, `UploadSessionListResponse`, `UploadChunkResponse`, and `UploadSessionCompleteResponse` types. Add client methods for create/list/get/chunk/complete/delete using `fetch`, `PUT` raw bytes, `X-Upload-Offset`, `X-Chunk-SHA256`, and `Content-Length`. Treat `409` as a structured offset/hash conflict and preserve `Retry-After` from `429`.

- [ ] **Step 4: Implement resumable transport and IndexedDB metadata**

Create a small module that:

- chooses sessions for files `>= 64 MiB` and leaves smaller files on `uploadLogs`;
- stores only session ID, Issue, name, size, lastModified, chunk size, confirmed offset, and per-chunk SHA-256 in IndexedDB;
- reads one chunk at a time, hashes it with `crypto.subtle.digest`, sends it, and updates metadata only after the server confirms the offset;
- on a lost response, GETs the session, verifies every confirmed prefix chunk hash against the newly selected file, and continues at the authoritative offset;
- retries network/5xx failures with bounded backoff, never counts retransmitted bytes twice, and stops on 401/403/409/410 with a user-visible message;
- removes metadata only after the session becomes DELIVERED or is explicitly cancelled.

The resumable operation must expose `onProgress(sentBytes, confirmedBytes, totalBytes)` so the Phase 1 queue can display confirmed transfer progress and preserve the existing two-file concurrency limit.

- [ ] **Step 5: Integrate session selection and recovery into the hook**

Use the resumable operation in the existing global queue for large files. On a new page load, list recoverable sessions for the current Issue and associate a selected local file only after metadata and confirmed-prefix hashes match. Keep backend processing polling separate; a delivered session yields the existing Bundle task ID.

- [ ] **Step 6: Run focused and complete frontend tests**

Run:

```bash
npx vitest run tests/resumable-upload.behavior.test.ts tests/upload-queue.behavior.test.ts tests/upload-polling.behavior.test.tsx
npm run lint
npm run build
npm test
```

Expected: all tests pass and Phase 1 queue tests remain green.

- [ ] **Step 7: Commit frontend resumable transport**

```bash
git add frontend/src/api/types.ts frontend/src/api/client.ts frontend/src/features/files/resumableUpload.ts frontend/src/features/files/uploadQueue.ts frontend/src/features/files/hooks/useUploadTask.ts frontend/tests/resumable-upload.behavior.test.ts
git commit -m "feat: resume large uploads from confirmed chunks"
```

### Task 5: Add cleanup, ownership, and end-to-end recovery verification

**Files:**
- Modify: `backend/src/routes/upload_sessions.rs`
- Modify: `backend/src/routes/issues.rs`
- Modify: `backend/src/main.rs`
- Modify: `backend/src/upload/session.rs`
- Modify: `backend/tests/upload_sessions.rs`
- Modify: `docs/superpowers/specs/2026-09-24-issue-181-upload-design.md`

- [ ] **Step 1: Add cleanup and deletion tests**

Verify session expiry after idle 24 hours and hard age 7 days, no expiry extension from status reads or duplicate old chunks, Issue deletion invalidates and removes session input, cleanup failures retain capacity reservations, and a lower temp-space limit blocks new sessions without deleting active ones.

- [ ] **Step 2: Implement periodic session cleanup and startup reconciliation**

Add a periodic worker alongside existing cleanup jobs. It must lock a session, transition it to EXPIRED/CANCELLED before filesystem deletion, release capacity only after successful deletion, and retry failed removals. Startup must reconcile session rows with `.uploads`, truncate tails beyond confirmed offsets, mark missing inputs FAILED, and rebuild the persistent temp budget before readiness opens.

- [ ] **Step 3: Run the full repository verification**

Run from the repository root:

```bash
git diff --check
cargo fmt --check
cargo check
cargo test
(cd frontend && npm test)
(cd frontend && npm run lint && npm run build)
```

Expected: all backend and frontend commands pass; existing archive-bomb, quota, ownership, Windows cleanup, and Phase 1 queue tests remain green.

- [ ] **Step 4: Update the design status and commit documentation**

Change the Phase 2 status in `docs/superpowers/specs/2026-09-24-issue-181-upload-design.md` to record the exact implemented endpoint and recovery boundary. Do not claim multi-process same-session support, background indexing resume, or automatic browser file access after reload unless separately implemented and tested.

```bash
git add backend/tests/upload_sessions.rs backend/src/routes/upload_sessions.rs backend/src/routes/issues.rs backend/src/main.rs backend/src/upload/session.rs docs/superpowers/specs/2026-09-24-issue-181-upload-design.md
git commit -m "test: verify resumable upload cleanup and recovery"
```
