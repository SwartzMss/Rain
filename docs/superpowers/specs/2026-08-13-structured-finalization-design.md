# Structured Skill Result Finalization

## Problem

The Skill Runner currently treats a tool-enabled model response with no tool calls as a final structured result. That response was requested with no response format, so in the default `json_object` mode the provider only guarantees JSON object syntax, not the nested `SkillRunResult` contract. A malformed `missing_context` value can therefore survive until server validation and fail both the initial response and the repair attempt.

## Design

Tool-enabled iterations remain retrieval/reasoning steps. When the model returns no tool calls, Rain appends that response to the conversation and sends a new dedicated finalization request with no tools, no tool choice, and the configured structured-output response format. Tool-limit and tool-error-limit exits use the same finalization prompt and request path. The existing Rust result validator and EvidenceLedger checks remain the final authority.

The finalization prompt explicitly declares all top-level and nested types. In particular, `missing_context` is a JSON `array<string>`; `[]` is used when there is no missing context, and string/object/null forms are forbidden. A minimal JSON skeleton communicates shape without inventing evidence or conclusions.

Validation errors retain their existing public error mapping. For statically known fields, internal diagnostics carry an allow-listed expected type and a fixed actual JSON type classification (`missing`, `null`, `boolean`, `number`, `string`, `array`, or `object`). Logs and repair prompts may include those safe labels, but never field values, full model responses, Skill text, or log contents.

## Verification

- A regression test proves a no-tool reasoning response is followed by a dedicated structured finalization request.
- Unit tests cover `missing_context` string, null, and object values and assert `invalid_field_type` with `array<string>` expectations.
- Integration tests verify safe expected/actual type logging and preserve EvidenceLedger, unsupported-claim, retry, cancellation, and provider error behavior.
- Full backend and frontend checks run before publication.
