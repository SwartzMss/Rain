# Large log baseline (PR0)

This opt-in tool measures the existing SQLite/CAS ingest pipeline and authenticated
search handlers. It does not change the search implementation or schema. No GiB
performance results are claimed here; collect release reports on the target host.

From `backend/`:

```sh
# Small functional check (debug builds are fine for this check only).
RAIN_BENCH_BYTES=16384 RAIN_BENCH_CONCURRENCY=2 RAIN_BENCH_QUERIES=2 \
  cargo test --test large_log_benchmark large_log_baseline -- --ignored --exact

# Baseline: 100 MiB per bundle, one bundle, 20 samples per query/scope.
RAIN_BENCH_REPORT=/tmp/rain-baseline.jsonl \
  cargo test --release --test large_log_benchmark large_log_baseline -- --ignored --exact --nocapture

# Example isolated GiB workload; repeat with concurrency 1, 2, and 4.
RAIN_BENCH_BYTES=1073741824 RAIN_BENCH_CONCURRENCY=4 \
  RAIN_BENCH_QUERIES=100 RAIN_BENCH_ITERATIONS=3 \
  RAIN_BENCH_REPORT=/tmp/rain-1gib-c4.jsonl \
  cargo test --release --test large_log_benchmark large_log_baseline -- --ignored --exact --nocapture
```

The large benchmark is always ignored by ordinary `cargo test`. Each iteration
uses a new random temporary directory and fresh database, closes the SQLite pool
before deleting files (including on workload panic), and appends one JSON object
to `RAIN_BENCH_REPORT` if set. Select an output location outside its temporary
directory. Ensure enough free disk for input, CAS, SQLite, and WAL; four bundles
multiply the input size. Do not run large workloads alongside other tests or
compilation when collecting measurements.

| Variable | Default | Meaning |
| --- | --- | --- |
| `RAIN_BENCH_BYTES` | `104857600` | Minimum uncompressed bytes **per bundle**, rounded up to a complete line |
| `RAIN_BENCH_CONCURRENCY` | `1` | `1`, `2`, or `4` simultaneous bundles sharing one issue/database |
| `RAIN_BENCH_VARIANT` | `plain` | `plain`, `zip`, or `targz`; archive creation streams from the fixture file |
| `RAIN_BENCH_WARMUP` | `0` | Unmeasured warmup requests per query/endpoint before samples |
| `RAIN_BENCH_QUERIES` | `20` | Samples per query and per endpoint |
| `RAIN_BENCH_ITERATIONS` | `1` | Independent fresh-database workloads |
| `RAIN_BENCH_HOST_NOTES` | unset | Host/storage/cache notes, e.g. device model and other workloads |
| `RAIN_BENCH_REPORT` | unset | Append-only JSONL output path; reports also print to stdout |

Fixtures are streamed line by line through a buffered file writer and SHA-256
hasher. They contain deterministic timestamped lines, a common `INFO` term, a
rare sentinel, a UUID, and Chinese text. Each concurrent stream has distinct
content to avoid accidentally benchmarking only CAS deduplication. Fixture
version, actual bytes, line count, and SHA-256 are recorded. Only a small fixture
unit test reads its complete 4 KiB output into memory. The workload never builds
an in-memory multipart upload.

Timing starts after generating inputs and creating PROCESSING bundle records.
It calls the production `process_uploaded_file` and
`finalize_bundle_ready_with_retry` APIs and waits for all bundles to become READY.
The receive/network path, multipart parser, and upload admission semaphore are
excluded. Concurrency therefore measures concurrent processing directly. The
benchmark uses default limits, except its issue quota is raised when necessary
to fit the requested aggregate content; the effective limits are in the report.
Use `RAIN_BENCH_VARIANT=zip` or `targz` for extraction workloads. These package
the same deterministic log into an archive on disk before timing. Both raw
fixture and uploaded archive byte counts/SHA-256 are reported. Compare variants
separately; throughput uses raw log bytes, not compressed upload size.

Search requests use a real authenticated Actix test application, both the bundle
and issue HTTP routes, a result size of 10, and all four query categories. Every
response must succeed and contain matches. Timings include handler execution
and JSON body decoding, but no network transport. Samples run serially in a
fixed order, with configurable warmup (zero by default) and no cache flush.
Only the configured warmup requests are excluded from percentiles. Raw
samples and nearest-rank p50/p95/p99 are recorded separately per term/endpoint;
20 samples are only a smoke baseline, so use more samples for tail estimates.
Searches happen after all ingest completes; this does not measure searches
competing with ingest.

A 100 ms sampler records DB/WAL file sizes, process RSS, CPU user/system ticks, and cumulative process
read/write byte counters throughout ingest. Linux uses `/proc/self/status` and
`/proc/self/io` plus process-wide `/proc/self/stat` CPU counters; tick-rate
metadata comes from `getconf CLK_TCK`; unavailable counters are JSON `null`, never zero substitutes.
These cover the whole test process, not individual bundles. Peak RSS is a sampled
peak and can miss short spikes; file sizes are logical lengths, not allocated
disk blocks. Use counter differences for I/O attribution and account for OS cache
effects. Machine architecture, logical CPU count, Linux CPU/memory data, OS,
filesystem/mount information (`df -T` on Linux), optional host notes,
rustc, git commit/working-tree status, build profile indicator, and effective
configuration accompany each report.

A scoped tracing subscriber aggregates production `log_index_file`,
`sqlite_write`, and `operation_phase` numeric counters/timings by metric, phase,
outcome, and SQLite operation. Only ingest-stage events are included; setup
and query events are excluded. These sums describe overlapping work across bundles, so they must
not be added together to infer wall time. Missing metrics mean instrumentation
was not emitted/captured, not a measured zero. Reports are diagnostic artifacts,
not automatic regression thresholds; compare the same host, build, fixture
hashes, concurrency, configuration, and cache conditions.

## First measured result (2026-09-22)

One release run used the plain-text fixture at a 100 MiB minimum, one Bundle,
20 samples per term/scope plus one warm-up. The complete run took 142.48
seconds because the eight query groups were serial; ingest reached READY in
61.552 seconds. It processed 104,857,650 source bytes and 1,248,225 lines.
Measured ingest throughput was 1.625 MiB/s and 20,279 lines/s.

The stage report identifies the current bottleneck: file read/clean/parse took
1.615 seconds, while the file summary spent 59.691 seconds around index writes.
The 250 index batches accumulated 4.266 seconds of SQL execution and 55.383
seconds in transaction finish/commit. CAS persist and verification together
were 0.242 seconds. Peak sampled RSS was 23.5 MiB, the SQLite file was 212.7
MiB, and the WAL was 7.0 MiB. These are sampled measurements from one host.

Search p95 for the same 100 MiB Bundle was approximately 213 ms for `INFO`,
628 ms for the rare sentinel, 2.80 s for the UUID, and 195 ms for the Chinese
term. The corresponding Issue fan-out p95 values were 53.8 ms, 3.21 ms,
6.51 ms, and 2.08 ms. The Bundle route is expensive even with one Bundle, so
moving index writes out of SQLite is the first performance priority. The full
1/2/4 Bundle and 1/5 GiB matrix remains pending. An earlier 1,000-sample run
exceeded several hours in the query phase and was terminated; it produced no
report and is not used for conclusions.
