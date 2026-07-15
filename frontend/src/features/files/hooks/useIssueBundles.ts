import { useCallback, useEffect, useRef, useState } from 'react';
import { normalizeApiError, rainApi } from '../../../api/client';
import type { IssueBundlesResponse, UploadSummary } from '../../../api/types';
import type { BundleFileState } from '../homeRows';

export function useIssueBundles(currentIssueCode: string, onIssueMissing: () => void) {
  const [bundles, setBundles] = useState<UploadSummary[]>([]);
  const [, setBundlesLoading] = useState(false);
  const [bundlesError, setBundlesError] = useState<string | null>(null);
  const [bundleFiles, setBundleFiles] = useState<Record<string, BundleFileState>>({});
  const selectedIssueRef = useRef(currentIssueCode);
  const bundleRequestIdRef = useRef(0);

  useEffect(() => {
    selectedIssueRef.current = currentIssueCode;
  }, [currentIssueCode]);

  const clearBundles = useCallback(() => {
    setBundles([]);
    setBundleFiles({});
    setBundlesError(null);
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
        setBundleFiles((prev) => {
          const validHashes = new Set(data.log_bundles.map((bundle) => bundle.hash));
          return Object.fromEntries(Object.entries(prev).filter(([hash]) => validHashes.has(hash)));
        });
      } catch (error) {
        if (requestId !== bundleRequestIdRef.current) return;
        const message = normalizeApiError(error);
        if (/not found|404/i.test(message)) {
          clearBundles();
          setBundlesError('Issue 不存在或已被删除');
          if (selectedIssueRef.current === trimmed) {
            onIssueMissing();
          }
          return;
        }
        setBundles([]);
        setBundlesError(message);
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
    setBundleFiles({});
    loadBundles(currentIssueCode).catch(() => undefined);
  }, [clearBundles, currentIssueCode, loadBundles]);

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
    bundlesError,
    clearBundles,
    loadBundleFiles,
    loadBundles,
    setBundlesError
  };
}
