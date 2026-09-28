import { Link, Navigate, Route, Routes, useLocation } from 'react-router-dom';
import { useAuth } from './auth/AuthContext';
import { AuthPage } from './features/auth/AuthPage';
import { AccountPage } from './features/auth/AccountPage';
import { BundleView } from './features/files/FilesView';
import { HomeView } from './features/files/HomeView';
import { TempResultRoute } from './features/files/TempResultView';
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
    <div className="flex min-h-screen flex-col text-slate-900">
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

      <main className="mx-auto w-full max-w-none flex-1 px-5 py-5">
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
          <Route path="/temp-results/:resultId" element={<TempResultRoute />} />
        </Routes>
      </main>

      <footer className="border-t border-white/10 bg-slate-950 text-slate-300">
        <div className="mx-auto flex w-full max-w-none flex-col items-center px-5 py-2 text-center text-sm sm:px-6">
          <nav aria-label="页脚导航" className="flex flex-wrap items-center justify-center gap-1">
              <a
                aria-label="联系作者（swartz_lubel@outlook.com）"
                className="inline-flex items-center gap-2 rounded-lg px-2.5 py-1.5 text-slate-300 no-underline transition hover:bg-white/10 hover:text-cyan-200"
                href="mailto:swartz_lubel@outlook.com"
                title="swartz_lubel@outlook.com"
              >
                <MailIcon />
                联系作者
              </a>
              <a
                aria-label="GitHub 仓库（新窗口打开）"
                className="inline-flex items-center gap-2 rounded-lg px-2.5 py-1.5 text-cyan-300 no-underline transition hover:bg-white/10 hover:text-cyan-200"
                href="https://github.com/SwartzMss/Rain"
                target="_blank"
                rel="noreferrer"
              >
                <GithubIcon />
                GitHub 仓库
                <ExternalLinkIcon />
              </a>
              <a
                aria-label="报告问题（新窗口打开）"
                className="inline-flex items-center gap-2 rounded-lg px-2.5 py-1.5 text-slate-300 no-underline transition hover:bg-white/10 hover:text-cyan-200"
                href="https://github.com/SwartzMss/Rain/issues"
                target="_blank"
                rel="noreferrer"
              >
                <IssueIcon />
                报告问题
                <ExternalLinkIcon />
              </a>
          </nav>
        </div>
      </footer>
    </div>
  );
}

function MailIcon() {
  return (
    <svg aria-hidden="true" className="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="1.8">
      <path strokeLinecap="round" strokeLinejoin="round" d="M3.75 6.75h16.5v10.5H3.75z" />
      <path strokeLinecap="round" strokeLinejoin="round" d="m4.5 7.5 7.5 6 7.5-6" />
    </svg>
  );
}

function GithubIcon() {
  return (
    <svg aria-hidden="true" className="h-4 w-4" fill="currentColor" viewBox="0 0 24 24">
      <path d="M12 .75a11.25 11.25 0 0 0-3.56 21.92c.56.1.77-.24.77-.54v-2.1c-3.14.68-3.8-1.33-3.8-1.33-.51-1.3-1.25-1.65-1.25-1.65-1.02-.7.08-.69.08-.69 1.13.08 1.73 1.16 1.73 1.16 1 1.72 2.62 1.23 3.26.94.1-.73.39-1.23.71-1.51-2.5-.28-5.13-1.25-5.13-5.56 0-1.23.44-2.23 1.16-3.02-.12-.28-.5-1.43.11-2.98 0 0 .95-.3 3.1 1.15a10.74 10.74 0 0 1 5.64 0c2.15-1.45 3.1-1.15 3.1-1.15.61 1.55.23 2.7.11 2.98.72.79 1.16 1.79 1.16 3.02 0 4.32-2.63 5.27-5.14 5.55.4.35.76 1.04.76 2.1v3.1c0 .3.2.65.78.54A11.25 11.25 0 0 0 12 .75Z" />
    </svg>
  );
}

function IssueIcon() {
  return (
    <svg aria-hidden="true" className="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="1.8">
      <path strokeLinecap="round" strokeLinejoin="round" d="M12 3.75a8.25 8.25 0 1 0 8.25 8.25A8.25 8.25 0 0 0 12 3.75Z" />
      <path strokeLinecap="round" strokeLinejoin="round" d="M12 8.25v4.5m0 3h.008" />
    </svg>
  );
}

function ExternalLinkIcon() {
  return (
    <svg aria-hidden="true" className="h-3.5 w-3.5 opacity-70" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="1.8">
      <path strokeLinecap="round" strokeLinejoin="round" d="M14.25 4.5h5.25v5.25M19.25 4.75 12 12m7.5-2.25v6.75a2.25 2.25 0 0 1-2.25 2.25H7.5a2.25 2.25 0 0 1-2.25-2.25V7.5A2.25 2.25 0 0 1 7.5 5.25h6.75" />
    </svg>
  );
}

export default App;
