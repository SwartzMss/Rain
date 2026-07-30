# Same-Origin HTTP Authentication Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove configurable secure cookies and credentialed CORS so Rain supports only its same-origin internal HTTP deployment.

**Architecture:** Keep the existing session token, `HttpOnly`, `SameSite=Lax`, path, and lifetime behavior, but make cookie construction parameterless and always non-Secure. Remove CORS configuration and middleware entirely so browser access falls back to the same-origin policy.

**Tech Stack:** Rust, Actix Web, Actix cookies, Cargo tests, Markdown and dotenv configuration.

---

### Task 1: Lock in the non-Secure cookie API

**Files:**
- Modify: `backend/src/auth/session.rs`
- Modify: `backend/tests/auth.rs`

- [ ] **Step 1: Change the integration test to require a non-Secure login cookie**

Add this assertion next to the existing `HttpOnly` and `SameSite=Lax` assertions:

```rust
assert!(!set_cookie.contains("Secure"));
```

- [ ] **Step 2: Run the authentication integration test**

Run: `cd backend && cargo test --test auth`

Expected: PASS for the new behavioral assertion because the current default is HTTP; this is a characterization test for the supported deployment boundary.

- [ ] **Step 3: Remove the cookie security parameter**

Change both cookie builders to parameterless security behavior:

```rust
pub fn session_cookie(token: String, ttl_seconds: u64) -> Cookie<'static> {
    Cookie::build(SESSION_COOKIE_NAME, token)
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(Duration::seconds(ttl_seconds.min(i64::MAX as u64) as i64))
        .finish()
}

pub fn cleared_session_cookie() -> Cookie<'static> {
    Cookie::build(SESSION_COOKIE_NAME, "")
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(Duration::ZERO)
        .finish()
}
```

Update every caller to stop passing `session_cookie_secure`.

- [ ] **Step 4: Run the authentication integration test again**

Run: `cd backend && cargo test --test auth`

Expected: PASS.

### Task 2: Remove secure-cookie and CORS configuration

**Files:**
- Modify: `backend/src/config.rs`
- Modify: `backend/src/main.rs`
- Modify: `backend/Cargo.toml`
- Modify: `backend/Cargo.lock`
- Modify: `backend/.env.example`

- [ ] **Step 1: Add a source-level regression check**

After implementation, the exact check is:

```bash
! rg -n "RAIN_SESSION_COOKIE_SECURE|RAIN_ALLOWED_ORIGINS|session_cookie_secure|CorsConfig|actix_cors|supports_credentials" backend/src backend/tests backend/.env.example
```

Before implementation this must fail because the removed configuration still exists.

- [ ] **Step 2: Verify the source-level check fails**

Run the command above.

Expected: non-zero exit status with matches in `config.rs`, `main.rs`, and `.env.example`.

- [ ] **Step 3: Remove both configuration surfaces**

Delete `AuthConfig.session_cookie_secure`, its default and environment parsing. Delete `CorsConfig`,
its tests, `AppConfig.cors`, and CORS loading. Delete `RAIN_SESSION_COOKIE_SECURE` and
`RAIN_ALLOWED_ORIGINS` from `backend/.env.example`. Remove the unused `actix-cors` dependency
from `backend/Cargo.toml` and refresh `backend/Cargo.lock`.

- [ ] **Step 4: Remove the CORS middleware**

Delete the `actix_cors::Cors` and header imports, the allowed-origin capture, CORS construction,
and `.wrap(cors)` from `backend/src/main.rs`.

- [ ] **Step 5: Verify the source-level check passes**

Run the command from Step 1.

Expected: exit status 0 because no removed identifiers remain in backend source, tests, or example configuration.

### Task 3: Update deployment documentation

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Remove obsolete configuration and deployment claims**

Delete both environment-variable rows, the HTTPS configuration example, and credentialed-CORS
instructions. State that the bundled frontend and API must be accessed from the same origin and
that this release targets trusted internal HTTP deployment.

- [ ] **Step 2: Verify obsolete names are absent**

Run:

```bash
! rg -n "RAIN_SESSION_COOKIE_SECURE|RAIN_ALLOWED_ORIGINS" README.md backend/.env.example backend/src backend/tests
```

Expected: exit status 0.

### Task 4: Enforce the same-origin boundary

**Files:**
- Create: `backend/src/auth/same_origin.rs`
- Modify: `backend/src/auth/mod.rs`
- Modify: `backend/src/main.rs`
- Modify: `backend/tests/auth.rs`
- Modify: `frontend/src/api/client.ts`
- Modify: `frontend/vite.config.ts`
- Modify: `frontend/.env.example`
- Modify: `frontend/tests/auth-state.mjs`
- Modify: `README.md`

- [ ] **Step 1: Add failing backend and frontend boundary tests**

Add an authentication integration test that sends an unsafe request with a foreign `Origin` and
expects `403 CROSS_ORIGIN_REQUEST_REJECTED`. Add frontend source assertions that the API client
contains no `VITE_API_BASE_URL` and uses an empty relative base.

- [ ] **Step 2: Verify both tests fail for the missing enforcement**

Run:

```bash
cd backend && cargo test --test auth unsafe_cross_origin_requests_are_rejected
cd ../frontend && node tests/auth-state.mjs
```

Expected: backend compilation fails because `auth::same_origin` does not exist, and the frontend
test fails because `VITE_API_BASE_URL` remains.

- [ ] **Step 3: Implement request-side enforcement and relative frontend API access**

Add middleware that permits safe methods and non-browser requests without `Origin`, accepts
browser requests marked `Sec-Fetch-Site: same-origin`, and otherwise rejects unsafe requests whose
`Origin` differs from the request scheme and host. Wrap the production app with it. Fix browser
API calls to relative `/api` paths and use a server-only `RAIN_DEV_API_PROXY_TARGET` for the Vite
proxy while preserving browser fetch metadata.

- [ ] **Step 4: Verify the focused tests pass**

Run the commands from Step 2.

Expected: both exit 0.

### Task 5: Verify and publish

**Files:**
- Modify: files changed in Tasks 1–3

- [ ] **Step 1: Format and verify the backend**

Run:

```bash
cd backend
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets --all-features -- -D warnings
cd ..
```

Expected: all commands exit 0.

- [ ] **Step 2: Verify the frontend and repository diff**

Run:

```bash
cd frontend
npm test
npm run lint
npm run build
cd ..
git diff --check
```

Expected: all commands exit 0.

- [ ] **Step 3: Review, commit, and push**

Commit the implementation with:

```bash
git add backend/src/auth/session.rs backend/src/auth/extractor.rs backend/src/routes/auth.rs \
  backend/src/config.rs backend/src/main.rs backend/Cargo.toml backend/Cargo.lock \
  backend/src/auth/same_origin.rs backend/tests/auth.rs backend/.env.example \
  frontend/src/api/client.ts frontend/vite.config.ts frontend/.env.example \
  frontend/tests/auth-state.mjs README.md \
  docs/superpowers/plans/2026-07-30-same-origin-http-auth.md
git commit -m "refactor: restrict auth to same-origin HTTP"
git push origin HEAD:agent/user-auth-session
```

Expected: PR #26 head advances to the new commit.
