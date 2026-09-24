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
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, reject, resolve };
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

  it('uploads each selected file independently with at most two active requests', async () => {
    const requests = new Map<string, ReturnType<typeof deferred<UploadResponse>>>();
    vi.mocked(rainApi.uploadLogs).mockImplementation(async (_issueCode, files) => {
      const file = files[0];
      const request = deferred<UploadResponse>();
      requests.set(file.name, request);
      return request.promise;
    });
    const loadBundles = vi.fn().mockResolvedValue(undefined);
    const loadIssues = vi.fn().mockResolvedValue(undefined);
    const { result, unmount } = renderHook(() =>
      useUploadTask({ currentIssueCode: 'ISSUE-QUEUE-A', loadBundles, loadIssues })
    );

    await act(async () => {
      await result.current.performUpload([
        new File(['a'], 'a.log'),
        new File(['b'], 'b.log'),
        new File(['c'], 'c.log')
      ]);
      await Promise.resolve();
    });

    expect(vi.mocked(rainApi.uploadLogs).mock.calls.map(([, files]) => files.map((file) => file.name))).toEqual([
      ['a.log'],
      ['b.log']
    ]);
    expect(result.current.uploadTasks.map((task) => task.status)).toEqual([
      'UPLOADING',
      'UPLOADING',
      'QUEUED'
    ]);

    await act(async () => {
      requests.get('a.log')!.resolve(uploadResponse('queue-a'));
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(vi.mocked(rainApi.uploadLogs).mock.calls.map(([, files]) => files.map((file) => file.name))).toEqual([
      ['a.log'],
      ['b.log'],
      ['c.log']
    ]);
    expect(result.current.uploadTasks.find((task) => task.name === 'a.log')?.status).toBe('ACCEPTED');

    await act(async () => {
      requests.get('b.log')!.resolve(uploadResponse('queue-b'));
      requests.get('c.log')!.resolve(uploadResponse('queue-c'));
      await Promise.resolve();
      await Promise.resolve();
    });
    unmount();
  });

  it('keeps a failed file independent from later files in the same selection', async () => {
    const requests = new Map<string, ReturnType<typeof deferred<UploadResponse>>>();
    vi.mocked(rainApi.uploadLogs).mockImplementation(async (_issueCode, files) => {
      const file = files[0];
      const request = deferred<UploadResponse>();
      requests.set(file.name, request);
      return request.promise;
    });
    const { result, unmount } = renderHook(() =>
      useUploadTask({
        currentIssueCode: 'ISSUE-QUEUE-B',
        loadBundles: vi.fn().mockResolvedValue(undefined),
        loadIssues: vi.fn().mockResolvedValue(undefined)
      })
    );

    await act(async () => {
      await result.current.performUpload([
        new File(['a'], 'failed.log'),
        new File(['b'], 'active.log'),
        new File(['c'], 'queued.log')
      ]);
      await Promise.resolve();
    });
    await act(async () => {
      requests.get('failed.log')!.reject(Object.assign(new Error('invalid upload'), { status: 400 }));
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(result.current.uploadTasks.find((task) => task.name === 'failed.log')?.status).toBe('FAILED');
    expect(requests.has('queued.log')).toBe(true);

    await act(async () => {
      requests.get('active.log')!.resolve(uploadResponse('queue-active'));
      requests.get('queued.log')!.resolve(uploadResponse('queue-queued'));
      await Promise.resolve();
      await Promise.resolve();
    });
    unmount();
  });

  it('filters tasks by their original Issue when the selected Issue changes', async () => {
    const issueAUpload = deferred<UploadResponse>();
    vi.mocked(rainApi.uploadLogs).mockReturnValueOnce(issueAUpload.promise);
    const { result, rerender, unmount } = renderHook(
      ({ issueCode }) =>
        useUploadTask({
          currentIssueCode: issueCode,
          loadBundles: vi.fn().mockResolvedValue(undefined),
          loadIssues: vi.fn().mockResolvedValue(undefined)
        }),
      { initialProps: { issueCode: 'ISSUE-QUEUE-C-A' } }
    );

    await act(async () => {
      await result.current.performUpload([new File(['a'], 'issue-a.log')]);
      await Promise.resolve();
    });
    rerender({ issueCode: 'ISSUE-QUEUE-C-B' });
    expect(result.current.uploadTasks).toEqual([]);
    expect(result.current.uploading).toBe(false);
    expect(result.current.uploadDisabled).toBe(false);

    await act(async () => {
      issueAUpload.resolve(uploadResponse('queue-issue-a'));
      await Promise.resolve();
      await Promise.resolve();
    });
    unmount();
  });

  it('keeps an in-flight upload associated with its original Issue when switching Issues', async () => {
    const uploadA = deferred<UploadResponse>();
    vi.mocked(rainApi.uploadLogs).mockReturnValueOnce(uploadA.promise);
    const loadBundles = vi.fn().mockResolvedValue(undefined);
    const loadIssues = vi.fn().mockResolvedValue(undefined);
    const { result, rerender, unmount } = renderHook(
      ({ issueCode }) => useUploadTask({ currentIssueCode: issueCode, loadBundles, loadIssues }),
      { initialProps: { issueCode: 'ISSUE-A' } }
    );
    let uploadPromise!: Promise<void>;

    await act(async () => {
      uploadPromise = result.current.performUpload([new File(['a'], 'a.log')]);
      await Promise.resolve();
    });
    expect(result.current.uploading).toBe(true);
    expect(result.current.uploadDisabled).toBe(false);

    rerender({ issueCode: 'ISSUE-B' });
    act(() => result.current.resetSelection());

    expect(result.current.uploading).toBe(false);
    expect(result.current.uploadDisabled).toBe(false);
    expect(result.current.uploadingRef.current).toBe(false);

    await act(async () => {
      uploadA.resolve(uploadResponse('task-a', 'PROCESSING'));
      await uploadPromise;
    });
    expect(result.current.uploading).toBe(false);
    expect(result.current.uploadDisabled).toBe(false);
    unmount();
  });

  it('does not show an old Issue upload failure after switching Issues', async () => {
    const uploadA = deferred<UploadResponse>();
    vi.mocked(rainApi.uploadLogs)
      .mockReturnValueOnce(uploadA.promise)
      .mockResolvedValueOnce(uploadResponse('task-b', 'PROCESSING'));
    const loadBundles = vi.fn().mockResolvedValue(undefined);
    const loadIssues = vi.fn().mockResolvedValue(undefined);
    const { result, rerender, unmount } = renderHook(
      ({ issueCode }) => useUploadTask({ currentIssueCode: issueCode, loadBundles, loadIssues }),
      { initialProps: { issueCode: 'ISSUE-A' } }
    );
    let uploadPromise!: Promise<void>;

    await act(async () => {
      uploadPromise = result.current.performUpload([new File(['a'], 'a.log')]);
      await Promise.resolve();
    });
    rerender({ issueCode: 'ISSUE-B' });
    act(() => result.current.resetSelection());
    expect(result.current.uploadDisabled).toBe(false);

    await act(async () => {
      uploadA.reject(new Error('Issue A upload failed'));
      await uploadPromise;
    });

    expect(result.current.uploading).toBe(false);
    expect(result.current.uploadDisabled).toBe(false);
    expect(result.current.uploadFailed).toBe(false);
    expect(result.current.uploadError).toBeNull();
    expect(result.current.uploadSelection).toEqual([]);
    expect(result.current.uploadingRef.current).toBe(false);

    await act(async () => {
      await result.current.performUpload([new File(['b'], 'b.log')]);
    });
    expect(rainApi.uploadLogs).toHaveBeenCalledTimes(2);
    unmount();
  });

  it('shows a failed upload when its Issue context has not been reset', async () => {
    vi.mocked(rainApi.uploadLogs).mockRejectedValueOnce(new Error('current Issue upload failed'));
    const loadBundles = vi.fn().mockResolvedValue(undefined);
    const loadIssues = vi.fn().mockResolvedValue(undefined);
    const { result, unmount } = renderHook(() =>
      useUploadTask({ currentIssueCode: 'ISSUE-CURRENT-FAIL', loadBundles, loadIssues })
    );

    await act(async () => {
      await result.current.performUpload([new File(['a'], 'a.log')]);
    });

    expect(result.current.uploading).toBe(false);
    expect(result.current.uploadDisabled).toBe(false);
    expect(result.current.uploadFailed).toBe(true);
    expect(result.current.uploadError).toContain('接收结果未确认');
    expect(result.current.uploadSelection).toEqual([{ name: 'a.log', sizeBytes: 1 }]);
    expect(result.current.uploadingRef.current).toBe(false);
    unmount();
  });

  it('does not let an old Issue post-upload refresh invalidate the selected Issue request', async () => {
    const uploadA = deferred<UploadResponse>();
    const issueB = deferred<IssueBundlesResponse>();
    let issueARequests = 0;
    vi.mocked(rainApi.uploadLogs).mockReturnValueOnce(uploadA.promise);
    vi.mocked(rainApi.fetchIssueBundles).mockImplementation(async (code) => {
      if (code === 'ISSUE-B') return issueB.promise;
      issueARequests += 1;
      return bundlesResponse([]);
    });
    const loadIssues = vi.fn().mockResolvedValue(undefined);
    const onIssueMissing = vi.fn();
    const { result, rerender, unmount } = renderHook(
      ({ issueCode }) => {
        const bundleState = useIssueBundles(issueCode, onIssueMissing);
        const uploadState = useUploadTask({
          currentIssueCode: issueCode,
          loadBundles: bundleState.loadBundles,
          loadIssues
        });
        return { bundleState, uploadState };
      },
      { initialProps: { issueCode: 'ISSUE-A' } }
    );
    await settle();
    let uploadPromise!: Promise<void>;

    await act(async () => {
      uploadPromise = result.current.uploadState.performUpload([new File(['a'], 'a.log')]);
      await Promise.resolve();
    });
    rerender({ issueCode: 'ISSUE-B' });
    await settle();
    expect(vi.mocked(rainApi.fetchIssueBundles).mock.calls.map(([code]) => code)).toEqual([
      'ISSUE-A',
      'ISSUE-B'
    ]);

    await act(async () => {
      uploadA.resolve(uploadResponse('task-a', 'PROCESSING'));
      await uploadPromise;
    });
    await act(async () => {
      issueB.resolve(bundlesResponse([bundle('bundle-b', 'READY')]));
      await issueB.promise;
    });

    expect(issueARequests).toBe(1);
    expect(vi.mocked(rainApi.fetchIssueBundles).mock.calls.map(([code]) => code)).toEqual([
      'ISSUE-A',
      'ISSUE-B'
    ]);
    expect(result.current.bundleState.bundles).toEqual([bundle('bundle-b', 'READY')]);
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
    expect(result.current.hasProcessingBundles).toBe(true);
    expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(2);
    expect(result.current.bundles).toEqual([bundle('pending', 'READY')]);
    expect(result.current.hasProcessingBundles).toBe(false);
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
    expect(result.current.hasProcessingBundles).toBe(true);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(3);
    expect(result.current.bundles[0].status.upload_status).toBe('READY');
    expect(result.current.hasProcessingBundles).toBe(false);

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

  it('retries a failed Bundle refresh when the last snapshot has no active Bundle', async () => {
    vi.mocked(rainApi.fetchIssueBundles)
      .mockResolvedValueOnce(bundlesResponse([]))
      .mockRejectedValueOnce(new Error('temporary post-upload failure'))
      .mockResolvedValueOnce(bundlesResponse([bundle('newly-accepted', 'READY')]));
    const onIssueMissing = vi.fn();
    const { result, unmount } = renderHook(() => useIssueBundles('ISSUE-1', onIssueMissing));
    await settle();
    expect(result.current.bundles).toEqual([]);

    await act(async () => {
      await result.current.loadBundles('ISSUE-1');
    });
    expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(2);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2999);
    });
    expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(2);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(3);
    expect(result.current.bundles).toEqual([bundle('newly-accepted', 'READY')]);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(3);
    unmount();
  });

  it('suppresses hidden polling and refreshes immediately when visibility returns', async () => {
    let hidden = false;
    const hiddenSpy = vi.spyOn(document, 'hidden', 'get').mockImplementation(() => hidden);
    vi.mocked(rainApi.fetchIssueBundles)
      .mockResolvedValueOnce(bundlesResponse([bundle('hidden', 'PROCESSING')]))
      .mockResolvedValueOnce(bundlesResponse([bundle('hidden', 'READY')]));
    const onIssueMissing = vi.fn();
    const { result, unmount } = renderHook(() => useIssueBundles('ISSUE-1', onIssueMissing));

    try {
      await settle();
      hidden = true;
      act(() => document.dispatchEvent(new Event('visibilitychange')));

      await act(async () => {
        await vi.advanceTimersByTimeAsync(3000);
      });
      expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(1);

      hidden = false;
      await act(async () => {
        document.dispatchEvent(new Event('visibilitychange'));
        await Promise.resolve();
      });
      expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(2);
      expect(result.current.bundles).toEqual([bundle('hidden', 'READY')]);

      await act(async () => {
        await vi.advanceTimersByTimeAsync(3000);
      });
      expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(2);
    } finally {
      unmount();
      hiddenSpy.mockRestore();
    }
  });

  it('avoids a duplicate refresh after visibility changes twice during an in-flight poll', async () => {
    let hidden = false;
    let pollResolved = false;
    const hiddenSpy = vi.spyOn(document, 'hidden', 'get').mockImplementation(() => hidden);
    const inFlightPoll = deferred<IssueBundlesResponse>();
    vi.mocked(rainApi.fetchIssueBundles)
      .mockResolvedValueOnce(bundlesResponse([bundle('visibility-race', 'PROCESSING')]))
      .mockReturnValueOnce(inFlightPoll.promise)
      .mockResolvedValueOnce(bundlesResponse([bundle('visibility-race', 'PROCESSING')]))
      .mockResolvedValueOnce(bundlesResponse([bundle('visibility-race', 'READY')]));
    const onIssueMissing = vi.fn();
    const { result, unmount } = renderHook(() => useIssueBundles('ISSUE-1', onIssueMissing));

    try {
      await settle();
      await act(async () => {
        await vi.advanceTimersByTimeAsync(3000);
      });
      expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(2);

      hidden = true;
      act(() => document.dispatchEvent(new Event('visibilitychange')));
      hidden = false;
      act(() => document.dispatchEvent(new Event('visibilitychange')));
      hidden = true;
      act(() => document.dispatchEvent(new Event('visibilitychange')));

      await act(async () => {
        pollResolved = true;
        inFlightPoll.resolve(bundlesResponse([bundle('visibility-race', 'PROCESSING')]));
        await inFlightPoll.promise;
      });
      expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(2);

      hidden = false;
      await act(async () => {
        document.dispatchEvent(new Event('visibilitychange'));
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(3);
      expect(result.current.bundles).toEqual([bundle('visibility-race', 'PROCESSING')]);

      await act(async () => {
        await vi.advanceTimersByTimeAsync(2999);
      });
      expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(3);

      await act(async () => {
        await vi.advanceTimersByTimeAsync(1);
      });
      expect(rainApi.fetchIssueBundles).toHaveBeenCalledTimes(4);
      expect(result.current.bundles).toEqual([bundle('visibility-race', 'READY')]);
    } finally {
      if (!pollResolved) {
        inFlightPoll.resolve(bundlesResponse([bundle('visibility-race', 'READY')]));
        await inFlightPoll.promise;
      }
      unmount();
      hiddenSpy.mockRestore();
    }
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
