import { useCallback, useEffect, useState } from 'react';
import { useNavigate, useParams } from 'react-router-dom';
import { normalizeApiError, rainApi } from '../../api/client';
import type { TempResultInfo, TempResultLinesResponse } from '../../api/types';

const PAGE_SIZES = [1000, 3000] as const;

export function TempResultView() {
  const { resultId = '' } = useParams<{ resultId: string }>();
  const navigate = useNavigate();
  const [result, setResult] = useState<TempResultInfo | null>(null);
  const [lines, setLines] = useState<TempResultLinesResponse | null>(null);
  const [start, setStart] = useState(0);
  const [pageSize, setPageSize] = useState<number>(PAGE_SIZES[0]);
  const [expression, setExpression] = useState('');
  const [loading, setLoading] = useState(true);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!resultId) return;
    setLoading(true);
    setError(null);
    try {
      const [metadata, content] = await Promise.all([
        rainApi.fetchTempResult(resultId),
        rainApi.fetchTempResultLines(resultId, { start, limit: pageSize })
      ]);
      setResult(metadata);
      setLines(content);
    } catch (loadError) {
      setError(normalizeApiError(loadError));
    } finally {
      setLoading(false);
    }
  }, [pageSize, resultId, start]);

  useEffect(() => {
    load().catch(() => undefined);
  }, [load]);

  const createFromResult = async () => {
    if (!expression.trim() || !resultId) return;
    setCreating(true);
    setError(null);
    try {
      const created = await rainApi.createTempResult({
        expression: expression.trim(),
        source_temp_id: resultId
      });
      navigate(`/temp-results/${created.id}`);
    } catch (createError) {
      setError(normalizeApiError(createError));
    } finally {
      setCreating(false);
    }
  };

  if (loading && !result) {
    return <p className="py-12 text-center text-sm text-slate-500">临时结果加载中...</p>;
  }

  return (
    <section className="panel space-y-4">
      {error ? <p className="rounded border border-rose-900/60 bg-rose-950/30 p-3 text-sm text-rose-300">{error}</p> : null}
      {result ? (
        <>
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div className="min-w-0">
              <p className="text-xs text-cyan-300">临时日志结果</p>
              <h2 className="truncate text-lg font-semibold text-white">{result.name}</h2>
              <p className="mt-1 text-xs text-slate-500">
                来源：{result.source_label} · 表达式：{result.expression} · 到期：{new Date(result.expires_at).toLocaleString()}
              </p>
            </div>
            <div className="flex flex-wrap gap-2 text-xs">
              <button
                type="button"
                className="rounded border border-rose-900/70 px-3 py-1.5 text-rose-300 hover:border-rose-700"
                onClick={async () => {
                  if (!window.confirm('确定删除这个临时结果吗？')) return;
                  await rainApi.deleteTempResult(resultId);
                  navigate('/');
                }}
              >
                删除
              </button>
            </div>
          </div>

          <div className="flex flex-wrap items-center gap-2 rounded-lg border border-slate-700 bg-slate-950/60 px-3 py-2">
            <input
              className="min-w-[220px] flex-1 bg-transparent text-sm text-white outline-none placeholder:text-slate-500"
              placeholder='继续过滤，例如：(ERROR OR WARN) AND NOT heartbeat'
              value={expression}
              onChange={(event) => setExpression(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter') createFromResult().catch(() => undefined);
              }}
            />
            <button
              type="button"
              className="rounded bg-cyan-500 px-3 py-1.5 text-xs font-semibold text-slate-950 disabled:opacity-50"
              disabled={creating || !expression.trim()}
              onClick={() => createFromResult().catch(() => undefined)}
            >
              {creating ? '搜索中...' : '搜索'}
            </button>
          </div>

          <div className="min-h-[65vh] overflow-auto rounded-lg bg-slate-950/70 p-3 text-xs leading-5 text-slate-100">
            <div className="grid grid-cols-[auto_1fr] gap-3 font-mono">
              <div className="select-none text-right text-slate-600">
                {lines?.lines.map((line) => <div key={line.line_number}>{line.line_number + 1}</div>)}
              </div>
              <div>
                {lines?.lines.map((line) => <div key={line.line_number} className="whitespace-pre">{line.content}</div>)}
              </div>
            </div>
          </div>

          {lines ? (
            <div className="flex flex-wrap items-center justify-end gap-2 text-xs text-slate-400">
              <select
                className="rounded border border-slate-700 bg-slate-950 px-2 py-1 text-slate-200"
                value={pageSize}
                onChange={(event) => {
                  setPageSize(Number(event.target.value));
                  setStart(0);
                }}
              >
                {PAGE_SIZES.map((size) => <option key={size} value={size}>{size} 行/页</option>)}
              </select>
              <span>第 {Math.floor(start / pageSize) + 1} / {Math.max(1, Math.ceil(lines.line_count / pageSize))} 页</span>
              <button
                type="button"
                className="rounded border border-slate-700 px-3 py-1 disabled:opacity-50"
                disabled={start === 0 || loading}
                onClick={() => setStart(Math.max(0, start - pageSize))}
              >上一页</button>
              <button
                type="button"
                className="rounded border border-slate-700 px-3 py-1 disabled:opacity-50"
                disabled={!lines.next_start || loading}
                onClick={() => setStart(lines.next_start ?? start + pageSize)}
              >下一页</button>
            </div>
          ) : null}
        </>
      ) : null}
    </section>
  );
}
