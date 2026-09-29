import type { FileNode, FileNodeResponse } from '../../api/types';
import { normalizeApiError } from '../../api/client';
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

export const mergeFlattenedExtractionChildren = (
  parent: TreeNode,
  extraction: TreeNode
): TreeNode => {
  if (
    parent.childrenSourceId !== extraction.rawId ||
    !isExtractionFolder(extraction, parent)
  ) {
    return parent;
  }

  return {
    ...parent,
    childrenIds: [...new Set([...parent.childrenIds, ...extraction.childrenIds])],
    hasMoreChildren: extraction.hasMoreChildren,
    childrenCursor: extraction.childrenCursor
  };
};

const compareTreeNodes = (left: TreeNode, right: TreeNode) => {
  if (left.is_dir !== right.is_dir) return left.is_dir ? -1 : 1;
  const leftName = new TextEncoder().encode(left.name);
  const rightName = new TextEncoder().encode(right.name);
  const nameLength = Math.min(leftName.length, rightName.length);
  let nameOrder = 0;
  for (let index = 0; index < nameLength; index += 1) {
    if (leftName[index] === rightName[index]) continue;
    nameOrder = leftName[index] < rightName[index] ? -1 : 1;
    break;
  }
  if (nameOrder === 0 && leftName.length !== rightName.length) {
    nameOrder = leftName.length < rightName.length ? -1 : 1;
  }
  if (nameOrder !== 0) return nameOrder;

  const leftRawId = Number(left.rawId);
  const rightRawId = Number(right.rawId);
  if (Number.isSafeInteger(leftRawId) && Number.isSafeInteger(rightRawId)) {
    return leftRawId - rightRawId;
  }
  return left.rawId < right.rawId ? -1 : left.rawId > right.rawId ? 1 : 0;
};

export const attachTreeChild = (
  parent: TreeNode,
  child: TreeNode,
  knownChildren: TreeNode[]
): { parent: TreeNode; child: TreeNode } => {
  const attachedChild = parent.rawId === 'root' && child.parentId !== parent.id
    ? { ...child, parentId: parent.id }
    : child;
  const childrenById = new Map(knownChildren.map((knownChild) => [knownChild.id, knownChild]));
  childrenById.set(attachedChild.id, attachedChild);
  const existingOrder = new Map(parent.childrenIds.map((childId, index) => [childId, index]));
  const nextChildrenIds = [...new Set([...parent.childrenIds, attachedChild.id])];

  nextChildrenIds.sort((leftId, rightId) => {
    const left = childrenById.get(leftId);
    const right = childrenById.get(rightId);
    if (!left || !right) {
      return (existingOrder.get(leftId) ?? Number.MAX_SAFE_INTEGER)
        - (existingOrder.get(rightId) ?? Number.MAX_SAFE_INTEGER);
    }
    return compareTreeNodes(left, right);
  });

  return {
    parent: { ...parent, childrenIds: nextChildrenIds },
    child: attachedChild
  };
};

export const mergeLoadedTreeNode = (
  existing: TreeNode | undefined,
  incoming: TreeNode
): TreeNode => {
  if (
    !existing
    || !existing.hasLoadedChildren
    || incoming.hasLoadedChildren
    || incoming.childrenLoadError
  ) {
    return incoming;
  }

  return {
    ...incoming,
    childrenIds: existing.childrenIds,
    hasLoadedChildren: existing.hasLoadedChildren,
    hasMoreChildren: existing.hasMoreChildren,
    childrenCursor: existing.childrenCursor,
    childrenSourceId: existing.childrenSourceId,
    childrenLoadError: existing.childrenLoadError
  };
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
      base.childrenLoadError = normalizeApiError(error);
    }
  }

  base.childrenIds = childrenNodes.map((child) => child.id);
  if (!base.childrenLoadError && base.childrenSourceId === nodeId) {
    base.hasMoreChildren = response.has_more === true;
    base.childrenCursor = response.next_cursor ?? null;
  }

  return { node: base, children: childrenNodes };
}
