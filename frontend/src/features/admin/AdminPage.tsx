import { useCallback, useEffect, useState } from 'react';
import { Navigate } from 'react-router-dom';
import { normalizeApiError, rainApi } from '../../api/client';
import type { AdminUser, AuditLog, UserRole, UserStatus } from '../../api/types';
import { useAuth } from '../../auth/AuthContext';
import { isAdmin } from '../../auth/permissions';
import {
  advanceCursor,
  currentCursor,
  retreatCursor,
  runAdminAction,
  type CursorHistory
} from './adminFlow';

export function AdminPage() {
  const auth = useAuth();
  const [users, setUsers] = useState<AdminUser[]>([]);
  const [logs, setLogs] = useState<AuditLog[]>([]);
  const [query, setQuery] = useState('');
  const [role, setRole] = useState<UserRole | ''>('');
  const [status, setStatus] = useState<UserStatus | ''>('');
  const [userHistory, setUserHistory] = useState<CursorHistory>([undefined]);
  const [nextUserCursor, setNextUserCursor] = useState<string | null>(null);
  const [auditHistory, setAuditHistory] = useState<CursorHistory>([undefined]);
  const [nextAuditCursor, setNextAuditCursor] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (auth.state.status !== 'AUTHENTICATED' || !isAdmin(auth.state.user)) return;
    setError(null);
    try {
      const [userPage, auditPage] = await Promise.all([
        rainApi.fetchAdminUsers({
          query: query || undefined,
          role: role || undefined,
          status: status || undefined,
          cursor: currentCursor(userHistory)
        }),
        rainApi.fetchAuditLogs(currentCursor(auditHistory))
      ]);
      setUsers(userPage.items);
      setNextUserCursor(userPage.next_cursor);
      setLogs(auditPage.items);
      setNextAuditCursor(auditPage.next_cursor);
    } catch (loadError) {
      setError(normalizeApiError(loadError));
    }
  }, [auth.state, query, role, status, userHistory, auditHistory]);

  useEffect(() => {
    void load();
  }, [load]);

  if (auth.state.status === 'LOADING') return <p>正在确认身份…</p>;
  if (auth.state.status !== 'AUTHENTICATED') {
    return <Navigate to="/login" replace state={{ from: '/admin' }} />;
  }
  if (!isAdmin(auth.state.user)) {
    return <section className="panel"><h2>403</h2><p>此页面需要管理员权限。</p></section>;
  }
  const currentUser = auth.state.user;

  const act = async (
    action: () => Promise<unknown>,
    message: string,
    selfRevocation = false
  ) => {
    if (!window.confirm('确认执行此管理操作？')) return;
    setError(null);
    try {
      await runAdminAction({ action, reload: load, refreshAuth: auth.refresh, selfRevocation });
      setNotice(message);
    } catch (actionError) {
      setError(normalizeApiError(actionError));
    }
  };

  return (
    <div className="space-y-5">
      <section className="panel space-y-3">
        <h2 className="text-xl font-semibold">用户管理</h2>
        {error ? <p className="text-rose-600">{error}</p> : null}
        {notice ? <p className="text-emerald-700">{notice}</p> : null}
        <div className="flex gap-2">
          <input className="rounded border px-3 py-2" placeholder="搜索用户名" value={query} onChange={(event) => { setQuery(event.target.value); setUserHistory([undefined]); }} />
          <select value={role} onChange={(event) => { setRole(event.target.value as UserRole | ''); setUserHistory([undefined]); }}><option value="">全部角色</option><option>USER</option><option>ADMIN</option></select>
          <select value={status} onChange={(event) => { setStatus(event.target.value as UserStatus | ''); setUserHistory([undefined]); }}><option value="">全部状态</option><option>ACTIVE</option><option>DISABLED</option></select>
        </div>
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead><tr><th>用户</th><th>角色</th><th>状态</th><th>Session</th><th>操作</th></tr></thead>
            <tbody>{users.map((user) => <tr key={user.id}><td>{user.username}</td><td>{user.role}</td><td>{user.status}</td><td>{user.active_session_count}</td><td className="space-x-2"><button onClick={() => void act(() => rainApi.changeUserRole(user.id, user.role === 'ADMIN' ? 'USER' : 'ADMIN'), '角色已更新')}>{user.role === 'ADMIN' ? '降级' : '提升'}</button><button onClick={() => void act(() => rainApi.changeUserStatus(user.id, user.status === 'ACTIVE' ? 'DISABLED' : 'ACTIVE'), '状态已更新')}>{user.status === 'ACTIVE' ? '停用' : '启用'}</button><button onClick={() => void act(() => rainApi.revokeUserSessions(user.id), 'Session 已撤销', user.id === currentUser.id)}>强制注销</button></td></tr>)}</tbody>
          </table>
        </div>
        <div className="flex justify-end gap-2"><button disabled={userHistory.length <= 1} onClick={() => setUserHistory((history) => retreatCursor(history))}>上一页</button><button disabled={!nextUserCursor} onClick={() => nextUserCursor && setUserHistory((history) => advanceCursor(history, nextUserCursor))}>下一页</button></div>
      </section>
      <section className="panel">
        <h2 className="text-xl font-semibold">管理员审计日志</h2>
        <table className="mt-3 w-full text-sm"><thead><tr><th>时间</th><th>动作</th><th>目标</th><th>变化</th></tr></thead><tbody>{logs.map((log) => <tr key={log.id}><td>{log.created_at}</td><td>{log.action}</td><td>{log.target_user_id || '-'}</td><td>{log.old_value || '-'} → {log.new_value || '-'}</td></tr>)}</tbody></table>
        <div className="flex justify-end gap-2"><button disabled={auditHistory.length <= 1} onClick={() => setAuditHistory((history) => retreatCursor(history))}>上一页</button><button disabled={!nextAuditCursor} onClick={() => nextAuditCursor && setAuditHistory((history) => advanceCursor(history, nextAuditCursor))}>下一页</button></div>
      </section>
    </div>
  );
}
