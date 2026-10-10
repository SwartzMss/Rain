import { StrictMode, type ReactNode } from 'react';
import { act, renderHook } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { rainApi } from '../src/api/client';
import { useViewerTabs } from '../src/features/files/hooks/useViewerTabs';
import type { SearchViewerTab } from '../src/features/files/viewerTabs';

vi.mock('../src/api/client', () => ({ rainApi: { deleteTempResult: vi.fn().mockResolvedValue(undefined) } }));
const tab = (id: string): SearchViewerTab => ({
  id, resultId: id, kind: 'search', title: id, pinned: false, scrollTop: 0,
  expression: 'ERROR', hits: [], total: 0, from: 0, pageSize: 100,
  pageHistory: [], source: { kind: 'issue', issueCode: 'TEST' }
});

describe('search tab closure', () => {
  it('keeps backend results for other viewers when tabs close, reset or unmount', () => {
    const { result, unmount } = renderHook(() => useViewerTabs(), {
      wrapper: ({ children }: { children: ReactNode }) => <StrictMode>{children}</StrictMode>
    });
    act(() => result.current.setViewerTabsState([tab('a'), tab('b')], 'a'));
    act(() => result.current.closeViewerTab('a'));
    expect(result.current.viewerTabs.map((item) => item.id)).toEqual(['b']);
    act(() => result.current.resetViewerTabs());
    expect(result.current.viewerTabs).toEqual([]);
    unmount();
    expect(rainApi.deleteTempResult).not.toHaveBeenCalled();
  });
});
