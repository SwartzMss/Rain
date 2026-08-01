import { useCallback, useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import { Navigate, NavLink } from 'react-router-dom';
import { normalizeApiError, rainApi } from '../../api/client';
import type { AdminUser, AuditLog, UserStatus } from '../../api/types';
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
    <div className="mx-auto max-w-6xl space-y-5">
      <nav className="flex flex-wrap items-center gap-2 rounded-xl border border-slate-200 bg-white p-2 text-sm">
        <NavLink className={({ isActive }) => `rounded-lg px-3 py-2 ${isActive ? 'bg-slate-900 text-white' : 'text-slate-600 hover:bg-slate-100'}`} to="/admin/users">用户管理</NavLink>
        <NavLink className={({ isActive }) => `rounded-lg px-3 py-2 ${isActive ? 'bg-slate-900 text-white' : 'text-slate-600 hover:bg-slate-100'}`} to="/admin/audit-logs">审计日志</NavLink>
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
      <section className="panel space-y-5">
        <div>
          <h1 className="text-2xl font-semibold">用户管理</h1>
          <p className="mt-1 text-sm text-slate-500">管理普通用户状态及登录会话</p>
        </div>
        <div className="grid gap-2 md:grid-cols-[minmax(0,1fr)_180px_auto]">
          <input className="rounded-lg border px-3 py-2" placeholder="搜索用户名" value={query} onChange={(event) => { setQuery(event.target.value); setHistory([undefined]); }} />
          <select className="rounded-lg border px-3 py-2" value={status} onChange={(event) => { setStatus(event.target.value as UserStatus | ''); setHistory([undefined]); }}><option value="">全部状态</option><option value="ACTIVE">启用</option><option value="DISABLED">停用</option></select>
          <button className="rounded-lg border px-3 py-2" type="button" onClick={() => void load()}>刷新</button>
        </div>
        {error ? <p className="rounded-lg bg-rose-50 p-3 text-sm text-rose-700">{error}</p> : null}
        {notice ? <p className="rounded-lg bg-emerald-50 p-3 text-sm text-emerald-700">{notice}</p> : null}
        {loading ? <p className="py-8 text-center text-sm text-slate-500">用户加载中…</p> : users.length === 0 ? <p className="py-8 text-center text-sm text-slate-500">没有匹配的普通用户</p> : (
          <div className="overflow-x-auto">
            <table className="w-full text-left text-sm"><thead><tr className="border-b"><th className="p-3">用户名</th><th className="p-3">状态</th><th className="p-3">活跃会话</th><th className="p-3">创建时间</th><th className="p-3">最近登录</th><th className="p-3">操作</th></tr></thead><tbody>
              {users.map((user) => <UserRow key={user.id} user={user} reload={load} onError={setError} onNotice={setNotice} />)}
            </tbody></table>
          </div>
        )}
        <div className="flex items-center justify-end gap-2 text-sm"><button className="rounded-lg border px-3 py-2 disabled:opacity-40" disabled={history.length <= 1} onClick={() => setHistory((value) => retreatCursor(value))}>上一页</button><button className="rounded-lg border px-3 py-2 disabled:opacity-40" disabled={!nextCursor} onClick={() => nextCursor && setHistory((value) => advanceCursor(value, nextCursor))}>下一页</button></div>
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
  return <tr className="border-b last:border-0"><td className="p-3 font-medium">{user.username}<span className="ml-2 text-xs text-slate-400">{user.id.slice(0, 8)}</span></td><td className="p-3"><span className={`rounded-full px-2 py-1 text-xs ${user.status === 'ACTIVE' ? 'bg-emerald-100 text-emerald-700' : 'bg-slate-200 text-slate-600'}`}>{user.status === 'ACTIVE' ? '启用' : '停用'}</span></td><td className="p-3">{user.active_session_count}</td><td className="p-3">{new Date(user.created_at).toLocaleString()}</td><td className="p-3">{user.last_login_at ? new Date(user.last_login_at).toLocaleString() : '从未登录'}</td><td className="p-3"><div className="flex flex-wrap gap-2"><button className="rounded-lg border px-2 py-1" type="button" onClick={() => void act(() => rainApi.changeUserStatus(user.id, user.status === 'ACTIVE' ? 'DISABLED' : 'ACTIVE'), user.status === 'ACTIVE' ? '用户已停用' : '用户已启用')}>{user.status === 'ACTIVE' ? '停用' : '启用'}</button><button className="rounded-lg border px-2 py-1 disabled:opacity-40" type="button" disabled={user.active_session_count === 0} onClick={() => void act(() => rainApi.revokeUserSessions(user.id), '活跃 Session 已注销')}>强制注销</button></div></td></tr>;
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
  return <AdminGuard><section className="panel space-y-5"><div><h1 className="text-2xl font-semibold">审计日志</h1><p className="mt-1 text-sm text-slate-500">查看管理员对用户和会话执行的安全操作</p></div>{error ? <p className="rounded-lg bg-rose-50 p-3 text-sm text-rose-700">{error}</p> : null}{loading ? <p className="py-8 text-center text-sm text-slate-500">审计日志加载中…</p> : logs.length === 0 ? <p className="py-8 text-center text-sm text-slate-500">暂无审计记录</p> : <div className="overflow-x-auto"><table className="w-full text-left text-sm"><thead><tr className="border-b"><th className="p-3">时间</th><th className="p-3">操作类型</th><th className="p-3">目标用户</th><th className="p-3">变更摘要</th><th className="p-3">客户端 IP</th></tr></thead><tbody>{logs.map((log) => <tr className="border-b last:border-0" key={log.id}><td className="p-3">{new Date(log.created_at).toLocaleString()}</td><td className="p-3">{log.action === 'ADMIN_BOOTSTRAPPED' ? '初始化管理员' : log.action === 'USER_STATUS_CHANGED' ? '变更用户状态' : log.action === 'USER_SESSIONS_REVOKED' ? '注销用户 Session' : log.action}</td><td className="p-3">{log.target_user_id ? log.target_user_id.slice(0, 8) : '—'}</td><td className="p-3">{log.old_value || log.new_value ? `${log.old_value ?? '—'} → ${log.new_value ?? '—'}` : '—'}</td><td className="p-3">{log.client_ip || '—'}</td></tr>)}</tbody></table></div>}<div className="flex justify-end gap-2"><button className="rounded-lg border px-3 py-2 disabled:opacity-40" disabled={history.length <= 1} onClick={() => setHistory((value) => retreatCursor(value))}>上一页</button><button className="rounded-lg border px-3 py-2 disabled:opacity-40" disabled={!nextCursor} onClick={() => nextCursor && setHistory((value) => advanceCursor(value, nextCursor))}>下一页</button></div></section></AdminGuard>;
}
