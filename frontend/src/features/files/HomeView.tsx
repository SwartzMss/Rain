import { FormEvent, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { rainApi } from '../../api/client';
import type { IssueBundlesResponse, UploadResponse } from '../../api/types';
import { StatusBadge } from '../../components/StatusBadge';

const formatBytes = (bytes: number) => {
  if (!bytes) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB'];
  const exponent = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** exponent;
  return `${value.toFixed(value >= 10 || exponent === 0 ? 0 : 1)} ${units[exponent]}`;
};

export function HomeView() {
  const navigate = useNavigate();

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

  const navigateToBundle = (hash: string, issueCode?: string, bundleName?: string) => {
    if (!hash) return;
    navigate(`/bundle/${hash}`, {
      state: {
        issue: issueCode || uploadIssueId || issueId || undefined,
        bundleName: bundleName || hash
      }
    });
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
      navigateToBundle(response.bundle_hash, response.issue_code, response.bundle_hash);
    } catch (error) {
      setUploadError((error as Error).message || '上传失败');
      setUploadSuccess(null);
    } finally {
      setUploading(false);
    }
  };

  return (
    <div className="space-y-6">
      <section className="panel space-y-4">
        <div className="flex items-start justify-between gap-3">
          <div>
            <p className="text-xs uppercase tracking-[0.2em] text-brand-500">Issue 操作</p>
            <h2 className="text-lg font-semibold text-white">创建 / 上传 / 查询</h2>
          </div>
          {issueData ? (
            <span className="rounded-full bg-slate-900 px-3 py-1 text-xs text-slate-300">
              {issueData.log_bundles.length} 个 bundle
            </span>
          ) : null}
        </div>

        <div className="grid gap-4 lg:grid-cols-2">
          <div className="space-y-3 rounded-lg border border-slate-800 bg-slate-900/60 p-4">
            <div className="flex items-center justify-between">
              <h3 className="text-sm font-semibold text-white">Issue 列表</h3>
              {issueData ? <span className="text-xs text-slate-400">{issueData.log_bundles.length} 个 bundle</span> : null}
            </div>

            <form onSubmit={handleIssueLookup} className="flex flex-col gap-3 sm:flex-row">
              <input
                className="flex-1 rounded-lg border border-slate-700 bg-slate-900 px-4 py-2 text-white focus:border-brand-500 focus:outline-none"
                placeholder="查询 Issue ID，例如 CN013"
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
              <div className="grid gap-3">
                {issueData.log_bundles.map((bundle) => {
                  return (
                    <button
                      key={bundle.hash}
                      type="button"
                      onClick={() => navigateToBundle(bundle.hash, issueData?.name ?? issueId, bundle.name)}
                      className="flex flex-col gap-2 rounded-lg border border-slate-800 bg-slate-900/70 px-4 py-3 text-left transition hover:border-slate-700 hover:bg-slate-900/80"
                    >
                      <div className="flex items-start justify-between gap-3">
                        <div className="min-w-0 space-y-1">
                          <p className="truncate text-base font-semibold text-white">{bundle.name}</p>
                          <p className="truncate font-mono text-[11px] text-slate-500">{bundle.hash}</p>
                        </div>
                        <StatusBadge status={bundle.status.upload_status} />
                      </div>
                      <p className="text-xs text-slate-500">点击进入文件页面。</p>
                    </button>
                  );
                })}
              </div>
            ) : null}
          </div>

          <form onSubmit={handleUpload} className="space-y-3 rounded-lg border border-slate-800 bg-slate-900/70 p-4">
            {uploadSuccess ? (
              <div className="rounded-lg border border-emerald-600/40 bg-emerald-500/10 px-3 py-2 text-xs text-emerald-200">
                <p className="font-semibold">上传成功</p>
                <p>Issue：{uploadSuccess.issue_code}</p>
                <p className="font-mono text-[11px] text-emerald-100">{uploadSuccess.bundle_hash}</p>
                <p>文件 {uploadSuccess.file_count} 个 · 共 {(uploadSuccess.total_bytes / 1024).toFixed(1)} KB</p>
              </div>
            ) : null}
            <h3 className="text-sm font-semibold text-white">创建 / 上传</h3>
            <label className="block text-sm text-slate-300">
              Issue ID
              <input
                className="mt-1 w-full rounded-lg border border-slate-700 bg-slate-950 px-4 py-2 text-white focus:border-brand-500 focus:outline-none"
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
              {selectedFiles && selectedFiles.length > 0 ? (
                <ul className="space-y-1 rounded-lg border border-slate-800 bg-slate-950/70 p-3 text-xs text-slate-200">
                  {Array.from(selectedFiles).map((file) => (
                    <li key={`${file.name}-${file.lastModified}`} className="flex items-center justify-between gap-3">
                      <span className="truncate">{file.name}</span>
                      <span className="shrink-0 text-slate-400">{formatBytes(file.size)}</span>
                    </li>
                  ))}
                </ul>
              ) : null}
            </div>
            <div className="flex items-center justify-end text-xs text-slate-500">
              <button
                type="submit"
                className="rounded-lg bg-brand-500 px-5 py-2 text-sm font-semibold text-slate-900 transition hover:bg-brand-700 disabled:opacity-60"
                disabled={uploading}
              >
                {uploading ? '上传中...' : '上传并打开'}
              </button>
            </div>
            {uploadError ? <p className="text-sm text-rose-300">{uploadError}</p> : null}
          </form>
        </div>
      </section>
    </div>
  );
}
