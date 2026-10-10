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
const PAGE_CLAIM_WINDOW_MS = 100;

type StoredWorkspaceSession = { sessionId: string; lastActivityAt: number; pageInstanceId?: string };
type WorkspacePageMessage = {
  type: 'probe' | 'presence';
  sessionId: string;
  pageInstanceId: string;
  documentInstanceId: string;
};

export type WorkspaceSessionPageLease = { sessionId: string; collision: boolean; close: () => void };
type ActivePageLock = {
  documentInstanceId: string;
  consumers: number;
  releaseLock: () => void;
};
const activePageLocks = new Map<string, ActivePageLock>();

function newInstanceId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

const documentInstanceId = newInstanceId();

function retainPageLock(sessionId: string, lock: ActivePageLock): WorkspaceSessionPageLease {
  lock.consumers += 1;
  let closed = false;
  return {
    sessionId,
    collision: false,
    close: () => {
      if (closed) return;
      closed = true;
      lock.consumers -= 1;
      if (lock.consumers === 0) {
        if (activePageLocks.get(sessionId) === lock) activePageLocks.delete(sessionId);
        lock.releaseLock();
      }
    }
  };
}

async function acquireWorkspaceSessionLock(
  sessionId: string,
  currentDocumentInstanceId: string
): Promise<WorkspaceSessionPageLease | null> {
  if (
    typeof navigator === 'undefined'
    || !navigator.locks
    || typeof navigator.locks.request !== 'function'
  ) return null;
  const current = activePageLocks.get(sessionId);
  if (current) {
    return current.documentInstanceId === currentDocumentInstanceId
      ? retainPageLock(sessionId, current)
      : { sessionId, collision: true, close: () => {} };
  }

  return new Promise((resolve) => {
    let settled = false;
    void navigator.locks.request(
      `rain.issue-workspace-session:${sessionId}`,
      { mode: 'exclusive', ifAvailable: true },
      async (lock) => {
        if (!lock) {
          settled = true;
          resolve({ sessionId, collision: true, close: () => {} });
          return;
        }
        let releaseLock!: () => void;
        const holdLock = new Promise<void>((release) => {
          releaseLock = release;
        });
        const activeLock: ActivePageLock = {
          documentInstanceId: currentDocumentInstanceId,
          consumers: 0,
          releaseLock
        };
        activePageLocks.set(sessionId, activeLock);
        settled = true;
        resolve(retainPageLock(sessionId, activeLock));
        await holdLock;
      }
    ).catch(() => {
      if (!settled) {
        settled = true;
        resolve(null);
      }
    });
  });
}

export async function openWorkspaceSessionPageLease(
  sessionId: string,
  pageInstanceId: string,
  detectExisting: boolean,
  currentDocumentInstanceId = documentInstanceId
): Promise<WorkspaceSessionPageLease> {
  const lockedLease = await acquireWorkspaceSessionLock(sessionId, currentDocumentInstanceId);
  if (lockedLease) return lockedLease;

  if (typeof BroadcastChannel === 'undefined') {
    // Without cross-tab coordination, never resume a persisted session.
    return { sessionId, collision: detectExisting, close: () => {} };
  }

  let channel: BroadcastChannel;
  try {
    channel = new BroadcastChannel(`rain.issue-workspace-session:${sessionId}`);
  } catch {
    return { sessionId, collision: detectExisting, close: () => {} };
  }
  let collision = false;
  channel.onmessage = (event: MessageEvent<WorkspacePageMessage>) => {
    const message = event.data;
    if (message?.sessionId !== sessionId || message.documentInstanceId === currentDocumentInstanceId) return;
    if (message.type === 'probe') {
      channel.postMessage({
        type: 'presence',
        sessionId,
        pageInstanceId,
        documentInstanceId: currentDocumentInstanceId
      } satisfies WorkspacePageMessage);
    } else if (message.type === 'presence') {
      collision = true;
    }
  };

  if (detectExisting) {
    channel.postMessage({
      type: 'probe',
      sessionId,
      pageInstanceId,
      documentInstanceId: currentDocumentInstanceId
    } satisfies WorkspacePageMessage);
    await new Promise<void>((resolve) => window.setTimeout(resolve, PAGE_CLAIM_WINDOW_MS));
  }

  return { sessionId, collision, close: () => channel.close() };
}

function readStoredSession(key: string): StoredWorkspaceSession | null {
  try {
    const value = sessionStorage.getItem(key);
    if (!value) return null;
    const parsed = JSON.parse(value) as Partial<StoredWorkspaceSession>;
    return typeof parsed.sessionId === 'string'
      && Number.isFinite(parsed.lastActivityAt)
      ? {
          sessionId: parsed.sessionId,
          lastActivityAt: parsed.lastActivityAt!,
          ...(typeof parsed.pageInstanceId === 'string' ? { pageInstanceId: parsed.pageInstanceId } : {})
        }
      : null;
  } catch {
    return null;
  }
}

