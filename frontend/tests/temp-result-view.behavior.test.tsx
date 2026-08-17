import { act, fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useNavigate } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { AuthProvider } from '../src/auth/AuthContext';
import { rainApi } from '../src/api/client';
import type { TempResultInfo, TempResultLinesResponse } from '../src/api/types';
import { TempResultRoute } from '../src/features/files/TempResultView';

vi.mock('../src/api/client', () => ({
  normalizeApiError: (error: unknown) => String(error),
  rainApi: {
    me: vi.fn(),
    fetchTempResult: vi.fn(),
    fetchTempResultLines: vi.fn(),
    previewTempResult: vi.fn(),
    deleteTempResult: vi.fn()
  }
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

function result(id: string, name = id): TempResultInfo {
  return {
    id,
    name,
    expression: 'ERROR',
    source_label: 'source.log',
    line_count: 4,
    size_bytes: 10,
    created_at: '2026-08-17T00:00:00Z',
    expires_at: '2026-08-18T00:00:00Z'
  };
}

function lines(start: number, nextStart: number | null): TempResultLinesResponse {
  return {
    start,
    limit: 2,
    line_count: 4,
    next_start: nextStart,
    lines: [
      { line_number: start, content: `line-${start}` },
      { line_number: start + 1, content: `line-${start + 1}` }
    ]
  };
}

function NavigationProbe() {
  const navigate = useNavigate();
  return <button type="button" onClick={() => navigate('/temp-results/B')}>去 B</button>;
}

function renderRoute() {
  return render(
    <MemoryRouter initialEntries={['/temp-results/A']}>
      <AuthProvider>
        <NavigationProbe />
        <Routes>
          <Route path="/temp-results/:resultId" element={<TempResultRoute />} />
        </Routes>
      </AuthProvider>
    </MemoryRouter>
  );
}

describe('standalone Temp Result view', () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(rainApi.me).mockResolvedValue({ authenticated: false, user: null });
  });

  it('does not display an old result while navigating to a new result or after delayed old responses', async () => {
    const oldMetadata = deferred<TempResultInfo>();
    const oldLines = deferred<TempResultLinesResponse>();
    const newMetadata = deferred<TempResultInfo>();
    const newLines = deferred<TempResultLinesResponse>();
    vi.mocked(rainApi.fetchTempResult)
      .mockReturnValueOnce(oldMetadata.promise)
      .mockReturnValueOnce(newMetadata.promise);
    vi.mocked(rainApi.fetchTempResultLines)
      .mockReturnValueOnce(oldLines.promise)
      .mockReturnValueOnce(newLines.promise);

    renderRoute();
    await act(async () => {
      oldMetadata.resolve(result('A'));
      oldLines.resolve(lines(0, 2));
    });
    expect(await screen.findByText('A')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '去 B' }));
    expect(screen.getByText('临时结果加载中...')).toBeInTheDocument();
    expect(screen.queryByText('A')).not.toBeInTheDocument();

    await act(async () => {
      newMetadata.resolve(result('B'));
      newLines.resolve(lines(0, 2));
    });
    expect(await screen.findByText('B')).toBeInTheDocument();

    await act(async () => {
      oldMetadata.resolve(result('A'));
      oldLines.resolve(lines(0, 2));
    });
    expect(screen.queryByText('A')).not.toBeInTheDocument();
    expect(screen.getByText('B')).toBeInTheDocument();
  });

  it('keeps the committed page and range when the next-page request fails', async () => {
    const nextPage = deferred<TempResultLinesResponse>();
    vi.mocked(rainApi.fetchTempResult).mockResolvedValueOnce(result('A'));
    vi.mocked(rainApi.fetchTempResultLines)
      .mockResolvedValueOnce(lines(0, 2))
      .mockReturnValueOnce(nextPage.promise);

    renderRoute();
    expect(await screen.findByText('1 - 2 / 4')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '下一页' }));
    expect(screen.getByText('1 - 2 / 4')).toBeInTheDocument();

    await act(async () => nextPage.reject(new Error('page failed')));
    expect(screen.getByText('1 - 2 / 4')).toBeInTheDocument();
    expect(screen.getByText('Error: page failed')).toBeInTheDocument();
  });

  it('ignores an older page response after a newer request succeeds', async () => {
    const olderPage = deferred<TempResultLinesResponse>();
    const newerPage = deferred<TempResultLinesResponse>();
    vi.mocked(rainApi.fetchTempResult).mockResolvedValue(result('A'));
    vi.mocked(rainApi.fetchTempResultLines)
      .mockResolvedValueOnce(lines(0, 2))
      .mockReturnValueOnce(olderPage.promise)
      .mockReturnValueOnce(newerPage.promise);

    renderRoute();
    expect(await screen.findByText('line-0')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '下一页' }));
    fireEvent.change(screen.getByRole('combobox'), { target: { value: '10000' } });

    await act(async () => {
      newerPage.resolve({
        ...lines(0, null),
        lines: [
          { line_number: 0, content: 'new-page-0' },
          { line_number: 1, content: 'new-page-1' }
        ]
      });
    });
    expect(screen.getByText('new-page-0')).toBeInTheDocument();

    await act(async () => olderPage.resolve(lines(2, null)));
    expect(screen.getByText('new-page-0')).toBeInTheDocument();
    expect(screen.queryByText('line-2')).not.toBeInTheDocument();
  });
});
