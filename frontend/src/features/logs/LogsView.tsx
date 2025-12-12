import { FormEvent, useState } from 'react';
import { rainApi } from '../../api/client';
import type { LogSearchResponse } from '../../api/types';

export function LogsView() {
  const [bundleId, setBundleId] = useState('lp1yp7');
  const [query, setQuery] = useState('error');
  const [timeline, setTimeline] = useState<string>('');
  const [result, setResult] = useState<LogSearchResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSearch = async (event: FormEvent) => {
    event.preventDefault();
    if (!bundleId.trim() || !query.trim()) return;
    setLoading(true);
    setError(null);
    try {
      const response = await rainApi.searchLogs(bundleId.trim(), query.trim(), timeline.trim() || undefined);
      setResult(response);
    } catch (err) {
      setError((err as Error).message || '搜索失败');
      setResult(null);
    } finally {
      setLoading(false);
    }
  };

  return (
    <section className="panel space-y-4">
      <header>
        <h2 className="text-lg font-semibold text-white">日志搜索</h2>
        <p className="text-sm text-slate-400">基于后端 `tsvector` 索引的关键词匹配。</p>
      </header>

      <form onSubmit={handleSearch} className="grid gap-3 md:grid-cols-[1fr_1fr_0.8fr_auto]">
        <input
          className="rounded-lg border border-slate-700 bg-slate-900 px-4 py-2 text-white focus:border-brand-500 focus:outline-none"
          placeholder="bundleId"
          value={bundleId}
          onChange={(event) => setBundleId(event.target.value)}
        />
        <input
          className="rounded-lg border border-slate-700 bg-slate-900 px-4 py-2 text-white focus:border-brand-500 focus:outline-none"
          placeholder="关键词，例如 error"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
        <input
          className="rounded-lg border border-slate-700 bg-slate-900 px-4 py-2 text-white focus:border-brand-500 focus:outline-none"
          placeholder="可选 timeline"
          value={timeline}
          onChange={(event) => setTimeline(event.target.value)}
        />
        <button
          type="submit"
          className="rounded-lg bg-brand-500 px-6 py-2 font-semibold text-slate-900 transition hover:bg-brand-700"
          disabled={loading}
        >
          {loading ? '搜索中...' : '搜索'}
        </button>
      </form>

      {error ? <p className="text-sm text-rose-300">{error}</p> : null}

      {result ? (
        <div className="space-y-2">
          <p className="text-sm text-slate-400">
            命中 {result.total} 条，展示 {result.hits.length} 条：
          </p>
          <ul className="space-y-3">
            {result.hits.map((hit) => (
              <li key={`${hit.file_id}-${hit.offset ?? hit.path}`} className="rounded-lg border border-slate-800 bg-slate-900 p-4">
                <p className="text-xs uppercase text-slate-500">
                  {hit.timeline ?? 'all'} · {hit.path}
                </p>
                <pre className="mt-2 whitespace-pre-wrap text-sm text-slate-100">{hit.snippet}</pre>
              </li>
            ))}
          </ul>
        </div>
      ) : (
        <p className="text-sm text-slate-500">输入 bundleId 和关键词后开始搜索。</p>
      )}
    </section>
  );
}
