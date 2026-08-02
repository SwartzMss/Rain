import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { IssueSelector } from '../src/features/files/components/IssueSelector';
import { UploadFileTable } from '../src/features/files/components/UploadFileTable';
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
  issuesError: null, issuesLoading: false, canWrite, onCreateClick: vi.fn(),
  onIssueSearchTextChange: vi.fn(), onRefreshIssues: vi.fn(), onSelectIssue: vi.fn(), onViewIssue: vi.fn()
});

describe('write permission behavior', () => {
  it('shows new Issue only for a writable user', () => {
    const { rerender } = render(<IssueSelector {...issueProps(false)} />);
    expect(screen.queryByRole('button', { name: /新建 Issue/ })).not.toBeInTheDocument();
    rerender(<IssueSelector {...issueProps(true)} />);
    expect(screen.getByRole('button', { name: /新建 Issue/ })).toBeInTheDocument();
  });

  it('shows file deletion only for a writable authenticated user', async () => {
    vi.mocked(rainApi.me).mockResolvedValue({ authenticated: true, user: { id: 'u', username: 'u', role: 'USER' } });
    render(<AuthProvider><UploadFileTable bundlesError={null} currentIssueCode="ISSUE" deletingKey={null} fileRows={[row]} canWrite={false} onDeleteRow={vi.fn()} /></AuthProvider>);
    expect(await screen.findByText('文件列表')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '删除' })).not.toBeInTheDocument();
  });
});
