import { Link, Route, Routes, useLocation, useParams } from 'react-router-dom';
import { BundleView } from './features/files/FilesView';
import { HomeView } from './features/files/HomeView';
import './App.css';

function App() {
  const location = useLocation();
  const onBundlePage = location.pathname.startsWith('/bundle') || location.pathname.startsWith('/issue/');

  return (
    <div className="min-h-screen bg-slate-950 text-slate-100">
      <header className="border-b border-slate-800 bg-slate-900/60 backdrop-blur">
        <div className="mx-auto flex w-full max-w-none flex-col gap-4 px-6 py-6 lg:flex-row lg:items-center lg:justify-between">
          <Link to="/" className="text-white no-underline">
            <h1 className="text-4xl font-semibold text-white">Rain</h1>
          </Link>
          <div className="flex flex-wrap items-center gap-2" />
        </div>
      </header>

      <main className="mx-auto w-full max-w-none px-6 py-6">
        <Routes>
          <Route path="/" element={<HomeView />} />
          <Route path="/issue/:issueCode" element={<BundleView />} />
          <Route path="/issue/:issueCode/bundle/:bundleHash" element={<BundleView />} />
          <Route path="/bundle/:bundleHash" element={<RedirectLegacyBundle />} />
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
