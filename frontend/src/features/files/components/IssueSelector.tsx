import type { IssueSummary } from '../../../api/types';

type IssueSelectorProps = {
  currentIssueCode: string;
  filteredIssues: IssueSummary[];
  issueError: string | null;
  issueSearchText: string;
  issuesError: string | null;
  issuesLoading: boolean;
  onCreateClick: () => void;
  onIssueSearchTextChange: (value: string) => void;
  onRefreshIssues: () => void;
  onSelectIssue: (value: string) => void;
  onViewIssue: (issueCode: string) => void;
};

export function IssueSelector({
  currentIssueCode,
  filteredIssues,
  issueError,
  issueSearchText,
  issuesError,
  issuesLoading,
  onCreateClick,
  onIssueSearchTextChange,
  onRefreshIssues,
  onSelectIssue,
  onViewIssue
}: IssueSelectorProps) {
  return (
    <aside className="flex min-h-[680px] flex-col rounded-2xl border border-slate-200/90 bg-white/95 p-4 shadow-[0_18px_48px_rgba(7,21,34,0.08)] backdrop-blur">
      <div className="mb-4 flex items-center justify-between gap-3">
        <h2 className="text-lg font-semibold text-slate-950">Issues</h2>
        <button
          type="button"
          className="rounded-lg bg-slate-950 px-3 py-2 text-sm font-semibold text-white shadow-sm transition hover:bg-slate-800"
          onClick={onCreateClick}
        >
          + 新建 Issue
        </button>
      </div>

      <div className="relative">
        <input
          className="w-full rounded-xl border border-slate-200 bg-slate-50 px-4 py-2.5 pr-10 text-sm text-slate-950 outline-none transition focus:border-sky-400 focus:bg-white focus:shadow-[0_0_0_3px_rgba(6,182,212,0.12)]"
          placeholder="搜索 Issue ID..."
          value={issueSearchText}
          onChange={(event) => onIssueSearchTextChange(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter') {
              event.preventDefault();
              onSelectIssue(issueSearchText);
            }
          }}
        />
        <span className="absolute right-3 top-2.5 text-slate-500">⌕</span>
      </div>

      {issueError ? <p className="mt-2 text-xs text-rose-600">{issueError}</p> : null}
      {issuesError ? (
        <div className="mt-3 rounded-lg border border-rose-500/30 bg-rose-500/10 p-3 text-xs text-rose-700">
          <p>{issuesError}</p>
          <button
            type="button"
            className="mt-2 rounded-lg border border-rose-300 bg-white/70 px-2.5 py-1.5 font-semibold text-rose-700 transition hover:bg-white disabled:opacity-60"
            onClick={onRefreshIssues}
            disabled={issuesLoading}
          >
            {issuesLoading ? '连接中...' : '重新连接'}
          </button>
        </div>
      ) : null}

      <div className="mt-5 flex-1 space-y-2 overflow-y-auto">
        {filteredIssues.map((issue) => {
          const active = issue.code === currentIssueCode;
          return (
            <button
              key={issue.code}
              type="button"
              title="双击查看 Issue 日志"
              className={[
                'flex w-full items-center justify-between rounded-xl border px-3 py-3 text-left transition',
                active ? 'border-sky-200 bg-gradient-to-r from-sky-100 to-cyan-50 text-slate-950 shadow-sm' : 'border-transparent text-slate-700 hover:border-slate-200 hover:bg-slate-50'
              ].join(' ')}
              onClick={() => onSelectIssue(issue.code)}
              onDoubleClick={() => onViewIssue(issue.code)}
            >
              <span className="min-w-0 flex-1">
                <span className="block truncate font-semibold">{issue.code}</span>
                <span className="block text-[10px] font-normal text-slate-500">双击查看日志</span>
              </span>
            </button>
          );
        })}
        {!filteredIssues.length ? (
          <p className="rounded-lg border border-slate-200 bg-slate-50 p-3 text-sm text-slate-500">
            暂无 Issue
          </p>
        ) : null}
      </div>
    </aside>
  );
}
