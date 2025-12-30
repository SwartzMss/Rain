import { Link, Route, Routes, useLocation } from 'react-router-dom';
import { BundleView } from './features/files/FilesView';
import { HomeView } from './features/files/HomeView';
import './App.css';

function App() {
  const location = useLocation();
  const onBundlePage = location.pathname.startsWith('/bundle');

  return (
    <div className="min-h-screen bg-slate-950 text-slate-100">
      <header className="border-b border-slate-800 bg-slate-900/60 backdrop-blur">
        <div className="mx-auto flex max-w-6xl flex-col gap-4 px-6 py-6 lg:flex-row lg:items-center lg:justify-between">
          <div>
            <p className="text-xs uppercase tracking-[0.3em] text-brand-500">Rain</p>
            <h1 className="text-3xl font-semibold text-white">Issue 控制台</h1>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <Link
              to="/"
              className="rounded-lg border border-slate-800 bg-slate-900/70 px-4 py-2 text-sm font-semibold text-white transition hover:border-slate-700 hover:bg-slate-800"
            >
              工作台
            </Link>
            {onBundlePage ? (
              <Link
                to="/"
                className="rounded-lg border border-brand-500/60 bg-brand-500/10 px-4 py-2 text-sm font-semibold text-brand-100 transition hover:border-brand-500 hover:bg-brand-500/20"
              >
                返回上传/查询
              </Link>
            ) : null}
          </div>
        </div>
      </header>

      <main className="mx-auto max-w-6xl px-6 py-6">
        <Routes>
          <Route path="/" element={<HomeView />} />
          <Route path="/bundle/:bundleHash" element={<BundleView />} />
        </Routes>
      </main>
    </div>
  );
}

export default App;
