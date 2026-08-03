# Issue #79: User Skill Runner Design

## Status and scope

This design implements GitHub issue #79 as one pull request composed of small,
independently verifiable commits. It preserves the issue's security model and
adaptive retrieval goals, with three product decisions confirmed after the issue
was written:

- Rain will not ship or support built-in Skills. Every Skill is private and owned
  by one user.
- Completed, failed, and cancelled diagnostic runs are temporary. They are kept
  for 24 hours for refresh and reconnect support, then deleted. Rain will not
  expose a diagnostic history list.
- Each Skill keeps at most one quality review for its current version. Editing
  the Skill clears that review; Rain will not retain review history.

The implementation remains a Rust/Actix and SQLite monolith with the existing
React/Vite frontend. It does not add general chat, shell access, network access
for Skills, user scripts, MCP, external CLIs, vector search, or write access to
Issue data.

## Product behavior

### Skill ownership

An authenticated non-admin user can create, read, update, enable, disable,
delete, and review only their own Skills. A Skill contains only:

- name;
- optional description; and
- `SKILL.md` Markdown.

Names are unique within one user's Skills. A content change increments the
version and content hash and removes the current quality review. Enabling or
disabling a Skill does not create a new version. Administrators cannot browse or
manage user Skills through product APIs or pages.

### Issue access and execution

Skill execution follows Issue read access rather than Issue ownership. Any
authenticated non-admin user who can view an active Issue can run one of their
own enabled Skills against it, even if another user created the Issue. Guests
cannot run Skills. A Skill never expands access to an Issue.

Only one run per user may be `QUEUED` or `RUNNING`. The restriction is global
across Issues and must be enforced atomically in SQLite.

### Temporary run results

A run is bound server-side to the initiating user, target Issue, Skill ID, Skill
version, and a temporary Skill content snapshot. The snapshot allows an active
run to finish consistently if the user edits or deletes its source Skill.

Terminal runs remain directly addressable by run ID for 24 hours so a page can
refresh or reconnect. No endpoint or UI lists old runs. Terminal run rows,
steps, snapshots, and results are deleted after 24 hours. Deleting an Issue or
user immediately cascades to their associated runs.

## Architecture

### AI provider

The backend adds an `ai_provider` boundary responsible for:

- resolving the effective configuration;
- making OpenAI-compatible Chat Completions requests;
- serializing tool definitions and multi-turn messages;
- parsing tool calls and final structured JSON;
- enforcing request timeouts and response size limits; and
- redacting credentials and sensitive request content from errors and logs.

The first version supports one effective provider. A valid database setting has
priority over environment variables. Environment fallback uses:

```dotenv
RAIN_AI_BASE_URL=http://127.0.0.1:8000/v1
RAIN_AI_API_KEY=...
RAIN_AI_MODEL=...
RAIN_AI_TIMEOUT_SECONDS=120
```

Database API keys are encrypted with authenticated encryption under a 32-byte
server master key supplied by `RAIN_AI_MASTER_KEY`. The configuration API never
returns ciphertext or plaintext. It returns whether a key is configured and a
mask suitable for display. An update without a new key preserves the current
key only while the Base URL is unchanged; changing the endpoint requires a new
key. Without a valid master key, environment configuration can still run, but
the server rejects attempts to persist a database API key.

The administrator connection test performs a small bounded request. An empty
request tests the current effective configuration. Testing unsaved values
requires a complete Base URL, API key, model, and timeout, so a stored secret is
never combined with a candidate endpoint. It returns diagnostics without
echoing credentials, authorization headers, or full model responses.

### Skill service

The `skills` boundary owns deterministic validation, persistence, ownership,
versioning, and quality evaluation. Validation rejects:

- blank names or Markdown;
- values beyond fixed platform size limits;
- duplicate names for the same user; and
- malformed text payloads.

Requests inside `SKILL.md` for unsupported tools do not grant permissions. They
are allowed to save, but the quality review reports them as conflicts.

Quality review uses the effective AI provider and a fixed rubric version. It
returns a fixed result containing:

```json
{
  "overall_score": 72,
  "grade": "NEEDS_IMPROVEMENT",
  "dimensions": {},
  "warnings": [],
  "suggestions": []
}
```

The six dimensions retain the weights defined in issue #79. Low scores warn but
never prevent execution. Re-evaluation replaces the current row. Editing the
Skill deletes it, leaving the UI in a `not evaluated` state.

### Issue-scoped tools

The `skill_tools` boundary exposes exactly three internal functions:

- `list_files`: lists files from `READY` Bundles in the bound Issue, with a stable cursor and optional path-prefix filter;
- `search_logs`: searches indexed text in that Issue and returns at most 20
  bounded matches; and
- `read_file_lines`: reads a bounded line range from a file in a `READY` Bundle
  belonging to that Issue.

