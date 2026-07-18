import { Link, Route, Routes } from 'react-router-dom';
import { BundleView } from './features/files/FilesView';
import { HomeView } from './features/files/HomeView';
import { TempResultView } from './features/files/TempResultView';
import { APP_VERSION } from './version';
import './App.css';

function App() {
  return (
    <div className="min-h-screen text-slate-900">
      <header className="sticky top-0 z-40 border-b border-white/10 bg-slate-950/95 shadow-lg shadow-slate-950/15 backdrop-blur-xl">
        <div className="mx-auto flex h-16 w-full max-w-none items-center justify-between gap-3 px-6">
          <Link to="/" className="text-white no-underline">
            <div className="flex flex-wrap items-center gap-2.5">
              <span className="flex h-9 w-9 items-center justify-center rounded-xl border border-cyan-300/30 bg-gradient-to-br from-cyan-300 to-teal-400 text-lg text-slate-950 shadow-lg shadow-cyan-950/30">☁</span>
              <h1 className="text-2xl font-semibold tracking-tight text-white">Rain</h1>
              <span className="rounded-full border border-cyan-300/25 bg-cyan-300/10 px-2.5 py-0.5 text-[11px] font-semibold tracking-wide text-cyan-200">
                {APP_VERSION}
              </span>
            </div>
          </Link>
          <div className="flex items-center gap-2 rounded-full border border-white/10 bg-white/5 px-3 py-1.5 text-sm font-medium text-slate-200">
            <span className="h-2.5 w-2.5 rounded-full bg-emerald-400 shadow-[0_0_10px_rgba(52,211,153,0.8)]" />
            <span>服务正常</span>
          </div>
        </div>
      </header>

      <main className="mx-auto w-full max-w-none px-5 py-5">
        <Routes>
          <Route path="/" element={<HomeView />} />
          <Route path="/issue/:issueCode" element={<BundleView />} />
          <Route path="/issue/:issueCode/bundle/:bundleHash" element={<BundleView />} />
          <Route path="/temp-results/:resultId" element={<TempResultView />} />
        </Routes>
      </main>
    </div>
  );
}

export default App;
