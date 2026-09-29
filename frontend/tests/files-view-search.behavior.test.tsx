import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useNavigate } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { BundleView } from '../src/features/files/FilesView';

const testMocks = vi.hoisted(() => ({
  validateSearchExpression: vi.fn(),
  execute: vi.fn(),
  cancel: vi.fn(),
  fetchSavedSearches: vi.fn(),
  fetchIssueBundles: vi.fn(),
  fetchFileNode: vi.fn(),
  fetchFileLines: vi.fn()
}));

vi.mock('../src/api/client', () => ({
  ApiError: class ApiError extends Error {},
  normalizeApiError: (error: unknown) => String(error),
  rainApi: {
    validateSearchExpression: testMocks.validateSearchExpression,
    fetchSavedSearches: testMocks.fetchSavedSearches,
    fetchIssueBundles: testMocks.fetchIssueBundles,
    fetchFileNode: testMocks.fetchFileNode,
    fetchFileLines: testMocks.fetchFileLines,
    deleteTempResult: vi.fn()
  }
}));

vi.mock('../src/auth/AuthContext', () => ({
  useAuth: () => ({
    state: {
      status: 'AUTHENTICATED',
      user: { id: 'user-1', username: 'tester', role: 'USER', status: 'ACTIVE' }
    }
  })
}));

vi.mock('../src/hooks/useSearchExecution', () => ({
  useSearchExecution: () => ({
    snapshot: {
      status: 'IDLE',
      searchId: null,
      scopeKey: null,
      elapsedMs: 0,
      errorMessage: null,
      cancelUnconfirmed: false
    },
    execute: testMocks.execute,
    cancel: testMocks.cancel
  })
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function NavigationProbe() {
  const navigate = useNavigate();
  return (
    <button type="button" onClick={() => navigate('/issues/ISSUE-2/bundles')}>
      切换到 ISSUE-2
    </button>
  );
}

function renderBundleView() {
  return render(
    <MemoryRouter initialEntries={['/issues/ISSUE-1/bundles']}>
      <NavigationProbe />
      <Routes>
        <Route path="/issues/:issueCode/bundles" element={<BundleView />} />
      </Routes>
    </MemoryRouter>
  );
}

describe('BundleView search expression flow', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    testMocks.fetchSavedSearches.mockResolvedValue([]);
    testMocks.fetchIssueBundles.mockResolvedValue({ log_bundles: [] });
    testMocks.execute.mockResolvedValue({ result_id: 'result-1', total: 0, lines: [] });
    testMocks.cancel.mockResolvedValue(undefined);
  });

  it('does not start a stale search after validation resolves out of order', async () => {
    const firstValidation = deferred<{ valid: true }>();
    const secondValidation = deferred<{ valid: true }>();
    testMocks.validateSearchExpression
      .mockReturnValueOnce(firstValidation.promise)
      .mockReturnValueOnce(secondValidation.promise);

    renderBundleView();
    fireEvent.click(await screen.findByRole('tab', { name: '高级表达式' }));
    const editor = screen.getByRole('textbox', { name: '日志内容搜索条件' });
    fireEvent.change(editor, { target: { value: 'A' } });
    fireEvent.click(screen.getByRole('button', { name: '搜索日志内容' }));
    await waitFor(() => expect(testMocks.validateSearchExpression).toHaveBeenCalledTimes(1));

    fireEvent.change(editor, { target: { value: 'B' } });
    fireEvent.click(screen.getByRole('button', { name: '搜索日志内容' }));
    await waitFor(() => expect(testMocks.validateSearchExpression).toHaveBeenCalledTimes(2));

    await act(async () => secondValidation.resolve({ valid: true }));
    await waitFor(() => expect(testMocks.execute).toHaveBeenCalledTimes(1));
    await act(async () => firstValidation.resolve({ valid: true }));

    await waitFor(() => expect(testMocks.execute).toHaveBeenCalledTimes(1));
    expect(testMocks.execute.mock.calls[0][0].expression).toBe('B');
  });

  it('does not start validation that was cleared while it was pending', async () => {
    const validation = deferred<{ valid: true }>();
    testMocks.validateSearchExpression.mockReturnValue(validation.promise);

    renderBundleView();
    fireEvent.click(await screen.findByRole('tab', { name: '高级表达式' }));
    const editor = screen.getByRole('textbox', { name: '日志内容搜索条件' });
    fireEvent.change(editor, { target: { value: 'A' } });
    fireEvent.click(screen.getByRole('button', { name: '搜索日志内容' }));
    await waitFor(() => expect(testMocks.validateSearchExpression).toHaveBeenCalledTimes(1));

    fireEvent.click(screen.getByRole('button', { name: '清除日志内容搜索' }));
    await act(async () => validation.resolve({ valid: true }));

    expect(testMocks.execute).not.toHaveBeenCalled();
  });

  it('invalidates pending validation when the issue context changes', async () => {
    const validation = deferred<{ valid: true }>();
    testMocks.validateSearchExpression.mockReturnValue(validation.promise);

    renderBundleView();
    fireEvent.click(await screen.findByRole('tab', { name: '高级表达式' }));
    const editor = screen.getByRole('textbox', { name: '日志内容搜索条件' });
    fireEvent.change(editor, { target: { value: 'A' } });
    fireEvent.click(screen.getByRole('button', { name: '搜索日志内容' }));
    await waitFor(() => expect(testMocks.validateSearchExpression).toHaveBeenCalledTimes(1));

    fireEvent.click(screen.getByRole('button', { name: '切换到 ISSUE-2' }));
    await screen.findByText('ISSUE-2');
    await act(async () => validation.resolve({ valid: true }));

    expect(testMocks.execute).not.toHaveBeenCalled();
  });
});
