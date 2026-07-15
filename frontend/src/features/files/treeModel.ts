import type { FileNode } from '../../api/types';
import { isArchiveNode } from './filePresentation';

export type TreeNode = Omit<FileNode, 'id' | 'children'> & {
  id: string;
  rawId: string;
  bundleId: string;
  parentId: string | null;
  childrenIds: string[];
  hasLoadedChildren: boolean;
};

export const formatSize = (bytes?: number) => {
  if (bytes === undefined || bytes === null) return '--';
  const units = ['B', 'KB', 'MB', 'GB'];
  let size = bytes;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  const fixed = unit === 0 ? size.toFixed(0) : size.toFixed(1);
  return `${fixed} ${units[unit]}`;
};

export const isExtractionFolder = (node: TreeNode, parent?: TreeNode | null) => {
  if (!node.is_dir || !node.name.toLowerCase().endsWith('_extracted')) return false;
  return parent ? isArchiveNode(parent) : false;
};

export const formatHitPath = (raw: string) => {
  const parts = raw.replace(/^\//, '').split('/');
  if (parts.length === 0) return raw;
  const [, ...rest] = parts;
  if (rest.length === 0) return raw.replace(/^\//, '');
  const normalized = rest.map((segment, index) => {
    if (index === 0 && segment.endsWith('_extracted')) {
      return segment.replace(/_extracted$/, '');
    }
    return segment;
  });
  return normalized.join('/');
};

export const toTreeNode = (
  bundleId: string,
  node: FileNode,
  parentId: string | null = null
): TreeNode => ({
  id: `${bundleId}:${node.id.toString()}`,
  rawId: node.id.toString(),
  bundleId,
  parentId,
  name: node.name,
  path: node.path,
  is_dir: node.is_dir,
  preview_kind: node.preview_kind,
  size_bytes: node.size_bytes,
  mime_type: node.mime_type,
  status: node.status,
  meta: node.meta,
  childrenIds: [],
  hasLoadedChildren: false
});
