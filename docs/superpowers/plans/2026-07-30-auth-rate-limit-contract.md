# Authentication Rate-Limit Contract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Preserve Argon2 errors during password changes and implement the three independent authentication rate-limit policies from Issue #25.

**Architecture:** Keep the bounded in-memory timestamp buckets, but make each operation supply its own key, limit, window, and record/check mode. Login consumes its IP bucket before authentication and records its username bucket only after a completed credential failure; registration consumes only an hourly IP bucket.

**Tech Stack:** Rust, Actix Web, Tokio Semaphore, Cargo unit and integration tests.

---

### Task 1: Preserve password-change Argon2 errors

**Files:**
- Modify: `backend/src/routes/auth.rs`
- Modify: `backend/tests/auth.rs`

- [ ] Add an integration test that saturates `auth_hash_permits`, calls the authenticated password-change endpoint, and expects HTTP 429 with code `TOO_MANY_REQUESTS`.
- [ ] Run the focused test and confirm it fails with HTTP 401.
- [ ] Replace the password verification `.map_err(|_| invalid_credentials())?` with direct `?` propagation.
- [ ] Run the focused test and the existing password-change tests; expect all to pass.

### Task 2: Split authentication rate-limit policies

**Files:**
- Modify: `backend/src/config.rs`
- Modify: `backend/src/lib.rs`
- Modify: `backend/src/routes/auth.rs`
- Modify: `backend/tests/auth.rs`
- Modify: `backend/.env.example`
- Modify: `README.md`

- [ ] Add failing tests for 20 login attempts per IP per minute, 10 username failures per five
  minutes, 10 registrations per IP per hour, successful-login exclusion from the username bucket,
  and expiry at each independent window.
- [ ] Replace `login_rate_limit_per_minute` and `register_rate_limit_per_minute` with
  `login_ip_limit_per_minute`, `login_username_failure_limit_per_5_minutes`, and
  `register_ip_limit_per_hour`, including environment parsing and positive-value validation.
- [ ] Generalize bucket checking so each call supplies its own duration and whether to record an
  event; retain bounded map eviction.
- [ ] Apply login IP checking before authentication, username-failure checking before Argon2,
  username-failure recording only for completed invalid credentials, and registration IP checking
  before validation.
- [ ] Change the 429 JSON error code from `AUTH_RATE_LIMITED` to `TOO_MANY_REQUESTS`.
- [ ] Update `.env.example` and README to document the three policies and variables.
- [ ] Run focused unit and integration tests; expect all to pass.

### Task 3: Verify and publish

**Files:**
- Modify: all files from Tasks 1–2

- [ ] Run `cargo fmt --check`, `cargo test --locked`, and
  `cargo clippy --locked --all-targets --all-features -- -D warnings` from `backend/`.
- [ ] Run `npm test`, `npm run lint`, and `npm run build` from `frontend/`.
- [ ] Run `git diff --check` and confirm no old rate-limit variable or error-code names remain.
- [ ] Request independent review, fix any Critical or Important findings, commit, and push to
  `origin/agent/user-auth-session` for PR #26.
