import { useState } from 'react';
import { NavTabs } from './components/NavTabs';
import { FilesView } from './features/files/FilesView';
import { LogsView } from './features/logs/LogsView';
import type { BundleInfo } from './lib/bundles';
import './App.css';

type TabId = 'files' | 'logs';

const tabs: { id: TabId; label: string; hint: string }[] = [
  { id: 'files', label: 'Files View', hint: '浏览上传结构' },
  { id: 'logs', label: 'Logs View', hint: '全文搜索日志' }
];

function App() {
  const [activeTab, setActiveTab] = useState<TabId>('files');
  const [activeBundle, setActiveBundle] = useState<BundleInfo | null>(null);
  const [recentBundles, setRecentBundles] = useState<BundleInfo[]>([]);

  const handleBundleSelected = (bundle: BundleInfo) => {
    setActiveBundle(bundle);
    setRecentBundles((prev) => {
      const filtered = prev.filter((item) => item.hash !== bundle.hash);
      return [bundle, ...filtered].slice(0, 6);
    });
  };

  return (
    <div className="min-h-screen bg-slate-950 text-slate-100">
      <header className="border-b border-slate-800 bg-slate-900/60 backdrop-blur">
        <div className="mx-auto flex max-w-6xl items-center justify-between px-6 py-4">
          <div>
            <p className="text-sm uppercase tracking-[0.3em] text-brand-500">Rain</p>
            <h1 className="text-2xl font-semibold text-white">日志解析控制台</h1>
          </div>
          <p className="text-sm text-slate-400">API: {import.meta.env.VITE_API_BASE_URL ?? 'http://localhost:8080'}</p>
        </div>
        <div className="mx-auto max-w-6xl px-6">
          <NavTabs tabs={tabs} activeId={activeTab} onChange={setActiveTab} />
        </div>
      </header>

      <main className="mx-auto max-w-6xl px-6 py-6">
        {activeTab === 'files' ? (
          <FilesView activeBundle={activeBundle} onBundleSelected={handleBundleSelected} />
        ) : (
          <LogsView
            activeBundle={activeBundle}
            recentBundles={recentBundles}
            onBundleSelected={handleBundleSelected}
          />
        )}
      </main>
    </div>
  );
}

export default App;
