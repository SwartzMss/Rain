export const shouldResetUploadAfterBundleDeletion = (
  deletedBundleHash: string,
  uploadTaskBundleHash?: string
): boolean => deletedBundleHash === uploadTaskBundleHash;
