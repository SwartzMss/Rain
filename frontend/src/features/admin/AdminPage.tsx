import { useCallback, useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import { Navigate, NavLink } from 'react-router-dom';
import { normalizeApiError, rainApi } from '../../api/client';
import type { AdminUser, AuditLog, UserStatus, AuthRateLimitEntry } from '../../api/types';
import { useAuth } from '../../auth/AuthContext';
import { isAdmin } from '../../auth/permissions';
import {
  advanceCursor,
  currentCursor,
  retreatCursor,
  runAdminAction,
  type CursorHistory
} from './adminFlow';

function AdminShell({ children }: { children: ReactNode }) {
  return (
    <div className="mx-auto max-w-7xl space-y-4">
      <nav className="inline-flex rounded-xl border border-slate-200 bg-white/80 p-1 text-sm shadow-sm">
        <NavLink className={({ isActive }) => `rounded-lg px-4 py-2 font-medium transition ${isActive ? 'bg-sky-600 text-white shadow-sm' : 'text-slate-600 hover:bg-slate-100 hover:text-slate-950'}`} to="/admin/users">用户管理</NavLink>
        <NavLink className={({ isActive }) => `rounded-lg px-4 py-2 font-medium transition ${isActive ? 'bg-sky-600 text-white shadow-sm' : 'text-slate-600 hover:bg-slate-100 hover:text-slate-950'}`} to="/admin/audit-logs">审计日志</NavLink>
        <NavLink className={({ isActive }) => `rounded-lg px-4 py-2 font-medium transition ${isActive ? 'bg-sky-600 text-white shadow-sm' : 'text-slate-600 hover:bg-slate-100 hover:text-slate-950'}`} to="/admin/auth-rate-limits">认证限流</NavLink>
        <NavLink className={({ isActive }) => `rounded-lg px-4 py-2 font-medium transition ${isActive ? 'bg-sky-600 text-white shadow-sm' : 'text-slate-600 hover:bg-slate-100 hover:text-slate-950'}`} to="/admin/settings">系统设置</NavLink>
      </nav>
      {children}
    </div>
  );
}

function AdminGuard({ children }: { children: ReactNode }) {
  const auth = useAuth();
  if (auth.state.status === 'LOADING') return <p>正在确认身份…</p>;
  if (auth.state.status !== 'AUTHENTICATED') return <Navigate to="/login" replace state={{ from: '/admin/users' }} />;
  if (!isAdmin(auth.state.user)) return <section className="panel"><h2 className="text-xl font-semibold">403</h2><p>此页面需要管理员权限。</p></section>;
  return <AdminShell>{children}</AdminShell>;
}

export function AdminPage() {
  return <Navigate to="/admin/users" replace />;
}

export function AdminSettingsPage() {
  const [allowed, setAllowed] = useState(true);
  const [ipLimit, setIpLimit] = useState(20);
  const [usernameLimit, setUsernameLimit] = useState(10);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [hasLoadedSettings, setHasLoadedSettings] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const load = useCallback(async () => { setLoading(true); setLoadError(null); try { const value = await rainApi.fetchAdminSettings(); setAllowed(value.allow_registration); setIpLimit(value.login_ip_limit_per_minute); setUsernameLimit(value.login_username_failure_limit_per_5_minutes); setHasLoadedSettings(true); } catch (e) { setHasLoadedSettings(false); setLoadError(normalizeApiError(e)); } finally { setLoading(false); } }, []);
  useEffect(() => { void load(); }, [load]);
  const save = async (value: boolean) => { setSaving(true); setMessage(null); setSaveError(null); try { const result = await rainApi.updateAdminSettings(value, ipLimit, usernameLimit); setAllowed(result.allow_registration); setIpLimit(result.login_ip_limit_per_minute); setUsernameLimit(result.login_username_failure_limit_per_5_minutes); setMessage('设置已保存'); } catch (e) { setSaveError(normalizeApiError(e)); await load(); } finally { setSaving(false); } };
  return <AdminGuard><section className="max-w-3xl rounded-2xl border border-slate-200 bg-white/95 p-6 shadow-xl"><h1 className="text-xl font-semibold text-slate-950">系统设置</h1><div className="mt-6 flex items-center justify-between rounded-xl border border-slate-200 p-4"><div><h2 className="font-semibold">用户注册</h2><p className="mt-1 text-sm text-slate-500">关闭后，新用户将无法注册；已有用户仍可正常登录和使用系统。</p></div><button type="button" disabled={loading || saving || !hasLoadedSettings || Boolean(loadError)} onClick={() => void save(!allowed)} className={`relative h-7 w-12 rounded-full transition ${allowed ? 'bg-cyan-600' : 'bg-slate-300'} disabled:opacity-50`}><span className={`absolute top-1 h-5 w-5 rounded-full bg-white shadow transition ${allowed ? 'left-6' : 'left-1'}`} /></button></div><div className="mt-4 grid gap-3 sm:grid-cols-2"><label className="text-sm text-slate-600">IP 每分钟阈值<input type="number" min="1" value={ipLimit} disabled={loading || saving || !hasLoadedSettings} onChange={(e) => setIpLimit(Number(e.target.value))} className="mt-1 w-full rounded-lg border border-slate-200 px-3 py-2" /></label><label className="text-sm text-slate-600">用户名失败 5 分钟阈值<input type="number" min="1" value={usernameLimit} disabled={loading || saving || !hasLoadedSettings} onChange={(e) => setUsernameLimit(Number(e.target.value))} className="mt-1 w-full rounded-lg border border-slate-200 px-3 py-2" /></label></div>{message ? <p className="mt-4 text-sm text-emerald-700">{message}</p> : null}{loadError ? <p className="mt-4 text-sm text-rose-700">{loadError}</p> : null}{saveError ? <p className="mt-4 text-sm text-rose-700">保存失败：{saveError}</p> : null}</section></AdminGuard>;
}

export function AuthRateLimitsPage() {
  const [usernameFailures, setUsernameFailures] = useState<AuthRateLimitEntry[]>([]);
  const [loginIps, setLoginIps] = useState<AuthRateLimitEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [clearingKey, setClearingKey] = useState<string | null>(null);
  const load = useCallback(async () => { setLoading(true); setError(null); try { const data = await rainApi.fetchAuthRateLimits(); setUsernameFailures(data.username_failures); setLoginIps(data.login_ips); } catch (e) { setError(normalizeApiError(e)); } finally { setLoading(false); } }, []);
  useEffect(() => { void load(); }, [load]);
  const clear = async (type: 'usernames' | 'ips', key?: string) => { if (!window.confirm(key ? '确认解除该限流？' : '确认清除全部该类限流？')) return; const operationKey = key ?? `all:${type}`; setClearingKey(operationKey); try { if (key) await rainApi.clearAuthRateLimit(type, key); else await rainApi.clearAllAuthRateLimits(type); setNotice('限流已清除'); await load(); } catch (e) { setError(normalizeApiError(e)); } finally { setClearingKey(null); } };
  const table = (title: string, type: 'usernames' | 'ips', items: AuthRateLimitEntry[], label: (item: AuthRateLimitEntry) => string) => <section className="mt-5 overflow-hidden rounded-xl border border-slate-200"><div className="flex items-center justify-between border-b border-slate-100 bg-slate-50 px-4 py-3"><h2 className="font-semibold">{title}</h2><button type="button" disabled={loading || Boolean(clearingKey) || !items.length} onClick={() => void clear(type)} className="rounded-lg border border-slate-200 bg-white px-3 py-1.5 text-xs disabled:opacity-50">清除全部</button></div>{!items.length ? <p className="p-5 text-sm text-slate-500">当前没有认证限流记录</p> : <div className="overflow-x-auto"><table className="w-full text-left text-sm"><thead className="bg-white text-xs text-slate-500"><tr><th className="px-4 py-2">标识</th><th className="px-4 py-2">次数/阈值</th><th className="px-4 py-2">最近事件</th><th className="px-4 py-2">恢复倒计时</th><th className="px-4 py-2">操作</th></tr></thead><tbody>{items.map((item) => <tr key={item.key} className="border-t border-slate-100"><td className="px-4 py-3">{label(item)}</td><td className="px-4 py-3">{item.current_count}/{item.limit} {item.limited ? <span className="text-rose-600">受限中</span> : null}</td><td className="px-4 py-3 text-slate-500">{item.last_event_at ?? '—'}</td><td className="px-4 py-3 text-slate-500">{item.limited ? `${item.retry_after_seconds}s` : '—'}</td><td className="px-4 py-3"><button type="button" disabled={loading || Boolean(clearingKey)} onClick={() => void clear(type, item.key)} className="text-rose-600 disabled:opacity-50">{clearingKey === item.key ? '清除中…' : '清除'}</button></td></tr>)}</tbody></table></div>}</section>;
  return <AdminGuard><section className="rounded-2xl border border-slate-200 bg-white/95 p-5 shadow-xl"><div className="flex items-center justify-between"><div><h1 className="text-xl font-semibold">认证限流</h1><p className="mt-2 text-sm text-slate-500">限流用于防止暴力破解，命中后返回 429；限流临时保存在当前进程内，重启后清空。</p></div><button type="button" onClick={() => void load()} disabled={loading} className="rounded-lg border border-slate-200 px-3 py-2 text-sm disabled:opacity-50">刷新</button></div>{notice ? <p className="mt-3 text-sm text-emerald-700">{notice}</p> : null}{error ? <p className="mt-3 text-sm text-rose-700">{error}</p> : null}{table('用户名失败限流', 'usernames', usernameFailures, (item) => item.username ?? item.key)}{table('登录 IP 限流', 'ips', loginIps, (item) => item.ip ?? item.key)}</section></AdminGuard>;
}

export function AdminUsersPage() {
  const auth = useAuth();
  const [users, setUsers] = useState<AdminUser[]>([]);
  const [query, setQuery] = useState('');
  const [status, setStatus] = useState<UserStatus | ''>('');
  const [history, setHistory] = useState<CursorHistory>([undefined]);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (auth.state.status !== 'AUTHENTICATED' || !isAdmin(auth.state.user)) return;
    setLoading(true);
    setError(null);
    try {
      const page = await rainApi.fetchAdminUsers({ query: query || undefined, status: status || undefined, cursor: currentCursor(history) });
      setUsers(page.items);
      setNextCursor(page.next_cursor);
    } catch (loadError) {
      setError(normalizeApiError(loadError));
    } finally {
      setLoading(false);
    }
  }, [auth.state, history, query, status]);

  useEffect(() => { void load(); }, [load]);

  return (
    <AdminGuard>
      <section className="overflow-hidden rounded-2xl border border-slate-200 bg-white/95 shadow-[0_18px_50px_rgba(15,23,42,0.08)]">
        <div className="flex flex-col gap-3 border-b border-slate-100 px-5 py-4 sm:flex-row sm:items-center sm:justify-between">
          <div>
            <div className="flex items-center gap-3">
              <span className="flex h-9 w-9 items-center justify-center rounded-xl bg-sky-50 text-lg text-sky-600">⌘</span>
              <h1 className="text-xl font-semibold tracking-tight text-slate-950">用户管理</h1>
            </div>
            <p className="mt-1 pl-12 text-sm text-slate-500">管理普通用户状态及登录会话</p>
          </div>
          <div className="flex items-center gap-2 text-xs text-slate-500">
            <span className="h-2 w-2 rounded-full bg-emerald-500" /> 当前显示 {users.length} 位用户
          </div>
        </div>
        <div className="border-b border-slate-100 bg-slate-50/70 px-5 py-4">
          <div className="grid gap-2 md:grid-cols-[minmax(0,1fr)_180px_auto]">
            <label className="relative block">
              <span className="sr-only">搜索用户名</span>
              <span className="pointer-events-none absolute inset-y-0 left-3 flex items-center text-slate-400">⌕</span>
              <input className="w-full rounded-xl border border-slate-200 bg-white py-2.5 pl-9 pr-3 text-sm outline-none transition placeholder:text-slate-400 focus:border-sky-400 focus:ring-4 focus:ring-sky-100" placeholder="搜索用户名" value={query} onChange={(event) => { setQuery(event.target.value); setHistory([undefined]); }} />
            </label>
            <select className="rounded-xl border border-slate-200 bg-white px-3 py-2.5 text-sm text-slate-700 outline-none focus:border-sky-400 focus:ring-4 focus:ring-sky-100" value={status} onChange={(event) => { setStatus(event.target.value as UserStatus | ''); setHistory([undefined]); }}><option value="">全部状态</option><option value="ACTIVE">启用</option><option value="DISABLED">停用</option></select>
            <button className="rounded-xl border border-slate-200 bg-white px-4 py-2.5 text-sm font-medium text-slate-700 transition hover:border-sky-300 hover:bg-sky-50 hover:text-sky-700" type="button" onClick={() => void load()}>刷新</button>
          </div>
        </div>
        {error ? <p className="mx-5 mt-4 rounded-xl border border-rose-200 bg-rose-50 px-4 py-3 text-sm text-rose-700">{error}</p> : null}
        {notice ? <p className="mx-5 mt-4 rounded-xl border border-emerald-200 bg-emerald-50 px-4 py-3 text-sm text-emerald-700">{notice}</p> : null}
        {loading ? <p className="py-8 text-center text-sm text-slate-500">用户加载中…</p> : users.length === 0 ? <p className="py-8 text-center text-sm text-slate-500">没有匹配的普通用户</p> : (
          <div className="mx-5 my-4 overflow-x-auto rounded-xl border border-slate-200">
            <table className="w-full min-w-[980px] text-left text-sm"><thead className="bg-slate-50 text-xs font-semibold text-slate-500"><tr className="border-b border-slate-200"><th className="px-4 py-3">用户名</th><th className="px-4 py-3">状态</th><th className="px-4 py-3">Issue 数</th><th className="px-4 py-3">占用容量</th><th className="px-4 py-3">活跃会话</th><th className="px-4 py-3">创建时间</th><th className="px-4 py-3">最近登录</th><th className="px-4 py-3">操作</th></tr></thead><tbody>
              {users.map((user) => <UserRow key={user.id} user={user} reload={load} onError={setError} onNotice={setNotice} />)}
            </tbody></table>
          </div>
        )}
        <div className="flex items-center justify-between border-t border-slate-100 px-5 py-3 text-xs text-slate-500"><span>{loading ? '正在同步…' : users.length ? `本页 ${users.length} 条` : '暂无数据'}</span><div className="flex gap-2"><button className="rounded-lg border border-slate-200 bg-white px-3 py-1.5 text-sm text-slate-600 transition hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-40" disabled={history.length <= 1} onClick={() => setHistory((value) => retreatCursor(value))}>上一页</button><button className="rounded-lg border border-slate-200 bg-white px-3 py-1.5 text-sm text-slate-600 transition hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-40" disabled={!nextCursor} onClick={() => nextCursor && setHistory((value) => advanceCursor(value, nextCursor))}>下一页</button></div></div>
      </section>
    </AdminGuard>
  );
}

