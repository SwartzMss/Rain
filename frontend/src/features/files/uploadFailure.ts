import type { UploadStatus } from '../../api/types';

type FailureState = {
  status: UploadStatus;
  failure_reason?: string | null;
};

export const uploadFailureMessage = (state: FailureState): string | null => {
  if (state.status !== 'FAILED') return null;
  const reason = state.failure_reason?.trim();
  return reason || '上传处理失败，请删除后重试';
};
