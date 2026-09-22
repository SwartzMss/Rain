# Tantivy bundle-index prototype

The `tantivy-search` feature adds an opt-in per-Bundle search backend. The
default build continues to use SQLite FTS.

The prototype stores one cleaned log chunk per document and keeps the file,
chunk, line, path, and event-time metadata needed for later filtering. A
bounded channel (two batches by default) feeds one blocking Tantivy writer;
the writer heap is capped at 64 MiB by default. The reader side can therefore
stop producing when the writer is saturated instead of retaining an entire
large file in memory.

Content search uses a bounded set of lower-cased n-gram candidates. Every
candidate is rechecked against the stored cleaned chunk with a contiguous
case-insensitive substring test. This is required because n-grams are only a
candidate generator: a query such as `abcd` must not match `abc ... bcd`.

The feature is built and tested with Tantivy 0.26.2:

```bash
cargo test --manifest-path backend/Cargo.toml --locked \
  --features tantivy-search search::tantivy
cargo test --manifest-path backend/Cargo.toml --locked \
  --features tantivy-search --test search_backend_parity
cargo clippy --manifest-path backend/Cargo.toml --locked \
  --features tantivy-search --lib -- -D warnings
```

Use `RAIN_SEARCH_BACKEND=tantivy` with a `--features tantivy-search` build to
select the backend for new uploads. The upload worker claims a generation,
builds it from normalized SQLite chunks through the bounded pipeline, reopens
and verifies the artifact, and only then allows the Bundle to become READY.
The Bundle content search route reads that published generation. Issue-wide
search and existing Bundles remain on SQLite until mixed-backend publication is
implemented.

Migration `0004` makes the FTS5 shadow triggers conditional on the Bundle
backend. Tantivy-owned uploads therefore keep the normalized `log_segments`
rows but avoid duplicating every chunk into SQLite FTS during ingest; legacy and
SQLite-owned Bundles retain the original trigger path.

Tantivy writers also pass through a process-wide admission semaphore. The
defaults allow one writer with a 64 MiB heap; `RAIN_SEARCH_TANTIVY_MAX_WRITERS`
and `RAIN_SEARCH_TANTIVY_WRITER_HEAP` set the aggregate writer budget for
larger machines.

The current implementation still needs crash recovery and large-file
comparison work before we can claim an end-to-end performance improvement.
