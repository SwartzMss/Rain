# Optimize Access Logs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Suppress routine successful HTTP access events while retaining slow requests and HTTP errors at useful severity levels.

**Architecture:** Add a focused middleware module that measures response latency and delegates deterministic severity selection to a pure classifier. Replace Actix's default access logger in the server stack while leaving all application tracing untouched.

**Tech Stack:** Rust, Actix Web 4, tracing, Tokio.

---

### Task 1: Classify useful access events

**Files:**
- Create: `backend/src/http_access_log.rs`
- Modify: `backend/src/main.rs`

- [ ] Add unit tests asserting fast 2xx/3xx produce no event, responses at 1,000 ms produce `WARN`, 4xx produce `WARN`, and 5xx produce `ERROR` even when slow.
- [ ] Run `cargo test --locked --bin backend http_access_log -- --nocapture` and confirm it fails because the classifier module does not exist.
- [ ] Implement `AccessLogLevel::{Warn, Error}` and `classify_access_log(status, elapsed)` with error status taking precedence over the one-second latency threshold.
- [ ] Run the focused tests and confirm all classification cases pass.

### Task 2: Replace the noisy middleware

**Files:**
- Modify: `backend/src/http_access_log.rs`
- Modify: `backend/src/main.rs`

- [ ] Implement an Actix `Transform`/`Service` middleware that records the start time, awaits the inner service, classifies the final response, and emits target `rain::http` fields `method`, `path`, `status`, `elapsed_ms`, and `peer_ip`.
- [ ] Remove `middleware::Logger` from `main.rs` and wrap the app with `HttpAccessLogger` without changing CORS, routes, state, or frontend fallback ordering.
- [ ] Run `cargo fmt --check`, `cargo clippy --locked -- -D warnings`, and `cargo test --locked`.
- [ ] Commit with `git commit -m "feat: filter noisy access logs"`.

### Task 3: Publish

**Files:**
- Verify all branch changes.

- [ ] Run `git diff --check` and inspect `git diff main...HEAD` for unrelated modifications.
- [ ] Push `agent/optimize-access-logs` and open a PR targeting `main` that explains the retained slow/error policy and verification commands.
