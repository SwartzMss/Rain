import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  beginWorkspaceSession,
  clearWorkspaceSessionRequest,
  releaseWorkspaceSessionRequest,
  retainWorkspaceSessionRequest,
  RequestCancelledError,
  rainApi
} from '../src/api/client';
import { SearchExecutionStatus } from '../src/components/SearchExecutionStatus';
import { useSearchExecution } from '../src/hooks/useSearchExecution';
import type { TempResultPreviewResponse } from '../src/api/types';
import { useEffect, useRef, useState } from 'react';

function previewResponse(resultId: string): TempResultPreviewResponse {
  return { result_id: resultId, total: 1, next_start: null, lines: [{ line_number: 0, content: 'ERROR', path: 'app.log' }] };
}

function ExecutionProbe() {
  const { snapshot, execute, cancel } = useSearchExecution();
  const started = useRef(false);
  useEffect(() => {
    if (started.current) return;
    started.current = true;
    void execute({ expression: 'ERROR', issue_code: 'ISSUE' }, { scopeKey: 'issue:ISSUE', workspaceSessionId: null });
  }, [execute]);
  return (
    <>
      <output data-testid="status">{snapshot.status}</output>
      <button type="button" onClick={() => { void cancel(); }}>cancel</button>
    </>
  );
}

function WorkspaceExecutionProbe() {
  const { execute } = useSearchExecution();
  const started = useRef(false);
  useEffect(() => {
    if (started.current) return;
    started.current = true;
    void execute(
      { expression: 'ERROR', issue_code: 'ISSUE-A' },
      { scopeKey: 'issue:ISSUE-A', workspaceSessionId: 'session-a' }
    );
  }, [execute]);
  return null;
}

