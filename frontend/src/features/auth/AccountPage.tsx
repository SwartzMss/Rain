import { useState, type FormEvent } from 'react';
import { Navigate } from 'react-router-dom';
import { normalizeApiError } from '../../api/client';
import { useAuth } from '../../auth/AuthContext';

export function AccountPage() {
  const auth = useAuth();
  const [currentPassword, setCurrentPassword] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const [submitting, setSubmitting] = useState(false);

  if (auth.state.status === 'GUEST') return <Navigate to="/login" replace />;
  if (auth.state.status === 'LOADING') return <p>正在确认身份…</p>;

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
    <section className="mx-auto mt-8 max-w-xl space-y-6 rounded-3xl border border-slate-200 bg-white p-8 shadow-xl shadow-slate-200/60">
      <div>
        <p className="text-sm font-semibold uppercase tracking-[0.2em] text-cyan-700">Rain Account</p>
        <h2 className="mt-2 text-3xl font-semibold">账户安全</h2>
        <p className="mt-2 text-sm text-slate-500">当前用户：{auth.state.user.username}</p>
      </div>
      <form className="space-y-4" onSubmit={submit}>
        <label className="block text-sm font-medium">当前密码
          <input className="mt-2 w-full rounded-xl border border-slate-300 px-4 py-3" type="password" minLength={8} maxLength={128} required value={currentPassword} onChange={(event) => setCurrentPassword(event.target.value)} />
        </label>
        <label className="block text-sm font-medium">新密码
          <input className="mt-2 w-full rounded-xl border border-slate-300 px-4 py-3" type="password" minLength={8} maxLength={128} required value={newPassword} onChange={(event) => setNewPassword(event.target.value)} />
        </label>
        {message ? <p className="rounded-xl bg-emerald-50 px-4 py-3 text-sm text-emerald-700">{message}</p> : null}
        {error ? <p className="rounded-xl bg-rose-50 px-4 py-3 text-sm text-rose-700">{error}</p> : null}
        <button className="rounded-xl bg-slate-950 px-5 py-3 font-semibold text-white disabled:opacity-60" disabled={submitting} type="submit">修改密码</button>
      </form>
      <div className="border-t border-slate-200 pt-5">
        <button className="rounded-xl border border-rose-300 px-5 py-3 font-semibold text-rose-700" type="button" onClick={() => void auth.logoutAll().catch((reason) => setError(normalizeApiError(reason)))}>
          退出所有设备
        </button>
      </div>
    </section>
  );
}
