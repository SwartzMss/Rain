import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { RequestCancelledError, rainApi } from '../src/api/client';
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
    void execute({ expression: 'ERROR', issue_code: 'ISSUE' }, { scopeKey: 'issue:ISSUE' });
  }, [execute]);
  return (
    <>
      <output data-testid="status">{snapshot.status}</output>
      <button type="button" onClick={() => { void cancel(); }}>cancel</button>
    </>
  );
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
      throw new Error(`unexpected request ${url}`);
    });
    vi.stubGlobal('fetch', fetchMock);

    function ReplaceProbe() {
      const { snapshot, execute } = useSearchExecution();
      const [loading, setLoading] = useState(false);
      const run = () => {
        setLoading(true);
        void execute({ expression: 'WARN', issue_code: 'ISSUE' }, { scopeKey: 'issue:ISSUE' });
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

  it('renders indeterminate progress without a fake percentage', () => {
    render(<SearchExecutionStatus snapshot={{ status: 'RUNNING', searchId: 'id', scopeKey: 'issue:X', elapsedMs: 1_250, errorMessage: null, cancelUnconfirmed: false }} onCancel={() => undefined} />);
    expect(screen.getByRole('progressbar')).not.toHaveAttribute('aria-valuenow');
    expect(screen.getByRole('button', { name: '取消搜索' })).toBeInTheDocument();
    expect(screen.getByText(/00:01/)).toBeInTheDocument();
  });
});
