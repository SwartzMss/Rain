import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useLocation, useParams } from 'react-router-dom';
import { normalizeApiError, rainApi } from '../../api/client';
import type { FileLinesResponse, FileNode, IssueLogSearchHit, UploadSummary } from '../../api/types';
import type { BundleInfo } from '../../lib/bundles';

type TreeNode = Omit<FileNode, 'id' | 'children'> & {
  id: string;
  rawId: string;
  bundleId: string;
  parentId: string | null;
  childrenIds: string[];
  hasLoadedChildren: boolean;
};

const archivePattern = /\.(zip|tar|gz|tgz|rar|7z)$/i;
const LINE_PAGE_SIZE = 200;
const isArchiveNode = (node?: { name: string }) => (node?.name ? archivePattern.test(node.name) : false);

const formatSize = (bytes?: number) => {
  if (bytes === undefined || bytes === null) return '--';
  const units = ['B', 'KB', 'MB', 'GB'];
  let size = bytes;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  const fixed = unit === 0 ? size.toFixed(0) : size.toFixed(1);
  return `${fixed} ${units[unit]}`;
};

const nodeTypeLabel = (node: TreeNode) => {
  if (node.is_dir) return '目录';
  if (isArchiveNode(node)) return '压缩包';
  return '文件';
};

const isExtractionFolder = (node: TreeNode, parent?: TreeNode | null) => {
  if (!node.is_dir || !node.name.toLowerCase().endsWith('_extracted')) return false;
  return parent ? isArchiveNode(parent) : false;
};

