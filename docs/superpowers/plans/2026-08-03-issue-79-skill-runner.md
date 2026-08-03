# Issue #79 Skill Runner Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add private user-authored Skills, administrator-managed OpenAI-compatible model configuration, and a bounded Issue-scoped adaptive log-analysis runner with temporary results.

**Architecture:** Extend the existing Actix/SQLite monolith with focused `ai_provider`, `skills`, `skill_tools`, and `skill_runner` modules, then expose typed HTTP routes consumed by small React feature components. Runs bind the authenticated user, visible Issue, and private Skill on the server; only three bounded read-only tools reach existing file and FTS storage.

**Tech Stack:** Rust 2024, Actix Web, Tokio, SQLx/SQLite, reqwest, aes-gcm, serde/serde_json, React 18, TypeScript, Vite, Vitest/Testing Library, Server-Sent Events.

---

## File map

- `backend/src/config.rs`: AI environment configuration and fixed runner limits.
- `backend/src/db.rs`: Skill, review, provider, run, and step schema plus reset order.
- `backend/src/ai_provider/{mod.rs,config.rs,crypto.rs,client.rs}`: effective provider resolution, encrypted secret envelope, and bounded Chat Completions client.
- `backend/src/models/{skills.rs,skill_runs.rs}`: HTTP and persistence DTOs.
- `backend/src/repositories/{skills.rs,skill_runs.rs}`: ownership-scoped Skill and temporary run persistence.
- `backend/src/services/{skill_tools.rs,skill_runner.rs}`: Issue-bound tools, evidence ledger, adaptive loop, cancellation, and event publishing.
- `backend/src/routes/{skills.rs,skill_runs.rs,ai_provider.rs}`: authenticated user, run, SSE, and administrator endpoints.
- `backend/src/lib.rs`: provider and runner runtime state.
- `backend/src/main.rs`, `backend/src/routes/mod.rs`: startup recovery, cleanup jobs, and route registration.
- `backend/tests/{skills.rs,ai_provider.rs,skill_runs.rs}`: endpoint and runner integration coverage.
- `frontend/src/api/{types.ts,client.ts}`: typed API surface.
- `frontend/src/features/skills/{SkillsPage.tsx,SkillEditor.tsx,SkillReviewPanel.tsx}`: private Skill lifecycle UI.
- `frontend/src/features/admin/AiProviderSettings.tsx`: administrator provider form and connection test.
- `frontend/src/features/skill-runs/{IssueSkillRunner.tsx,SkillRunResult.tsx,useSkillRun.ts}`: current run controls, SSE/poll recovery, and evidence navigation.
- `frontend/src/features/auth/AccountPage.tsx`, `frontend/src/features/admin/AdminPage.tsx`, `frontend/src/features/files/FilesView.tsx`, `frontend/src/App.tsx`: feature integration.
- `frontend/tests/{skills.behavior.test.tsx,ai-provider.behavior.test.tsx,skill-runner.behavior.test.tsx}`: UI behavior.
- `README.md`, `backend/.env.example`: operator configuration and product behavior.

### Task 1: Add AI and runner configuration

**Files:**
- Modify: `backend/Cargo.toml`
- Modify: `backend/src/config.rs`
- Modify: `backend/.env.example`
- Test: `backend/src/config.rs`

- [ ] **Step 1: Write failing configuration tests**

Add tests that parse a complete AI environment, reject a timeout outside `1..=300`, reject a malformed master key, and confirm these runner constants:

```rust
assert_eq!(SkillRunLimits::default().max_iterations, 8);
assert_eq!(SkillRunLimits::default().max_tool_calls, 24);
assert_eq!(SkillRunLimits::default().max_total_evidence_bytes, 128 * 1024);
assert_eq!(SkillRunLimits::default().terminal_retention_seconds, 24 * 60 * 60);
```

- [ ] **Step 2: Run the focused tests and confirm failure**

Run: `cd backend && cargo test config::tests::ai_`

Expected: compilation fails because `AiProviderEnv` and `SkillRunLimits` do not exist.

- [ ] **Step 3: Add dependencies and configuration types**

Add `reqwest` with JSON and rustls, `aes-gcm`, `async-stream`, and `tokio-util`. Define:

