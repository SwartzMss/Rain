# Resumable Upload Public Bundle Identity

## Problem

Resumable-upload finalization creates two distinct identifiers for the same bundle:

- `bundles.id` is the internal database identifier and is stored in `upload_sessions.bundle_id`.
- `bundles.hash` is the public bundle identifier used by `GET /api/uploads/{task_id}`.

The upload-session API currently exposes only `bundle_id`, so the frontend passes the internal ID to an endpoint that searches by `bundles.hash`. The production request therefore returns 404. Existing frontend tests miss this because they assign the same mock value to both identifiers.

## Chosen design

Add `bundle_hash` to every upload-session response. The server derives it from the authoritative relationship `upload_sessions.bundle_id = bundles.id` and `bundles.hash`, instead of changing the meaning of `bundle_id` or broadening the public upload-task lookup to accept internal IDs.

Keep `bundle_id` in the response for compatibility with existing clients and internal diagnostics. The frontend resumable-upload flow must use `bundle_hash` exclusively when calling `fetchUploadTask`. A delivered session without `bundle_hash` is treated as an invalid server response and fails with the existing delivery error path rather than issuing a request with an empty or internal identifier.

## Data flow

```text
upload_sessions.bundle_id ──JOIN bundles.id──> bundles.hash
          (internal)                              (public task ID)
                                                        │
                                      UploadSessionResponse.bundle_hash
                                                        │
                                      GET /api/uploads/{bundle_hash}
```

The response projection will use a left join or equivalent optional lookup so sessions without an attached bundle continue to serialize normally with `bundle_hash: null`. Sessions with a bundle must return the corresponding public hash. The same projection is used for create, get, list, chunk, and complete session responses.

## Components

1. Backend upload-session response mapping: add `bundle_hash: Option<String>` and load it from `bundles.hash` through the session’s internal bundle ID.
2. Frontend API type: add `bundle_hash?: string | null` while retaining `bundle_id` for compatibility.
3. Frontend resumable flow: replace both delivered-session calls to `fetchUploadTask(session.bundle_id)` with a required public-hash lookup.
4. Tests: use distinct values such as `internal-bundle-1` and `public-hash-1`; assert the request uses the public hash and add backend coverage that the response maps the joined hash.

## Error handling

For a delivered session, the frontend requires a non-empty `bundle_hash`. If it is absent, throw the existing “server did not return delivered upload task” error rather than falling back to `bundle_id`. This prevents the internal identifier from re-entering the public lookup path.

## Compatibility and scope

- No database migration is needed; both columns already exist.
- No change is made to `GET /api/uploads/{task_id}`; it remains a public-hash endpoint.
- No `WHERE hash = ? OR id = ?` fallback is introduced.
- Non-resumable multipart uploads and all existing bundle consumers remain unchanged.

## Verification

- Frontend regression tests must fail before the implementation when mocks distinguish internal ID from public hash, then pass after the implementation.
- Backend upload-session tests must verify a finalized/delivered session serializes the public `bundle_hash` separately from `bundle_id`.
- Run focused frontend and backend tests, formatting, and the repository’s relevant full test/build checks before creating the pull request.
