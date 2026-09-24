import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { normalizeApiError } from '../../../api/client';
import type { UploadResponse } from '../../../api/types';
import {
  createUploadQueue,
  type UploadQueueTask,
  type UploadQueueTaskStatus
} from '../uploadQueue';
import type { UploadSelectionItem } from '../uploadRows';
import { uploadFileWithResume } from '../resumableUpload';

const uploadQueue = createUploadQueue<UploadResponse>(uploadFileWithResume);

const transportStatuses: UploadQueueTaskStatus[] = ['QUEUED', 'UPLOADING', 'RETRY_WAIT'];
const failureStatuses: UploadQueueTaskStatus[] = ['FAILED', 'UNCONFIRMED'];
const refreshedTaskIds = new Set<string>();

const isTransporting = (task: UploadQueueTask<UploadResponse>) =>
  transportStatuses.includes(task.status);

const isFailed = (task: UploadQueueTask<UploadResponse>) => failureStatuses.includes(task.status);

export function useUploadTask(options: {
  currentIssueCode: string;
  loadBundles: (issueCode: string) => Promise<void>;
  loadIssues: () => Promise<void>;
}) {
  const { currentIssueCode, loadBundles, loadIssues } = options;
  const [validationError, setValidationError] = useState<string | null>(null);
  const uploadingRef = useRef(false);
  const allTasks = useSyncExternalStore(
    uploadQueue.subscribe,
    uploadQueue.getSnapshot,
    uploadQueue.getSnapshot
  );
  const uploadTasks = useMemo(
    () => allTasks.filter((task) => task.issueCode === currentIssueCode),
    [allTasks, currentIssueCode]
  );

  const uploading = uploadTasks.some(isTransporting);
  const uploadFailed = uploadTasks.some(isFailed);
  const uploadError = uploadTasks.find(isFailed)?.message ?? validationError;
  const uploadDisabled = !currentIssueCode;
  uploadingRef.current = uploading;

  useEffect(() => {
    const acceptedTasks = uploadTasks.filter(
      (task) => task.status === 'ACCEPTED' && !refreshedTaskIds.has(task.id)
    );
    acceptedTasks.forEach((task) => {
      refreshedTaskIds.add(task.id);
      void Promise.allSettled([loadBundles(task.issueCode), loadIssues()]);
    });
  }, [loadBundles, loadIssues, uploadTasks]);

  const performUpload = useCallback(
    async (files: File[]) => {
      setValidationError(null);
      if (!currentIssueCode) {
        setValidationError('请先选择或创建 Issue');
        return;
      }
      if (files.length === 0) {
        setValidationError('请至少选择一个文件');
        return;
      }
      uploadQueue.enqueue(currentIssueCode, files);
    },
    [currentIssueCode]
  );

  const resetSelection = useCallback(() => {
    setValidationError(null);
  }, []);

  const uploadSelection: UploadSelectionItem[] = uploadTasks
    .filter((task) => task.status !== 'ACCEPTED')
    .map((task) => ({
    name: task.name,
    sizeBytes: task.sizeBytes
    }));
  const activeTask = uploadTasks.find((task) => task.status === 'UPLOADING');

  return {
    performUpload,
    resetSelection,
    retryUpload: uploadQueue.retry,
    uploadDisabled,
    uploadError: uploadError ? normalizeApiError(uploadError) : null,
    uploadFailed,
    uploadProgress: activeTask?.progressPercent ?? 0,
    uploadSelection,
    tasks: uploadTasks,
    uploadTasks,
    uploading,
    uploadingRef
  };
}
