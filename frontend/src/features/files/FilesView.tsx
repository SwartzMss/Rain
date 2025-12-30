import { useCallback, useEffect, useState } from 'react';
import { useLocation, useNavigate, useParams } from 'react-router-dom';
import { rainApi } from '../../api/client';
import type { FileContentResponse, FileNode } from '../../api/types';
import type { BundleInfo } from '../../lib/bundles';

type TreeNode = Omit<FileNode, 'id' | 'children'> & {
  id: string;
  parentId: string | null;
  childrenIds: string[];
  hasLoadedChildren: boolean;
};

const archivePattern = /\.(zip|tar|gz|tgz|rar|7z)$/i;
const textualMimeHints = ['text', 'json', 'xml', 'yaml', 'yml', 'html', 'csv', 'javascript', 'typescript', 'css', 'shell', 'markdown'];

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

const canPreviewAsText = (node: TreeNode | null, content: FileContentResponse | null) => {
  const mime = (content?.mime_type ?? node?.mime_type ?? '').toLowerCase();
  if (mime && textualMimeHints.some((hint) => mime.includes(hint))) {
    return true;
  }
  if (content?.preview) {
    const sample = content.preview.slice(0, 2000);
    const controlMatches = sample.match(/[^\x09\x0A\x0D\x20-\x7E]/g);
    if (!controlMatches) return true;
    const ratio = controlMatches.length / sample.length;
    return ratio < 0.05;
  }
  return false;
};

const nodeTypeLabel = (node: TreeNode) => {
  if (node.is_dir) return '目录';
  if (isArchiveNode(node)) return '压缩包';
  return '文件';
};

