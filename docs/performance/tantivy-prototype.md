# Tantivy bundle-index v0.1

The default v0.1.x build includes and selects Tantivy for Bundle search. SQLite
remains the control-plane database. This release is intentionally a fresh
installation: it does not migrate pre-release SQLite search data, and startup
rejects a data directory containing SQLite-backed Bundles.

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

The backend is built and tested with Tantivy 0.26.2:

```bash
cargo test --manifest-path backend/Cargo.toml --locked \
  search::tantivy
cargo test --manifest-path backend/Cargo.toml --locked \
  --test search_backend_parity
cargo clippy --manifest-path backend/Cargo.toml --locked \
  --lib -- -D warnings
```

Use the default `RAIN_SEARCH_BACKEND=tantivy` build for uploads. The upload worker claims a generation,
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

Tantivy writers pass through a process-wide `SearchResourceBudget`. The
defaults allow one writer with a 64 MiB heap; `RAIN_SEARCH_TANTIVY_MAX_WRITERS`
and `RAIN_SEARCH_TANTIVY_WRITER_HEAP` set the aggregate writer budget for
larger machines. The budget is held from writer creation through publication
verification, and releases on success, failure, or cancellation. Build metrics
report admission wait, active/queued writers, heap reservation, and total build
time. Startup and the periodic cleanup task remove unpublished generations and
artifacts for deleted Bundles.

Skill search remains on the SQLite adapter in this release, and query
fan-out/reader caching are deliberately conservative. Old data directories are
not rebuilt automatically; see the large-log baseline for the first one-host
comparison rather than treating it as a production SLO.
