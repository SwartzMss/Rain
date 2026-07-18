import { useCallback, useMemo, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
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

export function HomeView() {
  const navigate = useNavigate();
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const [createDialogOpen, setCreateDialogOpen] = useState(false);
  const [newIssueCode, setNewIssueCode] = useState('');
  const [creatingIssue, setCreatingIssue] = useState(false);
  const [createIssueError, setCreateIssueError] = useState<string | null>(null);
  const [deletingIssue, setDeletingIssue] = useState<string | null>(null);
  const [deletingKey, setDeletingKey] = useState<string | null>(null);
  const [confirmDialog, setConfirmDialog] = useState<ConfirmDialogState | null>(null);

  const issues = useIssues();
  const selectedIssueRef = useRef(issues.currentIssueCode);
  selectedIssueRef.current = issues.currentIssueCode;

  const bundles = useIssueBundles(issues.currentIssueCode, issues.clearSelectedIssue);
  const upload = useUploadTask({
    currentIssueCode: issues.currentIssueCode,
    getSelectedIssueCode: () => selectedIssueRef.current,
    hasActiveBundleProcessing: bundles.bundles.some(
      (bundle) => bundle.status.upload_status === 'PROCESSING'
    ),
    loadBundles: bundles.loadBundles,
    loadIssues: issues.loadIssues
  });

  const fileRows = useMemo(
    () =>
      buildFileRows({
        activeTask: upload.activeTask,
        bundleFiles: bundles.bundleFiles,
        bundles: bundles.bundles,
        uploadFailed: upload.uploadFailed,
        uploadProgress: upload.uploadProgress,
        uploadSelection: upload.uploadSelection,
        uploadTask: upload.uploadTask,
        uploading: upload.uploading
      }),
    [
      bundles.bundleFiles,
      bundles.bundles,
      upload.activeTask,
      upload.uploadFailed,
      upload.uploadProgress,
      upload.uploadSelection,
      upload.uploadTask,
      upload.uploading
    ]
  );

  const selectIssue = useCallback(
    (value: string) => {
      const previousIssue = issues.currentIssueCode;
      const nextIssue = issues.selectIssue(value);
      if (nextIssue && nextIssue !== previousIssue) {
        upload.resetSelection();
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
            issues.setIssuesError(normalizeApiError(error));
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
              await rainApi.deleteFile(row.bundleHash, String(row.file.id));
              await bundles.loadBundleFiles(row.bundleHash);
            } else {
              await rainApi.deleteBundle(issues.currentIssueCode, row.bundleHash);
              await bundles.loadBundles(issues.currentIssueCode);
            }
            await issues.loadIssues();
          } catch (error) {
            bundles.setBundlesError(normalizeApiError(error));
          } finally {
            setDeletingKey(null);
          }
        }
      });
    },
    [bundles, issues]
  );

  return (
    <div className="grid min-h-[calc(100vh-72px)] gap-4 lg:grid-cols-[300px_minmax(0,1fr)]">
      <IssueSelector
        currentIssueCode={issues.currentIssueCode}
        filteredIssues={issues.filteredIssues}
        issueError={issues.issueError}
        issueSearchText={issues.issueSearchText}
        issuesError={issues.issuesError}
        issuesLoading={issues.issuesLoading}
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
            </div>
            {issues.currentIssueCode ? (
              <button
                type="button"
                className="rounded-lg border border-rose-500/60 px-4 py-2 text-sm font-semibold text-rose-600 transition hover:bg-rose-500/10 disabled:opacity-60"
                disabled={deletingIssue === issues.currentIssueCode}
                onClick={() => deleteIssue(issues.currentIssueCode)}
              >
                删除 Issue
              </button>
            ) : null}
          </div>

          <UploadPanel
            activeTask={upload.activeTask}
            currentIssueCode={issues.currentIssueCode}
            fileInputRef={fileInputRef}
            onFilesSelected={(files) => upload.performUpload(files).catch(() => undefined)}
            uploadDisabled={upload.uploadDisabled}
            uploadError={upload.uploadError}
            uploading={upload.uploading}
            uploadingRef={upload.uploadingRef}
          />
        </div>

        <UploadFileTable
          bundlesError={bundles.bundlesError}
          currentIssueCode={issues.currentIssueCode}
          deletingKey={deletingKey}
          fileRows={fileRows}
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
