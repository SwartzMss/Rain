import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { AuthProvider } from '../src/auth/AuthContext';
import { rainApi } from '../src/api/client';
import App from '../src/App';

vi.mock('../src/api/client', () => ({
  rainApi: { me: vi.fn(), login: vi.fn(), register: vi.fn(), logout: vi.fn(), changePassword: vi.fn() }
}));

vi.mock('../src/features/auth/AuthPage', () => ({ AuthPage: () => <p>auth page</p> }));
vi.mock('../src/features/auth/AccountPage', () => ({ AccountPage: () => <p>account page</p> }));
vi.mock('../src/features/files/FilesView', () => ({ BundleView: () => <p>home page</p> }));
vi.mock('../src/features/files/HomeView', () => ({ HomeView: () => <p>home page</p> }));
vi.mock('../src/features/files/TempResultView', () => ({ TempResultRoute: () => <p>temp result</p> }));
vi.mock('../src/features/admin/AdminPage', () => ({
  AdminPage: () => <p>admin page</p>,
  AdminUsersPage: () => <p>admin users page</p>,
  AuditLogsPage: () => <p>audit logs</p>,
  AdminSettingsPage: () => <p>admin settings</p>,
  AuthRateLimitsPage: () => <p>auth rate limits</p>
}));

function renderApp(path = '/') {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <AuthProvider><App /></AuthProvider>
    </MemoryRouter>
  );
}

describe('application behavior', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 200 })));
  });

  it('shows guest navigation and public home after an unauthenticated refresh', async () => {
    vi.mocked(rainApi.me).mockResolvedValueOnce({ authenticated: false, user: null });
    renderApp();
    await waitFor(() => expect(screen.getByText('访客模式')).toBeInTheDocument());
    expect(screen.getByRole('link', { name: '登录' })).toHaveAttribute('href', '/login');
    expect(screen.getByRole('link', { name: '注册' })).toHaveAttribute('href', '/register');
    expect(screen.getByText('服务正常')).toBeInTheDocument();
  });

  it('keeps the readiness indicator in checking state until the request settles', async () => {
    let resolveReadiness!: (response: Response) => void;
    vi.stubGlobal('fetch', vi.fn().mockImplementation(() => new Promise<Response>((resolve) => {
      resolveReadiness = resolve;
    })));
    vi.mocked(rainApi.me).mockResolvedValueOnce({ authenticated: false, user: null });
    renderApp();
    expect(screen.getByText('检测中')).toBeInTheDocument();
    resolveReadiness(new Response(null, { status: 200 }));
    await waitFor(() => expect(screen.getByText('服务正常')).toBeInTheDocument());
  });

  it('redirects an administrator from home to admin users and hides the account link', async () => {
    vi.mocked(rainApi.me).mockResolvedValueOnce({
      authenticated: true,
      user: { id: 'admin', username: 'root', role: 'ADMIN' }
    });
    renderApp();
    expect(await screen.findByText('admin users page')).toBeInTheDocument();
    expect(screen.getByText('root')).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: '账户' })).not.toBeInTheDocument();
  });

  it('shows a normal user account entry and reports readiness failure', async () => {
    vi.mocked(rainApi.me).mockResolvedValueOnce({
      authenticated: true,
      user: { id: 'user', username: 'alice', role: 'USER' }
    });
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 503 })));
    renderApp();
    await waitFor(() => expect(screen.getByText('alice')).toBeInTheDocument());
    expect(screen.getByRole('link', { name: '账户' })).toHaveAttribute('href', '/account');
    expect(screen.getByText('服务异常')).toBeInTheDocument();
  });
});
