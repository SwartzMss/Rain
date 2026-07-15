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

export function ViewerTabBar({
  tabs,
  activeTabId,
  onActivate,
  onTogglePinned,
  onClose
}: ViewerTabBarProps) {
  if (tabs.length === 0) return null;

  return (
    <div className="flex min-h-14 items-end gap-2 overflow-x-auto border-b border-slate-200 bg-slate-50 px-4 pt-3">
      {tabs.map((tab) => (
        <div
          key={tab.id}
          className={`flex h-10 max-w-64 shrink-0 items-center gap-2 rounded-t-md border px-3 text-xs shadow-sm transition ${
            tab.id === activeTabId
              ? 'border-sky-200 bg-white text-sky-700 shadow-[inset_0_-2px_0_rgba(37,99,235,0.85)]'
              : 'border-slate-200 bg-white/75 text-slate-600 hover:bg-white hover:text-slate-900'
          }`}
        >
          <span className={tab.kind === 'temp' ? 'text-cyan-700' : tab.kind === 'search' ? 'text-brand-700' : 'text-slate-500'}>
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
            className={tab.pinned ? 'text-cyan-700' : 'text-slate-600 hover:text-slate-600'}
            title={tab.pinned ? '取消固定' : '固定标签'}
            aria-label={tab.pinned ? '取消固定' : '固定标签'}
            onClick={() => onTogglePinned(tab.id)}
          >
            {tab.pinned ? '●' : '○'}
          </button>
          <button
            type="button"
            className="text-slate-600 hover:text-rose-600"
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
