# Authentication Response and Session Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore controlled business feedback, eliminate unnecessary Session activity writes, prevent caching of Session-dependent responses, and rate-limit failed password changes per user.

**Architecture:** Keep internal and unaudited errors generic, while introducing an owned-message public API error for explicitly controlled business details. Extend the existing Session repository and authentication rate-limit structures without database migrations, and apply cache policy through the `/api` scope response middleware.

**Tech Stack:** Rust 2024, Actix Web, SQLx/SQLite, Tokio, Chrono, existing in-memory authentication rate-limit buckets.

---

### Task 1: Restore controlled business error messages

**Files:**
- Modify: `backend/src/error.rs`
- Modify: `backend/src/routes/temp_results.rs`
- Modify: `backend/src/ingest/quota.rs`
- Modify: `backend/tests/smoke.rs`

- [ ] **Step 1: Write failing error-contract tests**

Add an `AppError::public` test with an owned `String` and restore smoke assertions that invalid detailed-search expressions include `搜索条件无效` and `位置`. Extend the existing Issue quota test to assert that the HTTP-safe error message contains the limit, current usage, and requested size.

- [ ] **Step 2: Run focused tests and verify RED**

Run:

```bash
cd backend
cargo test public_business_errors_keep_controlled_owned_messages --lib
cargo test upload_search_tree_and_delete_issue --test smoke
cargo test issue_quota --lib
```

Expected: the new constructor/variant is missing, and the smoke test receives the generic `请求无效` response.

- [ ] **Step 3: Add the controlled public error variant**

Extend `AppError` with an owned-message variant:

```rust
PublicApi {
    status: StatusCode,
    code: &'static str,
    message: String,
}
```

Add `AppError::public(status, code, message)` and serialize it with the same `code`/`message` shape as `Api`. Leave `Database`, `Io`, `Config`, and generic `BadRequest` sanitization unchanged.

- [ ] **Step 4: Migrate only audited business messages**

Change detailed-search parse failures to:

```rust
AppError::public(
    StatusCode::BAD_REQUEST,
    "SEARCH_EXPRESSION_INVALID",
    format!(
        "搜索条件无效，请检查 AND/OR/NOT 前后是否都有关键词（位置 {}：{}）",
        error.offset, error.message
    ),
)
```

Change Issue quota overflow to:

```rust
AppError::public(
    StatusCode::BAD_REQUEST,
    "ISSUE_QUOTA_EXCEEDED",
    format!(
        "Issue 内容超过 {} 上限；当前已使用 {}，本次新增内容至少 {}",
        format_binary_size(self.limit),
        format_binary_size(usage.max(0) as u64),
        format_binary_size(bytes as u64)
    ),
)
```

Do not expose other `BadRequest` strings.

- [ ] **Step 5: Run focused tests and verify GREEN**

Run the three focused commands from Step 2. Expected: all pass.

### Task 2: Skip fresh `last_seen_at` writes

**Files:**
- Modify: `backend/src/repositories/sessions.rs`

- [ ] **Step 1: Write failing timestamp decision tests**

Add tests for a helper accepting `Option<&str>` and a fixed `DateTime<Utc>`:

```rust
assert!(!last_seen_needs_update(Some("2026-07-30 10:00:00"), now));
assert!(last_seen_needs_update(Some("2026-07-30 09:54:59"), now));
assert!(last_seen_needs_update(None, now));
assert!(last_seen_needs_update(Some("invalid"), now));
```

Add a repository test proving a fresh Session resolution leaves its exact `last_seen_at` value unchanged and a stale Session advances it.

- [ ] **Step 2: Run the focused repository tests and verify RED**

Run:

```bash
cd backend
cargo test last_seen --lib
```

Expected: the decision helper is missing and the current lookup does not expose the timestamp needed for the pre-check.

- [ ] **Step 3: Select and evaluate `last_seen_at` before UPDATE**

Extend the Session lookup tuple/query to include `user_sessions.last_seen_at`. Parse SQLite timestamps as UTC using `%Y-%m-%d %H:%M:%S`; absent or invalid values are stale. Execute the existing conditional `UPDATE` only when the Rust helper reports stale. Keep the SQL time condition as a concurrent-update guard and keep update failure best-effort.

- [ ] **Step 4: Run focused tests and verify GREEN**

Run `cargo test last_seen --lib`. Expected: all timestamp and repository cases pass.

### Task 3: Prevent caching of identity and personal responses

