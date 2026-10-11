import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { useAuth } from '../../auth/AuthContext';
import { normalizeApiError, rainApi } from '../../api/client';
import { ConfirmDialog, type ConfirmDialogState } from './components/ConfirmDialog';
import { IssueCreateDialog } from './components/IssueCreateDialog';
import { IssueSelector } from './components/IssueSelector';
import { UploadFileTable } from './components/UploadFileTable';
import { UploadPanel } from './components/UploadPanel';
import { buildFileRows, canDeleteFileRow, type FileRow } from './homeRows';
import { useIssueBundles } from './hooks/useIssueBundles';
import { useIssues } from './hooks/useIssues';
import { useUploadTask } from './hooks/useUploadTask';
import { shouldShowOptimisticUploadTask } from './uploadRows';
import { isUser } from '../../auth/permissions';
import { IssueExpirationNotice } from './components/IssueExpirationNotice';
import { IssueDeleteButton } from './components/IssueDeleteButton';
import type { FileDeletionBatchResponse, FileDeletionJobResponse } from '../../api/types';
import { normalizeDeletionError } from './deleteFeedback';
import { readOnboardingProgress, updateOnboardingProgress } from './onboardingProgress';

const wait = (milliseconds: number) => new Promise<void>((resolve) => window.setTimeout(resolve, milliseconds));

async function waitForFileDeletion(jobId: string, onUpdate: (job: FileDeletionJobResponse) => void) {
  let job = await rainApi.fetchFileDeletionJob(jobId);
  for (let attempt = 0; attempt < 180; attempt += 1) {
    onUpdate(job);
    if (job.status === 'SUCCEEDED' || job.status === 'SUPERSEDED' || job.status === 'RETRY_WAIT') {
      return job;
    }
    await wait(attempt === 0 ? 250 : 750);
    job = await rainApi.fetchFileDeletionJob(jobId);
  }
  throw new Error('删除任务处理超时，请稍后刷新查看状态');
}

async function waitForFileDeletionBatch(batchId: string, onUpdate: (batch: FileDeletionBatchResponse) => void) {
  let batch = await rainApi.fetchFileDeletionBatch(batchId);
  for (let attempt = 0; attempt < 180; attempt += 1) {
    onUpdate(batch);
    if (batch.status === 'SUCCEEDED' || batch.status === 'PARTIAL' || batch.status === 'FAILED') {
      return batch;
    }
    await wait(attempt === 0 ? 250 : 750);
    batch = await rainApi.fetchFileDeletionBatch(batchId);
  }
  throw new Error('批量删除任务处理超时，请稍后刷新查看状态');
}

