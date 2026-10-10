# Open search result retention

## Accepted behavior

An open result must remain usable while the browser is running. Do not close tabs automatically or silently replace their contents. Keep the existing 30-minute retention as an inactivity grace period instead of a hard lifetime. Full results retain at least their existing seven-day expiry.

The user approved planning, implementation and PR creation after comparing permanent retention, automatic tab closure and renewal with delayed reclamation.

## Protocol

Add public `POST /api/temp-results/keep-alive`, accepting `{ result_ids: string[] }` (maximum 100 IDs per request), returning `{ unavailable_ids: string[] }`. Match the existing anonymous preview/read access. Validate 32-character hexadecimal IDs. Deduplicate IDs and atomically extend only ACTIVE, unexpired records to at least server-now + 30 minutes. Never revive expired/STAGING/DELETING records or shorten longer retention. Use the shared SQLite writer admission and a bounded batch update. A renewal and a cleanup claim cannot both succeed for the same expired version.

The browser deduplicates all open result IDs and renews immediately, every five minutes, and on focus/visibility/online recovery. Chunk requests at 100 IDs; serialize cycles, bound request duration and ignore responses after unmount. Closing a tab or leaving the page stops renewing that reference; remove automatic DELETE requests. Other pages keep renewing independently. After the final page disappears the normal expiry worker reclaims the result. No new lease table or browser-close delivery guarantee is needed.

## Expired result recovery

Unavailable results keep their tabs and loaded content. Show a clear expired/unavailable notice; block paging and filtering on that stale snapshot. For search tabs replay the saved query plan on explicit “重新搜索”, opening a fresh result without silently changing the old snapshot. Explain that current source data may differ. Preserve conditions when recovery fails and show an actionable error. Standalone temporary-result pages also renew; when no original query plan exists, explain that the user must search again from the source.

Network failures do not prove expiry; retain the snapshot and retry renewal. Browser suspension beyond the grace period can still cause expiry. Explicit authorized deletion remains available.

## Verification

Backend: renewal across original expiry; no resurrection; longer retention unchanged; cleanup claim ordering; invalid/oversized requests; guest access. Frontend: all open tabs, duplicate IDs, closure, multiple viewers, timer/focus/online, failed or stale responses, batches, expiry notice and explicit replay. Run frontend tests/build/lint and backend fmt/check/clippy/tests as resources permit.
