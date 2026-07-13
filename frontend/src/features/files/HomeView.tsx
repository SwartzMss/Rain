import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { normalizeApiError, normalizeIssueCode, rainApi } from '../../api/client';
import type {
  FileNode,
  IssueBundlesResponse,
  IssueSummary,
  UploadStage,
  UploadStatus,
  UploadSummary,
  UploadTaskResponse
} from '../../api/types';
import { createOptimisticUploadRows, type UploadSelectionItem } from './uploadRows';

const LAST_ISSUE_STORAGE_KEY = 'rain:last_issue_id';

type BundleFileState = {
  files: FileNode[];
  loading: boolean;
  loaded: boolean;
  error: string | null;
};

type FileRow = {
  key: string;
  bundleHash: string;
  bundleName: string;
  file?: FileNode;
  name: string;
  status: UploadStatus;
  stage: UploadStage | 'UPLOADING';
  progressPercent?: number;
  sizeBytes?: number;
};

const formatBytes = (bytes?: number | null) => {
  if (!bytes) return '-';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const exponent = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** exponent;
  return `${value.toFixed(value >= 10 || exponent === 0 ? 0 : 1)} ${units[exponent]}`;
};

const stageLabel = (stage: FileRow['stage'], progressPercent?: number) => {
  if (stage === 'UPLOADING') return `上传中 ${progressPercent ?? 0}%`;
  if (stage === 'PENDING') return '等待处理';
  if (stage === 'EXTRACTING') return '解压中';
  if (stage === 'INDEXING') return '建立索引';
  if (stage === 'READY') return '已完成';
  return '失败';
};

const stageClass = (stage: FileRow['stage']) => {
  if (stage === 'READY') return 'border-emerald-500/40 bg-emerald-500/10 text-emerald-300';
  if (stage === 'FAILED') return 'border-rose-500/40 bg-rose-500/10 text-rose-300';
  return 'border-sky-500/40 bg-sky-500/10 text-sky-300';
};

