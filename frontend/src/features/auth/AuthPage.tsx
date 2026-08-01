import { useState, type FormEvent } from 'react';
import { Link, Navigate, useLocation, useNavigate } from 'react-router-dom';
import { normalizeApiError } from '../../api/client';
import { useAuth } from '../../auth/AuthContext';
import { postLoginPath, safeReturnPath } from '../../auth/authState';

interface AuthPageProps {
  mode: 'login' | 'register';
}

interface AuthLocationState {
  from?: string;
  registered?: boolean;
}

export function AuthPage({ mode }: AuthPageProps) {
  const auth = useAuth();
  const location = useLocation();
  const navigate = useNavigate();
  const state = (location.state || {}) as AuthLocationState;
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const isLogin = mode === 'login';

  if (auth.state.status === 'AUTHENTICATED') {
    return <Navigate to={postLoginPath(auth.state.user, state.from)} replace />;
  }

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setError('');
    setSubmitting(true);
    try {
      if (isLogin) {
        const user = await auth.login({ username, password });
        navigate(postLoginPath(user, state.from), { replace: true });
      } else {
        await auth.register({ username, password });
        navigate('/login', {
          replace: true,
          state: { from: safeReturnPath(state.from), registered: true }
        });
      }
    } catch (submissionError) {
      setError(normalizeApiError(submissionError));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <section className="mx-auto mt-10 max-w-md rounded-3xl border border-slate-200 bg-white p-8 shadow-xl shadow-slate-200/60">
      <div className="mb-7">
        <p className="text-sm font-semibold uppercase tracking-[0.2em] text-cyan-700">
          Rain Account
        </p>
        <h2 className="mt-2 text-3xl font-semibold text-slate-950">
          {isLogin ? '登录 Rain' : '创建账户'}
        </h2>
        <p className="mt-2 text-sm leading-6 text-slate-500">
          {isLogin
            ? '登录后即可使用后续开放的写入与个人化功能。'
            : '用户名不区分大小写，密码长度为 8 到 128 个字符。'}
        </p>
      </div>

      {isLogin && state.registered && (
        <div className="mb-5 rounded-xl border border-emerald-200 bg-emerald-50 px-4 py-3 text-sm text-emerald-800">
          注册成功，请使用新账户登录。
        </div>
      )}

      <form className="space-y-5" onSubmit={submit}>
        <label className="block text-sm font-medium text-slate-700">
          用户名
          <input
            autoComplete="username"
            className="mt-2 w-full rounded-xl border border-slate-300 px-4 py-3 outline-none transition focus:border-cyan-500 focus:ring-4 focus:ring-cyan-100"
            maxLength={32}
            minLength={3}
            pattern="[A-Za-z0-9._-]{3,32}"
            required
            value={username}
            onChange={(event) => setUsername(event.target.value)}
          />
        </label>
        <label className="block text-sm font-medium text-slate-700">
          密码
          <input
            autoComplete={isLogin ? 'current-password' : 'new-password'}
            className="mt-2 w-full rounded-xl border border-slate-300 px-4 py-3 outline-none transition focus:border-cyan-500 focus:ring-4 focus:ring-cyan-100"
            maxLength={128}
            minLength={isLogin ? undefined : 8}
            required
            type="password"
            value={password}
            onChange={(event) => setPassword(event.target.value)}
          />
        </label>

        {error && (
          <div className="rounded-xl border border-rose-200 bg-rose-50 px-4 py-3 text-sm text-rose-700">
            {error}
          </div>
        )}

        <button
          className="w-full rounded-xl bg-slate-950 px-4 py-3 font-semibold text-white transition hover:bg-cyan-700 disabled:cursor-not-allowed disabled:opacity-60"
          disabled={submitting}
          type="submit"
        >
          {submitting ? '请稍候…' : isLogin ? '登录' : '注册'}
        </button>
      </form>

      <p className="mt-6 text-center text-sm text-slate-500">
        {isLogin ? '还没有账户？' : '已经有账户？'}{' '}
        <Link
          className="font-semibold text-cyan-700 hover:text-cyan-900"
          state={{ from: safeReturnPath(state.from) }}
          to={isLogin ? '/register' : '/login'}
        >
          {isLogin ? '注册' : '登录'}
        </Link>
      </p>
    </section>
  );
}
