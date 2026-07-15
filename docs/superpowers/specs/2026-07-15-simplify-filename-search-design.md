# Simplified filename search controls

## Goal

Make Issue-level filename search behave like a single-query search while leaving log-content boolean search unchanged. Filename mode must not expose token controls or the `+` commit action, and users must be able to clear the query and results in one action.

## UI structure

`BundleView` keeps the existing mode selector and renders mode-specific controls in the shared search bar:

- Filename mode renders a native controlled text input bound to dedicated filename query state. Enter submits the filename search directly. The existing Search button remains.
- Log-content mode renders the existing `SearchTokenEditor` with its token, `+`, AND, OR, and NOT behavior unchanged.
- Filename mode shows a text Clear button whenever it has non-empty query text, an executed search, results, an error, or a loading request. Search and Clear receive explicit accessible labels.

The controls remain inline with the current search icon and styling. No new reusable component is introduced because the filename input is used once and its behavior is owned by `BundleView`.

## State and transitions

Filename mode uses a plain `filenameQuery` string instead of `searchTokens` and `searchDraft`. Content mode continues to own the existing token and draft state. `runSearch` selects the source by mode: trimmed filename text for `mode: 'filename'`, serialized tokens for content preview.

Changing modes clears active results, count/display state, loading/error state, and result-filter state. It does not translate tokens into a filename query or filename text into content tokens, so stale state cannot leak across modes. Each mode may retain its own inactive editor value, but only the active mode can execute or display results.

Clear in filename mode:

1. empties the filename query;
2. clears results and resets executed/loading/error state;
3. clears result-filter state so the normal file tree is rendered;
4. returns focus to the filename input after React applies the state update.

If an in-flight request resolves after Clear or a mode switch, it must not repopulate stale results. A monotonically increasing request generation stored in a ref invalidates prior requests; only the current generation may update result, error, or loading state.

## Accessibility and keyboard behavior

The filename input has an explicit `aria-label` describing filename search. Its enclosing form handles Enter through submit semantics, and both Search and Clear buttons have accurate accessible names. Clear restores focus to the filename input without relying on mouse interaction.

## Testing

Frontend SSR/static tests render the filename search controls and assert that filename mode contains a plain input, Search and Clear labels, and no `+` or token/operator UI. Pure exported state helpers cover Clear visibility and stale-request generation where practical. Existing `SearchTokenEditor` tests continue to prove content-mode boolean controls remain available. `npm test`, TypeScript lint, and the production build are the final regression checks.
