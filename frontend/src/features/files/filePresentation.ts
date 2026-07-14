import type { FileNode } from '../../api/types';

const supportedArchivePattern = /\.(zip|tar\.gz|tgz|gz)$/i;

type FileCapabilities = Pick<FileNode, 'is_dir' | 'name' | 'preview_kind'>;

export const isArchiveNode = (node?: Partial<FileCapabilities> | null) => {
  if (!node) return false;
  if (node.preview_kind) return node.preview_kind === 'archive';
  return Boolean(node.name && supportedArchivePattern.test(node.name));
};

export const isBinaryNode = (node?: Partial<FileCapabilities> | null) =>
  Boolean(node && !node.is_dir && node.preview_kind === 'binary');

export const canPreviewText = (node?: Partial<FileCapabilities> | null) => {
  if (!node || node.is_dir) return false;
  if (node.preview_kind) return node.preview_kind === 'text';
  return !isArchiveNode(node);
};
