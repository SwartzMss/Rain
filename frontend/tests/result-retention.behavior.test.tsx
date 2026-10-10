import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { rainApi } from '../src/api/client';
import { useResultRetention } from '../src/features/files/hooks/useResultRetention';

vi.mock('../src/api/client', () => ({ rainApi: { keepAliveTempResults: vi.fn() } }));

describe('open result retention', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.mocked(rainApi.keepAliveTempResults).mockReset().mockResolvedValue({ unavailable_ids: [] });
  });
  afterEach(() => vi.useRealTimers());

  it('renews all unique open results beyond their original lifetime and stops on close', async () => {
    const { rerender, unmount } = renderHook(({ ids }) => useResultRetention(ids), {
      initialProps: { ids: ['a', 'b', 'a'] }
    });
    await act(async () => {});
    expect(rainApi.keepAliveTempResults).toHaveBeenLastCalledWith(['a', 'b'], expect.any(AbortSignal));
    await act(async () => vi.advanceTimersByTimeAsync(35 * 60_000));
    expect(rainApi.keepAliveTempResults).toHaveBeenCalledTimes(8);
    rerender({ ids: ['b'] });
    await act(async () => {});
    expect(rainApi.keepAliveTempResults).toHaveBeenLastCalledWith(['b'], expect.any(AbortSignal));
    unmount();
    const calls = vi.mocked(rainApi.keepAliveTempResults).mock.calls.length;
    await act(async () => vi.advanceTimersByTimeAsync(10 * 60_000));
    expect(rainApi.keepAliveTempResults).toHaveBeenCalledTimes(calls);
  });

  it('does not renew again just because an equivalent ID array rerenders', async () => {
    const { rerender } = renderHook(({ ids }) => useResultRetention(ids), { initialProps: { ids: ['b', 'a'] } });
    await act(async () => {});
    rerender({ ids: ['a', 'b', 'a'] });
    await act(async () => {});
    expect(rainApi.keepAliveTempResults).toHaveBeenCalledTimes(1);
  });

  it('chunks large tab sets and reports only confirmed unavailable results', async () => {
    vi.mocked(rainApi.keepAliveTempResults).mockResolvedValueOnce({ unavailable_ids: ['r0'] });
    const { result } = renderHook(() => useResultRetention(Array.from({ length: 205 }, (_, i) => `r${i}`)));
    await act(async () => {});
    expect(vi.mocked(rainApi.keepAliveTempResults).mock.calls.map(([ids]) => ids.length)).toEqual([100, 100, 5]);
    expect(result.current.unavailableIds.has('r0')).toBe(true);
  });

  it('retries transient errors on reconnect and focus without declaring expiry', async () => {
    vi.mocked(rainApi.keepAliveTempResults).mockRejectedValueOnce(new Error('offline'));
    const { result } = renderHook(() => useResultRetention(['a']));
    await act(async () => {});
    expect(result.current.unavailableIds.size).toBe(0);
    await act(async () => { window.dispatchEvent(new Event('online')); });
    await act(async () => { window.dispatchEvent(new Event('focus')); });
    expect(rainApi.keepAliveTempResults).toHaveBeenCalledTimes(3);
  });

  it('ignores stale responses after switching results', async () => {
    let finish!: (response: { unavailable_ids: string[] }) => void;
    vi.mocked(rainApi.keepAliveTempResults).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    const { result, rerender } = renderHook(({ ids }) => useResultRetention(ids), { initialProps: { ids: ['a'] } });
    rerender({ ids: ['b'] });
    await act(async () => { finish({ unavailable_ids: ['a'] }); });
    expect(result.current.unavailableIds.size).toBe(0);
    expect(vi.mocked(rainApi.keepAliveTempResults).mock.calls[0][1]?.aborted).toBe(true);
  });

  it('times out stalled renewal and retries on the next cycle', async () => {
    vi.mocked(rainApi.keepAliveTempResults).mockImplementationOnce(() => new Promise(() => {}));
    renderHook(() => useResultRetention(['a']));
    await act(async () => vi.advanceTimersByTimeAsync(5 * 60_000));
    expect(rainApi.keepAliveTempResults).toHaveBeenCalledTimes(2);
  });

  it('one viewer closing does not stop another viewer renewing the same result', async () => {
    const first = renderHook(() => useResultRetention(['shared']));
    const second = renderHook(() => useResultRetention(['shared']));
    await act(async () => {});
    first.unmount();
    await act(async () => vi.advanceTimersByTimeAsync(5 * 60_000));
    expect(rainApi.keepAliveTempResults).toHaveBeenCalledTimes(3);
    second.unmount();
  });
});
