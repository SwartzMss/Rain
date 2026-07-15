# Configurable application limits

## Goal

Move Rain's upload, archive, indexing, preview, and result-page limits from module constants into typed application configuration while preserving the current defaults. Configuration remains operationally simple: the release `.env` file is the only configuration file, and process environment variables override values loaded from that file.

## Configuration discovery and precedence

Rain keeps its existing `.env` discovery behavior: load `.env` beside the executable when present, otherwise try `.env` in the working directory. `dotenvy` does not replace variables already present in the process, so deployment environment variables take precedence over file values. Missing limit variables use compiled safe defaults.

Limit variables use explicit `RAIN_` names grouped by purpose:

- Upload: `RAIN_UPLOAD_MAX_FILES`, `RAIN_UPLOAD_MAX_FILE_SIZE`, `RAIN_UPLOAD_MAX_TOTAL_SIZE`, `RAIN_UPLOAD_MAX_TEXT_FIELD_SIZE`, `RAIN_UPLOAD_CONCURRENT_PROCESSING_TASKS`.
- Archive: `RAIN_ARCHIVE_MAX_EXTRACTED_SIZE`, `RAIN_ARCHIVE_MAX_ENTRY_SIZE`, `RAIN_ARCHIVE_MAX_ENTRIES`, `RAIN_ARCHIVE_MAX_PATH_DEPTH`, `RAIN_ARCHIVE_MAX_RECURSION_DEPTH`, `RAIN_ARCHIVE_MAX_OUTPUT_PATH_CHARS`, `RAIN_ARCHIVE_MAX_COMPRESSION_RATIO`.
- Indexing: `RAIN_INDEXING_MAX_LINE_SIZE`, `RAIN_INDEXING_CHUNK_LINES`, `RAIN_INDEXING_COMMIT_LINES`, `RAIN_INDEXING_LINE_OFFSET_INTERVAL`.
- API: `RAIN_API_FILE_PREVIEW_SIZE`, `RAIN_API_DEFAULT_LINE_PAGE_SIZE`, `RAIN_API_MAX_LINE_PAGE_SIZE`, `RAIN_API_DEFAULT_SEARCH_RESULTS`, `RAIN_API_MAX_SEARCH_RESULTS`.

Byte-size variables accept case-insensitive binary units such as `64 KiB`, `4 GiB`, and `20 GiB`. Plain integers represent bytes. Internal byte sizes use `u64`; conversion to platform-sized APIs is checked at the boundary.

## Typed state and data flow

`AppConfig` gains nested `UploadConfig`, `ArchiveConfig`, `IndexingConfig`, and `ApiConfig` values. Startup loads scalar values, parses byte sizes, validates the full configuration, initializes the processing semaphore from the configured concurrency, and stores the limit groups in shared `AppState` alongside the database pool and data root.

Routes read upload and API limits from `AppState`. Background ingestion receives the archive and indexing configuration explicitly, avoiding global mutable configuration and allowing tests to inject small limits. Values used inside spawned work are cloned immutable configuration.

## Validation and errors

Startup rejects zero for every count, size, depth, ratio, interval, and page limit that must be positive. It also rejects:

- archive entry size greater than archive extracted size;
- upload file size greater than upload total size;
- default line page size greater than its maximum;
- default search result count greater than its maximum;
- values that cannot be represented by a downstream API after checked conversion.

Errors name the exact environment variable and explain the invalid value or relationship. After logging is initialized, Rain logs the effective limit groups once at startup.

Archive copy failures distinguish the per-entry ceiling from the shared extracted-byte budget. Entry overflow reports the configured maximum entry size; exhausting the bundle budget reports the configured maximum extracted size. Human-readable formatting preserves useful sub-MiB values rather than displaying `0 MB`.

## Compatibility and documentation

When no new variables are set, behavior remains identical to the current constants. `backend/.env.example` lists every variable and default, with a short Chinese comment immediately above each setting explaining its purpose, units, and any important relationship or zero-value behavior. The README documents precedence, accepted size syntax, validation rules, and all defaults. Existing release packaging continues to ship only the executable and `.env`.

## Testing

Unit tests cover size parsing, defaults, environment overrides, invalid values, and cross-field validation. Ingestion tests inject deliberately small limits to exercise entry and bundle failures without large fixtures, including distinct gzip errors. Route and smoke tests construct shared state with explicit test configuration. Existing backend tests, formatting, checks, and the frontend build remain the final regression suite.
