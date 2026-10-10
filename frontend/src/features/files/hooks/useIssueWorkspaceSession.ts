import { useEffect, useRef, useState } from 'react';
import {
  beginWorkspaceSession,
  clearWorkspaceSessionRequest,
  hasWorkspaceSessionConsumers,
  rainApi,
  releaseWorkspaceSessionRequest,
  retainWorkspaceSessionRequest,
  resumeWorkspaceSession,
  type ActiveWorkspaceRequest
} from '../../../api/client';

export const ISSUE_WORKSPACE_IDLE_TIMEOUT_MS = 3 * 60 * 60 * 1000;
const ACTIVITY_SYNC_INTERVAL_MS = 4 * 60 * 1000;
const SESSION_STORAGE_WRITE_INTERVAL_MS = 1000;
const CHECK_INTERVAL_MS = 30_000;

type StoredWorkspaceSession = { sessionId: string; lastActivityAt: number };

function readStoredSession(key: string): StoredWorkspaceSession | null {
  try {
    const value = sessionStorage.getItem(key);
    if (!value) return null;
    const parsed = JSON.parse(value) as Partial<StoredWorkspaceSession>;
    return typeof parsed.sessionId === 'string'
      && Number.isFinite(parsed.lastActivityAt)
      ? { sessionId: parsed.sessionId, lastActivityAt: parsed.lastActivityAt! }
      : null;
  } catch {
    return null;
  }
}

function storeSession(key: string, sessionId: string, lastActivityAt: number): void {
  try {
    sessionStorage.setItem(key, JSON.stringify({ sessionId, lastActivityAt }));
  } catch {
    // The server session remains usable until its normal inactivity deadline.
  }
}

function clearStoredSession(key: string, expectedSessionId?: string): void {
  try {
    if (!expectedSessionId || readStoredSession(key)?.sessionId === expectedSessionId) {
      sessionStorage.removeItem(key);
    }
  } catch {
    // Storage can be disabled by browser privacy settings.
  }
}

