# Tantivy bundle-index prototype

The `tantivy-search` feature adds an opt-in prototype for the next indexing
stage. It is not selected by uploads or production routes yet.

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

Publication, crash recovery, deletion visibility, exact HTTP totals, and
production ingest routing remain later steps. No existing Bundle is switched
to this index by this change.

