import { useState, type FormEvent } from 'react';
import { Navigate } from 'react-router-dom';
import { normalizeApiError } from '../../api/client';
import { useAuth } from '../../auth/AuthContext';
import { isAdmin } from '../../auth/permissions';
import { SkillsPage } from '../skills/SkillsPage';

export function AccountPage() {
  const auth = useAuth();
  const [currentPassword, setCurrentPassword] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [section, setSection] = useState<'security' | 'skills'>('security');

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
    <section className={`mx-auto mt-6 space-y-5 rounded-2xl border border-slate-200 bg-white p-6 shadow-lg shadow-slate-200/50 ${section === 'skills' ? 'max-w-5xl' : 'max-w-md'}`}>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-sm text-slate-500">当前用户：{auth.state.user.username}</p>
        <div className="flex rounded-lg bg-slate-100 p-1" role="tablist" aria-label="账户设置">
          <button className={`rounded-md px-3 py-1.5 text-sm ${section === 'security' ? 'bg-white font-semibold shadow-sm' : 'text-slate-600'}`} role="tab" aria-selected={section === 'security'} onClick={() => setSection('security')}>账户安全</button>
          <button className={`rounded-md px-3 py-1.5 text-sm ${section === 'skills' ? 'bg-white font-semibold shadow-sm' : 'text-slate-600'}`} role="tab" aria-selected={section === 'skills'} onClick={() => setSection('skills')}>我的 Skills</button>
        </div>
      </div>
      {section === 'skills' ? <SkillsPage /> : <div className="space-y-5"><h2 className="text-xl font-semibold">账户安全</h2><form className="space-y-3" onSubmit={submit}>
        <label className="block text-sm font-medium">当前密码
          <input className="mt-1.5 w-full rounded-lg border border-slate-300 px-3 py-2" type="password" minLength={8} maxLength={128} required value={currentPassword} onChange={(event) => setCurrentPassword(event.target.value)} />
        </label>
        <label className="block text-sm font-medium">新密码
          <input className="mt-1.5 w-full rounded-lg border border-slate-300 px-3 py-2" type="password" minLength={8} maxLength={128} required value={newPassword} onChange={(event) => setNewPassword(event.target.value)} />
        </label>
        {message ? <p className="rounded-lg bg-emerald-50 px-3 py-2 text-sm text-emerald-700">{message}</p> : null}
        {error ? <p className="rounded-lg bg-rose-50 px-3 py-2 text-sm text-rose-700">{error}</p> : null}
        <button className="rounded-lg bg-slate-950 px-4 py-2 text-sm font-semibold text-white disabled:opacity-60" disabled={submitting} type="submit">修改密码</button>
      </form></div>}
    </section>
  );
}
