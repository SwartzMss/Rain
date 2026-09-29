import { ApiError, normalizeApiError } from '../../api/client';
import { API_ERROR_CODES } from '../../api/errorCodes';

export function normalizeDeletionError(error: unknown) {
  if (error instanceof ApiError) {
    switch (error.code) {
      case API_ERROR_CODES.fileDeleteBundleBusy:
        return '当前文件仍在处理中，暂不可删除，请等待处理完成后重试';
      case API_ERROR_CODES.fileDeleteAlreadyRunning:
        return '当前 Bundle 已有删除任务，请稍后重试';
      default:
        return normalizeApiError(error);
    }
  }

  return normalizeApiError(error);
}