export function BundleView() {
  const { bundleHash = '' } = useParams<{ bundleHash: string }>();
  const location = useLocation();
  const navigate = useNavigate();
  const locationState = (location.state as { issue?: string; bundleName?: string } | null) ?? null;

  const activeBundle: BundleInfo = {
    hash: bundleHash,
    name: locationState?.bundleName || bundleHash,
    issue: locationState?.issue
  };

  const bundleId = activeBundle.hash || '';
  const [treeNodes, setTreeNodes] = useState<Record<string, TreeNode>>({});
  const [expandedNodes, setExpandedNodes] = useState<Set<string>>(new Set());
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [treeLoading, setTreeLoading] = useState(false);
  const [treeError, setTreeError] = useState<string | null>(null);
  const [initializedBundleId, setInitializedBundleId] = useState<string>('');
  const [fileContent, setFileContent] = useState<FileContentResponse | null>(null);
  const [fileContentLoading, setFileContentLoading] = useState(false);
  const [fileContentError, setFileContentError] = useState<string | null>(null);

  const toTreeNode = (node: FileNode, parentId: string | null = null): TreeNode => ({
    id: node.id.toString(),
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
    async (bundle: string, nodeId: string, parentId: string | null = null): Promise<TreeNode | null> => {
      if (!bundle) return null;
      setTreeLoading(true);
      setTreeError(null);
      try {
        const response = await rainApi.fetchFileNode(bundle, nodeId);
        let normalized: TreeNode | null = null;

        setTreeNodes((prev) => {
          const next = { ...prev };
          const key = response.node.id.toString();
          const inferredParent = next[key]?.parentId ?? parentId ?? null;
          const base = toTreeNode(response.node, inferredParent);
          base.hasLoadedChildren = true;
          base.childrenIds = (response.children ?? []).map((child) => child.id.toString());
          normalized = base;
          next[key] = base;

          (response.children ?? []).forEach((child) => {
            const childId = child.id.toString();
            next[childId] = toTreeNode(child, base.id);
          });

          return next;
        });

        return normalized;
      } catch (error) {
        setTreeError((error as Error).message || '加载文件树失败');
        throw error;
      } finally {
        setTreeLoading(false);
      }
    },
    []
  );

  useEffect(() => {
    const selectedBundle = bundleId;
    if (!selectedBundle) {
      setTreeNodes({});
      setExpandedNodes(new Set());
      setSelectedNodeId(null);
      setInitializedBundleId('');
      setTreeError(null);
      return;
    }
    if (selectedBundle === initializedBundleId) {
      return;
    }
    let ignore = false;
    const init = async () => {
      setTreeNodes({});
      setExpandedNodes(new Set());
      setSelectedNodeId(null);
      setTreeError(null);
      try {
        const rootNode = await loadNode(selectedBundle, 'root', null);
        if (!ignore && rootNode) {
          const rootTree = rootNode as TreeNode;
          const firstChild = rootTree.childrenIds[0];
          setExpandedNodes(new Set([rootTree.id]));
          setSelectedNodeId(firstChild ?? null);
        }
      } catch {
        // error handled
      } finally {
        if (!ignore) {
          setInitializedBundleId(selectedBundle);
        }
      }
    };
    init();
    return () => {
      ignore = true;
    };
  }, [bundleId, initializedBundleId, loadNode]);

  const handleNodeClick = async (nodeId: string) => {
    if (!bundleId) return;
    let node: TreeNode | null = treeNodes[nodeId] ?? null;
    if (!node) {
      node = await loadNode(bundleId, nodeId, null);
    }
    if (!node) return;

    if (node.is_dir) {
      if (!node.hasLoadedChildren) {
        await loadNode(bundleId, node.id, node.parentId);
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
  const selectedChildren = selectedNode?.childrenIds
    ?.map((childId) => treeNodes[childId])
    .filter((child): child is TreeNode => Boolean(child));

  const activeIssueLabel = activeBundle.issue || '未知 Issue';
  const activeNodeLabel = selectedNode?.name || '未选择文件';

  useEffect(() => {
    setFileContent(null);
    setFileContentError(null);
    if (!bundleId || !selectedNode || selectedNode.is_dir) return;
    let ignore = false;
    const fetchContent = async () => {
      setFileContentLoading(true);
      try {
        const content = await rainApi.fetchFileContent(bundleId, selectedNode.id);
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

  const renderTreeNode = (nodeId: string, depth = 0): JSX.Element | null => {
    const node = treeNodes[nodeId];
    if (!node) return null;
    const isExpanded = expandedNodes.has(nodeId);
    const isSelected = selectedNodeId === nodeId;
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
              {node.is_dir ? `${node.childrenIds.length || 0} 子节点` : node.mime_type ?? 'file'}
            </p>
          </div>
          <span className="ml-auto text-xs text-slate-500">
            {node.is_dir ? (isExpanded ? '收起' : '展开') : formatSize(node.size_bytes)}
          </span>
        </button>
        {node.is_dir && isExpanded ? (
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

        <div className="grid gap-4 lg:grid-cols-[320px_minmax(0,1fr)]">
          <div className="space-y-3 rounded-lg border border-slate-800 bg-slate-900 p-3">
            <p className="text-xs text-slate-400">Issue: {activeIssueLabel}</p>
            {bundleId ? (
              treeNodes['root'] ? (
                <div className="space-y-2 text-sm text-slate-200">
                  {(treeNodes['root'].childrenIds.length ?? 0) > 0 ? (
                    treeNodes['root'].childrenIds.map((childId) => renderTreeNode(childId, 0))
                  ) : (
                    <p className="text-sm text-slate-500">暂无文件。</p>
                  )}
                </div>
              ) : treeLoading ? (
                <p className="text-sm text-slate-400">文件树加载中...</p>
              ) : (
                <p className="text-sm text-slate-500">选择左侧 Issue / Bundle 后自动加载文件树。</p>
              )
            ) : (
              <p className="text-sm text-slate-500">先选择 Issue / Bundle。</p>
            )}
          </div>

          <div className="rounded-lg border border-slate-800 bg-slate-900 p-4 text-sm text-slate-200">
            {selectedNode ? (
              <div className="space-y-4">
                {!selectedNode.is_dir ? (
                  <div className="space-y-2">
                    {fileContentLoading ? (
                      <p className="text-sm text-slate-500">读取中...</p>
                    ) : fileContentError ? (
                      <p className="text-sm text-rose-300">{fileContentError}</p>
                    ) : fileContent ? (
                      canPreviewAsText(selectedNode, fileContent) ? (
                        <div className="space-y-2">
                          <pre className="max-h-[70vh] overflow-auto rounded bg-slate-950/70 p-3 text-xs text-slate-100">
                            {fileContent.preview}
                          </pre>
                          {fileContent.truncated ? (
                            <p className="text-xs text-amber-300">已截断预览（最多 64KB）。</p>
                          ) : null}
                        </div>
                      ) : (
                        <p className="rounded border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-sm text-amber-100">
                          检测到二进制或非文本文件，暂不支持预览，建议下载到本地查看。
                        </p>
                      )
                    ) : (
                      <p className="text-sm text-slate-500">选择文件即可加载内容。</p>
                    )}
                  </div>
                ) : (
                  <p className="text-sm text-slate-500">选择文件后展示内容。</p>
                )}

              </div>
            ) : (
              <p className="text-sm text-slate-500">请选择一个 Issue / Bundle 并点击文件树中的节点。</p>
            )}
          </div>
        </div>
      </section>
    </div>
  );
}
