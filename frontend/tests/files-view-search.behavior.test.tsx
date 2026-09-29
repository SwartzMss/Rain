import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useNavigate } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { BundleView } from '../src/features/files/FilesView';

const testMocks = vi.hoisted(() => ({
  validateSearchExpression: vi.fn(),
  execute: vi.fn(),
  cancel: vi.fn(),
  fetchSavedSearches: vi.fn(),
  createSavedSearch: vi.fn(),
  updateSavedSearch: vi.fn(),
  markSavedSearchUsed: vi.fn(),
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
    createSavedSearch: testMocks.createSavedSearch,
    updateSavedSearch: testMocks.updateSavedSearch,
    markSavedSearchUsed: testMocks.markSavedSearchUsed,
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

const savedAdvancedSearch = {
  id: 'saved-1',
  name: 'Saved B',
  search_type: 'DETAIL' as const,
  query_text: 'B AND (C OR D)',
  options: { version: 1, editor_mode: 'advanced' },
  is_pinned: false,
  created_at: '2026-09-29T00:00:00Z',
  updated_at: '2026-09-29T00:00:00Z',
  last_used_at: null
};

describe('BundleView search expression flow', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    testMocks.fetchSavedSearches.mockResolvedValue([]);
    testMocks.fetchIssueBundles.mockResolvedValue({ log_bundles: [] });
    testMocks.execute.mockResolvedValue({ result_id: 'result-1', total: 0, lines: [] });
    testMocks.cancel.mockResolvedValue(undefined);
    testMocks.createSavedSearch.mockResolvedValue(undefined);
    testMocks.updateSavedSearch.mockResolvedValue(undefined);
    testMocks.markSavedSearchUsed.mockResolvedValue(undefined);
  });

  it('does not start a stale search after validation resolves out of order', async () => {
    const firstValidation = deferred<{ valid: true }>();
    const secondValidation = deferred<{ valid: true }>();
    testMocks.validateSearchExpression
      .mockReturnValueOnce(firstValidation.promise)
      .mockReturnValueOnce(secondValidation.promise);

    renderBundleView();
    const editor = screen.getByRole('textbox', { name: '日志内容搜索条件' });
    fireEvent.change(editor, { target: { value: 'A' } });
    fireEvent.click(screen.getByRole('button', { name: '搜索日志内容' }));
    await waitFor(() => expect(testMocks.validateSearchExpression).toHaveBeenCalledTimes(1));

    fireEvent.click(screen.getByRole('button', { name: '清除日志内容搜索' }));
    fireEvent.change(editor, { target: { value: 'B' } });
    fireEvent.click(screen.getByRole('button', { name: '搜索日志内容' }));
    await waitFor(() => expect(testMocks.validateSearchExpression).toHaveBeenCalledTimes(2));

    await act(async () => secondValidation.resolve({ valid: true }));
    await waitFor(() => expect(testMocks.execute).toHaveBeenCalledTimes(1));
    await act(async () => firstValidation.resolve({ valid: true }));

    await waitFor(() => expect(testMocks.execute).toHaveBeenCalledTimes(1));
    expect(testMocks.execute.mock.calls[0][0].expression).toBe('"B"');
  });

  it('does not start validation that was cleared while it was pending', async () => {
    const validation = deferred<{ valid: true }>();
    testMocks.validateSearchExpression.mockReturnValue(validation.promise);

    renderBundleView();
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
    const editor = screen.getByRole('textbox', { name: '日志内容搜索条件' });
    fireEvent.change(editor, { target: { value: 'A' } });
    fireEvent.click(screen.getByRole('button', { name: '搜索日志内容' }));
    await waitFor(() => expect(testMocks.validateSearchExpression).toHaveBeenCalledTimes(1));

    fireEvent.click(screen.getByRole('button', { name: '切换到 ISSUE-2' }));
    await screen.findByText('ISSUE-2');
    await act(async () => validation.resolve({ valid: true }));

    expect(testMocks.execute).not.toHaveBeenCalled();
  });

  it('does not start pending manual validation after using a saved search', async () => {
    const validation = deferred<{ valid: true }>();
    testMocks.fetchSavedSearches.mockResolvedValue([savedAdvancedSearch]);
    testMocks.validateSearchExpression
      .mockReturnValueOnce(validation.promise)
      .mockResolvedValue({ valid: true });

    renderBundleView();
    const editor = screen.getByRole('textbox', { name: '日志内容搜索条件' });
    fireEvent.change(editor, { target: { value: 'A' } });
    fireEvent.click(screen.getByRole('button', { name: '搜索日志内容' }));
    await waitFor(() => expect(testMocks.validateSearchExpression).toHaveBeenCalledTimes(1));

    fireEvent.click(screen.getByRole('button', { name: '我的搜索条件' }));
    fireEvent.click(await screen.findByRole('button', { name: '使用' }));
    await waitFor(() => expect(testMocks.execute).toHaveBeenCalledTimes(1));
    expect(testMocks.execute.mock.calls[0][0].expression).toBe(savedAdvancedSearch.query_text);

    await act(async () => validation.resolve({ valid: true }));
    expect(testMocks.execute).toHaveBeenCalledTimes(1);
  });

  it('keeps saved-search loading active when an earlier manual search finishes', async () => {
    const manualExecution = deferred<{ result_id: string; total: number; lines: never[] }>();
    const response = { result_id: 'result-1', total: 0, lines: [] };
    testMocks.fetchSavedSearches.mockResolvedValue([savedAdvancedSearch]);
    testMocks.validateSearchExpression.mockResolvedValue({ valid: true });
    testMocks.execute
      .mockReturnValueOnce(manualExecution.promise)
      .mockResolvedValueOnce(response);

    renderBundleView();
    const editor = screen.getByRole('textbox', { name: '日志内容搜索条件' });
    fireEvent.change(editor, { target: { value: 'A' } });
    fireEvent.click(screen.getByRole('button', { name: '搜索日志内容' }));
    await waitFor(() => expect(testMocks.execute).toHaveBeenCalledTimes(1));

    fireEvent.click(screen.getByRole('button', { name: '我的搜索条件' }));
    fireEvent.click(await screen.findByRole('button', { name: '使用' }));
    await waitFor(() => expect(testMocks.execute).toHaveBeenCalledTimes(2));

    await act(async () => manualExecution.resolve(response));
    await waitFor(() => expect(screen.getByRole('button', { name: '搜索日志内容' })).toBeDisabled());
  });

  it('round-trips a parenthesized saved search in the unified editor', async () => {
    let savedSearches: typeof savedAdvancedSearch[] = [];
    testMocks.fetchSavedSearches.mockImplementation(async () => savedSearches);
    testMocks.validateSearchExpression.mockResolvedValue({ valid: true });
    testMocks.createSavedSearch.mockImplementation(async (payload) => {
      const item = {
        ...payload,
        id: 'saved-advanced',
        is_pinned: false,
        created_at: '2026-09-29T00:00:00Z',
        updated_at: '2026-09-29T00:00:00Z',
        last_used_at: null
      };
      savedSearches = [item];
      return item;
    });

    renderBundleView();
    const editor = screen.getByRole('textbox', { name: '日志内容搜索条件' });
    fireEvent.change(editor, { target: { value: 'A' } });
    fireEvent.click(screen.getByRole('button', { name: 'AND' }));
    fireEvent.click(screen.getByRole('button', { name: '(' }));
    fireEvent.change(editor, { target: { value: 'B' } });
    fireEvent.click(screen.getByRole('button', { name: 'OR' }));
    fireEvent.change(editor, { target: { value: 'C' } });
    fireEvent.click(screen.getByRole('button', { name: ')' }));
    fireEvent.click(screen.getByRole('button', { name: '保存条件' }));
    fireEvent.change(screen.getByRole('textbox', { name: '名称' }), {
      target: { value: 'Advanced search' }
    });
    fireEvent.click(screen.getByRole('button', { name: '保存', exact: true }));

    await waitFor(() => expect(testMocks.createSavedSearch).toHaveBeenCalledTimes(1));
    expect(testMocks.createSavedSearch.mock.calls[0][0]).toMatchObject({
      query_text: '"A" AND ( "B" OR "C" )',
      options: { version: 1 }
    });

    fireEvent.click(screen.getByRole('button', { name: '我的搜索条件' }));
    fireEvent.click(await screen.findByRole('button', { name: '使用' }));
    await waitFor(() => expect(screen.getByRole('button', { name: '编辑关键词 A' })).toBeInTheDocument());
    expect(screen.getByRole('button', { name: '编辑关键词 B' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '编辑关键词 C' })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: '我的搜索条件' }));
    fireEvent.click(await screen.findByRole('button', { name: '编辑' }));
    expect(screen.getAllByRole('button', { name: '编辑关键词 A' })).toHaveLength(2);
    expect(screen.getByRole('textbox', { name: '编辑详细搜索条件' })).toBeInTheDocument();
  });

  it('round-trips a saved search with unary NOT in the unified editor', async () => {
    let savedSearches: typeof savedAdvancedSearch[] = [];
    testMocks.fetchSavedSearches.mockImplementation(async () => savedSearches);
    testMocks.createSavedSearch.mockImplementation(async (payload) => {
      const item = {
        ...payload,
        id: 'saved-simple',
        is_pinned: false,
        created_at: '2026-09-29T00:00:00Z',
        updated_at: '2026-09-29T00:00:00Z',
        last_used_at: null
      };
      savedSearches = [item];
      return item;
    });

    renderBundleView();
    fireEvent.change(screen.getByRole('textbox', { name: '日志内容搜索条件' }), {
      target: { value: 'A' }
    });
    fireEvent.click(screen.getByRole('button', { name: 'NOT' }));
    fireEvent.change(screen.getByRole('textbox', { name: '日志内容搜索条件' }), {
      target: { value: 'B' }
    });
    fireEvent.click(screen.getByRole('button', { name: '保存条件' }));
    fireEvent.change(screen.getByRole('textbox', { name: '名称' }), {
      target: { value: 'Simple search' }
    });
    fireEvent.click(screen.getByRole('button', { name: '保存', exact: true }));

    await waitFor(() => expect(testMocks.createSavedSearch).toHaveBeenCalledTimes(1));
    expect(testMocks.createSavedSearch.mock.calls[0][0]).toMatchObject({
      query_text: '"A" AND NOT "B"',
      options: { version: 1 }
    });

    fireEvent.click(screen.getByRole('button', { name: '我的搜索条件' }));
    fireEvent.click(await screen.findByRole('button', { name: '使用' }));
    await waitFor(() => expect(screen.getByRole('button', { name: '编辑关键词 A' })).toBeInTheDocument());
    expect(screen.getByRole('button', { name: '编辑关键词 A' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '编辑关键词 B' })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: '我的搜索条件' }));
    fireEvent.click(await screen.findByRole('button', { name: '编辑' }));
    expect(screen.getAllByRole('button', { name: '编辑关键词 A' })).toHaveLength(2);
  });

  it('reveals a root page-two source in the file tree without loading sibling pages', async () => {
    const response = {
      result_id: 'result-1',
      total: 1,
      lines: [{
        bundle_hash: 'bundle',
        file_id: 101,
        path: '/target.log',
        content: 'ERROR from target',
        line_number: 10
      }]
    };
    testMocks.fetchIssueBundles.mockResolvedValue({
      log_bundles: [{ hash: 'bundle', name: 'Bundle', status: { upload_status: 'READY' } }]
    });
    testMocks.fetchFileNode.mockImplementation(async (_bundleId, fileId) => {
      if (fileId === 'root') {
        return {
          node: {
            id: 'root',
            parent_id: null,
            name: 'bundle_root',
            path: '/',
            is_dir: true,
            preview_kind: 'directory'
          },
          children: [{
            id: 1,
            parent_id: null,
            name: 'first.log',
            path: '/first.log',
            is_dir: false,
            preview_kind: 'text'
          }],
          has_more: true,
          next_cursor: 'root-cursor'
        };
      }
      if (fileId === '101') {
        return {
          node: {
            id: 101,
            parent_id: null,
            name: 'target.log',
            path: '/target.log',
            is_dir: false,
            preview_kind: 'text'
          },
          children: [],
          has_more: false,
          next_cursor: null
        };
      }
      throw new Error(`unexpected file node request: ${fileId}`);
    });
    testMocks.fetchFileLines.mockResolvedValue({
      path: '/target.log',
      start: 0,
      limit: 1000,
      lines: [{ line_number: 10, content: 'ERROR from target' }]
    });
    testMocks.execute.mockImplementation(async (_request, options) => {
      options?.onSuccess?.(response);
      return response;
    });

    renderBundleView();
    await waitFor(() => expect(testMocks.fetchFileNode).toHaveBeenCalledWith('bundle', 'root', { limit: 100 }));
    fireEvent.change(screen.getByRole('textbox', { name: '日志内容搜索条件' }), {
      target: { value: 'ERROR' }
    });
    fireEvent.click(screen.getByRole('button', { name: '搜索日志内容' }));

    await waitFor(() => expect(screen.getByText('ERROR from target')).toBeInTheDocument());
    await act(async () => {});
    fireEvent.contextMenu(screen.getByText('ERROR from target'));
    fireEvent.click(await screen.findByRole('menuitem', { name: '在原文件中打开' }));

    const targetButton = await screen.findByRole('button', { name: 'target.log' });
    expect(targetButton).toHaveAttribute('aria-current', 'true');
    expect(testMocks.fetchFileNode.mock.calls.some(([, fileId, options]) => (
      fileId === 'root' && Boolean(options?.cursor)
    ))).toBe(false);
  });

  it('reveals a source inside an archive nested in another archive in one action', async () => {
    const response = {
      result_id: 'result-nested',
      total: 1,
      lines: [{
        bundle_hash: 'bundle',
        file_id: 30,
        path: '/outer.zip/inner.zip/source.log',
        content: 'ERROR from nested source',
        line_number: 10
      }]
    };
    testMocks.fetchIssueBundles.mockResolvedValue({
      log_bundles: [{ hash: 'bundle', name: 'Bundle', status: { upload_status: 'READY' } }]
    });
    testMocks.fetchFileNode.mockImplementation(async (_bundleId, fileId) => {
      const responses = {
        root: {
          node: {
            id: 'root', parent_id: null, name: 'bundle_root', path: '/',
            is_dir: true, preview_kind: 'directory'
          },
          children: [{
            id: 1, parent_id: null, name: 'first.log', path: '/first.log',
            is_dir: false, preview_kind: 'text'
          }, {
            id: 10, parent_id: null, name: 'outer.zip', path: '/outer.zip',
            is_dir: false, preview_kind: 'archive'
          }],
          has_more: false,
          next_cursor: null
        },
        '10': {
          node: {
            id: 10, parent_id: null, name: 'outer.zip', path: '/outer.zip',
            is_dir: false, preview_kind: 'archive'
          },
          children: [{
            id: 11, parent_id: 10, name: 'outer.zip_extracted',
            path: '/outer.zip_extracted', is_dir: true, preview_kind: 'directory'
          }],
          has_more: false,
          next_cursor: null
        },
        '11': {
          node: {
            id: 11, parent_id: 10, name: 'outer.zip_extracted',
            path: '/outer.zip_extracted', is_dir: true, preview_kind: 'directory'
          },
          children: [{
            id: 20, parent_id: 11, name: 'inner.zip', path: '/outer.zip/inner.zip',
            is_dir: false, preview_kind: 'archive'
          }],
          has_more: false,
          next_cursor: null
        },
        '20': {
          node: {
            id: 20, parent_id: 11, name: 'inner.zip', path: '/outer.zip/inner.zip',
            is_dir: false, preview_kind: 'archive'
          },
          children: [{
            id: 21, parent_id: 20, name: 'inner.zip_extracted',
            path: '/outer.zip/inner.zip_extracted', is_dir: true, preview_kind: 'directory'
          }],
          has_more: false,
          next_cursor: null
        },
        '21': {
          node: {
            id: 21, parent_id: 20, name: 'inner.zip_extracted',
            path: '/outer.zip/inner.zip_extracted', is_dir: true, preview_kind: 'directory'
          },
          children: [{
            id: 30, parent_id: 21, name: 'source.log',
            path: '/outer.zip/inner.zip/source.log', is_dir: false, preview_kind: 'text'
          }],
          has_more: false,
          next_cursor: null
        },
        '30': {
          node: {
            id: 30, parent_id: 21, name: 'source.log',
            path: '/outer.zip/inner.zip/source.log', is_dir: false, preview_kind: 'text'
          },
          children: [],
          has_more: false,
          next_cursor: null
        }
      } as const;
      const result = responses[String(fileId) as keyof typeof responses];
      if (!result) throw new Error(`unexpected file node request: ${fileId}`);
      return result;
    });
    testMocks.fetchFileLines.mockResolvedValue({
      path: '/outer.zip/inner.zip/source.log',
      start: 0,
      limit: 1000,
      lines: [{ line_number: 10, content: 'ERROR from nested source' }]
    });
    testMocks.execute.mockImplementation(async (_request, options) => {
      options?.onSuccess?.(response);
      return response;
    });

    renderBundleView();
    await waitFor(() => expect(testMocks.fetchFileNode).toHaveBeenCalledWith('bundle', 'root', { limit: 100 }));
    await waitFor(() => expect(
      document.querySelector('[data-file-tree-node-id="bundle:1"]')
    ).toBeInTheDocument());
    fireEvent.change(screen.getByRole('textbox', { name: '日志内容搜索条件' }), {
      target: { value: 'ERROR' }
    });
    fireEvent.click(screen.getByRole('button', { name: '搜索日志内容' }));

    await waitFor(() => expect(screen.getByText('ERROR from nested source')).toBeInTheDocument());
    fireEvent.contextMenu(screen.getByText('ERROR from nested source'));
    fireEvent.click(await screen.findByRole('menuitem', { name: '在原文件中打开' }));

    await waitFor(() => expect(
      document.querySelector('[data-file-tree-node-id="bundle:30"]')
    ).toBeInTheDocument());
    expect(document.querySelector('[data-file-tree-node-id="bundle:30"]'))
      .toHaveAttribute('aria-current', 'true');
  });
});
