import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useLocation, useNavigate, useParams } from 'react-router-dom';
import { ApiError, normalizeApiError, rainApi } from '../../api/client';
import type { IssueLogSearchHit, LogSearchHit, SavedSearch, SavedSearchPayload, UploadSummary } from '../../api/types';
import { useAuth } from '../../auth/AuthContext';
import type { BundleInfo } from '../../lib/bundles';
import { BinaryFileInfo } from './BinaryFileInfo';
import { SearchTokenEditor } from './SearchTokenEditor';
import { SearchExpressionEditor } from './SearchExpressionEditor';
import { canPreviewText, isArchiveNode, isBinaryNode } from './filePresentation';
import { isFileSearchConditionEmpty } from './fileSearchState';
import { getSearchHitSource } from './searchHitSource';
import { LINE_PAGE_SIZE_OPTIONS } from './linePageSizes';
import { uploadFailureMessage } from './uploadFailure';
import {
  canFinalizeSearch,
  deserializeSearchTokens,
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
  attachTreeChild,
  hydrateTreeNode,
  isExtractionFolder,
  mergeFlattenedExtractionChildren,
  toTreeNode,
  type TreeNode
} from './treeModel';
import { useViewerTabs } from './hooks/useViewerTabs';
import { useFileContent } from './hooks/useFileContent';
import {
  createFileContentCache,
  type FileContentRequestKey
} from './fileContentCache';
import { centerElementInScrollContainer } from './centerElementInScrollContainer';
import { ViewerTabBar } from './components/ViewerTabBar';
import { CodeLinesPane } from './components/CodeLinesPane';
import { FileTreeNode } from './components/FileTreeNode';
import { SearchResultViewer } from './components/SearchResultViewer';
import { SearchExecutionStatus } from '../../components/SearchExecutionStatus';
import { useSearchExecution } from '../../hooks/useSearchExecution';
import { PENDING_SAVED_SEARCH_KEY, takePendingSavedSearch } from './pendingSavedSearch';

