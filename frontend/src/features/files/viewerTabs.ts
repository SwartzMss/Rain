import type { IssueLogSearchHit } from '../../api/types';

type ViewerTabBase = {
  id: string;
  title: string;
  pinned: boolean;
  scrollTop: number;
};

export type FileViewerTab = ViewerTabBase & {
  kind: 'file';
  nodeId: string;
  lineStart: number;
  pageSize: number;
  targetLine: number | null;
};

export type SearchViewerTab = ViewerTabBase & {
  kind: 'search';
  expression: string;
  hits: IssueLogSearchHit[];
  total: number;
  from: number;
  pageSize: number;
  source:
    | { kind: 'issue'; issueCode: string }
    | { kind: 'file'; bundleHash: string; fileId: string }
    | { kind: 'temp'; resultId: string };
};

export type TempViewerTab = ViewerTabBase & {
  kind: 'temp';
  resultId: string;
  expression: string;
  lines: string[];
  total: number;
  from: number;
  pageSize: number;
};

export type ViewerTab = FileViewerTab | SearchViewerTab | TempViewerTab;

export type FileTabMetadata = {
  nodeId: string;
  title: string;
};

export function reconcileViewerTabs(
  tabs: ViewerTab[],
  activeTabId: string | null,
  filesByNodeId: Record<string, FileTabMetadata>
): { tabs: ViewerTab[]; activeTabId: string | null } {
  const activeIndex = activeTabId ? tabs.findIndex((tab) => tab.id === activeTabId) : -1;
  const reconciled = tabs.reduce<ViewerTab[]>((next, tab) => {
    if (tab.kind !== 'file') {
      next.push(tab);
      return next;
    }

    const metadata = filesByNodeId[tab.nodeId];
    if (!metadata) return next;

    next.push({
      ...tab,
      id: `file:${metadata.nodeId}`,
      nodeId: metadata.nodeId,
      title: metadata.title
    });
    return next;
  }, []);

  if (!activeTabId) {
    return { tabs: reconciled, activeTabId: null };
  }

  if (reconciled.some((tab) => tab.id === activeTabId)) {
    return { tabs: reconciled, activeTabId };
  }

  const fallback = reconciled[Math.min(Math.max(activeIndex, 0), reconciled.length - 1)] ?? null;
  return { tabs: reconciled, activeTabId: fallback?.id ?? null };
}

export function openOrActivateTab(tabs: ViewerTab[], incoming: ViewerTab): ViewerTab[] {
  const existingIndex = tabs.findIndex((tab) => tab.id === incoming.id);
  if (existingIndex >= 0) {
    return tabs.map((tab, index) => (index === existingIndex ? { ...incoming, pinned: tab.pinned } : tab));
  }
  return [...tabs, incoming];
}

export function togglePinnedTab(tabs: ViewerTab[], id: string): ViewerTab[] {
  return tabs.map((tab) => (tab.id === id ? { ...tab, pinned: !tab.pinned } : tab));
}

export function closeViewerTab(tabs: ViewerTab[], id: string): ViewerTab[] {
  return tabs.filter((tab) => tab.id !== id);
}