const getFileLabel = (file: FileNode) =>
  typeof file.meta?.original_name === 'string' ? (file.meta.original_name as string) : file.name;

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
  const [creatingIssue, setCreatingIssue] = useState(false);
  const [createIssueError, setCreateIssueError] = useState<string | null>(null);
  const [deletingIssue, setDeletingIssue] = useState<string | null>(null);
  const [bundles, setBundles] = useState<UploadSummary[]>([]);
  const [, setBundlesLoading] = useState(false);
  const [bundlesError, setBundlesError] = useState<string | null>(null);
  const [bundleFiles, setBundleFiles] = useState<Record<string, BundleFileState>>({});
  const [deletingKey, setDeletingKey] = useState<string | null>(null);
  const [confirmDialog, setConfirmDialog] = useState<{
    message: string;
    onConfirm: () => Promise<void> | void;
    busy?: boolean;
  } | null>(null);

  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const uploadingRef = useRef(false);
  const selectedIssueRef = useRef(selectedIssueCode);
  const bundleRequestIdRef = useRef(0);
  const [uploading, setUploading] = useState(false);
  const [uploadProgress, setUploadProgress] = useState(0);
  const [uploadError, setUploadError] = useState<string | null>(null);
  const [uploadTask, setUploadTask] = useState<UploadTaskResponse | null>(null);
  const [uploadSelection, setUploadSelection] = useState<UploadSelectionItem[]>([]);

  const currentIssueCode = selectedIssueCode.trim();
  const activeTask =
    uploadTask?.status === 'PROCESSING' || uploadTask?.status === 'PENDING' ? uploadTask : null;
  const uploadDisabled = !currentIssueCode || uploading || !!activeTask;

  const filteredIssues = useMemo(() => {
    const filter = issueSearchText.trim().toLowerCase();
    if (!filter) return issues;
    return issues.filter(
      (issue) =>
        issue.code.toLowerCase().includes(filter) || issue.name.toLowerCase().includes(filter)
    );
  }, [issueSearchText, issues]);

  const fileRows = useMemo<FileRow[]>(() => {
    const rows = bundles.flatMap<FileRow>((bundle) => {
      const status = bundle.status.upload_status;
      const currentTask = uploadTask?.bundle_hash === bundle.hash ? uploadTask : null;
      const stage = currentTask?.stage ?? bundle.stage;
      const state = bundleFiles[bundle.hash];
      if (status !== 'READY') {
        if (currentTask && uploadSelection.length > 0) {
          return uploadSelection.map((item, index) => ({
            key: `active-upload:${index}:${item.name}`,
            bundleHash: bundle.hash,
            bundleName: item.name,
            name: item.name,
            status,
            stage,
            progressPercent: currentTask.progress_percent,
            sizeBytes: item.sizeBytes
          }));
        }
        return [
          {
            key: currentTask ? 'active-upload' : bundle.hash,
            bundleHash: bundle.hash,
            bundleName: bundle.name || bundle.hash,
            name: bundle.name || bundle.hash,
            status,
            stage,
            sizeBytes: currentTask?.total_bytes ?? bundle.size_bytes ?? undefined
          }
        ];
      }

      return (state?.files ?? []).map((file) => ({
        key: `${bundle.hash}:${file.id}`,
        bundleHash: bundle.hash,
        bundleName: bundle.name || bundle.hash,
        file,
        name: getFileLabel(file),
        status,
        stage: 'READY',
        sizeBytes: file.size_bytes
      }));
    });
    const taskBundleVisible = !!uploadTask?.bundle_hash && bundles.some(
      (bundle) => bundle.hash === uploadTask.bundle_hash
    );
    if ((uploading || activeTask) && uploadSelection.length > 0 && !taskBundleVisible) {
      rows.unshift(
        ...createOptimisticUploadRows(
          uploadSelection,
          uploadProgress,
          uploadTask?.bundle_hash ?? ''
        )
      );
    }
    return rows;
  }, [activeTask, bundleFiles, bundles, uploadProgress, uploadSelection, uploadTask, uploading]);

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
    selectedIssueRef.current = currentIssueCode;
    if (currentIssueCode) {
      localStorage.setItem(LAST_ISSUE_STORAGE_KEY, currentIssueCode);
    } else {
      localStorage.removeItem(LAST_ISSUE_STORAGE_KEY);
    }
  }, [currentIssueCode]);

  const loadIssues = useCallback(async () => {
    setIssuesLoading(true);
    setIssuesError(null);
    try {
      setIssues(await rainApi.fetchIssues());
    } catch (error) {
      setIssuesError(normalizeApiError(error));
    } finally {
      setIssuesLoading(false);
    }
  }, []);

  const loadBundles = useCallback(async (code: string) => {
    const trimmed = code.trim();
    const requestId = ++bundleRequestIdRef.current;
    if (!trimmed) {
      setBundles([]);
      setBundleFiles({});
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
        const validHashes = new Set(data.log_bundles.map((bundle) => bundle.hash));
        return Object.fromEntries(Object.entries(prev).filter(([hash]) => validHashes.has(hash)));
      });
    } catch (error) {
      if (requestId !== bundleRequestIdRef.current) return;
      const message = normalizeApiError(error);
      if (/not found|404/i.test(message)) {
        setBundles([]);
        setBundleFiles({});
        setBundlesError('Issue 不存在或已被删除');
        if (selectedIssueRef.current === trimmed) {
          setSelectedIssueCode('');
          localStorage.removeItem(LAST_ISSUE_STORAGE_KEY);
        }
        return;
      }
      setBundles([]);
      setBundlesError(message);
    } finally {
      if (requestId === bundleRequestIdRef.current) {
        setBundlesLoading(false);
      }
    }
  }, []);

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
      const files = (response.children ?? []).filter((child) => child.meta?.kind === 'uploaded_file');
      setBundleFiles((prev) => ({
        ...prev,
        [hash]: { files, loading: false, loaded: true, error: null }
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
    for (const bundle of bundles) {
      if (bundle.status.upload_status !== 'READY') continue;
      const state = bundleFiles[bundle.hash];
      if (!state?.loaded && !state?.loading) {
        loadBundleFiles(bundle.hash).catch(() => undefined);
      }
    }
  }, [bundleFiles, bundles, loadBundleFiles]);

  useEffect(() => {
    if (!currentIssueCode) return;
    if (uploadTask?.task_id) return;
    const hasProcessing = bundles.some((bundle) => bundle.status.upload_status === 'PROCESSING');
    if (!hasProcessing) return;
    const timer = window.setTimeout(() => {
      loadBundles(currentIssueCode).catch(() => undefined);
    }, 3000);
    return () => window.clearTimeout(timer);
  }, [bundles, currentIssueCode, loadBundles, uploadTask?.task_id]);

  useEffect(() => {
    const taskId = uploadTask?.task_id;
    if (!taskId) return;
    if (uploadTask.status === 'READY' || uploadTask.status === 'FAILED') return;

    let cancelled = false;
    let timer: number | undefined;
    const poll = async () => {
      if (document.hidden) {
        timer = window.setTimeout(poll, 3000);
        return;
      }
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
        timer = window.setTimeout(poll, 3000);
      } catch (error) {
        if (!cancelled) {
          setUploadError(normalizeApiError(error));
          timer = window.setTimeout(poll, 5000);
        }
      }
    };
    poll().catch(() => undefined);
    return () => {
      cancelled = true;
      if (timer) window.clearTimeout(timer);
    };
  }, [loadBundles, loadIssues, uploadTask?.status, uploadTask?.task_id]);

  const selectIssue = (value: string) => {
    try {
      const code = normalizeIssueCode(value);
      setIssueError(null);
      setSelectedIssueCode(code);
    } catch (error) {
      setIssueError(normalizeApiError(error));
    }
  };

  const closeCreateDialog = () => {
    setCreateDialogOpen(false);
    setCreateIssueError(null);
    setNewIssueCode('');
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
        code
      });
      setIssues((prev) => [issue, ...prev.filter((item) => item.code !== issue.code)]);
      setSelectedIssueCode(issue.code);
      setIssueSearchText('');
      setBundles([]);
      setBundleFiles({});
      closeCreateDialog();
    } catch (error) {
      setCreateIssueError(normalizeApiError(error));
    } finally {
      setCreatingIssue(false);
    }
  };

  const performUpload = async (files: File[]) => {
    if (uploadingRef.current) return;
    if (activeTask) {
      setUploadError('当前上传任务仍在后台解析，请等待完成后再上传');
      return;
    }
    if (!currentIssueCode) {
      setUploadError('请先选择或创建 Issue');
      return;
    }
    if (files.length === 0) {
      setUploadError('请至少选择一个文件');
      return;
    }

    uploadingRef.current = true;
    setUploading(true);
    setUploadProgress(0);
    setUploadError(null);
    setUploadTask(null);
    setUploadSelection(files.map((file) => ({ name: file.name, sizeBytes: file.size })));
    try {
      const response = await rainApi.uploadLogs(currentIssueCode, files, setUploadProgress);
      setUploadTask({
        task_id: response.task_id,
        issue_code: response.issue_code,
        bundle_hash: response.bundle_hash,
        status: response.status,
        stage: response.stage,
        progress_percent: response.status === 'READY' ? 100 : 0,
        total_bytes: response.total_bytes
      });
      await loadBundles(currentIssueCode);
      await loadIssues();
    } catch (error) {
      setUploadError(normalizeApiError(error));
    } finally {
      uploadingRef.current = false;
      setUploading(false);
      setUploadProgress(0);
    }
  };

  const deleteIssue = (code: string) => {
    setConfirmDialog({
      message: `确定删除 Issue ${code} 及其上传吗？此操作不可恢复。`,
      onConfirm: async () => {
        setDeletingIssue(code);
        try {
          await rainApi.deleteIssue(code);
          if (currentIssueCode === code) {
            setSelectedIssueCode('');
            setBundles([]);
            setBundleFiles({});
          }
          await loadIssues();
        } catch (error) {
          setIssuesError(normalizeApiError(error));
        } finally {
          setDeletingIssue(null);
        }
      }
    });
  };

  const deleteRow = (row: FileRow) => {
    const target = row.file ? `文件 ${row.name}` : `日志包 ${row.name}`;
    setConfirmDialog({
      message: `确定删除${target}吗？此操作不可恢复。`,
      onConfirm: async () => {
        setDeletingKey(row.key);
        try {
          if (row.file) {
            await rainApi.deleteFile(row.bundleHash, String(row.file.id));
            await loadBundleFiles(row.bundleHash);
          } else {
            await rainApi.deleteBundle(currentIssueCode, row.bundleHash);
            await loadBundles(currentIssueCode);
          }
          await loadIssues();
        } catch (error) {
          setBundlesError(normalizeApiError(error));
        } finally {
          setDeletingKey(null);
        }
      }
    });
  };

  const handleUpload = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
  };

  return (
    <div className="grid min-h-[calc(100vh-72px)] gap-4 lg:grid-cols-[300px_minmax(0,1fr)]">
      <aside className="flex min-h-[680px] flex-col rounded-lg border border-slate-800 bg-slate-900/70 p-4">
        <div className="mb-4 flex items-center justify-between gap-3">
          <h2 className="text-lg font-semibold text-white">Issues</h2>
          <button
            type="button"
            className="rounded border border-sky-500/60 px-3 py-2 text-sm font-semibold text-sky-300 transition hover:bg-sky-500/10"
            onClick={() => setCreateDialogOpen(true)}
          >
            + 新建 Issue
          </button>
        </div>

        <div className="relative">
          <input
            className="w-full rounded-lg border border-slate-700 bg-slate-950/70 px-4 py-2 pr-10 text-sm text-white outline-none transition focus:border-sky-500"
            placeholder="搜索 Issue ID..."
            value={issueSearchText}
            onChange={(event) => setIssueSearchText(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                event.preventDefault();
                selectIssue(issueSearchText);
              }
            }}
          />
          <span className="absolute right-3 top-2.5 text-slate-500">⌕</span>
        </div>

        {issueError ? <p className="mt-2 text-xs text-rose-300">{issueError}</p> : null}
        {issuesError ? (
          <div className="mt-3 rounded-lg border border-rose-500/30 bg-rose-500/10 p-3 text-xs text-rose-200">
            <p>{issuesError}</p>
            <button
              type="button"
              className="mt-2 rounded border border-rose-400/50 px-2 py-1 text-rose-100 disabled:opacity-60"
              onClick={() => loadIssues().catch(() => undefined)}
              disabled={issuesLoading}
            >
              {issuesLoading ? '连接中...' : '重新连接'}
            </button>
          </div>
        ) : null}

        <div className="mt-5 flex-1 space-y-2 overflow-y-auto">
          {filteredIssues.map((issue) => {
            const active = issue.code === currentIssueCode;
            return (
              <button
                key={issue.code}
                type="button"
                title="双击查看 Issue 日志"
                className={[
                  'flex w-full items-center justify-between rounded-lg px-3 py-2.5 text-left transition',
                  active ? 'bg-sky-500/20 text-white' : 'text-slate-200 hover:bg-slate-800/80'
                ].join(' ')}
                onClick={() => selectIssue(issue.code)}
                onDoubleClick={() => navigate(`/issue/${encodeURIComponent(issue.code)}`)}
              >
                <span className="min-w-0 flex-1">
                  <span className="block truncate font-semibold">{issue.code}</span>
                  <span className="block text-[10px] font-normal text-slate-500">双击查看日志</span>
                </span>
              </button>
            );
          })}
          {!filteredIssues.length ? (
            <p className="rounded-lg border border-slate-800 bg-slate-950/40 p-3 text-sm text-slate-500">
              暂无 Issue
            </p>
          ) : null}
        </div>
      </aside>

      <section className="min-w-0 space-y-4">
        <div className="rounded-lg border border-slate-800 bg-slate-900/70">
          <div className="flex flex-col gap-3 border-b border-slate-800 p-4 md:flex-row md:items-start md:justify-between">
            <div>
              <h2 className="text-2xl font-semibold text-white">
                {currentIssueCode || '请选择 Issue'}
              </h2>
            </div>
            {currentIssueCode ? (
              <button
                type="button"
                className="rounded-lg border border-rose-500/60 px-4 py-2 text-sm font-semibold text-rose-300 transition hover:bg-rose-500/10 disabled:opacity-60"
                disabled={deletingIssue === currentIssueCode}
                onClick={() => deleteIssue(currentIssueCode)}
              >
                删除 Issue
              </button>
            ) : null}
          </div>

          <form onSubmit={handleUpload} className="space-y-3 p-4">
            <h3 className="text-lg font-semibold text-white">上传日志</h3>
            <input
              ref={fileInputRef}
              type="file"
              multiple
              className="hidden"
              disabled={uploadDisabled}
              onChange={(event) => {
                if (uploadDisabled || uploadingRef.current) return;
                const files = event.target.files;
                if (files?.length) {
                  performUpload(Array.from(files)).catch(() => undefined);
                }
                if (fileInputRef.current) {
                  fileInputRef.current.value = '';
                }
              }}
            />
            <div
              className="flex min-h-24 items-center justify-between gap-4 rounded-lg border border-dashed border-slate-700 bg-slate-950/40 px-5 py-4 text-sm transition aria-disabled:opacity-60"
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
                if (uploadDisabled || uploadingRef.current) return;
                if (event.dataTransfer.files.length) {
                  performUpload(Array.from(event.dataTransfer.files)).catch(() => undefined);
                }
              }}
            >
              <div className="flex min-w-0 items-center gap-4">
                <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-full border border-sky-500/60 text-xl text-sky-300">
                  ↑
                </div>
                <div>
                  <p className="font-semibold text-white">
                    {!currentIssueCode
                      ? '先选择或新建 Issue'
                      : uploading
                        ? '正在上传文件'
                        : activeTask
                          ? '后台解析中'
                          : '拖拽日志文件到这里，或点击选择文件'}
                  </p>
                  <p className="mt-1 text-xs text-slate-400">
                    支持 .log、.txt、.zip、.tar.gz、.tgz、.gz，单个文件最大 512 MB
                  </p>
                </div>
              </div>
              <button
                type="button"
                className="shrink-0 rounded-lg bg-sky-600 px-4 py-2 text-sm font-semibold text-white transition hover:bg-sky-500 disabled:opacity-60"
                disabled={uploadDisabled}
                onClick={(event) => {
                  event.stopPropagation();
                  if (!uploadDisabled) fileInputRef.current?.click();
                }}
              >
                选择文件
              </button>
            </div>

            <div className="min-h-7">
              {uploading || activeTask ? (
                <div className="grid gap-3 text-sm text-slate-300 md:grid-cols-[1fr_auto] md:items-center">
                  <div>
                    <div className="h-2 overflow-hidden rounded bg-slate-800">
                      <div
                        className={`h-full bg-sky-500 transition-[width] duration-300 ${activeTask ? 'animate-pulse' : ''}`}
                        style={{ width: `${uploading ? uploadProgress : 100}%` }}
                      />
                    </div>
                  </div>
                  <span>
                    {uploading
                      ? uploadProgress < 100
                        ? `上传 ${uploadProgress}%`
                        : '上传完成，正在提交...'
                      : activeTask
                        ? stageLabel(activeTask.stage)
                        : ''}
                  </span>
                </div>
              ) : null}
            </div>
            {uploadError ? <p className="text-sm text-rose-300">{uploadError}</p> : null}
          </form>
        </div>

        <div className="rounded-lg border border-slate-800 bg-slate-900/70 p-4">
          <div className="mb-4 flex items-center justify-between gap-3">
            <h3 className="text-lg font-semibold text-white">文件列表</h3>
            {bundlesError ? <span className="text-sm text-rose-300">{bundlesError}</span> : null}
          </div>
          <div className="overflow-x-auto rounded-lg border border-slate-800">
            <table className="min-w-full divide-y divide-slate-800 text-sm">
              <thead className="bg-slate-950/40 text-left text-xs uppercase text-slate-400">
                <tr>
                  <th className="px-4 py-2.5 font-medium">文件名</th>
                  <th className="px-4 py-2.5 font-medium">状态</th>
                  <th className="px-4 py-2.5 font-medium">大小</th>
                  <th className="px-4 py-2.5 font-medium">操作</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-slate-800 text-slate-200">
                {fileRows.map((row) => {
                  const deleting = deletingKey === row.key;
                  return (
                    <tr key={row.key}>
                      <td className="max-w-[360px] truncate px-4 py-3">
                        <span className="mr-2 text-slate-500">□</span>
                        {row.name}
                      </td>
                      <td className="px-4 py-3">
                        <span className={`rounded-full border px-2 py-1 text-xs ${stageClass(row.stage)}`}>
                          {stageLabel(row.stage, row.progressPercent)}
                        </span>
                      </td>
                      <td className="px-4 py-3">{formatBytes(row.sizeBytes)}</td>
                      <td className="whitespace-nowrap px-4 py-3">
                        {row.status === 'READY' && row.file ? (
                          <a
                            className="mr-4 text-sky-300 hover:text-sky-200"
                            href={rainApi.fileDownloadUrl(row.bundleHash, String(row.file.id))}
                          >
                            下载
                          </a>
                        ) : null}
                        {row.status === 'PROCESSING' || row.status === 'PENDING' ? (
                          <span className="mr-4 text-slate-500">等待完成</span>
                        ) : null}
                        {row.stage !== 'UPLOADING' ? (
                          <button
                            type="button"
                            className="text-rose-300 hover:text-rose-200 disabled:text-slate-600"
                            disabled={deleting || row.status === 'PROCESSING' || row.status === 'PENDING'}
                            onClick={() => deleteRow(row)}
                          >
                            {deleting ? '删除中...' : '删除'}
                          </button>
                        ) : null}
                      </td>
                    </tr>
                  );
                })}
                {!fileRows.length ? (
                  <tr>
                    <td colSpan={4} className="px-4 py-10 text-center text-slate-500">
                      {currentIssueCode ? '暂无文件' : '请选择一个 Issue'}
                    </td>
                  </tr>
                ) : null}
              </tbody>
            </table>
          </div>
        </div>
      </section>

      {createDialogOpen ? (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/70 p-4"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) closeCreateDialog();
          }}
        >
          <form
            className="w-full max-w-md rounded-lg border border-slate-800 bg-slate-900 p-5 shadow-2xl"
            onSubmit={(event) => {
              event.preventDefault();
              handleCreateIssue().catch(() => undefined);
            }}
            onKeyDown={(event) => {
              if (event.key === 'Escape') closeCreateDialog();
            }}
          >
            <h3 className="text-lg font-semibold text-white">新建 Issue</h3>
            <label className="mt-4 block text-sm text-slate-300">
              Issue ID
              <input
                className="mt-1 w-full rounded-lg border border-slate-700 bg-slate-950 px-4 py-2 text-white outline-none focus:border-sky-500"
                value={newIssueCode}
                onChange={(event) => setNewIssueCode(event.target.value)}
                placeholder="例如 CN014"
              />
            </label>
            {createIssueError ? <p className="mt-3 text-sm text-rose-300">{createIssueError}</p> : null}
            <div className="mt-5 flex justify-end gap-3">
              <button
                type="button"
                className="rounded-lg border border-slate-700 px-4 py-2 text-sm text-slate-200"
                onClick={closeCreateDialog}
                disabled={creatingIssue}
              >
                取消
              </button>
              <button
                type="submit"
                className="rounded-lg bg-sky-600 px-4 py-2 text-sm font-semibold text-white disabled:opacity-60"
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
          <div className="w-full max-w-sm rounded-lg border border-slate-800 bg-slate-900 p-5 shadow-2xl">
            <p className="text-sm text-slate-200">{confirmDialog.message}</p>
            <div className="mt-4 flex justify-end gap-3">
              <button
                type="button"
                className="rounded-lg border border-slate-700 px-4 py-2 text-sm text-slate-200"
                onClick={() => setConfirmDialog(null)}
                disabled={!!confirmDialog.busy}
              >
                取消
              </button>
              <button
                type="button"
                className="rounded-lg bg-rose-500 px-4 py-2 text-sm font-semibold text-white disabled:opacity-60"
                disabled={!!confirmDialog.busy}
                onClick={async () => {
                  if (!confirmDialog) return;
                  setConfirmDialog((prev) => (prev ? { ...prev, busy: true } : prev));
                  try {
                    await confirmDialog.onConfirm();
                  } finally {
                    setConfirmDialog(null);
                  }
                }}
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
