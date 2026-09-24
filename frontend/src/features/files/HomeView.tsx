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
import { isUser } from '../../auth/permissions';
import { IssueExpirationNotice } from './components/IssueExpirationNotice';
import { IssueDeleteButton } from './components/IssueDeleteButton';
import type { FileDeletionBatchResponse, FileDeletionJobResponse } from '../../api/types';
import { normalizeDeletionError } from './deleteFeedback';

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
  const canCreateIssue = auth.state.status === 'AUTHENTICATED' && isUser(auth.state.user);
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

  const issues = useIssues();
  const selectedIssue = issues.issues.find((issue) => issue.code === issues.currentIssueCode);

  const handleIssueMissing = useCallback(() => {
    issues.clearSelectedIssue();
    navigate('/', { replace: true });
  }, [issues.clearSelectedIssue, navigate]);

  const bundles = useIssueBundles(issues.currentIssueCode, handleIssueMissing);
  const canWrite = Boolean(selectedIssue?.can_write && bundles.canWrite);
  const ownerUsername = bundles.ownerUsername ?? selectedIssue?.owner_username ?? null;
  const upload = useUploadTask({
    currentIssueCode: issues.currentIssueCode,
    loadBundles: bundles.loadBundles,
    loadIssues: issues.loadIssues
  });
  const visibleUploadTasks = useMemo(
    () => upload.tasks.filter(
      (task) => task.status !== 'ACCEPTED' || !task.response || !bundles.bundles.some((bundle) => bundle.hash === task.response?.bundle_hash)
    ),
    [bundles.bundles, upload.tasks]
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
    } catch (error) {
      setCreateIssueError(normalizeApiError(error));
    } finally {
      setCreatingIssue(false);
    }
  }, [bundles, closeCreateDialog, issues, newIssueCode, upload]);

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
        const labels = new Map(rows.map((row) => [`${row.bundleHash}:${row.file?.id ?? ''}`, row.name]));
        const updateBatchView = (batch: FileDeletionBatchResponse) => {
          const failed = batch.items
            .filter((item) => item.status === 'FAILED')
            .map((item) => `${labels.get(`${item.bundle_id}:${item.file_id}`) ?? item.file_id}：${item.error_code ?? '删除失败'}`);
          const active = batch.items.find((item) => item.status === 'RUNNING' || item.status === 'QUEUED');
          setDeletionBatch({
            total: batch.total_items,
            completed: batch.completed_items,
            failed,
            current: active ? labels.get(`${active.bundle_id}:${active.file_id}`) ?? null : null
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
          } else {
            await rainApi.deleteBundle(issues.currentIssueCode, row.bundleHash);
            await bundles.loadBundles(issues.currentIssueCode);
          }
          completed += 1;
        } catch (error) {
          failed.push(`${row.name}：${normalizeDeletionError(error)}`);
          bundles.setBundlesError(normalizeDeletionError(error));
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
    [bundles, issues]
  );

  const deleteRow = useCallback((row: FileRow) => {
    const target = row.file ? `文件 ${row.name}` : `日志包 ${row.name}`;
    setConfirmDialog({
      message: `确定删除${target}吗？此操作不可恢复。`,
      onConfirm: () => deleteRows([row])
    });
  }, [deleteRows]);

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
    <div className="grid min-h-[calc(100vh-72px)] gap-4 lg:grid-cols-[300px_minmax(0,1fr)]">
      <IssueSelector
        currentIssueCode={issues.currentIssueCode}
        filteredIssues={issues.filteredIssues}
        issueError={issues.issueError}
        issueSearchText={issues.issueSearchText}
        issuesError={issues.issuesError}
        issuesLoading={issues.issuesLoading}
        canWrite={canWrite}
        canCreateIssue={canCreateIssue}
        onCreateClick={() => setCreateDialogOpen(true)}
        onIssueSearchTextChange={issues.setIssueSearchText}
        onRefreshIssues={() => issues.loadIssues().catch(() => undefined)}
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
              onFilesSelected={(files) => upload.performUpload(files).catch(() => undefined)}
              uploadDisabled={upload.uploadDisabled}
              uploadError={upload.uploadError}
              uploadTasks={visibleUploadTasks}
              uploading={upload.uploading}
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
                  <span className="ml-2 text-rose-700">失败 {deletionBatch.failed.length} 项</span>
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

      {createDialogOpen ? (
        <IssueCreateDialog
          creating={creatingIssue}
          error={createIssueError}
          issueCode={newIssueCode}
          onChangeIssueCode={setNewIssueCode}
          onClose={closeCreateDialog}
          onSubmit={() => handleCreateIssue().catch(() => undefined)}
        />
      ) : null}

      {confirmDialog ? (
        <ConfirmDialog
          dialog={confirmDialog}
          onBusyChange={(busy) => setConfirmDialog((prev) => (prev ? { ...prev, busy } : prev))}
          onCancel={() => setConfirmDialog(null)}
          onClose={() => setConfirmDialog(null)}
        />
      ) : null}
    </div>
  );
}
