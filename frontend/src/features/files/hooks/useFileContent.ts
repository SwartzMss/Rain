import { useEffect, useState } from 'react';
import { normalizeApiError, rainApi } from '../../../api/client';
import type { FileLinesResponse } from '../../../api/types';
import { canPreviewText } from '../filePresentation';
import type { TreeNode } from '../treeModel';

type UseFileContentOptions = {
  bundleId: string;
  selectedNode: TreeNode | null;
  defaultPageSize: number;
};

export function useFileContent({
  bundleId,
  selectedNode,
  defaultPageSize
}: UseFileContentOptions) {
  const [fileLines, setFileLines] = useState<FileLinesResponse | null>(null);
  const [lineStart, setLineStart] = useState(0);
  const [linePageSize, setLinePageSize] = useState(defaultPageSize);
  const [fileContentLoading, setFileContentLoading] = useState(false);
  const [fileContentError, setFileContentError] = useState<string | null>(null);
  const [targetLine, setTargetLine] = useState<number | null>(null);

  useEffect(() => {
    setFileLines(null);
    setFileContentError(null);
    setFileContentLoading(false);
    if (!selectedNode || !canPreviewText(selectedNode)) return;
    const bundleForContent = selectedNode.bundleId || bundleId;
    if (!bundleForContent) return;

    let ignore = false;
    const fetchContent = async () => {
      setFileContentLoading(true);
      try {
        const content = await rainApi.fetchFileLines(bundleForContent, selectedNode.rawId, {
          start: lineStart,
          limit: linePageSize
        });
        if (!ignore) {
          setFileLines(content);
        }
      } catch (error) {
        if (!ignore) {
          setFileContentError(normalizeApiError(error));
        }
      } finally {
        if (!ignore) {
          setFileContentLoading(false);
        }
      }
    };

    fetchContent();
    return () => {
      ignore = true;
    };
  }, [
    bundleId,
    selectedNode?.id,
    selectedNode?.is_dir,
    selectedNode?.preview_kind,
    lineStart,
    linePageSize
  ]);

  return {
    fileLines,
    lineStart,
    setLineStart,
    linePageSize,
    setLinePageSize,
    fileContentLoading,
    fileContentError,
    targetLine,
    setTargetLine
  };
}
