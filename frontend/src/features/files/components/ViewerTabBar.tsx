import { useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import type { ViewerTab } from '../viewerTabs';
import { FileIcon, SearchIcon } from './FileIcons';

type ViewerTabBarProps = {
  tabs: ViewerTab[];
  activeTabId: string | null;
  onActivate: (tab: ViewerTab) => void;
  onTogglePinned: (id: string) => void;
  onClose: (id: string) => void;
  onCloseMany: (ids: string[]) => void;
};

type TabMenu = { tabId: string; x: number; y: number };

const tabIcon = (tab: ViewerTab) => {
  if (tab.kind === 'file') return <FileIcon name={tab.title} />;
  return <SearchIcon className={tab.kind === 'temp' ? 'text-cyan-600' : 'text-sky-600'} />;
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
  onClose,
  onCloseMany
}: ViewerTabBarProps) {
  const [menu, setMenu] = useState<TabMenu | null>(null);

  useEffect(() => {
    if (!menu) return;
    const dismiss = () => setMenu(null);
    const dismissOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') dismiss();
    };
    window.addEventListener('pointerdown', dismiss);
    window.addEventListener('blur', dismiss);
    window.addEventListener('keydown', dismissOnEscape);
    return () => {
      window.removeEventListener('pointerdown', dismiss);
      window.removeEventListener('blur', dismiss);
      window.removeEventListener('keydown', dismissOnEscape);
    };
  }, [menu]);

  if (tabs.length === 0) return null;

  const contextIndex = menu ? tabs.findIndex((tab) => tab.id === menu.tabId) : -1;
  const contextTab = contextIndex >= 0 ? tabs[contextIndex] : null;
  const closeOthers = contextTab ? tabs.filter((tab) => tab.id !== contextTab.id && !tab.pinned).map((tab) => tab.id) : [];
  const closeRight = contextTab ? tabs.slice(contextIndex + 1).filter((tab) => !tab.pinned).map((tab) => tab.id) : [];
  const closeAll = tabs.filter((tab) => !tab.pinned).map((tab) => tab.id);

  const runAndDismiss = (action: () => void) => {
    action();
    setMenu(null);
  };

  return (
    <div className="flex min-h-14 items-end gap-2 overflow-x-auto border-b border-slate-200 bg-gradient-to-r from-slate-100 to-slate-50 px-4 pt-3">
      {tabs.map((tab) => (
        <div
          key={tab.id}
          onContextMenu={(event) => {
            event.preventDefault();
            setMenu({
              tabId: tab.id,
              x: Math.min(event.clientX, window.innerWidth - 220),
              y: Math.max(8, Math.min(event.clientY, window.innerHeight - 260))
            });
          }}
          className={`flex h-10 max-w-64 shrink-0 items-center gap-2 rounded-t-md border px-3 text-xs shadow-sm transition ${
            tab.id === activeTabId
              ? 'border-sky-300 bg-white text-sky-800 shadow-[inset_0_-3px_0_rgba(6,182,212,0.9),0_4px_14px_rgba(7,21,34,0.06)]'
              : 'border-slate-200 bg-white/75 text-slate-600 hover:bg-white hover:text-slate-900'
          }`}
        >
          <span className="flex shrink-0 items-center justify-center">
            {tabIcon(tab)}
          </span>
          <button
            type="button"
            className="min-w-0 flex-1 truncate text-left"
            title={tab.title}
            onClick={() => onActivate(tab)}
          >
            {tab.title}
          </button>
          {tab.pinned ? (
            <button
              type="button"
              className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-cyan-50 text-cyan-700 leading-none transition hover:bg-cyan-100 hover:text-cyan-800"
              title="取消固定"
              aria-label={`取消固定 ${tab.title}`}
              onClick={() => onTogglePinned(tab.id)}
            >
              <PinIcon pinned />
            </button>
          ) : null}
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
      {menu && contextTab
        ? createPortal(
            <div
              role="menu"
              aria-label={`${contextTab.title} 标签操作`}
              className="fixed z-[100] w-52 overflow-hidden rounded-lg border border-slate-200 bg-white py-1.5 text-[13px] text-slate-700 shadow-[0_18px_48px_rgba(7,21,34,0.22)]"
              style={{ left: menu.x, top: menu.y }}
              onPointerDown={(event) => event.stopPropagation()}
            >
              <button type="button" role="menuitem" className="flex w-full px-3 py-2 text-left hover:bg-sky-50 hover:text-sky-800" onClick={() => runAndDismiss(() => onTogglePinned(contextTab.id))}>
                {contextTab.pinned ? '取消固定标签' : '固定标签'}
              </button>
              <div className="my-1 border-t border-slate-100" />
              <button type="button" role="menuitem" className="flex w-full px-3 py-2 text-left hover:bg-sky-50 hover:text-sky-800" onClick={() => runAndDismiss(() => onClose(contextTab.id))}>
                关闭当前标签
              </button>
              <button type="button" role="menuitem" disabled={closeOthers.length === 0} className="flex w-full px-3 py-2 text-left hover:bg-sky-50 hover:text-sky-800 disabled:cursor-not-allowed disabled:text-slate-300 disabled:hover:bg-transparent" onClick={() => runAndDismiss(() => onCloseMany(closeOthers))}>
                关闭其他标签
              </button>
              <button type="button" role="menuitem" disabled={closeRight.length === 0} className="flex w-full px-3 py-2 text-left hover:bg-sky-50 hover:text-sky-800 disabled:cursor-not-allowed disabled:text-slate-300 disabled:hover:bg-transparent" onClick={() => runAndDismiss(() => onCloseMany(closeRight))}>
                关闭右侧标签
              </button>
              <div className="my-1 border-t border-slate-100" />
              <button type="button" role="menuitem" disabled={closeAll.length === 0} className="flex w-full px-3 py-2 text-left hover:bg-rose-50 hover:text-rose-700 disabled:cursor-not-allowed disabled:text-slate-300 disabled:hover:bg-transparent" onClick={() => runAndDismiss(() => onCloseMany(closeAll))}>
                关闭全部未固定标签
              </button>
            </div>,
            document.body
          )
        : null}
    </div>
  );
}
