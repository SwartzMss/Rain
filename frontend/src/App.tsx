import { Link, Route, Routes, useLocation, useParams } from 'react-router-dom';
import { BundleView } from './features/files/FilesView';
import { HomeView } from './features/files/HomeView';
import { TempResultView } from './features/files/TempResultView';
import { APP_VERSION } from './version';
import './App.css';

function App() {
  return (
    <div className="min-h-screen bg-slate-950 text-slate-100">
      <header className="border-b border-slate-800 bg-slate-950/90 backdrop-blur">
        <div className="mx-auto flex w-full max-w-none items-center justify-between gap-3 px-5 py-3">
          <Link to="/" className="text-white no-underline">
            <div className="flex flex-wrap items-center gap-2.5">
              <span className="text-xl text-sky-400">☁</span>
              <h1 className="text-xl font-semibold text-white">Rain</h1>
              <span className="rounded border border-slate-700 px-2 py-0.5 text-xs font-semibold text-slate-400">
                {APP_VERSION}
              </span>
            </div>
          </Link>
          <div className="flex items-center gap-2 text-sm text-slate-200">
            <span className="h-2.5 w-2.5 rounded-full bg-emerald-400" />
            <span>服务正常</span>
          </div>
        </div>
      </header>

      <main className="mx-auto w-full max-w-none px-5 py-5">
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
