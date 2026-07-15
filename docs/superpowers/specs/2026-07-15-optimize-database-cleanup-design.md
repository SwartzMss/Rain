# Bound database growth and failed-upload cleanup

## Goal

Prevent a large or failed upload from creating multi-gigabyte SQLite/WAL growth and long monolithic cleanup transactions, while preserving filename, full-text, and structured log search behavior for ordinary uploads.

## Confirmed cause

The observed failed bundle created 3,198,484 `log_events` plus 16,018 `log_segments` and FTS rows. Segment text is stored in `log_segments` and again in FTS, while parsed lines additionally store `message` and `raw` in `log_events`. Removing all event rows in one transaction took 14 seconds and produced a 1.14 GiB WAL. SQLite retained the 2.39 GiB main-file high-water mark after deletion.

The archive size guard is not changed. It correctly measures cumulative extracted output, including nested archives and their extracted contents.

## Bounded structured-event indexing

Add `RAIN_INDEXING_MAX_EVENTS_PER_BUNDLE`, defaulting to 250,000. Full log text continues to be chunked into `log_segments` and indexed in FTS after the cap is reached, so ordinary content search remains complete. Only additional structured `log_events` rows are skipped. Emit one structured warning per bundle when the cap is first reached, including the configured limit.

The counter is bundle-scoped and shared across every uploaded file and nested archive in that bundle. Configuration validation requires a positive value and `.env.example` documents the storage/search trade-off.

## Batched cleanup

Replace the single cleanup transaction with bounded batches. Delete dependent `log_events` and `log_line_offsets` in chunks of 10,000 rows, committing each chunk. Delete FTS and segment rows in smaller batches keyed by the bundle's segment IDs, then delete files. Preserve the bundle row and its already-persisted FAILED reason.

Each cleanup phase logs affected rows, batch count, and elapsed time. An error stops that database cleanup phase and is reported, but does not revert the bundle's terminal FAILED state or prevent filesystem cleanup.

The same batched helper is reused for explicit bundle deletion, issue deletion, retention cleanup, and failed-upload cleanup where practical so large successful bundles do not recreate the same monolithic WAL problem.

## WAL maintenance and disk diagnostics

After a large cleanup, request a best-effort `PRAGMA wal_checkpoint(TRUNCATE)` and log its busy/log/checkpointed page counts plus elapsed time. Checkpoint failure is non-fatal. Do not run automatic `VACUUM`: it requires an exclusive rewrite, temporary disk space close to the database size, and can block the running service.

Startup path diagnostics additionally report current sizes for `rain.db`, `rain.db-wal`, and `rain.db-shm` when they exist. This makes retained main-file space and active WAL growth distinguishable from logs alone.

## Testing

Tests verify the event cap across multiple chunks/files, continued segment indexing after the cap, configuration parsing/validation, multi-batch deletion, retained FAILED bundle state, checkpoint result handling, and missing database sidecar files. Final verification runs formatting, Clippy with denied warnings, and the complete backend test suite.
