import { deserializeSearchTokens, type SearchToken } from '../searchTokens';

export function detailEditorState(queryText: string | undefined, options?: Record<string, unknown>): {
  tokens: SearchToken[];
  error: string | null;
} {
  void options;
  if (!queryText) return { tokens: [], error: null };
  try {
    return { tokens: deserializeSearchTokens(queryText), error: null };
  } catch (error) {
    return { tokens: [], error: error instanceof Error ? error.message : '搜索条件无法加载' };
  }
}
