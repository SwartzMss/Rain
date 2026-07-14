export type UploadSelectionItem = {
  name: string;
  sizeBytes: number;
};

export const createOptimisticUploadRows = (
  items: UploadSelectionItem[],
  progressPercent: number,
  bundleHash = '',
  stage: 'UPLOADING' | 'FAILED' = 'UPLOADING'
) =>
  items.map((item, index) => ({
    key: `active-upload:${index}:${item.name}`,
    bundleHash,
    bundleName: item.name,
    name: item.name,
    status: stage === 'FAILED' ? ('FAILED' as const) : ('PROCESSING' as const),
    stage,
    progressPercent: stage === 'UPLOADING' ? progressPercent : undefined,
    sizeBytes: item.sizeBytes
  }));
