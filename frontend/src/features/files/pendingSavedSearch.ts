import type { SavedSearchPayload } from '../../api/types';
import { validateSearchTokens, type SearchToken } from './searchTokens';

export const PENDING_SAVED_SEARCH_KEY = 'rain.pendingSavedSearch';

function isSearchToken(value: unknown): value is SearchToken {
  if (!value || typeof value !== 'object') return false;
  const token = value as { kind?: unknown; value?: unknown };
  if (token.kind === 'term') return typeof token.value === 'string' && Boolean(token.value.trim());
  return token.kind === 'operator' && ['AND', 'OR', 'NOT'].includes(String(token.value));
}

function hasValidOptionalTokens(options: Record<string, unknown>): boolean {
  if (!('tokens' in options)) return true;
  if (
    !Array.isArray(options.tokens)
    || options.tokens.length === 0
    || !options.tokens.every(isSearchToken)
  ) {
    return false;
  }
  return validateSearchTokens(options.tokens).valid;
}

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
      || !['FILENAME', 'DETAIL'].includes(pending.search_type)
      || typeof pending.query_text !== 'string'
      || !pending.query_text.trim()
      || !pending.options
      || typeof pending.options !== 'object'
      || Array.isArray(pending.options)
      || Object.keys(pending.options).length === 0
      || !hasValidOptionalTokens(pending.options)
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
