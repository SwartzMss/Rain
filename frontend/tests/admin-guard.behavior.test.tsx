import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it, vi } from 'vitest';
import { AuthProvider } from '../src/auth/AuthContext';
import { AdminSettingsPage, AdminUsersPage, AuthRateLimitsPage } from '../src/features/admin/AdminPage';
import { rainApi } from '../src/api/client';

vi.mock('../src/api/client', () => ({
  normalizeApiError: (error: unknown) => error instanceof Error ? error.message : String(error),
  rainApi: { me: vi.fn(), fetchAdminUsers: vi.fn(), fetchAdminSettings: vi.fn(), updateAdminSettings: vi.fn(), fetchAuthRateLimits: vi.fn(), clearAuthRateLimit: vi.fn(), clearAllAuthRateLimits: vi.fn() }
}));

it('shows a forbidden state when a normal user opens the admin route directly', async () => {
  vi.mocked(rainApi.me).mockResolvedValueOnce({ authenticated: true, user: { id: 'u', username: 'alice', role: 'USER' } });
  render(<MemoryRouter initialEntries={['/admin/users']}><AuthProvider><AdminUsersPage /></AuthProvider></MemoryRouter>);
  await waitFor(() => expect(screen.getByText('此页面需要管理员权限。')).toBeInTheDocument());
});

it('keeps the registration switch disabled when settings loading fails', async () => {
  vi.mocked(rainApi.me).mockResolvedValueOnce({ authenticated: true, user: { id: 'a', username: 'admin', role: 'ADMIN' } });
  vi.mocked(rainApi.fetchAdminSettings).mockRejectedValueOnce(new Error('settings unavailable'));
  render(<MemoryRouter initialEntries={['/admin/settings']}><AuthProvider><AdminSettingsPage /></AuthProvider></MemoryRouter>);
  const toggle = (await screen.findAllByRole('button'))[0];
  await waitFor(() => expect(toggle).toBeDisabled());
  expect(screen.getByText('settings unavailable')).toBeInTheDocument();
});

it('saves rate limit thresholds without changing registration state', async () => {
  vi.mocked(rainApi.me).mockResolvedValueOnce({ authenticated: true, user: { id: 'a', username: 'admin', role: 'ADMIN' } });
  vi.mocked(rainApi.fetchAdminSettings).mockResolvedValueOnce({ allow_registration: true, updated_at: '', updated_by_username: 'admin', login_ip_limit_per_minute: 20, login_username_failure_limit_per_5_minutes: 10, issue_inactive_days: 0 });
  vi.mocked(rainApi.updateAdminSettings).mockResolvedValueOnce({ allow_registration: true, updated_at: '', updated_by_username: 'admin', login_ip_limit_per_minute: 50, login_username_failure_limit_per_5_minutes: 15, issue_inactive_days: 0 });
  render(<MemoryRouter initialEntries={['/admin/settings']}><AuthProvider><AdminSettingsPage /></AuthProvider></MemoryRouter>);
  await screen.findByDisplayValue('20');
  await userEvent.clear(screen.getByLabelText('IP 每分钟阈值'));
  await userEvent.type(screen.getByLabelText('IP 每分钟阈值'), '50');
  await userEvent.clear(screen.getByLabelText('用户名失败 5 分钟阈值'));
  await userEvent.type(screen.getByLabelText('用户名失败 5 分钟阈值'), '15');
  await userEvent.click(screen.getByRole('button', { name: '保存限流配置' }));
  await waitFor(() => expect(rainApi.updateAdminSettings).toHaveBeenCalledWith(undefined, 50, 15));
  expect(screen.getByText('设置已保存')).toBeInTheDocument();
});

it('saves issue inactivity independently', async () => {
  vi.mocked(rainApi.me).mockResolvedValueOnce({ authenticated: true, user: { id: 'a', username: 'admin', role: 'ADMIN' } });
  vi.mocked(rainApi.fetchAdminSettings).mockResolvedValueOnce({ allow_registration: true, updated_at: '', updated_by_username: 'admin', login_ip_limit_per_minute: 20, login_username_failure_limit_per_5_minutes: 10, issue_inactive_days: 0 });
  vi.mocked(rainApi.updateAdminSettings).mockResolvedValueOnce({ allow_registration: true, updated_at: '', updated_by_username: 'admin', login_ip_limit_per_minute: 20, login_username_failure_limit_per_5_minutes: 10, issue_inactive_days: 30 });
  render(<MemoryRouter initialEntries={['/admin/settings']}><AuthProvider><AdminSettingsPage /></AuthProvider></MemoryRouter>);
  const input = await screen.findByLabelText('非活跃天数');
  await userEvent.clear(input);
  await userEvent.type(input, '30');
  await userEvent.click(screen.getByRole('button', { name: '保存 Issue 过期配置' }));
  await waitFor(() => expect(rainApi.updateAdminSettings).toHaveBeenCalledWith(undefined, undefined, undefined, 30));
  expect(screen.getByText('Issue 过期配置已保存')).toBeInTheDocument();
});

