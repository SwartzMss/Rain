# AI Provider Retry Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Retry a bounded allow-list of transient AI Provider failures across every Provider call site while preserving existing timeouts, cancellation, public errors, and safe logs.

**Architecture:** Enrich `ProviderError` with parsed `Retry-After` metadata, then add one `ai_provider::retry` service that owns classification, backoff, attempts, and retry/final logging. Skill Run, Skill Review, and Provider Test continue to own their existing request context and outer timeout/cancellation boundaries, but delegate each completion operation to this service.

**Tech Stack:** Rust 2024, Tokio, Reqwest, tracing, httpdate, Actix Web, cargo test

---

## File map

- `backend/src/ai_provider/client.rs`: preserve safe HTTP status and parsed `Retry-After` metadata.
- `backend/src/ai_provider/retry.rs`: own retry classification, default backoff, attempt loop, and tests.
- `backend/src/ai_provider/observability.rs`: render safe retry and exhausted fields.
- `backend/src/ai_provider/mod.rs`: export the retry service.
- `backend/src/services/skill_runner.rs`: route model, final, and result-repair calls through the service.
- `backend/src/routes/skills.rs`: route Skill Review initial and repair calls through the service.
- `backend/src/routes/ai_provider.rs`: route Provider Test through the service.
- `backend/tests/skill_runner.rs`: prove a result-repair 429 can recover without changing the result contract.
- `backend/tests/ai_provider.rs`: preserve real HTTP failure and safe logging coverage under the new error shape.
- `backend/Cargo.toml`, `backend/Cargo.lock`: add direct `httpdate` dependency.

### Task 1: Preserve Retry-After on HTTP failures

**Files:**
- Modify: `backend/src/ai_provider/client.rs`
- Modify: `backend/src/ai_provider/observability.rs`
- Modify: `backend/src/services/skill_runner.rs`
- Modify: `backend/tests/skill_runner.rs`
- Modify: `backend/Cargo.toml`
- Modify: `backend/Cargo.lock`

- [ ] **Step 1: Write failing Retry-After parser tests**

Add tests in `client.rs` for integer seconds, a future HTTP-date, invalid input, an expired date, and the 10-second cap. The helper contract is:

```rust
fn parse_retry_after(value: Option<&HeaderValue>, now: SystemTime) -> Option<Duration>;

let three = HeaderValue::from_static("3");
let thirty = HeaderValue::from_static("30");
let invalid = HeaderValue::from_static("invalid");
assert_eq!(parse_retry_after(Some(&three), now), Some(Duration::from_secs(3)));
assert_eq!(parse_retry_after(Some(&thirty), now), Some(Duration::from_secs(10)));
assert_eq!(parse_retry_after(Some(&invalid), now), None);
```

For HTTP-date, format `now + 4s` with `httpdate::fmt_http_date` and assert a duration no greater than four seconds and no less than three seconds. Assert an HTTP-date before `now` returns `None`.

- [ ] **Step 2: Run the focused test and verify RED**

Run from `backend/`:

```bash
cargo test ai_provider::client::tests::retry_after
```

Expected: compilation fails because `parse_retry_after` and the enriched HTTP error shape do not exist.

- [ ] **Step 3: Add the safe HTTP failure metadata**

Replace the tuple HTTP variant with a named variant and add safe constructors/accessors:

```rust
#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub enum ProviderError {
    #[error("AI provider request timed out")]
    Timeout,
    #[error("AI provider request failed")]
    Transport(TransportReason),
    #[error("AI provider returned HTTP status {status}")]
    HttpStatus {
        status: u16,
        retry_after: Option<Duration>,
    },
    #[error("AI provider response exceeded the size limit")]
    ResponseTooLarge,
    #[error("AI provider returned an invalid response")]
    InvalidResponse,
}

impl ProviderError {
    pub fn http(status: u16) -> Self {
        Self::HttpStatus { status, retry_after: None }
    }

    pub fn http_with_retry_after(status: u16, retry_after: Duration) -> Self {
        Self::HttpStatus { status, retry_after: Some(retry_after) }
    }

    pub fn retry_after(self) -> Option<Duration> {
        match self {
            Self::HttpStatus { retry_after, .. } => retry_after,
            _ => None,
        }
    }
}
```

