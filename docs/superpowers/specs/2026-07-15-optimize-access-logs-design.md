# Optimize access logs

## Goal

Stop routine successful polling requests from flooding the default `INFO` logs while retaining requests that indicate errors or abnormal latency.

## Request classification

Replace Actix Web's default access `Logger` middleware with a small project-owned middleware. After the response completes, classify it using HTTP status and elapsed wall-clock time:

- status 500-599: log at `ERROR`;
- status 400-499: log at `WARN`;
- any response taking at least 1,000 milliseconds: log at `WARN`;
- all other responses: do not emit an access-log event.

Status takes precedence over latency, so a slow 5xx remains `ERROR`. Redirects and normal successful polling disappear from the default logs unless they are slow.

## Log contents

Each emitted event uses a dedicated `rain::http` tracing target and includes the request method, path including query string, response status, elapsed milliseconds, and best available peer IP. It deliberately omits Referer and User-Agent because those fields add noise and may contain unnecessary client details.

Existing application, startup recovery, upload failure, and Actix server lifecycle logs remain unchanged. Operators can enable additional framework diagnostics with `RUST_LOG`, but routine access events are not generated merely by raising the framework log level.

## Testing

Keep classification separate from Actix request plumbing so unit tests can cover fast success, slow success, fast 4xx, slow 4xx, and 5xx precedence deterministically without sleeping. Integration/build verification runs Rust formatting, Clippy with warnings denied, and the complete backend test suite.