The model cannot provide an Issue code. Tool execution begins with a trusted run
context resolved from the database. File reads accept only a file ID and line
range, verify membership in the bound Issue, cap lines and bytes, and use the
existing blob-backed line reader. Search reuses the existing FTS index without
allowing arbitrary SQL.

All tool arguments are schema-validated. Identical searches are executed once.
Previously read ranges are subtracted from later requests; overlapping evidence
ranges are merged. The runner maintains an evidence ledger so final references
can only cite files and ranges actually returned by a tool.

### Controlled adaptive runner

The `skill_runner` boundary owns a run from creation through a terminal state.
Its fixed limits are:

```text
execution_mode             = adaptive
scope                      = current_issue
allowed_tools              = list_files, search_logs, read_file_lines
max_iterations             = 8
max_tool_calls             = 24
search_results_per_call    = 20
max_evidence_ranges        = 30
max_tool_output_bytes      = 32 KiB
max_total_evidence_bytes   = 128 KiB
run_timeout                = 120 seconds
per_user_concurrency       = 1
terminal_retention         = 24 hours
```

The runner sends a fixed platform prompt, the user's Skill snapshot, and an
Issue overview. Instruction priority is always:

```text
platform security rules
> Rain runner rules
> user SKILL.md
> filenames, logs, and tool output
```

Files, filenames, logs, and tool output are explicitly untrusted evidence. Text
inside them cannot alter tools, scope, permissions, limits, or the output shape.

The model uses standard Chat Completions tool calls. Each assistant response
either requests validated tools or supplies the final fixed JSON result. Tool
responses are appended as untrusted data. A response that mixes an invalid final
result with unsupported calls is rejected.

When the model stops requesting tools, the runner validates this result:

```json
{
  "summary": "...",
  "observations": [{"text": "...", "evidence_ids": ["e1"]}],
  "inferences": [{"text": "...", "confidence": "MEDIUM", "evidence_ids": ["e1"]}],
  "missing_context": [],
  "evidence": []
}
```

Evidence entries have a unique evidence ID, Bundle hash, stable file ID, display
path, start line, end line, bounded excerpt, and explanation. Every observation
and inference cites at least one evidence ID, and each cited range must occur in
the evidence ledger. Invalid output receives one bounded repair attempt. A second invalid
response fails the run with a stable error code.

At any iteration, call, byte, range, or time limit, the runner disables further
tools and asks for a final result based on current evidence. The final instruction
requires an explicit statement of insufficient evidence. This forced completion
request is itself bounded by the overall timeout.

Cancellation persists a cancellation request and signals the local task. The
runner checks cancellation before and after model requests and tool calls. It
drops an in-flight model request where possible and will not execute later tool
calls. Conditional terminal-state updates prevent a late model response from
overwriting `CANCELLED`.

On startup, stale `QUEUED` or `RUNNING` rows are changed to `FAILED` with a
sanitized service-restart error. A periodic cleanup follows the repository's
existing cleanup-loop pattern and deletes terminal rows older than 24 hours.

## Data model

### `user_skills`

```sql
CREATE TABLE user_skills (
    id TEXT PRIMARY KEY,
    owner_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT,
    skill_markdown TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(owner_user_id, name)
);
```

### `skill_reviews`

```sql
CREATE TABLE skill_reviews (
    skill_id TEXT PRIMARY KEY REFERENCES user_skills(id) ON DELETE CASCADE,
    skill_version INTEGER NOT NULL,
    skill_content_hash TEXT NOT NULL,
    reviewer_model TEXT NOT NULL,
    rubric_version TEXT NOT NULL,
    overall_score INTEGER NOT NULL,
    grade TEXT NOT NULL,
    dimension_scores_json TEXT NOT NULL,
    findings_json TEXT NOT NULL,
    evaluated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
```

### `skill_runs` and `skill_run_steps`

`skill_runs` stores the owner, Issue, source Skill identity, version, temporary
snapshot, state, counters, cancellation flag, final JSON, sanitized failure, and
timestamps. It references the user and Issue with cascading deletion. The source
Skill ID is intentionally not a foreign key so deleting a Skill does not break
an active temporary run.

`skill_run_steps` references a run with cascading deletion. It stores sequence,
iteration, tool name, sanitized argument summary, hit count, evidence reference
metadata, elapsed milliseconds, status, and timestamps. It never stores complete
raw tool output.

A partial unique index on `skill_runs(user_id)` for `QUEUED` and `RUNNING`
states atomically enforces per-user concurrency.

### Provider settings

The existing `system_settings` mechanism stores non-secret provider values and
an encrypted key envelope. The envelope includes version and nonce so encryption
can evolve. Secret writes are only accepted when the server has a valid master
key. Public and admin read models never contain the envelope.

## HTTP API

### Current user's Skills

```text
GET    /api/me/skills
GET    /api/me/skills/{skill_id}
POST   /api/me/skills
PUT    /api/me/skills/{skill_id}
DELETE /api/me/skills/{skill_id}
POST   /api/me/skills/{skill_id}/review
```

