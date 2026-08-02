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
