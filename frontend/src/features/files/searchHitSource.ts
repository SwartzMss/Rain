import type { IssueLogSearchHit } from '../../api/types';

export type SearchHitSource = {
  bundleHash: string;
  fileId: string;
  nodeId: string;
  path: string;
  line: number | null;
};

export function getSearchHitSource(hit: IssueLogSearchHit): SearchHitSource | null {
  const bundleHash = hit.bundle_hash?.trim();
  const fileId = String(hit.file_id ?? '').trim();
  if (!bundleHash || !fileId) return null;

  return {
    bundleHash,
    fileId,
    nodeId: `${bundleHash}:${fileId}`,
    path: hit.path,
    line: typeof hit.line_number === 'number' && hit.line_number >= 0
      ? hit.line_number
      : null
  };
}

export function placeContextMenu(
  point: { x: number; y: number },
  menu: { width: number; height: number },
  viewport: { width: number; height: number },
  margin = 8
) {
  return {
    left: Math.max(margin, Math.min(point.x, viewport.width - menu.width - margin)),
    top: Math.max(margin, Math.min(point.y, viewport.height - menu.height - margin))
  };
}
