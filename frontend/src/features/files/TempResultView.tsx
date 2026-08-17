import { useCallback, useEffect, useRef, useState } from 'react';
import { useNavigate, useParams } from 'react-router-dom';
import { normalizeApiError, rainApi } from '../../api/client';
import type { TempResultInfo, TempResultLinesResponse } from '../../api/types';
import { useAuth } from '../../auth/AuthContext';
import { LINE_PAGE_SIZE_OPTIONS } from './linePageSizes';
import { isUser } from '../../auth/permissions';

type PageNavigation = 'next' | 'previous' | 'reset';

const DEFAULT_PAGE_SIZE = LINE_PAGE_SIZE_OPTIONS[0];

export function TempResultView() {
  const auth = useAuth();
  const { resultId = '' } = useParams<{ resultId: string }>();
  const navigate = useNavigate();
  const [result, setResult] = useState<TempResultInfo | null>(null);
  const [lines, setLines] = useState<TempResultLinesResponse | null>(null);
  const [start, setStart] = useState(0);
  const [pageHistory, setPageHistory] = useState<number[]>([]);
  const [pageSize, setPageSize] = useState<number>(DEFAULT_PAGE_SIZE);
  const [expression, setExpression] = useState('');
  const [loading, setLoading] = useState(true);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestGeneration = useRef(0);

  useEffect(() => {
    const generation = ++requestGeneration.current;
    setResult(null);
    setLines(null);
    setStart(0);
    setPageHistory([]);
    setPageSize(DEFAULT_PAGE_SIZE);
    setLoading(true);
    setError(null);

    const loadInitial = async () => {
      if (!resultId) return;
      try {
        const [metadata, content] = await Promise.all([
          rainApi.fetchTempResult(resultId),
          rainApi.fetchTempResultLines(resultId, { start: 0, limit: DEFAULT_PAGE_SIZE })
        ]);
        if (generation !== requestGeneration.current) return;
        setResult(metadata);
        setLines(content);
      } catch (loadError) {
        if (generation === requestGeneration.current) {
          setError(normalizeApiError(loadError));
        }
      } finally {
        if (generation === requestGeneration.current) {
          setLoading(false);
        }
      }
    };

    void loadInitial();
    return () => {
      if (generation === requestGeneration.current) {
        requestGeneration.current += 1;
      }
    };
  }, [resultId]);

  const loadPage = useCallback(async (
    requestedStart: number,
    requestedPageSize: number,
    navigation: PageNavigation,
    previousStart?: number
  ) => {
    const generation = ++requestGeneration.current;
    setLoading(true);
    setError(null);
    try {
      const content = await rainApi.fetchTempResultLines(resultId, {
        start: requestedStart,
        limit: requestedPageSize
      });
      if (generation !== requestGeneration.current) return;
      setLines(content);
      setStart(content.start);
      setPageSize(requestedPageSize);
      setPageHistory((history) => {
        if (navigation === 'next') {
          return [...history, previousStart ?? content.start];
        }
        if (navigation === 'previous') {
          return history.slice(0, -1);
        }
        return [];
      });
    } catch (loadError) {
      if (generation === requestGeneration.current) {
        setError(normalizeApiError(loadError));
      }
    } finally {
      if (generation === requestGeneration.current) {
        setLoading(false);
      }
    }
  }, [resultId]);

  const visibleResult = result?.id === resultId ? result : null;
  const visibleLines = visibleResult ? lines : null;

  const createFromResult = async () => {
    if (!expression.trim() || !resultId) return;
    setCreating(true);
    setError(null);
    try {
      const created = await rainApi.previewTempResult({
        expression: expression.trim(),
        source_temp_id: resultId,
        from: 0,
        size: LINE_PAGE_SIZE_OPTIONS[0]
      });
      navigate(`/temp-results/${created.result_id}`);
    } catch (createError) {
      setError(normalizeApiError(createError));
    } finally {
      setCreating(false);
    }
  };

  const deleteResult = async () => {
    if (!window.confirm('确定删除这个临时结果吗？')) return;
    setError(null);
    try {
      await rainApi.deleteTempResult(resultId);
      navigate('/');
    } catch (deleteError) {
      setError(normalizeApiError(deleteError));
    }
  };

  if (loading && !visibleResult) {
    return <p className="py-12 text-center text-sm text-slate-500">临时结果加载中...</p>;
  }

  return (
    <section className="panel space-y-4">
      {error ? <p className="rounded border border-rose-200 bg-rose-50 p-3 text-sm text-rose-600">{error}</p> : null}
      {visibleResult ? (
        <>
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div className="min-w-0">
              <p className="text-xs text-cyan-700">临时日志结果</p>
              <h2 className="truncate text-lg font-semibold text-slate-950">{visibleResult.name}</h2>
              <p className="mt-1 text-xs text-slate-500">
                来源：{visibleResult.source_label} · 表达式：{visibleResult.expression} · 到期：{new Date(visibleResult.expires_at).toLocaleString()}
              </p>
            </div>
            <div className="flex flex-wrap gap-2 text-xs">
              {auth.state.status === 'AUTHENTICATED' && isUser(auth.state.user) ? (
                <button
                  type="button"
                  className="rounded border border-rose-300 px-3 py-1.5 text-rose-600 hover:border-rose-700"
                  onClick={() => void deleteResult()}
                >
                  删除
                </button>
              ) : null}
            </div>
          </div>

          <div className="flex flex-wrap items-center gap-2 rounded-lg border border-slate-300 bg-slate-50 px-3 py-2">
            <input
              className="min-w-[220px] flex-1 bg-transparent text-sm text-slate-950 outline-none placeholder:text-slate-500"
              placeholder='继续过滤，例如：(ERROR OR WARN) AND NOT heartbeat'
              value={expression}
              onChange={(event) => setExpression(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter') createFromResult().catch(() => undefined);
              }}
            />
            <button
              type="button"
              className="rounded bg-cyan-600 px-3 py-1.5 text-xs font-semibold text-white disabled:opacity-50"
              disabled={creating || !expression.trim()}
              onClick={() => createFromResult().catch(() => undefined)}
            >
              {creating ? '搜索中...' : '搜索'}
            </button>
          </div>

          <div className="min-h-[65vh] overflow-auto rounded-lg bg-white p-3 text-xs leading-5 text-slate-900">
            <div className="grid grid-cols-[auto_1fr] gap-3 font-mono">
              <div className="select-none text-right text-slate-600">
                {visibleLines?.lines.map((line) => {
                  const lineKey = `${line.bundle_hash ?? ''}:${line.file_id ?? line.path ?? ''}:${line.line_number}`;
                  return <div key={lineKey}>{line.path ? `${line.path}:` : ''}{line.line_number + 1}</div>;
                })}
              </div>
              <div>
                {visibleLines?.lines.map((line) => {
                  const lineKey = `${line.bundle_hash ?? ''}:${line.file_id ?? line.path ?? ''}:${line.line_number}`;
                  return <div key={lineKey} className="whitespace-pre">{line.content}</div>;
                })}
              </div>
            </div>
          </div>

          {visibleLines ? (
            <div className="flex flex-wrap items-center justify-end gap-2 text-xs text-slate-500">
              <select
                className="rounded border border-slate-300 bg-white px-2 py-1 text-slate-700"
                value={pageSize}
                onChange={(event) => {
                  void loadPage(0, Number(event.target.value), 'reset');
                }}
              >
                {LINE_PAGE_SIZE_OPTIONS.map((size) => <option key={size} value={size}>{size} 行/页</option>)}
              </select>
              <span>{start + 1} - {start + visibleLines.lines.length} / {visibleLines.line_count}</span>
              <button
                type="button"
                className="rounded border border-slate-300 px-3 py-1 disabled:opacity-50"
                disabled={pageHistory.length === 0 || loading}
                onClick={() => {
                  const previousStart = pageHistory[pageHistory.length - 1];
                  if (previousStart === undefined) return;
                  void loadPage(previousStart, pageSize, 'previous');
                }}
              >上一页</button>
              <button
                type="button"
                className="rounded border border-slate-300 px-3 py-1 disabled:opacity-50"
                disabled={!visibleLines.next_start || loading}
                onClick={() => {
                  const nextStart = visibleLines.next_start ?? start + visibleLines.lines.length;
                  void loadPage(nextStart, pageSize, 'next', start);
                }}
              >下一页</button>
            </div>
          ) : null}
        </>
      ) : null}
    </section>
  );
}

export function TempResultRoute() {
  const { resultId = '' } = useParams<{ resultId: string }>();
  return <TempResultView key={resultId} />;
}