All routes derive the owner from the authenticated session and reject guests and
administrators. There is no owner ID parameter and no reviews-history endpoint.

### Runs

```text
POST   /api/issues/{issue_code}/skill-runs
GET    /api/skill-runs/{run_id}
GET    /api/skill-runs/{run_id}/events
POST   /api/skill-runs/{run_id}/cancel
GET    /api/skill-runs/{run_id}/result
```

Creation accepts only a Skill ID. The backend derives the user, validates Skill
ownership, resolves Issue visibility, and creates the binding. Run reads,
events, cancellation, and results require the initiating user. There is no list
endpoint.

The event endpoint uses server-sent events and emits:

```text
run.started
tool.started
tool.completed
iteration.completed
run.completed
run.failed
run.cancelled
```

Events contain bounded progress metadata, never raw logs or prompts. A client
that misses events calls `GET run` to obtain the authoritative snapshot.

### Administrator provider configuration

```text
GET  /api/admin/ai-provider
PUT  /api/admin/ai-provider
POST /api/admin/ai-provider/test
```

These reuse the existing administrator guard. Read responses report effective
source, readiness, base URL, model, timeout, and key mask. They never expose a
secret. Writes and connection tests create sanitized audit entries without key
material.

## Frontend design

### My Skills

The account page becomes a tabbed page with `Account security` and `My Skills`.
The Skills tab lists name, description, version, enabled state, and current score.
It supports create, edit, enable or disable, delete with confirmation, and manual
quality review.

The editor contains only name, description, and a multiline `SKILL.md` editor.
The review panel shows the overall score, grade, six dimensions, warnings, and
suggestions. Editing content immediately displays `not evaluated` after save.

### Administrator settings

The existing system settings page gains an AI model service card for base URL,
replacement API key, model, timeout, effective source, readiness, save, and test
connection. The key input is always blank and explains that leaving it blank
preserves the configured database key.

### Issue runner

The Issue file browser gains a compact Skill runner above its main workspace. It
contains a selector of the current user's enabled Skills, Run button, bounded
progress, and Cancel button. It has no prompt input and no chat bubbles.

The completed view presents summary, observations, inferences, missing context,
and evidence as separate sections. Clicking evidence reuses the existing file
viewer, opens the referenced file, and jumps to the cited lines. The UI shows
specific disabled reasons for a guest, missing provider, no enabled Skill,
another active run, or unavailable Issue.

The current run ID is retained in page state and recoverable URL state. This is
only a 24-hour refresh/reconnect mechanism, not a discoverable history feature.

## Error model

The API adds stable error codes for provider absence, invalid provider settings,
provider timeout or response failure, invalid Skill, ownership denial, active-run
conflict, invalid tool request, evidence limit, invalid model output, cancellation,
and expired run. Messages exposed to clients are sanitized.

Model errors never include authorization headers, API keys, complete requests,
full model responses, Skill Markdown, or raw evidence. Server logs use request
IDs, run IDs, phase, status code, counts, byte sizes, and elapsed time.

## Verification strategy

Backend tests cover:

- Skill validation, uniqueness, ownership, versioning, enabled state, review
  replacement, and review deletion after an edit;
- administrator guards, configuration precedence, key encryption, masking,
  replacement, master-key failure, test connection, and secret-free logs/errors;
- execution by a logged-in non-owner of a visible active Issue, and rejection of
  guests or another user's Skill;
- Issue binding for all three tools, `READY` Bundle checks, range validation,
  duplicate searches, overlapping reads, byte and evidence limits;
- normal convergence, malformed calls, one repair attempt, model failure, forced
  convergence, eight iterations, 24 calls, timeout, cancellation, and atomic
  terminal transitions;
- prompt-injection fixtures that remain data and cannot expand capabilities;
- SSE snapshots, restart recovery, concurrency conflict, and 24-hour cleanup.

Frontend tests cover:

- account-page tabs and Skill lifecycle interactions;
- current-review rendering and invalidation;
- administrator provider form, masking, source, connection test, and errors;
- runner disabled reasons, progress, cancellation, reconnect, structured result,
  and evidence navigation.

Completion requires Rust formatting and tests, frontend tests and type checking,
the production frontend build, diff whitespace checks, and confirmation that no
credential-like values appear in generated logs or fixtures.

## Delivery sequence

The single pull request uses focused commits in this order:

1. database schema, limits, provider configuration, and encrypted secret storage;
2. user Skill CRUD, ownership, validation, versioning, and current review;
3. Issue-scoped read-only tools and evidence ledger;
4. adaptive runner, cancellation, SSE, restart recovery, and cleanup;
5. account and administrator UI;
6. Issue runner and structured evidence UI;
7. security, integration, documentation, and full regression tests.

Each commit must leave its affected layer testable. The PR is opened only after
the complete issue scope passes the final verification suite.
