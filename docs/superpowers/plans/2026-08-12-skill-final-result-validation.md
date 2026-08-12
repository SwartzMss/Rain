# Skill Final Result Validation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Skill final-result validation actionable and safe, add allow-listed validation fields and targeted repair prompts, and opt into strict JSON Schema output when explicitly configured.

**Architecture:** Keep final-result validation in `backend/src/services/skill_runner.rs`, replacing the single `MissingField` bucket with a safe `(reason, field)` error. Add a provider-level structured-output mode that defaults to `json_object` and is exposed to the runner through the chat client trait. Final and repair requests select either the current object format or a fixed strict schema; tool requests remain unchanged.

**Tech Stack:** Rust, serde/serde_json, reqwest OpenAI-compatible chat completions, Tokio tests, SQLite integration tests, Vite frontend build.

---

### Task 1: Add the explicit structured-output capability mode

**Files:**
- Modify: `backend/src/config.rs`
- Modify: `backend/src/ai_provider/config.rs`
- Modify: `backend/src/ai_provider/client.rs`
- Modify: `backend/src/ai_provider/observability.rs`
- Test: `backend/src/config.rs`
- Test: `backend/src/ai_provider/client.rs`

- [ ] **Step 1: Add a failing configuration test**

Add tests for the new environment setting:

```rust
assert_eq!(StructuredOutputMode::parse(None).unwrap(), StructuredOutputMode::JsonObject);
assert_eq!(StructuredOutputMode::parse(Some("json_schema")).unwrap(), StructuredOutputMode::JsonSchema);
assert!(StructuredOutputMode::parse(Some("unsupported")).is_err());
```

Add a client test that constructs the default scripted/test provider and asserts its mode is `JsonObject`.

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
cargo test config::tests::structured_output_mode --lib -- --nocapture
cargo test ai_provider::client::tests::structured_output_mode --lib -- --nocapture
```

Expected: compile failures because `StructuredOutputMode` and the client capability accessor do not exist.

- [ ] **Step 3: Implement the mode and propagate it through provider resolution**

Define a copyable enum with stable strings:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructuredOutputMode {
    JsonObject,
    JsonSchema,
}
```

Parse `RAIN_AI_STRUCTURED_OUTPUT`, default to `JsonObject`, and reject every value other than `json_object` and `json_schema` with a configuration error. Store the mode in `AiProviderEnv` and `ResolvedAiProvider`; `AiProviderEnv::from_values` and `ResolvedAiProvider::candidate` must retain the JSON-object default so existing callers remain compatible. Use the environment mode for both database-backed and environment-backed providers.

Add a default trait method to `ChatCompletionClient`:

```rust
fn structured_output_mode(&self) -> StructuredOutputMode {
    StructuredOutputMode::JsonObject
}
```

Make `OpenAiChatClient` return the resolved mode. Extend provider observability’s fixed `response_format` labels to accept `json_schema` without logging any provider data.

- [ ] **Step 4: Run the focused tests and format**

Run:

```bash
cargo fmt --all -- --check
cargo test config::tests::structured_output_mode --lib -- --nocapture
cargo test ai_provider::client::tests::structured_output_mode --lib -- --nocapture
```

Expected: all focused tests pass and existing scripted clients continue to use `json_object`.

- [ ] **Step 5: Commit the provider capability change**

```bash
git add backend/src/config.rs backend/src/ai_provider/config.rs backend/src/ai_provider/client.rs backend/src/ai_provider/observability.rs
git commit -m "feat: configure structured result output mode"
```

### Task 2: Replace coarse result validation reasons with safe shape/type validation

**Files:**
- Modify: `backend/src/services/skill_runner.rs`
- Test: `backend/src/services/skill_runner.rs`

- [ ] **Step 1: Add failing unit tests for reason and field mapping**

Add table-driven tests around `parse_result`/`validate_result` for these payloads and expected pairs:

