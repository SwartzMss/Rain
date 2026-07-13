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
  source: { kind: 'issue'; issueCode: string } | { kind: 'file'; bundleHash: string; fileId: string };
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
