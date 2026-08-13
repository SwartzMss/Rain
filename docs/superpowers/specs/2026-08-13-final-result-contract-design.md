# Unified Skill Final Result Contract

## Problem

Rain currently repeats the `SkillRunResult` object shape in the shape validator, strict JSON Schema, finalization prompt, repair prompt, and tests. In `json_object` mode the provider does not receive the strict schema, so an evidence object with an extra field such as `source` reaches the validator as `unknown_field`. The generic repair instruction says only to remove fields outside the schema, but that schema was never shown to the model, so the single repair attempt can repeat the same invalid shape.

## Design

Define one internal, typed field contract for the top-level result and each nested object (`summary`, `observation`, `inference`, and `evidence`). Each field records its JSON name, validation field, and type. The shape validator uses these contracts for required and allowed fields; strict JSON Schema is generated from them; finalization and unknown-field repair prompts render the same names and types.

Unknown-field errors retain only safe contract metadata: the parent contract and count of unknown fields. Logs expose the allow-listed field names and unknown count, never the model-controlled unknown key or value. Repair instructions identify the affected object and list every legal field with its type, explicitly forbidding all other fields.

The existing strictness remains unchanged: unknown fields are rejected, JSON Schema keeps `additionalProperties: false`, and semantic validation, size limits, EvidenceLedger checks, and unsupported-claim checks remain authoritative.

## Verification

- Unit tests reject unknown fields in every object kind and assert contract-derived diagnostics.
- Contract consistency tests compare validator field lists, rendered prompts, and generated JSON Schema.
- A Runner regression sends an evidence item with an extra `source`, verifies the targeted repair request, then succeeds with a corrected result.
- Existing `json_object` and strict `json_schema` finalization/repair request tests remain green.
