# User Authentication and Session Design

## Scope

This change implements PR 1 from GitHub issue #25:

- username and password registration;
- username and password login;
- current-session lookup;
- logout;
- database-backed sessions;
- reusable optional and required user extractors;
- frontend authentication state, login page, and registration page.

Existing databases do not need an in-place migration. The operator will reset the
database, so the existing schema bootstrap may create the new tables directly.

This change does not yet protect existing write APIs. Guest read-only enforcement,
saved searches, password changes, rate limiting, periodic session cleanup, and CORS
hardening belong to later PRs.

## Backend Architecture

Authentication code lives under `backend/src/auth/`:

- `password.rs` validates and hashes passwords with Argon2id and verifies stored
  password hashes;
- `session.rs` generates opaque session tokens, hashes them with SHA-256, and
  defines cookie behavior;
- `extractor.rs` resolves request cookies into `OptionalUser` and `RequireUser`;
- `mod.rs` exposes the authentication types used by routes.

Persistence is split into focused repositories:

- `repositories/users.rs` creates users and finds them by normalized username or ID;
- `repositories/sessions.rs` creates, resolves, and revokes sessions.

`routes/auth.rs` owns the public HTTP contract. Business rules remain in the auth
and repository modules so they can be tested without duplicating HTTP concerns.

## Data Model

Database reset creates these tables:

```sql
CREATE TABLE users (
    id TEXT PRIMARY KEY,
    username TEXT NOT NULL,
    username_normalized TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'ACTIVE',
    role TEXT NOT NULL DEFAULT 'USER',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_login_at TEXT,
    password_changed_at TEXT
);

CREATE TABLE user_sessions (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at TEXT NOT NULL,
    revoked_at TEXT,
    user_agent TEXT,
    client_ip TEXT
);

CREATE INDEX idx_user_sessions_user ON user_sessions(user_id);
CREATE INDEX idx_user_sessions_expiry ON user_sessions(expires_at);
```

Usernames retain the submitted spelling for display and use a lowercase normalized
value for uniqueness and login. Valid usernames match
`^[A-Za-z0-9._-]{3,32}$`. Passwords contain 8 to 128 Unicode scalar values.

Only Argon2id password hashes are stored. Session tokens contain 32 cryptographically
secure random bytes. The raw URL-safe token exists only in the cookie and response
handling path; SQLite stores its SHA-256 hash.

## HTTP Contract

### Registration

`POST /api/auth/register` accepts:

```json
{"username":"swartz","password":"password123"}
```

It returns `201 Created` with the public user representation. Registration does
not create a session. Validation failures use `400`; a normalized-name conflict
uses `409`.

### Login

`POST /api/auth/login` accepts the same shape and returns the public user
representation. Invalid usernames and invalid passwords both return:

```json
{"code":"INVALID_CREDENTIALS","message":"用户名或密码错误"}
```

Successful login updates `last_login_at`, creates a seven-day session, and sets:

```text
rain_session=<opaque-token>; HttpOnly; SameSite=Lax; Path=/; Max-Age=604800
```

`Secure` is controlled by `RAIN_SESSION_COOKIE_SECURE`, defaulting to `false`.
The TTL is controlled by `RAIN_SESSION_TTL_SECONDS`, defaulting to `604800`.

### Current Identity

`GET /api/auth/me` always returns `200`.

Guest response:

```json
{"authenticated":false,"user":null}
```

Authenticated response:

```json
{"authenticated":true,"user":{"id":"...","username":"swartz"}}
```

Missing, malformed, expired, revoked, or unknown session tokens resolve as guest.

### Logout

`POST /api/auth/logout` revokes the matching database session when present, clears
the cookie with the same path and security attributes, and returns `204`. It is
idempotent for guests and stale cookies.

## Identity Extractors

`OptionalUser` resolves a valid session to an active user and otherwise contains no
user. `RequireUser` reuses the same resolution:

- missing, expired, revoked, or invalid session: `401 AUTHENTICATION_REQUIRED`;
- authenticated user whose status is not `ACTIVE`: `403`;
- active user: route execution continues.

PR 1 tests and exports `RequireUser`, but existing business write routes do not use
it until PR 2.

## Frontend

An `AuthProvider` initializes identity through `GET /api/auth/me` and exposes:

```ts
type AuthState =
  | { status: 'LOADING' }
  | { status: 'GUEST' }
  | { status: 'AUTHENTICATED'; user: User };
```

The API client sends `credentials: 'include'` for both `fetch` and upload XHR
requests. It understands structured API errors while preserving the current
fallback behavior for legacy `{error}` responses.

Routes `/login` and `/register` use the existing visual language. Registration
redirects to login without authenticating. Login refreshes auth state and returns
to the safe same-origin route supplied in navigation state, falling back to `/`.

The header renders:

- a neutral loading state during initialization;
- `只读模式`, `登录`, and `注册` for guests;
- the display username and a logout action for authenticated users.

PR 1 does not hide or disable existing create, upload, or delete controls because
the corresponding backend authorization boundary is introduced in PR 2.

## Error Handling and Security

API errors use `{code, message}`. Passwords, raw session tokens, and Cookie headers
must not be logged. Login errors never reveal whether a normalized username exists.
Inactive users cannot log in and cannot be resolved as authenticated.

Cookie security defaults support local HTTP development. Production deployments
must set `RAIN_SESSION_COOKIE_SECURE=true` when served over HTTPS.

## Testing

Backend tests cover:

- username validation and case-insensitive normalization;
- password length validation, Argon2id hashing, and verification;
- duplicate normalized usernames;
- successful and failed login behavior;
- session token hashing and the absence of raw tokens in SQLite;
- valid, expired, revoked, malformed, and inactive-user sessions;
- guest and authenticated `/api/auth/me`;
- idempotent logout and cookie clearing;
- `OptionalUser` and `RequireUser` outcomes.

Frontend tests cover auth state transitions, structured error parsing, and safe
login return-path handling. Type checking and the production build verify page and
provider integration.

## Acceptance Criteria

- Registration validates usernames and passwords and does not log the user in.
- Login creates a seven-day opaque Session in an HttpOnly cookie.
- Passwords use Argon2id; raw passwords and raw session tokens are absent from the
  database.
- `/api/auth/me` distinguishes guests and authenticated active users.
- Logout revokes the current session and clears the browser cookie.
- Auth extractors are ready for PR 2 without changing existing business write APIs.
- The frontend provides loading, guest, login, registration, authenticated, and
  logout flows.
- Authentication configuration is documented in both `.env.example` files and the
  README.
