# Same-Origin HTTP Authentication Design

## Goal

Simplify Rain for its current internal-network deployment by supporting authentication over
plain HTTP from the frontend served by the Rain backend, without configurable secure cookies
or cross-origin browser access.

## Deployment boundary

Rain serves both the frontend and `/api` endpoints from the same host and port. Browser clients
must access the API through that same origin. Separately hosted frontends and cross-origin API
clients are outside the supported deployment model.

This is not an unrestricted CORS configuration. Removing the CORS middleware leaves the
browser's same-origin policy in effect and avoids reflecting or approving arbitrary origins.

## Configuration changes

- Remove `RAIN_SESSION_COOKIE_SECURE` and the corresponding `AuthConfig` field.
- Remove `RAIN_ALLOWED_ORIGINS`, `CorsConfig`, and its `AppConfig` field.
- Remove both variables from the example environment and README.
- Remove HTTPS and cross-origin deployment instructions that depend on these settings.

## Runtime changes

- Session and cleared-session cookies remain `HttpOnly`, `SameSite=Lax`, and scoped to `/`.
- Session cookies never include the `Secure` attribute, so authentication works over internal
  HTTP.
- Remove the Actix CORS middleware. Same-origin frontend requests continue to work without CORS
  response headers; cross-origin browser requests are unsupported.

## Testing

- Update cookie unit and integration tests to use the parameterless cookie builders and assert
  that authentication cookies do not include `Secure`.
- Remove configuration tests for the deleted environment variables and CORS validation.
- Run formatting, backend tests, Clippy, frontend tests, frontend type checking/build, and
  `git diff --check`.

## Future HTTPS support

Adding HTTPS later requires restoring secure-cookie support before exposing Rain beyond the
trusted internal network. Supporting a separately hosted frontend additionally requires a
deliberate credentialed-CORS allowlist rather than an allow-all policy.