**Files:**
- Modify: `backend/src/routes/mod.rs`
- Modify: `backend/tests/auth.rs`

- [ ] **Step 1: Write failing integration tests**

Assert the exact header value `no-store, private` on:

- guest and authenticated `GET /api/auth/me`;
- an error response from `POST /api/auth/change-password`;
- authenticated `GET /api/me/saved-searches`.

Also assert an unrelated public endpoint does not receive this policy.

- [ ] **Step 2: Run focused integration tests and verify RED**

Run:

```bash
cd backend
cargo test session_dependent_responses_are_not_cacheable --test auth
```

Expected: the Session-dependent responses do not contain `Cache-Control`.

- [ ] **Step 3: Add scoped response middleware**

Add an Actix `from_fn` middleware around the `/api` scope. After awaiting the inner service, insert:

```rust
Cache-Control: no-store, private
```

when the request path starts with `/api/auth/` or `/api/me/`. Preserve unrelated API response headers and ensure the middleware processes both successful and error responses.

- [ ] **Step 4: Run the focused test and verify GREEN**

Run the command from Step 2. Expected: all cache-policy assertions pass.

### Task 4: Limit failed password changes per user

**Files:**
- Modify: `backend/src/lib.rs`
- Modify: `backend/src/routes/auth.rs`
- Modify: `backend/tests/auth.rs`

- [ ] **Step 1: Write failing rate-limit tests**

Add policy tests proving:

- the sixth failure within 15 minutes returns `TOO_MANY_REQUESTS`;
- failures expire after 15 minutes;
- a successful current-password verification clears the user bucket;
- filling the password-change map does not consume login-IP, failed-username, or registration-IP maps.

Add an integration test that seeds five failures for an authenticated user and confirms the next password-change request returns 429 before changing the password.

- [ ] **Step 2: Run focused tests and verify RED**

Run:

```bash
cd backend
cargo test password_change_failure --lib
cargo test password_change_failure --test auth
```

Expected: the policy, map, and clearing behavior do not exist.

- [ ] **Step 3: Add an independent policy and map**

Extend `AuthRateLimits` with:

```rust
pub change_password_user_failure: HashMap<String, AuthRateLimitBucket>
```

Add:

```rust
const CHANGE_PASSWORD_FAILURE_WINDOW: Duration = Duration::from_secs(15 * 60);
const CHANGE_PASSWORD_FAILURE_LIMIT: usize = 5;
const CHANGE_PASSWORD_FAILURE_MAX_BUCKETS: usize = 1024;
```

Add `AuthRateLimitPolicy::ChangePasswordUserFailure`, use a key derived from the authenticated user ID, and add a helper that removes the user bucket after successful verification.

- [ ] **Step 4: Enforce before Argon2 and record failures**

At password-change entry, check the user bucket without recording before any current-password Argon2 operation. Record a failure for invalid current-password length and completed verification mismatch. On successful verification, clear the failure bucket before hashing the replacement password.

Return the existing `429 TOO_MANY_REQUESTS` response when the bucket is full.

- [ ] **Step 5: Run focused tests and verify GREEN**

Run the commands from Step 2. Expected: all policy and integration tests pass.

### Task 5: Full verification and PR update

**Files:**
- Modify: `docs/superpowers/plans/2026-07-30-auth-response-and-session-hardening.md` only if implementation details require correcting the plan
- Update: GitHub PR #26 description

- [ ] **Step 1: Run backend verification**

```bash
cd backend
cargo fmt --check
cargo test
cargo check
cargo clippy --all-targets --all-features -- -D warnings
```

Expected: all commands pass.

- [ ] **Step 2: Run frontend compatibility verification**

```bash
cd frontend
npm test
npm run lint
npm run build
```

Expected: all commands pass.

- [ ] **Step 3: Inspect final diff**

```bash
git diff --check
git status --short
git diff --stat
```

Expected: no whitespace errors and only intended files are modified.

- [ ] **Step 4: Commit and push**

Commit the implementation with:

```bash
git add backend/src backend/tests docs/superpowers/plans/2026-07-30-auth-response-and-session-hardening.md
git commit -m "fix: harden auth response boundaries"
git push origin HEAD:agent/user-auth-session
```

- [ ] **Step 5: Update and verify PR #26**

Append a concise follow-up section documenting controlled business errors, conditional Session activity updates, no-store identity responses, and the 5-per-15-minute password-change failure policy. Verify the PR head SHA equals local `HEAD`.
