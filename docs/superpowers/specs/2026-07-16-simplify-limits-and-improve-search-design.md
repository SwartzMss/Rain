# Simplify Limits and Improve Search Design

## Goal

Replace the overlapping upload and archive size settings with one Issue-level content quota, remove unused structured-event indexing, and make Rain's UTF-8 log search behave more like literal substring search without replacing SQLite.

The initial scope deliberately excludes automatic detection or conversion of GBK, Big5, and other non-UTF-8 encodings.

## User-Facing Configuration

Rain exposes only three settings related to this workflow:

```env
# Maximum total size of final browsable files in one Issue.
RAIN_ISSUE_MAX_CONTENT_SIZE=4 GiB

# Maximum number of concurrent extraction and indexing jobs.
RAIN_UPLOAD_CONCURRENT_PROCESSING_TASKS=4

# Maximum bytes retained from one UTF-8 log line for viewing and indexing.
RAIN_INDEXING_MAX_LINE_SIZE=8 MiB
```

The default concurrent processing count changes from 2 to 4. The default maximum indexed line size changes from 1 MiB to 8 MiB. Lines beyond that size remain truncated and marked explicitly; the stored source file is not modified.

The following environment settings are removed:

- `RAIN_UPLOAD_MAX_FILES`
- `RAIN_UPLOAD_MAX_FILE_SIZE`
- `RAIN_UPLOAD_MAX_TOTAL_SIZE`
- `RAIN_UPLOAD_MAX_TEXT_FIELD_SIZE`
- `RAIN_ARCHIVE_MAX_EXTRACTED_SIZE`
- `RAIN_ARCHIVE_MAX_ENTRY_SIZE`
- `RAIN_ARCHIVE_MAX_ENTRIES`
- `RAIN_ARCHIVE_MAX_PATH_DEPTH`
- `RAIN_ARCHIVE_MAX_RECURSION_DEPTH`
- `RAIN_ARCHIVE_MAX_OUTPUT_PATH_CHARS`
- `RAIN_ARCHIVE_MAX_COMPRESSION_RATIO`
- `RAIN_INDEXING_MAX_EVENTS_PER_BUNDLE`
- `RAIN_INDEXING_CHUNK_LINES`
- `RAIN_INDEXING_COMMIT_LINES`
- `RAIN_INDEXING_LINE_OFFSET_INTERVAL`

Operational and security values that remain necessary become documented internal constants. Removing their environment variables does not remove their protections.

## Issue Content Accounting

An Issue's quota usage is the sum of final browsable file bytes in its successful and currently processing Bundles.

- A directly uploaded non-archive file counts at its actual byte size.
- A directory consumes no quota.
- An uploaded archive does not itself consume quota.
- Files extracted from an archive consume quota at their uncompressed byte sizes.
- A nested archive is an intermediate container and does not consume quota; only its final non-archive descendants count.
- Failed Bundles consume no quota after rollback.
- Deleting a Bundle immediately releases its accounted quota.
- Deleting an Issue releases all of its quota.

The database stores each Bundle's accounted content size so Issue usage can be calculated without scanning its full file tree. Existing ready Bundles are backfilled from their file records using the same container-versus-final-file rules.

Capacity reservation is database-backed and atomic. Every final file reserves its bytes before it becomes committed content. The reservation transaction checks the sum of ready and processing Bundles for the Issue, including concurrent jobs. This prevents two jobs from independently observing the same remaining capacity and jointly exceeding the quota.

If a new Bundle crosses the quota, the entire Bundle fails. Rain removes its extracted files and partial indexes, releases its reserved capacity, and retains only the existing failed-Bundle status record and an actionable failure reason. It never exposes a partially accepted Bundle.

The failure reason reports the Issue limit, current usage before the Bundle, and the minimum new content observed when the limit was crossed. It does not describe the failure as an archive configuration limit.

## Upload and Archive Safety

The Issue quota replaces separate per-file, per-request, per-entry, and per-Bundle extracted-byte configuration. Multipart input remains streamed to temporary storage; it is never buffered as one request in memory.

Rain retains fixed internal safeguards for:

- multipart text-field size;
- maximum files or archive entries processed per Bundle;
- archive path depth and recursive archive depth;
- output path length;
- compression ratio;
- path traversal, unsafe paths, collisions, and unsupported archive behavior.

