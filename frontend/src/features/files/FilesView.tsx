import { FormEvent, useCallback, useEffect, useState } from 'react';
import { rainApi } from '../../api/client';
import type { FileContentResponse, FileNode, IssueBundlesResponse, UploadResponse } from '../../api/types';
import { StatusBadge } from '../../components/StatusBadge';
import type { BundleInfo } from '../../lib/bundles';

interface FilesViewProps {
  activeBundle: BundleInfo | null;
  onBundleSelected: (bundle: BundleInfo) => void;
}

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

export function FilesView({ activeBundle, onBundleSelected }: FilesViewProps) {
  const [issueId, setIssueId] = useState('');
  const [uploadIssueId, setUploadIssueId] = useState('');
  const [issueData, setIssueData] = useState<IssueBundlesResponse | null>(null);
  const [issueLoading, setIssueLoading] = useState(false);
  const [issueError, setIssueError] = useState<string | null>(null);

  const [selectedFiles, setSelectedFiles] = useState<FileList | null>(null);
  const [fileInputKey, setFileInputKey] = useState(0);
  const [uploading, setUploading] = useState(false);
  const [uploadError, setUploadError] = useState<string | null>(null);
  const [uploadSuccess, setUploadSuccess] = useState<UploadResponse | null>(null);

  const bundleId = activeBundle?.hash ?? '';
  const [treeNodes, setTreeNodes] = useState<Record<string, TreeNode>>({});
  const [expandedNodes, setExpandedNodes] = useState<Set<string>>(new Set());
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [treeLoading, setTreeLoading] = useState(false);
  const [treeError, setTreeError] = useState<string | null>(null);
  const [initializedBundleId, setInitializedBundleId] = useState<string>('');
  const [fileContent, setFileContent] = useState<FileContentResponse | null>(null);
  const [fileContentLoading, setFileContentLoading] = useState(false);
  const [fileContentError, setFileContentError] = useState<string | null>(null);

  const fetchIssueBundles = async (value: string) => {
    const trimmed = value.trim();
    if (!trimmed) return;
    setIssueLoading(true);
    setIssueError(null);
    try {
      const data = await rainApi.fetchIssueBundles(trimmed);
      setIssueData(data);
    } catch (error) {
      setIssueError((error as Error).message || '查询失败');
      setIssueData(null);
      throw error;
    } finally {
      setIssueLoading(false);
    }
  };

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
          setSelectedNodeId((rootNode as TreeNode).id);
          if ((rootNode as TreeNode).is_dir) {
            setExpandedNodes(new Set([(rootNode as TreeNode).id]));
          }
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

  const handleIssueLookup = async (event: FormEvent) => {
    event.preventDefault();
    if (!issueId.trim()) return;
    await fetchIssueBundles(issueId).catch(() => undefined);
  };

  const handleUpload = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!uploadIssueId.trim()) {
      setUploadError('请输入 Issue ID');
      return;
    }
    if (!selectedFiles || selectedFiles.length === 0) {
      setUploadError('请至少选择一个文件');
      return;
    }
    setUploading(true);
    setUploadError(null);
    setUploadSuccess(null);
    try {
      const response = await rainApi.uploadLogs(uploadIssueId.trim(), Array.from(selectedFiles));
      setUploadSuccess(response);
      setIssueId(response.issue_code);
      setUploadIssueId(response.issue_code);
      setSelectedFiles(null);
      setFileInputKey((key) => key + 1);
      await fetchIssueBundles(response.issue_code).catch(() => undefined);
      onBundleSelected({
        hash: response.bundle_hash,
        name: response.bundle_hash,
        issue: response.issue_code
      });
    } catch (error) {
      setUploadError((error as Error).message || '上传失败');
      setUploadSuccess(null);
    } finally {
      setUploading(false);
    }
  };

  const selectedNode = selectedNodeId ? treeNodes[selectedNodeId] : null;
  const selectedChildren = selectedNode?.childrenIds
    ?.map((childId) => treeNodes[childId])
    .filter((child): child is TreeNode => Boolean(child));

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
      <section className="panel space-y-6">
        <div className="flex flex-col gap-1 sm:flex-row sm:items-center sm:justify-between">
          <div>
            <p className="text-xs uppercase tracking-[0.2em] text-brand-500">Step 1</p>
            <h2 className="text-lg font-semibold text-white">创建 Issue / 上传文件</h2>
            <p className="text-sm text-slate-400">同一界面完成 Issue 创建与文件上传，上传完成后自动跳转到对应文件结构。</p>
          </div>
          {uploadSuccess ? (
            <div className="rounded-lg border border-emerald-600/40 bg-emerald-500/10 px-4 py-3 text-xs text-emerald-200">
              <p className="font-semibold">上传成功</p>
              <p>Issue：{uploadSuccess.issue_code}</p>
              <p className="font-mono text-[11px] text-emerald-100">{uploadSuccess.bundle_hash}</p>
              <p>文件 {uploadSuccess.file_count} 个 · 共 {(uploadSuccess.total_bytes / 1024).toFixed(1)} KB</p>
            </div>
          ) : null}
        </div>

        <div className="grid gap-4 lg:grid-cols-[1.4fr_minmax(0,1fr)]">
          <form onSubmit={handleUpload} className="space-y-3 rounded-lg border border-slate-800 bg-slate-900/60 p-4">
            <label className="block text-sm text-slate-300">
              Issue ID
              <input
                className="mt-1 w-full rounded-lg border border-slate-700 bg-slate-950 px-4 py-2 text-white focus:border-brand-500 focus:outline-none"
                placeholder="例如 CN013（若不存在将自动创建）"
                value={uploadIssueId}
                onChange={(event) => setUploadIssueId(event.target.value)}
              />
            </label>
            <div className="space-y-2">
              <label className="block text-sm text-slate-300">上传日志 / 压缩包</label>
              <input
                key={fileInputKey}
                type="file"
                multiple
                onChange={(event) => setSelectedFiles(event.target.files)}
                className="w-full rounded-lg border border-dashed border-slate-700 bg-slate-950/60 px-4 py-2 text-sm text-slate-300 file:mr-4 file:rounded-md file:border-0 file:bg-brand-500 file:px-3 file:py-1 file:text-sm file:font-semibold file:text-slate-900"
              />
              <p className="text-xs text-slate-500">
                {selectedFiles?.length
                  ? `已选择 ${selectedFiles.length} 个文件`
                  : '支持 .log/.txt/.zip，压缩包会自动解压成目录结构。'}
              </p>
            </div>
            <div className="flex items-center justify-between text-xs text-slate-500">
              <span>新建 Issue 将自动写入数据库并生成 bundle。</span>
              <button
                type="submit"
                className="rounded-lg bg-brand-500 px-5 py-2 text-sm font-semibold text-slate-900 transition hover:bg-brand-700 disabled:opacity-60"
                disabled={uploading}
              >
                {uploading ? '上传中...' : '上传并进入'}
              </button>
            </div>
            {uploadError ? <p className="text-sm text-rose-300">{uploadError}</p> : null}
          </form>

          <div className="space-y-3 rounded-lg border border-dashed border-slate-800 bg-slate-900/40 p-4">
            <p className="text-sm font-semibold text-white">操作提示</p>
            <ul className="space-y-2 text-sm text-slate-300">
              <li>1. 输入想要的 Issue ID，系统不存在时会自动创建。</li>
              <li>2. 选择单个文件或压缩包，可一次选择多个条目。</li>
              <li>3. 点击“上传并进入”后，会自动定位到该 Issue / Bundle。</li>
            </ul>
            <div className="text-xs text-slate-500">
              需要查看历史上传？在下方输入 Issue ID 即可列出所有 bundle，点击即可进入子界面。
            </div>
          </div>
        </div>
      </section>

      <section className="panel space-y-4">
        <div className="flex flex-col gap-1 sm:flex-row sm:items-center sm:justify-between">
          <div>
            <p className="text-xs uppercase tracking-[0.2em] text-brand-500">Step 2</p>
            <h2 className="text-lg font-semibold text-white">选择 Issue / Bundle</h2>
            <p className="text-sm text-slate-400">输入 Issue ID 列出上传记录，点击即可进入文件结构子界面。</p>
          </div>
          {issueData ? (
            <span className="rounded-full bg-slate-900 px-3 py-1 text-xs text-slate-300">
              {issueData.log_bundles.length} 个 bundle
            </span>
          ) : null}
        </div>

        <form onSubmit={handleIssueLookup} className="flex flex-col gap-3 sm:flex-row">
          <input
            className="flex-1 rounded-lg border border-slate-700 bg-slate-900 px-4 py-2 text-white focus:border-brand-500 focus:outline-none"
            placeholder="Issue ID，例如 CN013"
            value={issueId}
            onChange={(event) => setIssueId(event.target.value)}
          />
          <button
            type="submit"
            className="rounded-lg bg-brand-500 px-6 py-2 font-semibold text-slate-900 transition hover:bg-brand-700"
            disabled={issueLoading}
          >
            {issueLoading ? '加载中...' : '查询'}
          </button>
        </form>

        {issueError ? <p className="text-sm text-rose-300">{issueError}</p> : null}

        {issueData ? (
          <div className="grid gap-3 md:grid-cols-2">
            {issueData.log_bundles.map((bundle) => {
              const isActive = bundle.hash === bundleId;
              return (
                <button
                  key={bundle.hash}
                  type="button"
                  onClick={() =>
                    onBundleSelected({
                      hash: bundle.hash,
                      name: bundle.name,
                      issue: issueData?.name ?? issueId
                    })
                  }
                  className={[
                    'flex flex-col gap-2 rounded-lg border px-4 py-3 text-left transition',
                    isActive
                      ? 'border-brand-500/70 bg-slate-900 shadow-sm'
                      : 'border-slate-800 bg-slate-900/50 hover:border-slate-700 hover:bg-slate-900/70'
                  ].join(' ')}
                >
                  <div className="flex items-start justify-between gap-3">
                    <div className="min-w-0 space-y-1">
                      <p className="truncate text-base font-semibold text-white">
                        {bundle.name}
                        {isActive ? <span className="ml-2 text-xs text-brand-400">当前</span> : null}
                      </p>
                      <p className="truncate font-mono text-[11px] text-slate-500">{bundle.hash}</p>
                    </div>
                    <StatusBadge status={bundle.status.upload_status} />
                  </div>
                  <p className="text-xs text-slate-500">点击进入，展开文件树并预览上传文件。</p>
                </button>
              );
            })}
          </div>
        ) : (
          <div className="rounded-lg border border-dashed border-slate-800 bg-slate-950/40 p-4 text-sm text-slate-500">
            上传完成或查询 Issue 后，这里会展示对应的 bundle 列表。
          </div>
        )}
      </section>

      <section className="panel space-y-4">
        <div className="flex flex-col gap-1 sm:flex-row sm:items-center sm:justify-between">
          <div>
            <p className="text-xs uppercase tracking-[0.2em] text-brand-500">Step 3</p>
            <h2 className="text-lg font-semibold text-white">文件结构 / 预览</h2>
            <p className="text-sm text-slate-400">
              选择 Issue 后进入子界面：左侧用图标区分目录 / 压缩包 / 文件，右侧展示文本预览；二进制则提示不支持。
            </p>
          </div>
          {bundleId ? (
            <div className="rounded-lg border border-slate-800 bg-slate-900/70 px-3 py-2 text-xs text-slate-300">
              当前 Bundle: <span className="font-mono text-slate-200">{bundleId}</span>
            </div>
          ) : null}
        </div>

        {treeError ? <p className="text-sm text-rose-300">{treeError}</p> : null}

        <div className="grid gap-6 lg:grid-cols-[320px_minmax(0,1fr)]">
          <div className="rounded-lg border border-slate-800 bg-slate-900 p-3">
            {bundleId ? (
              treeNodes['root'] ? (
                <div className="space-y-2 text-sm text-slate-200">
                  <p className="mb-2 text-xs text-slate-500">
                    点击目录展开子节点；ZIP 会被拆成多级目录。
                  </p>
                  {renderTreeNode('root')}
                </div>
              ) : treeLoading ? (
                <p className="text-sm text-slate-400">文件树加载中...</p>
              ) : (
                <p className="text-sm text-slate-500">选择上方的 bundle 后自动加载文件树。</p>
              )
            ) : (
              <p className="text-sm text-slate-500">先在上方选择 Issue / Bundle。</p>
            )}
          </div>

          <div className="rounded-lg border border-slate-800 bg-slate-900 p-4 text-sm text-slate-200">
            {selectedNode ? (
              <div className="space-y-4">
                <div className="flex items-start justify-between gap-4">
                  <div className="min-w-0 space-y-1">
                    <p className="text-xs uppercase text-slate-500">当前节点</p>
                    <p className="truncate text-lg font-semibold text-white">{selectedNode.name}</p>
                    <p className="break-all text-xs text-slate-500">{selectedNode.path}</p>
                  </div>
                  <span
                    className={[
                      'rounded-full px-3 py-1 text-xs font-semibold',
                      selectedNode.is_dir
                        ? 'bg-amber-500/20 text-amber-200'
                        : isArchiveNode(selectedNode)
                          ? 'bg-brand-500/20 text-brand-100'
                          : 'bg-slate-800 text-slate-200'
                    ].join(' ')}
                  >
                    {nodeTypeLabel(selectedNode)}
                  </span>
                </div>
                <p className="text-sm text-slate-400">
                  {selectedNode.is_dir ? '目录' : '文件'} · {formatSize(selectedNode.size_bytes)}
                </p>
                {selectedNode.meta ? (
                  <pre className="max-h-48 overflow-auto rounded bg-slate-950/70 p-3 text-xs text-slate-300">
                    {JSON.stringify(selectedNode.meta, null, 2)}
                  </pre>
                ) : null}

                {!selectedNode.is_dir ? (
                  <div className="space-y-2">
                    <h3 className="text-sm font-semibold text-slate-200">文件预览</h3>
                    {fileContentLoading ? (
                      <p className="text-sm text-slate-500">读取中...</p>
                    ) : fileContentError ? (
                      <p className="text-sm text-rose-300">{fileContentError}</p>
                    ) : fileContent ? (
                      canPreviewAsText(selectedNode, fileContent) ? (
                        <div className="space-y-2">
                          <pre className="max-h-80 overflow-auto rounded bg-slate-950/70 p-3 text-xs text-slate-100">
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
                  <p className="text-sm text-slate-500">选择文件后，右侧会自动展示文本内容。</p>
                )}

                <div className="rounded-lg border border-slate-800 bg-slate-950/40 p-3">
                  <div className="flex items-center justify-between text-sm text-slate-300">
                    <h3 className="font-semibold">子节点</h3>
                    <span className="text-xs text-slate-500">{selectedChildren?.length ?? 0} 项</span>
                  </div>
                  {selectedNode.is_dir ? (
                    selectedChildren && selectedChildren.length > 0 ? (
                      <ul className="mt-2 divide-y divide-slate-800">
                        {selectedChildren.map((child) => (
                          <li key={child.id} className="flex items-center gap-3 py-2">
                            <span
                              className={`flex h-8 w-8 items-center justify-center rounded border text-[10px] font-semibold ${
                                child.is_dir
                                  ? 'border-amber-400/60 text-amber-200'
                                  : isArchiveNode(child)
                                    ? 'border-brand-400/60 text-brand-200'
                                    : 'border-slate-600 text-slate-300'
                              }`}
                            >
                              {child.is_dir ? 'DIR' : isArchiveNode(child) ? 'ZIP' : 'FILE'}
                            </span>
                            <div className="min-w-0">
                              <p className="truncate text-sm text-white">{child.name}</p>
                              <p className="text-xs uppercase text-slate-500">
                                {child.is_dir ? '目录' : child.mime_type ?? 'file'}
                              </p>
                            </div>
                            <span className="ml-auto text-xs text-slate-500">
                              {child.is_dir ? '--' : formatSize(child.size_bytes)}
                            </span>
                          </li>
                        ))}
                      </ul>
                    ) : (
                      <p className="mt-2 text-sm text-slate-500">
                        {selectedNode.hasLoadedChildren ? '暂无子节点。' : '展开目录即可加载子节点。'}
                      </p>
                    )
                  ) : (
                    <p className="mt-2 text-sm text-slate-500">选择目录可查看其子节点。</p>
                  )}
                </div>
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
