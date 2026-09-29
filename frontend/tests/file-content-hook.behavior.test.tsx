import { act, renderHook, waitFor } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { FileLinesResponse } from '../src/api/types';
import { FileContentCache, type FileContentRequestKey } from '../src/features/files/fileContentCache';
import { useFileContent } from '../src/features/files/hooks/useFileContent';

const request: FileContentRequestKey = {
  bundle: 'bundle', file: 'file', context: 'issue', open: 'tab', start: 0, requestedLimit: 5000
};

const response = (content: string): FileLinesResponse => ({
  path: 'app.log', start: 0, limit: 5000,
  lines: [{ line_number: 0, content }]
});

describe('useFileContent', () => {
  it('renders a cached page as ready on the first render after a tab switch', async () => {
    const cache = new FileContentCache<FileLinesResponse>(async () => response('cached'));
    await cache.request('tab', request);

    const { result } = renderHook(() => useFileContent({ cache, owner: 'tab', request }));

    expect(result.current.fileLines?.lines[0]?.content).toBe('cached');
    expect(result.current.fileContentLoading).toBe(false);
    expect(result.current.fileContentError).toBeNull();
  });

  it('updates from loading to ready when the current request completes', async () => {
    let resolve!: (value: FileLinesResponse) => void;
    const cache = new FileContentCache<FileLinesResponse>(() => new Promise((next) => {
      resolve = next;
    }));
    const { result } = renderHook(() => useFileContent({ cache, owner: 'tab', request }));

    expect(result.current.fileContentLoading).toBe(true);
    act(() => resolve(response('loaded')));
    await waitFor(() => expect(result.current.fileLines?.lines[0]?.content).toBe('loaded'));
    expect(result.current.fileContentLoading).toBe(false);
  });
});
