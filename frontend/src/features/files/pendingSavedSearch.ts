import type { SavedSearchPayload } from '../../api/types';

export const PENDING_SAVED_SEARCH_KEY = 'rain.pendingSavedSearch';

export function takePendingSavedSearch(
  storage: Pick<Storage, 'getItem' | 'removeItem'>,
  authenticated: boolean,
  issueCode: string
): SavedSearchPayload | null {
  if (!authenticated) return null;
  const raw = storage.getItem(PENDING_SAVED_SEARCH_KEY);
  if (!raw) return null;
  try {
    const pending = JSON.parse(raw) as SavedSearchPayload;
    if (
      !pending
      || !['FILENAME', 'DETAIL'].includes(pending.search_type)
      || !['GLOBAL', 'ISSUE'].includes(pending.scope_type)
    ) {
      storage.removeItem(PENDING_SAVED_SEARCH_KEY);
      return null;
    }
    if (pending.scope_type === 'ISSUE' && pending.scope_key !== issueCode) return null;
    storage.removeItem(PENDING_SAVED_SEARCH_KEY);
    return pending;
  } catch {
    storage.removeItem(PENDING_SAVED_SEARCH_KEY);
    return null;
  }
}
