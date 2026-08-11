# Issue #95 Upload and Bundle Polling Design

## Goal

Allow a user to start another upload as soon as the current HTTP upload is accepted, while the Bundle list independently refreshes every three seconds until every `PENDING` or `PROCESSING` Bundle reaches `READY` or `FAILED`.

## Root cause

`useUploadTask` currently owns both the browser upload and one backend task. Its `activeTask` disables the upload panel and its task-specific polling competes with a separate one-shot Bundle refresh. After a reload there is no local task, and the Boolean Bundle effect does not retrigger when a refresh returns another `PROCESSING` Bundle.

## Considered approaches

1. **Move server-state polling into `useIssueBundles` (chosen).** This gives all Bundles one polling owner and lets `useUploadTask` represent only the browser request. It matches the existing hook boundaries and prevents per-task timers.
2. Add a third `useBundlePolling` hook. This isolates timer code but splits Bundle state and its refresh lifecycle across two hooks without a current reuse case.
3. Keep polling in `useUploadTask` and make its Bundle effect recursive. This is the smallest textual patch, but retains the coupling that caused the upload lock and keeps refreshed pages dependent on an upload-oriented hook.

## Components and data flow

### `useUploadTask`

The hook retains file selection, HTTP progress, the in-flight guard, and request errors. It releases the guard and upload-disabled state immediately after `uploadLogs` resolves. Bundle and issue refreshes happen after acceptance, but their duration does not keep the upload control disabled. The hook no longer polls a task or exposes an `activeTask` as the Issue-wide processing state.

### `useIssueBundles`

The hook remains the source of truth for every Bundle. It considers both `PENDING` and `PROCESSING` active. A single recursive timer refreshes the selected Issue after three seconds and schedules another refresh while active Bundles remain. The effect cleans up its timer on terminal state, Issue change, or unmount. Ordinary refresh failures preserve the last Bundle snapshot so a transient error cannot terminate polling; not-found handling keeps its existing behavior.

When the document is hidden, the polling loop delays network work. A visibility change back to visible triggers an immediate refresh without creating a second timer.

### Presentation

`HomeView`, `UploadPanel`, and `buildFileRows` stop treating the latest upload task as the whole Issue's background state. The Bundle response supplies backend status rows; only the current browser upload or a failed upload uses optimistic rows.

## Error handling

- HTTP upload errors stay visible in the upload panel and do not affect Bundle polling.
- Bundle refresh errors keep the last successful Bundle list and retry on the next polling interval.
- `RESOURCE_NOT_FOUND` still clears state and invokes the missing-Issue callback.
- Request IDs and selected-Issue checks continue to reject stale responses.

## Tests

Hook-level behavior tests use controlled API responses and fake timers to verify:

- a completed HTTP upload no longer disables a second upload;
- `PENDING` and `PROCESSING` both start polling;
- repeated active responses keep polling until a terminal response;
- one completed Bundle does not stop polling for another active Bundle;
- transient polling failures retry;
- switching Issues cancels the old polling chain;
- terminal Bundles stop further polling.

The full frontend test, type-check, and production build commands provide regression coverage.
