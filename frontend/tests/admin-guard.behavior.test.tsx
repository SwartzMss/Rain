import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it, vi } from 'vitest';
import { AuthProvider } from '../src/auth/AuthContext';
import { AdminUsersPage } from '../src/features/admin/AdminPage';
import { rainApi } from '../src/api/client';

vi.mock('../src/api/client', () => ({
  rainApi: { me: vi.fn(), fetchAdminUsers: vi.fn() }
}));

it('shows a forbidden state when a normal user opens the admin route directly', async () => {
  vi.mocked(rainApi.me).mockResolvedValueOnce({ authenticated: true, user: { id: 'u', username: 'alice', role: 'USER' } });
  render(<MemoryRouter initialEntries={['/admin/users']}><AuthProvider><AdminUsersPage /></AuthProvider></MemoryRouter>);
  await waitFor(() => expect(screen.getByText('此页面需要管理员权限。')).toBeInTheDocument());
});
