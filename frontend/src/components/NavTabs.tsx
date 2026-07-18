type Tab<T extends string = string> = {
  id: T;
  label: string;
  hint?: string;
};

interface NavTabsProps<T extends string = string> {
  tabs: Tab<T>[];
  activeId: T;
  onChange: (id: T) => void;
}

export function NavTabs<T extends string>({
  tabs,
  activeId,
  onChange
}: NavTabsProps<T>) {
  return (
    <nav className="flex gap-2 rounded-xl border border-slate-200 bg-slate-100/80 p-1 text-sm text-slate-500">
      {tabs.map((tab) => {
        const isActive = tab.id === activeId;
        return (
          <button
            key={tab.id}
            type="button"
            onClick={() => onChange(tab.id)}
            className={[
              'rounded-lg px-4 py-2.5 transition',
              isActive ? 'bg-white text-slate-950 shadow-sm' : 'hover:bg-white/60 hover:text-slate-900'
            ].join(' ')}
          >
            <div className="font-semibold">{tab.label}</div>
            {tab.hint ? <div className="text-xs text-slate-500">{tab.hint}</div> : null}
          </button>
        );
      })}
    </nav>
  );
}
