import { act, render, renderHook, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { IssueBundlesResponse } from '../src/api/types';
import { IssueExpirationNotice } from '../src/features/files/components/IssueExpirationNotice';
import { useIssueBundles } from '../src/features/files/hooks/useIssueBundles';
import { visibleInactivityExpiry } from '../src/features/files/issueExpiration';
import { ApiError, rainApi } from '../src/api/client';

vi.mock('../src/api/client', () => ({
  ApiError: class ApiError extends Error {
    constructor(message: string, readonly status?: number, readonly code?: string) {
      super(message);
    }
  },
  normalizeApiError: (error: unknown) => String(error),
  rainApi: {
    fetchIssueBundles: vi.fn(),
    fetchFileNode: vi.fn()
  }
}));

const NOW = new Date('2026-08-02T12:00:00Z');

function expiry(hours: number, renewedFromExpiring = false) {
  return {
    inactive_days: 7,
    expires_at: new Date(NOW.getTime() + hours * 60 * 60 * 1000).toISOString(),
    renewed_from_expiring: renewedFromExpiring
  };
}

function response(inactivityExpiry: ReturnType<typeof expiry> | null): IssueBundlesResponse {
  return {
    name: 'Issue',
    can_write: true,
    owner_username: 'owner',
    inactivity_expiry: inactivityExpiry,
    log_bundles: []
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, reject, resolve };
}

describe('Issue inactivity expiry notice', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(NOW);
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  it('starts at 72 hours and uses day and hour wording at the boundaries', () => {
    const { rerender } = render(
      <IssueExpirationNotice canWrite expiry={expiry(73)} />
    );
    expect(screen.queryByRole('status')).not.toBeInTheDocument();

    rerender(<IssueExpirationNotice canWrite expiry={expiry(72)} />);
    expect(screen.getByText('距离自动过期还有 3 天')).toBeInTheDocument();

    rerender(<IssueExpirationNotice canWrite expiry={expiry(25)} />);
    expect(screen.getByText('距离自动过期还有 2 天')).toBeInTheDocument();

    rerender(<IssueExpirationNotice canWrite expiry={expiry(24)} />);
    expect(screen.getByText('距离自动过期还有 24 小时')).toBeInTheDocument();

    rerender(<IssueExpirationNotice canWrite expiry={expiry(0)} />);
    expect(screen.getByText('该 Issue 已进入自动清理条件')).toBeInTheDocument();
  });

  it('is owner-only, explains whole-Issue deletion, and degrades safely for invalid dates', () => {
    const invalid = { inactive_days: 7, expires_at: 'not-a-date', renewed_from_expiring: false };
    const { rerender } = render(
      <IssueExpirationNotice canWrite={false} expiry={expiry(12)} />
    );
    expect(screen.queryByRole('status')).not.toBeInTheDocument();

    rerender(<IssueExpirationNotice canWrite expiry={invalid} />);
    expect(screen.getByText('该 Issue 已启用自动过期')).toBeInTheDocument();
    expect(
      screen.getByText('自动清理将删除整个 Issue 及其全部 Bundle、文件和日志内容。')
    ).toBeInTheDocument();
    expect(screen.queryByText(/预计时间/)).not.toBeInTheDocument();

    rerender(<IssueExpirationNotice canWrite expiry={expiry(12)} />);
    expect(screen.getByText(/访问或操作该 Issue 后会自动顺延/)).toBeInTheDocument();
    expect(screen.getByRole('status').textContent).not.toContain('Asia/Shanghai');
  });

  it('normalizes null and out-of-window snapshots before storing them', () => {
    expect(visibleInactivityExpiry(null, NOW.getTime())).toBeNull();
    expect(visibleInactivityExpiry(expiry(73), NOW.getTime())).toBeNull();
    expect(visibleInactivityExpiry(expiry(72), NOW.getTime())).toEqual(expiry(72));
    expect(visibleInactivityExpiry(expiry(168, true), NOW.getTime())).toEqual(expiry(168, true));
  });

  it('shows when this visit renewed an Issue that was already in the warning window', () => {
    render(<IssueExpirationNotice canWrite expiry={expiry(168, true)} />);
    expect(screen.getByText('本次访问已将自动过期时间顺延')).toBeInTheDocument();
    expect(screen.getByText(/预计时间/)).toBeInTheDocument();
  });

  it('clears stale state and ignores an older Issue response that arrives last', async () => {
    const first = deferred<IssueBundlesResponse>();
    const second = deferred<IssueBundlesResponse>();
    vi.mocked(rainApi.fetchIssueBundles)
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);
    const missing = vi.fn();
    const { result, rerender } = renderHook(
      ({ code }) => useIssueBundles(code, missing),
      { initialProps: { code: 'FIRST' } }
    );

    rerender({ code: 'SECOND' });
    expect(result.current.inactivityExpiry).toBeNull();
    await act(async () => second.resolve(response(expiry(12))));
    expect(result.current.inactivityExpiry).toEqual(expiry(12));
    await act(async () => first.resolve(response(expiry(1))));
    expect(result.current.inactivityExpiry).toEqual(expiry(12));

    vi.mocked(rainApi.fetchIssueBundles).mockResolvedValueOnce(response(null));
    await act(async () => result.current.loadBundles('SECOND'));
    expect(result.current.inactivityExpiry).toBeNull();

    vi.mocked(rainApi.fetchIssueBundles).mockResolvedValueOnce(response(expiry(73)));
    await act(async () => result.current.loadBundles('SECOND'));
    expect(result.current.inactivityExpiry).toBeNull();

    vi.mocked(rainApi.fetchIssueBundles).mockResolvedValueOnce(response(expiry(12)));
    await act(async () => result.current.loadBundles('SECOND'));
    expect(result.current.inactivityExpiry).toEqual(expiry(12));
    vi.mocked(rainApi.fetchIssueBundles).mockRejectedValueOnce(new Error('request failed'));
    await act(async () => result.current.loadBundles('SECOND'));
    expect(result.current.inactivityExpiry).toBeNull();

    vi.mocked(rainApi.fetchIssueBundles).mockResolvedValueOnce(response(expiry(12)));
    await act(async () => result.current.loadBundles('SECOND'));
    act(() => result.current.clearBundles());
    expect(result.current.inactivityExpiry).toBeNull();
  });

  it('invalidates pending success and failure responses when the page is cleared', async () => {
    const pendingSuccess = deferred<IssueBundlesResponse>();
    vi.mocked(rainApi.fetchIssueBundles).mockReturnValueOnce(pendingSuccess.promise);
    const successMissing = vi.fn();
    const { result, unmount } = renderHook(() => useIssueBundles('FIRST', successMissing));
    act(() => result.current.clearBundles());
    await act(async () => pendingSuccess.resolve(response(expiry(12))));
    expect(result.current.inactivityExpiry).toBeNull();
    expect(result.current.bundles).toEqual([]);
    unmount();

    const pendingFailure = deferred<IssueBundlesResponse>();
    vi.mocked(rainApi.fetchIssueBundles).mockReturnValueOnce(pendingFailure.promise);
    const failureMissing = vi.fn();
    const failureHook = renderHook(() => useIssueBundles('FIRST', failureMissing));
    act(() => failureHook.result.current.clearBundles());
    await act(async () => pendingFailure.reject(new Error('old request failed')));
    expect(failureHook.result.current.bundlesError).toBeNull();
    expect(failureHook.result.current.inactivityExpiry).toBeNull();
  });

  it('does not let an old 404 clear a newly selected Issue', async () => {
    const oldRequest = deferred<IssueBundlesResponse>();
    const newRequest = deferred<IssueBundlesResponse>();
    vi.mocked(rainApi.fetchIssueBundles)
      .mockReturnValueOnce(oldRequest.promise)
      .mockReturnValueOnce(newRequest.promise);
    const missing = vi.fn();
    const { result, rerender } = renderHook(
      ({ code }) => useIssueBundles(code, missing),
      { initialProps: { code: 'FIRST' } }
    );
    rerender({ code: 'SECOND' });
    await act(async () => newRequest.resolve(response(expiry(12))));
    await act(async () => oldRequest.reject(new ApiError('missing', 404, 'RESOURCE_NOT_FOUND')));
    expect(missing).not.toHaveBeenCalled();
    expect(result.current.inactivityExpiry).toEqual(expiry(12));
  });
});