```rust
pub struct AiProviderEnv {
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub timeout_seconds: u64,
    pub master_key: Option<[u8; 32]>,
}

pub struct SkillRunLimits {
    pub max_iterations: usize,
    pub max_tool_calls: usize,
    pub search_results_per_call: usize,
    pub max_evidence_ranges: usize,
    pub max_tool_output_bytes: usize,
    pub max_total_evidence_bytes: usize,
    pub timeout_seconds: u64,
    pub terminal_retention_seconds: u64,
}
```

Load `RAIN_AI_BASE_URL`, `RAIN_AI_API_KEY`, `RAIN_AI_MODEL`, `RAIN_AI_TIMEOUT_SECONDS`, and base64 `RAIN_AI_MASTER_KEY` without logging their values. Document them in `.env.example`.

- [ ] **Step 4: Run tests and format**

Run: `cd backend && cargo test config::tests::ai_ && cargo fmt --check`

Expected: all focused tests pass and formatting is clean.

- [ ] **Step 5: Commit**

```bash
git add backend/Cargo.toml backend/Cargo.lock backend/src/config.rs backend/.env.example
git commit -m "feat: add skill runner configuration"
```

### Task 2: Create schema and encrypted provider storage

**Files:**
- Modify: `backend/src/db.rs`
- Create: `backend/src/ai_provider/mod.rs`
- Create: `backend/src/ai_provider/config.rs`
- Create: `backend/src/ai_provider/crypto.rs`
- Modify: `backend/src/lib.rs`
- Test: `backend/src/ai_provider/crypto.rs`
- Test: `backend/tests/ai_provider.rs`

- [ ] **Step 1: Write failing schema and crypto tests**

Assert all new tables and the active-run partial unique index exist after `db::init`. Add a round-trip test and wrong-key rejection test around:

```rust
let encrypted = SecretCipher::new(key).encrypt("secret-value")?;
assert_eq!(SecretCipher::new(key).decrypt(&encrypted)?, "secret-value");
assert!(SecretCipher::new(other_key).decrypt(&encrypted).is_err());
```

- [ ] **Step 2: Run tests and confirm failure**

Run: `cd backend && cargo test ai_provider`

Expected: tests fail because the module, tables, and cipher are missing.

- [ ] **Step 3: Add database objects**

Create `user_skills` and `skill_reviews` with the columns in the approved SQL design. Add a singleton `ai_provider_settings` row with nullable base URL, encrypted key envelope, model, timeout, updater, and timestamps. Add `skill_runs` with the trusted binding and temporary state:

```sql
CREATE TABLE skill_runs (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    issue_code TEXT NOT NULL REFERENCES issues(code) ON DELETE CASCADE,
    skill_id TEXT NOT NULL,
    skill_version INTEGER NOT NULL,
    skill_name TEXT NOT NULL,
    skill_snapshot_markdown TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('QUEUED','RUNNING','SUCCEEDED','FAILED','CANCELLED')),
    iteration_count INTEGER NOT NULL DEFAULT 0,
    tool_call_count INTEGER NOT NULL DEFAULT 0,
    cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK (cancel_requested IN (0,1)),
    result_json TEXT,
    error_code TEXT,
    error_message TEXT,
    started_at TEXT,
    completed_at TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
```

Add `skill_run_steps` with `run_id`, sequence, iteration, tool name, argument summary, hit count, evidence metadata, elapsed milliseconds, status, and timestamps. Add:

```sql
CREATE UNIQUE INDEX IF NOT EXISTS idx_skill_runs_one_active_per_user
ON skill_runs(user_id)
WHERE status IN ('QUEUED', 'RUNNING');
```

Order reset statements from child tables to parents. Use `skill_id TEXT` without a foreign key in `skill_runs` and cascading foreign keys for user, Issue, review, and steps.

- [ ] **Step 4: Implement authenticated encryption and effective config resolution**

Use AES-256-GCM with a fresh 96-bit nonce per write. Store a versioned base64 envelope containing nonce and ciphertext. Implement:

```rust
pub async fn resolve_effective_config(
    pool: &SqlitePool,
    env: &AiProviderEnv,
) -> Result<Option<ResolvedAiProvider>, AppError>;
```

