import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { IssueBundlesResponse, UploadResponse, UploadStatus, UploadSummary } from '../src/api/types';
import { rainApi } from '../src/api/client';
import { useIssueBundles } from '../src/features/files/hooks/useIssueBundles';
import { useUploadTask } from '../src/features/files/hooks/useUploadTask';

vi.mock('../src/api/client', () => ({
  ApiError: class ApiError extends Error {
    constructor(message: string, readonly status?: number, readonly code?: string) {
      super(message);
    }
  },
  normalizeApiError: (error: unknown) => String(error),
  rainApi: {
    fetchFileNode: vi.fn(),
    fetchIssueBundles: vi.fn(),
    uploadLogs: vi.fn()
  }
}));

function uploadResponse(taskId: string, status: UploadStatus = 'PROCESSING'): UploadResponse {
  return {
    task_id: taskId,
    issue_code: 'ISSUE-1',
    bundle_hash: `bundle-${taskId}`,
    status,
    stage: status === 'READY' ? 'READY' : status === 'PENDING' ? 'PENDING' : 'INDEXING',
    file_count: 1,
    total_bytes: 1
  };
}

function bundle(hash: string, status: UploadStatus): UploadSummary {
  return {
    hash,
    name: hash,
    status: { upload_status: status },
    stage: status === 'READY' ? 'READY' : status === 'PENDING' ? 'PENDING' : status === 'FAILED' ? 'FAILED' : 'INDEXING',
    size_bytes: 1
  };
}

