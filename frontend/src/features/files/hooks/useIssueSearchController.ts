import { useSearchController } from './useSearchController';
import type { SearchToken } from '../searchTokens';

export function useIssueSearchController(initialTokens: SearchToken[] = [], initialError: string | null = null) {
  return useSearchController({ initialTokens, initialError });
}