A complete decryptable database row wins; otherwise a complete environment tuple wins. A partial configuration is unavailable and returns a sanitized reason.

- [ ] **Step 5: Run schema and crypto tests**

Run: `cd backend && cargo test ai_provider && cargo test db::tests`

Expected: new schema and cryptographic tests pass.

- [ ] **Step 6: Commit**

```bash
git add backend/src/db.rs backend/src/ai_provider backend/src/lib.rs backend/tests/ai_provider.rs
git commit -m "feat: store encrypted ai provider settings"
```

### Task 3: Implement the bounded OpenAI-compatible client and admin API

**Files:**
- Create: `backend/src/ai_provider/client.rs`
- Create: `backend/src/routes/ai_provider.rs`
- Modify: `backend/src/routes/mod.rs`
- Modify: `backend/src/models/admin.rs`
- Test: `backend/tests/ai_provider.rs`

- [ ] **Step 1: Add failing endpoint and fake-server tests**

Cover administrator-only reads and writes, masked key output, blank-key preservation, no-master-key rejection, database-over-environment precedence, test connection success, timeout, non-2xx response, and oversized response. Use a local Actix test server and assert returned errors do not contain the supplied secret.

- [ ] **Step 2: Run the tests and confirm failure**

Run: `cd backend && cargo test --test ai_provider`

Expected: routes return 404 and client types are unresolved.

- [ ] **Step 3: Implement DTOs and client**

Define an internal request boundary:

```rust
#[async_trait]
pub trait ChatCompletionClient: Send + Sync {
    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError>;
}
```

The reqwest implementation posts to `{base_url}/chat/completions`, supplies Bearer authentication, caps response bytes, applies the configured timeout, and only returns sanitized error categories.

- [ ] **Step 4: Implement administrator routes and audit entries**

Register `GET/PUT /api/admin/ai-provider` and `POST /api/admin/ai-provider/test`. Reuse `AdminUser`, update the singleton row transactionally, retain the old encrypted key when `api_key` is absent, and record only non-secret before/after metadata in `admin_audit_logs`.

- [ ] **Step 5: Run focused tests**

Run: `cd backend && cargo test --test ai_provider`

Expected: all provider authorization, persistence, masking, client, and redaction tests pass.

- [ ] **Step 6: Commit**

```bash
git add backend/src/ai_provider backend/src/routes/ai_provider.rs backend/src/routes/mod.rs backend/src/models/admin.rs backend/tests/ai_provider.rs
git commit -m "feat: manage openai compatible provider"
```

### Task 4: Add private Skill CRUD and current quality review

**Files:**
- Create: `backend/src/models/skills.rs`
- Create: `backend/src/repositories/skills.rs`
- Create: `backend/src/routes/skills.rs`
- Modify: `backend/src/models/mod.rs`
- Modify: `backend/src/repositories/mod.rs`
- Modify: `backend/src/routes/mod.rs`
- Test: `backend/tests/skills.rs`

- [ ] **Step 1: Write failing API tests**

Test guest rejection, admin rejection, same-user CRUD, cross-user 404 behavior, per-user case-insensitive name uniqueness, blank and oversized Markdown, version increment only on content changes, enable/disable behavior, delete, review replacement, review clearing after edit, provider absence, and unsupported capability warnings.

- [ ] **Step 2: Run tests and confirm failure**

Run: `cd backend && cargo test --test skills`

Expected: Skill endpoints return 404.

- [ ] **Step 3: Implement ownership-scoped repository operations**

Every query includes `owner_user_id = ?`. Use one transaction for update plus review invalidation. Compute `content_hash` as lowercase SHA-256 hex of UTF-8 Markdown. Return a public model that includes only the current review.

- [ ] **Step 4: Implement CRUD routes and deterministic validation**

Register the six `/api/me/skills` routes. Validate trimmed name length `1..=100`, description length at most 1000, and Markdown byte length `1..=64 KiB`. Map uniqueness to `SKILL_NAME_CONFLICT`.

- [ ] **Step 5: Implement quality review**

Send the fixed six-dimension rubric to `ChatCompletionClient`, request JSON, validate each dimension and the `0..=100` total, perform one repair attempt, and upsert the single `skill_reviews` row only when the Skill version and hash still match after the model call.

