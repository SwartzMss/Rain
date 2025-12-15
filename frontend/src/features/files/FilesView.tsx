import { FormEvent, useCallback, useEffect, useState } from 'react';
import { rainApi } from '../../api/client';
import type { FileNode, IssueBundlesResponse, UploadResponse } from '../../api/types';
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

export function FilesView({ activeBundle, onBundleSelected }: FilesViewProps) {
  const [issueId, setIssueId] = useState('');
  const [uploadIssueId, setUploadIssueId] = useState('');
  const [issueData, setIssueData] = useState<IssueBundlesResponse | null>(null);
  const [issueLoading, setIssueLoading] = useState(false);
  const [issueError, setIssueError] = useState<string | null>(null);

  const [bundleName, setBundleName] = useState('');
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
      const response = await rainApi.uploadLogs(
        uploadIssueId.trim(),
        Array.from(selectedFiles),
        bundleName.trim() || undefined
      );
      setUploadSuccess(response);
      setIssueId(response.issue_code);
      setUploadIssueId(response.issue_code);
      setBundleName('');
      setSelectedFiles(null);
      setFileInputKey((key) => key + 1);
      await fetchIssueBundles(response.issue_code).catch(() => undefined);
      onBundleSelected({
        hash: response.bundle_hash,
        name: response.bundle_name,
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

  const renderTreeNode = (nodeId: string, depth = 0): JSX.Element | null => {
    const node = treeNodes[nodeId];
    if (!node) return null;
    const isExpanded = expandedNodes.has(nodeId);
    const isSelected = selectedNodeId === nodeId;

    return (
      <div key={nodeId}>
        <button
          type="button"
          onClick={() => handleNodeClick(node.id).catch(() => undefined)}
          className={[
            'flex w-full items-center gap-2 rounded px-2 py-1 text-left text-sm transition',
            isSelected ? 'bg-slate-800/80 text-white' : 'hover:bg-slate-800/40'
          ].join(' ')}
          style={{ paddingLeft: `${depth * 12}px` }}
        >
          <span className="text-xs text-slate-500">
            {node.is_dir ? (isExpanded ? '▾' : '▸') : '•'}
          </span>
          <span className="truncate">{node.name}</span>
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
        <header>
          <h2 className="text-lg font-semibold text-white">上传日志</h2>
          <p className="text-sm text-slate-400">上传 `.log/.txt/.zip` 等文件，系统会自动创建 Issue 与 Bundle 并写入数据库。</p>
        </header>

        <form onSubmit={handleUpload} className="grid gap-3 md:grid-cols-2">
          <input
            className="rounded-lg border border-slate-700 bg-slate-900 px-4 py-2 text-white focus:border-brand-500 focus:outline-none"
            placeholder="Issue ID，例如 CN013（若不存在将自动创建）"
            value={uploadIssueId}
            onChange={(event) => setUploadIssueId(event.target.value)}
          />
          <input
            className="rounded-lg border border-slate-700 bg-slate-900 px-4 py-2 text-white focus:border-brand-500 focus:outline-none"
            placeholder="Bundle 名称，可选"
            value={bundleName}
            onChange={(event) => setBundleName(event.target.value)}
          />
          <div className="md:col-span-2">
            <input
              key={fileInputKey}
              type="file"
              multiple
              onChange={(event) => setSelectedFiles(event.target.files)}
              className="w-full rounded-lg border border-dashed border-slate-700 bg-slate-950/60 px-4 py-2 text-sm text-slate-300 file:mr-4 file:rounded-md file:border-0 file:bg-brand-500 file:px-3 file:py-1 file:text-sm file:font-semibold file:text-slate-900"
            />
            <p className="mt-1 text-xs text-slate-500">
              {selectedFiles?.length ? `已选择 ${selectedFiles.length} 个文件` : '支持一次选择多个日志文件，将按原始文件名存储'}
            </p>
          </div>
          <div className="md:col-span-2 text-right">
            <button
              type="submit"
              className="rounded-lg bg-brand-500 px-6 py-2 font-semibold text-slate-900 transition hover:bg-brand-700 disabled:opacity-60"
              disabled={uploading}
            >
              {uploading ? '上传中...' : '开始上传'}
            </button>
          </div>
        </form>

        {uploadError ? <p className="text-sm text-rose-300">{uploadError}</p> : null}
        {uploadSuccess ? (
          <div className="rounded-lg border border-emerald-600/40 bg-emerald-500/10 p-4 text-sm text-emerald-200">
            <p className="font-semibold">上传成功</p>
            <p>Issue：{uploadSuccess.issue_code}</p>
            <p>
              Bundle：{uploadSuccess.bundle_name}（ID: <code className="text-white">{uploadSuccess.bundle_hash}</code>）
            </p>
            <p>
              文件数量：{uploadSuccess.file_count}，共 {(uploadSuccess.total_bytes / 1024).toFixed(1)} KB
            </p>
            <p className="text-xs text-emerald-300/80">已自动定位到该 bundle，可在下方文件树/日志视图中浏览。</p>
          </div>
        ) : null}
      </section>

      <section className="panel space-y-4">
        <header>
          <h2 className="text-lg font-semibold text-white">案件信息 / Bundles</h2>
          <p className="text-sm text-slate-400">通过 issueId 查找上传历史，点击某个 bundle 即可展开文件树。</p>
        </header>

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
          <div className="space-y-2">
            <p className="text-sm text-slate-400">共 {issueData.log_bundles.length} 个 bundle：</p>
            <ul className="divide-y divide-slate-800 overflow-hidden rounded-lg border border-slate-800">
              {issueData.log_bundles.map((bundle) => {
                const isActive = bundle.hash === bundleId;
                return (
                  <li key={bundle.hash}>
                    <button
                      type="button"
                      onClick={() =>
                        onBundleSelected({
                          hash: bundle.hash,
                          name: bundle.name,
                          issue: issueData?.name ?? issueId
                        })
                      }
                      className={[
                        'flex w-full items-center justify-between px-4 py-3 text-left transition',
                        isActive ? 'bg-slate-900 text-white' : 'hover:bg-slate-900/40'
                      ].join(' ')}
                    >
                      <div>
                        <p className="font-medium">
                          {bundle.name}
                          {isActive ? <span className="ml-2 text-xs text-brand-400">（当前）</span> : null}
                        </p>
                        <p className="text-xs text-slate-500">{bundle.hash}</p>
                      </div>
                      <StatusBadge status={bundle.status.upload_status} />
                    </button>
                  </li>
                );
              })}
            </ul>
          </div>
        ) : (
          <p className="text-sm text-slate-500">暂无 bundle 数据，请上传文件或输入 issueId 并点击查询。</p>
        )}
      </section>

      <section className="panel space-y-4">
        <header>
          <h2 className="text-lg font-semibold text-white">文件树浏览</h2>
          <p className="text-sm text-slate-400">
            从左侧树结构选择目录/文件，可查看节点元数据与子节点；ZIP 文件会自动解压并展开成多级目录。
          </p>
        </header>

        {treeError ? <p className="text-sm text-rose-300">{treeError}</p> : null}

        <div className="grid gap-6 lg:grid-cols-[300px_minmax(0,1fr)]">
          <div className="rounded-lg border border-slate-800 bg-slate-900 p-3">
            {bundleId ? (
              treeNodes['root'] ? (
                <div className="space-y-2 text-sm text-slate-200">
                  <p className="mb-2 text-xs text-slate-500">
                    Bundle: <span className="font-mono text-slate-300">{bundleId}</span>
                  </p>
                  {renderTreeNode('root')}
                </div>
              ) : treeLoading ? (
                <p className="text-sm text-slate-400">文件树加载中...</p>
              ) : (
                <p className="text-sm text-slate-500">点击上方 bundle 后即可加载文件树。</p>
              )
            ) : (
              <p className="text-sm text-slate-500">先在上方选择一个 bundle。</p>
            )}
          </div>

          <div className="rounded-lg border border-slate-800 bg-slate-900 p-4 text-sm text-slate-200">
            {selectedNode ? (
              <div className="space-y-4">
                <div>
                  <p>
                    <span className="text-slate-500">节点：</span>
                    {selectedNode.name} ({selectedNode.path})
                  </p>
                  <p className="text-slate-400">
                    {selectedNode.is_dir ? '目录' : '文件'} · {selectedNode.size_bytes ?? 0} bytes
                  </p>
                  {selectedNode.meta ? (
                    <pre className="mt-3 max-h-48 overflow-auto rounded bg-slate-950/70 p-3 text-xs text-slate-300">
                      {JSON.stringify(selectedNode.meta, null, 2)}
                    </pre>
                  ) : null}
                </div>

                <div>
                  <h3 className="text-sm font-semibold text-slate-300">子节点</h3>
                  {selectedNode.is_dir ? (
                    selectedChildren && selectedChildren.length > 0 ? (
                      <table className="mt-2 w-full text-left text-sm text-slate-300">
                        <thead className="text-xs uppercase tracking-widest text-slate-500">
                          <tr>
                            <th className="pb-2">名称</th>
                            <th className="pb-2">类型</th>
                            <th className="pb-2">大小</th>
                          </tr>
                        </thead>
                        <tbody className="divide-y divide-slate-800">
                          {selectedChildren.map((child) => (
                            <tr key={child.id}>
                              <td className="py-2">{child.name}</td>
                              <td className="py-2 text-xs uppercase text-slate-500">
                                {child.is_dir ? 'DIR' : child.mime_type ?? 'FILE'}
                              </td>
                              <td className="py-2">{child.size_bytes ?? '--'}</td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    ) : (
                      <p className="text-sm text-slate-500">
                        {selectedNode.hasLoadedChildren ? '暂无子节点。' : '展开目录即可加载子节点。'}
                      </p>
                    )
                  ) : (
                    <p className="text-sm text-slate-500">选择目录可查看其子节点。</p>
                  )}
                </div>
              </div>
            ) : (
              <p className="text-sm text-slate-500">请选择一个 bundle 并点击文件树中的节点。</p>
            )}
          </div>
        </div>
      </section>
    </div>
  );
}