it('accepts zero and blocks empty or out-of-range issue inactivity values', async () => {
  vi.mocked(rainApi.me).mockResolvedValueOnce({ authenticated: true, user: { id: 'a', username: 'admin', role: 'ADMIN' } });
  vi.mocked(rainApi.fetchAdminSettings).mockResolvedValueOnce({ allow_registration: true, updated_at: '', updated_by_username: 'admin', login_ip_limit_per_minute: 20, login_username_failure_limit_per_5_minutes: 10, issue_inactive_days: 0 });
  vi.mocked(rainApi.updateAdminSettings).mockResolvedValueOnce({ allow_registration: true, updated_at: '', updated_by_username: 'admin', login_ip_limit_per_minute: 20, login_username_failure_limit_per_5_minutes: 10, issue_inactive_days: 0 });
  render(<MemoryRouter initialEntries={['/admin/settings']}><AuthProvider><AdminSettingsPage /></AuthProvider></MemoryRouter>);
  const input = await screen.findByLabelText('非活跃天数');
  const save = screen.getByRole('button', { name: '保存 Issue 过期配置' });
  await userEvent.click(save);
  await waitFor(() => expect(rainApi.updateAdminSettings).toHaveBeenCalledWith(undefined, undefined, undefined, 0));

  await userEvent.clear(input);
  expect(save).toBeDisabled();
  await userEvent.type(input, '31');
  expect(save).toBeDisabled();
});

it('reloads the persisted issue inactivity value after a save failure', async () => {
  vi.mocked(rainApi.me).mockResolvedValueOnce({ authenticated: true, user: { id: 'a', username: 'admin', role: 'ADMIN' } });
  const settings = { allow_registration: true, updated_at: '', updated_by_username: 'admin', login_ip_limit_per_minute: 20, login_username_failure_limit_per_5_minutes: 10, issue_inactive_days: 12 };
  vi.mocked(rainApi.fetchAdminSettings).mockResolvedValueOnce(settings).mockResolvedValueOnce(settings);
  vi.mocked(rainApi.updateAdminSettings).mockRejectedValueOnce(new Error('save failed'));
  render(<MemoryRouter initialEntries={['/admin/settings']}><AuthProvider><AdminSettingsPage /></AuthProvider></MemoryRouter>);
  const input = await screen.findByLabelText('非活跃天数');
  await userEvent.clear(input);
  await userEvent.type(input, '20');
  await userEvent.click(screen.getByRole('button', { name: '保存 Issue 过期配置' }));
  expect(await screen.findByText(/save failed/)).toBeInTheDocument();
  await waitFor(() => expect(input).toHaveValue(12));
});

it('loads rate limit records and clears a selected record after confirmation', async () => {
  vi.mocked(rainApi.me).mockResolvedValueOnce({ authenticated: true, user: { id: 'a', username: 'admin', role: 'ADMIN' } });
  vi.mocked(rainApi.fetchAuthRateLimits).mockResolvedValue({
    username_failures: [{ key: 'login:username:alice', username: 'alice', ip: null, current_count: 10, limit: 10, window_seconds: 300, last_event_at: 'now', retry_after_seconds: 20, limited: true }],
    login_ips: [{ key: 'login:ip:127.0.0.1', username: null, ip: '127.0.0.1', current_count: 1, limit: 20, window_seconds: 60, last_event_at: 'now', retry_after_seconds: 0, limited: false }]
  });
  vi.mocked(rainApi.clearAuthRateLimit).mockResolvedValue(undefined);
  vi.spyOn(window, 'confirm').mockReturnValue(true);
  render(<MemoryRouter initialEntries={['/admin/auth-rate-limits']}><AuthProvider><AuthRateLimitsPage /></AuthProvider></MemoryRouter>);
  expect(await screen.findByText('alice')).toBeInTheDocument();
  expect(screen.getByText('127.0.0.1')).toBeInTheDocument();
  expect(screen.getAllByText('—')).toHaveLength(1);
  await userEvent.click(screen.getAllByRole('button', { name: '清除' })[0]);
  expect(rainApi.clearAuthRateLimit).toHaveBeenCalledWith('usernames', 'login:username:alice');
});
