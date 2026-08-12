# AI Provider Error Observability Design

## Goal

Improve server-side AI Provider failure logs so operators can identify the request stage and safe failure category without exposing credentials, prompts, Skill content, Issue log content, or upstream response bodies.

## Compatibility boundary

Skill Run's public and persisted error contract does not change. Transport and HTTP status failures continue to surface as `AI_PROVIDER_REQUEST_FAILED` with the existing user-facing message. The change is limited to server-side observability.

## Provider failure classification

`ProviderError` remains the boundary returned by `ChatCompletionClient`, but transport failures carry a small allow-listed `TransportReason` instead of raw `reqwest::Error` data. The safe reasons are `connect_failed`, `dns_failed`, `tls_failed`, `connection_reset`, and `request_failed`. Other categories are `timeout`, `http_status`, `invalid_response`, and `response_too_large`.

The HTTP client derives only those values from `reqwest::Error` predicates and its source chain. It never retains or logs the URL, headers, request body, response body, or raw error display text.

## Logging architecture

A shared Provider observability helper emits one structured warning for a failed model call. Its context contains only explicit non-sensitive fields:

- `stage`
- optional `run_id` and `iteration`
- `elapsed_ms`
- `tools_enabled`
- `tool_choice`
- `response_format`
- `error_category`
- optional safe `http_status` or `reason`

Skill Runner wraps each Provider call and supplies one of `model_request`, `final_model_request`, or `result_repair`. The administrator Provider Test and Skill Review use the same helper with `provider_test` and `skill_review` stages. The helper's API does not accept prompts, credentials, URLs, arbitrary response content, or raw error strings.

## Error flow

On failure, the call site first emits the structured warning and then applies its existing external error mapping. Skill Runner therefore keeps returning `AI_PROVIDER_REQUEST_FAILED` for both transport and HTTP status failures while the log distinguishes them. Timeout, invalid response, and oversized response mappings remain unchanged.

## Testing

Unit tests capture tracing output from the shared helper and verify:

- HTTP 400 retains `error_category=http_status` and `http_status=400`.
- A transport failure retains only an allow-listed reason.
- Skill Run request contexts expose model/final/repair stages and Tool Calling flags.
- A set of credential, Authorization, URL, prompt, Skill Markdown, Issue log, and response-body sentinel strings cannot appear in emitted log output or formatted Provider errors.
- Existing external Skill Run error codes remain unchanged.

Integration tests continue exercising the real Skill Runner request sequence with scripted clients; no Provider protocol, Tool Calling behavior, retry policy, or model capability requirement changes.
