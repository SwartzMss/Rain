import { FormEvent, useCallback, useEffect, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { normalizeApiError, normalizeIssueCode, rainApi } from '../../api/client';
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
  const [issueError, setIssueError] = useState<string | null>(null);
  const [issues, setIssues] = useState<IssueSummary[]>([]);
  const [issuesLoading, setIssuesLoading] = useState(false);
  const [issuesError, setIssuesError] = useState<string | null>(null);
  const [createDialogOpen, setCreateDialogOpen] = useState(false);
  const [newIssueCode, setNewIssueCode] = useState('');
  const [newIssueName, setNewIssueName] = useState('');
  const [creatingIssue, setCreatingIssue] = useState(false);
  const [createIssueError, setCreateIssueError] = useState<string | null>(null);
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
  const uploadingRef = useRef(false);
  const [uploading, setUploading] = useState(false);
  const [uploadProgress, setUploadProgress] = useState(0);
  const [uploadError, setUploadError] = useState<string | null>(null);
  const [uploadSuccess, setUploadSuccess] = useState<UploadResponse | null>(null);
  const [uploadTask, setUploadTask] = useState<UploadTaskResponse | null>(null);
  const selectedIssueRef = useRef(selectedIssueCode);
  const bundleRequestIdRef = useRef(0);
  const hasActiveUploadTask =
    uploadTask?.status === 'PROCESSING' || uploadTask?.status === 'PENDING';
  const uploadDisabled = !selectedIssueCode || uploading || hasActiveUploadTask;

  useEffect(() => {
    const stored = localStorage.getItem(LAST_ISSUE_STORAGE_KEY);
    if (stored) {
      try {
        setSelectedIssueCode(normalizeIssueCode(stored));
      } catch {
        localStorage.removeItem(LAST_ISSUE_STORAGE_KEY);
      }
    }
  }, []);

  useEffect(() => {
    const trimmed = selectedIssueCode.trim();
    selectedIssueRef.current = trimmed;
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
      setIssuesError(normalizeApiError(error));
    } finally {
      setIssuesLoading(false);
    }
  }, []);

  const loadBundles = useCallback(
    async (code: string) => {
      const trimmed = code.trim();
      const requestId = ++bundleRequestIdRef.current;
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
        if (requestId !== bundleRequestIdRef.current || selectedIssueRef.current !== trimmed) {
          return;
        }
        setBundles(data.log_bundles);
        setBundleFiles((prev) => {
          const validHashes = new Set(data.log_bundles.map((item) => item.hash));
          return Object.fromEntries(
            Object.entries(prev).filter(([hash]) => validHashes.has(hash))
          );
        });
      } catch (error) {
        if (requestId !== bundleRequestIdRef.current) {
          return;
        }
        const message = normalizeApiError(error);
        if (/not found|404/i.test(message)) {
          setBundles([]);
          setBundleFiles({});
          setBundlesError('Issue 不存在或已被删除');
          if (selectedIssueRef.current === trimmed) {
            setSelectedIssueCode('');
            localStorage.removeItem(LAST_ISSUE_STORAGE_KEY);
          }
          throw new Error('未找到 Issue');
        } else {
          setBundlesError(normalizeApiError(error));
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
    setBundleFiles({});
    loadBundles(currentIssueCode).catch(() => undefined);
  }, [currentIssueCode, loadBundles]);

  useEffect(() => {
    if (!currentIssueCode) return;
    const hasProcessingBundle = bundles.some(
      (bundle) => bundle.status?.upload_status === 'PROCESSING'
    );
    if (!hasProcessingBundle) return;
    if (uploadTask?.task_id) return;

    const timer = window.setTimeout(() => {
      loadBundles(currentIssueCode).catch(() => undefined);
    }, 1500);
    return () => window.clearTimeout(timer);
  }, [bundles, currentIssueCode, loadBundles, uploadTask?.task_id]);

  useEffect(() => {
    const taskId = uploadTask?.task_id;
    if (!taskId) return;
    if (uploadTask.status === 'READY' || uploadTask.status === 'FAILED') return;

    let cancelled = false;
    let timer: number | undefined;

    const poll = async () => {
      try {
        const task = await rainApi.fetchUploadTask(taskId);
        if (cancelled) return;
        setUploadTask(task);
        if (task.status === 'READY' || task.status === 'FAILED') {
          if (selectedIssueRef.current === task.issue_code) {
            await loadBundles(task.issue_code);
          }
          await loadIssues();
          return;
        }
        timer = window.setTimeout(poll, 1500);
      } catch (error) {
        if (!cancelled) {
          setUploadError(normalizeApiError(error));
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
  }, [uploadTask?.task_id, uploadTask?.status, loadIssues, loadBundles]);

  const selectIssue = (value: string) => {
    try {
      const code = normalizeIssueCode(value);
      setIssueError(null);
      setSelectedIssueCode(code);
    } catch (error) {
      setIssueError(normalizeApiError(error));
    }
  };

  const handleCreateIssue = async () => {
    let code: string;
    try {
      code = normalizeIssueCode(newIssueCode);
    } catch (error) {
      setCreateIssueError(normalizeApiError(error));
      return;
    }

    setCreatingIssue(true);
    setCreateIssueError(null);
    try {
      const issue = await rainApi.createIssue({
        code,
        name: newIssueName.trim() || undefined
      });
      setIssues((prev) => [issue, ...prev.filter((item) => item.code !== issue.code)]);
      setSelectedIssueCode(issue.code);
      setIssueSearchText('');
      setBundles([]);
      setBundleFiles({});
      setNewIssueCode('');
      setNewIssueName('');
      setCreateDialogOpen(false);
      localStorage.setItem(LAST_ISSUE_STORAGE_KEY, issue.code);
    } catch (error) {
      setCreateIssueError(normalizeApiError(error));
    } finally {
      setCreatingIssue(false);
    }
  };

  const closeCreateDialog = () => {
    setCreateDialogOpen(false);
    setCreateIssueError(null);
    setNewIssueCode('');
    setNewIssueName('');
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
          error: normalizeApiError(error)
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
          if (selectedIssueCode.trim() === code) {
            setSelectedIssueCode('');
            setBundles([]);
            setBundleFiles({});
            setBundlesError(null);
          }
          loadIssues().catch(() => undefined);
        } catch (error) {
          setIssuesError(normalizeApiError(error));
        } finally {
          setDeletingIssue(null);
        }
      }
    });
  };

  const performUpload = async (files: File[]) => {
    if (uploadingRef.current) {
      return;
    }
    if (hasActiveUploadTask) {
      setUploadError('当前上传任务仍在后台解析，请等待完成后再上传');
      return;
    }
    if (!selectedIssueCode) {
      setUploadError('请先选择或创建 Issue');
      return;
    }
    if (!files || files.length === 0) {
      setUploadError('请至少选择一个文件');
      return;
    }
    uploadingRef.current = true;
    setUploading(true);
    setUploadProgress(0);
    setUploadError(null);
    setUploadSuccess(null);
    setUploadTask(null);
    try {
      const response = await rainApi.uploadLogs(selectedIssueCode, files, setUploadProgress);
      setUploadSuccess(response);
      setUploadTask({
        task_id: response.task_id,
        issue_code: response.issue_code,
        bundle_hash: response.bundle_hash,
        status: response.status,
        progress_percent: response.status === 'READY' ? 100 : 0,
        total_bytes: response.total_bytes
      });
      await loadBundles(selectedIssueCode);
      await loadIssues();
    } catch (error) {
      setUploadError(normalizeApiError(error));
      setUploadSuccess(null);
    } finally {
      uploadingRef.current = false;
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
                <button
                  type="button"
                  className="rounded border border-brand-500/50 px-3 py-1 text-xs font-semibold text-brand-100 transition hover:bg-brand-500/10"
                  onClick={() => {
                    setCreateDialogOpen(true);
                    setCreateIssueError(null);
                  }}
                >
                  + 新建 Issue
                </button>
              </div>
              <input
                className="w-full rounded-lg border border-slate-700 bg-slate-900 px-4 py-2 text-white focus:border-brand-500 focus:outline-none text-sm md:text-base"
                placeholder="输入 Issue ID，例如 CN013"
                value={issueSearchText}
                onChange={(event) => setIssueSearchText(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter') {
                    event.preventDefault();
                    selectIssue(issueSearchText);
                  }
                }}
              />
              {issueError ? <p className="text-sm text-rose-300">{issueError}</p> : null}
              {issuesError ? (
                <div className="flex items-center justify-between gap-2 rounded-lg border border-rose-500/30 bg-rose-500/10 px-3 py-2 text-xs text-rose-200">
                  <span>{issuesError}</span>
                  <button
                    type="button"
                    className="shrink-0 rounded border border-rose-400/50 px-2 py-1 text-[11px] font-semibold text-rose-100 transition hover:bg-rose-500/10 disabled:opacity-60"
                    onClick={() => {
                      loadIssues().catch(() => undefined);
                    }}
                    disabled={issuesLoading}
                  >
                    {issuesLoading ? '连接中...' : '重新连接'}
                  </button>
                </div>
              ) : null}
              <div className="max-h-56 space-y-1 overflow-y-auto text-sm">
                {issues
                  .filter((item) => {
                    const filter = issueSearchText.trim().toLowerCase();
                    if (!filter) return true;
                    return item.code.toLowerCase().includes(filter) || item.name.toLowerCase().includes(filter);
                  })
                  .map((item) => (
                    <div
                      key={item.code}
                      className="flex w-full items-center gap-2 rounded-lg border border-transparent px-3 py-2 transition hover:border-slate-700 hover:bg-slate-900/70"
                    >
                      <button
                        type="button"
                        onClick={() => selectIssue(item.code)}
                        className="flex min-w-0 flex-1 flex-col text-left"
                      >
                        <span className="truncate font-semibold text-white">{item.code}</span>
                        <span className="text-[10px] text-slate-500">{item.bundle_count} 个上传包</span>
                      </button>
                      <button
                        type="button"
                        onClick={() => navigate(`/issue/${encodeURIComponent(item.code)}`)}
                        className="shrink-0 rounded border border-slate-600 px-2 py-1 text-[11px] text-slate-200 transition hover:bg-slate-800"
                      >
                        查看
                      </button>
                      <button
                        type="button"
                        onClick={() => {
                          handleDeleteIssue(item.code).catch(() => undefined);
                        }}
                        className="shrink-0 rounded border border-rose-500/50 px-2 py-1 text-[11px] text-rose-200 transition hover:bg-rose-500/10 disabled:opacity-60"
                        disabled={deletingIssue === item.code}
                      >
                        {deletingIssue === item.code ? '删除中...' : '删除'}
                      </button>
                    </div>
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
              <h3 className="text-sm font-semibold text-white">上传日志</h3>
              {selectedIssueCode ? (
                <p className="text-sm text-slate-300">
                  当前上传到：<strong className="text-white">{selectedIssueCode}</strong>
                </p>
              ) : (
                <p className="text-sm text-slate-400">请先选择或创建一个 Issue</p>
              )}
              <div className="space-y-2">
                <input
                  ref={fileInputRef}
                  type="file"
                  multiple
                  className="hidden"
                  disabled={uploadDisabled}
                  onChange={(event) => {
                    if (uploadingRef.current) return;
                    if (uploadDisabled) return;
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
                  className="w-full rounded-lg border border-dashed border-slate-700 bg-slate-950/60 px-4 py-6 text-center text-sm text-slate-200 transition hover:border-brand-500 hover:bg-slate-900/70 cursor-pointer aria-disabled:cursor-not-allowed aria-disabled:opacity-60"
                  aria-disabled={uploadDisabled}
                  onClick={() => {
                    if (!uploadDisabled && !uploadingRef.current) {
                      fileInputRef.current?.click();
                    }
                  }}
                  onDragOver={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                  }}
                  onDrop={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    if (uploadDisabled) return;
                    if (uploadingRef.current) return;
                    if (event.dataTransfer?.files?.length) {
                      performUpload(Array.from(event.dataTransfer.files)).catch(() => undefined);
                    }
                  }}
                >
                  <p className="font-semibold text-white">
                    {uploading ? '上传中...' : hasActiveUploadTask ? '后台解析中...' : '拖拽文件到这里上传'}
                  </p>
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
                                          error: normalizeApiError(err)
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
          ) : (
            <div className="rounded-lg border border-slate-800 bg-slate-900/70 p-4 text-sm text-slate-400">
              请选择一个 Issue，或新建 Issue 后上传日志。
            </div>
          )}
          </div>
        </div>
      </section>

      {createDialogOpen ? (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/70 p-4"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) {
              closeCreateDialog();
            }
          }}
        >
          <form
            className="w-full max-w-md rounded-xl border border-slate-800 bg-slate-900/95 p-5 shadow-2xl"
            onSubmit={(event) => {
              event.preventDefault();
              handleCreateIssue().catch(() => undefined);
            }}
            onKeyDown={(event) => {
              if (event.key === 'Escape') {
                closeCreateDialog();
              }
            }}
          >
            <h3 className="text-sm font-semibold text-white">新建 Issue</h3>
            <div className="mt-4 space-y-3">
              <label className="block text-sm text-slate-300">
                Issue ID
                <input
                  className="mt-1 w-full rounded-lg border border-slate-700 bg-slate-950 px-4 py-2 text-white focus:border-brand-500 focus:outline-none"
                  value={newIssueCode}
                  onChange={(event) => setNewIssueCode(event.target.value)}
                  placeholder="例如 CN014"
                />
              </label>
              <label className="block text-sm text-slate-300">
                名称（可选）
                <input
                  className="mt-1 w-full rounded-lg border border-slate-700 bg-slate-950 px-4 py-2 text-white focus:border-brand-500 focus:outline-none"
                  value={newIssueName}
                  onChange={(event) => setNewIssueName(event.target.value)}
                />
              </label>
              {createIssueError ? <p className="text-sm text-rose-300">{createIssueError}</p> : null}
            </div>
            <div className="mt-5 flex justify-end gap-3 text-sm">
              <button
                type="button"
                className="rounded-lg border border-slate-700 px-4 py-2 text-slate-200 hover:border-slate-500"
                onClick={closeCreateDialog}
                disabled={creatingIssue}
              >
                取消
              </button>
              <button
                type="submit"
                className="rounded-lg bg-brand-500 px-4 py-2 font-semibold text-slate-900 transition hover:bg-brand-700 disabled:opacity-60"
                disabled={creatingIssue}
              >
                {creatingIssue ? '创建中...' : '创建'}
              </button>
            </div>
          </form>
        </div>
      ) : null}

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
