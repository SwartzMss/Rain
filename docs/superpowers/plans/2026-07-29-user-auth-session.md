# User Authentication and Session Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add username/password registration and opaque database-backed browser sessions, plus frontend login, registration, and global identity state.

**Architecture:** Focused auth modules implement validation, Argon2id passwords, opaque session tokens, and request extraction. SQLite repositories own user and session persistence, Actix routes expose the HTTP contract, and a React context owns browser identity state. Existing business write routes remain unchanged until issue #25 PR 2.

**Tech Stack:** Rust 2024, Actix Web, SQLx/SQLite, Argon2id, SHA-256, React 18, TypeScript, React Router, Vite.

---

## File Map

- Modify `backend/Cargo.toml`: add password hashing, random-token, cookie, and regex dependencies.
- Modify `backend/src/config.rs`: add authentication configuration and validation.
- Modify `backend/src/db.rs`: reset and create auth tables and indexes.
- Modify `backend/src/error.rs`: support stable structured API errors and auth statuses.
- Modify `backend/src/lib.rs`: export auth and models, and carry auth config in application state.
- Create `backend/src/auth/mod.rs`: shared public user and authenticated-user types.
- Create `backend/src/auth/password.rs`: username/password validation and Argon2id operations.
- Create `backend/src/auth/session.rs`: raw token generation, hashing, and cookie construction.
- Create `backend/src/auth/extractor.rs`: optional and required Actix request extractors.
- Create `backend/src/models/auth.rs`: authentication request and response payloads.
- Create `backend/src/repositories/users.rs`: user persistence.
- Create `backend/src/repositories/sessions.rs`: session persistence and resolution.
- Create `backend/src/routes/auth.rs`: register, login, me, and logout handlers.
- Modify `backend/src/routes/mod.rs`: register auth routes.
- Modify `backend/src/main.rs`: pass auth config into state.
- Create `backend/tests/auth.rs`: end-to-end authentication contract tests.
- Create `frontend/src/auth/authState.ts`: pure state transitions and safe return-path logic.
- Create `frontend/src/auth/AuthContext.tsx`: global auth provider and actions.
- Create `frontend/src/features/auth/AuthPage.tsx`: shared login/registration page.
- Modify `frontend/src/api/types.ts`: authentication payload types.
- Modify `frontend/src/api/client.ts`: credentials, structured errors, and auth API methods.
- Modify `frontend/src/main.tsx`: install `AuthProvider`.
- Modify `frontend/src/App.tsx`: auth routes and identity-aware header.
- Create `frontend/tests/auth-state.mjs`: frontend auth behavior tests.
- Modify `frontend/package.json`: run the new test.
- Modify `backend/.env.example`, `frontend/.env.example`, and `README.md`: document auth behavior and configuration.

### Task 1: Authentication configuration and schema

**Files:**
- Modify: `backend/src/config.rs`
- Modify: `backend/src/db.rs`
- Modify: `backend/src/lib.rs`
- Modify: `backend/src/main.rs`

- [ ] **Step 1: Write failing configuration and schema tests**

Add tests that assert:

```rust
assert_eq!(AuthConfig::default().session_ttl_seconds, 604_800);
assert!(!AuthConfig::default().session_cookie_secure);
assert!(AuthConfig { session_ttl_seconds: 0, ..Default::default() }.validate().is_err());
```

Extend the DB schema test to query `sqlite_master` and assert `users`,
`user_sessions`, `idx_user_sessions_user`, and `idx_user_sessions_expiry` exist.

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```bash
cd backend && cargo test config::tests db::tests
```

Expected: compilation or assertion failure because `AuthConfig` and auth tables do
not exist.

- [ ] **Step 3: Implement configuration and schema**

Add:

```rust
#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub session_ttl_seconds: u64,
    pub session_cookie_secure: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            session_ttl_seconds: 604_800,
            session_cookie_secure: false,
        }
    }
}
```

Load `RAIN_SESSION_TTL_SECONDS` and `RAIN_SESSION_COOKIE_SECURE` in `AppConfig`,
validate a positive TTL, add the two tables and indexes from the design, and drop
`user_sessions` before `users` during reset. Add `auth: AuthConfig` to `AppState`
and pass it from `main`.

- [ ] **Step 4: Run focused tests and verify GREEN**

Run:

```bash
cd backend && cargo test config::tests db::tests
```

Expected: all focused tests pass.

### Task 2: Validation, password hashing, and persistence

**Files:**
- Modify: `backend/Cargo.toml`
- Create: `backend/src/auth/mod.rs`
- Create: `backend/src/auth/password.rs`
- Create: `backend/src/models/auth.rs`
- Modify: `backend/src/models/mod.rs`
- Create: `backend/src/repositories/users.rs`
- Modify: `backend/src/repositories/mod.rs`

- [ ] **Step 1: Write failing unit tests**

Tests must cover:

