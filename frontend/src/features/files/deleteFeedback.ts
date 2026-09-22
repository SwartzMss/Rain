import { ApiError, normalizeApiError } from '../../api/client';

export function normalizeDeletionError(error: unknown) {
  return error instanceof ApiError && error.status === 409
    ? '当前有上传或处理中的 Bundle，暂不可删除，请等待处理完成后重试'
    : normalizeApiError(error);
}
