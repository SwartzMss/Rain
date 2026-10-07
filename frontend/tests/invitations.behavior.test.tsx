import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { rainApi } from '../src/api/client';
import { InvitationsPage } from '../src/features/admin/InvitationsPage';

vi.mock('../src/api/client', () => ({
  normalizeApiError: (error: unknown) => error instanceof Error ? error.message : String(error),
  rainApi: { fetchInvitations: vi.fn(), createInvitations: vi.fn(), revokeInvitation: vi.fn() }
}));

const emptyPage = { items: [], next_cursor: null };
const activeInvitation = {
  id: 'invite-1', batch_id: 'batch-1', note: 'test invite', created_by_username: 'admin',
  created_at: '2026-10-07 00:00:00', expires_at: null, status: 'ACTIVE' as const,
  used_by_username: null, used_at: null
};

describe('invitation administration behavior', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(rainApi.fetchInvitations).mockResolvedValue(emptyPage);
  });

  it('prevents duplicate submission and removes one-time codes when the dialog closes', async () => {
    let resolveCreate!: (value: { batch_id: string; invitations: Array<{ id: string; code: string; expires_at: null }> }) => void;
    vi.mocked(rainApi.createInvitations).mockReturnValueOnce(new Promise((resolve) => { resolveCreate = resolve; }));
    const user = userEvent.setup();
    render(<InvitationsPage />);
    await screen.findByText('暂无邀请码记录');
    await user.clear(screen.getByLabelText('生成数量'));
    await user.type(screen.getByLabelText('生成数量'), '2');
    await user.type(screen.getByLabelText('备注'), '测试批次');

    await user.click(screen.getByRole('button', { name: '生成邀请码' }));
    expect(screen.getByRole('button', { name: '正在生成…' })).toBeDisabled();
    expect(rainApi.createInvitations).toHaveBeenCalledTimes(1);
    expect(rainApi.createInvitations).toHaveBeenCalledWith({ count: 2, validity_days: 7, note: '测试批次' });

    await act(async () => resolveCreate({
      batch_id: 'batch-1',
      invitations: [{ id: 'invite-1', code: 'RAIN-SECRET-CODE', expires_at: null }]
    }));
    expect(await screen.findByRole('dialog')).toBeInTheDocument();
    expect(screen.getByText('RAIN-SECRET-CODE')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '我已保存，关闭' }));
    expect(screen.queryByText('RAIN-SECRET-CODE')).not.toBeInTheDocument();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('offers a manual copy fallback when clipboard access fails', async () => {
    vi.mocked(rainApi.createInvitations).mockResolvedValueOnce({
      batch_id: 'batch-1',
      invitations: [{ id: 'invite-1', code: 'RAIN-SECRET-CODE', expires_at: null }]
    });
    const user = userEvent.setup();
    render(<InvitationsPage />);
    await user.click(await screen.findByRole('button', { name: '生成邀请码' }));
    await screen.findByRole('dialog');
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: vi.fn().mockRejectedValue(new Error('clipboard unavailable')) }
    });
    await user.click(screen.getByRole('button', { name: '复制' }));
    expect(await screen.findByText('无法访问剪贴板，请手动选择并复制下方邀请码')).toBeInTheDocument();
  });

  it('refreshes the list after a revoke conflict and keeps the error visible', async () => {
    vi.mocked(rainApi.fetchInvitations)
      .mockResolvedValueOnce({ items: [activeInvitation], next_cursor: null })
      .mockResolvedValueOnce({ items: [{ ...activeInvitation, status: 'USED' }], next_cursor: null });
    vi.mocked(rainApi.revokeInvitation).mockRejectedValueOnce(new Error('邀请码已使用'));
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    const user = userEvent.setup();
    render(<InvitationsPage />);
    await screen.findByText('test invite');
    await user.click(screen.getByRole('button', { name: '撤销' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('邀请码已使用');
    expect(await screen.findAllByText('已使用')).toHaveLength(2);
    expect(rainApi.fetchInvitations).toHaveBeenCalledTimes(2);
  });
});