```rust
assert_eq!(normalize_username("Swartz"), "swartz");
assert!(validate_username("abc").is_ok());
assert!(validate_username("ab").is_err());
assert!(validate_username("用户名").is_err());
assert!(validate_password("12345678").is_ok());
assert!(validate_password("1234567").is_err());

let hash = hash_password("password123").expect("hash");
assert!(hash.starts_with("$argon2id$"));
assert!(verify_password("password123", &hash).expect("verify"));
assert!(!verify_password("wrong-password", &hash).expect("verify"));
```

Repository tests create `Swartz`, find it as `swartz`, and assert creating `SWARTZ`
returns a duplicate-username result.

- [ ] **Step 2: Run focused tests and verify RED**

Run:

```bash
cd backend && cargo test auth::password repositories::users
```

Expected: compilation failure because auth modules do not exist.

- [ ] **Step 3: Implement minimal password and user code**

Add `argon2`, `rand`, `regex`, and `base64` dependencies. Define:

```rust
pub struct AuthenticatedUser {
    pub id: String,
    pub username: String,
}

pub async fn create_user(
    pool: &SqlitePool,
    username: &str,
    password_hash: &str,
) -> Result<CreateUserOutcome, AppError>;

pub async fn find_by_normalized_username(
    pool: &SqlitePool,
    username_normalized: &str,
) -> Result<Option<UserRecord>, AppError>;
```

Map SQLite unique violations to `CreateUserOutcome::DuplicateUsername`. Hash with
`Argon2::default()` and `SaltString::generate(&mut OsRng)`.

- [ ] **Step 4: Run focused and backend tests**

Run:

```bash
cd backend && cargo test auth::password repositories::users
```

Expected: focused tests pass.

### Task 3: Opaque sessions and request extractors

**Files:**
- Modify: `backend/Cargo.toml`
- Create: `backend/src/auth/session.rs`
- Create: `backend/src/auth/extractor.rs`
- Create: `backend/src/repositories/sessions.rs`
- Modify: `backend/src/repositories/mod.rs`
- Modify: `backend/src/error.rs`

- [ ] **Step 1: Write failing session tests**

Tests must assert:

```rust
let token = generate_session_token();
assert!(token.len() >= 43);
assert_eq!(hash_session_token(&token).len(), 64);
assert_ne!(token, hash_session_token(&token));
```

Repository tests insert a session, resolve it, revoke it, and verify expired,
revoked, and inactive-user sessions do not resolve. Extractor tests assert missing
cookies produce `OptionalUser(None)` and `RequireUser` returns structured 401.

- [ ] **Step 2: Run focused tests and verify RED**

Run:

```bash
cd backend && cargo test auth::session repositories::sessions auth::extractor
```

Expected: compilation failure because session components do not exist.

- [ ] **Step 3: Implement session primitives and repositories**

Generate `[u8; 32]` with `OsRng`, encode URL-safe without padding, and store:

```rust
pub async fn create_session(
    pool: &SqlitePool,
    user_id: &str,
    token_hash: &str,
    expires_at: DateTime<Utc>,
    user_agent: Option<&str>,
    client_ip: Option<&str>,
) -> Result<String, AppError>;

pub async fn resolve_active_user(
    pool: &SqlitePool,
    token_hash: &str,
) -> Result<Option<AuthenticatedUser>, AppError>;

pub async fn revoke_by_token_hash(
    pool: &SqlitePool,
    token_hash: &str,
) -> Result<(), AppError>;
```

Add an API error variant carrying status, code, and safe public message. Implement
`FromRequest` for `OptionalUser` and `RequireUser` using `AppState`.

- [ ] **Step 4: Run focused tests and verify GREEN**

Run:

```bash
cd backend && cargo test auth::session repositories::sessions auth::extractor
```

Expected: focused tests pass.

### Task 4: Authentication HTTP API

**Files:**
- Create: `backend/src/routes/auth.rs`
- Modify: `backend/src/routes/mod.rs`
- Create: `backend/tests/auth.rs`

- [ ] **Step 1: Write failing endpoint tests**

Use an in-memory SQLite app and cover:

```text
POST /api/auth/register -> 201
POST duplicate-case username -> 409 USERNAME_ALREADY_EXISTS
GET /api/auth/me without cookie -> 200 authenticated=false
POST /api/auth/login wrong username -> 401 INVALID_CREDENTIALS
POST /api/auth/login wrong password -> identical 401 body
POST /api/auth/login correct password -> 200 and HttpOnly rain_session cookie
GET /api/auth/me with cookie -> authenticated=true
POST /api/auth/logout with cookie -> 204 and expired cookie
GET /api/auth/me with logged-out cookie -> authenticated=false
```

After login, query SQLite to assert the stored hash differs from the raw cookie
token.

- [ ] **Step 2: Run endpoint tests and verify RED**

Run:

```bash
cd backend && cargo test --test auth
```

Expected: 404 or compilation failure because routes do not exist.

