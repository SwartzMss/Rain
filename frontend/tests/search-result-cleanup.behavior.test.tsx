import { StrictMode, type ReactNode } from 'react';
import { act, renderHook } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { rainApi } from '../src/api/client';
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

  it('keeps result references with the workspace when tabs close, including StrictMode', () => {
    const { result } = renderHook(() => useViewerTabs(), {
      wrapper: ({ children }: { children: ReactNode }) => <StrictMode>{children}</StrictMode>
    });
    act(() => result.current.setViewerTabsState([tab('a', 'shared'), tab('b', 'shared')], 'a'));
    act(() => result.current.closeViewerTab('a'));
    expect(rainApi.deleteTempResult).not.toHaveBeenCalled();
    act(() => result.current.closeViewerTab('b'));
    expect(rainApi.deleteTempResult).not.toHaveBeenCalled();
  });

  it('does not release workspace results when a batch closes around a pinned tab', () => {
    const { result } = renderHook(() => useViewerTabs());
    const pinned = { ...tab('keep'), pinned: true };
    act(() => result.current.setViewerTabsState([tab('a'), tab('b'), pinned], 'a'));
    act(() => result.current.setViewerTabsState([pinned], 'keep'));
    expect(rainApi.deleteTempResult).not.toHaveBeenCalled();
    expect(result.current.viewerTabs).toEqual([pinned]);
  });

  it('keeps a result referenced by a parent and materialized tab for the workspace', () => {
    const { result } = renderHook(() => useViewerTabs());
    act(() => result.current.setViewerTabsState([
      tab('a', 'shared'),
      { id: 'temp', kind: 'temp', resultId: 'shared', expression: '', lines: [], total: 0,
        from: 0, pageSize: 100, pageHistory: [], title: 'temp', pinned: false, scrollTop: 0 }
    ], 'a'));
    act(() => result.current.closeViewerTab('a'));
    expect(rainApi.deleteTempResult).not.toHaveBeenCalled();
    act(() => result.current.closeViewerTab('temp'));
    expect(rainApi.deleteTempResult).not.toHaveBeenCalled();
  });

  it('does not send per-tab deletion requests when a result tab closes', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const { result } = renderHook(() => useViewerTabs());
    act(() => result.current.openViewerTab(tab('a')));
    act(() => result.current.closeViewerTab('a'));
    expect(result.current.viewerTabs).toEqual([]);
    expect(rainApi.deleteTempResult).not.toHaveBeenCalled();
    expect(warn).not.toHaveBeenCalled();
    warn.mockRestore();
  });

  it('keeps tab closure independent of result deletion', () => {
    const { result } = renderHook(() => useViewerTabs());
    act(() => result.current.openViewerTab(tab('expired')));
    act(() => result.current.closeViewerTab('expired'));
    act(() => result.current.openViewerTab(tab('guest')));
    act(() => result.current.closeViewerTab('guest'));
    expect(rainApi.deleteTempResult).not.toHaveBeenCalled();
  });
});
