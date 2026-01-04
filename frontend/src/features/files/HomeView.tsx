import { FormEvent, useCallback, useEffect, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { rainApi } from '../../api/client';
import type { FileNode, IssueBundlesResponse, IssueSummary, UploadResponse, UploadSummary } from '../../api/types';

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
  const [bundleFiles, setBundleFiles] = useState<Record<string, { files: FileNode[]; loading: boolean; error: string | null }>>({});
  const [deletingFileKey, setDeletingFileKey] = useState<string | null>(null);
  const currentIssueCode = uploadIssueId.trim();
  const [confirmDialog, setConfirmDialog] = useState<{
    message: string;
    onConfirm: () => Promise<void> | void;
    busy?: boolean;
  } | null>(null);

  const fileInputRef = useRef<HTMLInputElement | null>(null);
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

  const loadBundles = useCallback(
    async (code: string) => {
      const trimmed = code.trim();
      if (!trimmed) {
        setBundles([]);
        setBundleFiles({});
        setBundlesError(null);
        setBundlesLoading(false);
        return;
      }
      setBundlesLoading(true);
      setBundlesError(null);
      setBundleFiles({});
      try {
        const data: IssueBundlesResponse = await rainApi.fetchIssueBundles(trimmed);
        setBundles(data.log_bundles);
      } catch (error) {
        const message = (error as Error).message || '';
        if (/not found|404/i.test(message)) {
          setBundles([]);
          setBundlesError(null);
          if (issueId.trim() === trimmed) {
            setIssueId('');
            localStorage.removeItem(LAST_ISSUE_STORAGE_KEY);
          }
          if (uploadIssueId.trim() === trimmed) {
            setUploadIssueId('');
          }
          throw new Error('未找到 Issue');
        } else {
          setBundlesError(message || '加载上传列表失败');
          setBundles([]);
          throw error;
        }
      } finally {
        setBundlesLoading(false);
      }
    },
    []
  );

  useEffect(() => {
    loadIssues().catch(() => undefined);
  }, [loadIssues]);

  useEffect(() => {
    if (!currentIssueCode) {
      setBundles([]);
      setBundleFiles({});
      setBundlesError(null);
      return;
    }
    loadBundles(currentIssueCode).catch(() => undefined);
  }, [currentIssueCode, loadBundles]);

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

  const loadBundleFiles = async (hash: string) => {
    setBundleFiles((prev) => ({
      ...prev,
      [hash]: { files: prev[hash]?.files ?? [], loading: true, error: null }
    }));
    try {
      const response = await rainApi.fetchFileNode(hash, 'root');
      const filtered = (response.children ?? []).filter((child) => child.meta?.kind === 'uploaded_file');
      setBundleFiles((prev) => ({
        ...prev,
        [hash]: { files: filtered, loading: false, error: null }
      }));
    } catch (error) {
      setBundleFiles((prev) => ({
        ...prev,
        [hash]: { files: [], loading: false, error: (error as Error).message || '加载文件失败' }
      }));
    }
  };

  useEffect(() => {
    if (!bundles.length) return;
    bundles.forEach((bundle) => {
      if (!bundleFiles[bundle.hash]) {
        loadBundleFiles(bundle.hash).catch(() => undefined);
      }
    });
  }, [bundles, bundleFiles, loadBundleFiles]);

  const handleDeleteIssue = async (code: string) => {
    setConfirmDialog({
      message: `确定删除 Issue ${code} 及其上传吗？此操作不可恢复。`,
      onConfirm: async () => {
        setDeletingIssue(code);
        try {
          await rainApi.deleteIssue(code);
          setIssueId('');
          setIssueFilter('');
          setUploadIssueId('');
          setBundles([]);
          setBundleFiles({});
          setBundlesError(null);
          loadIssues().catch(() => undefined);
        } catch (error) {
          setIssuesError((error as Error).message || '删除失败');
        } finally {
          setDeletingIssue(null);
        }
      }
    });
  };

  const performUpload = async (files: File[]) => {
    if (!uploadIssueId.trim()) {
      setUploadError('请输入 Issue ID');
      return;
    }
    if (!files || files.length === 0) {
      setUploadError('请至少选择一个文件');
      return;
    }
    setUploading(true);
    setUploadError(null);
    setUploadSuccess(null);
    try {
      const response = await rainApi.uploadLogs(uploadIssueId.trim(), files);
      setUploadSuccess(response);
      setUploadIssueId(response.issue_code);
      setIssueId(response.issue_code);
      loadIssues().catch(() => undefined);
      loadBundles(response.issue_code).catch(() => undefined);
    } catch (error) {
      setUploadError((error as Error).message || '上传失败');
      setUploadSuccess(null);
    } finally {
      setUploading(false);
    }
  };
  const handleUpload = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    // no-op: upload is triggered by drop or file selection
  };

  return (
    <div className="min-h-screen space-y-6 pb-6 flex flex-col text-sm md:text-base">
      <section className="panel space-y-4 h-full flex-1 flex flex-col">
        <div className="flex items-start justify-between gap-3">
          <div>
            <p className="text-xs sm:text-sm uppercase tracking-[0.2em] text-brand-500">Issue 操作</p>
          </div>
        </div>

        <div className="grid items-stretch gap-4 lg:grid-cols-3 flex-1">
          <div className="space-y-3 rounded-lg border border-slate-800 bg-slate-900/60 p-4 h-full flex flex-col">
            <div className="space-y-2 rounded-lg border border-slate-800 bg-slate-900/60 p-3 text-sm md:text-base">
              <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
                <h4 className="text-sm sm:text-base font-semibold text-white">Issue 列表</h4>
              </div>
              <input
                className="w-full rounded-lg border border-slate-700 bg-slate-900 px-4 py-2 text-white focus:border-brand-500 focus:outline-none text-sm md:text-base"
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
          </div>

          <div className="space-y-4 lg:col-span-2 h-full flex flex-col">
            <form onSubmit={handleUpload} className="space-y-3 rounded-lg border border-slate-800 bg-slate-900/70 p-4 w-full">
              {uploadSuccess ? (
                <div className="rounded-lg border border-emerald-600/40 bg-emerald-500/10 px-3 py-2 text-xs text-emerald-200">
                  <p className="font-semibold">上传成功</p>
                  <p>Issue：{uploadSuccess.issue_code}</p>
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
                <input
                  ref={fileInputRef}
                  type="file"
                  multiple
                  className="hidden"
                  onChange={(event) => {
                    const files = event.target.files;
                    if (files && files.length > 0) {
                      performUpload(Array.from(files)).catch(() => undefined);
                    }
                    if (fileInputRef.current) {
                      fileInputRef.current.value = '';
                    }
                  }}
                />
                <div
                  className="w-full rounded-lg border border-dashed border-slate-700 bg-slate-950/60 px-4 py-6 text-center text-sm text-slate-200 transition hover:border-brand-500 hover:bg-slate-900/70 cursor-pointer"
                  onClick={() => fileInputRef.current?.click()}
                  onDragOver={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                  }}
                  onDrop={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    if (event.dataTransfer?.files?.length) {
                      performUpload(Array.from(event.dataTransfer.files)).catch(() => undefined);
                    }
                  }}
                >
                  <p className="font-semibold text-white">{uploading ? '上传中...' : '拖拽文件到这里上传'}</p>
                  <p className="text-xs text-slate-400">支持日志或压缩包，点击也可选择文件</p>
                </div>
              </div>
              {uploadError ? <p className="text-sm text-rose-300">{uploadError}</p> : null}
            </form>

            {currentIssueCode ? (
              <div className="space-y-3 rounded-lg border border-slate-800 bg-slate-900/70 p-4 flex-1 flex flex-col">
                <div className="flex items-center justify-between">
                  <h3 className="text-sm font-semibold text-white">当前文件</h3>
                </div>
              {bundlesError ? <p className="text-xs text-rose-300">{bundlesError}</p> : null}
              <div className="space-y-2 text-sm text-slate-200">
                {(() => {
                  const allFiles = bundles.flatMap((bundle) =>
                    (bundleFiles[bundle.hash]?.files ?? []).map((file) => ({
                      bundleHash: bundle.hash,
                      file
                    }))
                  );
                  const anyLoading =
                    bundlesLoading ||
                    bundles.some((bundle) => bundleFiles[bundle.hash]?.loading);
                  const anyError =
                    bundlesError ||
                    bundles.find((bundle) => bundleFiles[bundle.hash]?.error)?.hash;

                  if (anyLoading && allFiles.length === 0) {
                    return <p className="text-xs text-slate-500">文件加载中...</p>;
                  }
                  if (allFiles.length === 0) {
                    return <p className="text-xs text-slate-500">暂无文件</p>;
                  }
                  return (
                    <ul className="space-y-1 text-sm md:text-base text-slate-300">
                      {allFiles.map(({ bundleHash, file }) => {
                        const label =
                          typeof file.meta?.original_name === 'string'
                            ? (file.meta.original_name as string)
                            : file.name;
                        const key = `${bundleHash}:${file.id}`;
                        const deleting = deletingFileKey === key || deletingBundle === bundleHash;
                        return (
                          <li key={`${bundleHash}:${file.id}`} className="flex items-center gap-2">
                            <span className="truncate">
                              {label} ({((file.size_bytes ?? 0) / 1024).toFixed(1)} KB)
                            </span>
                            <button
                              type="button"
                              className="ml-auto rounded border border-rose-500/40 px-2 py-1 text-[11px] text-rose-200 transition hover:bg-rose-500/10 disabled:opacity-60"
                              disabled={deleting}
                              onClick={() => {
                                setConfirmDialog({
                                  message: `确定删除文件 ${label} 吗？此操作不可恢复。`,
                                  onConfirm: async () => {
                                    setDeletingFileKey(key);
                                    try {
                                      await rainApi.deleteFile(bundleHash, String(file.id));
                                      await loadBundleFiles(bundleHash);
                                      if (currentIssueCode) {
                                        await loadBundles(currentIssueCode);
                                      }
                                    } catch (err) {
                                      setBundleFiles((prev) => ({
                                        ...prev,
                                        [bundleHash]: {
                                          files: prev[bundleHash]?.files ?? [],
                                          loading: false,
                                          error: (err as Error).message || '删除文件失败'
                                        }
                                      }));
                                    } finally {
                                      setDeletingFileKey(null);
                                    }
                                  }
                                });
                              }}
                            >
                              {deleting ? '删除中...' : '删除'}
                            </button>
                          </li>
                        );
                      })}
                      {anyLoading ? <p className="text-xs text-slate-500">文件加载中...</p> : null}
                      {anyError && !bundlesError
                        ? (
                          <p className="text-xs text-rose-300">
                            {bundles
                              .map((bundle) => bundleFiles[bundle.hash]?.error)
                              .find((msg) => msg)}
                          </p>
                        )
                        : null}
                    </ul>
                  );
                })()}
              </div>
            </div>
          ) : null}
          </div>
        </div>
      </section>

      {confirmDialog ? (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/70 p-4">
          <div className="w-full max-w-sm rounded-xl border border-slate-800 bg-slate-900/90 p-5 shadow-2xl">
            <p className="text-sm text-slate-200">{confirmDialog.message}</p>
            <div className="mt-4 flex items-center justify-end gap-3 text-sm">
              <button
                type="button"
                className="rounded-lg border border-slate-700 px-4 py-2 text-slate-200 hover:border-slate-500"
                onClick={() => setConfirmDialog(null)}
                disabled={!!confirmDialog.busy}
              >
                取消
              </button>
              <button
                type="button"
                className="rounded-lg bg-rose-500 px-4 py-2 font-semibold text-slate-900 transition hover:bg-rose-400 disabled:opacity-60"
                onClick={async () => {
                  if (!confirmDialog) return;
                  setConfirmDialog((prev) => (prev ? { ...prev, busy: true } : prev));
                  try {
                    await confirmDialog.onConfirm();
                  } finally {
                    setConfirmDialog(null);
                  }
                }}
                disabled={!!confirmDialog.busy}
              >
                {confirmDialog.busy ? '处理中...' : '确定删除'}
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
