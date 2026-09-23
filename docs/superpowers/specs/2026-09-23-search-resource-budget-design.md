# Search Resource Budget Design

## Goal

Make Tantivy Bundle indexing concurrency explicit and observable so multiple large uploads cannot exhaust process memory or monopolize disk and CPU, while preserving the current default backend and search behavior.

## Scope

This change applies only to the opt-in `tantivy-search` indexing path. SQLite FTS remains the default backend. It does not migrate legacy Bundles, change skill search, remove FTS5, or add a hard operating-system CPU quota.

## Design

`SearchRuntime` becomes the owner of a `SearchResourceBudget`. The budget contains:

- a semaphore limiting the number of active Tantivy Bundle writers;
- the configured heap allowance passed to each Tantivy writer;
- counters for waiting and active writers used by indexing metrics.

The effective Tantivy writer heap reservation is `active_writer_count * writer_heap_size_bytes`. A Bundle acquires one permit before creating its staging pipeline and holds it through commit, verification, publication, or abort. Cancellation and every error path release the permit through the existing owned permit guard.

The budget does not throttle individual CPU instructions. It controls the number of concurrent CPU and disk-heavy writer jobs; this keeps the aggregate heap and I/O pressure bounded while allowing operators to raise concurrency on machines with spare capacity.

The upload job receives the budget object rather than separate semaphore and heap fields. The Bundle build session records admission wait duration and build duration in existing tracing metrics. No new database state is required.

## Configuration

Keep the existing environment variables and defaults for compatibility:

```text
RAIN_SEARCH_TANTIVY_MAX_WRITERS=1
RAIN_SEARCH_TANTIVY_WRITER_HEAP=64MiB
```

Validation continues to reject zero values. Renaming internal fields is allowed, but the public configuration names and default behavior remain stable.

## Tests and benchmark

- Unit-test budget construction and permit release after normal finish, error, and cancellation.
- Test that a second Bundle waits while the configured writer capacity is occupied.
- Keep the existing pipeline and publication tests unchanged except where they need the new budget type.
- Extend the large-log benchmark documentation to report admission wait and active writer settings for 1, 2, and 4 concurrent Bundles.
- Run default and `tantivy-search` format, check, clippy, unit, and smoke tests.

## Success criteria

- No Tantivy writer can start without a budget permit.
- A failed or cancelled build never permanently consumes a permit.
- Existing SQLite and Tantivy search results remain unchanged.
- With one permit, concurrent Bundle builds are serialized; with two permits, two builds can proceed concurrently.
- Metrics expose enough timing to distinguish parser time, admission wait, and writer build time.