const bundleStatusLabel = (bundle: UploadSummary) => {
  if (bundle.status.upload_status === 'PROCESSING' || bundle.status.upload_status === 'PENDING') {
    return '正在建立索引';
  }
  if (bundle.status.upload_status === 'FAILED') {
    return '处理失败';
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
        className="rounded bg-cyan-400/20 px-0.5 text-cyan-100"
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
  const [fileLines, setFileLines] = useState<FileLinesResponse | null>(null);
  const [lineStart, setLineStart] = useState(0);
  const [fileContentLoading, setFileContentLoading] = useState(false);
  const [fileContentError, setFileContentError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const [searchTerm, setSearchTerm] = useState('');
  const [searchResults, setSearchResults] = useState<IssueLogSearchHit[]>([]);
  const [searchLoading, setSearchLoading] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [searchExecuted, setSearchExecuted] = useState(false);
  const [searchMode, setSearchMode] = useState<'log' | 'detailed'>('log');
  const [resultFilterText, setResultFilterText] = useState('');
  const [targetLine, setTargetLine] = useState<number | null>(null);
  const [nonReadyBundles, setNonReadyBundles] = useState<UploadSummary[]>([]);
  const contentRef = useRef<HTMLDivElement | null>(null);

  const formatHitPath = (raw: string) => {
    const parts = raw.replace(/^\//, '').split('/');
    if (parts.length === 0) return raw;
    // drop bundle hash
    const [, ...rest] = parts;
    if (rest.length === 0) return raw.replace(/^\//, '');
    const normalized = rest.map((segment, index) => {
      if (index === 0 && segment.endsWith('_extracted')) {
        return segment.replace(/_extracted$/, '');
      }
      return segment;
    });
    return normalized.join('/');
  };

  const toTreeNode = (bundleId: string, node: FileNode, parentId: string | null = null): TreeNode => ({
    id: `${bundleId}:${node.id.toString()}`,
    rawId: node.id.toString(),
    bundleId,
    parentId,
    name: node.name,
    path: node.path,
    is_dir: node.is_dir,
    size_bytes: node.size_bytes,
    mime_type: node.mime_type,
    status: node.status,
    meta: node.meta,
    childrenIds: [],
    hasLoadedChildren: false
  });

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
    const keyword = searchTerm.trim();
    if (!issue || !keyword) {
      setSearchResults([]);
      setSearchError(null);
      setSearchExecuted(false);
      setResultFilterText('');
      return;
    }
    setSearchLoading(true);
    setSearchError(null);
    setSearchExecuted(true);
    setResultFilterText('');
    try {
      const response = await rainApi.searchIssueLogs(issue, keyword, {
        mode: searchMode === 'log' ? 'filename' : 'content',
        size: 50
      });
      setSearchResults(response.hits);
    } catch (error) {
      setSearchResults([]);
      setSearchError(normalizeApiError(error));
    } finally {
      setSearchLoading(false);
    }
  }, [issueCode, searchMode, searchTerm]);

  const changeSearchMode = (mode: 'log' | 'detailed') => {
    if (mode === searchMode) return;
    setSearchMode(mode);
    setSearchResults([]);
    setSearchError(null);
    setSearchExecuted(false);
    setResultFilterText('');
  };

  useEffect(() => {
    const issueCode = issueCodeFromRoute || locationState?.issue || '';
    const fallbackBundles = bundleId ? [{ hash: bundleId, name: activeBundle.name }] : [];

    let ignore = false;
    const init = async () => {
      setTreeLoading(true);
      setTreeError(null);
      setTreeNodes({});
      setExpandedNodes(new Set());
      setRootIds([]);
      setSelectedNodeId(null);
      setNonReadyBundles([]);

      let bundles = fallbackBundles;
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
          setTreeError(normalizeApiError(error));
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
          setTreeError(normalizeApiError(error));
        }
      }

      if (!ignore) {
        setExpandedNodes(new Set());
        setRootIds(collectedRoots);
        setSelectedNodeId((prev) => prev || first);
      }
      setTreeLoading(false);
    };

    init().catch(() => setTreeLoading(false));
    return () => {
      ignore = true;
    };
  }, [issueCodeFromRoute, locationState?.issue, bundleId, activeBundle.name, loadNode, refreshKey]);

  useEffect(() => {
    setSearchResults([]);
    setSearchError(null);
    setSearchExecuted(false);
    setResultFilterText('');
  }, [issueCode, refreshKey]);

  const currentFilteredResults = searchResults;
  const visibleSearchResults = useMemo(() => {
    const keyword = resultFilterText.trim().toLowerCase();
    if (!keyword) return currentFilteredResults;

    return currentFilteredResults.filter((hit) => {
      const path = hit.path?.toLowerCase() ?? '';
      const snippet = hit.snippet?.toLowerCase() ?? '';
      const timeline = hit.timeline?.toLowerCase() ?? '';
      return path.includes(keyword) || snippet.includes(keyword) || timeline.includes(keyword);
    });
  }, [currentFilteredResults, resultFilterText]);

  const handleNodeClick = async (nodeId: string, line?: number | null) => {
    if (typeof line === 'number' && line >= 0) {
      setTargetLine(line);
      setLineStart(Math.max(0, line - 20));
    } else {
      setTargetLine(null);
      setLineStart(0);
    }
    setSearchResults([]);
    setSearchError(null);
    setSearchExecuted(false);
    setResultFilterText('');
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
  };

  const selectedNode = selectedNodeId ? treeNodes[selectedNodeId] : null;

  const activeIssueLabel = activeBundle.issue || '未知 Issue';
  const activeNodeLabel = selectedNode?.name || '未选择文件';

  useEffect(() => {
    setFileLines(null);
    setFileContentError(null);
    if (!selectedNode || selectedNode.is_dir) return;
    const bundleForContent = selectedNode.bundleId || bundleId;
    if (!bundleForContent) return;
    let ignore = false;
    const fetchContent = async () => {
      setFileContentLoading(true);
      try {
        const content = await rainApi.fetchFileLines(bundleForContent, selectedNode.rawId, {
          start: lineStart,
          limit: LINE_PAGE_SIZE
        });
        if (!ignore) {
          setFileLines(content);
        }
      } catch (error) {
        if (!ignore) {
          setFileContentError(normalizeApiError(error));
        }
      } finally {
        if (!ignore) {
          setFileContentLoading(false);
        }
      }
    };
    fetchContent();
    return () => {
      ignore = true;
    };
  }, [bundleId, selectedNode?.id, selectedNode?.is_dir, lineStart]);

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
    const pre = contentRef.current;
    const lineHeightPx = 20;
    const clampedIndex = Math.min(
      Math.max(targetLine - (fileLines.start ?? 0), 0),
      Math.max(fileLines.lines.length - 1, 0)
    );
    pre.scrollTop = Math.max(0, clampedIndex * lineHeightPx);
  }, [fileLines, targetLine]);

  const renderTreeNode = (nodeId: string, depth = 0): JSX.Element | null => {
    const node = treeNodes[nodeId];
    if (!node) return null;
    const parentNode = node.parentId ? treeNodes[node.parentId] : null;
    if (isExtractionFolder(node, parentNode)) {
      return (
        <div key={nodeId} className="border-l border-slate-800 pl-3">
          {node.childrenIds.length > 0 ? (
            node.childrenIds.map((childId) => renderTreeNode(childId, depth))
          ) : (
            <p className="py-1 text-xs text-slate-500">暂无子节点</p>
          )}
        </div>
      );
    }
    const isExpanded = expandedNodes.has(nodeId);
    const isSelected = selectedNodeId === nodeId;
    const canExpand = node.is_dir || isArchiveNode(node);
    const badgeColor = node.is_dir
      ? 'border-amber-400/60 text-amber-200'
      : isArchiveNode(node)
        ? 'border-brand-400/70 text-brand-200'
        : 'border-slate-600 text-slate-200';

    return (
      <div key={nodeId}>
        <button
          type="button"
          onClick={() => handleNodeClick(node.id).catch(() => undefined)}
          className={[
            'flex w-full items-center gap-3 rounded-lg border border-transparent px-2 py-2 text-left text-sm transition',
            isSelected ? 'border-brand-500/60 bg-slate-800/80 shadow-sm text-white' : 'hover:border-slate-800 hover:bg-slate-900/40'
          ].join(' ')}
          style={{ paddingLeft: `${depth * 12}px` }}
        >
          <span className={`flex h-7 w-7 items-center justify-center rounded border text-[10px] font-semibold ${badgeColor}`}>
            {node.is_dir ? 'DIR' : isArchiveNode(node) ? 'ZIP' : 'FILE'}
          </span>
          <div className="min-w-0">
            <p className="truncate text-sm font-medium">{node.name}</p>
            <p className="text-[10px] uppercase text-slate-500">
              {canExpand ? `${node.childrenIds.length || 0} 子节点` : node.mime_type ?? 'file'}
            </p>
          </div>
          <span className="ml-auto text-xs text-slate-500">
            {canExpand ? (isExpanded ? '收起' : '展开') : formatSize(node.size_bytes)}
          </span>
        </button>
        {canExpand && isExpanded ? (
          <div className="border-l border-slate-800 pl-3">
            {node.childrenIds.length > 0 ? (
              node.childrenIds.map((childId) => renderTreeNode(childId, depth + 1))
            ) : (
              <p className="py-1 text-xs text-slate-500">暂无子节点</p>
            )}
          </div>
        ) : null}
      </div>
    );
  };

  return (
    <div className="space-y-5">
      <section className="panel space-y-4">
        {treeError ? <p className="text-sm text-rose-300">{treeError}</p> : null}

        <div className="grid gap-4 lg:grid-cols-[420px_minmax(0,1fr)]">
          <div className="space-y-3 rounded-lg border border-slate-800 bg-slate-900 p-3">
            <p className="text-xs text-slate-400">Issue: {activeIssueLabel}</p>
            {nonReadyBundles.length > 0 ? (
              <div className="space-y-1 rounded-lg border border-slate-800 bg-slate-950/60 p-3 text-xs text-slate-300">
                {nonReadyBundles.map((bundle) => (
                  <div key={bundle.hash} className="flex items-center justify-between gap-3">
                    <span className="truncate">{bundle.name || bundle.hash}</span>
                    <span className={bundle.status.upload_status === 'FAILED' ? 'text-rose-300' : 'text-amber-200'}>
                      {bundleStatusLabel(bundle)}
                    </span>
                  </div>
                ))}
              </div>
            ) : null}
            <div className="space-y-2 rounded-lg border border-slate-800 bg-slate-950/60 p-3">
              <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:gap-3">
                <input
                  className="w-full rounded-lg border border-slate-700 bg-slate-900 px-3 py-2 text-sm text-white focus:border-brand-500 focus:outline-none"
                  placeholder={
                    searchMode === 'log'
                      ? '按文件名搜索当前 Issue 的日志'
                      : '在当前 Issue 的所有文件内搜索内容'
                  }
                  value={searchTerm}
                  onChange={(event) => setSearchTerm(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter') {
                      event.preventDefault();
                      runSearch().catch(() => undefined);
                    }
                  }}
                />
                <button
                  type="button"
                  className={`w-full whitespace-nowrap rounded-lg px-4 py-2 text-sm font-semibold transition sm:w-auto ${
                    searchLoading
                      ? 'cursor-not-allowed bg-slate-700 text-slate-400'
                      : 'bg-brand-500 text-slate-900 hover:bg-brand-700 disabled:cursor-not-allowed disabled:bg-slate-700 disabled:text-slate-400'
                  }`}
                  onClick={() => runSearch().catch(() => undefined)}
                  disabled={searchLoading || !issueCode || !searchTerm.trim()}
                >
                  搜索
                </button>
              </div>
              <div className="flex flex-col gap-2 text-xs text-slate-300 sm:flex-row sm:items-center sm:gap-3">
                <span className="whitespace-nowrap">搜索模式</span>
                <div className="flex flex-wrap gap-2">
                  <button
                    type="button"
                    className={`rounded border px-3 py-1 ${searchMode === 'log' ? 'border-brand-500 bg-brand-500/20 text-brand-100' : 'border-slate-700 text-slate-300 hover:border-slate-500'}`}
                    onClick={() => changeSearchMode('log')}
                  >
                    日志模式
                  </button>
                  <button
                    type="button"
                    className={`rounded border px-3 py-1 ${searchMode === 'detailed' ? 'border-brand-500 bg-brand-500/20 text-brand-100' : 'border-slate-700 text-slate-300 hover:border-slate-500'}`}
                    onClick={() => changeSearchMode('detailed')}
                  >
                    搜索模式
                  </button>
                </div>
              </div>
              {searchError ? <p className="text-xs text-rose-300">{searchError}</p> : null}
            </div>
            {rootIds.length > 0 ? (
              <div className="space-y-2 text-sm text-slate-200">
                {rootIds.some((rootId) => (treeNodes[rootId]?.childrenIds.length ?? 0) > 0) ? (
                  rootIds.map((rootId) => (
                    <div key={rootId} className="space-y-1">
                      {(treeNodes[rootId]?.childrenIds ?? []).map((childId) => {
                        const topNode = treeNodes[childId];
                        if (!topNode) return null;
                        return renderTreeNode(childId, 0);
                      })}
                    </div>
                  ))
                ) : (
                  <p className="text-sm text-slate-500">暂无文件。</p>
                )}
              </div>
            ) : treeLoading ? (
              <p className="text-sm text-slate-400">文件树加载中...</p>
            ) : (
              <p className="text-sm text-slate-500">选择左侧 Issue / Bundle 后自动加载文件树。</p>
            )}
          </div>

          <div className="rounded-lg border border-slate-800 bg-slate-900 p-4 text-sm text-slate-200 min-h-[80vh]">
            <div className="space-y-4">
              <div className="space-y-2">
                {searchExecuted && currentFilteredResults.length > 0 ? (
                  <div className="flex min-h-11 flex-wrap items-center gap-2 rounded-lg border border-slate-700 bg-slate-950/60 px-3 py-2 focus-within:border-cyan-500/60">
                    <span className="shrink-0 text-slate-500" aria-hidden="true">⌕</span>
                    <input
                      className="min-w-[180px] flex-1 bg-transparent text-sm text-white outline-none placeholder:text-slate-500"
                      placeholder="在当前结果中搜索..."
                      value={resultFilterText}
                      onChange={(event) => setResultFilterText(event.target.value)}
                      onKeyDown={(event) => {
                        if (event.key === 'Escape') {
                          event.preventDefault();
                          setResultFilterText('');
                        }
                      }}
                    />
                    <span className="shrink-0 text-xs text-slate-400">
                      {resultFilterText.trim()
                        ? `已筛选 ${visibleSearchResults.length} / ${currentFilteredResults.length} 条`
                        : `${currentFilteredResults.length} 条结果`}
                    </span>
                    {resultFilterText ? (
                      <button
                        type="button"
                        className="shrink-0 text-xs text-slate-400 transition hover:text-white"
                        onClick={() => setResultFilterText('')}
                      >
                        清空
                      </button>
                    ) : null}
                  </div>
                ) : null}

                {searchExecuted ? (
                  searchLoading && currentFilteredResults.length === 0 ? (
                    <p className="py-8 text-center text-sm text-slate-500">正在搜索...</p>
                  ) : currentFilteredResults.length === 0 ? (
                    <p className="py-8 text-center text-sm text-slate-500">未搜索到相关日志。</p>
                  ) : visibleSearchResults.length === 0 && resultFilterText.trim() ? (
                    <p className="py-8 text-center text-sm text-slate-500">
                      当前结果中没有包含「{resultFilterText.trim()}」的内容。
                    </p>
                  ) : searchMode === 'log' ? (
                    <ul className="space-y-2">
                      {visibleSearchResults.map((hit, index) => {
                        const targetId = hit.bundle_hash
                          ? `${hit.bundle_hash}:${hit.file_id}`
                          : '';
                        const selected = !!targetId && selectedNodeId === targetId;
                        return (
                          <li
                            key={`${hit.bundle_hash ?? 'b'}:${hit.file_id}:${index}`}
                            className={`cursor-pointer space-y-1 rounded-lg border bg-slate-950/70 p-3 transition ${
                              selected
                                ? 'border-cyan-500/60'
                                : 'border-slate-800 hover:border-slate-700'
                            }`}
                            onClick={() => {
                              if (!targetId) return;
                              setSelectedNodeId(targetId);
                              handleNodeClick(targetId, null).catch(() => undefined);
                            }}
                          >
                            <p className="truncate font-mono text-xs text-slate-100">
                              {highlightText(hit.snippet, resultFilterText)}
                            </p>
                            <p className="truncate text-[11px] text-slate-500">
                              {highlightText(formatHitPath(hit.path), resultFilterText)}
                            </p>
                          </li>
                        );
                      })}
                    </ul>
                  ) : (
                    <ul className="space-y-2">
                      {visibleSearchResults.map((hit, index) => {
                        const targetId = hit.bundle_hash
                          ? `${hit.bundle_hash}:${hit.file_id}`
                          : '';
                        const selected = !!targetId && selectedNodeId === targetId;
                        return (
                          <li
                            key={`${hit.bundle_hash ?? 'b'}:${hit.file_id}:${index}`}
                            className={`cursor-pointer space-y-2 rounded-lg border bg-slate-950/70 p-3 transition ${
                              selected
                                ? 'border-cyan-500/60'
                                : 'border-slate-800 hover:border-slate-700'
                            }`}
                            onClick={() => {
                              if (!targetId) return;
                              setSelectedNodeId(targetId);
                              handleNodeClick(targetId, hit.line_number ?? null).catch(() => undefined);
                            }}
                          >
                            <div className="flex min-w-0 items-center justify-between gap-3 text-[11px]">
                              <p className="min-w-0 truncate text-slate-400">
                                {highlightText(formatHitPath(hit.path), resultFilterText)}
                              </p>
                              <span className="shrink-0 text-slate-500">
                                {hit.line_number !== null && hit.line_number !== undefined
                                  ? `行 ${hit.line_number + 1}${
                                      hit.line_end !== null && hit.line_end !== undefined
                                        ? ` - ${hit.line_end + 1}`
                                        : ''
                                    }`
                                  : '行号未知'}
                              </span>
                            </div>
                            <pre className="truncate font-mono text-xs text-slate-100">
                              {highlightText(hit.snippet, resultFilterText)}
                            </pre>
                          </li>
                        );
                      })}
                    </ul>
                  )
                ) : !selectedNode ? (
                  <p className="py-8 text-center text-sm text-slate-500">
                    输入关键词搜索当前 Issue 的日志。
                  </p>
                ) : isArchiveNode(selectedNode) ? (
                  <p className="text-sm text-slate-500">压缩包请在左侧展开查看内部文件。</p>
                ) : selectedNode.is_dir ? (
                  <p className="text-sm text-slate-500">当前为目录，选择文件后展示内容。</p>
                ) : fileContentLoading ? (
                  <p className="text-sm text-slate-500">读取中...</p>
                ) : fileContentError ? (
                  <p className="text-sm text-rose-300">{fileContentError}</p>
                ) : fileLines ? (
                  <div className="space-y-2">
                    <div className="flex flex-wrap items-center gap-2 text-xs text-slate-300">
                      <button
                        type="button"
                        className="rounded border border-slate-700 px-3 py-1 hover:border-slate-500 disabled:opacity-50"
                        disabled={lineStart <= 0 || fileContentLoading}
                        onClick={() => setLineStart(Math.max(0, lineStart - LINE_PAGE_SIZE))}
                      >
                        上一页
                      </button>
                      <button
                        type="button"
                        className="rounded border border-slate-700 px-3 py-1 hover:border-slate-500 disabled:opacity-50"
                        disabled={!fileLines.next_start || fileContentLoading}
                        onClick={() => setLineStart(fileLines.next_start ?? lineStart + LINE_PAGE_SIZE)}
                      >
                        下一页
                      </button>
                      <span>
                        行 {fileLines.start + 1}
                        {fileLines.lines.length > 0 ? ` - ${fileLines.start + fileLines.lines.length}` : ''}
                        {fileLines.line_count ? ` / ${fileLines.line_count}` : ''}
                      </span>
                      <a
                        className="ml-auto rounded border border-slate-700 px-3 py-1 text-slate-200 hover:border-slate-500"
                        href={rainApi.fileDownloadUrl(selectedNode.bundleId || bundleId, selectedNode.rawId)}
                      >
                        下载原文件
                      </a>
                    </div>
                    <div
                      ref={contentRef}
                      className="h-[70vh] overflow-auto rounded bg-slate-950/70 p-3 text-xs leading-5 text-slate-100"
                    >
                      <div className="grid grid-cols-[auto_1fr] gap-3 font-mono">
                        <div className="select-none text-right text-slate-500">
                          {fileLines.lines.map((line) => (
                            <div key={line.line_number}>{line.line_number + 1}</div>
                          ))}
                        </div>
                        <div>
                          {fileLines.lines.map((line) => (
                            <div key={line.line_number} className="whitespace-pre">
                              {line.content}
                            </div>
                          ))}
                        </div>
                      </div>
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