export function useIssueWorkspaceSession(
  issueCode: string,
  principalKey: string,
  onReset: () => void
) {
  const [ready, setReady] = useState(false);
  const [sessionError, setSessionError] = useState<string | null>(null);
  const resetRef = useRef(onReset);
  const sessionIdRef = useRef<string | null>(null);
  const requestRef = useRef<ActiveWorkspaceRequest | null>(null);
  const lastActivityRef = useRef(Date.now());
  const lastStoredActivityRef = useRef(0);
  const lastSentRef = useRef(0);
  const generationRef = useRef(0);
  const disposedRef = useRef(false);
  const syncInFlightRef = useRef(false);
  const syncTimerRef = useRef<number | null>(null);
  const blockFollowingClickRef = useRef(false);

  resetRef.current = onReset;

  useEffect(() => {
    if (!issueCode) {
      setReady(false);
      return;
    }

    disposedRef.current = false;
    generationRef.current += 1;
    const storageKey = `rain.issue-workspace:${encodeURIComponent(principalKey)}:${encodeURIComponent(issueCode.toUpperCase())}`;
    const savedSession = readStoredSession(storageKey);
    sessionIdRef.current = null;
    lastActivityRef.current = savedSession?.lastActivityAt ?? Date.now();
    lastStoredActivityRef.current = Date.now();
    lastSentRef.current = 0;
    setReady(false);
    setSessionError(null);
    let preserveOnUnmount = false;

    const startSession = (sessionToResume?: string) => {
      if (requestRef.current || sessionIdRef.current) return;
      const workspaceRequest = sessionToResume
        ? resumeWorkspaceSession(issueCode, sessionToResume)
        : beginWorkspaceSession(issueCode);
      const requestGeneration = generationRef.current;
      retainWorkspaceSessionRequest(workspaceRequest);
      requestRef.current = workspaceRequest;
      void workspaceRequest.promise.then((session) => {
        if (disposedRef.current || generationRef.current !== requestGeneration || requestRef.current !== workspaceRequest) {
          if (preserveOnUnmount) {
            storeSession(storageKey, session.session_id, lastActivityRef.current);
          } else if (hasWorkspaceSessionConsumers(workspaceRequest)) {
            return;
          } else {
            clearStoredSession(storageKey, session.session_id);
            clearWorkspaceSessionRequest(workspaceRequest);
            void rainApi.endWorkspaceSession(session.session_id, true).catch(() => {});
          }
          return;
        }
        sessionIdRef.current = session.session_id;
        storeSession(storageKey, session.session_id, lastActivityRef.current);
        lastStoredActivityRef.current = Date.now();
        lastSentRef.current = sessionToResume
          ? Date.now() - ACTIVITY_SYNC_INTERVAL_MS
          : Date.now();
        setReady(true);
      }).catch(() => {
        if (disposedRef.current || generationRef.current !== requestGeneration || requestRef.current !== workspaceRequest) return;
        releaseWorkspaceSessionRequest(workspaceRequest);
        clearWorkspaceSessionRequest(workspaceRequest);
        requestRef.current = null;
        if (sessionToResume) {
          clearStoredSession(storageKey, sessionToResume);
          startSession();
          return;
        }
        setReady(false);
        setSessionError('工作会话连接失败，请操作页面后重试');
      });
    };

    startSession(savedSession?.sessionId);

    const endCurrent = (keepalive = false) => {
      generationRef.current += 1;
      const activeRequest = requestRef.current;
      const sessionId = sessionIdRef.current;
      requestRef.current = null;
      sessionIdRef.current = null;
      if (activeRequest) {
        releaseWorkspaceSessionRequest(activeRequest);
        clearWorkspaceSessionRequest(activeRequest);
      }
      clearStoredSession(storageKey);
      if (sessionId) void rainApi.endWorkspaceSession(sessionId, keepalive).catch(() => {});
      setReady(false);
    };

    const resetIfIdle = () => {
      if (Date.now() - lastActivityRef.current < ISSUE_WORKSPACE_IDLE_TIMEOUT_MS) return false;
      endCurrent();
      lastActivityRef.current = Date.now();
      resetRef.current();
      return true;
    };

    const startSessionAfterReset = () => {
      if (requestRef.current || sessionIdRef.current) return;
      setSessionError(null);
      startSession();
    };

    const sendActivity = () => {
      const sessionId = sessionIdRef.current;
      if (!sessionId || syncInFlightRef.current || disposedRef.current) return;
      syncInFlightRef.current = true;
      lastSentRef.current = Date.now();
      void rainApi.recordWorkspaceActivity(sessionId).catch((error: unknown) => {
        const code = typeof error === 'object' && error !== null && 'code' in error
          ? (error as { code?: unknown }).code
          : undefined;
        if (code === 'WORKSPACE_SESSION_EXPIRED') {
          clearStoredSession(storageKey, sessionId);
          sessionIdRef.current = null;
          const activeRequest = requestRef.current;
          requestRef.current = null;
          if (activeRequest) {
            releaseWorkspaceSessionRequest(activeRequest);
            clearWorkspaceSessionRequest(activeRequest);
          }
          setReady(false);
          setSessionError('工作会话已结束，请再次操作页面以继续');
        }
      }).finally(() => {
        syncInFlightRef.current = false;
      });
    };

    const queueActivitySync = () => {
      if (syncTimerRef.current !== null) window.clearTimeout(syncTimerRef.current);
      const delay = Math.max(0, ACTIVITY_SYNC_INTERVAL_MS - (Date.now() - lastSentRef.current));
      syncTimerRef.current = window.setTimeout(() => {
        syncTimerRef.current = null;
        if (sessionIdRef.current && Date.now() - lastActivityRef.current < ISSUE_WORKSPACE_IDLE_TIMEOUT_MS) {
          sendActivity();
        }
      }, delay);
    };

    const onUserActivity = (event: Event) => {
      if (!event.isTrusted) return;
      if (event.type === 'click' && blockFollowingClickRef.current) {
        blockFollowingClickRef.current = false;
        event.preventDefault();
        event.stopImmediatePropagation();
        return;
      }
      if (resetIfIdle()) {
        if (event.type === 'pointerdown') blockFollowingClickRef.current = true;
        event.preventDefault();
        event.stopImmediatePropagation();
        return;
      }
      lastActivityRef.current = Date.now();
      if (
        sessionIdRef.current
        && lastActivityRef.current - lastStoredActivityRef.current >= SESSION_STORAGE_WRITE_INTERVAL_MS
      ) {
        storeSession(storageKey, sessionIdRef.current, lastActivityRef.current);
        lastStoredActivityRef.current = lastActivityRef.current;
      }
      if (!sessionIdRef.current && !requestRef.current) startSessionAfterReset();
      queueActivitySync();
    };

    const checkOnResume = () => {
      if (document.visibilityState === 'hidden') return;
      if (!resetIfIdle() && sessionIdRef.current && Date.now() - lastActivityRef.current < ISSUE_WORKSPACE_IDLE_TIMEOUT_MS) {
        queueActivitySync();
      }
    };

    const activityEvents = ['pointerdown', 'keydown', 'wheel', 'touchstart', 'click'] as const;
    for (const eventName of activityEvents) window.addEventListener(eventName, onUserActivity, true);
    window.addEventListener('focus', checkOnResume);
    window.addEventListener('pageshow', checkOnResume);
    document.addEventListener('visibilitychange', checkOnResume);
    const interval = window.setInterval(resetIfIdle, CHECK_INTERVAL_MS);
    checkOnResume();

    return () => {
      disposedRef.current = true;
      window.clearInterval(interval);
      if (syncTimerRef.current !== null) window.clearTimeout(syncTimerRef.current);
      syncTimerRef.current = null;
      for (const eventName of activityEvents) window.removeEventListener(eventName, onUserActivity, true);
      window.removeEventListener('focus', checkOnResume);
      window.removeEventListener('pageshow', checkOnResume);
      document.removeEventListener('visibilitychange', checkOnResume);
      const sessionId = sessionIdRef.current;
      const activeRequest = requestRef.current;
      requestRef.current = null;
      sessionIdRef.current = null;
      preserveOnUnmount = window.location.pathname.startsWith('/temp-results/');
      if (activeRequest) releaseWorkspaceSessionRequest(activeRequest, preserveOnUnmount);
      if (!preserveOnUnmount) clearStoredSession(storageKey);
      // Keep the old session alive while the standalone result page loads and attaches it
      // to its own session, so an already-expired preview cannot disappear in the handoff.
      if (sessionId && preserveOnUnmount) {
        storeSession(storageKey, sessionId, lastActivityRef.current);
      }
      if (sessionId && !preserveOnUnmount) {
        if (activeRequest) clearWorkspaceSessionRequest(activeRequest);
        void rainApi.endWorkspaceSession(sessionId, true).catch(() => {});
      }
    };
  }, [issueCode, principalKey]);

  return { ready, error: sessionError, generationRef, sessionIdRef };
}
