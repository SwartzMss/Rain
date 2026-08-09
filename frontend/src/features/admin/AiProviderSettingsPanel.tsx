import { useEffect, useState, type FormEvent } from 'react';
import { normalizeApiError, rainApi } from '../../api/client';

interface SavedProviderForm {
  baseUrl: string;
  model: string;
  timeout: number;
}

export function AiProviderSettingsPanel() {
  const [baseUrl, setBaseUrl] = useState('');
  const [apiKey, setApiKey] = useState('');
  const [model, setModel] = useState('');
  const [timeout, setTimeoutValue] = useState(30);
  const [source, setSource] = useState<string | null>(null);
  const [mask, setMask] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const [saved, setSaved] = useState<SavedProviderForm | null>(null);

  const load = async () => {
    try {
      const value = await rainApi.fetchAiProvider();
      setBaseUrl(value.base_url ?? '');
      setModel(value.model ?? '');
      setTimeoutValue(value.request_timeout_seconds);
      setSource(value.source ?? null);
      setMask(value.api_key_mask ?? null);
      setSaved(value.configured && value.base_url && value.model ? {
        baseUrl: value.base_url,
        model: value.model,
        timeout: value.request_timeout_seconds
      } : null);
    } catch (reason) {
      setError(normalizeApiError(reason));
    }
  };
  useEffect(() => { void load(); }, []);

  const save = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true); setError(''); setMessage('');
    try {
      const payload: { base_url: string; model: string; request_timeout_seconds: number; api_key?: string } = { base_url: baseUrl, model, request_timeout_seconds: timeout };
      if (apiKey) payload.api_key = apiKey;
      const value = await rainApi.updateAiProvider(payload);
      setApiKey(''); setSource(value.source ?? null); setMask(value.api_key_mask ?? null);
      setBaseUrl(value.base_url ?? baseUrl); setModel(value.model ?? model); setTimeoutValue(value.request_timeout_seconds);
      setSaved({ baseUrl: value.base_url ?? baseUrl, model: value.model ?? model, timeout: value.request_timeout_seconds });
      setMessage('AI 配置已保存');
    } catch (reason) { setError(normalizeApiError(reason)); } finally { setBusy(false); }
  };
  const test = async () => {
    setError(''); setMessage('');
    const normalizedBaseUrl = baseUrl.trim().replace(/\/+$/, '');
    const changed = !saved
      || normalizedBaseUrl !== saved.baseUrl.trim().replace(/\/+$/, '')
      || model.trim() !== saved.model.trim()
      || timeout !== saved.timeout;
    if (!apiKey.trim() && changed) {
      setError('测试修改后的配置需要重新输入 API Key');
      return;
    }
    setBusy(true);
    try {
      const value = apiKey.trim()
        ? await rainApi.testAiProvider({ base_url: normalizedBaseUrl, api_key: apiKey.trim(), model: model.trim(), request_timeout_seconds: timeout })
        : await rainApi.testAiProvider();
      setMessage(`连接成功，模型：${value.model}`);
    }
    catch (reason) { setError(normalizeApiError(reason)); } finally { setBusy(false); }
  };

  return (
    <section className="rounded-2xl border border-slate-200/90 bg-white/95 p-5 shadow-[0_12px_32px_rgba(15,23,42,0.06)] sm:p-6">
      <h2 className="text-lg font-semibold text-slate-950">AI Provider</h2>
      <p className="mt-1 text-sm text-slate-500">配置 OpenAI-compatible Chat Completions 服务。数据库配置优先于环境变量。</p>
      <form className="mt-5 grid gap-4 md:grid-cols-2" onSubmit={save}>
        <label className="text-sm font-medium text-slate-700">Base URL<input className="mt-2 w-full rounded-lg border border-slate-200 px-3 py-2.5" type="url" required value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} /></label>
        <label className="text-sm font-medium text-slate-700">模型<input className="mt-2 w-full rounded-lg border border-slate-200 px-3 py-2.5" required value={model} onChange={(event) => setModel(event.target.value)} /></label>
        <label className="text-sm font-medium text-slate-700">API Key<input className="mt-2 w-full rounded-lg border border-slate-200 px-3 py-2.5" type="password" placeholder={mask ? `保留现有密钥（${mask}）` : '输入 API Key'} value={apiKey} onChange={(event) => setApiKey(event.target.value)} /></label>
        <label className="text-sm font-medium text-slate-700">请求超时（秒）<input className="mt-2 w-full rounded-lg border border-slate-200 px-3 py-2.5" type="number" min="1" max="300" required value={timeout} onChange={(event) => setTimeoutValue(Number(event.target.value))} /></label>
        <div className="md:col-span-2 flex flex-wrap items-center justify-between gap-3 border-t border-slate-100 pt-4">
          <span className="text-xs text-slate-500">当前来源：{source === 'DATABASE' ? '数据库' : source === 'ENVIRONMENT' ? '环境变量' : '未配置'}</span>
          <div className="flex gap-2"><button className="rounded-lg border border-slate-300 px-4 py-2 text-sm font-semibold disabled:opacity-50" type="button" disabled={busy} onClick={() => void test()}>测试连接</button><button className="rounded-lg bg-cyan-600 px-4 py-2 text-sm font-semibold text-white disabled:opacity-50" type="submit" disabled={busy}>保存 AI 配置</button></div>
        </div>
      </form>
      {message ? <p className="mt-4 rounded-lg bg-emerald-50 px-3 py-2 text-sm text-emerald-700" role="status">{message}</p> : null}
      {error ? <p className="mt-4 rounded-lg bg-rose-50 px-3 py-2 text-sm text-rose-700" role="alert">{error}</p> : null}
    </section>
  );
}
