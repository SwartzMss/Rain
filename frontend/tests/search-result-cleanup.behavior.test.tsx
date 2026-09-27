import { StrictMode, type ReactNode } from 'react';
import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError, rainApi } from '../src/api/client';
import { useViewerTabs } from '../src/features/files/hooks/useViewerTabs';
import type { SearchViewerTab } from '../src/features/files/viewerTabs';

vi.mock('../src/api/client', async (original) => ({
  ...await original<typeof import('../src/api/client')>(),
  rainApi: { deleteTempResult: vi.fn() }
}));

const tab = (id: string, resultId = id): SearchViewerTab => ({
  id, resultId, kind: 'search', title: id, pinned: false, scrollTop: 0,
  expression: 'ERROR', hits: [], total: 0, from: 0, pageSize: 100,
  pageHistory: [], source: { kind: 'issue', issueCode: 'TEST' }
});

describe('search tab result cleanup', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(rainApi.deleteTempResult).mockResolvedValue(undefined);
  });

  it('deletes only after the last tab referencing a result closes, including StrictMode', () => {
    const { result } = renderHook(() => useViewerTabs(true), {
      wrapper: ({ children }: { children: ReactNode }) => <StrictMode>{children}</StrictMode>
    });
    act(() => result.current.setViewerTabsState([tab('a', 'shared'), tab('b', 'shared')], 'a'));
    act(() => result.current.closeViewerTab('a'));
    expect(rainApi.deleteTempResult).not.toHaveBeenCalled();
    act(() => result.current.closeViewerTab('b'));
    expect(rainApi.deleteTempResult).toHaveBeenCalledTimes(1);
    expect(rainApi.deleteTempResult).toHaveBeenCalledWith('shared');
  });

  it('cleans batch closures while preserving the remaining pinned result', () => {
    const { result } = renderHook(() => useViewerTabs(true));
    const pinned = { ...tab('keep'), pinned: true };
    act(() => result.current.setViewerTabsState([tab('a'), tab('b'), pinned], 'a'));
    act(() => result.current.setViewerTabsState([pinned], 'keep'));
    expect(vi.mocked(rainApi.deleteTempResult).mock.calls).toEqual([['a'], ['b']]);
    expect(result.current.viewerTabs).toEqual([pinned]);
  });

  it('does not delete a parent while a materialized result still has another tab for it', () => {
    const { result } = renderHook(() => useViewerTabs(true));
    act(() => result.current.setViewerTabsState([
      tab('a', 'shared'),
      { id: 'temp', kind: 'temp', resultId: 'shared', expression: '', lines: [], total: 0,
        from: 0, pageSize: 100, pageHistory: [], title: 'temp', pinned: false, scrollTop: 0 }
    ], 'a'));
    act(() => result.current.closeViewerTab('a'));
    expect(rainApi.deleteTempResult).not.toHaveBeenCalled();
    act(() => result.current.closeViewerTab('temp'));
    expect(rainApi.deleteTempResult).toHaveBeenCalledWith('shared');
  });

  it('closes even when deletion fails and leaves expiry cleanup as fallback', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    vi.mocked(rainApi.deleteTempResult).mockRejectedValueOnce(new Error('offline'));
    const { result } = renderHook(() => useViewerTabs(true));
    act(() => result.current.openViewerTab(tab('a')));
    act(() => result.current.closeViewerTab('a'));
    expect(result.current.viewerTabs).toEqual([]);
    await waitFor(() => expect(warn).toHaveBeenCalledOnce());
    warn.mockRestore();
  });

  it('accepts already expired results and never attempts guest deletion', async () => {
    vi.mocked(rainApi.deleteTempResult).mockRejectedValueOnce(new ApiError('expired', 404));
    const { result, rerender } = renderHook(({ allowed }) => useViewerTabs(allowed), { initialProps: { allowed: true } });
    act(() => result.current.openViewerTab(tab('expired')));
    act(() => result.current.closeViewerTab('expired'));
    await act(async () => {});
    rerender({ allowed: false });
    act(() => result.current.openViewerTab(tab('guest')));
    act(() => result.current.closeViewerTab('guest'));
    expect(rainApi.deleteTempResult).toHaveBeenCalledTimes(1);
  });
});