describe('interactive search execution', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it('keeps abort errors distinct from ordinary API errors', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new DOMException('aborted', 'AbortError')));
    await expect(rainApi.reserveSearchRequest('00000000-0000-4000-8000-000000000001'))
      .rejects.toBeInstanceOf(RequestCancelledError);
  });

  it('uses a valid UUID fallback when randomUUID is unavailable', async () => {
    let requestBody: { search_id?: string } | undefined;
    vi.stubGlobal('crypto', {});
    const fetchMock = vi.fn().mockImplementation((url: string, init: RequestInit = {}) => {
      if (url.endsWith('/api/search-requests') && init.method === 'POST') {
        requestBody = JSON.parse(String(init.body)) as { search_id?: string };
        return Promise.resolve(new Response(JSON.stringify({
          search_id: '00000000-0000-4000-8000-000000000001',
          cancel_token: 'token',
          expires_in_ms: 60_000
        }), { status: 201 }));
      }
      if (url.endsWith('/api/temp-results/preview')) {
        return Promise.resolve(new Response(JSON.stringify(previewResponse('result')), { status: 200 }));
      }
      throw new Error(`unexpected request ${url}`);
    });
    vi.stubGlobal('fetch', fetchMock);

    render(<ExecutionProbe />);
    await waitFor(() => expect(requestBody?.search_id).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/
    ));
  });

  it('keeps the workspace session captured by a search when another Issue session is active', async () => {
    const previewHeaders: Headers[] = [];
    const fetchMock = vi.fn().mockImplementation((url: string, init: RequestInit = {}) => {
      if (url.endsWith('/api/issues/ISSUE-B/workspace-sessions')) {
        return Promise.resolve(new Response(JSON.stringify({
          session_id: 'session-b',
          issue_code: 'ISSUE-B',
          server_now: '',
          last_activity_at: '',
          expires_at: ''
        }), { status: 201 }));
      }
      if (url.endsWith('/api/search-requests') && init.method === 'POST') {
        const body = JSON.parse(String(init.body)) as { search_id: string };
        return Promise.resolve(new Response(JSON.stringify({
          search_id: body.search_id,
          cancel_token: 'token',
          expires_in_ms: 60_000
        }), { status: 201 }));
      }
      if (url.endsWith('/api/temp-results/preview')) {
        previewHeaders.push(new Headers(init.headers));
        return Promise.resolve(new Response(JSON.stringify(previewResponse('result-a')), { status: 200 }));
      }
      throw new Error(`unexpected request ${url}`);
    });
    vi.stubGlobal('fetch', fetchMock);
    const issueBRequest = beginWorkspaceSession('ISSUE-B', 'test-user');
    await issueBRequest.promise;

    render(<WorkspaceExecutionProbe />);
    await waitFor(() => expect(previewHeaders).toHaveLength(1));

    expect(previewHeaders[0].get('X-Issue-Workspace-Session')).toBe('session-a');
    clearWorkspaceSessionRequest(issueBRequest);
  });

  it('does not reuse a handed-off workspace request for a different principal', async () => {
    const createdSessionIds: string[] = [];
    const fetchMock = vi.fn().mockImplementation((url: string) => {
      if (url.endsWith('/api/issues/ISSUE-HANDOFF/workspace-sessions')) {
        const sessionId = createdSessionIds.length === 0 ? 'user-session' : 'guest-session';
        createdSessionIds.push(sessionId);
        return Promise.resolve(new Response(JSON.stringify({
          session_id: sessionId,
          issue_code: 'ISSUE-HANDOFF',
          server_now: '',
          last_activity_at: '',
          expires_at: ''
        }), { status: 201 }));
      }
      throw new Error(`unexpected request ${url}`);
    });
    vi.stubGlobal('fetch', fetchMock);

    const userRequest = beginWorkspaceSession('ISSUE-HANDOFF', 'user-1');
    retainWorkspaceSessionRequest(userRequest);
    releaseWorkspaceSessionRequest(userRequest, true);
    const samePrincipalHandoff = beginWorkspaceSession('ISSUE-HANDOFF', 'user-1');
    const guestRequest = beginWorkspaceSession('ISSUE-HANDOFF', 'guest');

    expect(samePrincipalHandoff).toBe(userRequest);
    expect(guestRequest).not.toBe(userRequest);
    await Promise.all([userRequest.promise, guestRequest.promise]);
    expect(createdSessionIds).toEqual(['user-session', 'guest-session']);
    expect(fetchMock).toHaveBeenCalledTimes(2);

    clearWorkspaceSessionRequest(userRequest);
    clearWorkspaceSessionRequest(guestRequest);
  });

  it('cancels the preview with an independent DELETE request', async () => {
    let previewSignal: AbortSignal | undefined;
    const fetchMock = vi.fn().mockImplementation((url: string, init: RequestInit = {}) => {
      if (url.endsWith('/api/search-requests') && init.method === 'POST') {
        return Promise.resolve(new Response(JSON.stringify({
          search_id: '00000000-0000-4000-8000-000000000001',
          cancel_token: 'token',
          expires_in_ms: 60_000
        }), { status: 201 }));
      }
      if (url.endsWith('/api/temp-results/preview')) {
        previewSignal = init.signal ?? undefined;
        return new Promise((_resolve, reject) => {
          init.signal?.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')));
        });
      }
      if (url.includes('/api/search-requests/') && init.method === 'DELETE') {
        return Promise.resolve(new Response(JSON.stringify({ status: 'cancelled' }), { status: 200 }));
      }
      throw new Error(`unexpected request ${url}`);
    });
    vi.stubGlobal('fetch', fetchMock);

    render(<ExecutionProbe />);
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
    fireEvent.click(screen.getByRole('button', { name: 'cancel' }));
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('CANCELLED'));
    expect(previewSignal?.aborted).toBe(true);
    expect(fetchMock).toHaveBeenCalledWith(expect.stringContaining('/api/search-requests/'), expect.objectContaining({ method: 'DELETE' }));
  });

  it('settles cancellation after the user cancels before reservation returns', async () => {
    let resolveReservation!: (response: Response) => void;
    const reservation = new Promise<Response>((resolve) => {
      resolveReservation = resolve;
    });
    const fetchMock = vi.fn().mockImplementation((url: string, init: RequestInit = {}) => {
      if (url.endsWith('/api/search-requests') && init.method === 'POST') {
        return reservation;
      }
      if (url.includes('/api/search-requests/') && init.method === 'DELETE') {
        return Promise.resolve(new Response(JSON.stringify({ status: 'cancelled' }), { status: 200 }));
      }
      throw new Error(`unexpected request ${url}`);
    });
    vi.stubGlobal('fetch', fetchMock);

    render(<ExecutionProbe />);
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));
    fireEvent.click(screen.getByRole('button', { name: 'cancel' }));
    expect(screen.getByTestId('status')).toHaveTextContent('CANCELLING');

    await act(async () => {
      resolveReservation(new Response(JSON.stringify({
        search_id: '00000000-0000-4000-8000-000000000001',
        cancel_token: 'token',
        expires_in_ms: 60_000
      }), { status: 201 }));
    });

    await waitFor(() => expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining('/api/search-requests/'),
      expect.objectContaining({ method: 'DELETE' })
    ));
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('CANCELLED'));
  });

  it('does not let an aborted older execution overwrite a newer one', async () => {
    let previewCalls = 0;
    let resolveSecond!: (response: Response) => void;
    const fetchMock = vi.fn().mockImplementation((url: string, init: RequestInit = {}) => {
      if (url.endsWith('/api/search-requests') && init.method === 'POST') {
        return Promise.resolve(new Response(JSON.stringify({ search_id: 'id', cancel_token: 'token', expires_in_ms: 60_000 }), { status: 201 }));
      }
      if (url.endsWith('/api/temp-results/preview')) {
        previewCalls += 1;
        if (previewCalls === 1) {
          return new Promise((_resolve, reject) => init.signal?.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError'))));
        }
        return new Promise((resolve) => { resolveSecond = resolve; });
      }
      if (url.includes('/api/search-requests/') && init.method === 'DELETE') {
        return Promise.resolve(new Response(JSON.stringify({ status: 'cancelled' }), { status: 200 }));
      }
      throw new Error(`unexpected request ${url}`);
    });
    vi.stubGlobal('fetch', fetchMock);

    function ReplaceProbe() {
      const { snapshot, execute } = useSearchExecution();
      const [loading, setLoading] = useState(false);
      const run = () => {
        setLoading(true);
        void execute({ expression: 'WARN', issue_code: 'ISSUE' }, { scopeKey: 'issue:ISSUE', workspaceSessionId: null });
      };
      useEffect(() => {
        if (snapshot.status === 'CANCELLED' || snapshot.status === 'FAILED' || snapshot.status === 'SUCCEEDED') {
          setLoading(false);
        }
      }, [snapshot.status]);
      return (
        <>
          <output data-testid="status">{snapshot.status}</output>
          <output data-testid="loading">{String(loading)}</output>
          <button type="button" onClick={run}>new</button>
        </>
      );
    }

    render(<ReplaceProbe />);
    fireEvent.click(screen.getByRole('button', { name: 'new' }));
    await waitFor(() => expect(previewCalls).toBe(1));
    fireEvent.click(screen.getByRole('button', { name: 'new' }));
    await waitFor(() => expect(previewCalls).toBe(2));
    expect(screen.getByTestId('loading')).toHaveTextContent('true');
    await act(async () => resolveSecond(new Response(JSON.stringify(previewResponse('new')), { status: 200 })));
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('SUCCEEDED'));
  });

  it('serializes overlapping replacements so each newer execution cancels the one before it', async () => {
    let reservationCalls = 0;
    const reservationSearchIds: string[] = [];
    const previewSearchIds: string[] = [];
    const deleteSearchIds: string[] = [];
    let resolveLatestPreview!: (response: Response) => void;
    const fetchMock = vi.fn().mockImplementation((url: string, init: RequestInit = {}) => {
      if (url.endsWith('/api/search-requests') && init.method === 'POST') {
        const searchId = (JSON.parse(String(init.body)) as { search_id: string }).search_id;
        reservationSearchIds.push(searchId);
        reservationCalls += 1;
        return Promise.resolve(new Response(JSON.stringify({ search_id: searchId, cancel_token: `token-${reservationCalls}`, expires_in_ms: 60_000 }), { status: 201 }));
      }
      if (url.endsWith('/api/temp-results/preview')) {
        const body = JSON.parse(String(init.body)) as { search_id?: string };
        previewSearchIds.push(body.search_id ?? '');
        if (body.search_id === reservationSearchIds[1]) {
          return new Promise((resolve) => { resolveLatestPreview = resolve; });
        }
        return new Promise((_resolve, reject) => {
          init.signal?.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')));
        });
      }
      if (url.includes('/api/search-requests/') && init.method === 'DELETE') {
        deleteSearchIds.push(url.split('/').pop() ?? '');
        return Promise.resolve(new Response(JSON.stringify({ status: 'cancelled' }), { status: 200 }));
      }
      throw new Error(`unexpected request ${url}`);
    });
    vi.stubGlobal('fetch', fetchMock);

    function BurstProbe() {
      const { snapshot, execute } = useSearchExecution();
      const run = () => {
        void execute({ expression: 'ERROR', issue_code: 'ISSUE' }, { scopeKey: 'issue:ISSUE', workspaceSessionId: null });
      };
      return (
        <>
          <output data-testid="status">{snapshot.status}</output>
          <button type="button" onClick={run}>new</button>
        </>
      );
    }

    render(<BurstProbe />);
    fireEvent.click(screen.getByRole('button', { name: 'new' }));
    await waitFor(() => expect(previewSearchIds).toHaveLength(1));
    fireEvent.click(screen.getByRole('button', { name: 'new' }));
    fireEvent.click(screen.getByRole('button', { name: 'new' }));

    await waitFor(() => expect(reservationCalls).toBe(2));
    await waitFor(() => expect(deleteSearchIds).toContain(reservationSearchIds[0]));
    await waitFor(() => expect(previewSearchIds).toContain(reservationSearchIds[1]));
    await act(async () => resolveLatestPreview(new Response(JSON.stringify(previewResponse('latest')), { status: 200 })));
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('SUCCEEDED'));
  });

  it('waits for the previous cancellation before reserving a replacement execution', async () => {
    let resolveFirstReservation!: (response: Response) => void;
    let resolveCancellation!: (response: Response) => void;
    let resolveSecondPreview!: (response: Response) => void;
    let reservationCalls = 0;
    let deleteCalls = 0;
    let previewCalls = 0;
    const firstReservation = new Promise<Response>((resolve) => {
      resolveFirstReservation = resolve;
    });
    const cancellation = new Promise<Response>((resolve) => {
      resolveCancellation = resolve;
    });
    const fetchMock = vi.fn().mockImplementation((url: string, init: RequestInit = {}) => {
      if (url.endsWith('/api/search-requests') && init.method === 'POST') {
        reservationCalls += 1;
        if (reservationCalls === 1) return firstReservation;
        return Promise.resolve(new Response(JSON.stringify({
          search_id: '00000000-0000-4000-8000-000000000002',
          cancel_token: 'token-b',
          expires_in_ms: 60_000
        }), { status: 201 }));
      }
      if (url.includes('/api/search-requests/') && init.method === 'DELETE') {
        deleteCalls += 1;
        return cancellation;
      }
      if (url.endsWith('/api/temp-results/preview')) {
        previewCalls += 1;
        return new Promise((resolve) => { resolveSecondPreview = resolve; });
      }
      throw new Error(`unexpected request ${url}`);
    });
    vi.stubGlobal('fetch', fetchMock);

    function ReplaceProbe() {
      const { snapshot, execute } = useSearchExecution();
      return (
        <>
          <output data-testid="status">{snapshot.status}</output>
          <button type="button" onClick={() => { void execute({ expression: 'ERROR', issue_code: 'ISSUE' }, { scopeKey: 'issue:ISSUE', workspaceSessionId: null }); }}>new</button>
        </>
      );
    }

    render(<ReplaceProbe />);
    fireEvent.click(screen.getByRole('button', { name: 'new' }));
    await waitFor(() => expect(reservationCalls).toBe(1));
    fireEvent.click(screen.getByRole('button', { name: 'new' }));
    await act(async () => {
      await new Promise((resolve) => window.setTimeout(resolve, 0));
    });
    expect(deleteCalls).toBe(0);
    expect(reservationCalls).toBe(1);

    await act(async () => {
      resolveFirstReservation(new Response(JSON.stringify({
        search_id: '00000000-0000-4000-8000-000000000001',
        cancel_token: 'token-a',
        expires_in_ms: 60_000
      }), { status: 201 }));
    });
    await waitFor(() => expect(deleteCalls).toBe(1));
    expect(reservationCalls).toBe(1);

    await act(async () => {
      resolveCancellation(new Response(JSON.stringify({ status: 'cancelled' }), { status: 200 }));
    });
    await waitFor(() => expect(reservationCalls).toBe(2));
    await waitFor(() => expect(previewCalls).toBe(1));
    await act(async () => resolveSecondPreview(new Response(JSON.stringify(previewResponse('replacement')), { status: 200 })));
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('SUCCEEDED'));
  });

  it('settles a replacement request when cancellation fails before confirmation', async () => {
    let previewCalls = 0;
    let deleteCalls = 0;
    let replacementDone = false;
    const fetchMock = vi.fn().mockImplementation((url: string, init: RequestInit = {}) => {
      if (url.endsWith('/api/search-requests') && init.method === 'POST') {
        return Promise.resolve(new Response(JSON.stringify({
          search_id: previewCalls === 0
            ? '00000000-0000-4000-8000-000000000001'
            : '00000000-0000-4000-8000-000000000002',
          cancel_token: 'token',
          expires_in_ms: 60_000
        }), { status: 201 }));
      }
      if (url.endsWith('/api/temp-results/preview')) {
        previewCalls += 1;
        return new Promise((_resolve, reject) => {
          init.signal?.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')));
        });
      }
      if (url.includes('/api/search-requests/') && init.method === 'DELETE') {
        deleteCalls += 1;
        return Promise.reject(new Error('network down'));
      }
      throw new Error(`unexpected request ${url}`);
    });
    vi.stubGlobal('fetch', fetchMock);

    function ReplaceProbe() {
      const { snapshot, execute, cancel } = useSearchExecution();
      const run = () => {
        void execute({ expression: 'ERROR', issue_code: 'ISSUE' }, { scopeKey: 'issue:ISSUE', workspaceSessionId: null })
          .then(() => { replacementDone = true; });
      };
      return (
        <>
          <output data-testid="status">{snapshot.status}</output>
          <SearchExecutionStatus snapshot={snapshot} onCancel={() => { void cancel(); }} />
          <button type="button" onClick={run}>new</button>
        </>
      );
    }

    render(<ReplaceProbe />);
    fireEvent.click(screen.getByRole('button', { name: 'new' }));
    await waitFor(() => expect(previewCalls).toBe(1));
    fireEvent.click(screen.getByRole('button', { name: 'new' }));

    await waitFor(() => expect(replacementDone).toBe(true));
    expect(deleteCalls).toBe(1);
    expect(previewCalls).toBe(1);
    expect(screen.getByTestId('status')).toHaveTextContent('CANCELLING');
    expect(screen.getByRole('button', { name: '重试取消' })).toBeInTheDocument();
  });

  it('settles a replacement request when cancellation never responds', async () => {
    vi.useFakeTimers();
    let previewCalls = 0;
    let deleteCalls = 0;
    let replacementDone = false;
    const fetchMock = vi.fn().mockImplementation((url: string, init: RequestInit = {}) => {
      if (url.endsWith('/api/search-requests') && init.method === 'POST') {
        return Promise.resolve(new Response(JSON.stringify({
          search_id: '00000000-0000-4000-8000-000000000001',
          cancel_token: 'token',
          expires_in_ms: 60_000
        }), { status: 201 }));
      }
      if (url.endsWith('/api/temp-results/preview')) {
        previewCalls += 1;
        return new Promise((_resolve, reject) => {
          init.signal?.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')));
        });
      }
      if (url.includes('/api/search-requests/') && init.method === 'DELETE') {
        deleteCalls += 1;
        if (deleteCalls === 1) return new Promise(() => undefined);
        return Promise.resolve(new Response(JSON.stringify({ status: 'cancelled' }), { status: 200 }));
      }
      throw new Error(`unexpected request ${url}`);
    });
    vi.stubGlobal('fetch', fetchMock);

    function ReplaceProbe() {
      const { snapshot, execute, cancel } = useSearchExecution();
      const run = () => {
        void execute({ expression: 'ERROR', issue_code: 'ISSUE' }, { scopeKey: 'issue:ISSUE', workspaceSessionId: null })
          .then(() => { replacementDone = true; });
      };
      return (
        <>
          <output data-testid="status">{snapshot.status}</output>
          <SearchExecutionStatus snapshot={snapshot} onCancel={() => { void cancel(); }} />
          <button type="button" onClick={run}>new</button>
        </>
      );
    }

    const rendered = render(<ReplaceProbe />);
    try {
      fireEvent.click(screen.getByRole('button', { name: 'new' }));
      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(previewCalls).toBe(1);
      fireEvent.click(screen.getByRole('button', { name: 'new' }));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(5_000);
      });
      expect(replacementDone).toBe(true);
      expect(screen.getByTestId('status')).toHaveTextContent('CANCELLING');
    } finally {
      rendered.unmount();
      await act(async () => {
        await vi.advanceTimersByTimeAsync(5_000);
      });
      vi.useRealTimers();
    }
  });

  it('renders a failed execution message only in the execution status', () => {
    render(<SearchExecutionStatus snapshot={{
      status: 'FAILED',
      searchId: 'id',
      scopeKey: 'issue:X',
      elapsedMs: 1_250,
      errorMessage: '临时结果超过大小限制',
      cancelUnconfirmed: false
    }} onCancel={() => undefined} />);

    expect(screen.getAllByText('临时结果超过大小限制')).toHaveLength(1);
  });

  it('renders indeterminate progress without a fake percentage', () => {
    render(<SearchExecutionStatus snapshot={{ status: 'RUNNING', searchId: 'id', scopeKey: 'issue:X', elapsedMs: 1_250, errorMessage: null, cancelUnconfirmed: false }} onCancel={() => undefined} />);
    expect(screen.getByRole('progressbar')).not.toHaveAttribute('aria-valuenow');
    expect(screen.getByRole('button', { name: '取消搜索' })).toBeInTheDocument();
    expect(screen.getByText(/00:01/)).toBeInTheDocument();
  });
});