- [ ] **Step 6: Run tests**

Run: `cd backend && cargo test --test skills`

Expected: all Skill isolation, version, validation, and review tests pass.

- [ ] **Step 7: Commit**

```bash
git add backend/src/models backend/src/repositories backend/src/routes backend/tests/skills.rs
git commit -m "feat: add private user skills"
```

### Task 5: Implement Issue-bound read-only tools and evidence ledger

**Files:**
- Create: `backend/src/services/skill_tools.rs`
- Modify: `backend/src/services/mod.rs`
- Test: `backend/src/services/skill_tools.rs`

- [ ] **Step 1: Write failing tool tests**

Create two Issues with ready and non-ready Bundles. Assert `list_files` omits non-ready data, `search_logs` never returns the other Issue, `read_file_lines` rejects a foreign file, line ranges are bounded, repeated searches are cached, overlapping reads return only unseen lines, and evidence ranges merge.

- [ ] **Step 2: Run tests and confirm failure**

Run: `cd backend && cargo test skill_tools::tests`

Expected: compilation fails because `SkillToolExecutor` is missing.

- [ ] **Step 3: Implement trusted run context and tool schemas**

Define:

```rust
pub struct SkillRunContext {
    pub run_id: String,
    pub user_id: String,
    pub issue_code: String,
}

pub enum SkillToolCall {
    ListFiles,
    SearchLogs { query: String },
    ReadFileLines { file_id: i64, start: usize, end: usize },
}
```

Do not include `issue_code` in model-callable arguments.

- [ ] **Step 4: Implement tools and ledger**

Use SQL joins through `bundles.issue_code` and `bundles.status='READY'`. Reuse the FTS query and blob-backed line reader. Cap one read at 200 lines and each returned tool payload at 32 KiB. Track exact searches, read intervals, total bytes, and at most 30 merged evidence ranges.

- [ ] **Step 5: Run tests**

Run: `cd backend && cargo test skill_tools::tests`

Expected: all scope, bounds, deduplication, and ledger tests pass.

- [ ] **Step 6: Commit**

```bash
git add backend/src/services/mod.rs backend/src/services/skill_tools.rs
git commit -m "feat: add issue scoped skill tools"
```

### Task 6: Add temporary run persistence, concurrency, recovery, and cleanup

**Files:**
- Create: `backend/src/models/skill_runs.rs`
- Create: `backend/src/repositories/skill_runs.rs`
- Modify: `backend/src/models/mod.rs`
- Modify: `backend/src/repositories/mod.rs`
- Modify: `backend/src/lib.rs`
- Modify: `backend/src/routes/mod.rs`
- Modify: `backend/src/main.rs`
- Test: `backend/src/repositories/skill_runs.rs`

- [ ] **Step 1: Write failing state-machine tests**

Assert valid transitions `QUEUED -> RUNNING -> terminal`, cancellation cannot be overwritten by success, a second active run for one user conflicts, different users can run concurrently, startup recovery fails stale active rows, terminal rows survive for less than 24 hours, and older rows cascade-delete their steps.

- [ ] **Step 2: Run tests and confirm failure**

Run: `cd backend && cargo test skill_runs::tests`

Expected: run repository symbols are missing.

- [ ] **Step 3: Implement run repository and event runtime**

Use conditional SQL updates that include the expected current status. Add a `SkillRunRuntime` containing cancellation tokens and broadcast senders keyed by run ID. Expose bounded event subscriptions without persisting raw event payloads.

- [ ] **Step 4: Implement restart recovery and cleanup job**

At startup update active rows to `FAILED` with `SERVICE_RESTARTED`. Add a periodic job every five minutes that deletes terminal rows where `completed_at <= datetime('now', '-24 hours')`.

- [ ] **Step 5: Run tests**

Run: `cd backend && cargo test skill_runs::tests`

Expected: state, concurrency, recovery, and retention tests pass.

- [ ] **Step 6: Commit**

```bash
git add backend/src/models backend/src/repositories backend/src/lib.rs backend/src/routes/mod.rs backend/src/main.rs
git commit -m "feat: persist temporary skill runs"
```

### Task 7: Implement the adaptive runner

