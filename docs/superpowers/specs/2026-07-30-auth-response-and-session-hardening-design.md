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

## Password-Change Attempt Limiting

Add a fourth independent in-memory rate-limit map keyed by authenticated user ID:

- window: 15 minutes;
- limit: 5 password-change attempts that enter the expensive phase;
- independent capacity, so exhaustion cannot block login-IP, registration-IP, or failed-username policies.

Each user also has a process-local in-flight guard. A password-change request must acquire it before reading the password hash or acquiring an Argon2 permit. A second concurrent request for the same user returns `429 TOO_MANY_REQUESTS` without entering password verification or replacement-password hashing.

After acquiring the guard, a request checks and records the attempt bucket before current-password validation or Argon2 work. Successful verification does not clear the bucket, so correct-password requests cannot bypass the frequency policy. Invalid current-password format remains cheap and returns `CURRENT_PASSWORD_INVALID`, but still counts as a password-change attempt.

Tests verify the sixth attempt is rejected before Argon2 execution, concurrent correct-password requests allow only one request into the expensive phase, the in-flight guard releases on every return path, and the new bucket cannot consume the other policy maps’ capacity.

## Active Session Cap

Each user may have at most 20 active, unexpired Sessions.

Login Session creation becomes one transaction that:

1. verifies the user is still active and the password hash is unchanged;
2. deletes expired and revoked Sessions for that user;
3. inserts the new Session;
4. deletes the oldest active Sessions beyond the newest 20.
5. updates `last_login_at` and `updated_at`.

The cap operation uses deterministic ordering by `created_at DESC, rowid DESC`, preserving the most recently inserted Session when SQLite timestamps share one-second precision. The insert and pruning occur in the same SQLite write transaction, so concurrent successful logins cannot leave the user above the cap after their transactions commit.

The transaction commits only after login metadata is updated. Any database failure rolls back the new Session, stale-row cleanup, Session pruning, and metadata together, so a client-facing login failure cannot leave an unknown Session token or remove an existing Session.

Password replacement remains unchanged because it revokes every existing Session before inserting exactly one replacement. Repository tests cover sequential and concurrent logins, preservation of the newest Sessions, and isolation between users.

## Compatibility and Validation

No database migration or new environment variable is introduced. The limit is a fixed security policy consistent with the existing fixed bucket capacities.

Validation includes focused red/green tests, backend formatting, all backend tests, `cargo check`, Clippy with warnings denied, frontend tests/lint/build, and `git diff --check`.
