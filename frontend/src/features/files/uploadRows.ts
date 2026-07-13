export type UploadSelectionItem = {
  name: string;
  sizeBytes: number;
};

export const createOptimisticUploadRows = (
  items: UploadSelectionItem[],
  progressPercent: number,
  bundleHash = ''
) =>
  items.map((item, index) => ({
    key: `active-upload:${index}:${item.name}`,
    bundleHash,
    bundleName: item.name,
    name: item.name,
    status: 'PROCESSING' as const,
    stage: 'UPLOADING' as const,
    progressPercent,
    sizeBytes: item.sizeBytes
  }));
