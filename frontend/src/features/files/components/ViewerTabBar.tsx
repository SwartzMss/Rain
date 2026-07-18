import type { ViewerTab } from '../viewerTabs';

type ViewerTabBarProps = {
  tabs: ViewerTab[];
  activeTabId: string | null;
  onActivate: (tab: ViewerTab) => void;
  onTogglePinned: (id: string) => void;
  onClose: (id: string) => void;
};

const tabIcon = (kind: ViewerTab['kind']) => {
  if (kind === 'file') return '□';
  if (kind === 'search') return '⌕';
  return '◈';
};

function PinIcon({ pinned }: { pinned: boolean }) {
  return (
    <svg
      aria-hidden="true"
      viewBox="0 0 24 24"
      className="h-[18px] w-[18px]"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.9"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path
        d="M6 3h12l-1.2 7 3.2 3H4l3.2-3L6 3Z"
        fill={pinned ? 'currentColor' : 'none'}
        fillOpacity={pinned ? 0.2 : 0}
      />
      <path d="M12 13v8" />
    </svg>
  );
}

export function ViewerTabBar({
  tabs,
  activeTabId,
  onActivate,
  onTogglePinned,
  onClose
}: ViewerTabBarProps) {
  if (tabs.length === 0) return null;

  return (
    <div className="flex min-h-14 items-end gap-2 overflow-x-auto border-b border-slate-200 bg-gradient-to-r from-slate-100 to-slate-50 px-4 pt-3">
      {tabs.map((tab) => (
        <div
          key={tab.id}
          className={`flex h-10 max-w-64 shrink-0 items-center gap-2 rounded-t-md border px-3 text-xs shadow-sm transition ${
            tab.id === activeTabId
              ? 'border-sky-300 bg-white text-sky-800 shadow-[inset_0_-3px_0_rgba(6,182,212,0.9),0_4px_14px_rgba(7,21,34,0.06)]'
              : 'border-slate-200 bg-white/75 text-slate-600 hover:bg-white hover:text-slate-900'
          }`}
        >
          <span className={`shrink-0 text-lg font-semibold leading-none ${tab.kind === 'temp' ? 'text-cyan-700' : tab.kind === 'search' ? 'text-brand-700' : 'text-slate-500'}`}>
            {tabIcon(tab.kind)}
          </span>
          <button
            type="button"
            className="min-w-0 flex-1 truncate text-left"
            title={tab.title}
            onClick={() => onActivate(tab)}
          >
            {tab.title}
          </button>
          <button
            type="button"
            className={`flex h-7 w-7 shrink-0 items-center justify-center rounded-lg leading-none transition hover:bg-slate-100 ${tab.pinned ? 'bg-cyan-50 text-cyan-700' : 'text-slate-500 hover:text-slate-800'}`}
            title={tab.pinned ? '取消固定' : '固定标签'}
            aria-label={tab.pinned ? '取消固定' : '固定标签'}
            onClick={() => onTogglePinned(tab.id)}
          >
            <PinIcon pinned={tab.pinned} />
          </button>
          <button
            type="button"
            className="flex h-7 w-7 shrink-0 items-center justify-center rounded text-2xl font-light leading-none text-slate-500 transition hover:bg-rose-50 hover:text-rose-600"
            aria-label={`关闭 ${tab.title}`}
            onClick={() => onClose(tab.id)}
          >
            ×
          </button>
        </div>
      ))}
    </div>
  );
}