Update `code`, `category`, `http_status`, observability matches, runner error matches, and test fixtures to use `ProviderError::http(status)`.

- [ ] **Step 4: Parse Retry-After before returning HTTP errors**

Add `httpdate = "1"` and implement:

```rust
const MAX_RETRY_AFTER: Duration = Duration::from_secs(10);

fn parse_retry_after(value: Option<&HeaderValue>, now: SystemTime) -> Option<Duration> {
    let value = value?.to_str().ok()?;
    let duration = if let Ok(seconds) = value.parse::<u64>() {
        Duration::from_secs(seconds)
    } else {
        httpdate::parse_http_date(value).ok()?.duration_since(now).ok()?
    };
    Some(duration.min(MAX_RETRY_AFTER))
}
```

Before consuming the response, create the failure with `status.as_u16()` and `parse_retry_after(response.headers().get(RETRY_AFTER), SystemTime::now())`. Keep response bodies out of the error.

- [ ] **Step 5: Run focused and affected tests and verify GREEN**

Run:

```bash
cargo test ai_provider::client
cargo test ai_provider::observability
cargo test --test skill_runner provider_failure
```

Expected: all selected tests pass and existing public error assertions remain unchanged.

- [ ] **Step 6: Commit the metadata change**

```bash
git add backend/Cargo.toml backend/Cargo.lock backend/src/ai_provider/client.rs backend/src/ai_provider/observability.rs backend/src/services/skill_runner.rs backend/tests/skill_runner.rs
git commit -m "Preserve provider retry metadata"
```

### Task 2: Implement the unified retry service and safe logs

**Files:**
- Create: `backend/src/ai_provider/retry.rs`
- Modify: `backend/src/ai_provider/mod.rs`
- Modify: `backend/src/ai_provider/observability.rs`

- [ ] **Step 1: Write failing retry classification and attempt-loop tests**

In `retry.rs`, define a scripted `ChatCompletionClient` and tests asserting:

```rust
for status in [429, 502, 503, 504] {
    assert!(is_retryable(ProviderError::http(status)));
}
for status in [400, 401, 403, 404, 500] {
    assert!(!is_retryable(ProviderError::http(status)));
}
assert!(is_retryable(ProviderError::Transport(TransportReason::ConnectFailed)));
assert!(is_retryable(ProviderError::Transport(TransportReason::ConnectionReset)));
assert!(!is_retryable(ProviderError::Timeout));
assert!(!is_retryable(ProviderError::InvalidResponse));
```

Use zero-duration `retry_after` errors in async tests so they run immediately:

```rust
let client = ScriptedClient::new([
    Err(ProviderError::http_with_retry_after(429, Duration::ZERO)),
    Ok(valid_response()),
]);
let result = complete_with_retry(&client, request(), context()).await;
assert!(result.is_ok());
assert_eq!(client.attempts(), 2);
```

Also assert non-retryable errors make one attempt and three retryable errors make exactly three attempts before returning the last error.

- [ ] **Step 2: Write failing log tests**

Extend observability capture tests to require retry logs containing:

```text
stage=result_repair
attempt=1
max_attempts=3
error_category=http_status
http_status=429
backoff_ms=1000
```

Require exhausted logs to contain `attempt=3 max_attempts=3 retry_exhausted=true`, and verify the existing sensitive sentinels remain absent.

- [ ] **Step 3: Run retry and observability tests and verify RED**

Run:

```bash
cargo test ai_provider::retry
cargo test ai_provider::observability
```

Expected: compilation fails because the retry service and new logging functions do not exist.

- [ ] **Step 4: Implement retry classification and delay selection**

Create constants and pure helpers:

