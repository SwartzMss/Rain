# Issue #224: Tantivy-backed temporary preview design

## Goal

Make `POST /api/temp-results/preview` reuse a published Tantivy Bundle index for
common searches without changing the existing temporary-result response or
exact expression semantics. Tantivy is a candidate source only; raw lines remain
the authority for verification and materialization.

## Scope

The first fast path supports an expression that parses to `Expression::Term`
with at least three ASCII characters, no leading or trailing whitespace, and no
NUL byte. This includes ordinary keywords and quoted ASCII phrases after the
existing parser normalization. The following continue to use raw scanning:

- boolean expressions, including `AND`, `OR`, and `NOT`;
- terms shorter than the Tantivy trigram minimum;
- terms whose case-folding or byte semantics are not guaranteed to match the
  Tantivy tokenizer;
- `source_temp_id` inputs and sources without Bundle/File identity;
- missing, unpublished, incompatible, or over-bounded index candidates.

This deliberately keeps the first implementation small while providing a
correct extension point for a broader expression planner later.

## Architecture

### Source identity

Resolved Bundle/File sources carry their internal `bundle_id` and `file_id` in
addition to the existing raw path and display metadata. Issue sources retain
their existing source order and receive the same identity information. A
materialized temporary result has no Bundle/File identity and therefore always
uses the current raw-scan path.

### Search planning

Before materialization, the service classifies the parsed expression. A source
is eligible for Tantivy only when the expression classifier accepts it and the
source has Bundle/File identity. The planner then reads the Bundle's current
publication row and validates:

1. backend is Tantivy;
2. state is `READY` or `NEEDS_REBUILD`;
3. schema and tokenizer versions match the current Tantivy constants; and
4. the current generation has a valid artifact path.

An eligible query reuses the existing search-plane infrastructure:

- the process query semaphore;
- a visibility snapshot when the publication is not fully compacted;
- the generation lease registry; and
- the existing bounded Tantivy candidate search.

The candidate request is restricted to the source file. The planner requests a
bounded all-candidate window. If the result reaches the hard search window and
may be incomplete, it abandons the fast path for that source and records a raw
fallback reason instead of risking a false negative.

Fast-path and raw-scan sources may coexist in one Issue preview. Source order
remains the order returned by source resolution.

### Candidate verification and materialization

Tantivy returns matching chunk metadata, including `line_start` and `line_end`.
The temporary-result executor accepts optional candidate line ranges per source:

1. use the nearest sparse `log_line_offsets` entry to seek close to the first
   candidate range;
2. read only through the candidate ranges, skipping lines outside them;
3. run the existing `ExpressionChunkMatcher` on each raw line;
4. write verified matches through the existing `.log`, `.meta`, and `.idx`
   pipeline; and
5. apply the existing `from`/`size`, output-size, retention, and publication
   behavior after verification.

An empty candidate set produces an empty result without scanning the source.
Candidate ranges are sorted and merged before scanning so a result cannot
contain duplicate lines if future index changes produce overlapping ranges.

The raw path remains unchanged for sources without a candidate plan. This
preserves raw-line whitespace, invalid UTF-8 decoding, truncation markers,
metadata inheritance, and all existing timeout/resource protections.

## Error handling and observability

Index absence, publication incompatibility, unsupported expressions, candidate
overflow, and Tantivy-specific lookup failures are non-fatal plan failures:
the source falls back to raw scanning and the response contract remains
unchanged. Raw source errors, materialization errors, and existing timeout
errors continue to fail the request as they do today.

Each preview emits a structured `temp_result_preview` event containing:

- `search_backend`: `tantivy`, `raw_scan`, or `mixed`;
- candidate-query elapsed time;
- candidate count;
- verified match count; and
- a fallback reason when a source cannot use Tantivy.

Generation leases and visibility snapshots stay alive for the candidate query;
the materializer only consumes the resulting line ranges and never owns index
lifecycle state.

## Testing strategy

- Unit-test expression classification for supported terms and every fallback
  category.
- Unit-test candidate-range materialization, including sparse seeking, exact
  matcher verification, ordering, empty candidates, and duplicate-range
  merging.
- Add an integration-level publication fixture that resolves a READY Tantivy
  generation, obtains candidates for one file, and verifies the preview output
  matches the raw expression result.
- Preserve and extend the existing smoke preview coverage for Bundle/File
  requests, quoted phrases, and unsupported boolean expressions.
- Run backend formatting, clippy, the full Tantivy-enabled backend suite, and
  the existing frontend build before creating the PR.
