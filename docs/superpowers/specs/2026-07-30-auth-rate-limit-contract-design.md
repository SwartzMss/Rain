# Authentication Rate-Limit Contract Design

## Goal

Align authentication error propagation and rate limiting with Issue #25 without changing Session,
password, or authorization behavior.

## Argon2 error propagation

`run_argon2` remains the single boundary that maps capacity exhaustion to HTTP 429 and blocking or
hashing failures to HTTP 500. Login, registration, and password changes must propagate those
errors unchanged. A password endpoint returns a credential-specific 401 only when Argon2 completed
successfully and reported a mismatch.

## Independent rate-limit policies

Authentication uses three independent buckets:

- Login IP attempts: 20 requests in 60 seconds. Every login attempt is recorded before credential
  verification.
- Login username failures: 10 failed attempts in 300 seconds. The bucket is checked before the
  expensive verification and recorded only after a completed invalid-credential outcome.
- Registration IP attempts: 10 requests in 3600 seconds. Every registration attempt is recorded
  before validation and hashing.

Registration has no username bucket. Successful login does not change the username-failure bucket.
Argon2 capacity or internal failures do not count as password failures because credentials were not
actually evaluated.

All policy rejections use HTTP 429 and JSON code `TOO_MANY_REQUESTS`.

## Configuration

Expose these explicit settings:

```dotenv
RAIN_AUTH_LOGIN_IP_LIMIT_PER_MINUTE=20
RAIN_AUTH_LOGIN_USERNAME_FAILURE_LIMIT_PER_5_MINUTES=10
RAIN_AUTH_REGISTER_IP_LIMIT_PER_HOUR=10
```

Remove the older per-minute login and registration settings so names, values, and windows cannot
contradict one another.

## Storage and cleanup

The existing bounded in-memory map remains process-local. Bucket operations accept their own
window, prune expired timestamps before checking, preserve the global capacity bound, and evict the
oldest inactive bucket when necessary. Restarting Rain resets limits, consistent with the existing
in-memory design.

## Tests

- Saturating Argon2 during password change returns 429 `TOO_MANY_REQUESTS`, not a password error.
- A completed current-password mismatch returns `CURRENT_PASSWORD_INVALID`.
- Login IP attempts, username failures, and registration IP attempts enforce their independent
  limits and windows.
- Successful login does not add a username failure.
- Defaults, environment examples, README, and API error codes match the contract.

