import { useCallback, useEffect, useSyncExternalStore } from 'react';
import { normalizeApiError, RequestCancelledError } from '../../../api/client';
import type { FileLinesResponse } from '../../../api/types';
import {
  FileContentCache,
  type FileContentRequestKey
} from '../fileContentCache';

type UseFileContentOptions = {
  cache: FileContentCache<FileLinesResponse>;
  owner: string | null;
  request: FileContentRequestKey | null;
};

const EMPTY_SNAPSHOT = { value: null, error: null };

export function useFileContent({ cache, owner, request }: UseFileContentOptions) {
  const snapshot = useSyncExternalStore(
    (listener) => owner ? cache.subscribe(owner, listener) : () => undefined,
    () => cache.getSnapshot(owner, request),
    () => EMPTY_SNAPSHOT
  );

  useEffect(() => {
    if (!owner || !request) return;
    let active = true;
    void cache.request(owner, request).catch((error: unknown) => {
      if (!active || error instanceof RequestCancelledError || (error instanceof Error && error.name === 'AbortError')) return;
      // The cache exposes this error in the next snapshot. Cancellation is
      // intentionally invisible during tab changes.
    });
    return () => {
      active = false;
    };
  }, [cache, owner, request]);

  const retryFileContent = useCallback(() => {
    if (!owner || !request) return;
    cache.reset(owner);
    void cache.request(owner, request).catch(() => undefined);
  }, [cache, owner, request]);

  useEffect(() => {
    if (owner) cache.activate(owner);
  }, [cache, owner, request]);

  return {
    fileLines: snapshot.value,
    fileContentLoading: Boolean(request && snapshot.value === null && snapshot.error === null),
    fileContentError: snapshot.error === null ? null : normalizeApiError(snapshot.error),
    retryFileContent
  };
}