export function HomeView() {
  const navigate = useNavigate();
  const auth = useAuth();
  const isGuest = auth.state.status === 'GUEST';
  const canCreateIssue = auth.state.status === 'AUTHENTICATED' && isUser(auth.state.user);
  const onboardingIdentity = auth.state.status === 'AUTHENTICATED' ? auth.state.user.id : 'guest';
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const [createDialogOpen, setCreateDialogOpen] = useState(false);
  const [newIssueCode, setNewIssueCode] = useState('');
  const [creatingIssue, setCreatingIssue] = useState(false);
  const [createIssueError, setCreateIssueError] = useState<string | null>(null);
  const [deletingIssue, setDeletingIssue] = useState<string | null>(null);
  const [deletingKey, setDeletingKey] = useState<string | null>(null);
  const [deletingKeys, setDeletingKeys] = useState<Set<string>>(new Set());
  const [selectedRowKeys, setSelectedRowKeys] = useState<Set<string>>(new Set());
  const [deletionBatch, setDeletionBatch] = useState<{
    total: number;
    completed: number;
    failed: string[];
    current: string | null;
  } | null>(null);
  const [deletionJob, setDeletionJob] = useState<{ label: string; job: FileDeletionJobResponse } | null>(null);
  const [confirmDialog, setConfirmDialog] = useState<ConfirmDialogState | null>(null);
  const [guideEnabled, setGuideEnabled] = useState(true);
  const [guideIntroDismissed, setGuideIntroDismissed] = useState(false);
  const [guideCompleted, setGuideCompleted] = useState<string[]>([]);
  const [guideChooserOpen, setGuideChooserOpen] = useState(false);
  const [guideTask, setGuideTask] = useState<'create' | 'upload' | 'delete' | null>(null);
  const [guideStep, setGuideStep] = useState('');
  const [guideUploadBaselineIds, setGuideUploadBaselineIds] = useState<string[]>([]);
  const guideDeleteTargetKeyRef = useRef<string | null>(null);
  const guideDeleteSucceededRef = useRef(false);

  const issues = useIssues();
  const selectedIssue = issues.issues.find((issue) => issue.code === issues.currentIssueCode);

  const handleIssueMissing = useCallback(() => {
    issues.clearSelectedIssue();
    navigate('/', { replace: true });
  }, [issues.clearSelectedIssue, navigate]);

  const bundles = useIssueBundles(issues.currentIssueCode, handleIssueMissing);
  const canWrite = Boolean(bundles.canWrite && (selectedIssue ? selectedIssue.can_write : true));
  const ownerUsername = bundles.ownerUsername ?? selectedIssue?.owner_username ?? null;
  const upload = useUploadTask({
    currentIssueCode: issues.currentIssueCode,
    loadBundles: bundles.loadBundles,
    loadIssues: issues.loadIssues
  });
  const completeHomeGuide = useCallback((task: 'create' | 'upload' | 'delete') => {
    const completed = [...new Set([...guideCompleted, task])];
    setGuideCompleted(completed);
    setGuideIntroDismissed(true);
    updateOnboardingProgress(onboardingIdentity, { introDismissed: true, completed });
    setGuideStep('complete');
  }, [guideCompleted, onboardingIdentity]);

  const readyBundles = bundles.bundles.filter((bundle) => bundle.status.upload_status === 'READY');
  const deleteGuideLoading = bundles.bundlesLoading || readyBundles.some((bundle) => !bundles.bundleFiles[bundle.hash]?.loaded);
  const hasDeletableFiles = readyBundles.some((bundle) => (bundles.bundleFiles[bundle.hash]?.files.length ?? 0) > 0);

  useEffect(() => {
    const progress = readOnboardingProgress(onboardingIdentity);
    setGuideEnabled(progress.enabled);
    setGuideIntroDismissed(progress.introDismissed);
    setGuideCompleted(progress.completed);
    setGuideTask(null);
    setGuideStep('');
  }, [onboardingIdentity]);

  useEffect(() => {
    const openGuide = () => setGuideChooserOpen(true);
    window.addEventListener('rain:open-onboarding', openGuide);
    return () => window.removeEventListener('rain:open-onboarding', openGuide);
  }, []);

  useEffect(() => {
    if (guideTask === 'upload' && guideStep === 'upload-issue' && issues.currentIssueCode && !bundles.bundlesLoading) {
      setGuideStep(canWrite ? 'upload-action' : 'upload-access');
    }
    if (guideTask === 'delete' && guideStep === 'delete-issue' && issues.currentIssueCode && !bundles.bundlesLoading) {
      setGuideStep(canWrite
        ? (deleteGuideLoading ? 'delete-loading' : hasDeletableFiles ? 'delete-action' : 'delete-empty')
        : 'delete-access');
    }
    if (guideTask === 'delete' && guideStep === 'delete-loading' && !deleteGuideLoading) {
      setGuideStep(canWrite
        ? (hasDeletableFiles ? 'delete-action' : 'delete-empty')
        : 'delete-access');
    }
  }, [canWrite, deleteGuideLoading, guideStep, guideTask, hasDeletableFiles, issues.currentIssueCode]);

  useEffect(() => {
    if (guideTask !== 'upload' || guideStep !== 'upload-wait') return;
    const newTasks = upload.tasks.filter((task) => !guideUploadBaselineIds.includes(task.id));
    if (!newTasks.length) return;
    const finished = newTasks.every((task) => ['ACCEPTED', 'HANDED_OFF', 'FAILED', 'UNCONFIRMED'].includes(task.status));
    if (!finished) return;
    if (newTasks.some((task) => task.status === 'FAILED' || task.status === 'UNCONFIRMED')) {
      setGuideStep('upload-failed');
      return;
    }
    completeHomeGuide('upload');
  }, [completeHomeGuide, guideStep, guideTask, guideUploadBaselineIds, upload.tasks]);

  const startHomeGuide = (task: 'create' | 'upload' | 'delete') => {
    setGuideChooserOpen(false);
    setGuideIntroDismissed(true);
    updateOnboardingProgress(onboardingIdentity, { introDismissed: true });
    setGuideTask(task);
    if (task === 'create') {
      setGuideStep(canCreateIssue ? 'create-button' : 'create-access');
    } else if (task === 'upload') {
      setGuideStep(isGuest ? 'upload-access' : !issues.currentIssueCode ? 'upload-issue' : canWrite ? 'upload-action' : 'upload-access');
    } else {
      guideDeleteSucceededRef.current = false;
      guideDeleteTargetKeyRef.current = null;
      setGuideStep(isGuest
        ? 'delete-access'
        : !issues.currentIssueCode
        ? 'delete-issue'
        : deleteGuideLoading
          ? 'delete-loading'
          : !canWrite
            ? 'delete-access'
            : hasDeletableFiles
              ? 'delete-action'
              : 'delete-empty');
    }
  };
  const stopHomeGuide = () => {
    setGuideTask(null);
    setGuideStep('');
    setGuideChooserOpen(false);
    guideDeleteTargetKeyRef.current = null;
  };
  const dismissHomeGuideIntro = () => {
    setGuideIntroDismissed(true);
    updateOnboardingProgress(onboardingIdentity, { introDismissed: true });
  };
  const setHomeGuideEnabled = (enabled: boolean) => {
    setGuideEnabled(enabled);
    updateOnboardingProgress(onboardingIdentity, { enabled });
  };
  const cancelGuideDelete = () => {
    setConfirmDialog(null);
    if (guideTask === 'delete' && guideStep === 'delete-confirm' && !guideDeleteSucceededRef.current) {
      setGuideStep('delete-action');
    }
    guideDeleteTargetKeyRef.current = null;
  };

  const existingBundleHashes = useMemo(
    () => new Set(bundles.bundles.map((bundle) => bundle.hash)),
    [bundles.bundles]
  );

  useEffect(() => {
    upload.acknowledgeAcceptedTasks(existingBundleHashes);
  }, [existingBundleHashes, upload.acknowledgeAcceptedTasks]);

  const visibleUploadTasks = useMemo(
    () => upload.tasks.filter((task) => shouldShowOptimisticUploadTask(task, existingBundleHashes)),
    [existingBundleHashes, upload.tasks]
  );

  const fileRows = useMemo(
    () =>
      buildFileRows({
        bundleFiles: bundles.bundleFiles,
        bundles: bundles.bundles,
        uploadTasks: visibleUploadTasks
      }),
    [bundles.bundleFiles, bundles.bundles, visibleUploadTasks]
  );

  const selectIssue = useCallback(
    (value: string) => {
      const previousIssue = issues.currentIssueCode;
      const nextIssue = issues.selectIssue(value);
      if (nextIssue && nextIssue !== previousIssue) {
        upload.resetSelection();
        setDeletionJob(null);
        setSelectedRowKeys(new Set());
        setDeletionBatch(null);
      }
    },
    [issues, upload]
  );

  const closeCreateDialog = useCallback(() => {
    setCreateDialogOpen(false);
    setCreateIssueError(null);
    setNewIssueCode('');
  }, []);

  const handleCreateIssue = useCallback(async () => {
    setCreatingIssue(true);
    setCreateIssueError(null);
    try {
      await issues.createIssue(newIssueCode);
      bundles.clearBundles();
      upload.resetSelection();
      closeCreateDialog();
      if (guideTask === 'create' && guideStep === 'create-form') completeHomeGuide('create');
    } catch (error) {
      setCreateIssueError(normalizeApiError(error));
    } finally {
      setCreatingIssue(false);
    }
  }, [bundles, closeCreateDialog, completeHomeGuide, guideStep, guideTask, issues, newIssueCode, upload]);

  const deleteIssue = useCallback(
    (code: string) => {
      setConfirmDialog({
        message: `确定删除 Issue ${code} 及其上传吗？此操作不可恢复。`,
        onConfirm: async () => {
          setDeletingIssue(code);
          try {
            await issues.deleteIssue(code);
            if (issues.currentIssueCode === code) {
              bundles.clearBundles();
              upload.resetSelection();
            }
          } catch (error) {
            issues.setIssuesError(normalizeDeletionError(error));
            await bundles.loadBundles(code).catch(() => undefined);
          } finally {
            setDeletingIssue(null);
          }
        }
      });
    },
    [bundles, issues, upload]
  );

  useEffect(() => {
    const validKeys = new Set(fileRows.filter(canDeleteFileRow).map((row) => row.key));
    setSelectedRowKeys((current) => {
      const next = new Set([...current].filter((key) => validKeys.has(key)));
      return next.size === current.size ? current : next;
    });
  }, [fileRows]);

  const deleteRows = useCallback(
    async (requestedRows: FileRow[]) => {
      const rows = requestedRows.filter(canDeleteFileRow);
      if (!rows.length) return;

      setConfirmDialog(null);
      const keys = new Set(rows.map((row) => row.key));
      setDeletingKeys(keys);
      setDeletionBatch({ total: rows.length, completed: 0, failed: [], current: null });

      if (rows.length > 1 && rows.every((row) => row.file)) {
        const labels = new Map(rows.map((row) => [String(row.file?.id ?? ''), row.name]));
        const updateBatchView = (batch: FileDeletionBatchResponse) => {
          const failed = batch.items
            .filter((item) => item.status === 'FAILED')
            .map((item) => `${labels.get(String(item.file_id)) ?? item.file_id}：${item.error_code ?? '删除失败'}`);
          const active = batch.items.find((item) => item.status === 'RUNNING' || item.status === 'QUEUED');
          setDeletionBatch({
            total: batch.total_items,
            completed: batch.completed_items,
            failed,
            current: active ? labels.get(String(active.file_id)) ?? null : null
          });
        };
        try {
          const batch = await rainApi.createFileDeletionBatch(rows.map((row) => ({
            bundle_id: row.bundleHash,
            file_id: String(row.file?.id ?? '')
          })));
          updateBatchView(batch);
          await waitForFileDeletionBatch(batch.batch_id, updateBatchView);
        } catch (error) {
          const message = normalizeDeletionError(error);
          bundles.setBundlesError(message);
          setDeletionBatch({
            total: rows.length,
            completed: 0,
            failed: rows.map((row) => `${row.name}：${message}`),
            current: null
          });
        } finally {
          setDeletingKeys(new Set());
          setSelectedRowKeys((current) => {
            const next = new Set(current);
            for (const row of rows) next.delete(row.key);
            return next;
          });
          setDeletingKey(null);
          await Promise.allSettled([
            ...[...new Set(rows.map((row) => row.bundleHash))].map((bundleHash) => bundles.loadBundleFiles(bundleHash)),
            bundles.loadBundles(issues.currentIssueCode),
            issues.loadIssues()
          ]);
        }
        return;
      }

      let completed = 0;
      const failed: string[] = [];
      for (const row of rows) {
        const target = row.file ? `文件 ${row.name}` : `日志包 ${row.name}`;
        setDeletingKey(row.key);
        setDeletionBatch((current) => current ? { ...current, current: target } : current);
        try {
          if (row.file) {
            const queuedJob = await rainApi.deleteFile(row.bundleHash, String(row.file.id));
            setDeletionJob({ label: target, job: queuedJob });
            const finishedJob = await waitForFileDeletion(queuedJob.job_id, (job) => {
              setDeletionJob((current) => current ? { ...current, job } : { label: target, job });
            });
            if (finishedJob.status === 'RETRY_WAIT') {
              throw new Error(finishedJob.last_error_code || '删除任务等待重试');
            }
            await bundles.loadBundleFiles(row.bundleHash);
            if (guideTask === 'delete' && guideDeleteTargetKeyRef.current === row.key) {
              guideDeleteSucceededRef.current = true;
              guideDeleteTargetKeyRef.current = null;
              completeHomeGuide('delete');
            }
          } else {
            await rainApi.deleteBundle(issues.currentIssueCode, row.bundleHash);
            await bundles.loadBundles(issues.currentIssueCode);
          }
          completed += 1;
        } catch (error) {
          failed.push(`${row.name}：${normalizeDeletionError(error)}`);
          bundles.setBundlesError(normalizeDeletionError(error));
          if (guideTask === 'delete' && guideDeleteTargetKeyRef.current === row.key) setGuideStep('delete-error');
        } finally {
          setDeletionBatch({ total: rows.length, completed, failed: [...failed], current: null });
          setDeletingKeys((current) => {
            const next = new Set(current);
            next.delete(row.key);
            return next;
          });
          setSelectedRowKeys((current) => {
            const next = new Set(current);
            next.delete(row.key);
            return next;
          });
          setDeletingKey(null);
        }
      }

      await Promise.allSettled([
        bundles.loadBundles(issues.currentIssueCode),
        issues.loadIssues()
      ]);
      setDeletionBatch({ total: rows.length, completed, failed, current: null });
    },
    [bundles, completeHomeGuide, guideStep, guideTask, issues]
  );

  const deleteRow = useCallback((row: FileRow) => {
    const target = row.file ? `文件 ${row.name}` : `压缩包 ${row.name}`;
    if (guideTask === 'delete' && guideStep === 'delete-action' && row.file) {
      guideDeleteSucceededRef.current = false;
      guideDeleteTargetKeyRef.current = row.key;
      setGuideStep('delete-confirm');
    }
    setConfirmDialog({
      message: `确定删除${target}吗？此操作不可恢复。`,
      onConfirm: () => deleteRows([row])
    });
  }, [deleteRows, guideStep, guideTask]);

  const handleFilesSelected = (files: File[]) => {
    if (guideTask === 'upload' && guideStep === 'upload-action') {
      setGuideUploadBaselineIds(upload.tasks.map((task) => task.id));
      setGuideStep('upload-wait');
    }
    upload.performUpload(files).catch(() => undefined);
  };

  const deleteSelectedRows = useCallback(() => {
    const rows = fileRows.filter((row) => selectedRowKeys.has(row.key));
    if (!rows.length) return;
    const preview = rows.slice(0, 3).map((row) => row.name).join('、');
    const suffix = rows.length > 3 ? ` 等 ${rows.length} 项` : '';
    setConfirmDialog({
      message: `确定删除选中的 ${rows.length} 项（${preview}${suffix}）吗？此操作不可恢复。`,
      onConfirm: () => deleteRows(rows)
    });
  }, [deleteRows, fileRows, selectedRowKeys]);

  useEffect(() => {
    if (!deletionJob || deletionJob.job.status === 'SUCCEEDED' || deletionJob.job.status === 'SUPERSEDED' || deletionBatch) {
      return undefined;
    }
    const timer = window.setTimeout(() => {
      rainApi.fetchFileDeletionJob(deletionJob.job.job_id)
        .then((job) => setDeletionJob((current) => current ? { ...current, job } : null))
        .catch(() => undefined);
    }, 3000);
    return () => window.clearTimeout(timer);
  }, [deletionBatch, deletionJob]);

  const deletionStatus = deletionJob?.job.status === 'RETRY_WAIT'
    ? '清理暂未完成，系统将自动重试'
    : deletionJob?.job.status === 'SUCCEEDED'
      ? '删除完成'
      : deletionJob?.job.status === 'SUPERSEDED'
        ? '已由日志包删除任务接管'
        : '后台删除中';

  return (
    <div className="space-y-4">
      {guideEnabled && !guideIntroDismissed && auth.state.status !== 'LOADING' ? (
        isGuest ? (
          <section aria-label="访客快速上手" className="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-cyan-200 bg-cyan-50 px-4 py-3 text-sm">
            <div><p className="font-semibold text-slate-900">访客模式</p><p className="mt-0.5 text-slate-600">你可以浏览、搜索和分享日志。登录普通用户后可新建 Issue、保存搜索条件；上传或删除还需要对应 Issue 的写入权限。</p></div>
            <div className="flex flex-wrap gap-2">
              <button type="button" className="rounded-md bg-slate-950 px-3 py-2 font-semibold text-white" onClick={() => navigate('/login', { state: { from: '/' } })}>登录 / 注册</button>
              <button type="button" className="rounded-md px-2 py-2 text-slate-600 hover:bg-white" onClick={dismissHomeGuideIntro}>知道了</button>
            </div>
          </section>
        ) : (
          <section aria-label="快速上手" className="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-cyan-200 bg-cyan-50 px-4 py-3 text-sm">
            <div><p className="font-semibold text-slate-900">快速上手</p><p className="mt-0.5 text-slate-600">学习新建 Issue、上传压缩包和删除日志文件。</p></div>
            <div className="flex flex-wrap gap-2">
              <button type="button" className="rounded-md bg-slate-950 px-3 py-2 font-semibold text-white" onClick={() => startHomeGuide('create')}>新建 Issue</button>
              <button type="button" className="rounded-md border border-sky-300 bg-white px-3 py-2 font-semibold text-sky-800" onClick={() => startHomeGuide('upload')}>上传压缩包</button>
              <button type="button" className="rounded-md border border-sky-300 bg-white px-3 py-2 font-semibold text-sky-800" onClick={() => startHomeGuide('delete')}>删除日志文件</button>
              <button type="button" className="rounded-md px-2 py-2 text-slate-600 hover:bg-white" onClick={dismissHomeGuideIntro}>暂时跳过</button>
            </div>
          </section>
        )
      ) : null}
      <div className="grid min-h-[calc(100vh-72px)] gap-4 lg:grid-cols-[300px_minmax(0,1fr)]">
      <IssueSelector
        currentIssueCode={issues.currentIssueCode}
        filteredIssues={issues.filteredIssues}
        issueError={issues.issueError}
        issueSearchText={issues.issueSearchText}
        issueScope={issues.issueScope}
        issuesError={issues.issuesError}
        issuesLoading={issues.issuesLoading}
        issuesLoadingMore={issues.issuesLoadingMore}
        hasMoreIssues={issues.hasMoreIssues}
        canChooseScope={issues.canChooseScope}
        canWrite={canWrite}
        canCreateIssue={canCreateIssue}
        highlightCreateButton={guideTask === 'create' && guideStep === 'create-button'}
        onCreateClick={() => {
          setCreateDialogOpen(true);
          if (guideTask === 'create' && guideStep === 'create-button') setGuideStep('create-form');
        }}
        onIssueScopeChange={issues.changeIssueScope}
        onIssueSearchTextChange={issues.setIssueSearchText}
        onRefreshIssues={() => issues.loadIssues().catch(() => undefined)}
        onLoadMoreIssues={() => issues.loadMoreIssues().catch(() => undefined)}
        onSelectIssue={selectIssue}
        onViewIssue={(issueCode) => navigate(`/issue/${encodeURIComponent(issueCode)}`)}
      />

      <section className="min-w-0 space-y-4">
        <div className="overflow-hidden rounded-2xl border border-slate-200/90 bg-white/95 shadow-[0_18px_48px_rgba(7,21,34,0.08)] backdrop-blur">
          <div className="flex flex-col gap-3 border-b border-slate-200 p-4 md:flex-row md:items-start md:justify-between">
            <div>
              <h2 className="text-2xl font-semibold text-slate-950">
                {issues.currentIssueCode || '请选择 Issue'}
              </h2>
              {selectedIssue && ownerUsername ? <p className="mt-1 text-sm text-slate-500">所有者：{ownerUsername}</p> : null}
              <IssueExpirationNotice
                canWrite={canWrite}
                expiry={bundles.inactivityExpiry}
              />
            </div>
            <IssueDeleteButton
              issueCode={issues.currentIssueCode}
              canWrite={canWrite}
              blocked={upload.uploading || bundles.hasProcessingBundles}
              deleting={deletingIssue === issues.currentIssueCode}
              onDelete={() => deleteIssue(issues.currentIssueCode)}
            />
          </div>

          {canWrite ? (
            <UploadPanel
              currentIssueCode={issues.currentIssueCode}
              canWrite={canWrite}
              fileInputRef={fileInputRef}
              onFilesSelected={handleFilesSelected}
              uploadDisabled={upload.uploadDisabled}
              uploadError={upload.uploadError}
              uploadTasks={visibleUploadTasks}
              uploading={upload.uploading}
              highlightUpload={guideTask === 'upload' && guideStep === 'upload-action'}
            />
          ) : null}
        </div>

        {deletionJob || deletionBatch ? (
          <div className="rounded-xl border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-900">
            {deletionBatch ? (
              <>
                <span className="font-semibold">
                  {deletionBatch.completed + deletionBatch.failed.length >= deletionBatch.total
                    ? deletionBatch.failed.length
                      ? `已处理 ${deletionBatch.completed}/${deletionBatch.total} 项`
                      : `已删除 ${deletionBatch.total} 项`
                    : `正在删除 ${deletionBatch.completed}/${deletionBatch.total} 项`}
                </span>
                {deletionBatch.current ? `：${deletionBatch.current}` : null}
                {deletionBatch.failed.length ? (
                  <>
                    <span className="ml-2 text-rose-700">失败 {deletionBatch.failed.length} 项</span>
                    <ul className="mt-2 space-y-1 text-xs text-rose-700">
                      {deletionBatch.failed.map((message) => <li key={message}>{message}</li>)}
                    </ul>
                  </>
                ) : null}
              </>
            ) : (
              <><span className="font-semibold">{deletionJob?.label ?? '删除任务'}</span>：{deletionStatus}</>
            )}
            {!deletionBatch && deletionJob?.job.status === 'RETRY_WAIT' && deletionJob.job.last_error_code ? (
              <span className="ml-2 text-amber-700">（{deletionJob.job.last_error_code}）</span>
            ) : null}
          </div>
        ) : null}

        <UploadFileTable
          bundlesError={bundles.bundlesError}
          currentIssueCode={issues.currentIssueCode}
          deletingKey={deletingKey}
          deletingKeys={deletingKeys}
          selectedRowKeys={selectedRowKeys}
          fileRows={fileRows}
          canWrite={canWrite}
          onDeleteRow={deleteRow}
          onToggleRow={(row) => setSelectedRowKeys((current) => {
            const next = new Set(current);
            if (next.has(row.key)) next.delete(row.key); else next.add(row.key);
            return next;
          })}
          onToggleAll={(checked, rows) => setSelectedRowKeys((current) => {
            const next = new Set(current);
            for (const row of rows) {
              if (checked) next.add(row.key); else next.delete(row.key);
            }
            return next;
          })}
          onClearSelection={() => setSelectedRowKeys(new Set())}
          onDeleteSelected={deleteSelectedRows}
          onRetryUpload={upload.retryUpload}
        />
      </section>

      </div>

      {createDialogOpen ? (
        <IssueCreateDialog
          creating={creatingIssue}
          error={createIssueError}
          issueCode={newIssueCode}
          onChangeIssueCode={setNewIssueCode}
          onClose={closeCreateDialog}
          onSubmit={() => handleCreateIssue().catch(() => undefined)}
          guidance={guideTask === 'create' && guideStep === 'create-form' ? '给 Issue 填一个容易识别的编号，然后点击“创建”。' : undefined}
        />
      ) : null}

      {confirmDialog ? (
        <ConfirmDialog
          dialog={confirmDialog}
          onBusyChange={(busy) => setConfirmDialog((prev) => (prev ? { ...prev, busy } : prev))}
          onCancel={cancelGuideDelete}
          onClose={cancelGuideDelete}
        />
      ) : null}
      {guideChooserOpen ? (
        <div role="dialog" aria-label="新手指引" className="fixed right-4 top-20 z-[70] w-[min(23rem,calc(100vw-2rem))] rounded-xl border border-slate-200 bg-white p-4 shadow-2xl">
          <div className="flex items-start justify-between gap-3">
            <div><h2 className="font-semibold text-slate-950">你想了解什么？</h2><p className="mt-1 text-xs text-slate-500">{isGuest ? '访客可以浏览、搜索和分享；创建、上传和删除需要登录及相应权限。' : '教程会跟随实际操作推进，随时可以退出。'}</p></div>
            <button type="button" aria-label="关闭新手指引选择" className="rounded px-2 text-xl text-slate-500 hover:bg-slate-100" onClick={() => setGuideChooserOpen(false)}>×</button>
          </div>
          <label className="mt-3 flex cursor-pointer items-center justify-between rounded-lg bg-slate-50 px-3 py-2 text-sm text-slate-700"><span>自动显示新手提示</span><input type="checkbox" checked={guideEnabled} onChange={(event) => setHomeGuideEnabled(event.target.checked)} /></label>
          <div className="mt-3 grid gap-2">
            <button type="button" className="rounded-lg border border-slate-200 p-3 text-left hover:border-sky-400 hover:bg-sky-50" onClick={() => startHomeGuide('create')}><span className="block text-sm font-semibold">新建 Issue{isGuest ? '（登录后可用）' : ''}</span><span className="mt-1 block text-xs text-slate-600">{isGuest ? '登录普通用户账号后可创建。' : '创建一个 Issue 来整理日志。'}</span></button>
            <button type="button" className="rounded-lg border border-slate-200 p-3 text-left hover:border-sky-400 hover:bg-sky-50" onClick={() => startHomeGuide('upload')}><span className="block text-sm font-semibold">上传压缩包{isGuest ? '（登录后可用）' : ''}</span><span className="mt-1 block text-xs text-slate-600">{isGuest ? '登录普通用户账号，并对目标 Issue 有写入权限。' : '将日志包添加到一个有写入权限的 Issue。'}</span></button>
            <button type="button" className="rounded-lg border border-slate-200 p-3 text-left hover:border-sky-400 hover:bg-sky-50" onClick={() => startHomeGuide('delete')}><span className="block text-sm font-semibold">删除日志文件{isGuest ? '（登录后可用）' : ''}</span><span className="mt-1 block text-xs text-slate-600">{isGuest ? '登录普通用户账号，并对目标 Issue 有写入权限。' : '从当前 Issue 的文件列表中移除不需要的文件。'}</span></button>
          </div>
        </div>
      ) : null}
      {guideTask ? (
        <aside aria-live="polite" className="fixed bottom-4 right-4 z-40 w-[min(26rem,calc(100vw-2rem))] rounded-xl border border-sky-200 bg-white p-4 shadow-2xl">
          <div className="flex items-start justify-between gap-3"><div><p className="text-xs font-semibold uppercase tracking-wide text-sky-700">新手指引 · {guideTask === 'create' ? '新建 Issue' : guideTask === 'upload' ? '上传压缩包' : '删除日志文件'}</p><p className="mt-1 text-sm leading-5 text-slate-700">
            {guideStep === 'create-button' ? '点击左侧“新建 Issue”，然后填写一个容易识别的 Issue ID。' : null}
            {guideStep === 'create-form' ? '在弹窗里填写 Issue ID 并点击创建。只有创建成功后教程才会完成。' : null}
            {guideStep === 'create-access' ? '新建 Issue 需要普通用户账户。' : null}
            {guideStep === 'upload-issue' ? '先选择一个 Issue，上传的压缩包会放在该 Issue 下面。' : null}
            {guideStep === 'upload-action' ? '点击“选择文件”或把 .zip、.7z、.tar.gz 等支持的日志压缩包拖到上传区域。' : null}
            {guideStep === 'upload-wait' ? '上传已加入任务队列。Rain 会在后台接收、校验、解压并建立索引。' : null}
            {guideStep === 'upload-failed' ? '上传没有成功。查看上传区域的错误信息，然后重试；教程仍可从这里重新打开。' : null}
            {guideStep === 'upload-access' || guideStep === 'delete-access' ? (isGuest ? '访客不能上传或删除文件。登录普通用户账号后，还需要对目标 Issue 有写入权限。' : '这个 Issue 是只读的。上传和删除文件需要该 Issue 的写入权限。') : null}
            {guideStep === 'delete-issue' ? '先选择包含待删除文件的 Issue。' : null}
            {guideStep === 'delete-loading' ? '正在读取 Issue 中的文件…' : null}
            {guideStep === 'delete-action' ? '在“文件列表”中找到要删除的日志文件，点击这一行的“删除”。这只会删除选中的文件。' : null}
            {guideStep === 'delete-confirm' ? '确认窗口会说明将删除的文件；确认后，这个文件会从当前 Issue 中移除。' : null}
            {guideStep === 'delete-empty' ? '当前 Issue 没有可删除的日志文件。先选择一个包含已处理日志的 Issue。' : null}
            {guideStep === 'delete-error' ? '删除请求失败。检查页面上的错误信息后，可以重新尝试。' : null}
            {guideStep === 'complete' ? (guideTask === 'create' ? 'Issue 已创建。现在可以选择它并上传日志包。' : guideTask === 'upload' ? '服务器已接收压缩包，后台会继续校验、解压并建立索引。' : '日志文件已删除；同一压缩包中的其他文件不受影响。') : null}
          </p></div><button type="button" aria-label="退出新手指引" className="-mr-2 -mt-2 rounded px-2 text-xl text-slate-500 hover:bg-slate-100" onClick={stopHomeGuide}>×</button></div>
          <div className="mt-3 flex justify-end gap-2">
            {(guideStep === 'create-access' || guideStep === 'upload-access' || guideStep === 'delete-access') && isGuest ? <button type="button" className="rounded-md bg-slate-950 px-3 py-2 text-xs font-semibold text-white" onClick={() => navigate('/login', { state: { from: '/' } })}>登录 / 注册</button> : null}
            {guideStep === 'delete-empty' ? <button type="button" className="rounded-md border border-slate-300 px-3 py-2 text-xs font-semibold text-slate-700" onClick={() => startHomeGuide('upload')}>去上传压缩包</button> : null}
            {guideStep === 'upload-failed' || guideStep === 'delete-error' ? <button type="button" className="rounded-md border border-slate-300 px-3 py-2 text-xs font-semibold text-slate-700" onClick={() => setGuideStep(guideTask === 'upload' ? 'upload-action' : 'delete-action')}>重试</button> : null}
            {guideStep === 'complete' ? <button type="button" className="rounded-md bg-slate-950 px-3 py-2 text-xs font-semibold text-white" onClick={stopHomeGuide}>完成</button> : null}
            {guideStep !== 'complete' && guideStep !== 'create-access' && guideStep !== 'upload-access' && guideStep !== 'delete-access' ? <button type="button" className="rounded-md border border-slate-300 px-3 py-2 text-xs font-semibold text-slate-700" onClick={stopHomeGuide}>退出指引</button> : null}
          </div>
        </aside>
      ) : null}
    </div>
  );
}
