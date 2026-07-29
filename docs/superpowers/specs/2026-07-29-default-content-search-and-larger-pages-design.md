# Default content search and larger pages

## Goal

Make issue-level log-content search the default file-view search mode and increase line-oriented page sizes from 1,000/3,000 to 5,000/10,000 consistently across the application.

## Search mode

The file view opens with “搜日志内容” selected. Users can still switch to “按文件名”. Existing internal mode identifiers remain unchanged because renaming them does not affect user behavior and would expand the change unnecessarily.

## Pagination

All line-oriented viewers use two choices:

- 5,000 lines, selected by default.
- 10,000 lines, available as the larger option and enforced as the server maximum.

This applies to source-file pages, materialized temporary-result pages, search-result viewer pages, and temporary-result preview creation. The server's configurable defaults become 5,000 and 10,000 so frontend requests are not clamped back to the old values.

The ordinary keyword-search hit limit is separate from line-oriented viewing and remains unchanged.

## Configuration and documentation

Update `ApiConfig` defaults, `.env.example`, and the README configuration table. Environment overrides remain supported, subject to the existing validation that the default cannot exceed the maximum.

## Testing

Add or update tests proving:

- The file view initializes in log-content mode.
- Both frontend line-page option lists are exactly 5,000 and 10,000.
- Temporary-result preview defaults to 5,000 and clamps requests to 10,000.
- Backend API configuration defaults are 5,000 and 10,000.

Run focused frontend and backend tests, the complete frontend test suite, TypeScript checking, the production build, Rust formatting checks, and relevant Rust tests.