```rust
pub const MAX_PROVIDER_ATTEMPTS: usize = 3;
const DEFAULT_BACKOFFS: [Duration; 2] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
];

fn is_retryable(error: ProviderError) -> bool {
    matches!(
        error,
        ProviderError::HttpStatus { status: 429 | 502 | 503 | 504, .. }
            | ProviderError::Transport(TransportReason::ConnectFailed | TransportReason::ConnectionReset)
    )
}

fn retry_delay(error: ProviderError, failed_attempt: usize) -> Duration {
    error.retry_after().unwrap_or(DEFAULT_BACKOFFS[failed_attempt - 1])
}
```

- [ ] **Step 5: Implement the three-attempt loop**

The helper clones the request per attempt, includes wait time in elapsed time, logs each retry before sleeping, and logs the final error once:

```rust
pub async fn complete_with_retry(
    client: &dyn ChatCompletionClient,
    request: ChatRequest,
    mut context: ProviderRequestContext<'_>,
) -> Result<ChatResponse, ProviderError> {
    let started = Instant::now();
    for attempt in 1..=MAX_PROVIDER_ATTEMPTS {
        match client.complete(request.clone()).await {
            Ok(response) => return Ok(response),
            Err(error) => {
                context.elapsed_ms = started.elapsed().as_millis() as u64;
                let exhausted = is_retryable(error) && attempt == MAX_PROVIDER_ATTEMPTS;
                if is_retryable(error) && !exhausted {
                    let backoff = retry_delay(error, attempt);
                    log_provider_retry(context, error, attempt, MAX_PROVIDER_ATTEMPTS, backoff);
                    tokio::time::sleep(backoff).await;
                    continue;
                }
                log_provider_failure(context, error, attempt, MAX_PROVIDER_ATTEMPTS, exhausted);
                return Err(error);
            }
        }
    }
    unreachable!("provider attempt loop always returns")
}
```

Derive `Clone` for `ChatRequest`.

- [ ] **Step 6: Implement structured retry/final logging**

Add `log_provider_retry` and extend `log_provider_failure` to emit the common safe request shape plus attempt fields. Match named `HttpStatus { status, .. }` and `Transport(reason)` so only allow-listed status/reason values are rendered. Retry logs include `backoff_ms`; exhausted final logs include `retry_exhausted=true`. Do not accept request data or raw errors as strings.

- [ ] **Step 7: Run focused tests and verify GREEN**

Run:

```bash
cargo test ai_provider::retry
cargo test ai_provider::observability
```

Expected: classification, recovery, exhaustion, backoff, log field, and sensitive-data tests all pass.

- [ ] **Step 8: Commit the retry service**

```bash
git add backend/src/ai_provider/mod.rs backend/src/ai_provider/retry.rs backend/src/ai_provider/observability.rs
git commit -m "Add bounded AI provider retries"
```

### Task 3: Route every Provider operation through the service

**Files:**
- Modify: `backend/src/services/skill_runner.rs`
- Modify: `backend/src/routes/skills.rs`
- Modify: `backend/src/routes/ai_provider.rs`
- Modify: `backend/tests/skill_runner.rs`
- Modify: `backend/tests/skills.rs`
- Modify: `backend/tests/ai_provider.rs`

- [ ] **Step 1: Add the failing result-repair recovery integration test**

Add a Skill Runner test whose scripted sequence is an invalid initial model result, a zero-backoff 429 on `result_repair`, and then a valid insufficient-evidence result:

```rust
let client = Arc::new(RecordingClient {
    responses: Mutex::new(VecDeque::from([
    Ok(ChatResponse {
        message: ChatMessage {
            role: "assistant".into(),
            content: Some("not json".into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        },
    }),
    Err(ProviderError::http_with_retry_after(429, Duration::ZERO)),
    insufficient_evidence_response(),
    ])),
    requests: Mutex::new(Vec::new()),
});

SkillRunner::execute(state, run.id.clone(), client.clone(), cancellation).await;

assert_eq!(stored.status, "SUCCEEDED");
assert_eq!(client.requests.lock().unwrap().len(), 3);
assert!(output.contains("stage=result_repair"));
assert!(output.contains("attempt=1"));
assert!(output.contains("http_status=429"));
```

- [ ] **Step 2: Run the integration test and verify RED**

Run:

```bash
cargo test --test skill_runner result_repair_retries_a_transient_429 -- --exact
```