```text
{}                                      -> missing_top_level_field + summary
missing evidence                        -> missing_top_level_field + evidence
summary missing evidence_ids            -> missing_nested_field + summary.evidence_ids
summary.status = 1                      -> invalid_field_type + summary.status
summary.status = "MAYBE"                -> invalid_summary_status + summary.status
summary.extra = true                    -> unknown_field + summary
summary.evidence_ids = "e1"             -> invalid_field_type + summary.evidence_ids
observations[0].text = ""               -> empty_required_text + observations[].text
observations = [51 items]               -> invalid_array_size + observations
missing_context = "gap"                 -> invalid_field_type + missing_context
INSUFFICIENT_EVIDENCE with [] context   -> invalid_missing_context + missing_context
serialized result > 256 KiB             -> result_too_large + null
```

Add tests that unknown model keys never appear in `validation_field` or the display reason, and that evidence reference and unsupported-claim failures preserve their existing categories.

- [ ] **Step 2: Run the validation tests and verify they fail**

Run:

```bash
cargo test services::skill_runner::tests::result_validation_returns_safe_reasons --lib -- --nocapture
cargo test services::skill_runner::tests::result_validation_maps_fields --lib -- --nocapture
```

Expected: existing `missing_field` assertions fail or the new tests do not compile, proving the tests exercise behavior absent from the current implementation.

- [ ] **Step 3: Introduce `ValidationField` and `ResultValidationError`**

