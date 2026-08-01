import { useState, type FormEvent } from 'react';
import { Navigate } from 'react-router-dom';
import { normalizeApiError } from '../../api/client';
import { useAuth } from '../../auth/AuthContext';
import { isAdmin } from '../../auth/permissions';

export function AccountPage() {
  const auth = useAuth();
  const [currentPassword, setCurrentPassword] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const [submitting, setSubmitting] = useState(false);

  if (auth.state.status === 'GUEST') return <Navigate to="/login" replace />;
  if (auth.state.status === 'LOADING') return <p>正在确认身份…</p>;
  if (isAdmin(auth.state.user)) return <Navigate to="/admin/users" replace />;

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setSubmitting(true);
    setError('');
    setMessage('');
    try {
      await auth.changePassword({
        current_password: currentPassword,
        new_password: newPassword
      });
      setCurrentPassword('');
      setNewPassword('');
      setMessage('密码已修改，其他设备上的会话已退出。');
    } catch (reason) {
      setError(normalizeApiError(reason));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <section className="mx-auto mt-6 max-w-md space-y-5 rounded-2xl border border-slate-200 bg-white p-6 shadow-lg shadow-slate-200/50">
      <div>
        <h2 className="text-xl font-semibold">账户安全</h2>
        <p className="mt-1 text-sm text-slate-500">当前用户：{auth.state.user.username}</p>
      </div>
      <form className="space-y-3" onSubmit={submit}>
        <label className="block text-sm font-medium">当前密码
          <input className="mt-1.5 w-full rounded-lg border border-slate-300 px-3 py-2" type="password" minLength={8} maxLength={128} required value={currentPassword} onChange={(event) => setCurrentPassword(event.target.value)} />
        </label>
        <label className="block text-sm font-medium">新密码
          <input className="mt-1.5 w-full rounded-lg border border-slate-300 px-3 py-2" type="password" minLength={8} maxLength={128} required value={newPassword} onChange={(event) => setNewPassword(event.target.value)} />
        </label>
        {message ? <p className="rounded-lg bg-emerald-50 px-3 py-2 text-sm text-emerald-700">{message}</p> : null}
        {error ? <p className="rounded-lg bg-rose-50 px-3 py-2 text-sm text-rose-700">{error}</p> : null}
        <button className="rounded-lg bg-slate-950 px-4 py-2 text-sm font-semibold text-white disabled:opacity-60" disabled={submitting} type="submit">修改密码</button>
      </form>
    </section>
  );
}
