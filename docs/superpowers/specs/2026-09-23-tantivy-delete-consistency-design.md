# Tantivy deletion consistency design

## Goal

Guarantee that a file hidden by `visible_files` cannot be returned by Bundle, Issue-wide, or future Skill Tantivy search, including while an asynchronous file or directory deletion is queued, running, or retrying. Keep immutable Tantivy generations and compact stale documents asynchronously.

## Design

`visible_files` remains the search visibility authority. Each Tantivy request obtains one visibility snapshot containing visible file IDs before reading the immutable artifact. Candidate documents are filtered by that snapshot before exact text filtering, counting, sorting, and pagination. The snapshot is loaded in one SQL query, so directory deletion reuses the existing recursive view and does not create N+1 queries.

The publication record gains a monotonic visibility revision and an active/dirty distinction. Enqueuing a deletion increments the revision and marks the Bundle `NEEDS_REBUILD` without taking the current READY generation offline. Search continues against the active generation plus the current visibility snapshot.

A background rebuild copies only visible documents from the active generation into a new immutable generation, verifies it, and atomically publishes it. The build records the revision it covered; if another deletion occurs during the build, the Bundle remains dirty and is rebuilt again. Failed builds leave the active generation queryable. Old generations are removed only after publication and after no search lease references them; interrupted staging and retired artifacts are retried at startup/periodically.

## Data flow

```text
enqueue deletion transaction
  -> visible_files hides subtree
  -> visibility_revision += 1; state = NEEDS_REBUILD
  -> search snapshot excludes subtree immediately
  -> background rebuild(active generation, snapshot)
  -> verify new artifact
  -> publish generation atomically with covered revision
  -> retire and garbage-collect old artifact
```

## Correctness rules

- Exact `total` counts only visible, filtered, exact-text matches.
- Pagination is applied after filtering; the collector keeps enough rows for `from + size` while still scanning all candidates for `total`.
- Bundle and Issue-wide searches use the same snapshot/filter contract.
- A deletion job in `QUEUED`, `RUNNING`, or `RETRY_WAIT` is invisible immediately after its enqueue transaction commits.
- An empty rebuilt index is valid and publishable.
- A failed rebuild never changes the active generation.

## Verification

Regression coverage includes single-file and recursive directory deletion, deletion in progress and retry, exact total/pagination after hidden candidates, Issue-wide mixed search, rebuild failure, rebuild during a second deletion, generation publication, stale-artifact cleanup, and restart recovery. Required checks are `cargo test --locked`, `cargo clippy --locked --lib -- -D warnings`, and `cargo fmt --check`.
