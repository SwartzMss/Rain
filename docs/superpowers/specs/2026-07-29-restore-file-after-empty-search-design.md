# Restore File Content After Clearing File Search

## Problem

After a search within the active file returns no matches, the viewer displays
“当前文件中没有相关日志。” The dedicated “清空” button restores the original
file content, but removing the final search token with its `×` button,
Backspace, or deleting the draft text does not. Those editor actions clear the
search condition without resetting the file-search result state.

## Design

Keep the behavior local to the active-file search in `FilesView`. Whenever both
the file-search token list and draft text become empty, reset the file-search
result state through the existing `clearFileSearch` operation. This makes every
way of emptying the condition share the same state transition as the dedicated
“清空” button.

Do not change `SearchTokenEditor`: it is shared by issue searches and
search-within-results, and it should remain responsible only for editing search
conditions.

The reset must not run while either tokens or non-empty draft text remain.

## Testing

Add a focused regression test for the active-file search state rule:

- a non-empty token list does not request a reset;
- non-empty draft text does not request a reset;
- empty tokens plus empty draft text request a reset.

Run the frontend test suite, TypeScript checks, and production build after the
implementation.