**Files:**
- Create: `backend/src/services/skill_runner.rs`
- Modify: `backend/src/services/mod.rs`
- Test: `backend/src/services/skill_runner.rs`

- [ ] **Step 1: Write failing scripted-client tests**

Use a fake `ChatCompletionClient` with queued responses. Cover list/search/read flow, valid completion, unsupported tool rejection, malformed arguments, exact duplicate suppression, one JSON repair, second invalid result failure, forced convergence at 8 iterations and 24 calls, evidence and byte exhaustion, timeout, prompt-injection text remaining in tool data, and cancellation during a model request.

- [ ] **Step 2: Run tests and confirm failure**

Run: `cd backend && cargo test skill_runner::tests`

Expected: `SkillRunner` is missing.

- [ ] **Step 3: Implement prompt construction and loop**

Build platform messages from fixed static text, append the Skill snapshot as a lower-priority instruction, and label every file overview and tool response as untrusted evidence. Accept either tool calls or a final JSON response, never arbitrary actions. Increment iteration and call counters before executing bounded work.

- [ ] **Step 4: Implement completion validation and repair**

Deserialize the fixed result, reject empty summaries and evidence outside the ledger, and send one repair instruction containing only validation errors and the invalid response within the response-size cap. At a limit, set `tools` to empty and demand a final result that records insufficient context.

- [ ] **Step 5: Implement cancellation and event emission**

Wrap provider calls in `tokio::select!` with the run cancellation token. Emit bounded `run.started`, `tool.started`, `tool.completed`, `iteration.completed`, and terminal events. Persist step summaries without raw output.

- [ ] **Step 6: Run tests**

Run: `cd backend && cargo test skill_runner::tests`

Expected: all loop, safety, limit, repair, and cancellation tests pass.

- [ ] **Step 7: Commit**

```bash
git add backend/src/services/skill_runner.rs backend/src/services/mod.rs
git commit -m "feat: run adaptive issue skill analysis"
```

### Task 8: Expose run creation, status, SSE, cancellation, and result APIs

**Files:**
- Create: `backend/src/routes/skill_runs.rs`
- Modify: `backend/src/routes/mod.rs`
- Test: `backend/tests/skill_runs.rs`

- [ ] **Step 1: Write failing route tests**

Test guest rejection, admin rejection, any logged-in user running against a visible active Issue, foreign Skill rejection, disabled Skill rejection, absent provider, active-run conflict, status owner isolation, result availability only after success, SSE content type and terminal event, cancellation ownership, and expired run 404 behavior.

- [ ] **Step 2: Run tests and confirm failure**

Run: `cd backend && cargo test --test skill_runs`

Expected: run endpoints return 404.

- [ ] **Step 3: Implement run creation and reads**

Creation accepts `{ "skill_id": "..." }`, resolves the session user, checks the active Issue, loads an enabled owned Skill, resolves the provider, inserts the run, registers runtime state, and spawns `SkillRunner`. Status responses expose only bounded progress and sanitized errors.

- [ ] **Step 4: Implement SSE and cancellation**

Stream `text/event-stream` with no-store headers and heartbeats. Begin with an authoritative run snapshot, then broadcast events. Cancellation conditionally marks the owned active run and signals the local token; repeated cancellation is idempotent.

- [ ] **Step 5: Run route and ownership tests**

Run: `cd backend && cargo test --test skill_runs && cargo test --test ownership`

Expected: run APIs pass without regressing existing ownership behavior.

- [ ] **Step 6: Commit**

```bash
git add backend/src/routes/skill_runs.rs backend/src/routes/mod.rs backend/tests/skill_runs.rs
git commit -m "feat: expose skill run api"
```

### Task 9: Add frontend API types and client methods

**Files:**
- Modify: `frontend/src/api/types.ts`
- Modify: `frontend/src/api/client.ts`
- Test: `frontend/tests/api-auth-revalidation.behavior.test.tsx`

- [ ] **Step 1: Extend the API contract test**

Assert the client uses `/api/me/skills`, `/api/admin/ai-provider`, and Issue-scoped run creation paths, encodes identifiers, sends JSON bodies, and preserves authentication revalidation behavior for 401 responses.

- [ ] **Step 2: Run tests and confirm failure**