function storeSession(key: string, sessionId: string, lastActivityAt: number, pageInstanceId: string): void {
  try {
    sessionStorage.setItem(key, JSON.stringify({ sessionId, lastActivityAt, pageInstanceId }));
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
    const effectGeneration = generationRef.current;
    const storageKey = `rain.issue-workspace:${encodeURIComponent(principalKey)}:${encodeURIComponent(issueCode.toUpperCase())}`;
    const savedSession = readStoredSession(storageKey);
    let pageInstanceId = savedSession?.pageInstanceId ?? newInstanceId();
    let pageLease: WorkspaceSessionPageLease | null = null;
    sessionIdRef.current = null;
    lastActivityRef.current = savedSession?.lastActivityAt ?? Date.now();
    lastStoredActivityRef.current = Date.now();
    lastSentRef.current = 0;
    setReady(false);
    setSessionError(null);
    let preserveOnUnmount = false;
    let restorationPending = Boolean(savedSession);

    const startSession = (sessionToResume?: string) => {
      if (requestRef.current || sessionIdRef.current) return;
      const workspaceRequest = sessionToResume
        ? resumeWorkspaceSession(issueCode, sessionToResume)
        : beginWorkspaceSession(issueCode);
      const requestGeneration = generationRef.current;
      retainWorkspaceSessionRequest(workspaceRequest);
      requestRef.current = workspaceRequest;
      void workspaceRequest.promise.then(async (session) => {
        if (disposedRef.current || generationRef.current !== requestGeneration || requestRef.current !== workspaceRequest) {
          if (preserveOnUnmount) {
            storeSession(storageKey, session.session_id, lastActivityRef.current, pageInstanceId);
          } else if (hasWorkspaceSessionConsumers(workspaceRequest)) {
            return;
          } else {
            clearStoredSession(storageKey, session.session_id);
            clearWorkspaceSessionRequest(workspaceRequest);
            void rainApi.endWorkspaceSession(session.session_id, true).catch(() => {});
          }
          return;
        }
        if (pageLease?.sessionId !== session.session_id) {
          pageLease?.close();
          const nextLease = await openWorkspaceSessionPageLease(
            session.session_id,
            pageInstanceId,
            false
          );
          if (disposedRef.current || generationRef.current !== requestGeneration || requestRef.current !== workspaceRequest) {
            nextLease.close();
            return;
          }
          pageLease = nextLease;
        }
        sessionIdRef.current = session.session_id;
        storeSession(storageKey, session.session_id, lastActivityRef.current, pageInstanceId);
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

    const restoreSession = async () => {
      if (!savedSession) {
        startSession();
        return;
      }
      const lease = await openWorkspaceSessionPageLease(
        savedSession.sessionId,
        pageInstanceId,
        true
      );
      restorationPending = false;
      if (disposedRef.current || generationRef.current !== effectGeneration) {
        lease.close();
        return;
      }
      if (lease.collision) {
        lease.close();
        clearStoredSession(storageKey, savedSession.sessionId);
        pageInstanceId = newInstanceId();
        lastActivityRef.current = Date.now();
        startSession();
        return;
      }
      pageLease = lease;
      startSession(savedSession.sessionId);
    };
    void restoreSession();

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
      pageLease?.close();
      pageLease = null;
      restorationPending = false;
      clearStoredSession(storageKey, sessionId ?? undefined);
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
          pageLease?.close();
          pageLease = null;
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
        storeSession(storageKey, sessionIdRef.current, lastActivityRef.current, pageInstanceId);
        lastStoredActivityRef.current = lastActivityRef.current;
      }
      if (!restorationPending && !sessionIdRef.current && !requestRef.current) startSessionAfterReset();
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
      pageLease?.close();
      pageLease = null;
      if (!preserveOnUnmount) clearStoredSession(storageKey, sessionId ?? undefined);
      // Keep the old session alive while the standalone result page loads and attaches it
      // to its own session, so an already-expired preview cannot disappear in the handoff.
      if (sessionId && preserveOnUnmount) {
        storeSession(storageKey, sessionId, lastActivityRef.current, pageInstanceId);
      }
      if (sessionId && !preserveOnUnmount) {
        if (activeRequest) clearWorkspaceSessionRequest(activeRequest);
        void rainApi.endWorkspaceSession(sessionId, true).catch(() => {});
      }
    };
  }, [issueCode, principalKey]);

  return { ready, error: sessionError, generationRef, sessionIdRef };
}
