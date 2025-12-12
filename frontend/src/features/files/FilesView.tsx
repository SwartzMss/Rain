import { FormEvent, useState } from 'react';
import { rainApi } from '../../api/client';
import type { FileNodeResponse, IssueBundlesResponse } from '../../api/types';
import { StatusBadge } from '../../components/StatusBadge';

export function FilesView() {
  const [issueId, setIssueId] = useState('CN013');
  const [issueData, setIssueData] = useState<IssueBundlesResponse | null>(null);
  const [issueLoading, setIssueLoading] = useState(false);
  const [issueError, setIssueError] = useState<string | null>(null);

  const [bundleId, setBundleId] = useState('lp1yp7');
  const [fileId, setFileId] = useState('root');
  const [nodeResponse, setNodeResponse] = useState<FileNodeResponse | null>(null);
  const [nodeLoading, setNodeLoading] = useState(false);
  const [nodeError, setNodeError] = useState<string | null>(null);

  const handleIssueLookup = async (event: FormEvent) => {
    event.preventDefault();
    if (!issueId.trim()) return;
    setIssueLoading(true);
    setIssueError(null);
    try {
      const data = await rainApi.fetchIssueBundles(issueId.trim());
      setIssueData(data);
    } catch (error) {
      setIssueError((error as Error).message || '查询失败');
      setIssueData(null);
    } finally {
      setIssueLoading(false);
    }
  };

  const handleFileLookup = async (event: FormEvent) => {
    event.preventDefault();
    if (!bundleId.trim() || !fileId.trim()) return;
    setNodeLoading(true);
    setNodeError(null);
    try {
      const response = await rainApi.fetchFileNode(bundleId.trim(), fileId.trim());
      setNodeResponse(response);
    } catch (error) {
      setNodeError((error as Error).message || '加载文件失败');
      setNodeResponse(null);
    } finally {
      setNodeLoading(false);
    }
  };

  return (
    <div className="space-y-6">
      <section className="panel space-y-4">
        <header>
          <h2 className="text-lg font-semibold text-white">案件信息 / Bundles</h2>
          <p className="text-sm text-slate-400">通过 issueId 查找上传历史，快速定位 bundle ID。</p>
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
            <ul className="divide-y divide-slate-800 rounded-lg border border-slate-800">
              {issueData.log_bundles.map((bundle) => (
                <li key={bundle.hash} className="flex items-center justify-between px-4 py-3">
                  <div>
                    <p className="font-medium text-white">{bundle.name}</p>
                    <p className="text-xs text-slate-500">{bundle.hash}</p>
                  </div>
                  <StatusBadge status={bundle.status.upload_status} />
                </li>
              ))}
            </ul>
          </div>
        ) : (
          <p className="text-sm text-slate-500">暂无 bundle 数据，输入 issueId 并点击查询。</p>
        )}
      </section>

      <section className="panel space-y-4">
        <header>
          <h2 className="text-lg font-semibold text-white">文件树浏览</h2>
          <p className="text-sm text-slate-400">指定 bundleId + fileId 获取节点元数据与子节点。</p>
        </header>

        <form onSubmit={handleFileLookup} className="grid gap-3 md:grid-cols-[1fr_1fr_auto]">
          <input
            className="rounded-lg border border-slate-700 bg-slate-900 px-4 py-2 text-white focus:border-brand-500 focus:outline-none"
            placeholder="bundleId"
            value={bundleId}
            onChange={(event) => setBundleId(event.target.value)}
          />
          <input
            className="rounded-lg border border-slate-700 bg-slate-900 px-4 py-2 text-white focus:border-brand-500 focus:outline-none"
            placeholder="fileId，例如 root 或数值 ID"
            value={fileId}
            onChange={(event) => setFileId(event.target.value)}
          />
          <button
            type="submit"
            className="rounded-lg bg-brand-500 px-6 py-2 font-semibold text-slate-900 transition hover:bg-brand-700"
            disabled={nodeLoading}
          >
            {nodeLoading ? '加载中...' : '获取'}
          </button>
        </form>

        {nodeError ? <p className="text-sm text-rose-300">{nodeError}</p> : null}

        {nodeResponse ? (
          <div className="space-y-3">
            <div className="rounded-lg bg-slate-900 p-4 text-sm text-slate-200">
              <p>
                <span className="text-slate-500">节点：</span>
                {nodeResponse.node.name} ({nodeResponse.node.path})
              </p>
              <p className="text-slate-400">
                {nodeResponse.node.is_dir ? '目录' : '文件'} · {nodeResponse.node.size_bytes ?? 0} bytes
              </p>
              {nodeResponse.node.meta ? (
                <pre className="mt-3 overflow-auto rounded bg-slate-950/70 p-3 text-xs text-slate-300">
                  {JSON.stringify(nodeResponse.node.meta, null, 2)}
                </pre>
              ) : null}
            </div>

            <div>
              <h3 className="text-sm font-semibold text-slate-300">子节点</h3>
              {nodeResponse.children && nodeResponse.children.length > 0 ? (
                <table className="mt-2 w-full text-left text-sm text-slate-300">
                  <thead className="text-xs uppercase tracking-widest text-slate-500">
                    <tr>
                      <th className="pb-2">名称</th>
                      <th className="pb-2">类型</th>
                      <th className="pb-2">大小</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-slate-800">
                    {nodeResponse.children.map((child) => (
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
                <p className="text-sm text-slate-500">暂无子节点。</p>
              )}
            </div>
          </div>
        ) : (
          <p className="text-sm text-slate-500">输入 bundleId 与 fileId 后查询节点。</p>
        )}
      </section>
    </div>
  );
}