Run: `cd frontend && npm test`

Expected: new method references fail type checking or assertions.

- [ ] **Step 3: Add types and methods**

Define `UserSkill`, `SkillReview`, `AiProviderSettings`, `SkillRun`, `SkillRunEvent`, `SkillRunResult`, and `SkillEvidence`. Add CRUD, review, provider, run, cancel, status, and result methods. Provide an SSE URL builder rather than placing `EventSource` inside the generic request helper.

- [ ] **Step 4: Run type and API tests**

Run: `cd frontend && npm run lint && npm test`

Expected: TypeScript and client behavior tests pass.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/api/types.ts frontend/src/api/client.ts frontend/tests/api-auth-revalidation.behavior.test.tsx
git commit -m "feat: add skill runner frontend api"
```

### Task 10: Build My Skills UI

**Files:**
- Create: `frontend/src/features/skills/SkillsPage.tsx`
- Create: `frontend/src/features/skills/SkillEditor.tsx`
- Create: `frontend/src/features/skills/SkillReviewPanel.tsx`
- Modify: `frontend/src/features/auth/AccountPage.tsx`
- Test: `frontend/tests/skills.behavior.test.tsx`

- [ ] **Step 1: Write failing behavior tests**

Render the account page with an authenticated user and mock API responses. Test the two tabs, empty state, create, edit, enable/disable, delete confirmation, validation message, review request, score dimensions, warning/suggestion rendering, and score removal after content save.

- [ ] **Step 2: Run the focused test and confirm failure**

Run: `cd frontend && npx vitest run tests/skills.behavior.test.tsx`

Expected: My Skills controls are absent.

- [ ] **Step 3: Implement focused components**

Keep API state in `SkillsPage`; keep form state and client-side byte-length hints in `SkillEditor`; keep the six-dimension rendering in `SkillReviewPanel`. Preserve server errors through `normalizeApiError` and require confirmation before delete or discarding dirty edits.

- [ ] **Step 4: Integrate account tabs**

Retain the existing password form unchanged inside `Account security`; render `SkillsPage` under `My Skills`. Continue redirecting guests and administrators according to current behavior.

- [ ] **Step 5: Run tests and type checking**

Run: `cd frontend && npx vitest run tests/skills.behavior.test.tsx && npm run lint`

Expected: Skill UI tests and TypeScript pass.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/features/skills frontend/src/features/auth/AccountPage.tsx frontend/tests/skills.behavior.test.tsx
git commit -m "feat: manage private skills in account"
```

### Task 11: Build administrator AI provider UI

**Files:**
- Create: `frontend/src/features/admin/AiProviderSettings.tsx`
- Modify: `frontend/src/features/admin/AdminPage.tsx`
- Test: `frontend/tests/ai-provider.behavior.test.tsx`

- [ ] **Step 1: Write failing form tests**

Test masked-key display, blank replacement input, effective source labels, save success, validation failure, connection-test success/failure, no-master-key error, and confirmation that returned key material is never placed into the password input.

- [ ] **Step 2: Run the focused test and confirm failure**

Run: `cd frontend && npx vitest run tests/ai-provider.behavior.test.tsx`

Expected: provider settings card is absent.

- [ ] **Step 3: Implement and integrate the card**

Use a password input with empty initial value, numeric timeout bounded to `1..=300`, explicit `保存配置` and `测试连接` actions, and source/readiness badges. Add the card to the existing `AdminSettingsPage` without exposing user Skills.

- [ ] **Step 4: Run tests and type checking**

Run: `cd frontend && npx vitest run tests/ai-provider.behavior.test.tsx && npm run lint`

