import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { HomeView } from '../src/features/files/HomeView';

const testMocks = vi.hoisted(() => ({
  fetchIssueBundles: vi.fn()
}));

vi.mock('../src/api/client', () => ({
  ApiError: class ApiError extends Error {
    code?: string;
  },
  normalizeApiError: (error: unknown) => error instanceof Error ? error.message : String(error),
  rainApi: { fetchIssueBundles: testMocks.fetchIssueBundles }
}));

vi.mock('../src/auth/AuthContext', () => ({
  useAuth: () => ({
    state: { status: 'AUTHENTICATED', user: { id: 'guide-user', username: 'writer', role: 'USER' } }
  })
}));

vi.mock('../src/features/files/hooks/useIssues', async () => {
  const { useCallback, useState } = await import('react');
  const issue = { code: 'ISSUE-1', can_write: true, owner_username: 'writer' };
  return {
    useIssues: () => {
      const [currentIssueCode, setCurrentIssueCode] = useState('');
      const clearSelectedIssue = useCallback(() => setCurrentIssueCode(''), []);
      const selectIssue = useCallback((code: string) => {
        setCurrentIssueCode(code);
        return code;
      }, []);
      return {
        canChooseScope: false,
        changeIssueScope: vi.fn(),
        clearSelectedIssue,
        createIssue: vi.fn(),
        currentIssueCode,
        deleteIssue: vi.fn(),
        filteredIssues: [issue],
        hasMoreIssues: false,
        issueError: null,
        issueScope: 'all',
        issueSearchText: '',
        issues: [issue],
        issuesError: null,
        issuesLoading: false,
        issuesLoadingMore: false,
        loadIssues: vi.fn().mockResolvedValue(undefined),
        loadMoreIssues: vi.fn().mockResolvedValue(undefined),
        selectIssue,
        setIssueSearchText: vi.fn(),
        setIssuesError: vi.fn()
      };
    }
  };
});

vi.mock('../src/features/files/hooks/useUploadTask', () => ({
  useUploadTask: () => ({
    acknowledgeAcceptedTasks: vi.fn(),
    performUpload: vi.fn().mockResolvedValue(undefined),
    resetSelection: vi.fn(),
    retryUpload: vi.fn(),
    tasks: [],
    uploadDisabled: false,
    uploadError: null,
    uploading: false
  })
}));

vi.mock('../src/features/files/components/ConfirmDialog', () => ({ ConfirmDialog: () => null }));
vi.mock('../src/features/files/components/IssueCreateDialog', () => ({ IssueCreateDialog: () => null }));
vi.mock('../src/features/files/components/IssueExpirationNotice', () => ({ IssueExpirationNotice: () => null }));
vi.mock('../src/features/files/components/IssueDeleteButton', () => ({ IssueDeleteButton: () => null }));
vi.mock('../src/features/files/components/UploadFileTable', () => ({ UploadFileTable: () => null }));
vi.mock('../src/features/files/components/UploadPanel', () => ({
  UploadPanel: ({ highlightUpload }: { highlightUpload: boolean }) => <div>{highlightUpload ? '上传操作已就绪' : null}</div>
}));
vi.mock('../src/features/files/components/IssueSelector', () => ({
  IssueSelector: ({ onSelectIssue }: { onSelectIssue: (code: string) => void }) => (
    <button type="button" onClick={() => onSelectIssue('ISSUE-1')}>选择测试 Issue</button>
  )
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

function renderHomeView() {
  return render(<MemoryRouter><HomeView /></MemoryRouter>);
}

describe('HomeView onboarding Issue permissions', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    testMocks.fetchIssueBundles.mockResolvedValue({
      log_bundles: [], can_write: true, owner_username: 'writer', inactivity_expiry: null
    });
  });

  it('waits for the selected Issue permission before advancing the upload guide', async () => {
    const permissionRequest = deferred<{
      log_bundles: never[];
      can_write: boolean;
      owner_username: string;
      inactivity_expiry: null;
    }>();
    testMocks.fetchIssueBundles.mockReturnValue(permissionRequest.promise);
    renderHomeView();

    fireEvent.click(screen.getByRole('button', { name: '上传压缩包' }));
    expect(screen.getByText('先选择一个 Issue，上传的压缩包会放在该 Issue 下面。')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '选择测试 Issue' }));

    expect(await screen.findByText('正在读取当前 Issue 的权限…')).toBeInTheDocument();
    expect(screen.queryByText('这个 Issue 是只读的。上传和删除文件需要该 Issue 的写入权限。')).not.toBeInTheDocument();

    await act(async () => {
      permissionRequest.resolve({
        log_bundles: [], can_write: true, owner_username: 'writer', inactivity_expiry: null
      });
      await permissionRequest.promise;
    });
    expect(await screen.findByText('点击“选择文件”或把 .zip、.7z、.tar.gz 等支持的日志压缩包拖到上传区域。')).toBeInTheDocument();
    expect(screen.getByText('上传操作已就绪')).toBeInTheDocument();
  });

  it('waits for the selected Issue permission before advancing the delete guide', async () => {
    const permissionRequest = deferred<{
      log_bundles: never[];
      can_write: boolean;
      owner_username: string;
      inactivity_expiry: null;
    }>();
    testMocks.fetchIssueBundles.mockReturnValue(permissionRequest.promise);
    renderHomeView();

    fireEvent.click(screen.getByRole('button', { name: '删除日志文件' }));
    expect(screen.getByText('先选择包含待删除文件的 Issue。')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '选择测试 Issue' }));

    expect(await screen.findByText('正在读取 Issue 的权限和文件…')).toBeInTheDocument();
    expect(screen.queryByText('这个 Issue 是只读的。上传和删除文件需要该 Issue 的写入权限。')).not.toBeInTheDocument();

    await act(async () => {
      permissionRequest.resolve({
        log_bundles: [], can_write: true, owner_username: 'writer', inactivity_expiry: null
      });
      await permissionRequest.promise;
    });
    expect(await screen.findByText('当前 Issue 没有可删除的日志文件。先选择一个包含已处理日志的 Issue。')).toBeInTheDocument();
  });
});