- [ ] **Step 3: Implement endpoints**

Define payloads:

```rust
pub struct CredentialsRequest {
    pub username: String,
    pub password: String,
}

pub struct AuthMeResponse {
    pub authenticated: bool,
    pub user: Option<PublicUser>,
}
```

Registration validates before hashing, then maps duplicate names to a structured
409. Login always maps missing user, disabled user, and password mismatch to the
same `INVALID_CREDENTIALS` response. Logout is idempotent.

- [ ] **Step 4: Run endpoint and full backend tests**

Run:

```bash
cd backend && cargo test --test auth
cd backend && cargo test
```

Expected: all backend tests pass.

### Task 5: Frontend auth state and API client

**Files:**
- Create: `frontend/src/auth/authState.ts`
- Create: `frontend/src/auth/AuthContext.tsx`
- Modify: `frontend/src/api/types.ts`
- Modify: `frontend/src/api/client.ts`
- Create: `frontend/tests/auth-state.mjs`
- Modify: `frontend/package.json`

- [ ] **Step 1: Write failing frontend tests**

Assert:

```ts
assert.equal(toAuthState({ authenticated: false, user: null }).status, 'GUEST');
assert.equal(
  toAuthState({ authenticated: true, user: { id: '1', username: 'swartz' } }).status,
  'AUTHENTICATED'
);
assert.equal(safeReturnPath('/issue/CN013'), '/issue/CN013');
assert.equal(safeReturnPath('https://evil.example'), '/');
assert.equal(safeReturnPath('//evil.example'), '/');
```

Also inspect the API client source for `credentials: 'include'` and structured
`message` parsing.

- [ ] **Step 2: Run frontend test and verify RED**

Run:

```bash
cd frontend && node tests/auth-state.mjs
```

Expected: module-not-found failure.

- [ ] **Step 3: Implement frontend types, client, and provider**

Add `User`, `AuthMeResponse`, and `ApiError` types. Ensure `fetch` uses:

```ts
fetch(url, { ...init, headers, credentials: 'include' })
```

Set `xhr.withCredentials = true`. Add `register`, `login`, `me`, and `logout`
methods. `AuthProvider` loads `/me` once, exposes `login`, `register`, `logout`,
and moves to guest state after a failed refresh caused by an invalid session.

- [ ] **Step 4: Run frontend test and type check**

Run:

```bash
cd frontend && node tests/auth-state.mjs
cd frontend && npm run lint
```

Expected: test and type check pass.

### Task 6: Login, registration, and authenticated header

**Files:**
- Create: `frontend/src/features/auth/AuthPage.tsx`
- Modify: `frontend/src/main.tsx`
- Modify: `frontend/src/App.tsx`
- Modify: `frontend/src/App.css`

- [ ] **Step 1: Extend the failing UI source test**

Check that `App.tsx` registers `/login` and `/register`, renders guest links and an
authenticated logout action, and that `main.tsx` wraps `App` in `AuthProvider`.

- [ ] **Step 2: Run the frontend auth test and verify RED**

Run:

```bash
cd frontend && node tests/auth-state.mjs
```

Expected: source assertions fail because UI integration is absent.

- [ ] **Step 3: Implement the pages and header**

Use one shared `AuthPage` with a `mode: 'login' | 'register'` property. On register,
redirect to `/login` with a success notice. On login, call the provider and navigate
to `safeReturnPath(location.state?.from)`. Render the header state without changing
existing business controls.

- [ ] **Step 4: Run frontend verification**

Run:

```bash
cd frontend && npm test
cd frontend && npm run lint
cd frontend && npm run build
```

Expected: all frontend tests, type checking, and production build pass.

### Task 7: Documentation and final verification

**Files:**
- Modify: `backend/.env.example`
- Modify: `frontend/.env.example`
- Modify: `README.md`

- [ ] **Step 1: Add configuration and product documentation**

Document:

```dotenv
RAIN_SESSION_TTL_SECONDS=604800
RAIN_SESSION_COOKIE_SECURE=false
```

Explain registration, login, logout, the lack of password recovery, the requirement
to enable secure cookies behind HTTPS, and that PR 1 does not yet enforce guest
read-only access.

- [ ] **Step 2: Format and run complete verification**

Run:

```bash
cd backend && cargo fmt --check
cd backend && cargo test
cd backend && cargo clippy --all-targets --all-features -- -D warnings
cd frontend && npm test
cd frontend && npm run lint
cd frontend && npm run build
git diff --check
```

Expected: every command exits 0 with no warnings or failures.

- [ ] **Step 3: Review scope and security invariants**

Confirm from the diff and tests that:

- no existing business route gained auth enforcement;
- raw passwords and session tokens are never persisted or logged;
- invalid login cases have identical public responses;
- cookies are HttpOnly, SameSite Lax, path `/`, and configurable Secure;
- only active, unexpired, unrevoked sessions authenticate;
- documentation does not claim guest read-only enforcement is already active.
