# Recover failed uploads and time-bound startup recovery

## Goal

Ensure every background upload reaches a terminal state, exposes an actionable failure reason, and never requires a restart to re-enable uploads. On restart, recover stale work and perform maintenance with bounded waits and visible diagnostics so optional cleanup cannot indefinitely block HTTP startup.

## Persistent failure state

The `bundles` table gains a nullable `failure_reason` text column through the existing idempotent schema preparation path. New uploads start with no reason. READY finalization clears the reason. Runtime failures and restart recovery set `status = 'FAILED'`, `process_stage = 'FAILED'`, and a user-facing reason in one statement.

Upload task and Issue bundle responses expose `failure_reason`. Internal logs retain the full error chain and identifiers, while the stored reason is normalized for users: preserve actionable bad-request messages, otherwise use a stable operation-level explanation rather than leaking database paths or internals.

## Runtime failure finalization

Background processing uses a dedicated failure-finalization function. It first retries the terminal database update a bounded number of times with short backoff. Only after attempting to persist FAILED does it remove partial database/file artifacts. Cleanup errors are logged with bundle ID/hash and never suppress the already-recorded failure state.

Cleanup is best effort and separated into independently logged database and filesystem operations where possible. The final temporary upload-directory removal is also logged on failure. Permit-acquisition failure follows the same terminal-state helper.

If all status-update retries fail, the task logs a high-severity structured event. A later restart still finds the bundle as PROCESSING and marks it FAILED through stale-task recovery.

## Frontend behavior

Both upload-task and bundle summary types include `failure_reason`. Polling already treats FAILED as terminal; on receipt it must also set the visible upload error, clear active-processing state naturally through the terminal task status, reload bundles/issues, and leave the upload selector enabled. Failed rows display their specific reason when present and remain deletable. A generic retry/delete instruction is used when older records lack a reason.

## Startup recovery

Tracing is initialized before recovery. Startup immediately logs the effective database URL plus resolved SQLite file path when applicable, data root, and log directory.

Schema preparation remains mandatory and may abort startup. After schema creation, these recovery stages run sequentially:

1. mark stale PROCESSING bundles FAILED with a restart reason;
2. remove stale `.tmp` entries;
3. clean expired bundles when retention is enabled.

Each stage logs start, completion, elapsed milliseconds, affected count, timeout, or error. Each has a 15-second `tokio::time::timeout`. Timeout or failure is non-fatal: startup proceeds to the next stage and eventually binds HTTP. Sequential execution prevents stale `.tmp` cleanup from racing newly accepted uploads.

Temporary cleanup treats a missing `.tmp` directory as success. Failure to inspect or remove one entry is reported with its path and does not prevent attempts on remaining entries; the stage returns a summary of removed and failed entries. A stage-level timeout still caps pathological filesystem calls.

## Testing

Backend tests cover schema migration/default null reasons, terminal update with reason, gzip-limit processing reaching FAILED, cleanup failure not preventing FAILED, stale PROCESSING restart recovery, and startup-stage timeout/non-fatal policy through extracted testable recovery helpers. Temporary-cleanup tests include one failing entry where platform behavior permits reliable simulation, with injected operations used for deterministic failure/timeout coverage.

Frontend tests cover terminal FAILED polling behavior, restored upload availability, and visible failure text while retaining existing upload-row behavior. The existing smoke upload test is also exercised repeatedly so timing-sensitive tasks cannot silently remain in `PROCESSING/INDEXING`. Final verification runs backend formatting, Clippy with warnings denied, backend tests, frontend tests, TypeScript lint, and production build.
