import type { SavedSearchPayload } from '../../api/types';

export const PENDING_SAVED_SEARCH_KEY = 'rain.pendingSavedSearch';

export function takePendingSavedSearch(
  storage: Pick<Storage, 'getItem' | 'removeItem'>,
  authenticated: boolean
): SavedSearchPayload | null {
  if (!authenticated) return null;
  const raw = storage.getItem(PENDING_SAVED_SEARCH_KEY);
  if (!raw) return null;
  try {
    const pending = JSON.parse(raw) as SavedSearchPayload;
    if (
      !pending
      || typeof pending !== 'object'
      || pending.search_type !== 'DETAIL'
      || typeof pending.query_text !== 'string'
      || !pending.query_text.trim()
      || !pending.options
      || typeof pending.options !== 'object'
      || Array.isArray(pending.options)
    ) {
      storage.removeItem(PENDING_SAVED_SEARCH_KEY);
      return null;
    }
    storage.removeItem(PENDING_SAVED_SEARCH_KEY);
    return pending;
  } catch {
    storage.removeItem(PENDING_SAVED_SEARCH_KEY);
    return null;
  }
}
