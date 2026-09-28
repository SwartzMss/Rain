import { useCallback, useEffect, useRef, useState } from 'react';
import { ApiError, RequestCancelledError, normalizeApiError, rainApi } from '../api/client';
import type { SearchExecutionStatus, TempResultPreviewResponse } from '../api/types';

export interface SearchExecutionSnapshot {
  status: SearchExecutionStatus;
  searchId: string | null;
  scopeKey: string | null;
  elapsedMs: number;
  errorMessage: string | null;
  cancelUnconfirmed: boolean;
}

interface SearchExecutionOptions {
  scopeKey: string;
  onSuccess?: (result: TempResultPreviewResponse) => void;
}

interface ActiveExecution {
  generation: number;
  searchId: string;
  scopeKey: string;
  startedAt: number;
  reservationController: AbortController;
  executionController: AbortController;
  cancelController: AbortController | null;
  cancelToken: string | null;
  cancelRequested: boolean;
  settleCancellationUi: boolean;
  finished: boolean;
}

const initialSnapshot: SearchExecutionSnapshot = {
  status: 'IDLE',
  searchId: null,
  scopeKey: null,
  elapsedMs: 0,
  errorMessage: null,
  cancelUnconfirmed: false
};

function newSearchId(): string {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
    return crypto.randomUUID();
  }
  return `${Date.now().toString(16)}-${Math.random().toString(16).slice(2)}`;
}

function elapsedSince(startedAt: number): number {
  return Math.max(0, performance.now() - startedAt);
}

function withTimeout<T>(promise: Promise<T>, controller: AbortController, timeoutMs: number): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timeout = window.setTimeout(() => {
      controller.abort();
      reject(new Error('搜索请求预注册超时'));
    }, timeoutMs);
    promise.then(
      (value) => {
        window.clearTimeout(timeout);
        resolve(value);
      },
      (error) => {
        window.clearTimeout(timeout);
        reject(error);
      }
    );
  });
}

