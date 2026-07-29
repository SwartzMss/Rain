# Refresh failed upload deletion

## Goal

When a user deletes a failed upload from the Home file list, remove the row immediately after the delete request and list refresh succeed.

## Root cause

The backend correctly deletes the failed bundle and `loadBundles` refreshes the bundle list. However, the upload hook still retains the failed task and its selected files. Once the refreshed bundle list no longer contains the deleted bundle, `buildFileRows` treats that retained state as an orphaned optimistic upload and recreates the failed row.

## Design

After a bundle deletion succeeds, compare the deleted bundle hash with the current upload task's bundle hash. If they match, reset the upload selection state before refreshing the bundle and issue lists. This clears the failed task, selected filenames, progress, and failure message that generated the stale row.

Deleting a normal bundle or a bundle unrelated to the current upload task must not reset unrelated upload state. No compatibility behavior is required for retaining a deleted failed task.

## Error handling

Reset upload state only after the delete API succeeds. If deletion fails, preserve the failed row and its error context, and display the existing deletion error.

## Testing

Add a focused regression test around the deletion-state decision:

- A deleted bundle matching the current failed upload task requests an upload-state reset.
- A deleted bundle that does not match the current task does not reset upload state.
- Absence of a current upload task does not reset upload state.

Run the frontend test suite, TypeScript lint, and production build.
