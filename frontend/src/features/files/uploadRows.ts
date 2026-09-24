import type { UploadResponse } from '../../api/types';
import type { UploadQueueTask } from './uploadQueue';

export type UploadSelectionItem = {
  name: string;
  sizeBytes: number;
};

export type LocalUploadStage =
  | 'QUEUED'
  | 'UPLOADING'
  | 'RETRY_WAIT'
  | 'ACCEPTED'
  | 'FAILED'
  | 'UNCONFIRMED';

export type UploadTaskSnapshot = UploadQueueTask<UploadResponse>;

export const createOptimisticUploadRows = (
  tasks: readonly UploadTaskSnapshot[],
  existingBundleHashes: ReadonlySet<string>
) =>
  tasks
    .filter((task) => !task.response || !existingBundleHashes.has(task.response.bundle_hash))
    .map((task) => ({
      key: task.id,
      bundleHash: task.response?.bundle_hash ?? '',
      bundleName: task.name,
      name: task.name,
      status: task.status === 'FAILED' || task.status === 'UNCONFIRMED'
        ? ('FAILED' as const)
        : (task.response?.status ?? 'PENDING'),
      stage: task.status as LocalUploadStage,
      progressPercent: task.progressPercent,
      sizeBytes: task.sizeBytes,
      failureReason: task.message
    }));