function UserRow({ user, reload, onError, onNotice }: { user: AdminUser; reload: () => Promise<void>; onError: (message: string) => void; onNotice: (message: string) => void }) {
  const auth = useAuth();
  const act = async (action: () => Promise<unknown>, message: string) => {
    if (!window.confirm('确认执行此管理操作？')) return;
    try {
      await runAdminAction({ action, reload, refreshAuth: auth.refresh });
      onNotice(message);
    } catch (error) {
      onError(normalizeApiError(error));
    }
  };
  return <tr className="border-b border-slate-100 transition hover:bg-sky-50/40 last:border-0"><td className="px-4 py-3.5"><div className="font-semibold text-slate-900">{user.username}</div><div className="mt-0.5 font-mono text-[11px] text-slate-400">{user.id.slice(0, 8)}</div></td><td className="px-4 py-3.5"><span className={`inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium ${user.status === 'ACTIVE' ? 'bg-emerald-50 text-emerald-700 ring-1 ring-inset ring-emerald-200' : 'bg-slate-100 text-slate-600 ring-1 ring-inset ring-slate-200'}`}><span className={`h-1.5 w-1.5 rounded-full ${user.status === 'ACTIVE' ? 'bg-emerald-500' : 'bg-slate-400'}`} />{user.status === 'ACTIVE' ? '启用' : '停用'}</span></td><td className="px-4 py-3.5 font-medium tabular-nums text-slate-700">{user.issue_count}</td><td className="whitespace-nowrap px-4 py-3.5 font-medium tabular-nums text-slate-700">{formatBytes(user.storage_bytes)}</td><td className="px-4 py-3.5 font-medium tabular-nums text-slate-700">{user.active_session_count}</td><td className="whitespace-nowrap px-4 py-3.5 tabular-nums text-slate-600">{new Date(user.created_at).toLocaleString()}</td><td className="whitespace-nowrap px-4 py-3.5 tabular-nums text-slate-600">{user.last_login_at ? new Date(user.last_login_at).toLocaleString() : '从未登录'}</td><td className="px-4 py-3.5"><div className="flex flex-wrap gap-2"><button className="rounded-lg border border-slate-200 bg-white px-2.5 py-1.5 text-xs font-medium text-slate-700 transition hover:border-sky-300 hover:bg-sky-50 hover:text-sky-700" type="button" onClick={() => void act(() => rainApi.changeUserStatus(user.id, user.status === 'ACTIVE' ? 'DISABLED' : 'ACTIVE'), user.status === 'ACTIVE' ? '用户已停用' : '用户已启用')}>{user.status === 'ACTIVE' ? '停用' : '启用'}</button><button className="rounded-lg border border-rose-200 bg-white px-2.5 py-1.5 text-xs font-medium text-rose-600 transition hover:bg-rose-50 disabled:cursor-not-allowed disabled:opacity-40" type="button" disabled={user.active_session_count === 0} onClick={() => void act(() => rainApi.revokeUserSessions(user.id), '活跃 Session 已注销')}>注销会话</button></div></td></tr>;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KiB', 'MiB', 'GiB', 'TiB'];
  let value = bytes;
  let unit = 'B';
  for (const nextUnit of units) {
    value /= 1024;
    unit = nextUnit;
    if (value < 1024 || nextUnit === 'TiB') break;
  }
  return `${value >= 10 ? value.toFixed(0) : value.toFixed(1)} ${unit}`;
}

