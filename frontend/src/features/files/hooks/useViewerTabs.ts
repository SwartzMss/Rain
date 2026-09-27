import { useCallback, useEffect, useMemo, useReducer, useRef } from 'react';
import { ApiError, rainApi } from '../../../api/client';
import {
  closeViewerTab,
  openOrActivateTab,
  togglePinnedTab,
  type ViewerTab
} from '../viewerTabs';

type ViewerTabsState = {
  tabs: ViewerTab[];
  activeTabId: string | null;
};

type ViewerTabsAction =
  | { type: 'reset' }
  | { type: 'setTabs'; tabs: ViewerTab[]; activeTabId: string | null }
  | { type: 'open'; tab: ViewerTab }
  | { type: 'activate'; id: string }
  | { type: 'close'; id: string }
  | { type: 'togglePinned'; id: string }
  | { type: 'update'; update: (tabs: ViewerTab[]) => ViewerTab[] };

const initialState: ViewerTabsState = {
  tabs: [],
  activeTabId: null
};

function viewerTabsReducer(
  state: ViewerTabsState,
  action: ViewerTabsAction
): ViewerTabsState {
  switch (action.type) {
    case 'reset':
      return initialState;
    case 'setTabs':
      return { tabs: action.tabs, activeTabId: action.activeTabId };
    case 'open':
      return {
        tabs: openOrActivateTab(state.tabs, action.tab),
        activeTabId: action.tab.id
      };
    case 'activate':
      return { ...state, activeTabId: action.id };
    case 'close': {
      const remaining = closeViewerTab(state.tabs, action.id);
      if (state.activeTabId !== action.id) {
        return { ...state, tabs: remaining };
      }
      const next = remaining[remaining.length - 1] ?? null;
      return { tabs: remaining, activeTabId: next?.id ?? null };
    }
    case 'togglePinned':
      return { ...state, tabs: togglePinnedTab(state.tabs, action.id) };
    case 'update': {
      const tabs = action.update(state.tabs);
      const activeStillExists = state.activeTabId
        ? tabs.some((tab) => tab.id === state.activeTabId)
        : false;
      return {
        tabs,
        activeTabId: activeStillExists ? state.activeTabId : tabs[tabs.length - 1]?.id ?? null
      };
    }
    default:
      return state;
  }
}

export function useViewerTabs(canDeleteResults = false) {
  const [state, dispatch] = useReducer(viewerTabsReducer, initialState);
  const ownedResults = useRef(new Set<string>());
  useEffect(() => {
    for (const tab of state.tabs) {
      if (tab.kind === 'search') ownedResults.current.add(tab.resultId);
    }
    const retained = new Set(state.tabs.flatMap((tab) => tab.kind === 'file' ? [] : [tab.resultId]));
    const removed = [...ownedResults.current].filter((id) => !retained.has(id));
    for (const id of removed) ownedResults.current.delete(id);
    if (!canDeleteResults) return;
    for (const id of removed) {
      void rainApi.deleteTempResult(id).catch((error: unknown) => {
        if (error instanceof ApiError && error.status === 404) return;
        // Closing remains immediate; expiry cleanup recovers failed requests.
        console.warn('Temporary search result cleanup failed; expiry cleanup will retry', error);
      });
    }
  }, [state.tabs, canDeleteResults]);
  const initializedRef = useRef(false);
  const activeTab = useMemo(
    () => state.tabs.find((tab) => tab.id === state.activeTabId) ?? null,
    [state.activeTabId, state.tabs]
  );

  const openTab = useCallback((tab: ViewerTab) => {
    initializedRef.current = true;
    dispatch({ type: 'open', tab });
  }, []);

  const activateTab = useCallback((tab: ViewerTab) => {
    initializedRef.current = true;
    dispatch({ type: 'activate', id: tab.id });
  }, []);

  const closeTab = useCallback((id: string) => {
    dispatch({ type: 'close', id });
  }, []);

  const setTabs = useCallback((tabs: ViewerTab[], activeTabId: string | null) => {
    initializedRef.current = tabs.length > 0;
    dispatch({ type: 'setTabs', tabs, activeTabId });
  }, []);

  const resetTabs = useCallback(() => {
    initializedRef.current = false;
    dispatch({ type: 'reset' });
  }, []);

  const updateTabs = useCallback((update: (tabs: ViewerTab[]) => ViewerTab[]) => {
    dispatch({ type: 'update', update });
  }, []);

  const togglePinned = useCallback((id: string) => {
    dispatch({ type: 'togglePinned', id });
  }, []);

  return {
    viewerTabs: state.tabs,
    activeViewerTabId: state.activeTabId,
    activeViewerTab: activeTab,
    viewerInitializedRef: initializedRef,
    openViewerTab: openTab,
    activateViewerTab: activateTab,
    closeViewerTab: closeTab,
    setViewerTabsState: setTabs,
    resetViewerTabs: resetTabs,
    updateViewerTabs: updateTabs,
    togglePinnedViewerTab: togglePinned
  };
}
