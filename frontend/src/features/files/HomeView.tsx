import { FormEvent, useCallback, useEffect, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { rainApi } from '../../api/client';
import type {
  FileNode,
  IssueBundlesResponse,
  IssueSummary,
  UploadResponse,
  UploadSummary,
  UploadTaskResponse
} from '../../api/types';

const LAST_ISSUE_STORAGE_KEY = 'rain:last_issue_id';

const formatBytes = (bytes: number) => {
  if (!bytes) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB'];
  const exponent = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** exponent;
  return `${value.toFixed(value >= 10 || exponent === 0 ? 0 : 1)} ${units[exponent]}`;
};

type BundleFileState = {
  files: FileNode[];
  loading: boolean;
  loaded: boolean;
  error: string | null;
};

export function HomeView() {
  const navigate = useNavigate();

  const [selectedIssueCode, setSelectedIssueCode] = useState('');
  const [issueSearchText, setIssueSearchText] = useState('');
  const [issueDraftCode, setIssueDraftCode] = useState('');
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
  const [bundleFiles, setBundleFiles] = useState<Record<string, BundleFileState>>({});
  const [deletingFileKey, setDeletingFileKey] = useState<string | null>(null);
  const currentIssueCode = selectedIssueCode.trim();
  const [confirmDialog, setConfirmDialog] = useState<{
    message: string;
    onConfirm: () => Promise<void> | void;
    busy?: boolean;
  } | null>(null);

  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const selectedLoadHandledRef = useRef(false);
  const [uploading, setUploading] = useState(false);
  const [creatingIssue, setCreatingIssue] = useState(false);
  const [uploadProgress, setUploadProgress] = useState(0);
  const [uploadError, setUploadError] = useState<string | null>(null);
  const [uploadSuccess, setUploadSuccess] = useState<UploadResponse | null>(null);
  const [uploadTask, setUploadTask] = useState<UploadTaskResponse | null>(null);

  useEffect(() => {
    const stored = localStorage.getItem(LAST_ISSUE_STORAGE_KEY);
    if (stored) {
      setSelectedIssueCode(stored);
      setIssueDraftCode(stored);
    }
  }, []);

  useEffect(() => {
    const trimmed = selectedIssueCode.trim();
    if (trimmed) {
      localStorage.setItem(LAST_ISSUE_STORAGE_KEY, trimmed);
    } else {
      localStorage.removeItem(LAST_ISSUE_STORAGE_KEY);
    }
  }, [selectedIssueCode]);

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
        setBundlesError(null);
        setBundlesLoading(false);
        return;
      }
      setBundlesLoading(true);
      setBundlesError(null);
      try {
        const data: IssueBundlesResponse = await rainApi.fetchIssueBundles(trimmed);
        setBundles(data.log_bundles);
        setBundleFiles((prev) => {
          const validHashes = new Set(data.log_bundles.map((item) => item.hash));
          return Object.fromEntries(
            Object.entries(prev).filter(([hash]) => validHashes.has(hash))
          );
        });
      } catch (error) {
        const message = (error as Error).message || '';
        if (/not found|404/i.test(message)) {
          setBundles([]);
          setBundlesError(null);
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
    if (selectedLoadHandledRef.current) {
      selectedLoadHandledRef.current = false;
      return;
    }
    setBundleFiles({});
    loadBundles(currentIssueCode).catch(() => undefined);
  }, [currentIssueCode, loadBundles]);

  useEffect(() => {
    if (!currentIssueCode) return;
    const hasProcessingBundle = bundles.some(
      (bundle) => bundle.status?.upload_status === 'PROCESSING'
    );
    if (!hasProcessingBundle) return;

    const timer = window.setTimeout(() => {
      loadBundles(currentIssueCode).catch(() => undefined);
    }, 1500);
    return () => window.clearTimeout(timer);
  }, [bundles, currentIssueCode, loadBundles]);

  useEffect(() => {
    const taskId = uploadSuccess?.task_id;
    if (!taskId) return;

    let cancelled = false;
    let timer: number | undefined;

    const poll = async () => {
      try {
        const task = await rainApi.fetchUploadTask(taskId);
        if (cancelled) return;
        setUploadTask(task);
        loadIssues().catch(() => undefined);
        loadBundles(task.issue_code).catch(() => undefined);
        if (task.status !== 'READY' && task.status !== 'FAILED') {
          timer = window.setTimeout(poll, 1500);
        }
      } catch (error) {
        if (!cancelled) {
          setUploadError((error as Error).message || '查询上传任务失败');
        }
      }
    };

    poll().catch(() => undefined);

    return () => {
      cancelled = true;
      if (timer) {
        window.clearTimeout(timer);
      }
    };
  }, [uploadSuccess?.task_id, loadIssues, loadBundles]);

  const openIssue = async (value: string) => {
    const trimmed = value.trim();
    if (!trimmed) return;
    setIssueLoading(true);
    setIssueError(null);
    try {
      const data = await rainApi.fetchIssueBundles(trimmed);
      setBundles(data.log_bundles);
      setBundleFiles({});
      setBundlesError(null);
      selectedLoadHandledRef.current = true;
      setSelectedIssueCode(trimmed);
      setIssueDraftCode(trimmed);
      navigate(`/issue/${trimmed}`);
    } catch (error) {
      const message = (error as Error).message || '查询失败';
      setIssueError(message === '未找到 Issue' ? '未找到 Issue，请先在右侧创建或上传日志' : message);
    } finally {
      setIssueLoading(false);
    }
  };

  const handleCreateIssue = async () => {
    const code = issueDraftCode.trim();
    if (!code) {
      setUploadError('请输入 Issue ID');
      return;
    }

    setCreatingIssue(true);
    setUploadError(null);
    setUploadSuccess(null);
    setUploadTask(null);
    try {
      const issue = await rainApi.createIssue({ code });
      setIssueDraftCode(issue.code);
      setSelectedIssueCode(issue.code);
      setIssueSearchText(issue.code);
      await loadIssues();
    } catch (error) {
      setUploadError((error as Error).message || '创建 Issue 失败');
    } finally {
      setCreatingIssue(false);
    }
  };

  const loadBundleFiles = useCallback(async (hash: string) => {
    setBundleFiles((prev) => ({
      ...prev,
      [hash]: {
        files: prev[hash]?.files ?? [],
        loading: true,
        loaded: prev[hash]?.loaded ?? false,
        error: null
      }
    }));
    try {
      const response = await rainApi.fetchFileNode(hash, 'root');
      const filtered = (response.children ?? []).filter((child) => child.meta?.kind === 'uploaded_file');
      setBundleFiles((prev) => ({
        ...prev,
        [hash]: { files: filtered, loading: false, loaded: true, error: null }
      }));
    } catch (error) {
      setBundleFiles((prev) => ({
        ...prev,
        [hash]: {
          files: prev[hash]?.files ?? [],
          loading: false,
          loaded: true,
          error: (error as Error).message || '加载文件失败'
        }
      }));
    }
  }, []);

  useEffect(() => {
    if (!bundles.length) return;
    bundles.forEach((bundle) => {
      if (bundle.status?.upload_status !== 'READY') return;
      const existing = bundleFiles[bundle.hash];
      if (!existing?.loaded && !existing?.loading) {
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
          setSelectedIssueCode('');
          setIssueSearchText('');
          setIssueDraftCode('');
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
    if (!issueDraftCode.trim()) {
      setUploadError('请输入 Issue ID');
      return;
    }
    if (!files || files.length === 0) {
      setUploadError('请至少选择一个文件');
      return;
    }
    setUploading(true);
    setUploadProgress(0);
    setUploadError(null);
    setUploadSuccess(null);
    setUploadTask(null);
    try {
      const response = await rainApi.uploadLogs(issueDraftCode.trim(), files, setUploadProgress);
      setUploadSuccess(response);
      setUploadTask({
        task_id: response.task_id,
        issue_code: response.issue_code,
        bundle_hash: response.bundle_hash,
        status: response.status,
        progress_percent: response.status === 'READY' ? 100 : 0,
        total_bytes: response.total_bytes
      });
      setIssueDraftCode(response.issue_code);
      setSelectedIssueCode(response.issue_code);
      loadIssues().catch(() => undefined);
    } catch (error) {
      setUploadError((error as Error).message || '上传失败');
      setUploadSuccess(null);
    } finally {
      setUploading(false);
      setUploadProgress(0);
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
                value={issueSearchText}
                onChange={(event) => setIssueSearchText(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter') {
                    event.preventDefault();
                    openIssue(issueSearchText).catch(() => undefined);
                  }
                }}
              />
              {issueError ? <p className="text-sm text-rose-300">{issueError}</p> : null}
              {issuesError ? <p className="text-xs text-rose-300">{issuesError}</p> : null}
              <div className="max-h-56 space-y-1 overflow-y-auto text-sm">
                {issues
                  .filter((item) => {
                    const filter = issueSearchText.trim().toLowerCase();
                    if (!filter) return true;
                    return item.code.toLowerCase().includes(filter) || item.name.toLowerCase().includes(filter);
                  })
                  .map((item) => (
                    <button
                      key={item.code}
                      type="button"
                      onClick={() => {
                        setSelectedIssueCode(item.code);
                        setIssueDraftCode(item.code);
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
                  <p className="font-semibold">上传已接收</p>
                  <p>Issue：{uploadSuccess.issue_code}</p>
                  <p>任务：{uploadSuccess.task_id}</p>
                  <p>文件 {uploadSuccess.file_count} 个 · 共 {(uploadSuccess.total_bytes / 1024).toFixed(1)} KB</p>
                  <p>
                    后台状态：
                    {uploadTask?.status === 'READY'
                      ? '解析完成'
                      : uploadTask?.status === 'FAILED'
                        ? '解析失败'
                        : '解析中'}
                  </p>
                </div>
              ) : null}
              <h3 className="text-sm font-semibold text-white">创建 Issue / 上传日志</h3>
              <label className="block text-sm text-slate-300">
                Issue ID
                <div className="mt-1 flex flex-col gap-2 sm:flex-row">
                  <input
                    className="w-full rounded-lg border border-slate-700 bg-slate-950 px-4 py-2 text-white focus:border-brand-500 focus:outline-none"
                    value={issueDraftCode}
                    onChange={(event) => setIssueDraftCode(event.target.value)}
                  />
                  <button
                    type="button"
                    className="rounded-lg border border-brand-500/60 px-4 py-2 text-sm font-semibold text-brand-100 transition hover:bg-brand-500/10 disabled:opacity-60"
                    onClick={() => {
                      handleCreateIssue().catch(() => undefined);
                    }}
                    disabled={creatingIssue || !issueDraftCode.trim()}
                  >
                    {creatingIssue ? '创建中...' : '创建 Issue'}
                  </button>
                </div>
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
                {uploading ? (
                  <div className="space-y-1">
                    <div className="h-2 overflow-hidden rounded bg-slate-800">
                      <div
                        className="h-full bg-brand-500 transition-all"
                        style={{ width: `${uploadProgress}%` }}
                      />
                    </div>
                    <p className="text-xs text-slate-400">
                      {uploadProgress < 100 ? `上传 ${uploadProgress}%` : '上传完成，正在解析...'}
                    </p>
                  </div>
                ) : null}
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
                    if (bundles.some((bundle) => bundle.status?.upload_status === 'PROCESSING')) {
                      return <p className="text-xs text-slate-500">后台解析中...</p>;
                    }
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
                                          loaded: true,
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
