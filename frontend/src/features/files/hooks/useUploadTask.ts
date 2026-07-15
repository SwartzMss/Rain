import { useCallback, useEffect, useReducer, useRef } from 'react';
import { normalizeApiError, rainApi } from '../../../api/client';
import type { UploadTaskResponse } from '../../../api/types';
import { uploadFailureMessage } from '../uploadFailure';
import type { UploadSelectionItem } from '../uploadRows';

type UploadState =
  | { status: 'idle'; selection: UploadSelectionItem[]; task: null; message: string | null; progress: number }
  | { status: 'uploading'; selection: UploadSelectionItem[]; task: null; message: string | null; progress: number }
  | {
      status: 'processing';
      selection: UploadSelectionItem[];
      task: UploadTaskResponse;
      message: string | null;
      progress: number;
    }
  | {
      status: 'failed';
      selection: UploadSelectionItem[];
      task: UploadTaskResponse | null;
      message: string;
      progress: number;
    }
  | {
      status: 'completed';
      selection: UploadSelectionItem[];
      task: UploadTaskResponse;
      message: string | null;
      progress: number;
    };

type UploadAction =
  | { type: 'reset-selection' }
  | { type: 'error'; message: string }
  | { type: 'upload-started'; selection: UploadSelectionItem[] }
  | { type: 'upload-progress'; progress: number }
  | { type: 'task-started'; task: UploadTaskResponse }
  | { type: 'task-polled'; task: UploadTaskResponse }
  | { type: 'upload-failed'; message: string }
  | { type: 'upload-finished' };

const initialUploadState: UploadState = {
  status: 'idle',
  selection: [],
  task: null,
  message: null,
  progress: 0
};

function uploadReducer(state: UploadState, action: UploadAction): UploadState {
  switch (action.type) {
    case 'reset-selection':
      return { status: 'idle', selection: [], task: null, message: null, progress: 0 };
    case 'error':
      return { ...state, message: action.message };
    case 'upload-started':
      return { status: 'uploading', selection: action.selection, task: null, message: null, progress: 0 };
    case 'upload-progress':
      return { ...state, progress: action.progress };
    case 'task-started':
      return {
        status: action.task.status === 'READY' ? 'completed' : 'processing',
        selection: state.selection,
        task: action.task,
        message: null,
        progress: 0
      };
    case 'task-polled': {
      if (action.task.status === 'FAILED') {
        return {
          status: 'failed',
          selection: state.selection,
          task: action.task,
          message: uploadFailureMessage(action.task) ?? '上传处理失败',
          progress: state.progress
        };
      }
      if (action.task.status === 'READY') {
        return {
          status: 'completed',
          selection: state.selection,
          task: action.task,
          message: null,
          progress: 100
        };
      }
      return {
        status: 'processing',
        selection: state.selection,
        task: action.task,
        message: null,
        progress: state.progress
      };
    }
    case 'upload-failed':
      return { status: 'failed', selection: state.selection, task: state.task, message: action.message, progress: 0 };
    case 'upload-finished':
      return state.status === 'uploading' ? { ...state, status: 'idle', progress: 0 } : { ...state, progress: 0 };
  }
}

export function useUploadTask(options: {
  currentIssueCode: string;
  hasActiveBundleProcessing: boolean;
  loadBundles: (issueCode: string) => Promise<void>;
  loadIssues: () => Promise<void>;
  getSelectedIssueCode: () => string;
}) {
  const { currentIssueCode, getSelectedIssueCode, hasActiveBundleProcessing, loadBundles, loadIssues } = options;
  const [state, dispatch] = useReducer(uploadReducer, initialUploadState);
  const uploadingRef = useRef(false);

  const uploadTask = state.task;
  const activeTask =
    uploadTask?.status === 'PROCESSING' || uploadTask?.status === 'PENDING' ? uploadTask : null;
  const uploading = state.status === 'uploading';
  const uploadFailed = state.status === 'failed';
  const uploadDisabled = !currentIssueCode || uploading || !!activeTask;

  const resetSelection = useCallback(() => {
    dispatch({ type: 'reset-selection' });
  }, []);

  const performUpload = useCallback(
    async (files: File[]) => {
      if (uploadingRef.current) return;
      if (activeTask) {
        dispatch({ type: 'error', message: '当前上传任务仍在后台解析，请等待完成后再上传' });
        return;
      }
      if (!currentIssueCode) {
        dispatch({ type: 'error', message: '请先选择或创建 Issue' });
        return;
      }
      if (files.length === 0) {
        dispatch({ type: 'error', message: '请至少选择一个文件' });
        return;
      }

      uploadingRef.current = true;
      dispatch({
        type: 'upload-started',
        selection: files.map((file) => ({ name: file.name, sizeBytes: file.size }))
      });
      try {
        const response = await rainApi.uploadLogs(currentIssueCode, files, (progress) => {
          dispatch({ type: 'upload-progress', progress });
        });
        dispatch({
          type: 'task-started',
          task: {
            task_id: response.task_id,
            issue_code: response.issue_code,
            bundle_hash: response.bundle_hash,
            status: response.status,
            stage: response.stage,
            progress_percent: response.status === 'READY' ? 100 : 0,
            total_bytes: response.total_bytes
          }
        });
        await loadBundles(currentIssueCode);
        await loadIssues();
      } catch (error) {
        dispatch({ type: 'upload-failed', message: normalizeApiError(error) });
      } finally {
        uploadingRef.current = false;
        dispatch({ type: 'upload-finished' });
      }
    },
    [activeTask, currentIssueCode, loadBundles, loadIssues]
  );

  useEffect(() => {
    if (!currentIssueCode) return;
    if (uploadTask?.task_id) return;
    if (!hasActiveBundleProcessing) return;
    const timer = window.setTimeout(() => {
      loadBundles(currentIssueCode).catch(() => undefined);
    }, 3000);
    return () => window.clearTimeout(timer);
  }, [currentIssueCode, hasActiveBundleProcessing, loadBundles, uploadTask?.task_id]);

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
        dispatch({ type: 'task-polled', task });
        if (task.status === 'READY' || task.status === 'FAILED') {
          if (getSelectedIssueCode() === task.issue_code) {
            await loadBundles(task.issue_code);
          }
          await loadIssues();
          return;
        }
        timer = window.setTimeout(poll, 3000);
      } catch (error) {
        if (!cancelled) {
          dispatch({ type: 'error', message: normalizeApiError(error) });
          timer = window.setTimeout(poll, 5000);
        }
      }
    };
    poll().catch(() => undefined);
    return () => {
      cancelled = true;
      if (timer) window.clearTimeout(timer);
    };
  }, [getSelectedIssueCode, loadBundles, loadIssues, uploadTask?.status, uploadTask?.task_id]);

  return {
    activeTask,
    performUpload,
    resetSelection,
    uploadDisabled,
    uploadError: state.message,
    uploadFailed,
    uploadProgress: state.progress,
    uploadSelection: state.selection,
    uploadTask,
    uploading,
    uploadingRef
  };
}
