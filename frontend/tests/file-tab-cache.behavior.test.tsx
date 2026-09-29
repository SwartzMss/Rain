import { fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { AuthProvider } from '../src/auth/AuthContext';
import { BundleView } from '../src/features/files/FilesView';
import { rainApi } from '../src/api/client';

vi.mock('../src/api/client', () => ({
  ApiError: class ApiError extends Error {},
  RequestCancelledError: class RequestCancelledError extends Error {},
  normalizeApiError: (error: unknown) => error instanceof Error ? error.message : String(error),
  rainApi: {
    me: vi.fn(),
    fetchSavedSearches: vi.fn(),
    fetchIssueBundles: vi.fn(),
    fetchFileNode: vi.fn(),
    fetchFileLines: vi.fn(),
    deleteTempResult: vi.fn()
  }
}));

const api = vi.mocked(rainApi);

function renderBundle() {
  return render(
    <MemoryRouter initialEntries={['/issue/ISSUE/bundle/bundle']}>
      <AuthProvider>
        <Routes>
          <Route path="/issue/:issueCode/bundle/:bundleHash" element={<BundleView />} />
        </Routes>
      </AuthProvider>
    </MemoryRouter>
  );
}

const fileNode = (id: number, name: string) => ({
  id,
  parent_id: 0,
  name,
  path: `/${name}`,
  is_dir: false as const,
  preview_kind: 'text' as const,
  size_bytes: 10,
  status: 'READY'
});

describe('BundleView file tab cache', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.me.mockResolvedValue({ authenticated: true, user: { id: 'user', username: 'alice', role: 'USER' } });
    api.fetchSavedSearches.mockResolvedValue([]);
    api.fetchIssueBundles.mockResolvedValue({
      log_bundles: [{ hash: 'bundle', name: 'Bundle', status: { upload_status: 'READY' } }]
    });
    api.fetchFileNode.mockResolvedValue({
      node: { id: 0, parent_id: null, name: 'root', path: '/', is_dir: true, preview_kind: 'directory' },
      children: [fileNode(1, 'A.log'), fileNode(2, 'B.log')],
      has_more: false,
      next_cursor: null
    });
    api.fetchFileLines.mockImplementation(async (_bundle, file) => ({
      path: file === '1' ? 'A.log' : 'B.log',
      start: 0,
      limit: 5000,
      lines: [{ line_number: 0, content: file === '1' ? 'A line' : 'B line' }]
    }));
  });

  it('shows cached A synchronously after switching A to B and back', async () => {
    renderBundle();

    expect(await screen.findByText('A line')).toBeInTheDocument();
    expect(api.fetchFileLines).toHaveBeenCalledTimes(1);

    fireEvent.click(document.querySelector<HTMLButtonElement>('[data-file-tree-node-id="bundle:2"]')!);
    expect(await screen.findByText('B line')).toBeInTheDocument();
    expect(api.fetchFileLines).toHaveBeenCalledTimes(2);

    fireEvent.click(document.querySelector<HTMLButtonElement>('[data-file-tree-node-id="bundle:1"]')!);
    expect(screen.getByText('A line')).toBeInTheDocument();
    expect(screen.queryByText('读取中...')).not.toBeInTheDocument();
    expect(api.fetchFileLines).toHaveBeenCalledTimes(2);
  });
});
