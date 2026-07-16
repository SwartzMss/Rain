import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useLocation, useParams } from 'react-router-dom';
import { normalizeApiError, rainApi } from '../../api/client';
import type { IssueLogSearchHit, LogSearchHit, UploadSummary } from '../../api/types';
import type { BundleInfo } from '../../lib/bundles';
import { BinaryFileInfo } from './BinaryFileInfo';
import { SearchTokenEditor } from './SearchTokenEditor';
import { canPreviewText, isArchiveNode, isBinaryNode } from './filePresentation';
import { shouldShowFilenameClear } from './filenameSearch';
import { getSearchHitSource } from './searchHitSource';
import { uploadFailureMessage } from './uploadFailure';
import {
  canFinalizeSearch,
  finalizeSearchTokens,
  formatSearchTokens,
  getSearchTerms,
  serializeSearchTokens,
  type SearchToken
} from './searchTokens';
import {
  reconcileViewerTabs,
  type ViewerTab
} from './viewerTabs';
import {
  formatHitPath,
  isExtractionFolder,
  toTreeNode,
  type TreeNode
} from './treeModel';
import { useViewerTabs } from './hooks/useViewerTabs';
import { useFileContent } from './hooks/useFileContent';
import { ViewerTabBar } from './components/ViewerTabBar';
import { CodeLinesPane } from './components/CodeLinesPane';
import { FileTreeNode } from './components/FileTreeNode';
import { SearchResultViewer } from './components/SearchResultViewer';

const LINE_PAGE_SIZE_OPTIONS = [1000, 3000] as const;

const bundleStatusLabel = (bundle: UploadSummary) => {
  if (bundle.status.upload_status === 'PROCESSING' || bundle.status.upload_status === 'PENDING') {
    return '正在建立索引';
  }
  if (bundle.status.upload_status === 'FAILED') {
    return uploadFailureMessage({
      status: bundle.status.upload_status,
      failure_reason: bundle.failure_reason
    }) ?? '处理失败';
  }
  return bundle.status.upload_status;
};

function highlightText(text: string, keyword: string): React.ReactNode {
  const normalizedKeyword = keyword.trim();
  if (!normalizedKeyword) return text;

  const lowerText = text.toLowerCase();
  const lowerKeyword = normalizedKeyword.toLowerCase();
  const parts: React.ReactNode[] = [];
  let start = 0;
  let matchIndex = lowerText.indexOf(lowerKeyword, start);

  while (matchIndex !== -1) {
    if (matchIndex > start) {
      parts.push(text.slice(start, matchIndex));
    }
    const end = matchIndex + normalizedKeyword.length;
    parts.push(
      <mark
        key={`${matchIndex}-${end}`}
        className="rounded bg-cyan-400/20 px-0.5 text-cyan-800"
      >
        {text.slice(matchIndex, end)}
      </mark>
    );
    start = end;
    matchIndex = lowerText.indexOf(lowerKeyword, start);
  }

  if (start < text.length) {
    parts.push(text.slice(start));
  }

  return parts;
}

type BundleViewProps = {
  legacyBundleHash?: string;
  legacyState?: unknown;
};

