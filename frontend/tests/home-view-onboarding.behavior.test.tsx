import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { HomeView } from '../src/features/files/HomeView';

const testMocks = vi.hoisted(() => ({
  fetchIssueBundles: vi.fn(),
  fetchFileNode: vi.fn(),
  setUploadTasks: vi.fn()
}));

vi.mock('../src/api/client', () => ({
  ApiError: class ApiError extends Error {
    code?: string;
  },
  normalizeApiError: (error: unknown) => error instanceof Error ? error.message : String(error),
  rainApi: {
    fetchIssueBundles: testMocks.fetchIssueBundles,
    fetchFileNode: testMocks.fetchFileNode
  }
}));

vi.mock('../src/auth/AuthContext', () => ({
  useAuth: () => ({
    state: { status: 'AUTHENTICATED', user: { id: 'guide-user', username: 'writer', role: 'USER' } }
  })
}));

vi.mock('../src/features/files/hooks/useIssues', async () => {
  const { useCallback, useState } = await import('react');
  const issues = [
    { code: 'ISSUE-1', can_write: true, owner_username: 'writer' },
    { code: 'ISSUE-2', can_write: true, owner_username: 'writer' }
  ];
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
        filteredIssues: issues,
        hasMoreIssues: false,
        issueError: null,
        issueScope: 'all',
        issueSearchText: '',
        issues,
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

vi.mock('../src/features/files/hooks/useUploadTask', async () => {
  const { useCallback, useState } = await import('react');
  type TestUploadTask = {
    id: string;
    issueCode: string;
    file: File;
    name: string;
    sizeBytes: number;
    status: 'FAILED' | 'QUEUED' | 'ACCEPTED';
    progressPercent: number;
    message: string | null;
    response: null | { bundle_hash: string; status: 'PENDING' };
  };
  return {
    useUploadTask: () => {
      const [tasks, setTasks] = useState<TestUploadTask[]>([]);
      testMocks.setUploadTasks.mockImplementation((nextTasks: TestUploadTask[]) => setTasks(nextTasks));
      const performUpload = useCallback(async () => {
        setTasks([{
          id: 'retry-task', issueCode: 'ISSUE-1', file: new File(['log'], 'logs.zip'), name: 'logs.zip',
          sizeBytes: 3, status: 'FAILED', progressPercent: 0, message: 'network error', response: null
        }]);
      }, []);
      const retryUpload = useCallback((taskId: string) => {
        setTasks((current) => current.map((task) => task.id === taskId
          ? { ...task, status: 'QUEUED', message: null }
          : task));
      }, []);
      return {
        acknowledgeAcceptedTasks: useCallback(() => undefined, []),
        performUpload,
        resetSelection: useCallback(() => undefined, []),
        retryUpload,
        tasks,
        uploadDisabled: false,
        uploadError: null,
        uploading: tasks.some((task) => task.status === 'QUEUED')
      };
    }
  };
});

vi.mock('../src/features/files/components/ConfirmDialog', () => ({ ConfirmDialog: () => null }));
vi.mock('../src/features/files/components/IssueCreateDialog', () => ({ IssueCreateDialog: () => null }));
vi.mock('../src/features/files/components/IssueExpirationNotice', () => ({ IssueExpirationNotice: () => null }));
vi.mock('../src/features/files/components/IssueDeleteButton', () => ({ IssueDeleteButton: () => null }));
vi.mock('../src/features/files/components/UploadFileTable', () => ({
  UploadFileTable: ({
    fileRows,
    onRetryUpload
  }: {
    fileRows: Array<{ uploadTaskId?: string; stage: string }>;
    onRetryUpload: (taskId: string) => void;
  }) => (
    <div>
      {fileRows.filter((row) => row.uploadTaskId && row.stage === 'FAILED').map((row) => (
        <button key={row.uploadTaskId} type="button" onClick={() => onRetryUpload(row.uploadTaskId!)}>重试失败上传</button>
      ))}
    </div>
  )
}));
vi.mock('../src/features/files/components/UploadPanel', () => ({
  UploadPanel: ({
    highlightUpload,
    onFilesSelected
  }: {
    highlightUpload: boolean;
    onFilesSelected: (files: File[]) => void;
  }) => <div>{highlightUpload ? <><span>上传操作已就绪</span><button type="button" onClick={() => onFilesSelected([new File(['log'], 'logs.zip')])}>模拟选择上传文件</button></> : null}</div>
}));
vi.mock('../src/features/files/components/IssueSelector', () => ({
  IssueSelector: ({ onSelectIssue }: { onSelectIssue: (code: string) => void }) => (
    <div>
      <button type="button" onClick={() => onSelectIssue('ISSUE-1')}>选择测试 Issue</button>
      <button type="button" onClick={() => onSelectIssue('ISSUE-2')}>选择含文件 Issue</button>
    </div>
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
    testMocks.setUploadTasks.mockReset();
    testMocks.fetchIssueBundles.mockResolvedValue({
      log_bundles: [], can_write: true, owner_username: 'writer', inactivity_expiry: null
    });
    testMocks.fetchFileNode.mockResolvedValue({ children: [] });
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

  it('rechecks files after switching Issues from the empty delete guide', async () => {
    testMocks.fetchIssueBundles.mockImplementation(async (issueCode: string) => issueCode === 'ISSUE-2'
      ? {
        log_bundles: [{ hash: 'bundle-2', name: 'logs.zip', stage: 'READY', status: { upload_status: 'READY' } }],
        can_write: true, owner_username: 'writer', inactivity_expiry: null
      }
      : { log_bundles: [], can_write: true, owner_username: 'writer', inactivity_expiry: null });
    testMocks.fetchFileNode.mockResolvedValue({
      children: [{
        id: 17, name: 'service.log', path: '/service.log', is_dir: false,
        preview_kind: 'text', size_bytes: 3, meta: { kind: 'uploaded_file' }
      }]
    });
    renderHomeView();

    fireEvent.click(screen.getByRole('button', { name: '删除日志文件' }));
    fireEvent.click(screen.getByRole('button', { name: '选择测试 Issue' }));
    expect(await screen.findByText('当前 Issue 没有可删除的日志文件。先选择一个包含已处理日志的 Issue。')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: '选择含文件 Issue' }));
    expect(await screen.findByText('正在读取 Issue 的权限和文件…')).toBeInTheDocument();
    expect(await screen.findByText('在“文件列表”中找到要删除的日志文件，点击这一行的“删除”。这只会删除选中的文件。')).toBeInTheDocument();
    expect(screen.queryByText('当前 Issue 没有可删除的日志文件。先选择一个包含已处理日志的 Issue。')).not.toBeInTheDocument();
  });

  it('resumes tracking an upload when the failed task is retried', async () => {
    renderHomeView();

    fireEvent.click(screen.getByRole('button', { name: '上传压缩包' }));
    fireEvent.click(screen.getByRole('button', { name: '选择测试 Issue' }));
    expect(await screen.findByText('上传操作已就绪')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '模拟选择上传文件' }));

    expect(await screen.findByText('上传没有成功。查看上传区域的错误信息，然后重试；教程仍可从这里重新打开。')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '重试失败上传' }));
    expect(await screen.findByText('上传已加入任务队列。Rain 会在后台接收、校验、解压并建立索引。')).toBeInTheDocument();
    expect(screen.queryByText('上传没有成功。查看上传区域的错误信息，然后重试；教程仍可从这里重新打开。')).not.toBeInTheDocument();

    act(() => testMocks.setUploadTasks([{
      id: 'retry-task', issueCode: 'ISSUE-1', file: new File(['log'], 'logs.zip'), name: 'logs.zip',
      sizeBytes: 3, status: 'ACCEPTED', progressPercent: 100, message: null,
      response: { bundle_hash: 'accepted-bundle', status: 'PENDING' }
    }]));
    expect(await screen.findByText('服务器已接收压缩包，后台会继续校验、解压并建立索引。')).toBeInTheDocument();
  });
});
