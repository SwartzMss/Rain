import { useCallback, useEffect, useRef, useState } from 'react';
import { ApiError, normalizeApiError, rainApi } from '../../../api/client';
import type { ViewerTab } from '../viewerTabs';

type PageState = { loading: boolean; error: string | null };
type PageRequest = { sequence: number; resultId: string; contextKey: string };

export function useViewerPaginationController(
  contextKey: string,
  updateViewerTabs: (update: (tabs: ViewerTab[]) => ViewerTab[]) => void,
  onUnavailable?: (resultId: string) => void
) {
  const [states, setStates] = useState<Record<string, PageState>>({});
  const requestsRef = useRef(new Map<string, PageRequest>());
  const contextRef = useRef(contextKey);

  useEffect(() => {
    contextRef.current = contextKey;
    requestsRef.current.clear();
    setStates({});
  }, [contextKey]);

  useEffect(() => () => {
    requestsRef.current.clear();
  }, []);

  const loadPage = useCallback(async (
    tab: ViewerTab,
    from: number,
    pageSize: number,
    navigation: 'next' | 'previous' | 'reset'
  ) => {
    if (tab.kind !== 'search' && tab.kind !== 'temp') return;
    const resultId = (tab as Extract<ViewerTab, { kind: 'search' }> | Extract<ViewerTab, { kind: 'temp' }>).resultId;
    const previous = requestsRef.current.get(tab.id);
    const request: PageRequest = {
      sequence: (previous?.sequence ?? 0) + 1,
      resultId,
      contextKey
    };
    requestsRef.current.set(tab.id, request);
    setStates((current) => ({ ...current, [tab.id]: { loading: true, error: null } }));
    const isCurrent = () => {
      const active = requestsRef.current.get(tab.id);
      return active?.sequence === request.sequence
        && active.resultId === request.resultId
        && active.contextKey === contextRef.current;
    };
    try {
      const response = await rainApi.fetchTempResultLines(resultId, {
        start: from,
        limit: pageSize
      });
      if (!isCurrent()) return;
      updateViewerTabs((tabs) => tabs.map((item) => {
        if (item.id !== tab.id || item.kind !== tab.kind) return item;
        if (item.resultId !== request.resultId) return item;
        const pageHistory = navigation === 'next'
          ? [...(item.pageHistory ?? []), item.from]
          : navigation === 'previous'
            ? (item.pageHistory ?? []).slice(0, -1)
            : [];
        if (item.kind === 'temp') {
          return {
            ...item,
            lines: response.lines.map((line) => line.content),
            total: response.line_count,
            from: response.start,
            pageSize: response.limit,
            pageHistory,
            scrollTop: 0
          };
        }
        return {
          ...item,
          hits: response.lines.map((line) => ({
            bundle_hash: line.bundle_hash ?? undefined,
            file_id: line.file_id ?? '',
            path: line.path ?? '',
            snippet: line.content,
            line_number: line.line_number
          })),
          total: response.line_count,
          from: response.start,
          pageSize: response.limit,
          pageHistory,
          scrollTop: 0
        };
      }));
    } catch (error) {
      if (isCurrent()) {
        if (error instanceof ApiError && error.status === 404) onUnavailable?.(resultId);
        setStates((current) => ({ ...current, [tab.id]: { loading: false, error: normalizeApiError(error) } }));
      }
      return;
    }
    if (isCurrent()) {
      setStates((current) => ({ ...current, [tab.id]: { loading: false, error: null } }));
    }
  }, [contextKey, onUnavailable, updateViewerTabs]);

  const getState = useCallback((tabId: string | null): PageState => {
    return tabId ? states[tabId] ?? { loading: false, error: null } : { loading: false, error: null };
  }, [states]);

  const invalidate = useCallback((tabId?: string) => {
    if (tabId) requestsRef.current.delete(tabId);
    else requestsRef.current.clear();
    if (tabId) setStates((current) => ({ ...current, [tabId]: { loading: false, error: null } }));
    else setStates({});
  }, []);

  return { loadPage, getState, invalidate };
}
