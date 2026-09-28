import type { FileNode, FileNodeResponse } from '../../api/types';
import { isArchiveNode } from './filePresentation';

export type TreeNode = Omit<FileNode, 'id' | 'children'> & {
  id: string;
  rawId: string;
  bundleId: string;
  parentId: string | null;
  childrenIds: string[];
  hasLoadedChildren: boolean;
  hasMoreChildren: boolean;
  childrenCursor: string | null;
  childrenSourceId: string;
  childrenLoadError: string | null;
};

export type TreeNodeFetcher = (
  nodeId: string,
  options?: { cursor?: string | null; limit?: number }
) => Promise<FileNodeResponse>;

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
  // The first extracted directory is an internal container named from the
  // uploaded archive's storage hash. It is flattened in the file tree and
  // should not leak into user-facing search paths either.
  const visible = rest[0]?.toLowerCase().endsWith('_extracted') ? rest.slice(1) : rest;
  const normalized = visible.map((segment) => segment.replace(/_extracted$/i, ''));
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
  parentId: parentId ?? (node.parent_id == null ? null : `${bundleId}:${node.parent_id}`),
  name: node.name,
  path: node.path,
  is_dir: node.is_dir,
  preview_kind: node.preview_kind,
  size_bytes: node.size_bytes,
  mime_type: node.mime_type,
  status: node.status,
  meta: node.meta,
  childrenIds: [],
  hasLoadedChildren: false,
  hasMoreChildren: false,
  childrenCursor: null,
  childrenSourceId: node.id.toString(),
  childrenLoadError: null
});

export async function hydrateTreeNode(
  bundleId: string,
  nodeId: string,
  parentId: string | null,
  fetchNode: TreeNodeFetcher,
  limit = 100
): Promise<{ node: TreeNode; children: TreeNode[] }> {
  const response = await fetchNode(nodeId, { limit });
  const base = toTreeNode(bundleId, response.node, parentId);
  const childrenNodes: TreeNode[] = [];
  base.hasLoadedChildren = true;
  base.childrenSourceId = nodeId;

  for (const child of response.children ?? []) {
    const childNode = toTreeNode(bundleId, child, base.id);
    if (!isExtractionFolder(childNode, base)) {
      childrenNodes.push(childNode);
      continue;
    }

    try {
      const extracted = await fetchNode(child.id.toString(), { limit });
      (extracted.children ?? []).forEach((grand) => {
        childrenNodes.push(toTreeNode(bundleId, grand, base.id));
      });
      if (extracted.has_more && extracted.next_cursor) {
        base.hasMoreChildren = true;
        base.childrenCursor = extracted.next_cursor;
        base.childrenSourceId = child.id.toString();
      }
    } catch (error) {
      base.hasLoadedChildren = false;
      base.childrenLoadError = error instanceof Error ? error.message : '解压目录加载失败';
    }
  }

  base.childrenIds = childrenNodes.map((child) => child.id);
  if (!base.childrenLoadError && base.childrenSourceId === nodeId) {
    base.hasMoreChildren = response.has_more === true;
    base.childrenCursor = response.next_cursor ?? null;
  }

  return { node: base, children: childrenNodes };
}
