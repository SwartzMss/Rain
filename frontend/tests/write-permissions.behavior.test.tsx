import { createRef } from 'react';
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { IssueSelector } from '../src/features/files/components/IssueSelector';
import { UploadFileTable } from '../src/features/files/components/UploadFileTable';
import { UploadPanel } from '../src/features/files/components/UploadPanel';
import { IssueDeleteButton } from '../src/features/files/components/IssueDeleteButton';
import { AuthProvider } from '../src/auth/AuthContext';
import { rainApi } from '../src/api/client';

vi.mock('../src/api/client', () => ({
  rainApi: { me: vi.fn(), fileDownloadUrl: vi.fn(() => '/download') }
}));

const row = {
  key: 'bundle:file', bundleHash: 'bundle', bundleName: 'bundle', name: 'file.log',
  status: 'READY' as const, stage: 'READY' as const, sizeBytes: 10
};

const issueProps = (canWrite: boolean) => ({
  currentIssueCode: '', filteredIssues: [], issueError: null, issueSearchText: '',
  issueScope: 'all' as const, issuesError: null, issuesLoading: false, issuesLoadingMore: false,
  hasMoreIssues: false, canChooseScope: false, canWrite, canCreateIssue: canWrite, onCreateClick: vi.fn(),
  onIssueScopeChange: vi.fn(), onIssueSearchTextChange: vi.fn(), onRefreshIssues: vi.fn(), onLoadMoreIssues: vi.fn(), onSelectIssue: vi.fn(), onViewIssue: vi.fn()
});

describe('write permission behavior', () => {
  it('shows new Issue only for a writable user', () => {
    const { rerender } = render(<IssueSelector {...issueProps(false)} />);
    expect(screen.queryByRole('button', { name: /新建 Issue/ })).not.toBeInTheDocument();
    rerender(<IssueSelector {...issueProps(true)} />);
    expect(screen.getByRole('button', { name: /新建 Issue/ })).toBeInTheDocument();
  });

  it('switches between my Issues and all Issues', () => {
    const onIssueScopeChange = vi.fn();
    render(<IssueSelector {...issueProps(false)} canChooseScope issueScope="mine" onIssueScopeChange={onIssueScopeChange} />);
    expect(screen.getByRole('tab', { name: '我的' })).toHaveAttribute('aria-selected', 'true');
    fireEvent.click(screen.getByRole('tab', { name: '全部' }));
    expect(onIssueScopeChange).toHaveBeenCalledWith('all');
  });

  it('shows Issue names and owners in the Issue list', () => {
    const props = { ...issueProps(false), filteredIssues: [
      { code: 'OWNED', name: 'Owned', bundle_count: 0, can_write: false, owner_username: 'owner' },
      { code: 'UNKNOWN', name: 'Unknown', bundle_count: 0, can_write: false, owner_username: null }
    ] };
    render(<IssueSelector {...props} />);
    expect(screen.getByText('Owned')).toBeInTheDocument();
    expect(screen.getByText('所有者：owner')).toBeInTheDocument();
  });

  it('keeps double-click navigation for an Issue', () => {
    const onViewIssue = vi.fn();
    render(<IssueSelector {...issueProps(false)} filteredIssues={[{ code: 'ISSUE', name: 'Issue', bundle_count: 0, can_write: false, owner_username: 'owner' }]} onViewIssue={onViewIssue} />);
    fireEvent.doubleClick(screen.getByRole('button', { name: /ISSUE/ }));
    expect(onViewIssue).toHaveBeenCalledWith('ISSUE');
  });

  it('shows file deletion only for a writable authenticated user', async () => {
    vi.mocked(rainApi.me).mockResolvedValue({ authenticated: true, user: { id: 'u', username: 'u', role: 'USER' } });
    const { rerender } = render(<AuthProvider><UploadFileTable bundlesError={null} currentIssueCode="ISSUE" deletingKey={null} fileRows={[row]} canWrite={false} onDeleteRow={vi.fn()} /></AuthProvider>);
    expect(await screen.findByText('文件列表')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '删除' })).not.toBeInTheDocument();
    rerender(<AuthProvider><UploadFileTable bundlesError={null} currentIssueCode="ISSUE" deletingKey={null} fileRows={[row]} canWrite onDeleteRow={vi.fn()} /></AuthProvider>);
    expect(screen.getByRole('button', { name: '删除' })).toBeInTheDocument();
  });

  it('hides file deletion while a Bundle is processing and shows the reason', async () => {
    vi.mocked(rainApi.me).mockResolvedValue({ authenticated: true, user: { id: 'u', username: 'u', role: 'USER' } });
    const processingRow = {
      ...row,
      status: 'PROCESSING' as const,
      stage: 'EXTRACTING' as const
    };
    render(<AuthProvider><UploadFileTable bundlesError={null} currentIssueCode="ISSUE" deletingKey={null} fileRows={[processingRow]} canWrite onDeleteRow={vi.fn()} /></AuthProvider>);
    expect(await screen.findByText('解压中')).toBeInTheDocument();
    expect(await screen.findByText('处理中，暂不可删除')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '删除' })).not.toBeInTheDocument();
  });

  it('keeps Issue deletion disabled without exposing a processing message', () => {
    const onDelete = vi.fn();
    const { rerender } = render(<IssueDeleteButton issueCode="ISSUE" canWrite blocked deleting={false} onDelete={onDelete} />);
    expect(screen.getByRole('button', { name: '删除 Issue' })).toBeDisabled();
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '删除 Issue' }));
    expect(onDelete).not.toHaveBeenCalled();

    rerender(<IssueDeleteButton issueCode="ISSUE" canWrite={true} blocked={false} deleting={false} onDelete={onDelete} />);
    expect(screen.getByRole('button', { name: '删除 Issue' })).toBeEnabled();
    fireEvent.click(screen.getByRole('button', { name: '删除 Issue' }));
    expect(onDelete).toHaveBeenCalledTimes(1);
  });

  it('disables upload selection for guests and enables it for writable users', () => {
    const props = {
      currentIssueCode: 'ISSUE', fileInputRef: createRef<HTMLInputElement>(),
      onFilesSelected: vi.fn(), uploadDisabled: false, uploadError: null, uploading: false,
      uploadingRef: createRef<boolean>()
    };
    const { rerender } = render(<UploadPanel {...props} canWrite={false} />);
    expect(screen.getByRole('button', { name: '选择文件' })).toBeDisabled();
    rerender(<UploadPanel {...props} canWrite />);
    expect(screen.getByRole('button', { name: '选择文件' })).toBeEnabled();
  });
});
