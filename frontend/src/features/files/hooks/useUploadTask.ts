import { useCallback, useReducer, useRef } from 'react';
import { normalizeApiError, rainApi } from '../../../api/client';
import type { UploadSelectionItem } from '../uploadRows';

type UploadState =
  | { status: 'idle'; selection: UploadSelectionItem[]; message: string | null; progress: number }
  | { status: 'uploading'; selection: UploadSelectionItem[]; message: string | null; progress: number }
  | {
      status: 'failed';
      selection: UploadSelectionItem[];
      message: string;
      progress: number;
    };

type UploadAction =
  | { type: 'reset-selection' }
  | { type: 'error'; message: string }
  | { type: 'upload-started'; selection: UploadSelectionItem[] }
  | { type: 'upload-progress'; progress: number }
  | { type: 'upload-failed'; message: string }
  | { type: 'upload-finished' };

const initialUploadState: UploadState = {
  status: 'idle',
  selection: [],
  message: null,
  progress: 0
};

function uploadReducer(state: UploadState, action: UploadAction): UploadState {
  switch (action.type) {
    case 'reset-selection':
      return state.status === 'uploading'
        ? { ...state, selection: [], message: null, progress: 0 }
        : { status: 'idle', selection: [], message: null, progress: 0 };
    case 'error':
      return { ...state, message: action.message };
    case 'upload-started':
      return { status: 'uploading', selection: action.selection, message: null, progress: 0 };
    case 'upload-progress':
      return { ...state, progress: action.progress };
    case 'upload-failed':
      return { status: 'failed', selection: state.selection, message: action.message, progress: 0 };
    case 'upload-finished':
      return state.status === 'uploading' ? { ...state, status: 'idle', progress: 0 } : { ...state, progress: 0 };
  }
}

export function useUploadTask(options: {
  currentIssueCode: string;
  loadBundles: (issueCode: string) => Promise<void>;
  loadIssues: () => Promise<void>;
}) {
  const { currentIssueCode, loadBundles, loadIssues } = options;
  const [state, dispatch] = useReducer(uploadReducer, initialUploadState);
  const uploadingRef = useRef(false);

  const uploading = state.status === 'uploading';
  const uploadFailed = state.status === 'failed';
  const uploadDisabled = !currentIssueCode || uploading;

  const resetSelection = useCallback(() => {
    dispatch({ type: 'reset-selection' });
  }, []);

  const performUpload = useCallback(
    async (files: File[]) => {
      if (uploadingRef.current) return;
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
        await rainApi.uploadLogs(currentIssueCode, files, (progress) => {
          dispatch({ type: 'upload-progress', progress });
        });
      } catch (error) {
        dispatch({ type: 'upload-failed', message: normalizeApiError(error) });
        uploadingRef.current = false;
        dispatch({ type: 'upload-finished' });
        return;
      }

      uploadingRef.current = false;
      dispatch({ type: 'upload-finished' });
      await Promise.allSettled([loadBundles(currentIssueCode), loadIssues()]);
    },
    [currentIssueCode, loadBundles, loadIssues]
  );

  return {
    performUpload,
    resetSelection,
    uploadDisabled,
    uploadError: state.message,
    uploadFailed,
    uploadProgress: state.progress,
    uploadSelection: state.selection,
    uploading,
    uploadingRef
  };
}
