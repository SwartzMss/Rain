# Issue #120 Unified Final Result Contract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate Skill final-result contract drift and make `unknown_field` repair actionable in both structured-output modes.

**Architecture:** Introduce static typed contracts for the result and nested objects. Drive shape validation, strict JSON Schema, finalization instructions, repair instructions, and safe diagnostics from those contracts while preserving all semantic and EvidenceLedger validation.

**Tech Stack:** Rust, serde/serde_json, tracing, Tokio, Actix, SQLx test harness.

---

### Task 1: Lock the failure and contract behavior with tests

**Files:**
- Modify: `backend/src/services/skill_runner.rs`
- Modify: `backend/tests/skill_runner.rs`

- [ ] Add unit cases for unknown fields in the top-level, summary, observation, inference, and evidence objects.
- [ ] Assert unknown-field errors carry only the affected allow-list and unknown-field count.
- [ ] Assert finalization and repair prompts enumerate the exact evidence fields and types.
- [ ] Add a Runner test with `reasoning -> evidence containing source -> corrected evidence` and verify the repair request and safe logs.
- [ ] Run focused tests and confirm they fail before implementation.

### Task 2: Implement the authoritative contract

**Files:**
- Modify: `backend/src/services/skill_runner.rs`

- [ ] Define typed static contracts for top-level, summary, observation, inference, and evidence fields.
- [ ] Change shape validation to derive required/allowed fields and unknown-field metadata from the contracts.
- [ ] Generate nested strict JSON Schema properties and required arrays from the same contracts.
- [ ] Render exact finalization and `unknown_field` repair instructions from the contracts.
- [ ] Render an explicit `read_file_lines` call/response to final evidence conversion policy from the evidence contract.
- [ ] Add safe `validation_allowed_fields` and `validation_unknown_field_count` log fields without logging unknown keys or values.

### Task 3: Verify behavior and preserved security properties

**Files:**
- Test: `backend/src/services/skill_runner.rs`
- Test: `backend/tests/skill_runner.rs`

- [ ] Run focused unit and Runner integration tests.
- [ ] Confirm strict schema keeps `additionalProperties: false` and both response modes use the same finalization/repair path.
- [ ] Confirm EvidenceLedger, unsupported-claim, storage-failure, cancellation, and provider-error tests remain green.

### Task 4: Full verification and publication

- [ ] Run `cargo fmt --all -- --check`, `cargo test --all-targets`, and Clippy from `backend`.
- [ ] Run `npm test` and `npm run build` from `frontend`.
- [ ] Run `git diff --check` and inspect the scoped diff.
- [ ] Commit, push `agent/issue-120-final-result-contract`, and create a draft PR against `main` containing `Fixes #120`.
