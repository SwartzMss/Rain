import { render, screen, waitFor } from '@testing-library/react';
import { beforeEach, it, expect, vi } from 'vitest';
import { AuthProvider, useAuth } from '../src/auth/AuthContext';
import { rainApi } from '../src/api/client';

function Probe() {
  const auth = useAuth();
  return <><output data-testid="status">{auth.state.status}</output><button onClick={() => void rainApi.logout().catch(() => undefined)}>request</button></>;
}

beforeEach(() => vi.restoreAllMocks());

it('revalidates AuthProvider after the real client receives a target 401', async () => {
  const fetchMock = vi.fn()
    .mockResolvedValueOnce(new Response(JSON.stringify({ authenticated: false, user: null }), { status: 200 }))
    .mockResolvedValueOnce(new Response(JSON.stringify({ code: 'AUTHENTICATION_REQUIRED', message: 'login required' }), { status: 401 }))
    .mockResolvedValueOnce(new Response(JSON.stringify({ authenticated: true, user: { id: 'u', username: 'alice', role: 'USER' } }), { status: 200 }));
  vi.stubGlobal('fetch', fetchMock);
  const user = (await import('@testing-library/user-event')).default.setup();
  render(<AuthProvider><Probe /></AuthProvider>);
  await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('GUEST'));
  await user.click(screen.getByRole('button', { name: 'request' }));
  await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('AUTHENTICATED'));
  expect(fetchMock).toHaveBeenCalledTimes(3);
});

it('revalidates AuthProvider after the real client receives ADMIN_REQUIRED 403', async () => {
  const fetchMock = vi.fn()
    .mockResolvedValueOnce(new Response(JSON.stringify({ authenticated: false, user: null }), { status: 200 }))
    .mockResolvedValueOnce(new Response(JSON.stringify({ code: 'ADMIN_REQUIRED', message: 'admin required' }), { status: 403 }))
    .mockResolvedValueOnce(new Response(JSON.stringify({ authenticated: true, user: { id: 'u', username: 'alice', role: 'USER' } }), { status: 200 }));
  vi.stubGlobal('fetch', fetchMock);
  const user = (await import('@testing-library/user-event')).default.setup();
  render(<AuthProvider><Probe /></AuthProvider>);
  await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('GUEST'));
  await user.click(screen.getByRole('button', { name: 'request' }));
  await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('AUTHENTICATED'));
  expect(fetchMock).toHaveBeenCalledTimes(3);
});
