import type { FileNode, UploadStage, UploadStatus, UploadSummary } from '../../api/types';
import { uploadFailureMessage } from './uploadFailure';
import { createOptimisticUploadRows, type UploadSelectionItem } from './uploadRows';

export type BundleFileState = {
  files: FileNode[];
  loading: boolean;
  loaded: boolean;
  error: string | null;
};

export type FileRow = {
  key: string;
  bundleHash: string;
  bundleName: string;
  file?: FileNode;
  name: string;
  status: UploadStatus;
  stage: UploadStage | 'UPLOADING';
  progressPercent?: number;
  sizeBytes?: number;
  failureReason?: string | null;
};

export const formatBytes = (bytes?: number | null) => {
  if (!bytes) return '-';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const exponent = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** exponent;
  return `${value.toFixed(value >= 10 || exponent === 0 ? 0 : 1)} ${units[exponent]}`;
};

export const stageLabel = (stage: FileRow['stage'], progressPercent?: number) => {
  if (stage === 'UPLOADING') return `上传中 ${progressPercent ?? 0}%`;
  if (stage === 'PENDING') return '等待处理';
  if (stage === 'RECEIVING') return '接收文件';
  if (stage === 'EXTRACTING') return '解压中';
  if (stage === 'INDEXING') return '建立索引';
  if (stage === 'PUBLISHING') return '发布中';
  if (stage === 'READY') return '已完成';
  return '失败';
};

export const stageClass = (stage: FileRow['stage']) => {
  if (stage === 'READY') return 'border-emerald-500/40 bg-emerald-500/10 text-emerald-700';
  if (stage === 'FAILED') return 'border-rose-500/40 bg-rose-500/10 text-rose-600';
  return 'border-sky-500/40 bg-sky-500/10 text-sky-700';
};

const getFileLabel = (file: FileNode) =>
  typeof file.meta?.original_name === 'string' ? (file.meta.original_name as string) : file.name;

export function buildFileRows(options: {
  bundles: UploadSummary[];
  bundleFiles: Record<string, BundleFileState>;
  uploadSelection: UploadSelectionItem[];
  uploading: boolean;
  uploadFailed: boolean;
  uploadProgress: number;
}): FileRow[] {
  const {
    bundleFiles,
    bundles,
    uploadFailed,
    uploadProgress,
    uploadSelection,
    uploading
  } = options;
  const rows = bundles.flatMap<FileRow>((bundle) => {
    const status = bundle.status.upload_status;
    const stage = bundle.stage;
    const state = bundleFiles[bundle.hash];
    if (status !== 'READY') {
      return [
        {
          key: bundle.hash,
          bundleHash: bundle.hash,
          bundleName: bundle.name || bundle.hash,
          name: bundle.name || bundle.hash,
          status,
          stage,
          sizeBytes: bundle.size_bytes ?? undefined,
          failureReason: uploadFailureMessage({
            status,
            failure_reason: bundle.failure_reason
          })
        }
      ];
    }

    return (state?.files ?? []).map((file) => ({
      key: `${bundle.hash}:${file.id}`,
      bundleHash: bundle.hash,
      bundleName: bundle.name || bundle.hash,
      file,
      name: getFileLabel(file),
      status,
      stage: 'READY',
      sizeBytes: file.size_bytes
    }));
  });
  if ((uploading || uploadFailed) && uploadSelection.length > 0) {
    rows.unshift(
      ...createOptimisticUploadRows(
        uploadSelection,
        uploadProgress,
        '',
        uploadFailed ? 'FAILED' : 'UPLOADING'
      )
    );
  }
  return rows;
}
