import { useCallback, useEffect, useState } from 'react';
import { useLocation, useParams } from 'react-router-dom';
import { rainApi } from '../../api/client';
import type { FileContentResponse, FileNode, IssueLogSearchHit } from '../../api/types';
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
  const [fileContent, setFileContent] = useState<FileContentResponse | null>(null);
  const [fileContentLoading, setFileContentLoading] = useState(false);
  const [fileContentError, setFileContentError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const [searchTerm, setSearchTerm] = useState('');
  const [searchResults, setSearchResults] = useState<IssueLogSearchHit[]>([]);
  const [searchLoading, setSearchLoading] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);

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
        setTreeError((error as Error).message || '加载文件树失败');
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
      return;
    }
    setSearchLoading(true);
    setSearchError(null);
    try {
      const response = await rainApi.searchIssueLogs(issue, keyword, { size: 500 });
      setSearchResults(response.hits);
    } catch (error) {
      setSearchResults([]);
      setSearchError((error as Error).message || '搜索失败');
    } finally {
      setSearchLoading(false);
    }
  }, [issueCode, searchTerm]);

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

      let bundles = fallbackBundles;
      if (issueCode) {
        try {
          const data = await rainApi.fetchIssueBundles(issueCode);
          const list = data.log_bundles.map((bundle) => ({ hash: bundle.hash, name: bundle.name || bundle.hash }));
          if (list.length > 0) {
            bundles = list;
          }
        } catch (error) {
          setTreeError((error as Error).message || '加载 Issue 失败');
        }
      }

      const expandedAll = new Set<string>();
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

          const queue = result.children.filter((child) => child.is_dir || isArchiveNode(child));
          while (queue.length > 0) {
            const current = queue.shift()!;
            const res = await loadNode(bundle.hash, current.rawId, current.parentId);
            if (res?.node && (res.node.is_dir || isArchiveNode(res.node))) {
              res.children
                .filter((child) => child.is_dir || isArchiveNode(child))
                .forEach((child) => queue.push(child));
            }
          }

          collectedRoots.push(result.node.id);
          if (!first) {
            first = result.node.childrenIds[0] ?? result.node.id;
          }
        } catch (error) {
          setTreeError((error as Error).message || '加载文件树失败');
        }
      }

      if (!ignore) {
        setExpandedNodes(expandedAll);
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
  }, [issueCode, refreshKey]);

  const handleNodeClick = async (nodeId: string) => {
    setSearchResults([]);
    setSearchError(null);
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
    setFileContent(null);
    setFileContentError(null);
    if (!selectedNode || selectedNode.is_dir) return;
    const bundleForContent = selectedNode.bundleId || bundleId;
    if (!bundleForContent) return;
    let ignore = false;
    const fetchContent = async () => {
      setFileContentLoading(true);
      try {
        const content = await rainApi.fetchFileContent(bundleForContent, selectedNode.rawId);
        if (!ignore) {
          setFileContent(content);
        }
      } catch (error) {
        if (!ignore) {
          setFileContentError((error as Error).message || '加载文件失败');
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
  }, [bundleId, selectedNode?.id, selectedNode?.is_dir]);

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
    <div className="space-y-6">
      <section className="panel space-y-4">
        {treeError ? <p className="text-sm text-rose-300">{treeError}</p> : null}

        <div className="grid gap-4 lg:grid-cols-[460px_minmax(0,1fr)]">
          <div className="space-y-3 rounded-lg border border-slate-800 bg-slate-900 p-3">
            <p className="text-xs text-slate-400">Issue: {activeIssueLabel}</p>
            <div className="space-y-2 rounded-lg border border-slate-800 bg-slate-950/60 p-3">
              <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:gap-3">
                <input
                  className="w-full rounded-lg border border-slate-700 bg-slate-900 px-3 py-2 text-sm text-white focus:border-brand-500 focus:outline-none"
                  placeholder="在当前 Issue 的所有文件内搜索内容"
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
                  className="rounded-lg bg-brand-500 px-4 py-2 text-sm font-semibold text-slate-900 transition hover:bg-brand-700 disabled:opacity-60"
                  onClick={() => runSearch().catch(() => undefined)}
                  disabled={searchLoading || !issueCode || !searchTerm.trim()}
                >
                  {searchLoading ? '搜索中...' : '搜索'}
                </button>
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
                {searchResults.length > 0 ? (
                  <pre className="whitespace-pre-wrap rounded-lg border border-slate-800 bg-slate-950/70 p-3 text-xs text-slate-100">
                    {searchResults.map((hit) => hit.snippet).join('\n')}
                  </pre>
                ) : !selectedNode ? (
                  <p className="text-sm text-slate-500">请选择一个文件查看内容。</p>
                ) : isArchiveNode(selectedNode) ? (
                  <p className="text-sm text-slate-500">压缩包请在左侧展开查看内部文件。</p>
                ) : selectedNode.is_dir ? (
                  <p className="text-sm text-slate-500">当前为目录，选择文件后展示内容。</p>
                ) : fileContentLoading ? (
                  <p className="text-sm text-slate-500">读取中...</p>
                ) : fileContentError ? (
                  <p className="text-sm text-rose-300">{fileContentError}</p>
                ) : fileContent ? (
                  <div className="space-y-2">
                    <pre className="h-[70vh] overflow-auto rounded bg-slate-950/70 p-3 text-xs text-slate-100">
                      {fileContent.preview}
                    </pre>
                    {fileContent.truncated ? (
                      <p className="text-xs text-amber-300">已截断预览（最多 64KB）。</p>
                    ) : null}
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