ZIP, tar.gz, gzip, directly uploaded files, and recursively nested archives use one Bundle-scoped accounting object connected to the Issue quota. Known archive entry sizes are reserved before extraction. Streaming gzip output reserves safely as bytes are produced. A failure at any depth aborts and rolls back the whole Bundle.

Internal request guards may reject pathological transport input before processing, but no second user-configurable content ceiling may conflict with `RAIN_ISSUE_MAX_CONTENT_SIZE`.

## Remove Structured Events

The current `log_events` pipeline is unused by routes and frontend features. Rain removes:

- `EventBudget` and its 250,000-event cap;
- basic event parsing during ingestion;
- writes to `log_events`;
- the `log_events` table and its indexes;
- event cleanup phases and related tests;
- the structured-event-cap warning.

This removal does not affect file browsing, line paging, `log_segments`, or FTS search. Existing `log_events` data is dropped during database initialization or migration so it stops consuming SQLite and WAL space.

If structured level, timestamp, component, or AI analysis is implemented later, it will receive a feature-specific data model and completeness policy rather than silently storing only the first arbitrary number of events.

## Search Semantics and FTS Migration

Rain continues to use SQLite. The content FTS table changes from the default token-oriented tokenizer to FTS5 trigram indexing so searches of at least three characters can match literal substrings inside identifiers and uninterrupted text.

This specifically improves partial searches for UUIDs, request IDs, URLs, IP-like values, error codes, class names, stack traces, and contiguous Chinese text. Query construction no longer rewrites whitespace-separated input into quoted tokens joined with `AND`; it generates a safe query consistent with literal substring intent.

Queries shorter than three characters use an explicit bounded fallback instead of pretending trigram FTS can answer them. The fallback searches only ready Bundles within the selected Bundle or Issue scope and preserves the existing pagination limits. Its implementation must avoid an unbounded response even though such queries may require scanning indexed segment text.

Database initialization detects the previous FTS schema, recreates `log_segments_fts` with the trigram tokenizer, and repopulates it from `log_segments`. Rebuilding is idempotent and leaves the relational segment source intact if interrupted. Startup logs make a rebuild visible because large existing databases may require meaningful time and temporary disk space.

Only UTF-8-compatible text recognized by the current classifier is in scope. Encoding detection and transcoding are explicitly deferred.

## API and UI Scope

Existing upload and search endpoints remain stable. Search results retain their current response shape. This change does not add a capacity progress indicator to the frontend; it only returns a clear upload failure when the Issue quota is exceeded. A usage display can be added separately after the accounting model is established.

## Error Handling and Recovery

- Quota overflow is a user-facing bad request associated with the failed Bundle.
- Unsafe archives continue to produce specific safety errors.
- Partial file, FTS, line-offset, and quota state is removed through the existing batched cleanup path.
- A failed cleanup is logged with Bundle and Issue identifiers and must not falsely mark the Bundle ready.
- FTS migration failure prevents normal startup instead of serving silently incomplete search results.
- Existing source files and `log_segments` remain the authoritative inputs for an FTS rebuild.

## Testing

Configuration tests cover the three retained settings, their new defaults, environment overrides, positive validation, and absence of removed environment-driven fields.

Quota tests cover direct files, archives, nested archives, mixed uploads, exact-limit acceptance, one-byte overflow, deletion release, failed-upload rollback, existing-data backfill, and two concurrent Bundles contending for the same final bytes.

Archive tests retain coverage for fixed entry-count, recursion, path, compression-ratio, and traversal protections across ZIP, tar.gz, and gzip.

Ingestion and cleanup tests confirm no `log_events` rows, schema, indexes, warnings, or cleanup phases remain while segments, FTS, offsets, and line paging still work.

Search tests cover literal substrings within punctuation-heavy identifiers, UUIDs, Chinese text, multiple words, quotes and FTS metacharacters, fewer-than-three-character fallback, Bundle scope, Issue scope, ready-status filtering, pagination, and rebuilding an old FTS schema.

Final verification runs formatting, Clippy with warnings denied, the full Rust test suite, frontend checks affected by API fixtures, and whitespace/diff inspection.

## Implementation Workstreams

1. Simplify configuration and establish internal safety/performance constants.
2. Add Bundle content accounting and existing-data backfill.
3. Implement atomic Issue quota reservation, rollback, and release.
4. Adapt multipart and recursive archive processing to the unified quota and fixed safeguards.
5. Remove structured-event parsing, storage, schema, and cleanup.
6. Migrate FTS to trigram substring search with a short-query fallback.
7. Update tests and documentation, then run full verification.