Replace the current reason-only return type with:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValidationField {
    Summary,
    SummaryStatus,
    SummaryText,
    SummaryEvidenceIds,
    Observations,
    ObservationText,
    ObservationEvidenceIds,
    Inferences,
    InferenceText,
    InferenceConfidence,
    InferenceEvidenceIds,
    MissingContext,
    Evidence,
    EvidenceId,
    EvidenceBundleHash,
    EvidenceFileId,
    EvidencePath,
    EvidenceStartLine,
    EvidenceEndLine,
    EvidenceExcerpt,
    EvidenceExplanation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResultValidationError {
    reason: ResultValidationReason,
    field: Option<ValidationField>,
}
```

Implement `ValidationField::as_str()` with only fixed strings such as `summary`, `summary.status`, `summary.evidence_ids`, `observations`, `observations[].text`, `observations[].evidence_ids`, `inferences`, `inferences[].text`, `inferences[].confidence`, `inferences[].evidence_ids`, `missing_context`, `evidence`, and the documented evidence subfields. Implement `ResultValidationReason::as_str()` for `invalid_json`, `missing_top_level_field`, `missing_nested_field`, `unknown_field`, `invalid_field_type`, `invalid_summary_status`, `invalid_confidence`, `empty_required_text`, `invalid_array_size`, `invalid_missing_context`, `invalid_evidence_reference`, `unsupported_claim`, and `result_too_large`. Keep `run_error()` mapping unchanged.

- [ ] **Step 4: Implement explicit top-level and nested shape checks**

Before `serde_json::from_value`, require the root object, reject unknown top-level keys, require the five top-level fields, require `summary` to be an object, and validate each nested object against its fixed key set. Return the parent allow-listed field for unknown nested keys; return `None` when the unknown key cannot be safely identified. Check scalar/array/object types explicitly so deserialization cannot collapse them into `missing_field`.

- [ ] **Step 5: Map business constraints to actionable reasons**

After successful deserialization, preserve the current safety rules but map them as follows:

- empty or oversized required text → `empty_required_text` or `result_too_large` with the relevant fixed field;
- collection count over the existing limit → `invalid_array_size` with the collection field;
- invalid `INSUFFICIENT_EVIDENCE` context/evidence IDs → `invalid_missing_context` with `missing_context` or `summary.evidence_ids`;
- unsupported observation/inference/summary claim → `unsupported_claim` with the relevant evidence-id field;
- oversized serialized result → `result_too_large` with `None`.

Keep `validate_evidence` as the final server-side gate and return `invalid_evidence_reference` with `evidence` when its ledger check fails.

- [ ] **Step 6: Run the validation tests and commit**

Run:

```bash
cargo fmt --all -- --check
cargo test services::skill_runner::tests::result_validation_returns_safe_reasons --lib -- --nocapture
cargo test services::skill_runner::tests::result_validation_maps_fields --lib -- --nocapture
```

Then commit:

```bash
git add backend/src/services/skill_runner.rs
git commit -m "fix: classify skill result validation failures"
```

### Task 3: Add the fixed SkillRunResult JSON Schema

**Files:**
- Modify: `backend/src/services/skill_runner.rs`
- Test: `backend/src/services/skill_runner.rs`

- [ ] **Step 1: Add a failing schema test**

Add a test that obtains `skill_result_response_format(StructuredOutputMode::JsonSchema)` and asserts:

```text
type == "json_schema"
json_schema.name == "skill_run_result"
json_schema.strict == true
schema.additionalProperties == false
schema.required == [summary, observations, inferences, missing_context, evidence]
summary.status has enum [SUPPORTED, INSUFFICIENT_EVIDENCE]
inference.confidence has enum [LOW, MEDIUM, HIGH]
```

Also assert the JSON-object mode remains exactly `{"type":"json_object"}`.

- [ ] **Step 2: Run the schema test and verify it fails**

Run:

```bash
cargo test services::skill_runner::tests::skill_result_response_format --lib -- --nocapture
```

Expected: compile failure because the response-format helper does not exist.

- [ ] **Step 3: Implement the schema builder**

Create a pure `skill_result_schema() -> Value` with fixed required properties and `additionalProperties: false` on every object. Express the existing server limits where JSON Schema can represent them: enum values, required fields, array `maxItems`, string `maxLength`, evidence line integer types, and bounded evidence fields. Create:

```rust
fn skill_result_response_format(mode: StructuredOutputMode) -> Value
```

which returns either the current JSON object shape or:

```json
{
  "type": "json_schema",
  "json_schema": {
    "name": "skill_run_result",
    "strict": true,
    "schema": {
      "type": "object",
      "additionalProperties": false,
      "required": ["summary", "observations", "inferences", "missing_context", "evidence"],
      "properties": {
        "summary": { "type": "object", "additionalProperties": false },
        "observations": { "type": "array", "maxItems": 50 },
        "inferences": { "type": "array", "maxItems": 50 },
        "missing_context": { "type": "array", "maxItems": 50 },
        "evidence": { "type": "array", "maxItems": 30 }
      }
    }
  }
}
```

Do not remove server-side validation; schema output cannot validate EvidenceLedger membership.

- [ ] **Step 4: Run the schema tests and commit**

Run:

```bash
cargo fmt --all -- --check
cargo test services::skill_runner::tests::skill_result_response_format --lib -- --nocapture
```

Then commit:

```bash
git add backend/src/services/skill_runner.rs
git commit -m "feat: define strict skill result schema"
```

### Task 4: Use the capability mode and generate targeted repair prompts

**Files:**
- Modify: `backend/src/services/skill_runner.rs`
- Modify: `backend/src/ai_provider/observability.rs`
- Test: `backend/tests/skill_runner.rs`

- [ ] **Step 1: Add failing Runner request-shape tests**

Extend the recording client test fixture with a configurable `StructuredOutputMode`. Add two integration tests that return an invalid first result followed by a valid insufficient-evidence result. Assert that final and repair requests both carry `json_object` in default mode and the complete `json_schema` object in schema mode. Assert tool-enabled requests still have `response_format: null`.

Add a repair-prompt assertion for a missing `evidence` payload: the repair request must contain the fixed field and targeted instruction, while a payload containing a key such as `model_secret` must not put that key in logs or prompt text.

- [ ] **Step 2: Run the request-shape tests and verify they fail**

Run:

```bash
cargo test --test skill_runner result_repair_uses_configured_response_format -- --nocapture
cargo test --test skill_runner repair_prompt_targets_validation_field -- --nocapture
```

Expected: the current implementation always sends `json_object` and the repair prompt is generic, so the new assertions fail.

- [ ] **Step 3: Select response format from the client capability**

Replace the hard-coded final and repair `json_object` values with `skill_result_response_format(client.structured_output_mode())`. Keep the model/tool request’s `response_format: None`. Pass the selected static label (`json_object` or `json_schema`) into `ProviderRequestContext` so observability remains accurate and allow-listed.

- [ ] **Step 4: Build a safe targeted repair prompt**

Add a formatter that accepts `ResultValidationError` and emits fixed text for each reason. Include `ValidationField::as_str()` only when present. For unknown-field errors, mention only the fixed parent field and say “remove fields not in the schema”; never interpolate the model’s key. Append the existing complete-result requirements so targeted guidance cannot weaken evidence and claim rules.

- [ ] **Step 5: Update validation logging without leaking model data**

Log `validation_reason` and `validation_field` from the safe error value on both initial and repair failure. Keep the existing repair attempt count, run error mapping, and absence of raw result content.

- [ ] **Step 6: Run integration tests and commit**

Run:

```bash
cargo fmt --all -- --check
cargo test --test skill_runner result_repair_uses_configured_response_format -- --nocapture
cargo test --test skill_runner repair_prompt_targets_validation_field -- --nocapture
cargo test --test skill_runner runner_repairs_a_structured_result_with_forged_evidence -- --nocapture
```

Then commit:

```bash
git add backend/src/services/skill_runner.rs backend/src/ai_provider/observability.rs backend/tests/skill_runner.rs
git commit -m "feat: target skill result repairs"
```

### Task 5: Complete regression coverage and verify the branch

**Files:**
- Modify: `backend/src/config.rs`
- Modify: `backend/src/ai_provider/client.rs`
- Modify: `backend/src/ai_provider/config.rs`
- Modify: `backend/src/ai_provider/observability.rs`
- Modify: `backend/src/services/skill_runner.rs`
- Modify: `backend/tests/skill_runner.rs`
- Modify: `backend/tests/ai_provider.rs`
- Modify: `backend/tests/skills.rs`

- [ ] **Step 1: Update existing log expectations**

Keep review requests using `json_object`, and update only Skill Runner final/repair expectations to use the configured mode label. Add a test proving malformed evidence remains `SKILL_EVIDENCE_INVALID`, unsupported claims remain `SKILL_RESULT_INVALID`, and a repair that remains invalid does not expose the detailed reason in the public error message.

- [ ] **Step 2: Run focused and regression suites**

Run:

```bash
cargo test services::skill_runner::tests --lib -- --nocapture
cargo test --test skill_runner -- --nocapture
cargo test --test ai_provider -- --nocapture
cargo test --test skills -- --nocapture
```

Expected: all focused suites pass; structured-output configuration does not change review behavior or Provider retry behavior.

- [ ] **Step 3: Run the complete verification suite**

Run sequentially:

```bash
cargo fmt --all -- --check
npm run build
cargo test
cargo test --test smoke passive_cleanup_checkpoint_does_not_wait_for_a_reader -- --nocapture --test-threads=1
cargo test --test smoke upload_search_tree_and_delete_issue -- --nocapture --test-threads=1
git diff --check
git status --short --branch
```

Expected: formatting, frontend build, complete Rust tests, both smoke tests, and diff checks pass; the full suite may report its existing parallel smoke-test timing sensitivity, which must be recorded separately if it recurs while each affected test passes alone.

- [ ] **Step 4: Commit any final test-only changes**

```bash
git add backend/src/config.rs backend/src/ai_provider/client.rs backend/src/ai_provider/config.rs backend/src/ai_provider/observability.rs backend/src/services/skill_runner.rs backend/tests/skill_runner.rs backend/tests/ai_provider.rs backend/tests/skills.rs
git commit -m "test: cover skill result validation and fallback"
```

- [ ] **Step 5: Inspect the final diff before publishing**

Run:

```bash
git diff origin/main...HEAD --stat
git diff origin/main...HEAD --check
git status --short --branch
```

Confirm no frontend lockfile, generated build output, raw model output, or unrelated files are included.
