import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { AuthProvider, useAuth } from '../src/auth/AuthContext';
import { rainApi } from '../src/api/client';
import { AuthPage } from '../src/features/auth/AuthPage';

vi.mock('../src/api/client', () => ({
  rainApi: { me: vi.fn(), login: vi.fn(), register: vi.fn(), logout: vi.fn(), changePassword: vi.fn(), fetchRegistrationStatus: vi.fn() }
}));

function AuthProbe() {
  const auth = useAuth();
  return <div>
    <output data-testid="status">{auth.state.status}</output>
    {auth.state.status === 'AUTHENTICATED' && <output>{auth.state.user.username}</output>}
    <button onClick={() => void auth.login({ username: 'alice', password: 'secret' })}>login</button>
    <button onClick={() => void auth.logout()}>logout</button>
  </div>;
}

describe('authentication behavior', () => {
  beforeEach(() => vi.clearAllMocks());

  it('renders guest after an unavailable session and logs in and out through the provider', async () => {
    vi.mocked(rainApi.me).mockRejectedValueOnce(new Error('offline'));
    vi.mocked(rainApi.login).mockResolvedValueOnce({ id: '1', username: 'alice', role: 'USER' });
    vi.mocked(rainApi.logout).mockResolvedValueOnce(undefined);
    const user = userEvent.setup();
    render(<AuthProvider><AuthProbe /></AuthProvider>);
    expect(screen.getByTestId('status')).toHaveTextContent('LOADING');
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('GUEST'));
    await user.click(screen.getByRole('button', { name: 'login' }));
    expect(await screen.findByText('alice')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'logout' }));
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('GUEST'));
  });

  it('revalidates authentication after an authentication-required event', async () => {
    vi.mocked(rainApi.me)
      .mockResolvedValueOnce({ authenticated: false, user: null })
      .mockResolvedValueOnce({ authenticated: true, user: { id: '2', username: 'bob', role: 'ADMIN' } });
    render(<AuthProvider><AuthProbe /></AuthProvider>);
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('GUEST'));
    window.dispatchEvent(new Event('rain:authentication-required'));
    await waitFor(() => expect(screen.getByText('bob')).toBeInTheDocument());
    expect(rainApi.me).toHaveBeenCalledTimes(2);
  });

  it('hides the login registration link and blocks the registration form when disabled', async () => {
    vi.mocked(rainApi.me).mockResolvedValue({ authenticated: false, user: null });
    vi.mocked(rainApi.fetchRegistrationStatus).mockResolvedValue({ allow_registration: false });
    const { rerender } = render(<MemoryRouter><AuthProvider><AuthPage mode="login" /></AuthProvider></MemoryRouter>);
    await waitFor(() => expect(screen.queryByRole('link', { name: '注册' })).not.toBeInTheDocument());
    rerender(<MemoryRouter><AuthProvider><AuthPage mode="register" /></AuthProvider></MemoryRouter>);
    expect(await screen.findByText('注册已关闭')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '注册' })).not.toBeInTheDocument();
  });
});