function bundlesResponse(logBundles: UploadSummary[]): IssueBundlesResponse {
  return {
    name: 'Issue',
    can_write: true,
    owner_username: 'owner',
    inactivity_expiry: null,
    log_bundles: logBundles
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

async function settle() {
  await act(async () => {
    await Promise.resolve();
  });
}

describe('upload and bundle polling behavior', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.mocked(rainApi.fetchFileNode).mockResolvedValue({ children: [] });
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.resetAllMocks();
  });

  it('allows upload B to start after upload A is accepted with PROCESSING status', async () => {
    let finishRefreshA!: () => void;
    const refreshA = new Promise<void>((resolve) => {
      finishRefreshA = resolve;
    });
    vi.mocked(rainApi.uploadLogs)
      .mockResolvedValueOnce(uploadResponse('task-1', 'PROCESSING'))
      .mockResolvedValueOnce(uploadResponse('task-2', 'PROCESSING'));
    const loadBundles = vi.fn().mockReturnValueOnce(refreshA).mockResolvedValue(undefined);
    const loadIssues = vi.fn().mockResolvedValue(undefined);
    const { result, unmount } = renderHook(() =>
      useUploadTask({
        currentIssueCode: 'ISSUE-1',
        loadBundles,
        loadIssues
      })
    );

    await act(async () => {
      void result.current.performUpload([new File(['a'], 'a.log')]);
      await Promise.resolve();
    });
    expect(result.current.uploading).toBe(false);
    expect(result.current.uploadDisabled).toBe(false);

    await act(async () => {
      void result.current.performUpload([new File(['b'], 'b.log')]);
      await Promise.resolve();
    });

    expect(rainApi.uploadLogs).toHaveBeenCalledTimes(2);
    finishRefreshA();
    await settle();
    unmount();
  });

  it('starts polling when the selected Issue contains a PENDING Bundle', async () => {
    vi.mocked(rainApi.fetchIssueBundles)
      .mockResolvedValueOnce(bundlesResponse([bundle('pending', 'PENDING')]))
      .mockResolvedValueOnce(bundlesResponse([bundle('pending', 'READY')]));
    const onIssueMissing = vi.fn();
    const { result, unmount } = renderHook(() => useIssueBundles('ISSUE-1', onIssueMissing));
    await settle();
    expect(result.current.bundles).toEqual([bundle('pending', 'PENDING')]);
    expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(2);
    expect(result.current.bundles).toEqual([bundle('pending', 'READY')]);
    unmount();
  });

  it('keeps polling repeated PROCESSING responses until the Bundle is READY, then stops', async () => {
    vi.mocked(rainApi.fetchIssueBundles)
      .mockResolvedValueOnce(bundlesResponse([bundle('processing', 'PROCESSING')]))
      .mockResolvedValueOnce(bundlesResponse([bundle('processing', 'PROCESSING')]))
      .mockResolvedValueOnce(bundlesResponse([bundle('processing', 'READY')]));
    const onIssueMissing = vi.fn();
    const { result, unmount } = renderHook(() => useIssueBundles('ISSUE-1', onIssueMissing));
    await settle();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(2);
    expect(result.current.bundles[0].status.upload_status).toBe('PROCESSING');

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(3);
    expect(result.current.bundles[0].status.upload_status).toBe('READY');

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(3);
    unmount();
  });

  it('stops polling after a PROCESSING Bundle reaches FAILED', async () => {
    vi.mocked(rainApi.fetchIssueBundles)
      .mockResolvedValueOnce(bundlesResponse([bundle('failed', 'PROCESSING')]))
      .mockResolvedValueOnce(bundlesResponse([bundle('failed', 'FAILED')]));
    const onIssueMissing = vi.fn();
    const { result, unmount } = renderHook(() => useIssueBundles('ISSUE-1', onIssueMissing));
    await settle();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(2);
    expect(result.current.bundles[0].status.upload_status).toBe('FAILED');

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(2);
    unmount();
  });

  it('continues polling when one Bundle is READY while another remains PROCESSING', async () => {
    vi.mocked(rainApi.fetchIssueBundles)
      .mockResolvedValueOnce(bundlesResponse([bundle('a', 'PROCESSING'), bundle('b', 'PROCESSING')]))
      .mockResolvedValueOnce(bundlesResponse([bundle('a', 'READY'), bundle('b', 'PROCESSING')]))
      .mockResolvedValueOnce(bundlesResponse([bundle('a', 'READY'), bundle('b', 'READY')]));
    const onIssueMissing = vi.fn();
    const { result, unmount } = renderHook(() => useIssueBundles('ISSUE-1', onIssueMissing));
    await settle();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(2);
    expect(result.current.bundles.map((item) => item.status.upload_status)).toEqual(['READY', 'PROCESSING']);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(3);
    expect(result.current.bundles.map((item) => item.status.upload_status)).toEqual(['READY', 'READY']);
    unmount();
  });

  it('retries a transient polling failure without discarding the last Bundle snapshot', async () => {
    vi.mocked(rainApi.fetchIssueBundles)
      .mockResolvedValueOnce(bundlesResponse([bundle('retry', 'PROCESSING')]))
      .mockRejectedValueOnce(new Error('temporary failure'))
      .mockResolvedValueOnce(bundlesResponse([bundle('retry', 'READY')]));
    const onIssueMissing = vi.fn();
    const { result, unmount } = renderHook(() => useIssueBundles('ISSUE-1', onIssueMissing));
    await settle();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(2);
    expect(result.current.bundles).toEqual([bundle('retry', 'PROCESSING')]);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(3);
    expect(result.current.bundles).toEqual([bundle('retry', 'READY')]);
    unmount();
  });

  it('does not overlap polling requests and schedules the next poll after the current one settles', async () => {
    const secondPoll = deferred<IssueBundlesResponse>();
    vi.mocked(rainApi.fetchIssueBundles)
      .mockResolvedValueOnce(bundlesResponse([bundle('slow', 'PROCESSING')]))
      .mockReturnValueOnce(secondPoll.promise)
      .mockResolvedValueOnce(bundlesResponse([bundle('slow', 'READY')]));
    const onIssueMissing = vi.fn();
    const { result, unmount } = renderHook(() => useIssueBundles('ISSUE-1', onIssueMissing));
    await settle();
    let pollResolved = false;

    try {
      await act(async () => {
        await vi.advanceTimersByTimeAsync(3001);
      });
      expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(2);

      await act(async () => {
        await vi.advanceTimersByTimeAsync(3000);
      });
      expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(2);

      await act(async () => {
        pollResolved = true;
        secondPoll.resolve(bundlesResponse([bundle('slow', 'PROCESSING')]));
        await secondPoll.promise;
      });
      expect(result.current.bundles[0].status.upload_status).toBe('PROCESSING');

      await act(async () => {
        await vi.advanceTimersByTimeAsync(2999);
      });
      expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(2);

      await act(async () => {
        await vi.advanceTimersByTimeAsync(1);
      });
      expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(3);
    } finally {
      if (!pollResolved) {
        secondPoll.resolve(bundlesResponse([bundle('slow', 'READY')]));
        await secondPoll.promise;
      }
      unmount();
    }
  });

  it('cancels a pending polling timer when the hook unmounts', async () => {
    vi.mocked(rainApi.fetchIssueBundles).mockResolvedValueOnce(
      bundlesResponse([bundle('unmounted', 'PROCESSING')])
    );
    const onIssueMissing = vi.fn();
    const { unmount } = renderHook(() => useIssueBundles('ISSUE-1', onIssueMissing));
    await settle();
    unmount();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(1);
  });

  it('cancels the old polling chain when switching Issues', async () => {
    let issueBRequests = 0;
    vi.mocked(rainApi.fetchIssueBundles).mockImplementation(async (code) => {
      if (code === 'ISSUE-A') return bundlesResponse([bundle('a', 'PROCESSING')]);
      issueBRequests += 1;
      return bundlesResponse([bundle('b', issueBRequests === 1 ? 'PROCESSING' : 'READY')]);
    });
    const onIssueMissing = vi.fn();
    const { result, rerender, unmount } = renderHook(
      ({ issueCode }) => useIssueBundles(issueCode, onIssueMissing),
      { initialProps: { issueCode: 'ISSUE-A' } }
    );
    await settle();
    rerender({ issueCode: 'ISSUE-B' });
    await settle();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(3);
    expect(vi.mocked(rainApi.fetchIssueBundles).mock.calls.map(([code]) => code)).toEqual([
      'ISSUE-A',
      'ISSUE-B',
      'ISSUE-B'
    ]);
    expect(result.current.bundles[0].status.upload_status).toBe('READY');
    unmount();
  });
});
