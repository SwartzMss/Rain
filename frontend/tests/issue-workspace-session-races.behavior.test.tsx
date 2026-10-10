import { act, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { useIssueWorkspaceSession } from '../src/features/files/hooks/useIssueWorkspaceSession';

const testMocks = vi.hoisted(() => ({
  beginWorkspaceSession: vi.fn(),
  resumeWorkspaceSession: vi.fn(),
  recordWorkspaceActivity: vi.fn(),
  clearWorkspaceSessionRequest: vi.fn()
}));

vi.mock('../src/api/client', () => ({
  beginWorkspaceSession: testMocks.beginWorkspaceSession,
  resumeWorkspaceSession: testMocks.resumeWorkspaceSession,
  retainWorkspaceSessionRequest: vi.fn((request: { consumers: number }) => { request.consumers += 1; }),
  releaseWorkspaceSessionRequest: vi.fn((request: { consumers: number }) => { request.consumers = Math.max(0, request.consumers - 1); }),
  hasWorkspaceSessionConsumers: vi.fn((request: { consumers: number }) => request.consumers > 0),
  clearWorkspaceSessionRequest: testMocks.clearWorkspaceSessionRequest,
  rainApi: {
    recordWorkspaceActivity: testMocks.recordWorkspaceActivity,
    endWorkspaceSession: vi.fn(() => Promise.resolve())
  }
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

function WorkspaceProbe({ issueCode }: { issueCode: string }) {
  const workspace = useIssueWorkspaceSession(issueCode, 'race-test-user', () => {});
  return (
    <>
      <output data-testid="ready">{String(workspace.ready)}</output>
      <output data-testid="session">{workspace.sessionIdRef.current ?? ''}</output>
    </>
  );
}

function resolveSession(sessionId: string, issueCode: string) {
  return {
    session_id: sessionId,
    issue_code: issueCode,
    server_now: '',
    last_activity_at: '',
    expires_at: ''
  };
}

describe('Issue workspace session races', () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
    sessionStorage.clear();
    localStorage.clear();
    vi.restoreAllMocks();
    vi.clearAllMocks();
  });

  it('ignores an expired activity response from the previous Issue session', async () => {
    vi.useFakeTimers();
    vi.stubGlobal('navigator', {});
    vi.stubGlobal('BroadcastChannel', undefined);
    const pointerDownHandlers: EventListener[] = [];
    const originalAddEventListener = window.addEventListener;
    vi.spyOn(window, 'addEventListener').mockImplementation(function (
      type: string,
      listener: EventListenerOrEventListenerObject | null,
      options?: boolean | AddEventListenerOptions
    ) {
      if (type === 'pointerdown' && typeof listener === 'function') pointerDownHandlers.push(listener);
      originalAddEventListener.call(window, type, listener, options);
    });
    const dispatchTrustedPointerDown = () => {
      const event = {
        isTrusted: true,
        type: 'pointerdown',
        preventDefault: vi.fn(),
        stopImmediatePropagation: vi.fn()
      } as unknown as Event;
      pointerDownHandlers[pointerDownHandlers.length - 1]?.(event);
    };

    const sessionA = deferred<ReturnType<typeof resolveSession>>();
    const sessionB = deferred<ReturnType<typeof resolveSession>>();
    const activityA = deferred<void>();
    const activityB = deferred<void>();
    let requestA: { issueCode: string; consumers: number; promise: Promise<ReturnType<typeof resolveSession>> } | undefined;
    let requestB: { issueCode: string; consumers: number; promise: Promise<ReturnType<typeof resolveSession>> } | undefined;
    testMocks.beginWorkspaceSession.mockImplementation((issueCode: string) => {
      const request = {
        issueCode,
        consumers: 0,
        promise: issueCode === 'ISSUE-A' ? sessionA.promise : sessionB.promise
      };
      if (issueCode === 'ISSUE-A') requestA = request;
      else requestB = request;
      return request;
    });
    testMocks.resumeWorkspaceSession.mockImplementation(() => { throw new Error('unexpected resume'); });
    testMocks.recordWorkspaceActivity.mockImplementation((sessionId: string) => (
      sessionId === 'session-a' ? activityA.promise : activityB.promise
    ));

    const view = render(<WorkspaceProbe issueCode="ISSUE-A" />);
    await act(async () => sessionA.resolve(resolveSession('session-a', 'ISSUE-A')));
    expect(screen.getByTestId('session')).toHaveTextContent('session-a');

    await act(async () => {
      vi.advanceTimersByTime(4 * 60 * 1000 + 1);
      dispatchTrustedPointerDown();
      vi.advanceTimersByTime(0);
    });
    expect(testMocks.recordWorkspaceActivity).toHaveBeenCalledWith('session-a');

    view.rerender(<WorkspaceProbe issueCode="ISSUE-B" />);
    await act(async () => sessionB.resolve(resolveSession('session-b', 'ISSUE-B')));
    expect(screen.getByTestId('session')).toHaveTextContent('session-b');

    await act(async () => {
      vi.advanceTimersByTime(4 * 60 * 1000 + 1);
      dispatchTrustedPointerDown();
      vi.advanceTimersByTime(0);
    });
    expect(testMocks.recordWorkspaceActivity).toHaveBeenCalledWith('session-b');

    await act(async () => {
      activityA.reject(Object.assign(new Error('expired'), { code: 'WORKSPACE_SESSION_EXPIRED' }));
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(screen.getByTestId('ready')).toHaveTextContent('true');
    expect(screen.getByTestId('session')).toHaveTextContent('session-b');
    expect(testMocks.clearWorkspaceSessionRequest).not.toHaveBeenCalledWith(requestB);
    expect(requestA).toBeDefined();

    await act(async () => {
      activityA.resolve();
      activityB.resolve();
      await Promise.resolve();
    });
    view.unmount();
  });
});
