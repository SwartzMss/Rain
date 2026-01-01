import { FormEvent, useCallback, useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { rainApi } from '../../api/client';
import type { IssueBundlesResponse, IssueSummary, UploadResponse, UploadSummary } from '../../api/types';

const LAST_ISSUE_STORAGE_KEY = 'rain:last_issue_id';

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
  const [issueFilter, setIssueFilter] = useState('');
  const [uploadIssueId, setUploadIssueId] = useState('');
  const [issueLoading, setIssueLoading] = useState(false);
  const [issueError, setIssueError] = useState<string | null>(null);
  const [issues, setIssues] = useState<IssueSummary[]>([]);
  const [issuesLoading, setIssuesLoading] = useState(false);
  const [issuesError, setIssuesError] = useState<string | null>(null);
  const [deletingIssue, setDeletingIssue] = useState<string | null>(null);
  const [bundles, setBundles] = useState<UploadSummary[]>([]);
  const [bundlesLoading, setBundlesLoading] = useState(false);
  const [bundlesError, setBundlesError] = useState<string | null>(null);
  const [deletingBundle, setDeletingBundle] = useState<string | null>(null);

  const [selectedFiles, setSelectedFiles] = useState<FileList | null>(null);
  const [fileInputKey, setFileInputKey] = useState(0);
  const [uploading, setUploading] = useState(false);
  const [uploadError, setUploadError] = useState<string | null>(null);
  const [uploadSuccess, setUploadSuccess] = useState<UploadResponse | null>(null);

  useEffect(() => {
    const stored = localStorage.getItem(LAST_ISSUE_STORAGE_KEY);
    if (stored) {
      setIssueId(stored);
    }
  }, []);

  useEffect(() => {
    const trimmed = issueId.trim();
    if (trimmed) {
      localStorage.setItem(LAST_ISSUE_STORAGE_KEY, trimmed);
    } else {
      localStorage.removeItem(LAST_ISSUE_STORAGE_KEY);
    }
  }, [issueId]);

  const loadIssues = useCallback(async () => {
    setIssuesLoading(true);
    setIssuesError(null);
    try {
      const data = await rainApi.fetchIssues();
      setIssues(data);
    } catch (error) {
      setIssuesError((error as Error).message || '加载 Issue 列表失败');
    } finally {
      setIssuesLoading(false);
    }
  }, []);

  useEffect(() => {
    loadIssues().catch(() => undefined);
  }, [loadIssues]);

  useEffect(() => {
    if (!issueId.trim()) {
      setBundles([]);
      return;
    }
    loadBundles(issueId).catch(() => undefined);
  }, [issueId, loadBundles]);

  const loadBundles = useCallback(
    async (code: string) => {
      const trimmed = code.trim();
      if (!trimmed) {
        setBundles([]);
        return;
      }
      setBundlesLoading(true);
      setBundlesError(null);
      try {
        const data: IssueBundlesResponse = await rainApi.fetchIssueBundles(trimmed);
        setBundles(data.log_bundles);
      } catch (error) {
        setBundlesError((error as Error).message || '加载上传列表失败');
        setBundles([]);
      } finally {
        setBundlesLoading(false);
      }
    },
    []
  );

  const openIssue = async (value: string) => {
    const trimmed = value.trim();
    if (!trimmed) return;
    setIssueLoading(true);
    setIssueError(null);
    try {
      await loadBundles(trimmed);
      setIssueId(trimmed);
      setUploadIssueId(trimmed);
      navigate(`/issue/${trimmed}`);
    } catch (error) {
      setIssueError((error as Error).message || '查询失败');
    } finally {
      setIssueLoading(false);
    }
  };

  const handleDeleteIssue = async (code: string) => {
    const confirmed = window.confirm(`确定删除 Issue ${code} 及其上传吗？此操作不可恢复。`);
    if (!confirmed) return;
    setDeletingIssue(code);
    try {
      await rainApi.deleteIssue(code);
      setIssueId('');
      setIssueFilter('');
      setUploadIssueId('');
      setBundles([]);
      loadIssues().catch(() => undefined);
    } catch (error) {
      setIssuesError((error as Error).message || '删除失败');
    } finally {
      setDeletingIssue(null);
    }
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
      loadIssues().catch(() => undefined);
      loadBundles(response.issue_code).catch(() => undefined);
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
        </div>

        <div className="grid gap-4 lg:grid-cols-2">
          <div className="space-y-3 rounded-lg border border-slate-800 bg-slate-900/60 p-4">
            <div className="space-y-2 rounded-lg border border-slate-800 bg-slate-900/60 p-3">
              <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
                <h4 className="text-sm font-semibold text-white">Issue 列表</h4>
              </div>
              <input
                className="w-full rounded-lg border border-slate-700 bg-slate-900 px-4 py-2 text-white focus:border-brand-500 focus:outline-none"
                placeholder="输入 Issue ID，例如 CN013"
                value={issueFilter}
                onChange={(event) => setIssueFilter(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter') {
                    event.preventDefault();
                    openIssue(issueFilter).catch(() => undefined);
                  }
                }}
              />
              {issueError ? <p className="text-sm text-rose-300">{issueError}</p> : null}
              {issuesError ? <p className="text-xs text-rose-300">{issuesError}</p> : null}
              <div className="max-h-56 space-y-1 overflow-y-auto text-sm">
                {issues
                  .filter((item) => {
                    const filter = issueFilter.trim().toLowerCase();
                    if (!filter) return true;
                    return item.code.toLowerCase().includes(filter) || item.name.toLowerCase().includes(filter);
                  })
                  .map((item) => (
                    <button
                      key={item.code}
                      type="button"
                      onClick={() => {
                        setIssueId(item.code);
                        setUploadIssueId(item.code);
                        loadBundles(item.code).catch(() => undefined);
                      }}
                      onDoubleClick={() => openIssue(item.code).catch(() => undefined)}
                      className="flex w-full items-center justify-between rounded-lg border border-transparent px-3 py-2 text-left transition hover:border-slate-700 hover:bg-slate-900/70"
                      disabled={issueLoading}
                    >
                      <span className="font-semibold text-white">{item.code}</span>
                      <div className="flex items-center gap-2 text-[11px] text-slate-400">
                        <span className="text-[10px] text-slate-500">双击打开</span>
                        <button
                          type="button"
                          onClick={(event) => {
                            event.stopPropagation();
                            handleDeleteIssue(item.code).catch(() => undefined);
                          }}
                          className="rounded border border-rose-500/50 px-2 py-1 text-[11px] text-rose-200 transition hover:bg-rose-500/10 disabled:opacity-60"
                          disabled={deletingIssue === item.code}
                        >
                          {deletingIssue === item.code ? '删除中...' : '删除'}
                        </button>
                      </div>
                    </button>
                  ))}
              </div>
            </div>
            {issueId.trim() ? (
              <div className="space-y-2 rounded-lg border border-slate-800 bg-slate-900/60 p-3">
                <div className="flex items-center justify-between">
                  <h4 className="text-sm font-semibold text-white">上传记录（{issueId}）</h4>
                  <button
                    type="button"
                    className="text-xs text-brand-300 hover:text-brand-200"
                    onClick={() => loadBundles(issueId).catch(() => undefined)}
                    disabled={bundlesLoading}
                  >
                    {bundlesLoading ? '刷新中...' : '刷新'}
                  </button>
                </div>
                {bundlesError ? <p className="text-xs text-rose-300">{bundlesError}</p> : null}
                <div className="space-y-1">
                  {bundles.length === 0 && !bundlesLoading ? (
                    <p className="text-xs text-slate-500">暂无上传记录</p>
                  ) : (
                    bundles.map((bundle) => (
                      <div
                        key={bundle.hash}
                        className="flex items-center justify-between rounded-lg border border-slate-800 bg-slate-900/70 px-3 py-2 text-sm text-white"
                      >
                        <span className="truncate">{bundle.name || bundle.hash}</span>
                        <button
                          type="button"
                          onClick={() => handleDeleteBundle(issueId, bundle.hash).catch(() => undefined)}
                          className="rounded border border-rose-500/50 px-2 py-1 text-[11px] text-rose-200 transition hover:bg-rose-500/10 disabled:opacity-60"
                          disabled={deletingBundle === bundle.hash}
                        >
                          {deletingBundle === bundle.hash ? '删除中...' : '删除'}
                        </button>
                      </div>
                    ))
                  )}
                </div>
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
                {uploading ? '上传中...' : '上传'}
              </button>
            </div>
            {uploadError ? <p className="text-sm text-rose-300">{uploadError}</p> : null}
          </form>
        </div>
      </section>
    </div>
  );
}
