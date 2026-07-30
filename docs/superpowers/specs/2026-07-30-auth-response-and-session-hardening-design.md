# Authentication Response and Session Hardening Design

## Goal

Correct four review regressions without reopening internal error disclosure:

- preserve safe, actionable business validation messages;
- avoid attempting a SQLite write for every authenticated request;
- prevent caching of Session-dependent identity and personal-data responses;
- prevent one active user from continuously exhausting the shared Argon2 permits through failed password-change attempts.

## Error Boundary

`Database`, `Io`, and `Config` remain internal errors. Their complete details are logged on the server, while clients receive only stable generic codes and messages.

`BadRequest` also remains generic because existing call sites include parser, multipart, archive-library, and other strings that have not all been audited for public disclosure.

A new public business-error variant carries:

- an HTTP status;
- a stable static error code;
- an owned, controlled `String` message.

It is used only where the application intentionally constructs user-facing details. The first migrations restore:

- detailed-search expression syntax messages, including the parse position and reason;
- Issue quota messages containing the configured limit, current usage, and requested increase.

Existing static `AppError::Api` responses retain their current contract. Tests prove that internal values remain absent while the two migrated business errors retain actionable details.

## Session Activity Writes

The Session lookup query also selects `last_seen_at`. Rust parses the stored SQLite timestamp and compares it with the current UTC time.

The repository executes the existing best-effort `UPDATE` only when:

- the Session resolved successfully; and
- `last_seen_at` is absent, invalid, or at least five minutes old.

The SQL condition remains in the `UPDATE` as a concurrency guard. A repository test observes SQLite update-hook activity to prove a recently seen Session performs no update statement, while a stale Session performs one.

## Cache Policy

All `/api/auth/*` and `/api/me/*` responses receive:

```http
Cache-Control: no-store, private
```

The policy is applied at scoped middleware level so it covers successful responses, authentication errors, and future endpoints under either namespace. Other public API and embedded asset caching behavior is unchanged.

Integration tests cover `/api/auth/me`, `/api/auth/change-password` error responses, and `/api/me/saved-searches`.

## Password-Change Failure Limiting

Add a fourth independent in-memory rate-limit map keyed by authenticated user ID:

- window: 15 minutes;
- limit: 5 failed current-password verifications;
- independent capacity, so exhaustion cannot block login-IP, registration-IP, or failed-username policies.

Before acquiring an Argon2 permit, password change checks this bucket without incrementing it. A completed current-password mismatch records a failure. Invalid password length continues to return `CURRENT_PASSWORD_INVALID` without Argon2 work and also records a failure, preventing a cheap bypass of the endpoint policy.

Successful password verification clears the user’s failure bucket before hashing the new password. When limited, the endpoint returns the existing stable `429 TOO_MANY_REQUESTS` contract.

Tests verify the sixth failed attempt is rejected before Argon2 execution, successful verification clears prior failures, and the new bucket cannot consume the other policy maps’ capacity.

## Compatibility and Validation

No database migration or new environment variable is introduced. The limit is a fixed security policy consistent with the existing fixed bucket capacities.

Validation includes focused red/green tests, backend formatting, all backend tests, `cargo check`, Clippy with warnings denied, frontend tests/lint/build, and `git diff --check`.