export function BundleView(props?: BundleViewProps) {
  const params = useParams<{ issueCode?: string; bundleHash?: string }>();
  const bundleHash = params.bundleHash || props?.legacyBundleHash || '';
  const issueCodeFromRoute = params.issueCode;
  const location = useLocation();
  const locationState = (location.state as { issue?: string; bundleName?: string } | null) ?? (props?.legacyState as
    | { issue?: string; bundleName?: string }
    | null);
  const issueCode = issueCodeFromRoute || locationState?.issue || '';

  const activeBundle: BundleInfo = {
    hash: bundleHash,
    name: locationState?.bundleName || bundleHash,
    issue: issueCode
  };

  const bundleId = activeBundle.hash || '';
  const [rootIds, setRootIds] = useState<string[]>([]);
  const [treeNodes, setTreeNodes] = useState<Record<string, TreeNode>>({});
  const [expandedNodes, setExpandedNodes] = useState<Set<string>>(new Set());
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [treeLoading, setTreeLoading] = useState(false);
  const [treeError, setTreeError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const [filenameQuery, setFilenameQuery] = useState('');
  const [searchTokens, setSearchTokens] = useState<SearchToken[]>([]);
  const [searchDraft, setSearchDraft] = useState('');
  const [searchResults, setSearchResults] = useState<IssueLogSearchHit[]>([]);
  const [searchLoading, setSearchLoading] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [searchExecuted, setSearchExecuted] = useState(false);
  const [searchMode, setSearchMode] = useState<'log' | 'detailed'>('log');
  const [resultFilterTokens, setResultFilterTokens] = useState<SearchToken[]>([]);
  const [resultFilterDraft, setResultFilterDraft] = useState('');
  const [fileSearchTokens, setFileSearchTokens] = useState<SearchToken[]>([]);
  const [fileSearchDraft, setFileSearchDraft] = useState('');
  const [fileSearchResults, setFileSearchResults] = useState<LogSearchHit[]>([]);
  const [fileSearchTotal, setFileSearchTotal] = useState(0);
  const [fileSearchFrom, setFileSearchFrom] = useState(0);
  const [fileSearchLoading, setFileSearchLoading] = useState(false);
  const [fileSearchError, setFileSearchError] = useState<string | null>(null);
  const [fileSearchExecuted, setFileSearchExecuted] = useState(false);
  const [nonReadyBundles, setNonReadyBundles] = useState<UploadSummary[]>([]);
  const [sourceActionMessage, setSourceActionMessage] = useState<string | null>(null);
  const contentRef = useRef<HTMLDivElement | null>(null);
  const filenameInputRef = useRef<HTMLInputElement | null>(null);
  const searchRequestGenerationRef = useRef(0);
  const contextKeyRef = useRef<string | null>(null);
  const viewerTabsRef = useRef<ViewerTab[]>([]);
  const activeViewerTabIdRef = useRef<string | null>(null);
  const selectedNodeIdRef = useRef<string | null>(null);
  const treeNodesRef = useRef<Record<string, TreeNode>>({});
  const {
    viewerTabs,
    activeViewerTabId,
    activeViewerTab,
    viewerInitializedRef,
    openViewerTab,
    activateViewerTab,
    closeViewerTab,
    setViewerTabsState,
    resetViewerTabs,
    updateViewerTabs,
    togglePinnedViewerTab
  } = useViewerTabs();
  const selectedNode = selectedNodeId ? treeNodes[selectedNodeId] : null;
  const {
    fileLines,
    lineStart,
    setLineStart,
    linePageSize,
    setLinePageSize,
    fileContentLoading,
    fileContentError,
    targetLine,
    setTargetLine
  } = useFileContent({
    bundleId,
    selectedNode,
    defaultPageSize: LINE_PAGE_SIZE_OPTIONS[0]
  });

  useEffect(() => {
    viewerTabsRef.current = viewerTabs;
  }, [viewerTabs]);

  useEffect(() => {
    activeViewerTabIdRef.current = activeViewerTabId;
  }, [activeViewerTabId]);

  useEffect(() => {
    selectedNodeIdRef.current = selectedNodeId;
  }, [selectedNodeId]);

  useEffect(() => {
    treeNodesRef.current = treeNodes;
  }, [treeNodes]);

  const loadNode = useCallback(
    async (
      bundle: string,
      nodeId: string,
      parentId: string | null = null
    ): Promise<{ node: TreeNode; children: TreeNode[] } | null> => {
      if (!bundle) return null;
      setTreeLoading(true);
      setTreeError(null);
      try {
        const response = await rainApi.fetchFileNode(bundle, nodeId);
        let normalized: TreeNode | null = null;
        const childrenNodes: TreeNode[] = [];
        const key = response.node.id.toString();
        const inferredParent = parentId ?? null;
        const base = toTreeNode(bundle, response.node, inferredParent);
        base.hasLoadedChildren = true;

        // flatten extraction folders under archives
        for (const child of response.children ?? []) {
          const childNode = toTreeNode(bundle, child, base.id);
          const parentForChild = base;
          if (isExtractionFolder(childNode, parentForChild)) {
            try {
              const extracted = await rainApi.fetchFileNode(bundle, child.id.toString());
              (extracted.children ?? []).forEach((grand) => {
                const grandNode = toTreeNode(bundle, grand, base.id);
                childrenNodes.push(grandNode);
              });
            } catch {
              // ignore extraction load errors
            }
          } else {
            childrenNodes.push(childNode);
          }
        }

        base.childrenIds = childrenNodes.map((child) => child.id);
        normalized = base;

        setTreeNodes((prev) => {
          const next = { ...prev };
          next[base.id] = base;
          childrenNodes.forEach((child) => {
            next[child.id] = child;
          });
          return next;
        });

        return normalized ? { node: normalized, children: childrenNodes } : null;
      } catch (error) {
        setTreeError(normalizeApiError(error));
        throw error;
      } finally {
        setTreeLoading(false);
      }
    },
    []
  );

  const runSearch = useCallback(async () => {
    const issue = issueCode;
    if (!issue) {
      setSearchResults([]);
      setSearchError(null);
      setSearchExecuted(false);
      setResultFilterTokens([]);
      setResultFilterDraft('');
      return;
    }
    let keyword = filenameQuery.trim();
    let title = keyword;
    if (searchMode === 'detailed') {
      let finalizedTokens: SearchToken[];
      try {
        finalizedTokens = finalizeSearchTokens(searchTokens, searchDraft);
      } catch (error) {
        setSearchError(error instanceof Error ? error.message : '搜索条件无效');
        return;
      }
      keyword = serializeSearchTokens(finalizedTokens);
      title = formatSearchTokens(finalizedTokens);
      setSearchTokens(finalizedTokens);
      setSearchDraft('');
    } else if (!keyword) {
      return;
    }
    const requestGeneration = ++searchRequestGenerationRef.current;
    setSearchLoading(true);
    setSearchError(null);
    setSearchExecuted(true);
    setResultFilterTokens([]);
    setResultFilterDraft('');
    setFileSearchResults([]);
    setFileSearchTokens([]);
    setFileSearchDraft('');
    setFileSearchTotal(0);
    setFileSearchFrom(0);
    setFileSearchError(null);
    setFileSearchExecuted(false);
    try {
      if (searchMode === 'log') {
        const response = await rainApi.searchIssueLogs(issue, keyword, {
          mode: 'filename',
          size: 50
        });
        if (requestGeneration !== searchRequestGenerationRef.current) return;
        setSearchResults(response.hits);
      } else {
        const response = await rainApi.previewTempResult({
          expression: keyword,
          issue_code: issue,
          from: 0,
          size: LINE_PAGE_SIZE_OPTIONS[0]
        });
        if (requestGeneration !== searchRequestGenerationRef.current) return;
        const hits = response.lines.map((line) => ({
          bundle_hash: line.bundle_hash,
          file_id: line.file_id ?? '',
          path: line.path,
          snippet: line.content,
          line_number: line.line_number
        }));
        setSearchResults(hits);
        const id = `search:${Date.now()}`;
        openViewerTab({
          id,
          kind: 'search',
          resultId: response.result_id,
          title,
          pinned: false,
          scrollTop: 0,
          expression: keyword,
          hits,
          total: response.total,
          from: 0,
          pageSize: LINE_PAGE_SIZE_OPTIONS[0],
          source: { kind: 'issue', issueCode: issue }
        });
      }
    } catch (error) {
      if (requestGeneration !== searchRequestGenerationRef.current) return;
      setSearchResults([]);
      setSearchError(normalizeApiError(error));
    } finally {
      if (requestGeneration === searchRequestGenerationRef.current) {
        setSearchLoading(false);
      }
    }
  }, [filenameQuery, issueCode, openViewerTab, searchDraft, searchMode, searchTokens]);

  const clearFilenameSearch = useCallback(() => {
    searchRequestGenerationRef.current += 1;
    setFilenameQuery('');
    setSearchResults([]);
    setSearchLoading(false);
    setSearchError(null);
    setSearchExecuted(false);
    setResultFilterTokens([]);
    setResultFilterDraft('');
    window.requestAnimationFrame(() => filenameInputRef.current?.focus());
  }, []);

  const handleSearchSubmit = (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    void runSearch();
  };

  const changeSearchMode = (mode: 'log' | 'detailed') => {
    if (mode === searchMode) return;
    searchRequestGenerationRef.current += 1;
    setSearchMode(mode);
    setSearchResults([]);
    setSearchLoading(false);
    setSearchError(null);
    setSearchExecuted(false);
    setResultFilterTokens([]);
    setResultFilterDraft('');
  };

  useEffect(() => {
    const issueCode = issueCodeFromRoute || locationState?.issue || '';
    const fallbackBundles = bundleId ? [{ hash: bundleId, name: activeBundle.name }] : [];
    const contextKey = `${issueCode}\u0000${bundleId}`;
    const isContextChange = contextKeyRef.current !== contextKey;
    contextKeyRef.current = contextKey;

    let ignore = false;
    const init = async () => {
      setTreeLoading(true);
      setTreeError(null);
      const activeTabIdSnapshot = activeViewerTabIdRef.current;
      const tabsSnapshot = activeTabIdSnapshot && contentRef.current
        ? viewerTabsRef.current.map((tab) =>
            tab.id === activeTabIdSnapshot ? { ...tab, scrollTop: contentRef.current?.scrollTop ?? tab.scrollTop } : tab
          )
        : viewerTabsRef.current;

      if (isContextChange) {
        setTreeNodes({});
        setExpandedNodes(new Set());
        setRootIds([]);
        setSelectedNodeId(null);
        setNonReadyBundles([]);
        resetViewerTabs();
      } else if (tabsSnapshot !== viewerTabsRef.current) {
        setViewerTabsState(tabsSnapshot, activeTabIdSnapshot);
      }

      let bundles = fallbackBundles;
      let loadFailed = false;
      if (issueCode) {
        try {
          const data = await rainApi.fetchIssueBundles(issueCode);
          const notReady = data.log_bundles.filter(
            (bundle) => bundle.status.upload_status !== 'READY'
          );
          if (!ignore) {
            setNonReadyBundles(notReady);
          }
          const list = data.log_bundles
            .filter((bundle) => bundle.status.upload_status === 'READY')
            .map((bundle) => ({ hash: bundle.hash, name: bundle.name || bundle.hash }));
          bundles = list;
        } catch (error) {
          loadFailed = true;
          if (!ignore) {
            setTreeError(normalizeApiError(error));
          }
        }
      }

      const collectedRoots: string[] = [];
      let first: string | null = null;

      for (const bundle of bundles) {
        try {
          const result = await loadNode(bundle.hash, 'root', null);
          if (!result) continue;

          setTreeNodes((prev) => {
            const next = { ...prev };
            const current = next[result.node.id];
            if (current) {
              next[result.node.id] = { ...current, name: bundle.name };
            }
            return next;
          });

          collectedRoots.push(result.node.id);
          if (!first) {
            first = result.node.childrenIds[0] ?? result.node.id;
          }
        } catch (error) {
          loadFailed = true;
          if (!ignore) {
            setTreeError(normalizeApiError(error));
          }
        }
      }

      const fileTabMetadata: Record<string, { nodeId: string; title: string }> = {};
      if (!isContextChange && !loadFailed) {
        for (const tab of tabsSnapshot) {
          if (tab.kind !== 'file') continue;
          const [tabBundleId, rawFileId] = tab.nodeId.includes(':')
            ? tab.nodeId.split(/:(.+)/)
            : [bundleId, tab.nodeId];
          const previousNode = treeNodesRef.current[tab.nodeId];
          try {
            const result = await loadNode(tabBundleId, rawFileId, previousNode?.parentId ?? null);
            const node = result?.node ?? null;
            if (node && !node.is_dir && !isArchiveNode(node)) {
              fileTabMetadata[tab.nodeId] = {
                nodeId: node.id,
                title: node.name
              };
            }
          } catch (error) {
            loadFailed = true;
            if (!ignore) {
              setTreeError(normalizeApiError(error));
            }
            break;
          }
        }
      }

      if (!ignore) {
        if (isContextChange) {
          setRootIds(collectedRoots);
          setExpandedNodes(new Set());
          setSelectedNodeId(first);
        } else if (!loadFailed) {
          setRootIds(collectedRoots);
          const reconciled = reconcileViewerTabs(
            tabsSnapshot,
            activeTabIdSnapshot,
            fileTabMetadata
          );
          setViewerTabsState(reconciled.tabs, reconciled.activeTabId);

          const activeTab = reconciled.tabs.find((tab) => tab.id === reconciled.activeTabId) ?? null;
          if (activeTab?.kind === 'file') {
            setSelectedNodeId(activeTab.nodeId);
            setLineStart(activeTab.lineStart);
            setLinePageSize(activeTab.pageSize);
            setTargetLine(activeTab.targetLine);
          } else if (tabsSnapshot.find((tab) => tab.id === activeTabIdSnapshot)?.kind === 'file') {
            setSelectedNodeId(null);
          } else if (!selectedNodeIdRef.current) {
            setSelectedNodeId(first);
          }
        } else if (!selectedNodeIdRef.current) {
          setSelectedNodeId(first);
        }
      }
      setTreeLoading(false);
    };

    init().catch(() => setTreeLoading(false));
    return () => {
      ignore = true;
    };
  }, [
    issueCodeFromRoute,
    locationState?.issue,
    bundleId,
    activeBundle.name,
    loadNode,
    refreshKey,
    resetViewerTabs,
    setViewerTabsState
  ]);

  useEffect(() => {
    searchRequestGenerationRef.current += 1;
    setFilenameQuery('');
    setSearchTokens([]);
    setSearchDraft('');
    setSearchResults([]);
    setSearchLoading(false);
    setSearchError(null);
    setSearchExecuted(false);
    setResultFilterTokens([]);
    setResultFilterDraft('');
  }, [issueCode]);

  const activeSearchResults = useMemo<IssueLogSearchHit[]>(() => {
    if (activeViewerTab?.kind === 'search') return activeViewerTab.hits;
    if (activeViewerTab?.kind === 'temp') {
      return activeViewerTab.lines.map((content, index) => ({
        file_id: activeViewerTab.resultId,
        path: '',
        snippet: content,
        line_number: activeViewerTab.from + index
      }));
    }
    return [];
  }, [activeViewerTab]);

  const handleNodeClick = async (
    nodeId: string,
    line?: number | null,
    options?: { preserveSearch?: boolean }
  ) => {
    if (typeof line === 'number' && line >= 0) {
      setTargetLine(line);
      setLineStart(Math.floor(line / linePageSize) * linePageSize);
    } else {
      setTargetLine(null);
      setLineStart(0);
    }
    if (!options?.preserveSearch) {
      setSearchResults([]);
      setSearchError(null);
      setSearchExecuted(false);
      setResultFilterTokens([]);
      setResultFilterDraft('');
    }
    let node: TreeNode | null = treeNodes[nodeId] ?? null;
    const [prefBundle, rawFromId] = nodeId.includes(':') ? nodeId.split(/:(.+)/) : [bundleId, nodeId];
    const bundleForNode = node?.bundleId || prefBundle || bundleId;
    if (!bundleForNode) return;
    if (!node) {
      const result = await loadNode(bundleForNode, rawFromId, null);
      node = result?.node ?? null;
    }
    if (!node) return;

    const canExpand = node.is_dir || isArchiveNode(node);
    if (canExpand) {
      if (!node.hasLoadedChildren) {
        await loadNode(bundleForNode, node.rawId, node.parentId);
      }
      setExpandedNodes((prev) => {
        const next = new Set(prev);
        if (next.has(node.id)) {
          next.delete(node.id);
        } else {
          next.add(node.id);
        }
        return next;
      });
    }

    setSelectedNodeId(node.id);
    if (!node.is_dir && !isArchiveNode(node)) {
      openViewerTab({
        id: `file:${node.id}`,
        kind: 'file',
        title: node.name,
        pinned: false,
        scrollTop: 0,
        nodeId: node.id,
        lineStart: typeof line === 'number' && line >= 0
          ? Math.floor(line / linePageSize) * linePageSize
          : 0,
        pageSize: linePageSize,
        targetLine: typeof line === 'number' && line >= 0 ? line : null
      });
    }
  };

  const openSearchHitSource = async (hit: IssueLogSearchHit) => {
    const source = getSearchHitSource(hit);
    if (!source) {
      setSourceActionMessage('来源文件信息不可用');
      return;
    }
    try {
      await handleNodeClick(source.nodeId, source.line, { preserveSearch: true });
      setSourceActionMessage(
        source.line === null ? '已打开文件，原始行号不可用' : null
      );
    } catch (error) {
      setSourceActionMessage(normalizeApiError(error));
    }
  };

  const copySearchHitPath = async (hit: IssueLogSearchHit) => {
    if (!hit.path) {
      setSourceActionMessage('文件路径不可用');
      return;
    }
    try {
      await navigator.clipboard.writeText(hit.path);
      setSourceActionMessage('已复制文件路径');
    } catch {
      setSourceActionMessage('复制文件路径失败');
    }
  };

  const activateViewerTabWithState = (tab: ViewerTab) => {
    if (activeViewerTabId && contentRef.current) {
      const scrollTop = contentRef.current.scrollTop;
      updateViewerTabs((tabs) =>
        tabs.map((item) => (item.id === activeViewerTabId ? { ...item, scrollTop } : item))
      );
    }
    activateViewerTab(tab);
    if (tab.kind === 'file') {
      setSelectedNodeId(tab.nodeId);
      setLineStart(tab.lineStart);
      setLinePageSize(tab.pageSize);
      setTargetLine(tab.targetLine);
    }
    window.requestAnimationFrame(() => {
      if (contentRef.current) contentRef.current.scrollTop = tab.scrollTop;
    });
  };

  const closeTab = (id: string) => {
    const index = viewerTabs.findIndex((tab) => tab.id === id);
    const remaining = viewerTabs.filter((tab) => tab.id !== id);
    closeViewerTab(id);
    if (activeViewerTabId === id) {
      const next = remaining[Math.min(index, remaining.length - 1)] ?? null;
      if (next?.kind === 'file') {
        setSelectedNodeId(next.nodeId);
        setLineStart(next.lineStart);
        setLinePageSize(next.pageSize);
        setTargetLine(next.targetLine);
      }
    }
  };

  useEffect(() => {
    if (viewerInitializedRef.current) return;
    if (!selectedNode || selectedNode.is_dir || isArchiveNode(selectedNode)) return;
    openViewerTab({
      id: `file:${selectedNode.id}`,
      kind: 'file',
      title: selectedNode.name,
      pinned: false,
      scrollTop: 0,
      nodeId: selectedNode.id,
      lineStart,
      pageSize: linePageSize,
      targetLine
    });
  }, [linePageSize, lineStart, openViewerTab, selectedNode, targetLine]);

  useEffect(() => {
    if (!activeViewerTab || activeViewerTab.kind !== 'file') return;
    updateViewerTabs((tabs) =>
      tabs.map((tab) =>
        tab.id === activeViewerTab.id && tab.kind === 'file'
          ? { ...tab, lineStart, pageSize: linePageSize, targetLine }
          : tab
      )
    );
  }, [activeViewerTab?.id, linePageSize, lineStart, targetLine, updateViewerTabs]);

  const clearFileSearch = useCallback(() => {
    setFileSearchTokens([]);
    setFileSearchDraft('');
    setFileSearchResults([]);
    setFileSearchTotal(0);
    setFileSearchFrom(0);
    setFileSearchError(null);
    setFileSearchExecuted(false);
  }, []);

  const runFileSearch = useCallback(async (from = 0) => {
    if (!selectedNode || !canPreviewText(selectedNode)) return;
    const selectedBundleId = selectedNode.bundleId || bundleId;
    if (!selectedBundleId) return;
    let finalizedTokens: SearchToken[];
    try {
      finalizedTokens = finalizeSearchTokens(fileSearchTokens, fileSearchDraft);
    } catch (error) {
      setFileSearchError(error instanceof Error ? error.message : '搜索条件无效');
      return;
    }
    const expression = serializeSearchTokens(finalizedTokens);
    const title = formatSearchTokens(finalizedTokens);
    setFileSearchTokens(finalizedTokens);
    setFileSearchDraft('');

    setFileSearchLoading(true);
    setFileSearchError(null);
    setFileSearchExecuted(true);
    try {
      const response = await rainApi.previewTempResult({
        expression,
        bundle_hash: selectedBundleId,
        file_id: selectedNode.rawId,
        from,
        size: LINE_PAGE_SIZE_OPTIONS[0]
      });
      const hits = response.lines.map((line) => ({
        bundle_hash: selectedBundleId,
        file_id: selectedNode.rawId,
        path: selectedNode.path,
        snippet: line.content,
        line_number: line.line_number,
        offset: line.line_number
      }));
      setFileSearchResults(hits);
      setFileSearchTotal(response.total);
      setFileSearchFrom(from);
      if (from === 0 && hits.length > 0) {
        const id = `search:${Date.now()}`;
        openViewerTab({
          id,
          kind: 'search',
          resultId: response.result_id,
          title,
          pinned: false,
          scrollTop: 0,
          expression,
          hits,
          total: response.total,
          from: 0,
          pageSize: LINE_PAGE_SIZE_OPTIONS[0],
          source: { kind: 'file', bundleHash: selectedBundleId, fileId: selectedNode.rawId }
        });
        setFileSearchResults([]);
        setFileSearchExecuted(false);
      }
    } catch (error) {
      setFileSearchResults([]);
      setFileSearchTotal(0);
      setFileSearchError(normalizeApiError(error));
    } finally {
      setFileSearchLoading(false);
    }
  }, [bundleId, fileSearchDraft, fileSearchTokens, openViewerTab, selectedNode]);

  const searchWithinActiveResults = useCallback(async () => {
    if (!activeViewerTab || (activeViewerTab.kind !== 'search' && activeViewerTab.kind !== 'temp')) return;
    let finalizedTokens: SearchToken[];
    try {
      finalizedTokens = finalizeSearchTokens(resultFilterTokens, resultFilterDraft);
    } catch (error) {
      setSearchError(error instanceof Error ? error.message : '搜索条件无效');
      return;
    }
    const nestedExpression = serializeSearchTokens(finalizedTokens);
    const title = formatSearchTokens(finalizedTokens);
    const expression = nestedExpression;
    const source = {
      kind: 'temp' as const,
      resultId: activeViewerTab.resultId
    };

    setSearchLoading(true);
    setSearchError(null);
    try {
      const payload = {
        expression,
        source_temp_id: source.resultId,
        from: 0,
        size: LINE_PAGE_SIZE_OPTIONS[0]
      };
      const response = await rainApi.previewTempResult(payload);
      const hits = response.lines.map((line) => ({
        bundle_hash: line.bundle_hash,
        file_id: line.file_id ?? '',
        path: line.path,
        snippet: line.content,
        line_number: line.line_number
      }));
      setResultFilterTokens([]);
      setResultFilterDraft('');
      openViewerTab({
        id: `search:${Date.now()}`,
        kind: 'search',
        resultId: response.result_id,
        title,
        pinned: false,
        scrollTop: 0,
        expression,
        hits,
        total: response.total,
        from: 0,
        pageSize: LINE_PAGE_SIZE_OPTIONS[0],
        source
      });
    } catch (error) {
      setSearchError(normalizeApiError(error));
    } finally {
      setSearchLoading(false);
    }
  }, [activeViewerTab, openViewerTab, resultFilterDraft, resultFilterTokens]);

  const loadViewerPage = useCallback(async (tab: ViewerTab, from: number, pageSize: number) => {
    setSearchLoading(true);
    setSearchError(null);
    try {
      if (tab.kind === 'temp') {
        const response = await rainApi.fetchTempResultLines(tab.resultId, {
          start: from,
          limit: pageSize
        });
        updateViewerTabs((tabs) => tabs.map((item) => item.id === tab.id && item.kind === 'temp'
          ? {
              ...item,
              lines: response.lines.map((line) => line.content),
              total: response.line_count,
              from: response.start,
              pageSize: response.limit,
              scrollTop: 0
            }
          : item));
        return;
      }
      if (tab.kind !== 'search') return;
      const response = await rainApi.fetchTempResultLines(tab.resultId, {
        start: from,
        limit: pageSize
      });
      const hits = response.lines.map((line) => ({
        bundle_hash: line.bundle_hash ?? undefined,
        file_id: line.file_id ?? '',
        path: line.path ?? '',
        snippet: line.content,
        line_number: line.line_number
      }));
      updateViewerTabs((tabs) => tabs.map((item) => item.id === tab.id && item.kind === 'search'
        ? {
            ...item,
            hits,
            total: response.line_count,
            from: response.start,
            pageSize: response.limit,
            scrollTop: 0
          }
        : item));
    } catch (error) {
      setSearchError(normalizeApiError(error));
    } finally {
      setSearchLoading(false);
    }
  }, [updateViewerTabs]);

  const activeIssueLabel = activeBundle.issue || '未知 Issue';

  useEffect(() => {
    clearFileSearch();
  }, [selectedNode?.id, clearFileSearch]);

  useEffect(() => {
    if (!selectedNode) return;
    if (!selectedNode.is_dir && !isArchiveNode(selectedNode)) return;
    if (selectedNode.hasLoadedChildren) return;
    loadNode(selectedNode.bundleId || bundleId, selectedNode.rawId, selectedNode.parentId).catch(() => undefined);
  }, [
    bundleId,
    selectedNode?.id,
    selectedNode?.is_dir,
    selectedNode?.parentId,
    selectedNode?.hasLoadedChildren,
    loadNode,
    selectedNode
  ]);

  useEffect(() => {
    if (selectedNodeId) return;
    if (rootIds.length === 0) return;
    const firstRoot = treeNodes[rootIds[0]];
    const resolveVisible = (nodeId: string | null): string | null => {
      if (!nodeId) return null;
      const node = treeNodes[nodeId];
      if (!node) return null;
      const parent = node.parentId ? treeNodes[node.parentId] : null;
      if (isExtractionFolder(node, parent)) {
        return resolveVisible(node.childrenIds[0] ?? null);
      }
      return nodeId;
    };
    const candidate = resolveVisible(firstRoot?.childrenIds[0] ?? rootIds[0]);
    if (candidate) {
      setSelectedNodeId(candidate);
    }
  }, [rootIds, treeNodes, selectedNodeId]);

  useEffect(() => {
    if (!selectedNode) return;
    if (!selectedNode.is_dir && !isArchiveNode(selectedNode)) return;
    setExpandedNodes((prev) => {
      if (prev.has(selectedNode.id)) return prev;
      const next = new Set(prev);
      next.add(selectedNode.id);
      return next;
    });
  }, [selectedNode]);

  useEffect(() => {
    if (!contentRef.current) return;
    if (targetLine === null || targetLine === undefined) return;
    if (!fileLines) return;
    const target = contentRef.current.querySelector<HTMLElement>(
      `[data-source-line="${targetLine}"]`
    );
    target?.scrollIntoView({ block: 'center' });
  }, [fileLines, targetLine]);

  useEffect(() => {
    if (!contentRef.current || !activeViewerTab || targetLine !== null) return;
    contentRef.current.scrollTop = activeViewerTab.scrollTop;
  }, [activeViewerTab?.id, activeViewerTab?.scrollTop, fileLines, targetLine]);

  const searchHighlightTerm = searchMode === 'log'
    ? filenameQuery.trim()
    : getSearchTerms(searchTokens)[0] ?? searchDraft.trim();
  const fileSearchHighlightTerm = getSearchTerms(fileSearchTokens)[0] ?? fileSearchDraft.trim();
  const resultFilterHighlightTerm = getSearchTerms(resultFilterTokens)[0] ?? resultFilterDraft.trim();
  const canRunSearch = searchMode === 'log'
    ? Boolean(filenameQuery.trim())
    : canFinalizeSearch(searchTokens, searchDraft);
  const showFilenameClear = searchMode === 'log' && shouldShowFilenameClear({
    query: filenameQuery,
    executed: searchExecuted,
    resultCount: searchResults.length,
    loading: searchLoading,
    error: searchError
  });
  const canRunFileSearch = canFinalizeSearch(fileSearchTokens, fileSearchDraft);
  const canRunResultFilter = canFinalizeSearch(resultFilterTokens, resultFilterDraft);

  return (
    <div className="space-y-5">
      <section className="panel overflow-hidden !p-0">
        {treeError ? (
          <p className="m-4 rounded-md border border-rose-200 bg-rose-50 px-3 py-2 text-sm text-rose-600">
            {treeError}
          </p>
        ) : null}

        <div className="grid min-h-[calc(100vh-104px)] gap-0 lg:grid-cols-[330px_minmax(0,1fr)]">
          <div className="flex min-h-0 flex-col border-r border-slate-200 bg-white">
            <div className="border-b border-slate-200 px-4 py-4">
              <div className="mb-4 flex items-start justify-between gap-3">
                <div className="min-w-0">
                  <p className="text-xs font-semibold text-slate-500">当前 Issue</p>
                  <p className="mt-1 truncate text-xl font-semibold leading-6 text-slate-950">{activeIssueLabel}</p>
                </div>
                <button
                  type="button"
                  className="rounded-md border border-slate-200 bg-white px-3 py-2 text-xs font-medium text-slate-600 shadow-sm shadow-slate-100 hover:border-slate-300 hover:text-slate-950"
                  title="刷新文件树"
                  onClick={() => setRefreshKey((key) => key + 1)}
                >
                  刷新
                </button>
              </div>
              <form
                className="flex min-h-11 items-start gap-2 rounded-md border border-slate-200 bg-white px-3 py-2 shadow-sm shadow-slate-100 focus-within:border-sky-400"
                onSubmit={handleSearchSubmit}
              >
                <span className="mt-1.5 shrink-0 text-slate-500" aria-hidden="true">⌕</span>
                {searchMode === 'log' ? (
                  <input
                    ref={filenameInputRef}
                    className="h-8 min-w-0 flex-1 bg-transparent px-1 text-sm text-slate-950 outline-none placeholder:text-slate-500"
                    aria-label="文件名搜索"
                    placeholder="搜索文件或目录..."
                    value={filenameQuery}
                    disabled={searchLoading}
                    onChange={(event) => setFilenameQuery(event.target.value)}
                  />
                ) : (
                  <SearchTokenEditor
                    tokens={searchTokens}
                    draft={searchDraft}
                    onTokensChange={setSearchTokens}
                    onDraftChange={setSearchDraft}
                    placeholder="输入完整关键词或短语..."
                    ariaLabel="日志内容搜索条件"
                    disabled={searchLoading}
                  />
                )}
                {showFilenameClear ? (
                  <button
                    type="button"
                    className="mt-0.5 shrink-0 rounded border border-slate-300 px-3 py-1.5 text-xs font-semibold text-slate-600 transition hover:border-slate-400 hover:text-slate-950"
                    aria-label="清除文件名搜索"
                    onClick={clearFilenameSearch}
                  >
                    清除
                  </button>
                ) : null}
                <button
                  type="submit"
                  className="mt-0.5 shrink-0 rounded bg-slate-200 px-3 py-1.5 text-xs font-semibold text-slate-900 transition hover:bg-slate-300 disabled:cursor-not-allowed disabled:opacity-50"
                  aria-label={searchMode === 'log' ? '搜索文件名' : '搜索日志内容'}
                  disabled={searchLoading || !issueCode || !canRunSearch}
                >
                  搜索
                </button>
              </form>
              <div className="mt-3 flex items-center gap-2 text-xs text-slate-500">
                <span className="shrink-0">搜索方式</span>
                <div className="flex rounded-md border border-slate-200 bg-slate-50 p-0.5 shadow-inner">
                  <button
                    type="button"
                    className={`rounded px-4 py-1.5 font-semibold transition ${searchMode === 'log' ? 'border border-sky-300 bg-white text-sky-700 shadow-sm' : 'text-slate-500 hover:text-slate-950'}`}
                    onClick={() => changeSearchMode('log')}
                  >
                    按文件名
                  </button>
                  <button
                    type="button"
                    className={`rounded px-4 py-1.5 font-semibold transition ${searchMode === 'detailed' ? 'border border-sky-300 bg-white text-sky-700 shadow-sm' : 'text-slate-500 hover:text-slate-950'}`}
                    onClick={() => changeSearchMode('detailed')}
                  >
                    搜日志内容
                  </button>
                </div>
              </div>
              {searchError ? <p className="mt-2 text-xs text-rose-600">{searchError}</p> : null}
            </div>
            <div className="min-h-0 flex-1 overflow-auto px-4 py-3">
            {nonReadyBundles.length > 0 ? (
              <div className="space-y-1 rounded-lg border border-slate-200 bg-slate-50 p-3 text-xs text-slate-600">
                {nonReadyBundles.map((bundle) => (
                  <div key={bundle.hash} className="flex items-center justify-between gap-3">
                    <span className="truncate">{bundle.name || bundle.hash}</span>
                    <span className={bundle.status.upload_status === 'FAILED' ? 'text-rose-600' : 'text-amber-700'}>
                      {bundleStatusLabel(bundle)}
                    </span>
                  </div>
                ))}
              </div>
            ) : null}
            {searchMode === 'log' && searchExecuted ? (
              searchLoading && searchResults.length === 0 ? (
                <p className="py-6 text-center text-sm text-slate-500">正在匹配文件名...</p>
              ) : searchResults.length === 0 ? (
                <p className="py-6 text-center text-sm text-slate-500">未找到匹配的日志文件。</p>
              ) : (
                <div className="space-y-1 text-sm text-slate-700">
                  <p className="px-2 pb-1 text-xs text-slate-500">找到 {searchResults.length} 个日志文件</p>
                  {searchResults.map((hit, index) => {
                    const targetId = hit.bundle_hash ? `${hit.bundle_hash}:${hit.file_id}` : '';
                    const selected = !!targetId && selectedNodeId === targetId;
                    return (
                      <button
                        key={`${hit.bundle_hash ?? 'b'}:${hit.file_id}:${index}`}
                        type="button"
                        className={`group flex h-10 w-full items-center gap-2 rounded border px-2 text-left transition ${
                          selected
                            ? 'border-sky-200 bg-sky-50 text-sky-700 shadow-[inset_3px_0_0_rgba(37,99,235,0.82)]'
                            : 'border-transparent text-slate-600 hover:bg-slate-100 hover:text-slate-950'
                        }`}
                        onClick={() => {
                          if (!targetId) return;
                          handleNodeClick(targetId, null, { preserveSearch: true }).catch(() => undefined);
                        }}
                      >
                        <span className="flex h-5 w-6 shrink-0 items-center justify-center rounded border border-slate-200 bg-white text-[10px] font-semibold text-slate-600 shadow-sm shadow-slate-100">
                          □
                        </span>
                        <div className="min-w-0 flex-1">
                          <p className="truncate text-[13px] font-medium leading-4">
                            {highlightText(hit.snippet, searchHighlightTerm)}
                          </p>
                          <p className="truncate text-[10px] leading-3 text-slate-500">
                            {highlightText(formatHitPath(hit.path), searchHighlightTerm)}
                          </p>
                        </div>
                      </button>
                    );
                  })}
                </div>
              )
            ) : rootIds.length > 0 ? (
              <div className="space-y-2 text-sm text-slate-700">
                {rootIds.some((rootId) => (treeNodes[rootId]?.childrenIds.length ?? 0) > 0) ? (
                  rootIds.map((rootId) => (
                    <div key={rootId} className="space-y-1">
                      {(treeNodes[rootId]?.childrenIds ?? []).map((childId) => {
                        const topNode = treeNodes[childId];
                        if (!topNode) return null;
                        return (
                          <FileTreeNode
                            key={childId}
                            nodeId={childId}
                            treeNodes={treeNodes}
                            expandedNodes={expandedNodes}
                            selectedNodeId={selectedNodeId}
                            onNodeClick={(nodeId) => {
                              handleNodeClick(nodeId).catch(() => undefined);
                            }}
                          />
                        );
                      })}
                    </div>
                  ))
                ) : (
                  <p className="text-sm text-slate-500">暂无文件。</p>
                )}
              </div>
            ) : treeLoading ? (
              <p className="text-sm text-slate-500">文件树加载中...</p>
            ) : (
              <p className="text-sm text-slate-500">选择左侧 Issue / Bundle 后自动加载文件树。</p>
            )}
            </div>
          </div>

          <div className="flex min-h-[calc(100vh-104px)] flex-col bg-slate-50 text-sm text-slate-700">
            <ViewerTabBar
              tabs={viewerTabs}
              activeTabId={activeViewerTabId}
              onActivate={activateViewerTabWithState}
              onTogglePinned={togglePinnedViewerTab}
              onClose={closeTab}
            />
            <p
              aria-live="polite"
              className={`px-4 py-1 text-xs ${sourceActionMessage ? 'text-slate-600' : 'sr-only'}`}
            >
              {sourceActionMessage ?? ''}
            </p>
            <div className="flex min-h-0 flex-1 flex-col p-4">
              <div className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-md border border-slate-200 bg-white shadow-sm shadow-slate-100">
                {activeViewerTab?.kind === 'file' && selectedNode &&
                canPreviewText(selectedNode) ? (
                  <div className="flex min-h-14 flex-wrap items-center gap-3 border-b border-slate-200 bg-white px-4 py-3 focus-within:border-sky-400">
                    <span className="mt-1.5 shrink-0 self-start text-slate-500" aria-hidden="true">⌕</span>
                    <SearchTokenEditor
                      className="min-w-[220px]"
                      tokens={fileSearchTokens}
                      draft={fileSearchDraft}
                      onTokensChange={setFileSearchTokens}
                      onDraftChange={setFileSearchDraft}
                      placeholder="输入当前文件的关键词或短语..."
                      ariaLabel="当前文件搜索条件"
                      disabled={fileSearchLoading}
                    />
                    {fileSearchExecuted ? (
                      <span className="shrink-0 text-xs text-slate-500">{fileSearchTotal} 个结果</span>
                    ) : null}
                    {fileSearchTokens.length > 0 || fileSearchDraft ? (
                      <button
                        type="button"
                        className="shrink-0 rounded border border-transparent px-2 py-1 text-xs text-slate-500 transition hover:border-slate-300 hover:text-slate-950"
                        onClick={clearFileSearch}
                      >
                        清空
                      </button>
                    ) : null}
                    <button
                      type="button"
                      className="shrink-0 rounded border border-slate-300 bg-white px-3 py-1.5 text-xs font-semibold text-slate-700 transition hover:border-slate-500 disabled:cursor-not-allowed disabled:opacity-50"
                      disabled={fileSearchLoading || !canRunFileSearch}
                      onClick={() => runFileSearch(0).catch(() => undefined)}
                    >
                      搜索
                    </button>
                  </div>
                ) : null}

                {activeViewerTab?.kind === 'file' && fileSearchExecuted ? (
                  fileSearchLoading && fileSearchResults.length === 0 ? (
                    <p className="py-8 text-center text-sm text-slate-500">正在搜索当前文件...</p>
                  ) : fileSearchError ? (
                    <p className="py-8 text-center text-sm text-rose-600">{fileSearchError}</p>
                  ) : fileSearchResults.length === 0 ? (
                    <p className="py-8 text-center text-sm text-slate-500">当前文件中没有相关日志。</p>
                  ) : (
                    <div className="flex min-h-0 flex-1 flex-col gap-2">
                      <div className="min-h-0 flex-1 space-y-2 overflow-auto">
                        {fileSearchResults.map((hit, index) => (
                          <button
                            key={`${hit.file_id}:${hit.offset ?? hit.line_number ?? index}:${index}`}
                            type="button"
                            className="w-full space-y-1 rounded-md border border-slate-200 bg-white p-3 text-left transition hover:border-sky-200 hover:bg-sky-50/40"
                            onClick={() => {
                              const line = hit.line_number ?? hit.offset ?? null;
                              clearFileSearch();
                              handleNodeClick(selectedNodeId || '', line, { preserveSearch: true }).catch(() => undefined);
                            }}
                          >
                            <div className="flex items-center justify-between gap-3 text-[11px] text-slate-500">
                              <span className="truncate">{formatHitPath(hit.path)}</span>
                              <span className="shrink-0">
                                {hit.line_number !== undefined || hit.offset !== undefined
                                  ? `行 ${(hit.line_number ?? hit.offset ?? 0) + 1}`
                                  : '行号未知'}
                              </span>
                            </div>
                            <pre className="truncate font-mono text-xs text-slate-900">
                              {highlightText(hit.snippet, fileSearchHighlightTerm)}
                            </pre>
                          </button>
                        ))}
                      </div>
                      <div className="flex items-center justify-between text-xs text-slate-500">
                        <span>
                          {fileSearchFrom + 1} - {Math.min(fileSearchFrom + fileSearchResults.length, fileSearchTotal)} / {fileSearchTotal}
                        </span>
                        <div className="flex gap-2">
                          <button
                            type="button"
                            className="rounded border border-slate-300 px-3 py-1 hover:border-slate-500 disabled:opacity-50"
                            disabled={fileSearchFrom === 0 || fileSearchLoading}
                            onClick={() => runFileSearch(Math.max(0, fileSearchFrom - 50)).catch(() => undefined)}
                          >
                            上一页
                          </button>
                          <button
                            type="button"
                            className="rounded border border-slate-300 px-3 py-1 hover:border-slate-500 disabled:opacity-50"
                            disabled={fileSearchFrom + fileSearchResults.length >= fileSearchTotal || fileSearchLoading}
                            onClick={() => runFileSearch(fileSearchFrom + 50).catch(() => undefined)}
                          >
                            下一页
                          </button>
                        </div>
                      </div>
                    </div>
                  )
                ) : activeViewerTab?.kind === 'search' || activeViewerTab?.kind === 'temp' ? (
                  <SearchResultViewer
                    activeViewerTab={activeViewerTab}
                    results={activeSearchResults}
                    resultFilterTokens={resultFilterTokens}
                    resultFilterDraft={resultFilterDraft}
                    onResultFilterTokensChange={setResultFilterTokens}
                    onResultFilterDraftChange={setResultFilterDraft}
                    onClearResultFilter={() => {
                      setResultFilterTokens([]);
                      setResultFilterDraft('');
                    }}
                    onSearchWithinResults={() => searchWithinActiveResults().catch(() => undefined)}
                    canRunResultFilter={canRunResultFilter}
                    searchLoading={searchLoading}
                    contentRef={contentRef}
                    pageSizeOptions={LINE_PAGE_SIZE_OPTIONS}
                    onLoadPage={(tab, from, pageSize) => {
                      loadViewerPage(tab, from, pageSize).catch(() => undefined);
                    }}
                    highlightTerm={resultFilterHighlightTerm}
                    renderHighlightedText={highlightText}
                    onOpenSource={openSearchHitSource}
                    onCopySourcePath={copySearchHitPath}
                  />
                ) : activeViewerTab?.kind !== 'file' || !selectedNode ? (
                  <p className="py-8 text-center text-sm text-slate-500">
                    输入关键词搜索当前 Issue 的日志。
                  </p>
                ) : isArchiveNode(selectedNode) ? (
                  <p className="text-sm text-slate-500">压缩包请在左侧展开查看内部文件。</p>
                ) : selectedNode.is_dir ? (
                  <p className="text-sm text-slate-500">当前为目录，选择文件后展示内容。</p>
                ) : isBinaryNode(selectedNode) ? (
                  <BinaryFileInfo
                    node={selectedNode}
                  />
                ) : fileContentLoading ? (
                  <p className="text-sm text-slate-500">读取中...</p>
                ) : fileContentError ? (
                  <p className="text-sm text-rose-600">{fileContentError}</p>
                ) : fileLines ? (
                  <div className="flex min-h-0 flex-1 flex-col gap-2">
                    <CodeLinesPane
                      lines={fileLines.lines}
                      contentRef={contentRef}
                      lineNumberOffset={fileLines.start}
                      targetLine={targetLine}
                    />
                    <div className="flex flex-wrap items-center justify-end gap-2 border-t border-slate-200 bg-slate-50 px-4 py-2 text-xs text-slate-500">
                      <label className="flex items-center gap-2">
                        <span>每页</span>
                        <select
                          className="rounded border border-slate-300 bg-white px-2 py-1 text-slate-700 outline-none focus:border-cyan-500/60"
                          value={linePageSize}
                          onChange={(event) => {
                            setLinePageSize(Number(event.target.value));
                            setLineStart(0);
                            setTargetLine(null);
                          }}
                        >
                          {LINE_PAGE_SIZE_OPTIONS.map((size) => (
                            <option key={size} value={size}>{size} 行</option>
                          ))}
                        </select>
                      </label>
                      <span className="min-w-[86px] text-center">
                        第 {Math.floor(fileLines.start / linePageSize) + 1}
                        {fileLines.line_count
                          ? ` / ${Math.max(1, Math.ceil(fileLines.line_count / linePageSize))} 页`
                          : ' 页'}
                      </span>
                      <button
                        type="button"
                        className="rounded border border-slate-300 px-3 py-1 text-slate-600 hover:border-slate-500 disabled:opacity-50"
                        disabled={lineStart <= 0 || fileContentLoading}
                        onClick={() => setLineStart(Math.max(0, lineStart - linePageSize))}
                      >
                        上一页
                      </button>
                      <button
                        type="button"
                        className="rounded border border-slate-300 px-3 py-1 text-slate-600 hover:border-slate-500 disabled:opacity-50"
                        disabled={!fileLines.next_start || fileContentLoading}
                        onClick={() => setLineStart(fileLines.next_start ?? lineStart + linePageSize)}
                      >
                        下一页
                      </button>
                    </div>
                  </div>
                ) : (
                  <p className="text-sm text-slate-500">选择文件即可加载内容。</p>
                )}
              </div>
            </div>
          </div>
        </div>
      </section>
    </div>
  );
}
