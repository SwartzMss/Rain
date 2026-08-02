import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it, vi } from 'vitest';
import { AuthProvider } from '../src/auth/AuthContext';
import { AdminSettingsPage, AdminUsersPage } from '../src/features/admin/AdminPage';
import { rainApi } from '../src/api/client';

vi.mock('../src/api/client', () => ({
  normalizeApiError: (error: unknown) => error instanceof Error ? error.message : String(error),
  rainApi: { me: vi.fn(), fetchAdminUsers: vi.fn(), fetchAdminSettings: vi.fn(), updateAdminSettings: vi.fn() }
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
  const toggle = await screen.findByRole('button');
  await waitFor(() => expect(toggle).toBeDisabled());
  expect(screen.getByText('settings unavailable')).toBeInTheDocument();
});