export function AuditLogsPage() {
  const auth = useAuth();
  const [logs, setLogs] = useState<AuditLog[]>([]);
  const [history, setHistory] = useState<CursorHistory>([undefined]);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(async () => {
    if (auth.state.status !== 'AUTHENTICATED' || !isAdmin(auth.state.user)) return;
    setLoading(true);
    try {
      const page = await rainApi.fetchAuditLogs(currentCursor(history));
      setLogs(page.items);
      setNextCursor(page.next_cursor);
    } catch (loadError) {
      setError(normalizeApiError(loadError));
    } finally {
      setLoading(false);
    }
  }, [auth.state, history]);
  useEffect(() => { void load(); }, [load]);
  const today = new Date().toDateString();
  const todayCount = logs.filter((log) => new Date(log.created_at).toDateString() === today).length;
  const userChangeCount = logs.filter((log) => log.action === 'USER_STATUS_CHANGED').length;
  const sessionActionCount = logs.filter((log) => log.action === 'USER_SESSIONS_REVOKED').length;
  return <AdminGuard><section className="overflow-hidden rounded-2xl border border-slate-200 bg-white/95 shadow-[0_18px_50px_rgba(15,23,42,0.08)]"><div className="flex flex-col gap-4 border-b border-slate-100 px-5 py-5 lg:flex-row lg:items-center lg:justify-between"><div className="flex items-center gap-3"><span className="flex h-11 w-11 items-center justify-center rounded-2xl bg-indigo-50 text-xl text-indigo-600">▣</span><div><h1 className="text-xl font-semibold tracking-tight text-slate-950">审计日志</h1><p className="mt-1 text-sm text-slate-500">查看管理员对用户和会话执行的安全操作</p></div></div><div className="grid grid-cols-2 gap-x-8 gap-y-3 text-sm sm:grid-cols-4 lg:min-w-[520px]"><AuditMetric label="本页事件" value={logs.length} tone="indigo" /><AuditMetric label="本页今日操作" value={todayCount} tone="emerald" /><AuditMetric label="本页用户变更" value={userChangeCount} tone="amber" /><AuditMetric label="本页会话操作" value={sessionActionCount} tone="violet" /></div></div><div className="border-b border-slate-100 bg-slate-50/70 px-5 py-4"><div className="flex flex-wrap items-center justify-between gap-3"><div className="text-sm font-medium text-slate-700">操作记录</div><button className="rounded-xl border border-slate-200 bg-white px-4 py-2.5 text-sm font-medium text-slate-700 transition hover:border-sky-300 hover:bg-sky-50 hover:text-sky-700" type="button" onClick={() => void load()}>刷新</button></div></div>{error ? <p className="mx-5 mt-4 rounded-xl border border-rose-200 bg-rose-50 px-4 py-3 text-sm text-rose-700">{error}</p> : null}{loading ? <p className="py-10 text-center text-sm text-slate-500">审计日志加载中…</p> : logs.length === 0 ? <p className="py-10 text-center text-sm text-slate-500">暂无审计记录</p> : <div className="mx-5 my-4 overflow-x-auto rounded-xl border border-slate-200"><table className="w-full min-w-[760px] text-left text-sm"><thead className="bg-slate-50 text-xs font-semibold text-slate-500"><tr className="border-b border-slate-200"><th className="px-4 py-3">时间</th><th className="px-4 py-3">操作类型</th><th className="px-4 py-3">目标用户</th><th className="px-4 py-3">变更摘要</th><th className="px-4 py-3">客户端 IP</th></tr></thead><tbody>{logs.map((log) => <tr className="border-b border-slate-100 transition hover:bg-sky-50/40 last:border-0" key={log.id}><td className="whitespace-nowrap px-4 py-3.5 tabular-nums text-slate-600">{new Date(log.created_at).toLocaleString()}</td><td className="px-4 py-3.5"><span className="inline-flex rounded-full bg-indigo-50 px-2.5 py-1 text-xs font-medium text-indigo-700 ring-1 ring-inset ring-indigo-200">{log.action === 'ADMIN_BOOTSTRAPPED' ? '初始化管理员' : log.action === 'USER_STATUS_CHANGED' ? '变更用户状态' : log.action === 'USER_SESSIONS_REVOKED' ? '注销用户 Session' : log.action}</span></td><td className="px-4 py-3.5"><div className="font-semibold text-slate-900">{log.target_username || '用户已删除'}</div>{log.target_user_id ? <div className="mt-0.5 font-mono text-[11px] text-slate-400">{log.target_user_id.slice(0, 8)}</div> : <div className="mt-0.5 text-xs text-slate-400">系统操作</div>}</td><td className="px-4 py-3.5 text-slate-600">{log.old_value || log.new_value ? `${log.old_value ?? '—'} → ${log.new_value ?? '—'}` : '—'}</td><td className="px-4 py-3.5 font-mono text-xs text-slate-600">{log.client_ip || '—'}</td></tr>)}</tbody></table></div>}<div className="flex items-center justify-between border-t border-slate-100 px-5 py-3 text-xs text-slate-500"><span>{loading ? '正在同步…' : logs.length ? `本页 ${logs.length} 条` : '暂无数据'}</span><div className="flex gap-2"><button className="rounded-lg border border-slate-200 bg-white px-3 py-1.5 text-sm text-slate-600 transition hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-40" disabled={history.length <= 1} onClick={() => setHistory((value) => retreatCursor(value))}>上一页</button><button className="rounded-lg border border-slate-200 bg-white px-3 py-1.5 text-sm text-slate-600 transition hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-40" disabled={!nextCursor} onClick={() => nextCursor && setHistory((value) => advanceCursor(value, nextCursor))}>下一页</button></div></div></section></AdminGuard>;
}

function AuditMetric({ label, value, tone }: { label: string; value: number; tone: 'indigo' | 'emerald' | 'amber' | 'violet' }) {
  const toneClass = { indigo: 'bg-indigo-50 text-indigo-600', emerald: 'bg-emerald-50 text-emerald-600', amber: 'bg-amber-50 text-amber-600', violet: 'bg-violet-50 text-violet-600' }[tone];
  return <div className="border-l border-slate-200 pl-4 first:border-l-0 first:pl-0"><div className="text-xs text-slate-500">{label}</div><div className={`mt-1 inline-flex rounded-lg px-2 py-0.5 text-lg font-semibold ${toneClass}`}>{value}</div></div>;
}
