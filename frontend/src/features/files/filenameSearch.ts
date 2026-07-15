export type FilenameSearchState = {
  query: string;
  executed: boolean;
  resultCount: number;
  loading: boolean;
  error: string | null;
};

export function shouldShowFilenameClear(state: FilenameSearchState): boolean {
  return Boolean(
    state.query.trim() ||
      state.executed ||
      state.resultCount > 0 ||
      state.loading ||
      state.error
  );
}
