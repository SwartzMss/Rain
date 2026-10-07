import { useEffect, useState, type FormEvent } from 'react';
import { Link, Navigate, useLocation, useNavigate } from 'react-router-dom';
import { ApiError, normalizeApiError, rainApi } from '../../api/client';
import { useAuth } from '../../auth/AuthContext';
import { postLoginPath, safeReturnPath } from '../../auth/authState';

interface AuthPageProps {
  mode: 'login' | 'register';
}

interface AuthLocationState {
  from?: string;
  registered?: boolean;
}

function registrationStateFromStatus(status: Awaited<ReturnType<typeof rainApi.fetchRegistrationStatus>>) {
  return status.registration_mode ?? (!status.allow_registration ? 'CLOSED' : status.requires_invite_code ? 'INVITE_ONLY' : 'OPEN');
}

export function AuthPage({ mode }: AuthPageProps) {
  const auth = useAuth();
  const location = useLocation();
  const navigate = useNavigate();
  const state = (location.state || {}) as AuthLocationState;
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [inviteCode, setInviteCode] = useState('');
  const [error, setError] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const isLogin = mode === 'login';
  const [registrationState, setRegistrationState] = useState<'LOADING' | 'CLOSED' | 'INVITE_ONLY' | 'OPEN' | 'ERROR'>('LOADING');
  const [registrationStatusAttempt, setRegistrationStatusAttempt] = useState(0);

  useEffect(() => {
    let active = true;
    setRegistrationState('LOADING');
    void rainApi.fetchRegistrationStatus().then((status) => {
      if (active) {
        setRegistrationState(registrationStateFromStatus(status));
      }
    }).catch(() => {
      if (active) setRegistrationState('ERROR');
    });
    return () => { active = false; };
  }, [isLogin, registrationStatusAttempt]);

  if (auth.state.status === 'AUTHENTICATED') {
    return <Navigate to={postLoginPath(auth.state.user, state.from)} replace />;
  }

  if (!isLogin && ['LOADING', 'CLOSED', 'ERROR'].includes(registrationState)) {
    const message = registrationState === 'LOADING' ? '正在确认注册状态…' : registrationState === 'CLOSED' ? '当前系统未开放用户注册，请联系管理员。' : '暂时无法确认注册状态，请稍后重试。';
    return <section className="mx-auto mt-10 max-w-md rounded-3xl border border-slate-200 bg-white p-8 shadow-xl"><h2 className="text-2xl font-semibold text-slate-950">{registrationState === 'LOADING' ? '正在确认注册状态' : registrationState === 'CLOSED' ? '注册已关闭' : '注册状态不可用'}</h2><p className="mt-3 text-sm text-slate-500">{message}</p>{registrationState === 'ERROR' ? <button className="mt-5 rounded-lg bg-slate-950 px-4 py-2 font-semibold text-white" onClick={() => setRegistrationStatusAttempt((attempt) => attempt + 1)} type="button">重新检查</button> : null}{registrationState !== 'LOADING' ? <Link className="mt-6 ml-4 inline-block font-semibold text-cyan-700" to="/login">返回登录</Link> : null}</section>;
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
        await auth.register({ username, password, ...(registrationState === 'INVITE_ONLY' ? { invite_code: inviteCode } : {}) });
        navigate('/login', {
          replace: true,
          state: { from: safeReturnPath(state.from), registered: true }
        });
      }
    } catch (submissionError) {
      const message = normalizeApiError(submissionError);
      setError(!isLogin && !(submissionError instanceof ApiError)
        ? `${message} 若刚才提交结果不确定，请先尝试使用该账号登录，再决定是否重试。`
        : message);
      if (!isLogin) {
        try {
          setRegistrationState(registrationStateFromStatus(await rainApi.fetchRegistrationStatus()));
        } catch { /* Keep the last confirmed mode; the server still enforces policy. */ }
      }
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

        {!isLogin && registrationState === 'INVITE_ONLY' ? (
          <label className="block text-sm font-medium text-slate-700">
            邀请码
            <input
              autoComplete="off"
              className="mt-2 w-full rounded-xl border border-slate-300 px-4 py-3 font-mono uppercase tracking-wide outline-none transition focus:border-cyan-500 focus:ring-4 focus:ring-cyan-100"
              maxLength={64}
              required
              value={inviteCode}
              onChange={(event) => setInviteCode(event.target.value.toUpperCase())}
            />
          </label>
        ) : null}
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
          <div role="alert" className="rounded-xl border border-rose-200 bg-rose-50 px-4 py-3 text-sm text-rose-700">
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
        {isLogin ? (registrationState === 'OPEN' || registrationState === 'INVITE_ONLY' ? <><span>还没有账户？ </span><Link className="font-semibold text-cyan-700 hover:text-cyan-900" state={{ from: safeReturnPath(state.from) }} to="/register">注册</Link></> : null) : <><span>已经有账户？ </span><Link className="font-semibold text-cyan-700 hover:text-cyan-900" state={{ from: safeReturnPath(state.from) }} to="/login">登录</Link></>}
      </p>
    </section>
  );
}
