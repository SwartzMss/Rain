import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { useAuth } from '../../auth/AuthContext';
import { normalizeApiError, rainApi } from '../../api/client';
import { ConfirmDialog, type ConfirmDialogState } from './components/ConfirmDialog';
import { IssueCreateDialog } from './components/IssueCreateDialog';
import { IssueSelector } from './components/IssueSelector';
import { UploadFileTable } from './components/UploadFileTable';
import { UploadPanel } from './components/UploadPanel';
import { buildFileRows, type FileRow } from './homeRows';
import { useIssueBundles } from './hooks/useIssueBundles';
import { useIssues } from './hooks/useIssues';
import { useUploadTask } from './hooks/useUploadTask';
import { isUser } from '../../auth/permissions';
import { IssueExpirationNotice } from './components/IssueExpirationNotice';
import { IssueDeleteButton } from './components/IssueDeleteButton';
import type { FileDeletionJobResponse } from '../../api/types';
import { normalizeDeletionError } from './deleteFeedback';

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

  const fileRows = useMemo(
    () =>
      buildFileRows({
        bundleFiles: bundles.bundleFiles,
        bundles: bundles.bundles,
        uploadFailed: upload.uploadFailed,
        uploadProgress: upload.uploadProgress,
        uploadSelection: upload.uploadSelection,
        uploading: upload.uploading
      }),
    [
      bundles.bundleFiles,
      bundles.bundles,
      upload.uploadFailed,
      upload.uploadProgress,
      upload.uploadSelection,
      upload.uploading
    ]
  );

  const selectIssue = useCallback(
    (value: string) => {
      const previousIssue = issues.currentIssueCode;
      const nextIssue = issues.selectIssue(value);
      if (nextIssue && nextIssue !== previousIssue) {
        upload.resetSelection();
        setDeletionJob(null);
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

  const deleteRow = useCallback(
    (row: FileRow) => {
      const target = row.file ? `文件 ${row.name}` : `日志包 ${row.name}`;
      setConfirmDialog({
        message: `确定删除${target}吗？此操作不可恢复。`,
        onConfirm: async () => {
          setDeletingKey(row.key);
          try {
            if (row.file) {
              const job = await rainApi.deleteFile(row.bundleHash, String(row.file.id));
              setDeletionJob({ label: target, job });
              setConfirmDialog(null);
              await bundles.loadBundleFiles(row.bundleHash);
            } else {
              await rainApi.deleteBundle(issues.currentIssueCode, row.bundleHash);
              await bundles.loadBundles(issues.currentIssueCode);
            }
            await issues.loadIssues();
          } catch (error) {
            bundles.setBundlesError(normalizeDeletionError(error));
            await Promise.allSettled([
              bundles.loadBundles(issues.currentIssueCode),
              issues.loadIssues()
            ]);
          } finally {
            setDeletingKey(null);
          }
        }
      });
    },
    [bundles, issues]
  );

  useEffect(() => {
    if (!deletionJob || deletionJob.job.status === 'SUCCEEDED' || deletionJob.job.status === 'SUPERSEDED') {
      return undefined;
    }
    const timer = window.setTimeout(() => {
      rainApi.fetchFileDeletionJob(deletionJob.job.job_id)
        .then((job) => setDeletionJob((current) => current ? { ...current, job } : null))
        .catch(() => undefined);
    }, 3000);
    return () => window.clearTimeout(timer);
  }, [deletionJob]);

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
              checking={upload.checking}
              uploadNotice={upload.uploadNotice}
              uploading={upload.uploading}
              uploadingRef={upload.uploadingRef}
            />
          ) : null}
        </div>

        {deletionJob ? (
          <div className="rounded-xl border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-900">
            <span className="font-semibold">{deletionJob.label}</span>：{deletionStatus}
            {deletionJob.job.status === 'RETRY_WAIT' && deletionJob.job.last_error_code ? (
              <span className="ml-2 text-amber-700">（{deletionJob.job.last_error_code}）</span>
            ) : null}
          </div>
        ) : null}

        <UploadFileTable
          bundlesError={bundles.bundlesError}
          currentIssueCode={issues.currentIssueCode}
          deletingKey={deletingKey}
          fileRows={fileRows}
          canWrite={canWrite}
          onDeleteRow={deleteRow}
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
