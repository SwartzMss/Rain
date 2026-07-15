import { Link, Route, Routes, useLocation, useParams } from 'react-router-dom';
import { BundleView } from './features/files/FilesView';
import { HomeView } from './features/files/HomeView';
import { TempResultView } from './features/files/TempResultView';
import { APP_VERSION } from './version';
import './App.css';

function App() {
  return (
    <div className="min-h-screen text-slate-900">
      <header className="sticky top-0 z-40 border-b border-slate-200/80 bg-white/90 shadow-sm shadow-slate-200/70 backdrop-blur">
        <div className="mx-auto flex h-16 w-full max-w-none items-center justify-between gap-3 px-6">
          <Link to="/" className="text-slate-950 no-underline">
            <div className="flex flex-wrap items-center gap-2.5">
              <span className="flex h-9 w-9 items-center justify-center rounded-lg border border-sky-200 bg-sky-100 text-lg text-sky-700 shadow-sm shadow-sky-100">☁</span>
              <h1 className="text-2xl font-semibold tracking-tight text-slate-950">Rain</h1>
              <span className="rounded-md border border-sky-200 bg-sky-50 px-2 py-0.5 text-xs font-semibold text-sky-700">
                {APP_VERSION}
              </span>
            </div>
          </Link>
          <div className="flex items-center gap-2 text-sm font-medium text-slate-700">
            <span className="h-2.5 w-2.5 rounded-full bg-emerald-400" />
            <span>服务正常</span>
            <span className="text-slate-400">⌄</span>
          </div>
        </div>
      </header>

      <main className="mx-auto w-full max-w-none px-5 py-4">
        <Routes>
          <Route path="/" element={<HomeView />} />
          <Route path="/issue/:issueCode" element={<BundleView />} />
          <Route path="/issue/:issueCode/bundle/:bundleHash" element={<BundleView />} />
          <Route path="/bundle/:bundleHash" element={<RedirectLegacyBundle />} />
          <Route path="/temp-results/:resultId" element={<TempResultView />} />
        </Routes>
      </main>
    </div>
  );
}

function RedirectLegacyBundle() {
  const { bundleHash = '' } = useParams<{ bundleHash: string }>();
  const state = useLocation().state;
  return <BundleView legacyBundleHash={bundleHash} legacyState={state} />;
}

export default App;
