import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { normalizeApiError, rainApi } from '../../../api/client';
import type { TempResultPreviewResponse } from '../../../api/types';
import { useSearchExecution, type SearchExecutionSnapshot } from '../../../hooks/useSearchExecution';
import { finalizeSearchTokens, serializeSearchTokens, type SearchToken } from '../searchTokens';

export type SearchControllerStatus =
  | 'IDLE'
  | 'VALIDATING'
  | 'RUNNING'
  | 'CANCELLING'
  | 'CANCELLED'
  | 'SUCCEEDED'
  | 'FAILED';

export type SearchControllerRunOptions = {
  expression: string;
  payload: Parameters<typeof rainApi.previewTempResult>[0];
  scopeKey: string;
  onSuccess?: (response: TempResultPreviewResponse) => void;
  onFailure?: (error: unknown) => void;
};

export type SearchController = {
  tokens: SearchToken[];
  draft: string;
  error: string | null;
  executed: boolean;
  status: SearchControllerStatus;
  busy: boolean;
  snapshot: SearchExecutionSnapshot;
  setTokens: (tokens: SearchToken[]) => void;
  setDraft: (draft: string) => void;
  run: (options: SearchControllerRunOptions) => Promise<TempResultPreviewResponse | undefined>;
  cancel: () => Promise<void>;
  clear: () => void;
  invalidate: () => void;
  setEditor: (tokens: SearchToken[], draft?: string) => void;
  setError: (message: string | null) => void;
};

type SearchControllerOptions = {
  initialTokens?: SearchToken[];
  initialError?: string | null;
};

export function useSearchController(options: SearchControllerOptions = {}): SearchController {
  const [tokens, setTokensState] = useState<SearchToken[]>(options.initialTokens ?? []);
  const [draft, setDraftState] = useState('');
  const [validationError, setValidationError] = useState<string | null>(options.initialError ?? null);
  const [executed, setExecuted] = useState(false);
  const [phase, setPhase] = useState<'IDLE' | 'VALIDATING' | 'RUNNING'>('IDLE');
  const intentRef = useRef(0);
  const mountedRef = useRef(true);
  const validationAbortRef = useRef<AbortController | null>(null);
  const { snapshot, execute, cancel: cancelExecution } = useSearchExecution();

  useEffect(() => () => {
    mountedRef.current = false;
    intentRef.current += 1;
    validationAbortRef.current?.abort();
  }, []);

  useEffect(() => {
    if (phase !== 'RUNNING') return;
    if (snapshot.status === 'SUCCEEDED' || snapshot.status === 'FAILED' || snapshot.status === 'CANCELLED') {
      setPhase('IDLE');
    }
  }, [phase, snapshot.status]);

  const invalidate = useCallback(() => {
    intentRef.current += 1;
    setPhase('IDLE');
    validationAbortRef.current?.abort();
    validationAbortRef.current = null;
    void cancelExecution();
  }, [cancelExecution]);

  const setTokens = useCallback((next: SearchToken[]) => {
    invalidate();
    setTokensState(next);
    setValidationError(null);
  }, [invalidate]);

  const setDraft = useCallback((next: string) => {
    invalidate();
    setDraftState(next);
    setValidationError(null);
  }, [invalidate]);

  const setEditor = useCallback((nextTokens: SearchToken[], nextDraft = '') => {
    invalidate();
    setTokensState(nextTokens);
    setDraftState(nextDraft);
    setValidationError(null);
  }, [invalidate]);

  const setError = useCallback((message: string | null) => {
    setValidationError(message);
  }, []);

  const clear = useCallback(() => {
    invalidate();
    setTokensState([]);
    setDraftState('');
    setValidationError(null);
    setExecuted(false);
  }, [invalidate]);

  const cancel = useCallback(async () => {
    invalidate();
    await cancelExecution();
  }, [cancelExecution, invalidate]);

  const run = useCallback(async ({ expression, payload, scopeKey, onSuccess, onFailure }: SearchControllerRunOptions) => {
    const intent = ++intentRef.current;
    validationAbortRef.current?.abort();
    const validationController = new AbortController();
    validationAbortRef.current = validationController;
    setPhase('VALIDATING');
    setValidationError(null);
    setExecuted(true);
    try {
      await rainApi.validateSearchExpression(expression);
    } catch (error) {
      if (mountedRef.current && intent === intentRef.current && !validationController.signal.aborted) {
        setValidationError(normalizeApiError(error));
      }
      return undefined;
    } finally {
      if (validationAbortRef.current === validationController) validationAbortRef.current = null;
    }
    if (!mountedRef.current || intent !== intentRef.current || validationController.signal.aborted) return undefined;
    setPhase('RUNNING');
    const result = await execute(payload, {
      scopeKey,
      onFailure,
      onSuccess: (response) => {
        if (!mountedRef.current || intent !== intentRef.current) return;
        onSuccess?.(response);
      }
    });
    return mountedRef.current && intent === intentRef.current ? result : undefined;
  }, [execute]);

  const status: SearchControllerStatus = phase === 'VALIDATING'
    ? 'VALIDATING'
    : phase === 'RUNNING' && snapshot.status === 'IDLE'
      ? 'RUNNING'
    : validationError
    ? 'FAILED'
    : snapshot.status;
  const busy = snapshot.status === 'RUNNING'
    || snapshot.status === 'CANCELLING'
    || phase === 'VALIDATING'
    || phase === 'RUNNING';
  const error = validationError ?? (phase === 'VALIDATING' ? null : snapshot.errorMessage);

  return useMemo(() => ({
    tokens,
    draft,
    error,
    executed,
    status,
    busy,
    snapshot,
    setTokens,
    setDraft,
    run,
    cancel,
    clear,
    invalidate,
    setEditor,
    setError
  }), [busy, cancel, clear, draft, error, executed, invalidate, run, setDraft, setEditor, setError, setTokens, snapshot, status, tokens]);
}

export function currentExpression(tokens: SearchToken[], draft: string, allowOperators = true): { expression: string; tokens: SearchToken[] } {
  const finalizedTokens = finalizeSearchTokens(tokens, draft, allowOperators);
  return { expression: serializeSearchTokens(finalizedTokens), tokens: finalizedTokens };
}