export function useSearchExecution() {
  const [snapshot, setSnapshot] = useState<SearchExecutionSnapshot>(initialSnapshot);
  const generationRef = useRef(0);
  const activeRef = useRef<ActiveExecution | null>(null);

  const updateElapsed = useCallback(() => {
    const active = activeRef.current;
    if (!active || active.finished) return;
    setSnapshot((current) => {
      if (current.searchId !== active.searchId || current.status === 'CANCELLED' || current.status === 'SUCCEEDED' || current.status === 'FAILED') {
        return current;
      }
      return { ...current, elapsedMs: elapsedSince(active.startedAt) };
    });
  }, []);

  useEffect(() => {
    if (snapshot.status !== 'RUNNING' && snapshot.status !== 'CANCELLING') return;
    const timer = window.setInterval(updateElapsed, 250);
    return () => window.clearInterval(timer);
  }, [snapshot.status, updateElapsed]);

  const requestCancellation = useCallback(async (active: ActiveExecution, invalidate: boolean) => {
    if (active.finished) return;
    active.cancelRequested = true;
    if (invalidate) {
      active.settleCancellationUi = true;
      generationRef.current += 1;
    }
    active.executionController.abort();
    if (invalidate) {
      setSnapshot((current) => current.searchId === active.searchId
        ? { ...current, status: 'CANCELLING', cancelUnconfirmed: false, elapsedMs: elapsedSince(active.startedAt) }
        : current);
    }

    if (!active.cancelToken) return;
    const updateCancellationUi = invalidate || active.settleCancellationUi;
    active.cancelController?.abort();
    active.cancelController = new AbortController();
    try {
      const response = await rainApi.cancelSearchRequest(active.searchId, active.cancelToken, active.cancelController.signal);
      if (!response || response.status === 'cancelled' || response.status === 'timeout') {
        active.finished = true;
        if (updateCancellationUi) setSnapshot((current) => current.searchId === active.searchId ? { ...current, status: 'CANCELLED', cancelUnconfirmed: false } : current);
      } else if (response.status === 'completed' || response.status === 'failed') {
        active.finished = true;
        if (updateCancellationUi) setSnapshot((current) => current.searchId === active.searchId ? { ...current, status: 'CANCELLED', cancelUnconfirmed: false } : current);
      } else if (updateCancellationUi) {
        setSnapshot((current) => current.searchId === active.searchId ? { ...current, status: 'CANCELLING', cancelUnconfirmed: true } : current);
      }
    } catch (error) {
      if (error instanceof RequestCancelledError || !updateCancellationUi) return;
      setSnapshot((current) => current.searchId === active.searchId
        ? { ...current, status: 'CANCELLING', cancelUnconfirmed: true, errorMessage: '取消请求未确认，可重试；服务器安全时限仍生效' }
        : current);
    }
  }, []);

  const cancel = useCallback(async () => {
    const active = activeRef.current;
    if (!active || active.finished) return;
    await requestCancellation(active, true);
  }, [requestCancellation]);

  const execute = useCallback(async (
    payload: Parameters<typeof rainApi.previewTempResult>[0],
    options: SearchExecutionOptions
  ): Promise<TempResultPreviewResponse | undefined> => {
    const previous = activeRef.current;
    if (previous && !previous.finished) void requestCancellation(previous, false);
    const generation = ++generationRef.current;
    const active: ActiveExecution = {
      generation,
      searchId: newSearchId(),
      scopeKey: options.scopeKey,
      startedAt: performance.now(),
      reservationController: new AbortController(),
      executionController: new AbortController(),
      cancelController: null,
      cancelToken: null,
      cancelRequested: false,
      settleCancellationUi: false,
      finished: false
    };
    activeRef.current = active;
    setSnapshot({ status: 'RUNNING', searchId: active.searchId, scopeKey: active.scopeKey, elapsedMs: 0, errorMessage: null, cancelUnconfirmed: false });

    const isCurrent = () => generationRef.current === generation && activeRef.current?.searchId === active.searchId;
    try {
      const reservation = await withTimeout(
        rainApi.reserveSearchRequest(active.searchId, active.reservationController.signal),
        active.reservationController,
        5_000
      );
      active.cancelToken = reservation.cancel_token;
      if (active.cancelRequested || !isCurrent()) {
        await requestCancellation(active, false);
        return undefined;
      }
      const result = await rainApi.previewTempResult(payload, {
        searchId: active.searchId,
        cancelToken: active.cancelToken,
        signal: active.executionController.signal
      });
      if (!isCurrent() || active.cancelRequested) return undefined;
      active.finished = true;
      setSnapshot({ status: 'SUCCEEDED', searchId: active.searchId, scopeKey: active.scopeKey, elapsedMs: elapsedSince(active.startedAt), errorMessage: null, cancelUnconfirmed: false });
      options.onSuccess?.(result);
      return result;
    } catch (error) {
      if (active.finished) return undefined;
      if (active.cancelRequested) {
        if (!active.cancelToken) {
          active.finished = true;
          if (active.settleCancellationUi) {
            setSnapshot((current) => current.searchId === active.searchId
              ? { ...current, status: 'CANCELLED', cancelUnconfirmed: false, elapsedMs: elapsedSince(active.startedAt) }
              : current);
          }
        } else if (active.settleCancellationUi) {
          setSnapshot((current) => current.searchId === active.searchId
            ? { ...current, status: 'CANCELLING', elapsedMs: elapsedSince(active.startedAt) }
            : current);
        }
        return undefined;
      }
      if (!isCurrent()) return undefined;
      if (error instanceof RequestCancelledError) {
        setSnapshot((current) => ({ ...current, status: 'CANCELLING', elapsedMs: elapsedSince(active.startedAt) }));
        return undefined;
      }
      const message = error instanceof ApiError && error.code === 'TEMP_RESULT_SCAN_TIMEOUT'
        ? '搜索达到系统安全时限，已停止。建议缩小搜索范围或调整搜索条件'
        : normalizeApiError(error);
      setSnapshot({ status: 'FAILED', searchId: active.searchId, scopeKey: active.scopeKey, elapsedMs: elapsedSince(active.startedAt), errorMessage: message, cancelUnconfirmed: false });
      active.finished = true;
      return undefined;
    }
  }, [cancel, requestCancellation]);

  useEffect(() => () => {
    const active = activeRef.current;
    if (!active) return;
    generationRef.current += 1;
    void requestCancellation(active, false);
  }, [requestCancellation]);

  return { snapshot, execute, cancel };
}
