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
  workspaceSessionId: string | null;
  onSuccess?: (result: TempResultPreviewResponse) => void;
}

interface ActiveExecution {
  generation: number;
  searchId: string;
  scopeKey: string;
  workspaceSessionId: string | null;
  startedAt: number;
  reservationController: AbortController;
  executionController: AbortController;
  cancelController: AbortController | null;
  cancelToken: string | null;
  cancellationPromise: Promise<CancellationResult> | null;
  cancelRequested: boolean;
  settleCancellationUi: boolean;
  finished: boolean;
  finishedPromise: Promise<void>;
  resolveFinished: (() => void) | null;
}

type CancellationResult = 'finished' | 'waiting-for-token' | 'unconfirmed';

const CANCEL_REQUEST_TIMEOUT_MS = 5_000;
const CANCELLATION_CONFIRMATION_TIMEOUT_MS = 10_000;
const REPLACEMENT_CANCEL_WAIT_MS = 6_000;

const initialSnapshot: SearchExecutionSnapshot = {
  status: 'IDLE',
  searchId: null,
  scopeKey: null,
  elapsedMs: 0,
  errorMessage: null,
  cancelUnconfirmed: false
};

function newSearchId(): string {
  const bytes = new Uint8Array(16);
  if (typeof crypto !== 'undefined' && typeof crypto.getRandomValues === 'function') {
    crypto.getRandomValues(bytes);
  } else {
    for (let index = 0; index < bytes.length; index += 1) {
      bytes[index] = Math.floor(Math.random() * 256);
    }
  }
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

function finishExecution(active: ActiveExecution): void {
  if (active.finished) return;
  active.finished = true;
  active.resolveFinished?.();
  active.resolveFinished = null;
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

function waitForFinished(active: ActiveExecution, timeoutMs: number): Promise<boolean> {
  if (active.finished) return Promise.resolve(true);
  return new Promise((resolve) => {
    let settled = false;
    const timeout = window.setTimeout(() => {
      if (settled) return;
      settled = true;
      resolve(active.finished);
    }, timeoutMs);
    active.finishedPromise.then(() => {
      if (settled) return;
      settled = true;
      window.clearTimeout(timeout);
      resolve(true);
    });
  });
}

export function useSearchExecution() {
  const [snapshot, setSnapshot] = useState<SearchExecutionSnapshot>(initialSnapshot);
  const generationRef = useRef(0);
  const requestSequenceRef = useRef(0);
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

  const requestCancellation = useCallback((active: ActiveExecution, invalidate: boolean): Promise<CancellationResult> => {
    if (active.finished) return Promise.resolve('finished');
    const existingCancellation = active.cancellationPromise;
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

    if (existingCancellation) return existingCancellation;
    const updateCancellationUi = () => invalidate || active.settleCancellationUi;
    const cancelToken = active.cancelToken;
    if (!cancelToken) return Promise.resolve('waiting-for-token');
    const cancellationPromise = (async (): Promise<CancellationResult> => {
      const deadline = performance.now() + CANCELLATION_CONFIRMATION_TIMEOUT_MS;
      const markUnconfirmed = () => {
        if (updateCancellationUi()) {
          setSnapshot((current) => current.searchId === active.searchId
            ? { ...current, status: 'CANCELLING', cancelUnconfirmed: true, errorMessage: '取消请求未确认，可重试；服务器安全时限仍生效' }
            : current);
        }
      };
      try {
        while (!active.finished) {
          const remainingMs = deadline - performance.now();
          if (remainingMs <= 0) {
            markUnconfirmed();
            return 'unconfirmed';
          }
          active.cancelController?.abort();
          active.cancelController = new AbortController();
          const response = await withTimeout(
            rainApi.cancelSearchRequest(active.searchId, cancelToken, active.cancelController.signal),
            active.cancelController,
            Math.min(CANCEL_REQUEST_TIMEOUT_MS, remainingMs)
          );
          if (!response || response.status === 'cancelled' || response.status === 'timeout') {
            finishExecution(active);
            if (updateCancellationUi()) setSnapshot((current) => current.searchId === active.searchId ? { ...current, status: 'CANCELLED', cancelUnconfirmed: false } : current);
          } else if (response.status === 'completed' || response.status === 'failed') {
            finishExecution(active);
            if (updateCancellationUi()) setSnapshot((current) => current.searchId === active.searchId ? { ...current, status: 'CANCELLED', cancelUnconfirmed: false } : current);
          } else if (updateCancellationUi()) {
            setSnapshot((current) => current.searchId === active.searchId ? { ...current, status: 'CANCELLING', cancelUnconfirmed: true } : current);
          }
          if (active.finished) return 'finished';
          const waitMs = Math.min(1_000, deadline - performance.now());
          if (waitMs <= 0) {
            markUnconfirmed();
            return 'unconfirmed';
          }
          await new Promise<void>((resolve) => window.setTimeout(resolve, waitMs));
        }
        return 'finished';
      } catch (error) {
        if (error instanceof RequestCancelledError || !updateCancellationUi()) return 'unconfirmed';
        markUnconfirmed();
        return 'unconfirmed';
      } finally {
        active.cancellationPromise = null;
      }
    })();
    active.cancellationPromise = cancellationPromise;
    return cancellationPromise;
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
    const requestSequence = ++requestSequenceRef.current;
    const previous = activeRef.current;
    if (previous && !previous.finished) {
      const cancellation = await requestCancellation(previous, false);
      if (cancellation === 'waiting-for-token') {
        await waitForFinished(previous, REPLACEMENT_CANCEL_WAIT_MS);
      }
      if (!previous.finished) {
        if (requestSequence !== requestSequenceRef.current) return undefined;
        previous.settleCancellationUi = true;
        setSnapshot((current) => current.searchId === previous.searchId
          ? { ...current, status: 'CANCELLING', cancelUnconfirmed: true, errorMessage: '旧搜索尚未确认停止，请重试取消后再搜索' }
          : current);
        return undefined;
      }
    }
    if (requestSequence !== requestSequenceRef.current) return undefined;
    const generation = ++generationRef.current;
    let resolveFinished!: () => void;
    const finishedPromise = new Promise<void>((resolve) => {
      resolveFinished = resolve;
    });
    const active: ActiveExecution = {
      generation,
      searchId: newSearchId(),
      scopeKey: options.scopeKey,
      workspaceSessionId: options.workspaceSessionId,
      startedAt: performance.now(),
      reservationController: new AbortController(),
      executionController: new AbortController(),
      cancelController: null,
      cancelToken: null,
      cancellationPromise: null,
      cancelRequested: false,
      settleCancellationUi: false,
      finished: false,
      finishedPromise,
      resolveFinished
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
        signal: active.executionController.signal,
        workspaceSessionId: active.workspaceSessionId
      });
      if (!isCurrent() || active.cancelRequested) return undefined;
      finishExecution(active);
      setSnapshot({ status: 'SUCCEEDED', searchId: active.searchId, scopeKey: active.scopeKey, elapsedMs: elapsedSince(active.startedAt), errorMessage: null, cancelUnconfirmed: false });
      options.onSuccess?.(result);
      return result;
    } catch (error) {
      if (active.finished) return undefined;
      if (active.cancelRequested) {
        if (!active.cancelToken) {
          finishExecution(active);
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
      finishExecution(active);
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
