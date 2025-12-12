type Tab = {
  id: string;
  label: string;
  hint?: string;
};

interface NavTabsProps {
  tabs: Tab[];
  activeId: string;
  onChange: (id: string) => void;
}

export function NavTabs({ tabs, activeId, onChange }: NavTabsProps) {
  return (
    <nav className="flex gap-4 border-b border-slate-800 text-sm text-slate-400">
      {tabs.map((tab) => {
        const isActive = tab.id === activeId;
        return (
          <button
            key={tab.id}
            type="button"
            onClick={() => onChange(tab.id)}
            className={[
              'py-3',
              'border-b-2',
              isActive ? 'border-brand-500 text-white' : 'border-transparent hover:text-slate-100'
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
