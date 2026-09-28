import type { SearchExecutionSnapshot } from '../hooks/useSearchExecution';

interface SearchExecutionStatusProps {
  snapshot: SearchExecutionSnapshot;
  onCancel: () => void;
}

function formatElapsed(elapsedMs: number): string {
  const totalSeconds = Math.floor(elapsedMs / 1_000);
  return `${String(Math.floor(totalSeconds / 60)).padStart(2, '0')}:${String(totalSeconds % 60).padStart(2, '0')}`;
}

export function SearchExecutionStatus({ snapshot, onCancel }: SearchExecutionStatusProps) {
  if (snapshot.status === 'IDLE') return null;

  const running = snapshot.status === 'RUNNING';
  const cancelling = snapshot.status === 'CANCELLING';
  const message = snapshot.status === 'SUCCEEDED'
    ? `搜索完成 · ${formatElapsed(snapshot.elapsedMs)}`
    : snapshot.status === 'CANCELLED'
      ? `搜索已取消 · ${formatElapsed(snapshot.elapsedMs)}`
      : snapshot.status === 'FAILED'
        ? snapshot.errorMessage ?? '搜索失败'
        : cancelling
          ? snapshot.cancelUnconfirmed ? '取消请求未确认，可重试；服务器安全时限仍生效' : '正在停止搜索…'
          : `搜索中… ${formatElapsed(snapshot.elapsedMs)}`;

  return (
    <div className="mt-2 flex flex-wrap items-center gap-2 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-xs text-slate-600" aria-live="polite">
      {(running || cancelling) && <div className="h-1.5 min-w-[120px] flex-1 animate-pulse rounded-full bg-cyan-200 motion-reduce:animate-none" role="progressbar" aria-label="搜索进行中" />}
      <span>{message}</span>
      {running && <button className="rounded border border-slate-300 px-2 py-1 font-semibold hover:border-slate-500" type="button" onClick={onCancel}>取消搜索</button>}
      {cancelling && snapshot.cancelUnconfirmed && <button className="rounded border border-slate-300 px-2 py-1 font-semibold hover:border-slate-500" type="button" onClick={onCancel}>重试取消</button>}
    </div>
  );
}
