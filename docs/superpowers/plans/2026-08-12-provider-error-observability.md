# AI Provider Error Observability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add safe structured AI Provider failure logs while preserving Skill Run's existing public error contract.

**Architecture:** Extend `ProviderError` with allow-listed transport metadata, then centralize structured warning emission in a Provider observability module whose context accepts only safe fields. Call sites supply stage-specific request metadata before applying their unchanged public error mappings.

**Tech Stack:** Rust 2024, reqwest, tracing, tracing-subscriber, Tokio, Actix Web.

---

### Task 1: Safe Provider classifications and logging helper

**Files:**
- Modify: `backend/src/ai_provider/client.rs`
- Create: `backend/src/ai_provider/observability.rs`
- Modify: `backend/src/ai_provider/mod.rs`

- [ ] **Step 1: Write failing unit tests**

Add tests that expect `ProviderError::HttpStatus(400)` to expose `http_status` and `http_status=400`, expect `ProviderError::Transport(TransportReason::ConnectionReset)` to expose only `reason=connection_reset`, and capture a warning from this API:

```rust
log_provider_failure(
    ProviderRequestContext {
        stage: "model_request",
        run_id: Some("run-1"),
        iteration: Some(1),
        elapsed_ms: 21_000,
        tools_enabled: true,
        tool_choice: Some("auto"),
        response_format: None,
    },
    ProviderError::HttpStatus(400),
);
```

Assert that sentinel API keys, Authorization values, prompt text, Skill Markdown, Issue log text, response bodies, and credential-bearing URLs are absent from both the captured event and `ProviderError` formatting.

- [ ] **Step 2: Verify RED**

Run: `cargo test ai_provider::observability::tests --lib`

Expected: compilation fails because `observability`, `ProviderRequestContext`, `TransportReason`, and the logging API do not exist.

- [ ] **Step 3: Implement minimal safe metadata**

Implement the allow-listed transport enum and metadata accessors:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportReason {
    ConnectFailed,
    DnsFailed,
    TlsFailed,
    ConnectionReset,
    RequestFailed,
}

impl ProviderError {
    pub fn category(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Transport(_) => "transport",
            Self::HttpStatus(_) => "http_status",
            Self::ResponseTooLarge => "response_too_large",
            Self::InvalidResponse => "invalid_response",
        }
    }

    pub fn http_status(self) -> Option<u16> {
        match self { Self::HttpStatus(status) => Some(status), _ => None }
    }

    pub fn transport_reason(self) -> Option<&'static str> {
        match self { Self::Transport(reason) => Some(reason.as_str()), _ => None }
    }
}
```

Change transport construction sites to retain only `TransportReason`, and add `ProviderRequestContext` plus `log_provider_failure` in `observability.rs`. Emit separate exhaustive `tracing::warn!` branches so HTTP status and transport reason are present only when applicable. Never accept arbitrary strings other than fixed stage and request-shape labels.

- [ ] **Step 4: Verify GREEN**

Run: `cargo test ai_provider::observability::tests --lib`

Expected: all new observability tests pass.

- [ ] **Step 5: Commit**

```bash
git add backend/src/ai_provider/client.rs backend/src/ai_provider/observability.rs backend/src/ai_provider/mod.rs
git commit -m "Add safe provider failure metadata"
```

### Task 2: Skill Runner stage-aware failure logs

**Files:**
- Modify: `backend/src/services/skill_runner.rs`
- Modify: `backend/tests/skill_runner.rs`

- [ ] **Step 1: Write failing tests**

Add focused tests for a scripted HTTP 400 and transport failure. Capture logs while running `SkillRunner::execute`, assert the stored error remains `AI_PROVIDER_REQUEST_FAILED`, and assert the log includes the correct stage, category, status/reason, elapsed time, `tools_enabled`, and request-shape label. Add request sequences that reach `final_model_request` and `result_repair` so each stage is covered.

- [ ] **Step 2: Verify RED**

Run: `cargo test --test skill_runner provider_failure`

Expected: tests fail because Skill Runner currently emits only the final generic `AI_PROVIDER_REQUEST_FAILED` event.

- [ ] **Step 3: Log each model-call failure**

Before mapping Provider failures, call the shared helper with:

```rust
ProviderRequestContext {
    stage: "model_request",
    run_id: Some(run_id),
    iteration: Some(iteration),
    elapsed_ms: model_started.elapsed().as_millis() as u64,
    tools_enabled: true,
    tool_choice: Some("auto"),
    response_format: None,
}
```

Use the equivalent fixed metadata for `final_model_request` and `result_repair`. Keep `runner_provider_error` unchanged for Transport and HTTP status, preserving `AI_PROVIDER_REQUEST_FAILED`.

- [ ] **Step 4: Verify GREEN**

Run: `cargo test --test skill_runner`

Expected: all Skill Runner integration tests pass.

- [ ] **Step 5: Commit**

```bash
git add backend/src/services/skill_runner.rs backend/tests/skill_runner.rs
git commit -m "Log skill model failure context"
```

### Task 3: Provider Test and Skill Review consistency

**Files:**
- Modify: `backend/src/routes/ai_provider.rs`
- Modify: `backend/src/routes/skills.rs`
- Modify: `backend/tests/ai_provider.rs`
- Modify: `backend/tests/skills.rs`

- [ ] **Step 1: Write failing route tests**

Add tests or focused internal helper tests that capture failed Provider Test and Skill Review calls and expect the shared `provider_test`, `skill_review`, and `skill_review_repair` stages with `tools_enabled=false` and the appropriate response-format label.

- [ ] **Step 2: Verify RED**

Run: `cargo test --test ai_provider provider_failure && cargo test --test skills provider_failure`

Expected: tests fail because those routes do not yet call the shared observability helper.

- [ ] **Step 3: Add shared logging calls**

Measure each request with `Instant`, emit the fixed safe context on `Err`, then return the route's existing sanitized `AppError`. Do not log base URL, model, request messages, Skill body, response content, or raw error display text.

- [ ] **Step 4: Verify GREEN**

Run: `cargo test --test ai_provider && cargo test --test skills`

Expected: both integration suites pass.

- [ ] **Step 5: Commit**

```bash
git add backend/src/routes/ai_provider.rs backend/src/routes/skills.rs backend/tests/ai_provider.rs backend/tests/skills.rs
git commit -m "Unify provider failure logging"
```

### Task 4: Full verification and PR preparation

**Files:**
- Verify all modified files and documentation.

- [ ] **Step 1: Format and lint**

Run: `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings`

Expected: both commands exit 0 with no warnings.

- [ ] **Step 2: Run complete test suite**

Run: `cargo test`

Expected: all automated tests pass; only the existing manual benchmark remains ignored.

- [ ] **Step 3: Check scope and whitespace**

Run: `git diff origin/main...HEAD --check && git status --short && git diff origin/main...HEAD --stat`

Expected: no whitespace errors and only Issue #103 implementation, tests, design, and plan files are present.

- [ ] **Step 4: Security scan the diff**

Run: `rg -n "Authorization|Bearer|api_key|messages|body" backend/src/ai_provider/observability.rs backend/src/services/skill_runner.rs backend/src/routes/ai_provider.rs backend/src/routes/skills.rs`

Expected: no sensitive values flow into Provider failure log fields; any matches are existing request construction or explicit negative tests.

- [ ] **Step 5: Commit remaining verification-only adjustments if present**

If formatting changed tracked Rust files, inspect those paths with `git diff`, stage each reviewed path explicitly, and commit them with `git commit -m "Harden provider observability tests"`. If the worktree is clean, make no empty commit.
