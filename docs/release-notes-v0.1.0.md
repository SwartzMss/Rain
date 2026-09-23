# Rain v0.1.0

Rain v0.1.0 is the first release of the Tantivy-first architecture for large
log bundles.

## What changed

- Tantivy is enabled by default and is the default Bundle content-search
  backend.
- SQLite remains the local control-plane database for users, Issues, Bundle
  metadata, line locations, and lifecycle state.
- Upload indexing streams cleaned chunks into a bounded Tantivy writer instead
  of duplicating the full searchable body in SQLite FTS5.
- Tantivy writer admission and heap size are bounded by
  `RAIN_SEARCH_TANTIVY_MAX_WRITERS` and
  `RAIN_SEARCH_TANTIVY_WRITER_HEAP`.
- Release packages include the embedded frontend, an external `.env`, and a
  `VERSION` file.

## Fresh installation requirement

This release does not migrate pre-release SQLite search indexes. Start v0.1.0
with a new `DATABASE_URL` and `RAIN_DATA_ROOT`, then upload the logs again. If
the existing database contains SQLite-backed Bundle search data, the default
Tantivy startup check stops with an actionable fresh-data-directory error.

`RAIN_SEARCH_BACKEND=sqlite_fts` remains available for tests and old-data
diagnosis. It is not the v0.1.0 default deployment path.

The AI diagnosis and Skill features are not included in this release. Their
legacy database tables may remain in an upgraded database for compatibility,
but Rain no longer exposes their APIs, loads provider credentials, or sends
model requests.

## Validation

The release branch must pass the normal backend and frontend checks, plus a
clean-install smoke test that uploads a large log, searches it, restarts Rain,
and searches it again. Use the benchmark matrix in
[`performance/large-log-baseline.md`](performance/large-log-baseline.md) to
characterize 1/2/4 concurrent bundles and 1/5 GiB workloads on the target
machine; the existing single-host measurements are diagnostic rather than a
general performance guarantee.
