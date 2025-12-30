import { useState } from 'react';
import { FilesView } from './features/files/FilesView';
import type { BundleInfo } from './lib/bundles';
import './App.css';

function App() {
  const [activeBundle, setActiveBundle] = useState<BundleInfo | null>(null);

  const handleBundleSelected = (bundle: BundleInfo) => {
    setActiveBundle(bundle);
  };

  return (
    <div className="min-h-screen bg-slate-950 text-slate-100">
      <header className="border-b border-slate-800 bg-slate-900/60 backdrop-blur">
        <div className="mx-auto flex max-w-6xl items-start justify-between gap-4 px-6 py-6">
          <div>
            <p className="text-xs uppercase tracking-[0.3em] text-brand-500">Rain</p>
            <h1 className="text-3xl font-semibold text-white">Issue 控制台</h1>
            <p className="mt-1 text-sm text-slate-400">创建 Issue、上传文件后，直接在同一界面浏览结构与预览文本。</p>
          </div>
          <div className="rounded-lg border border-slate-800 bg-slate-900/70 px-4 py-3 text-sm text-slate-300">
            <p className="text-xs uppercase tracking-wide text-slate-500">当前选择</p>
            {activeBundle ? (
              <div className="mt-1 space-y-1">
                <p className="font-semibold text-white">{activeBundle.issue ?? '未命名 Issue'}</p>
                <p className="font-mono text-xs text-slate-400">{activeBundle.hash}</p>
              </div>
            ) : (
              <p className="mt-1 text-slate-500">尚未选择 Issue/Bundles，上传或查询后自动定位。</p>
            )}
          </div>
        </div>
      </header>

      <main className="mx-auto max-w-6xl px-6 py-6">
        <FilesView activeBundle={activeBundle} onBundleSelected={handleBundleSelected} />
      </main>
    </div>
  );
}

export default App;
