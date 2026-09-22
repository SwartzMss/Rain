# Tantivy bundle-index prototype

The `tantivy-search` feature adds an opt-in per-Bundle search backend. The
default build continues to use SQLite FTS.

The upload worker streams each cleaned log chunk directly into a per-Bundle
build session. A bounded channel (two batches by default) feeds one blocking
Tantivy writer; the writer heap is capped at 64 MiB by default. The reader side
stops producing when the writer is saturated instead of retaining an entire
large file in memory. There is no post-ingest pass that reads the whole file
back from SQLite.

Tantivy owns searchable and stored chunk content. SQLite keeps row identity,
line ranges, sparse line offsets, event-time metadata, and line counts. For a
Tantivy-owned Bundle, `log_segments.content` is an empty compatibility value;
the conditional FTS triggers skip those rows, so the cleaned body is not
duplicated in SQLite.

Content search uses a bounded set of lower-cased fixed trigram candidates
(`rain_ngram_v2`). Every
candidate is rechecked against the stored cleaned chunk with a contiguous
case-insensitive substring test. This is required because n-grams are only a
candidate generator: a query such as `abcd` must not match `abc ... bcd`.
The HTTP Bundle and Issue routes require at least three characters, matching
the tokenizer bound.

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
streams normalized chunks through the bounded pipeline, persists sparse metadata
in batches, reopens and verifies the artifact, and only then allows the Bundle
to become READY. The Bundle content search route reads that published
generation. Issue-wide content search merges SQLite and Tantivy Bundles,
applies a stable `(offset, bundle_hash, file_id, chunk_index)` order, and
performs global pagination.

Migration `0004` makes the FTS5 shadow triggers conditional on the Bundle
backend. Tantivy-owned uploads therefore keep the normalized `log_segments`
rows but avoid duplicating every chunk into SQLite FTS during ingest; legacy and
SQLite-owned Bundles retain the original trigger path.

Tantivy writers also pass through a process-wide admission semaphore. The
defaults allow one writer with a 64 MiB heap; `RAIN_SEARCH_TANTIVY_MAX_WRITERS`
and `RAIN_SEARCH_TANTIVY_WRITER_HEAP` set the aggregate writer budget for
larger machines. Startup and the periodic cleanup task remove unpublished
generations and artifacts for deleted Bundles.

The current implementation is still opt-in. Skill search remains on the
SQLite adapter, and query fan-out/reader caching are deliberately conservative;
legacy Bundles are not rebuilt automatically. See the large-log baseline for
the first one-host comparison rather than treating it as a production SLO.