const bundleStatusLabel = (bundle: UploadSummary) => {
  if (bundle.status.upload_status === 'PROCESSING' || bundle.status.upload_status === 'PENDING') {
    if (bundle.stage === 'RECEIVING') return '正在接收文件';
    if (bundle.stage === 'VALIDATING') return '正在校验压缩内容';
    if (bundle.stage === 'EXTRACTING') return '正在解压';
    if (bundle.stage === 'INDEXING') return '正在建立索引';
    if (bundle.stage === 'PUBLISHING') return '正在发布';
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

const FILE_TREE_PAGE_SIZE = 100;

type TreeLoadGuard = {
  contextKey: string;
  generation: number;
  isCurrent: () => boolean;
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

function detailEditorState(queryText: string | undefined, options?: Record<string, unknown>): {
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

export function BundleView() {
  const auth = useAuth();
  const navigate = useNavigate();
  const params = useParams<{ issueCode?: string; bundleHash?: string }>();
  const bundleHash = params.bundleHash || '';
  const issueCodeFromRoute = params.issueCode;
  const location = useLocation();
  const locationState = location.state as { issue?: string; bundleName?: string } | null;
  const issueCode = issueCodeFromRoute || locationState?.issue || '';
  const [pendingSavedSearch] = useState(() => takePendingSavedSearch(
    sessionStorage,
    auth.state.status === 'AUTHENTICATED'
  ));
  const [pendingDetailEditor] = useState(() => detailEditorState(
    pendingSavedSearch?.search_type === 'DETAIL' ? pendingSavedSearch.query_text : undefined,
    pendingSavedSearch?.search_type === 'DETAIL' ? pendingSavedSearch.options : undefined
  ));

  const activeBundle: BundleInfo = {
    hash: bundleHash,
    name: locationState?.bundleName || bundleHash,
    issue: issueCode
  };

  const bundleId = activeBundle.hash || '';
  const hasFileContext = Boolean(issueCode || bundleId);
  const [rootIds, setRootIds] = useState<string[]>([]);
  const [treeNodes, setTreeNodes] = useState<Record<string, TreeNode>>({});
  const [expandedNodes, setExpandedNodes] = useState<Set<string>>(new Set());
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [treeLoading, setTreeLoading] = useState(false);
  const [treeError, setTreeError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const [searchTokens, setSearchTokens] = useState<SearchToken[]>(
    pendingDetailEditor.tokens
  );
  const [searchDraft, setSearchDraft] = useState('');
  const [searchLoading, setSearchLoading] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(pendingDetailEditor.error);
  const [searchExecuted, setSearchExecuted] = useState(false);
  const [savedSearches, setSavedSearches] = useState<SavedSearch[]>([]);
  const [savedSearchesOpen, setSavedSearchesOpen] = useState(false);
  const [saveDialogOpen, setSaveDialogOpen] = useState(Boolean(pendingSavedSearch));
  const [savedSearchName, setSavedSearchName] = useState('');
  const [savedSearchError, setSavedSearchError] = useState('');
  const [editingSavedSearch, setEditingSavedSearch] = useState<SavedSearch | null>(null);
  const [editingSearchTokens, setEditingSearchTokens] = useState<SearchToken[]>([]);
  const [editingSearchDraft, setEditingSearchDraft] = useState('');
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
  const [lineStart, setLineStart] = useState(0);
  const [linePageSize, setLinePageSize] = useState<number>(LINE_PAGE_SIZE_OPTIONS[0]);
  const [targetLine, setTargetLine] = useState<number | null>(null);
  const [nonReadyBundles, setNonReadyBundles] = useState<UploadSummary[]>([]);
  const [sourceActionMessage, setSourceActionMessage] = useState<string | null>(null);
  const contentRef = useRef<HTMLDivElement | null>(null);
  const fileTreeContainerRef = useRef<HTMLDivElement | null>(null);
  const searchRequestGenerationRef = useRef(0);
  const saveDialogGenerationRef = useRef(0);
  const editingSaveGenerationRef = useRef(0);
  const restoredPendingSearchRef = useRef(Boolean(pendingSavedSearch));
  const contextKeyRef = useRef<string | null>(null);
  const refreshGenerationRef = useRef(0);
  const viewerTabsRef = useRef<ViewerTab[]>([]);
  const activeViewerTabIdRef = useRef<string | null>(null);
  const selectedNodeIdRef = useRef<string | null>(null);
  const treeNodesRef = useRef<Record<string, TreeNode>>({});
  const pendingFilePageRef = useRef<{
    tabId: string;
    from: number;
    navigation: 'next' | 'previous' | 'reset';
    previousStart?: number;
  } | null>(null);
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
  } = useViewerTabs(auth.state.status === 'AUTHENTICATED' && auth.state.user.role === 'USER');
  const issueSearchExecution = useSearchExecution();
  const fileSearchExecution = useSearchExecution();
  const viewerSearchExecution = useSearchExecution();
  useEffect(() => {
    const status = issueSearchExecution.snapshot.status;
    if (status === 'RUNNING' || status === 'CANCELLING' || status === 'CANCELLED' || status === 'SUCCEEDED') {
      setSearchError(null);
    }
    if (status === 'CANCELLED' || status === 'FAILED' || status === 'SUCCEEDED') {
      setSearchLoading(false);
    }
  }, [issueSearchExecution.snapshot.status]);
  useEffect(() => {
    if (fileSearchExecution.snapshot.status === 'FAILED') {
      setFileSearchError(fileSearchExecution.snapshot.errorMessage);
    }
    if (fileSearchExecution.snapshot.status === 'CANCELLED'
      || fileSearchExecution.snapshot.status === 'FAILED'
      || fileSearchExecution.snapshot.status === 'SUCCEEDED') {
      setFileSearchLoading(false);
    }
  }, [fileSearchExecution.snapshot.errorMessage, fileSearchExecution.snapshot.status]);
  useEffect(() => {
    const status = viewerSearchExecution.snapshot.status;
    if (status === 'RUNNING' || status === 'CANCELLING' || status === 'CANCELLED' || status === 'SUCCEEDED') {
      setSearchError(null);
    }
    if (status === 'CANCELLED' || status === 'FAILED' || status === 'SUCCEEDED') {
      setSearchLoading(false);
    }
  }, [viewerSearchExecution.snapshot.status]);
  const selectedNode = selectedNodeId ? treeNodes[selectedNodeId] : null;
  const fileContextKey = `${issueCode}\u0000${bundleId}`;
  const fileContentCache = useMemo(() => createFileContentCache(
    (request: FileContentRequestKey, signal: AbortSignal) => rainApi.fetchFileLines(
      request.bundle,
      request.file,
      { start: request.start, limit: request.requestedLimit, signal }
    ),
    {
      estimateBytes: (response) => 256
        + response.path.length * 2
        + response.lines.reduce((sum, line) => sum + 64 + line.content.length * 2, 0)
    }
  ), [fileContextKey]);
  useEffect(() => () => fileContentCache.reset(), [fileContentCache]);
  const activeFileNode = activeViewerTab?.kind === 'file'
    ? treeNodes[activeViewerTab.nodeId] ?? null
    : null;
  const activeFileRequest = useMemo<FileContentRequestKey | null>(() => {
    if (activeViewerTab?.kind !== 'file' || !activeFileNode || !canPreviewText(activeFileNode)) return null;
    return {
      bundle: activeFileNode.bundleId || bundleId,
      file: activeFileNode.rawId,
      context: fileContextKey,
      open: activeViewerTab.id,
      start: activeViewerTab.lineStart,
      requestedLimit: activeViewerTab.pageSize
    };
  }, [activeFileNode, activeViewerTab, bundleId, fileContextKey]);
  const {
    fileLines,
    fileContentLoading,
    fileContentError,
    retryFileContent
  } = useFileContent({
    cache: fileContentCache,
    owner: activeViewerTab?.kind === 'file' ? activeViewerTab.id : null,
    request: activeFileRequest
  });

  const currentSavedSearchPayload = useCallback((): SavedSearchPayload | null => {
    try {
      const tokens = finalizeSearchTokens(searchTokens, searchDraft);
      return {
        name: savedSearchName,
        search_type: 'DETAIL',
        query_text: serializeSearchTokens(tokens),
        options: { version: 1 }
      };
    } catch {
      return null;
    }
  }, [savedSearchName, searchDraft, searchTokens]);

  const loadSavedSearches = useCallback(async () => {
    if (auth.state.status !== 'AUTHENTICATED') return;
    const items = await rainApi.fetchSavedSearches();
    setSavedSearches(items.filter((item) => item.search_type === 'DETAIL'));
  }, [auth.state.status]);

  useEffect(() => {
    if (auth.state.status !== 'AUTHENTICATED') {
      setSavedSearches([]);
      return;
    }
    void loadSavedSearches().catch((error) => setSavedSearchError(normalizeApiError(error)));
    if (!pendingSavedSearch) {
      const pending = takePendingSavedSearch(sessionStorage, true);
      if (pending) {
        const editor = detailEditorState(pending.query_text, pending.options);
        setSearchTokens(editor.tokens);
        setSearchDraft('');
        if (editor.error) setSearchError(editor.error);
        setSaveDialogOpen(true);
      }
    }
  }, [auth.state.status, issueCode, loadSavedSearches, pendingSavedSearch]);

  const beginSaveSearch = () => {
    const payload = currentSavedSearchPayload();
    if (!payload) {
      setSavedSearchError('请先输入有效搜索条件');
      return;
    }
    if (auth.state.status !== 'AUTHENTICATED') {
      sessionStorage.setItem(PENDING_SAVED_SEARCH_KEY, JSON.stringify(payload));
      navigate('/login', { state: { from: `${location.pathname}${location.search}` } });
      return;
    }
    saveDialogGenerationRef.current += 1;
    setSavedSearchError('');
    setSaveDialogOpen(true);
  };

  const saveSearch = async () => {
    const saveGeneration = saveDialogGenerationRef.current;
    const payload = currentSavedSearchPayload();
    if (!payload || !savedSearchName.trim()) {
      setSavedSearchError('请输入名称并确认搜索条件有效');
      return;
    }
    try {
      await rainApi.validateSearchExpression(payload.query_text);
      if (saveGeneration !== saveDialogGenerationRef.current) return;
      await rainApi.createSavedSearch({ ...payload, name: savedSearchName.trim() });
      if (saveGeneration !== saveDialogGenerationRef.current) return;
      setSavedSearchName('');
      setSaveDialogOpen(false);
      await loadSavedSearches();
    } catch (error) {
      setSavedSearchError(normalizeApiError(error));
    }
  };

  const useSavedSearch = async (item: SavedSearch) => {
    const requestGeneration = ++searchRequestGenerationRef.current;
    const isCurrentSearch = () => requestGeneration === searchRequestGenerationRef.current;
    const editor = detailEditorState(item.query_text, item.options);
    if (editor.error) {
      setSearchError(editor.error);
      return;
    }
    try {
      await rainApi.validateSearchExpression(item.query_text);
    } catch (error) {
      if (isCurrentSearch()) setSearchError(normalizeApiError(error));
      return;
    }
    if (!isCurrentSearch()) return;
    setSearchTokens(editor.tokens);
    setSearchDraft('');
    setSearchError(null);
    setSearchLoading(true);
    const response = await issueSearchExecution.execute(
      { expression: item.query_text, issue_code: issueCode, from: 0, size: LINE_PAGE_SIZE_OPTIONS[0] },
      {
        scopeKey: `issue:${issueCode}`,
        onSuccess: (response) => {
          if (!isCurrentSearch()) return;

          const hits = response.lines.map((line) => ({
            bundle_hash: line.bundle_hash,
            file_id: line.file_id ?? '',
            path: line.path,
            snippet: line.content,
            line_number: line.line_number
          }));
          setSearchExecuted(true);
          openViewerTab({
            id: `search:${Date.now()}`,
            kind: 'search',
            resultId: response.result_id,
            title: item.name,
            pinned: false,
            scrollTop: 0,
            expression: item.query_text,
            hits,
            total: response.total,
            from: 0,
            pageSize: LINE_PAGE_SIZE_OPTIONS[0],
            pageHistory: [],
            source: { kind: 'issue', issueCode }
          });
        }
      }
    );
    if (!response || !isCurrentSearch()) return;
    await rainApi.markSavedSearchUsed(item.id);
    if (isCurrentSearch()) setSavedSearchesOpen(false);
  };

  const updateEditingSavedSearch = async () => {
    if (!editingSavedSearch) return;
    const saveGeneration = editingSaveGenerationRef.current;
    try {
      const finalizedTokens = finalizeSearchTokens(editingSearchTokens, editingSearchDraft);
      const queryText = serializeSearchTokens(finalizedTokens);
      await rainApi.validateSearchExpression(queryText);
      if (saveGeneration !== editingSaveGenerationRef.current) return;
      await rainApi.updateSavedSearch(editingSavedSearch.id, {
        name: editingSavedSearch.name.trim(),
        search_type: 'DETAIL',
        query_text: queryText,
        options: { version: 1 },
        is_pinned: editingSavedSearch.is_pinned
      });
      if (saveGeneration !== editingSaveGenerationRef.current) return;
      setEditingSavedSearch(null);
      await loadSavedSearches();
    } catch (error) {
      setSavedSearchError(normalizeApiError(error));
    }
  };

  const beginEditingSavedSearch = (item: SavedSearch) => {
    const editor = detailEditorState(item.query_text, item.options);
    setEditingSearchTokens(editor.tokens);
    setEditingSearchDraft('');
    setSavedSearchError(editor.error ?? '');
    editingSaveGenerationRef.current += 1;
    setEditingSavedSearch({ ...item });
  };

  useEffect(() => {
    viewerTabsRef.current = viewerTabs;
  }, [viewerTabs]);

  useEffect(() => {
    activeViewerTabIdRef.current = activeViewerTabId;
  }, [activeViewerTabId]);

  useEffect(() => {
    void viewerSearchExecution.cancel();
  }, [activeViewerTab?.id]);

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
      parentId: string | null = null,
      guard?: TreeLoadGuard
    ): Promise<{ node: TreeNode; children: TreeNode[] } | null> => {
      if (!bundle) return null;
      const canCommit = () => !guard || (
        guard.generation === refreshGenerationRef.current
        && contextKeyRef.current === guard.contextKey
        && guard.isCurrent()
      );
      if (canCommit()) {
        setTreeLoading(true);
        setTreeError(null);
      }
      try {
        const result = await hydrateTreeNode(
          bundle,
          nodeId,
          parentId ?? null,
          (childId, options) => rainApi.fetchFileNode(bundle, childId, options),
          FILE_TREE_PAGE_SIZE
        );
        const { node: normalized, children: childrenNodes } = result;
        if (normalized.childrenLoadError && canCommit()) {
          setTreeError(normalized.childrenLoadError);
        }

        if (canCommit()) {
          setTreeNodes((prev) => {
            const next = { ...prev };
            next[normalized.id] = normalized;
            childrenNodes.forEach((child) => {
              next[child.id] = child;
            });
            return next;
          });
        }

        return { node: normalized, children: childrenNodes };
      } catch (error) {
        if (canCommit()) {
          setTreeError(normalizeApiError(error));
        }
        throw error;
      } finally {
        if (canCommit()) {
          setTreeLoading(false);
        }
      }
    },
    []
  );

  const loadMoreNode = useCallback(
    async (node: TreeNode): Promise<{ node: TreeNode; children: TreeNode[] } | null> => {
      if (!node.hasMoreChildren || !node.childrenCursor) return null;
      setTreeLoading(true);
      setTreeError(null);
      try {
        const response = await rainApi.fetchFileNode(node.bundleId, node.childrenSourceId, {
          cursor: node.childrenCursor,
          limit: FILE_TREE_PAGE_SIZE
        });
        const childrenNodes = (response.children ?? []).map((child) =>
          toTreeNode(node.bundleId, child, node.id)
        );
        const nextNode: TreeNode = {
          ...node,
          childrenIds: [...new Set([...node.childrenIds, ...childrenNodes.map((child) => child.id)])],
          hasMoreChildren: response.has_more === true,
          childrenCursor: response.next_cursor ?? null
        };
        setTreeNodes((prev) => {
          const current = prev[node.id];
          if (!current) return prev;
          const next = { ...prev };
          next[node.id] = {
            ...current,
            childrenIds: nextNode.childrenIds,
            hasMoreChildren: nextNode.hasMoreChildren,
            childrenCursor: nextNode.childrenCursor
          };
          childrenNodes.forEach((child) => {
            next[child.id] = child;
          });
          return next;
        });
        return { node: nextNode, children: childrenNodes };
      } catch (error) {
        setTreeError(normalizeApiError(error));
        return null;
      } finally {
        setTreeLoading(false);
      }
    },
    []
  );

  const runSearch = useCallback(async () => {
    const requestGeneration = ++searchRequestGenerationRef.current;
    const isCurrentSearch = () => requestGeneration === searchRequestGenerationRef.current;
    const issue = issueCode;
    if (!issue) {
      setSearchError(null);
      setSearchExecuted(false);
      setResultFilterTokens([]);
      setResultFilterDraft('');
      return;
    }
    let keyword: string;
    let title: string;
    let finalizedTokens: SearchToken[];
    try {
      finalizedTokens = finalizeSearchTokens(searchTokens, searchDraft);
      keyword = serializeSearchTokens(finalizedTokens);
      title = formatSearchTokens(finalizedTokens);
    } catch (error) {
      setSearchError(error instanceof Error ? error.message : '搜索条件无效');
      return;
    }
    try {
      await rainApi.validateSearchExpression(keyword);
    } catch (error) {
      if (isCurrentSearch()) setSearchError(normalizeApiError(error));
      return;
    }
    if (!isCurrentSearch()) return;
    setSearchTokens(finalizedTokens);
    setSearchDraft('');
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
      const response = await issueSearchExecution.execute(
        {
          expression: keyword,
          issue_code: issue,
          from: 0,
          size: LINE_PAGE_SIZE_OPTIONS[0]
        },
        {
          scopeKey: `issue:${issue}`,
          onSuccess: (response) => {
            if (requestGeneration !== searchRequestGenerationRef.current) return;
            const hits = response.lines.map((line) => ({
              bundle_hash: line.bundle_hash,
              file_id: line.file_id ?? '',
              path: line.path,
              snippet: line.content,
              line_number: line.line_number
            }));
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
              pageHistory: [],
              source: { kind: 'issue', issueCode: issue }
            });
          }
        }
      );
      if (!response) return;
    } catch (error) {
      if (requestGeneration !== searchRequestGenerationRef.current) return;
      setSearchError(normalizeApiError(error));
    } finally {
      if (requestGeneration === searchRequestGenerationRef.current) {
        setSearchLoading(false);
      }
    }
  }, [issueCode, openViewerTab, searchDraft, searchTokens]);

  const clearDetailedSearch = useCallback(() => {
    searchRequestGenerationRef.current += 1;
    void issueSearchExecution.cancel();
    setSearchTokens([]);
    setSearchDraft('');
    setSearchLoading(false);
    setSearchError(null);
    setSearchExecuted(false);
    setResultFilterTokens([]);
    setResultFilterDraft('');
  }, [issueSearchExecution]);

  const handleSearchSubmit = (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    void runSearch();
  };

  useEffect(() => {
    const issueCode = issueCodeFromRoute || locationState?.issue || '';
    const fallbackBundles = bundleId ? [{ hash: bundleId, name: activeBundle.name }] : [];
    const contextKey = `${issueCode}\u0000${bundleId}`;
    const isContextChange = contextKeyRef.current !== contextKey;
    contextKeyRef.current = contextKey;

    let ignore = false;
    const refreshGeneration = ++refreshGenerationRef.current;
    const isCurrentRefresh = () => !ignore
      && refreshGeneration === refreshGenerationRef.current
      && contextKeyRef.current === contextKey;
    const refreshGuard: TreeLoadGuard = { contextKey, generation: refreshGeneration, isCurrent: isCurrentRefresh };
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
        searchRequestGenerationRef.current += 1;
        void issueSearchExecution.cancel();
        void fileSearchExecution.cancel();
        void viewerSearchExecution.cancel();
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
          if (isCurrentRefresh()) {
            setNonReadyBundles(notReady);
          }
          const list = data.log_bundles
            .filter((bundle) => bundle.status.upload_status === 'READY')
            .map((bundle) => ({ hash: bundle.hash, name: bundle.name || bundle.hash }));
          bundles = list;
        } catch (error) {
          loadFailed = true;
          if (error instanceof ApiError && (error.code === 'RESOURCE_NOT_FOUND' || error.status === 404)) {
            if (isCurrentRefresh()) {
              navigate('/', { replace: true });
            }
            return;
          }
          if (isCurrentRefresh()) {
            setTreeError(normalizeApiError(error));
          }
        }
      }

      const collectedRoots: string[] = [];
      let first: string | null = null;

      for (const bundle of bundles) {
        try {
          const result = await loadNode(bundle.hash, 'root', null, refreshGuard);
          if (!result) continue;

          if (isCurrentRefresh()) {
            setTreeNodes((prev) => {
              const next = { ...prev };
              const current = next[result.node.id];
              if (current) {
                next[result.node.id] = { ...current, name: bundle.name };
              }
              return next;
            });
          }

          collectedRoots.push(result.node.id);
          if (!first) {
            first = result.node.childrenIds[0] ?? result.node.id;
          }
        } catch (error) {
          loadFailed = true;
          if (isCurrentRefresh()) {
            setTreeError(normalizeApiError(error));
          }
        }
      }

      const fileTabMetadata: Record<string, { nodeId: string; title: string }> = {};
      const refreshedFileNodes: Record<string, TreeNode> = {};
      const invalidFileTabIds = new Set<string>();
      if (!isContextChange && !loadFailed) {
        for (const tab of tabsSnapshot) {
          if (tab.kind !== 'file') continue;
          const [tabBundleId, rawFileId] = tab.nodeId.includes(':')
            ? tab.nodeId.split(/:(.+)/)
            : [bundleId, tab.nodeId];
          const previousNode = treeNodesRef.current[tab.nodeId];
          try {
            const result = await loadNode(
              tabBundleId,
              rawFileId,
              previousNode?.parentId ?? null,
              refreshGuard
            );
            const node = result?.node ?? null;
            if (node && !node.is_dir && !isArchiveNode(node)) {
              fileTabMetadata[tab.nodeId] = {
                nodeId: node.id,
                title: node.name
              };
              refreshedFileNodes[tab.nodeId] = node;
            }
          } catch (error) {
            if (error instanceof ApiError && (error.code === 'RESOURCE_NOT_FOUND' || error.status === 404)) {
              invalidFileTabIds.add(tab.id);
              continue;
            }
            loadFailed = true;
            if (isCurrentRefresh()) {
              setTreeError(normalizeApiError(error));
            }
            break;
          }
        }
      }

      if (isCurrentRefresh()) {
        if (isContextChange) {
          setRootIds(collectedRoots);
          setExpandedNodes(new Set());
          setSelectedNodeId(first);
        } else if (!loadFailed) {
          setRootIds(collectedRoots);
          const currentTabs = viewerTabsRef.current;
          const currentTabMetadata = { ...fileTabMetadata };
          const validatedTabIds = new Set(tabsSnapshot.filter((tab) => tab.kind === 'file').map((tab) => tab.id));
          for (const tab of currentTabs) {
            if (tab.kind !== 'file' || validatedTabIds.has(tab.id) || currentTabMetadata[tab.nodeId]) continue;
            currentTabMetadata[tab.nodeId] = { nodeId: tab.nodeId, title: tab.title };
          }
          for (const tab of currentTabs) {
            if (tab.kind !== 'file' || !validatedTabIds.has(tab.id)) continue;
            const refreshed = refreshedFileNodes[tab.nodeId];
            if (invalidFileTabIds.has(tab.id)) {
              fileContentCache.reset(tab.id);
              continue;
            }
            if (!refreshed) {
              fileContentCache.reset(tab.id);
              continue;
            }
            const previous = treeNodesRef.current[tab.nodeId];
            if (previous && (
              previous.bundleId !== refreshed.bundleId
              || previous.rawId !== refreshed.rawId
              || previous.preview_kind !== refreshed.preview_kind
              || previous.size_bytes !== refreshed.size_bytes
              || previous.status !== refreshed.status
            )) {
              fileContentCache.reset(tab.id);
            }
          }
          const reconciled = reconcileViewerTabs(
            currentTabs,
            activeViewerTabIdRef.current,
            currentTabMetadata
          );
          setViewerTabsState(reconciled.tabs, reconciled.activeTabId);

          const activeTab = reconciled.tabs.find((tab) => tab.id === reconciled.activeTabId) ?? null;
          if (activeTab?.kind === 'file') {
            setSelectedNodeId(activeTab.nodeId);
            setLineStart(activeTab.lineStart);
            setLinePageSize(activeTab.pageSize);
            setTargetLine(activeTab.targetLine);
          } else if (currentTabs.find((tab) => tab.id === activeViewerTabIdRef.current)?.kind === 'file') {
            setSelectedNodeId(null);
          } else if (!selectedNodeIdRef.current) {
            setSelectedNodeId(first);
          }
        } else if (!selectedNodeIdRef.current) {
          setSelectedNodeId(first);
        }
      }
      if (isCurrentRefresh()) {
        setTreeLoading(false);
      }
    };

    init().catch(() => {
      if (isCurrentRefresh()) {
        setTreeLoading(false);
      }
    });
    return () => {
      ignore = true;
    };
  }, [
    issueCodeFromRoute,
    locationState?.issue,
    bundleId,
    activeBundle.name,
    loadNode,
    navigate,
    refreshKey,
    resetViewerTabs,
    setViewerTabsState
  ]);

  useEffect(() => {
    if (restoredPendingSearchRef.current) {
      restoredPendingSearchRef.current = false;
      return;
    }
    searchRequestGenerationRef.current += 1;
    setSearchTokens([]);
    setSearchDraft('');
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
    options?: { preserveSearch?: boolean; node?: TreeNode | null }
  ) => {
    pendingFilePageRef.current = null;
    if (!options?.preserveSearch) {
      setSearchError(null);
      setSearchExecuted(false);
      setResultFilterTokens([]);
      setResultFilterDraft('');
    }
    let node: TreeNode | null = options?.node ?? treeNodes[nodeId] ?? null;
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
      const tabId = `file:${node.id}`;
      const existing = viewerTabsRef.current.find((tab) => tab.id === tabId);
      const pageSize = existing?.kind === 'file' ? existing.pageSize : linePageSize;
      const sourceLine = typeof line === 'number' && line >= 0 ? line : null;
      const nextStart = sourceLine === null ? 0 : Math.floor(sourceLine / pageSize) * pageSize;
      if (sourceLine !== null) {
        setTargetLine(sourceLine);
        setLineStart(nextStart);
        setLinePageSize(pageSize);
        updateViewerTabs((tabs) => tabs.map((tab) => tab.id === tabId && tab.kind === 'file'
          ? { ...tab, lineStart: nextStart, pageSize, pageHistory: [], targetLine: sourceLine, scrollTop: 0 }
          : tab));
      } else if (existing?.kind === 'file') {
        setLineStart(existing.lineStart);
        setLinePageSize(existing.pageSize);
        setTargetLine(existing.targetLine);
      } else {
        setTargetLine(null);
        setLineStart(0);
      }
      openViewerTab({
        id: tabId,
        kind: 'file',
        title: node.name,
        pinned: false,
        scrollTop: 0,
        nodeId: node.id,
        lineStart: sourceLine === null ? 0 : nextStart,
        pageSize,
        pageHistory: [],
        targetLine: sourceLine
      });
    }
  };

  const revealSourceNode = async (source: ReturnType<typeof getSearchHitSource>) => {
    if (!source) return null;

    const knownNodes = new Map(Object.entries(treeNodesRef.current));
    const remember = (node: TreeNode, children: TreeNode[]) => {
      knownNodes.set(node.id, node);
      children.forEach((child) => knownNodes.set(child.id, child));
    };
    const updateTreeNodes = (nodes: TreeNode[]) => {
      nodes.forEach((node) => knownNodes.set(node.id, node));
      setTreeNodes((prev) => {
        const next = { ...prev };
        nodes.forEach((node) => {
          next[node.id] = node;
        });
        return next;
      });
    };
    const attachChild = (parent: TreeNode, child: TreeNode) => {
      const knownChildren = parent.childrenIds
        .map((childId) => knownNodes.get(childId))
        .filter((knownChild): knownChild is TreeNode => Boolean(knownChild));
      const attached = attachTreeChild(parent, child, knownChildren);
      updateTreeNodes([attached.parent, attached.child]);
      return attached;
    };
    const loadKnownNode = async (nodeId: string, parentId: string | null) => {
      const result = await loadNode(source.bundleHash, nodeId, parentId);
      if (!result) return null;
      remember(result.node, result.children);
      return result.node;
    };

    let current = knownNodes.get(source.nodeId) ?? null;
    if (!current) {
      current = await loadKnownNode(source.fileId, null);
    }
    if (!current) return null;

    const sourceNode = current;

    const ancestors: string[] = [];
    const visited = new Set<string>();
    while (true) {
      const activeNode: TreeNode | null = current;
      const parentId: string | null = activeNode ? activeNode.parentId : null;
      if (!parentId || visited.has(parentId)) break;
      visited.add(parentId);
      ancestors.push(parentId);
      let parent: TreeNode | null = knownNodes.get(parentId) ?? null;
      const isFlattenedExtraction = (candidate: TreeNode) =>
        activeNode !== null &&
        candidate.childrenSourceId === activeNode.rawId &&
        isExtractionFolder(activeNode, candidate);
      if (
        !parent ||
        !parent.hasLoadedChildren ||
        (activeNode !== null &&
          !parent.childrenIds.includes(activeNode.id) &&
          !isFlattenedExtraction(parent))
      ) {
        const rawParentId = parentId.split(/:(.+)/)[1] ?? '';
        parent = await loadKnownNode(rawParentId, parent?.parentId ?? null);
      }
      if (parent && activeNode) {
        const flattened = mergeFlattenedExtractionChildren(parent, activeNode);
        if (flattened !== parent) {
          const flattenedParent: TreeNode = flattened;
          parent = flattenedParent;
          updateTreeNodes([flattenedParent]);
        } else if (
          !parent.childrenIds.includes(activeNode.id)
          && !isExtractionFolder(activeNode, parent)
        ) {
          parent = attachChild(parent, activeNode).parent;
        }
      }
      if (!parent) break;
      current = parent;
    }

    if (ancestors.length > 0) {
      setExpandedNodes((prev) => new Set([...prev, ...ancestors]));
    }

    const syntheticRootId = `${source.bundleHash}:root`;
    const syntheticRoot = knownNodes.get(syntheticRootId)
      ?? await loadKnownNode('root', null);
    if (syntheticRoot && current.id !== syntheticRoot.id && !current.parentId) {
      attachChild(syntheticRoot, current);
    }
    return sourceNode;
  };

  const openSearchHitSource = async (hit: IssueLogSearchHit) => {
    const source = getSearchHitSource(hit);
    if (!source) {
      setSourceActionMessage('来源文件信息不可用');
      return;
    }
    try {
      const revealedNode = await revealSourceNode(source);
      await handleNodeClick(source.nodeId, source.line, {
        preserveSearch: true,
        node: revealedNode
      });
      setSourceActionMessage(
        source.line === null ? '已打开文件，原始行号不可用' : null
      );
    } catch (error) {
      setSourceActionMessage(normalizeApiError(error));
    }
  };

  const activateViewerTabWithState = (tab: ViewerTab) => {
    pendingFilePageRef.current = null;
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
    fileContentCache.reset(id);
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

  const closeTabs = (ids: string[]) => {
    if (ids.length === 0) return;
    const closing = new Set(ids);
    const activeIndex = viewerTabs.findIndex((tab) => tab.id === activeViewerTabId);
    const remaining = viewerTabs.filter((tab) => !closing.has(tab.id));
    const activeRemains = activeViewerTabId
      ? remaining.find((tab) => tab.id === activeViewerTabId) ?? null
      : null;
    const next = activeRemains ?? remaining[Math.min(Math.max(activeIndex, 0), remaining.length - 1)] ?? null;
    for (const id of closing) fileContentCache.reset(id);
    setViewerTabsState(remaining, next?.id ?? null);
    if (next?.kind === 'file') {
      setSelectedNodeId(next.nodeId);
      setLineStart(next.lineStart);
      setLinePageSize(next.pageSize);
      setTargetLine(next.targetLine);
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
      pageHistory: [],
      targetLine
    });
  }, [linePageSize, lineStart, openViewerTab, selectedNode, targetLine]);

  useEffect(() => {
    const pending = pendingFilePageRef.current;
    if (!pending) return;
    const activeId = activeViewerTabIdRef.current;
    if (activeId !== pending.tabId) {
      pendingFilePageRef.current = null;
      return;
    }
    if (fileContentLoading) return;
    if (fileContentError || fileLines?.start !== pending.from) {
      if (fileContentError) pendingFilePageRef.current = null;
      return;
    }
    updateViewerTabs((tabs) => tabs.map((tab) => {
      if (tab.id !== pending.tabId || tab.kind !== 'file') return tab;
      return {
        ...tab,
        pageHistory: pending.navigation === 'next'
          ? [...(tab.pageHistory ?? []), pending.previousStart ?? tab.lineStart]
          : pending.navigation === 'previous'
            ? (tab.pageHistory ?? []).slice(0, -1)
            : []
      };
    }));
    pendingFilePageRef.current = null;
  }, [activeViewerTabId, fileContentError, fileContentLoading, fileLines, updateViewerTabs]);

  const clearFileSearch = useCallback(() => {
    void fileSearchExecution.cancel();
    setFileSearchTokens([]);
    setFileSearchDraft('');
    setFileSearchResults([]);
    setFileSearchTotal(0);
    setFileSearchFrom(0);
    setFileSearchError(null);
    setFileSearchExecuted(false);
  }, []);

  useEffect(() => {
    if (fileSearchExecuted && isFileSearchConditionEmpty(fileSearchTokens, fileSearchDraft)) {
      clearFileSearch();
    }
  }, [clearFileSearch, fileSearchDraft, fileSearchExecuted, fileSearchTokens]);

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
      const response = await fileSearchExecution.execute(
        {
          expression,
          bundle_hash: selectedBundleId,
          file_id: selectedNode.rawId,
          from,
          size: LINE_PAGE_SIZE_OPTIONS[0]
        },
        {
          scopeKey: `file:${selectedBundleId}:${selectedNode.rawId}`,
          onSuccess: (response) => {
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
                pageHistory: [],
                source: { kind: 'file', bundleHash: selectedBundleId, fileId: selectedNode.rawId }
              });
              setFileSearchResults([]);
              setFileSearchExecuted(false);
            }
          }
        }
      );
      if (!response) return;
    } catch (error) {
      setFileSearchError(normalizeApiError(error));
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
      const response = await viewerSearchExecution.execute(payload, {
        scopeKey: `viewer:${activeViewerTab.id}:${activeViewerTab.resultId}`,
        onSuccess: (response) => {
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
            pageHistory: [],
            source
          });
        }
      });
      if (!response) return;
    } catch (error) {
      setSearchError(normalizeApiError(error));
    }
  }, [activeViewerTab, openViewerTab, resultFilterDraft, resultFilterTokens]);

  const loadViewerPage = useCallback(async (
    tab: ViewerTab,
    from: number,
    pageSize: number,
    navigation: 'next' | 'previous' | 'reset'
  ) => {
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
              pageHistory: navigation === 'next'
                ? [...(item.pageHistory ?? []), item.from]
                : navigation === 'previous'
                  ? (item.pageHistory ?? []).slice(0, -1)
                  : [],
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
            pageHistory: navigation === 'next'
              ? [...(item.pageHistory ?? []), item.from]
              : navigation === 'previous'
                ? (item.pageHistory ?? []).slice(0, -1)
                : [],
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
    if (selectedNode.hasLoadedChildren || selectedNode.childrenLoadError) return;
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
    const container = fileTreeContainerRef.current;
    if (!container || !selectedNodeId) return;
    const target = Array.from(
      container.querySelectorAll<HTMLElement>('[data-file-tree-node-id]')
    ).find((element) => element.dataset.fileTreeNodeId === selectedNodeId);
    if (!target || typeof target.scrollIntoView !== 'function') return;
    target.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  }, [expandedNodes, selectedNodeId, treeNodes]);

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
    const scrollContainer = contentRef.current;
    if (!scrollContainer) return;
    if (targetLine === null || targetLine === undefined) return;
    if (!fileLines) return;
    const target = scrollContainer.querySelector<HTMLElement>(
      `[data-source-line="${targetLine}"]`
    );
    if (target) {
      centerElementInScrollContainer(scrollContainer, target);
    }
  }, [fileLines, targetLine]);

  useEffect(() => {
    if (!contentRef.current || !activeViewerTab || targetLine !== null) return;
    contentRef.current.scrollTop = activeViewerTab.scrollTop;
  }, [activeViewerTab?.id, activeViewerTab?.scrollTop, fileLines, targetLine]);

  const fileSearchHighlightTerm = getSearchTerms(fileSearchTokens)[0] ?? fileSearchDraft.trim();
  const resultFilterHighlightTerm = getSearchTerms(resultFilterTokens)[0] ?? resultFilterDraft.trim();
  const canRunSearch = canFinalizeSearch(searchTokens, searchDraft);
  const showDetailedClear = (
    searchTokens.length > 0
    || Boolean(searchDraft.trim())
    || searchExecuted
    || searchLoading
    || Boolean(searchError)
  );
  const canRunFileSearch = canFinalizeSearch(fileSearchTokens, fileSearchDraft);
  const canRunResultFilter = canFinalizeSearch(resultFilterTokens, resultFilterDraft);

  return (
    <div className="space-y-5">
      <section className="panel overflow-hidden !p-0 lg:h-[calc(100vh-104px)]">
        {treeError ? (
          <p className="m-4 rounded-md border border-rose-200 bg-rose-50 px-3 py-2 text-sm text-rose-600">
            {treeError}
          </p>
        ) : null}

        <div className="bundle-layout min-h-[calc(100vh-104px)] gap-0 lg:h-full lg:min-h-0 lg:overflow-hidden">
          <div className="relative flex min-h-0 flex-col border-r border-slate-200 bg-white">
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
                <SearchExpressionEditor
                  tokens={searchTokens}
                  draft={searchDraft}
                  onTokensChange={(tokens) => { searchRequestGenerationRef.current += 1; setSearchTokens(tokens); setSearchError(null); }}
                  onDraftChange={(draft) => { searchRequestGenerationRef.current += 1; setSearchDraft(draft); setSearchError(null); }}
                  placeholder="输入关键词"
                  ariaLabel="日志内容搜索条件"
                  disabled={searchLoading}
                />
                {showDetailedClear ? (
                  <button
                    type="button"
                    className="mt-0.5 shrink-0 rounded border border-slate-300 px-3 py-1.5 text-xs font-semibold text-slate-600 transition hover:border-slate-400 hover:text-slate-950"
                    aria-label="清除日志内容搜索"
                    onClick={clearDetailedSearch}
                  >
                    清除
                  </button>
                ) : null}
                <button
                  type="submit"
                  className="mt-0.5 shrink-0 rounded bg-slate-200 px-3 py-1.5 text-xs font-semibold text-slate-900 transition hover:bg-slate-300 disabled:cursor-not-allowed disabled:opacity-50"
                  aria-label="搜索日志内容"
                  disabled={searchLoading || !issueCode || !canRunSearch}
                >
                  搜索
                </button>
              </form>
              <div className="mt-3 flex w-full items-center justify-between gap-2 text-xs text-slate-500">
                {auth.state.status === 'AUTHENTICATED' ? (
                  <>
                    <button
                      type="button"
                      className="rounded-md border border-slate-300 bg-white px-2.5 py-1.5 font-semibold text-slate-700 hover:border-sky-400"
                      onClick={() => setSavedSearchesOpen((open) => !open)}
                    >
                      我的搜索条件
                    </button>
                    <button
                      type="button"
                      className="rounded-md border border-slate-300 bg-white px-2.5 py-1.5 font-semibold text-slate-700 hover:border-sky-400"
                      onClick={beginSaveSearch}
                    >
                      保存条件
                    </button>
                  </>
                ) : null}
              </div>
              <SearchExecutionStatus
                snapshot={issueSearchExecution.snapshot}
                onCancel={() => { void issueSearchExecution.cancel(); }}
              />
              {savedSearchesOpen ? (
                <div className="mt-3 space-y-2 rounded-lg border border-slate-200 bg-slate-50 p-3">
                  {savedSearches.length === 0 ? <p className="text-xs text-slate-500">暂无搜索条件</p> : savedSearches.map((item) => (
                    <div key={item.id} className="rounded-md border border-slate-200 bg-white p-2 text-xs">
                      <div className="flex items-center gap-2">
                        <span className="min-w-0 flex-1 truncate font-semibold">{item.is_pinned ? '★ ' : ''}{item.name}</span>
                        <button className="text-sky-700" type="button" onClick={() => void useSavedSearch(item).catch((error) => setSavedSearchError(normalizeApiError(error)))}>使用</button>
                        <button className="text-slate-700" type="button" onClick={() => beginEditingSavedSearch(item)}>编辑</button>
                        <button className="text-rose-700" type="button" onClick={() => void rainApi.deleteSavedSearch(item.id).then(loadSavedSearches).catch((error) => setSavedSearchError(normalizeApiError(error)))}>删除</button>
                      </div>
                      <p className="mt-1 truncate text-slate-500">{item.query_text}</p>
                    </div>
                  ))}
                </div>
              ) : null}
              {searchError ? <p className="mt-2 text-xs text-rose-600">{searchError}</p> : null}
              {savedSearchError ? <p className="mt-2 text-xs text-rose-600">{savedSearchError}</p> : null}
            </div>
            <div ref={fileTreeContainerRef} className="min-h-0 flex-1 overflow-x-auto overflow-y-auto px-4 py-3">
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
            {rootIds.length > 0 ? (
              <div className="min-w-max space-y-2 text-sm text-slate-700">
                {rootIds.some((rootId) => {
                  const root = treeNodes[rootId];
                  return (root?.childrenIds.length ?? 0) > 0 || root?.hasMoreChildren;
                }) ? (
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
                            loading={treeLoading}
                            onRetryLoad={(node) => {
                              loadNode(node.bundleId, node.rawId, node.parentId).catch(() => undefined);
                            }}
                            onLoadMore={(node) => {
                              loadMoreNode(node).catch(() => undefined);
                            }}
                          />
                        );
                      })}
                      {treeNodes[rootId]?.hasMoreChildren ? (
                        <button
                          type="button"
                          className="px-2 py-1 text-xs text-sky-700 hover:text-sky-950"
                          disabled={treeLoading}
                          onClick={() => {
                            const root = treeNodes[rootId];
                            if (root) loadMoreNode(root).catch(() => undefined);
                          }}
                        >
                          {treeLoading ? '加载中…' : '加载更多'}
                        </button>
                      ) : null}
                    </div>
                  ))
                ) : (
                  <p className="text-sm text-slate-500">暂无文件。</p>
                )}
              </div>
            ) : treeLoading ? (
              <p className="text-sm text-slate-500">文件树加载中...</p>
            ) : hasFileContext ? (
              <p className="text-sm text-slate-500">暂无可用文件。</p>
            ) : null}
            </div>
          </div>

          <div className="flex min-h-[calc(100vh-104px)] flex-col bg-slate-50 text-sm text-slate-700 lg:h-full lg:min-h-0 lg:overflow-hidden">
            <ViewerTabBar
              tabs={viewerTabs}
              activeTabId={activeViewerTabId}
              onActivate={activateViewerTabWithState}
              onTogglePinned={togglePinnedViewerTab}
              onClose={closeTab}
              onCloseMany={closeTabs}
            />
            <p
              aria-live="polite"
              className={`px-4 py-1 text-xs ${sourceActionMessage ? 'text-slate-600' : 'sr-only'}`}
            >
              {sourceActionMessage ?? ''}
            </p>
            <div className="flex min-h-0 flex-1 flex-col p-4">
              <div className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-md border border-slate-200 bg-white shadow-sm shadow-slate-100">
                {activeViewerTab?.kind === 'file' && activeFileNode &&
                canPreviewText(activeFileNode) ? (
                  <div className="flex min-h-14 flex-wrap items-center gap-3 border-b border-slate-200 bg-white px-4 py-3 focus-within:border-sky-400">
                    <span className="mt-1.5 shrink-0 self-start text-slate-500" aria-hidden="true">⌕</span>
                    <SearchTokenEditor
                      className="min-w-[220px]"
                      tokens={fileSearchTokens}
                      draft={fileSearchDraft}
                      onTokensChange={setFileSearchTokens}
                      onDraftChange={setFileSearchDraft}
                      placeholder="输入关键词"
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
                  <>
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
                      onLoadPage={(tab, from, pageSize, navigation) => {
                        loadViewerPage(tab, from, pageSize, navigation).catch(() => undefined);
                      }}
                      highlightTerm={resultFilterHighlightTerm}
                      renderHighlightedText={highlightText}
                      onOpenSource={openSearchHitSource}
                    />
                  </>
                ) : activeViewerTab?.kind !== 'file' || !activeFileNode ? (
                  <p className="py-8 text-center text-sm text-slate-500">
                    输入关键词搜索当前 Issue 的日志。
                  </p>
                ) : isArchiveNode(activeFileNode) ? (
                  <p className="text-sm text-slate-500">压缩包请在左侧展开查看内部文件。</p>
                ) : activeFileNode.is_dir ? (
                  <p className="text-sm text-slate-500">当前为目录，选择文件后展示内容。</p>
                ) : isBinaryNode(activeFileNode) ? (
                  <BinaryFileInfo
                    node={activeFileNode}
                  />
                ) : fileContentLoading ? (
                  <p className="text-sm text-slate-500">读取中...</p>
                ) : fileContentError ? (
                  <div className="flex flex-col items-center justify-center gap-3 py-8 text-sm text-rose-600">
                    <p>{fileContentError}</p>
                    <button
                      type="button"
                      className="rounded border border-slate-300 px-3 py-1 text-slate-700 hover:border-slate-500"
                      onClick={retryFileContent}
                    >
                      重试
                    </button>
                  </div>
                ) : fileLines ? (
                  <div className="flex min-h-0 flex-1 flex-col gap-2">
                    <CodeLinesPane
                      lines={fileLines.lines}
                      contentRef={contentRef}
                      lineNumberOffset={fileLines.start}
                      targetLine={targetLine}
                      fileName={activeFileNode.name}
                    />
                    <div className="flex flex-wrap items-center justify-end gap-2 border-t border-slate-200 bg-slate-50 px-4 py-2 text-xs text-slate-500">
                      <label className="flex items-center gap-2">
                        <span>每页</span>
                        <select
                          className="rounded border border-slate-300 bg-white px-2 py-1 text-slate-700 outline-none focus:border-cyan-500/60"
                          value={linePageSize}
                          onChange={(event) => {
                            if (activeViewerTab?.kind !== 'file') return;
                            const nextPageSize = Number(event.target.value);
                            pendingFilePageRef.current = {
                              tabId: activeViewerTab.id,
                              from: 0,
                              navigation: 'reset'
                            };
                            setLinePageSize(nextPageSize);
                            setLineStart(0);
                            setTargetLine(null);
                            updateViewerTabs((tabs) => tabs.map((tab) => tab.id === activeViewerTab.id && tab.kind === 'file'
                              ? { ...tab, lineStart: 0, pageSize: nextPageSize, pageHistory: [], targetLine: null, scrollTop: 0 }
                              : tab));
                          }}
                        >
                          {LINE_PAGE_SIZE_OPTIONS.map((size) => (
                            <option key={size} value={size}>{size} 行</option>
                          ))}
                        </select>
                      </label>
                      <span className="min-w-[120px] text-center">
                        {fileLines.lines.length > 0
                          ? `${fileLines.start + 1} - ${fileLines.start + fileLines.lines.length}`
                          : '0'}
                        {fileLines.line_count ? ` / ${fileLines.line_count}` : ''}
                      </span>
                      <button
                        type="button"
                        className="rounded border border-slate-300 px-3 py-1 text-slate-600 hover:border-slate-500 disabled:opacity-50"
                        disabled={activeViewerTab?.kind !== 'file' || (activeViewerTab.pageHistory ?? []).length === 0 || fileContentLoading}
                        onClick={() => {
                          if (activeViewerTab?.kind !== 'file') return;
                          const pageHistory = activeViewerTab.pageHistory ?? [];
                          const previousStart = pageHistory[pageHistory.length - 1];
                          if (previousStart === undefined) return;
                          pendingFilePageRef.current = {
                            tabId: activeViewerTab.id,
                            from: previousStart,
                            navigation: 'previous'
                          };
                          updateViewerTabs((tabs) => tabs.map((tab) => tab.id === activeViewerTab.id && tab.kind === 'file'
                            ? { ...tab, lineStart: previousStart, targetLine: null }
                            : tab));
                          setLineStart(previousStart);
                        }}
                      >
                        上一页
                      </button>
                      <button
                        type="button"
                        className="rounded border border-slate-300 px-3 py-1 text-slate-600 hover:border-slate-500 disabled:opacity-50"
                        disabled={!fileLines.next_start || fileContentLoading}
                        onClick={() => {
                          if (activeViewerTab?.kind !== 'file') return;
                          const nextStart = fileLines.next_start ?? lineStart + fileLines.lines.length;
                          pendingFilePageRef.current = {
                            tabId: activeViewerTab.id,
                            from: nextStart,
                            navigation: 'next',
                            previousStart: lineStart
                          };
                          updateViewerTabs((tabs) => tabs.map((tab) => tab.id === activeViewerTab.id && tab.kind === 'file'
                            ? { ...tab, lineStart: nextStart, targetLine: null }
                            : tab));
                          setLineStart(nextStart);
                        }}
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
      {saveDialogOpen ? (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/45 p-4">
          <div className="w-full max-w-md rounded-2xl bg-white p-6 shadow-2xl">
            <h3 className="text-xl font-semibold">保存搜索条件</h3>
            <label className="mt-4 block text-sm font-medium">名称
              <input className="mt-2 w-full rounded-lg border border-slate-300 px-3 py-2" maxLength={80} value={savedSearchName} onChange={(event) => { saveDialogGenerationRef.current += 1; setSavedSearchName(event.target.value); }} autoFocus />
            </label>
            {savedSearchError ? <p className="mt-3 text-sm text-rose-600">{savedSearchError}</p> : null}
            <div className="mt-6 flex justify-end gap-3">
              <button className="rounded-lg border border-slate-300 px-4 py-2" type="button" onClick={() => { saveDialogGenerationRef.current += 1; setSaveDialogOpen(false); }}>取消</button>
              <button className="rounded-lg bg-slate-950 px-4 py-2 font-semibold text-white" type="button" onClick={() => void saveSearch()}>保存</button>
            </div>
          </div>
        </div>
      ) : null}
      {editingSavedSearch ? (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/45 p-4">
          <div className="w-full max-w-lg space-y-4 rounded-2xl bg-white p-6 shadow-2xl">
            <h3 className="text-xl font-semibold">编辑搜索条件</h3>
            <label className="block text-sm font-medium">名称
              <input className="mt-1 w-full rounded-lg border border-slate-300 px-3 py-2" maxLength={80} value={editingSavedSearch.name} onChange={(event) => { editingSaveGenerationRef.current += 1; setEditingSavedSearch({ ...editingSavedSearch, name: event.target.value }); }} />
            </label>
            <label className="block text-sm font-medium">搜索表达式
              <div className="mt-1 rounded-lg border border-slate-300 px-3 py-2">
                <SearchExpressionEditor
                  tokens={editingSearchTokens}
                  draft={editingSearchDraft}
                  onTokensChange={(tokens) => { editingSaveGenerationRef.current += 1; setEditingSearchTokens(tokens); setSavedSearchError(''); }}
                  onDraftChange={(draft) => { editingSaveGenerationRef.current += 1; setEditingSearchDraft(draft); setSavedSearchError(''); }}
                  placeholder="输入关键词"
                  ariaLabel="编辑详细搜索条件"
                />
              </div>
            </label>
            <div className="flex gap-6">
              <label className="flex items-center gap-2 text-sm font-medium">
                <input type="checkbox" checked={editingSavedSearch.is_pinned} onChange={(event) => { editingSaveGenerationRef.current += 1; setEditingSavedSearch({ ...editingSavedSearch, is_pinned: event.target.checked }); }} />
                置顶
              </label>
            </div>
            <div className="flex justify-end gap-3">
              {savedSearchError ? <p className="mr-auto self-center text-sm text-rose-600">{savedSearchError}</p> : null}
              <button className="rounded-lg border border-slate-300 px-4 py-2" type="button" onClick={() => { editingSaveGenerationRef.current += 1; setEditingSavedSearch(null); }}>取消</button>
              <button className="rounded-lg bg-slate-950 px-4 py-2 font-semibold text-white" type="button" onClick={() => void updateEditingSavedSearch()}>保存修改</button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
