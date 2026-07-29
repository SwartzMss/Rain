import type { SearchToken } from './searchTokens';

export function isFileSearchConditionEmpty(tokens: SearchToken[], draft: string) {
  return tokens.length === 0 && draft.length === 0;
}
