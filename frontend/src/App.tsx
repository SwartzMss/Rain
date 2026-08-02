import { Link, Navigate, Route, Routes, useLocation } from 'react-router-dom';
import { useAuth } from './auth/AuthContext';
import { AuthPage } from './features/auth/AuthPage';
import { AccountPage } from './features/auth/AccountPage';
import { BundleView } from './features/files/FilesView';
import { HomeView } from './features/files/HomeView';
import { TempResultView } from './features/files/TempResultView';
import { APP_VERSION } from './version';
import './App.css';
import { isAdmin } from './auth/permissions';
import { AdminPage, AdminUsersPage, AuditLogsPage, AdminSettingsPage, AuthRateLimitsPage } from './features/admin/AdminPage';
import { useEffect, useState } from 'react';

function App() {
  const auth = useAuth();
  const location = useLocation();
  const returnPath = `${location.pathname}${location.search}`;
  const [serviceStatus, setServiceStatus] = useState<'checking' | 'healthy' | 'unhealthy'>('checking');

  useEffect(() => {
    let active = true;
    const checkHealth = async () => {
      try {
        const response = await fetch('/readyz', { cache: 'no-store' });
        if (active) setServiceStatus(response.ok ? 'healthy' : 'unhealthy');
      } catch {
        if (active) setServiceStatus('unhealthy');
      }
    };
    void checkHealth();
    const timer = window.setInterval(() => void checkHealth(), 30_000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, []);

  return (
    <div className="min-h-screen text-slate-900">
      <header className="sticky top-0 z-40 border-b border-white/10 bg-slate-950/95 shadow-lg shadow-slate-950/15 backdrop-blur-xl">
        <div className="mx-auto flex h-16 w-full max-w-none items-center justify-between gap-3 px-6">
          <Link to={auth.state.status === 'AUTHENTICATED' && isAdmin(auth.state.user) ? '/admin/users' : '/'} className="text-white no-underline">
            <div className="flex flex-wrap items-center gap-2.5">
              <span className="flex h-9 w-9 items-center justify-center rounded-xl border border-cyan-300/30 bg-gradient-to-br from-cyan-300 to-teal-400 text-lg text-slate-950 shadow-lg shadow-cyan-950/30">☁</span>
              <h1 className="text-2xl font-semibold tracking-tight text-white">Rain</h1>
              <span className="rounded-full border border-cyan-300/25 bg-cyan-300/10 px-2.5 py-0.5 text-[11px] font-semibold tracking-wide text-cyan-200">
                {APP_VERSION}
              </span>
            </div>
          </Link>
          <div className="flex items-center gap-3 text-sm font-medium text-slate-200">
            <div className="flex items-center gap-2 rounded-full border border-white/10 bg-white/5 px-3 py-1.5">
              <span className={`h-2.5 w-2.5 rounded-full ${serviceStatus === 'healthy' ? 'bg-emerald-400 shadow-[0_0_10px_rgba(52,211,153,0.8)]' : serviceStatus === 'checking' ? 'bg-amber-300 shadow-[0_0_10px_rgba(252,211,77,0.8)]' : 'bg-rose-400 shadow-[0_0_10px_rgba(251,113,133,0.8)]'}`} />
              <span>{serviceStatus === 'healthy' ? '服务正常' : serviceStatus === 'checking' ? '检测中' : '服务异常'}</span>
            </div>
            {auth.state.status === 'LOADING' && (
              <span className="rounded-full border border-white/10 px-3 py-1.5 text-slate-400">
                正在确认身份…
              </span>
            )}
            {auth.state.status === 'GUEST' && (
              <>
                <span className="rounded-full border border-amber-300/20 bg-amber-300/10 px-3 py-1.5 text-amber-200">
                  访客模式
                </span>
                <Link
                  className="text-slate-200 no-underline hover:text-white"
                  state={{ from: returnPath }}
                  to="/login"
                >
                  登录
                </Link>
                <Link
                  className="rounded-full bg-cyan-300 px-3 py-1.5 font-semibold text-slate-950 no-underline hover:bg-cyan-200"
                  state={{ from: returnPath }}
                  to="/register"
                >
                  注册
                </Link>
              </>
            )}
            {auth.state.status === 'AUTHENTICATED' && (
              <>
                <span className="rounded-full border border-cyan-300/20 bg-cyan-300/10 px-3 py-1.5 text-cyan-100">
                  {auth.state.user.username}
                </span>
                {!isAdmin(auth.state.user) ? <Link className="text-slate-300 no-underline hover:text-white" to="/account">账户</Link> : null}
                <button
                  className="text-slate-300 hover:text-white"
                  onClick={() => {
                    void auth.logout().catch((error) => {
                      window.alert(error instanceof Error ? error.message : '退出登录失败');
                    });
                  }}
                  type="button"
                >
                  退出登录
                </button>
              </>
            )}
          </div>
        </div>
      </header>

      <main className="mx-auto w-full max-w-none px-5 py-5">
        <Routes>
          <Route path="/" element={auth.state.status === 'AUTHENTICATED' && isAdmin(auth.state.user) ? <Navigate to="/admin/users" replace /> : <HomeView />} />
          <Route path="/login" element={<AuthPage mode="login" />} />
          <Route path="/register" element={<AuthPage mode="register" />} />
          <Route path="/account" element={<AccountPage />} />
          <Route path="/admin" element={<AdminPage />} />
          <Route path="/admin/users" element={<AdminUsersPage />} />
          <Route path="/admin/audit-logs" element={<AuditLogsPage />} />
          <Route path="/admin/settings" element={<AdminSettingsPage />} />
          <Route path="/admin/auth-rate-limits" element={<AuthRateLimitsPage />} />
          <Route path="/issue/:issueCode" element={<BundleView />} />
          <Route path="/issue/:issueCode/bundle/:bundleHash" element={<BundleView />} />
          <Route path="/temp-results/:resultId" element={<TempResultView />} />
        </Routes>
      </main>
    </div>
  );
}

export default App;