Expected: administrator UI tests and TypeScript pass.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/features/admin/AiProviderSettings.tsx frontend/src/features/admin/AdminPage.tsx frontend/tests/ai-provider.behavior.test.tsx
git commit -m "feat: configure ai provider in admin"
```

### Task 12: Build the Issue runner and evidence navigation

**Files:**
- Create: `frontend/src/features/skill-runs/useSkillRun.ts`
- Create: `frontend/src/features/skill-runs/IssueSkillRunner.tsx`
- Create: `frontend/src/features/skill-runs/SkillRunResult.tsx`
- Modify: `frontend/src/features/files/FilesView.tsx`
- Test: `frontend/tests/skill-runner.behavior.test.tsx`

- [ ] **Step 1: Write failing runner UI tests**

Cover guest, missing provider, empty Skill list, disabled run due to conflict, start, progress counters, SSE update, SSE failure followed by status recovery, cancel, structured result sections, expired result, and evidence-click callback carrying file ID plus line range.

- [ ] **Step 2: Run the focused test and confirm failure**

Run: `cd frontend && npx vitest run tests/skill-runner.behavior.test.tsx`

Expected: runner components do not exist.

- [ ] **Step 3: Implement the run hook**

`useSkillRun` creates a run, stores the current run ID in session storage keyed by Issue, opens `EventSource`, polls status after stream failure, closes streams on terminal state/unmount, and exposes `start`, `cancel`, `status`, `progress`, `result`, and `error`.

- [ ] **Step 4: Implement controls and result rendering**

`IssueSkillRunner` loads enabled Skills and displays exact disabled reasons. `SkillRunResult` renders summary, observations, inferences, missing context, and evidence separately. Evidence buttons call `onOpenEvidence({ fileId, startLine, endLine })`.

- [ ] **Step 5: Integrate with the existing viewer**

Place the compact runner above the current Issue workspace. Resolve an evidence file through existing Issue tree loading, select its node, open the viewer tab, set the page start, and set the target line. Do not add a chat input or run history.

- [ ] **Step 6: Run tests and type checking**

Run: `cd frontend && npx vitest run tests/skill-runner.behavior.test.tsx && npm run lint`

Expected: runner behavior, evidence navigation, and TypeScript pass.

- [ ] **Step 7: Commit**

```bash
git add frontend/src/features/skill-runs frontend/src/features/files/FilesView.tsx frontend/tests/skill-runner.behavior.test.tsx
git commit -m "feat: run skills from issue viewer"
```

### Task 13: Security regression, documentation, and release verification

**Files:**
- Modify: `README.md`
- Modify: `doc/DB.md`
- Modify: `frontend/package.json`
- Test: `backend/tests/ai_provider.rs`
- Test: `backend/tests/skill_runs.rs`
- Test: `frontend/tests/skills.behavior.test.tsx`
- Test: `frontend/tests/ai-provider.behavior.test.tsx`
- Test: `frontend/tests/skill-runner.behavior.test.tsx`

- [ ] **Step 1: Add explicit security regression fixtures**

Add a log line containing instructions to ignore rules, call a shell, read another Issue, and emit a forged evidence range. Assert the fake provider sees it only inside an untrusted tool message, the tool set remains exactly three functions, the foreign read is rejected, and the forged final evidence fails validation. Add a provider secret fixture and assert it is absent from errors and captured tracing output.

- [ ] **Step 2: Run security tests**

Run: `cd backend && cargo test --test ai_provider --test skill_runs`

Expected: injection, cross-Issue, forged evidence, and redaction tests pass.

- [ ] **Step 3: Update operator and database documentation**

Document provider environment variables, master-key requirements, database-over-environment precedence, private Skill behavior, fixed runner limits, 24-hour result retention, new tables, and the absence of built-in Skills and diagnostic history.

- [ ] **Step 4: Register all frontend tests in the standard script**

Ensure `npm test` runs the three new Vitest files as part of the existing `vitest run` phase and retains all existing Node behavior tests.

- [ ] **Step 5: Run complete verification**

Run:

```bash
cd backend && cargo fmt --check
cd backend && cargo test
cd frontend && npm test
cd frontend && npm run lint
cd frontend && npm run build
git diff --check
git status --short
```

Expected: every command exits 0; status contains only intentional tracked changes before the final commit.

- [ ] **Step 6: Commit final integration**

```bash
git add README.md doc/DB.md backend frontend
git commit -m "test: verify secure skill runner workflow"
```

- [ ] **Step 7: Review and publish**

Review the complete diff against `docs/superpowers/specs/2026-08-03-issue-79-skill-runner-design.md`, push `codex/issue-79-skill-runner`, and open one pull request referencing and closing issue #79. The PR body must summarize provider security, private Skill ownership, bounded tools, cancellation, temporary retention, and the complete verification commands.