Expected: FAIL because Skill Runner still calls `client.complete` directly and stores a failed Run.

- [ ] **Step 3: Migrate Skill Runner call sites**

Replace the three direct completion paths with:

```rust
response = complete_with_retry(
    client.as_ref(),
    request,
    ProviderRequestContext {
        stage: ProviderRequestStage::ModelRequest,
        run_id: Some(run_id),
        iteration: Some(iteration),
        elapsed_ms: 0,
        tools_enabled: true,
        tool_choice: Some("auto"),
        response_format: None,
    },
) => response.map_err(runner_provider_error)?,
```

Use `FinalModelRequest` and `ResultRepair` with their existing safe shape fields in the other two locations. Keep each existing `tokio::select!` cancellation branch around the entire retry helper future. Remove call-site `log_provider_failure` and request timers because the helper now logs final failures once.

- [ ] **Step 4: Migrate Skill Review calls**

Replace both direct calls with `complete_with_retry(&client, request, ProviderRequestContext::skill_review(repair, 0))`. Keep both inside `with_review_budget`, so attempts and sleeps consume the existing review total timeout. Map the returned final error to the unchanged `review_failed()` response.

- [ ] **Step 5: Migrate Provider Test**

Call `complete_with_retry(&client, request, ProviderRequestContext::provider_test(0))`. Remove the route-level failure logger; keep audit storage and `provider_error` mapping unchanged.

- [ ] **Step 6: Update existing integration assertions**

For existing retryable-failure tests, supply three scripted failures (or zero backoff metadata) and assert final logs include `attempt=3`, `max_attempts=3`, and `retry_exhausted=true`. Keep 400 tests at one attempt and assert no exhausted marker. Update real HTTP server fixtures only for the named HTTP error representation; do not weaken sensitive-log assertions.

- [ ] **Step 7: Run affected suites and verify GREEN**

Run:

```bash
cargo test --test skill_runner
cargo test --test skills
cargo test --test ai_provider
```

Expected: result repair recovers after one 429, retry exhaustion stays bounded, non-retryable failures remain immediate, and existing API contracts pass.

- [ ] **Step 8: Commit call-site migration**

```bash
git add backend/src/services/skill_runner.rs backend/src/routes/skills.rs backend/src/routes/ai_provider.rs backend/tests/skill_runner.rs backend/tests/skills.rs backend/tests/ai_provider.rs
git commit -m "Use provider retries across AI workflows"
```

### Task 4: Verify and publish

**Files:**
- Verify all changed backend and documentation files.

- [ ] **Step 1: Format and run the full backend suite**

Run from repository root or `backend/` as appropriate:

```bash
cargo fmt --check --manifest-path backend/Cargo.toml
cargo test --manifest-path backend/Cargo.toml
cargo clippy --manifest-path backend/Cargo.toml --all-targets --all-features -- -D warnings
```

Expected: formatting is clean, all automated tests pass (the existing manual benchmark remains ignored), and clippy reports no warnings.

- [ ] **Step 2: Inspect requirement coverage and final patch**

```bash
git diff --check a6eb58cb6b91955f4628acc2981ec07440f7d251...HEAD
git diff --stat a6eb58cb6b91955f4628acc2981ec07440f7d251...HEAD
git status --short
```

Confirm the patch changes only Provider retry/error/logging call paths, focused tests, dependency metadata, and the Issue #106 design/plan docs. Re-read each Issue #106 acceptance criterion and map it to a passing test or preserved outer timeout.

- [ ] **Step 3: Request independent code review**

Review base `a6eb58cb6b91955f4628acc2981ec07440f7d251` through current HEAD for retry correctness, timeout/cancellation preservation, attempt off-by-one errors, `Retry-After` parsing, duplicate logs, sensitive data exposure, and public contract changes. Resolve all Critical and Important findings before publishing.

- [ ] **Step 4: Push and create the Draft PR**

Push `agent/issue-106-provider-retry` and open a Draft PR to `main` summarizing retry policy, timeout behavior, safe logs, and verification. Include `Closes #106`.
