import { useCallback, useEffect, useRef, useState } from 'react';
import { ApiError, normalizeApiError, rainApi } from '../../../api/client';
import type { IssueBundlesResponse, IssueInactivityExpiry, UploadSummary } from '../../../api/types';
import type { BundleFileState } from '../homeRows';
import { visibleInactivityExpiry } from '../issueExpiration';

export function useIssueBundles(currentIssueCode: string, onIssueMissing: () => void) {
  const [bundles, setBundles] = useState<UploadSummary[]>([]);
  const [canWrite, setCanWrite] = useState(false);
  const [ownerUsername, setOwnerUsername] = useState<string | null>(null);
  const [inactivityExpiry, setInactivityExpiry] = useState<IssueInactivityExpiry | null>(null);
  const [, setBundlesLoading] = useState(false);
  const [bundlesError, setBundlesError] = useState<string | null>(null);
  const [refreshRetryPending, setRefreshRetryPending] = useState(false);
  const [bundleFiles, setBundleFiles] = useState<Record<string, BundleFileState>>({});
  const selectedIssueRef = useRef(currentIssueCode);
  const bundleRequestIdRef = useRef(0);

  useEffect(() => {
    selectedIssueRef.current = currentIssueCode;
  }, [currentIssueCode]);

  const clearBundles = useCallback(() => {
    bundleRequestIdRef.current += 1;
    setBundles([]);
    setBundlesLoading(false);
    setCanWrite(false);
    setOwnerUsername(null);
    setInactivityExpiry(null);
    setBundleFiles({});
    setBundlesError(null);
    setRefreshRetryPending(false);
  }, []);

  const loadBundles = useCallback(
    async (code: string) => {
      const trimmed = code.trim();
      const requestId = ++bundleRequestIdRef.current;
      if (!trimmed) {
        clearBundles();
        setBundlesLoading(false);
        return;
      }

      setBundlesLoading(true);
      setBundlesError(null);
      try {
        const data: IssueBundlesResponse = await rainApi.fetchIssueBundles(trimmed);
        if (requestId !== bundleRequestIdRef.current || selectedIssueRef.current !== trimmed) {
          return;
        }
        setBundles(data.log_bundles);
        setRefreshRetryPending(false);
        setCanWrite(data.can_write);
        setOwnerUsername(data.owner_username);
        setInactivityExpiry(visibleInactivityExpiry(data.inactivity_expiry));
        setBundleFiles((prev) => {
          const validHashes = new Set(data.log_bundles.map((bundle) => bundle.hash));
          return Object.fromEntries(Object.entries(prev).filter(([hash]) => validHashes.has(hash)));
        });
      } catch (error) {
        if (requestId !== bundleRequestIdRef.current || selectedIssueRef.current !== trimmed) {
          return;
        }
        const message = normalizeApiError(error);
        if (error instanceof ApiError && error.code === 'RESOURCE_NOT_FOUND') {
          clearBundles();
          setBundlesError('Issue 不存在或已被删除');
          if (selectedIssueRef.current === trimmed) {
            onIssueMissing();
          }
          return;
        }
        setInactivityExpiry(null);
        setBundlesError(message);
        setRefreshRetryPending(true);
      } finally {
        if (requestId === bundleRequestIdRef.current) {
          setBundlesLoading(false);
        }
      }
    },
    [clearBundles, onIssueMissing]
  );

  const loadBundleFiles = useCallback(async (hash: string) => {
    setBundleFiles((prev) => ({
      ...prev,
      [hash]: {
        files: prev[hash]?.files ?? [],
        loading: true,
        loaded: prev[hash]?.loaded ?? false,
        error: null
      }
    }));

    try {
      const response = await rainApi.fetchFileNode(hash, 'root');
      const files = (response.children ?? []).filter((child) => child.meta?.kind === 'uploaded_file');
      setBundleFiles((prev) => ({
        ...prev,
        [hash]: { files, loading: false, loaded: true, error: null }
      }));
    } catch (error) {
      setBundleFiles((prev) => ({
        ...prev,
        [hash]: {
          files: prev[hash]?.files ?? [],
          loading: false,
          loaded: true,
          error: normalizeApiError(error)
        }
      }));
    }
  }, []);

  useEffect(() => {
    if (!currentIssueCode) {
      clearBundles();
      return;
    }
    clearBundles();
    loadBundles(currentIssueCode).catch(() => undefined);
  }, [clearBundles, currentIssueCode, loadBundles]);

  const hasActiveBundles = bundles.some((bundle) => {
    const status = bundle.status.upload_status;
    return status === 'PENDING' || status === 'PROCESSING';
  });
  const shouldPollBundles = hasActiveBundles || refreshRetryPending;

  useEffect(() => {
    if (!currentIssueCode || !shouldPollBundles) return;

    let cancelled = false;
    let polling = false;
    let refreshOnVisible = false;
    let timer: number | undefined;

    const clearTimer = () => {
      if (timer === undefined) return;
      window.clearTimeout(timer);
      timer = undefined;
    };

    const schedulePoll = () => {
      clearTimer();
      if (cancelled || document.hidden) return;
      timer = window.setTimeout(() => {
        timer = undefined;
        poll().catch(() => undefined);
      }, 3000);
    };

    const poll = async () => {
      if (cancelled || polling || document.hidden) return;
      polling = true;
      try {
        await loadBundles(currentIssueCode);
      } finally {
        polling = false;
        if (cancelled) return;
        if (refreshOnVisible && !document.hidden) {
          refreshOnVisible = false;
          poll().catch(() => undefined);
          return;
        }
        schedulePoll();
      }
    };

    const handleVisibilityChange = () => {
      if (document.hidden) {
        clearTimer();
        return;
      }
      clearTimer();
      if (polling) {
        refreshOnVisible = true;
        return;
      }
      poll().catch(() => undefined);
    };

    document.addEventListener('visibilitychange', handleVisibilityChange);
    schedulePoll();

    return () => {
      cancelled = true;
      clearTimer();
      document.removeEventListener('visibilitychange', handleVisibilityChange);
    };
  }, [currentIssueCode, loadBundles, shouldPollBundles]);

  useEffect(() => {
    for (const bundle of bundles) {
      if (bundle.status.upload_status !== 'READY') continue;
      const state = bundleFiles[bundle.hash];
      if (!state?.loaded && !state?.loading) {
        loadBundleFiles(bundle.hash).catch(() => undefined);
      }
    }
  }, [bundleFiles, bundles, loadBundleFiles]);

  return {
    bundleFiles,
    bundles,
    canWrite,
    inactivityExpiry,
    ownerUsername,
    bundlesError,
    clearBundles,
    loadBundleFiles,
    loadBundles,
    setBundlesError
  };
}
