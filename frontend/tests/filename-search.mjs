import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

import { createServer } from 'vite';

const server = await createServer({
  appType: 'custom',
  logLevel: 'silent',
  server: { middlewareMode: true }
});

try {
  const { shouldShowFilenameClear } = await server.ssrLoadModule(
    '/src/features/files/filenameSearch.ts'
  );
  const { formatHitPath } = await server.ssrLoadModule(
    '/src/features/files/treeModel.ts'
  );

  const idle = {
    query: '',
    executed: false,
    resultCount: 0,
    loading: false,
    error: null
  };

  assert.equal(shouldShowFilenameClear(idle), false);
  assert.equal(shouldShowFilenameClear({ ...idle, query: 'kernel.log' }), true);
  assert.equal(shouldShowFilenameClear({ ...idle, executed: true }), true);
  assert.equal(shouldShowFilenameClear({ ...idle, resultCount: 2 }), true);
  assert.equal(shouldShowFilenameClear({ ...idle, loading: true }), true);
  assert.equal(shouldShowFilenameClear({ ...idle, error: 'network failed' }), true);
  assert.equal(
    formatHitPath('/bundle/da3f956aee7549908f7dfcba7a754e03.zip_extracted/TickerOneStock/README.md'),
    'TickerOneStock/README.md'
  );
  assert.equal(formatHitPath('/bundle/project/src/main.ts'), 'project/src/main.ts');

  const filesView = await readFile(
    new URL('../src/features/files/FilesView.tsx', import.meta.url),
    'utf8'
  );
  assert.match(filesView, /aria-label="文件名搜索"/);
  assert.match(filesView, /aria-label="清除文件名搜索"/);
  assert.match(filesView, /onSubmit=\{handleSearchSubmit\}/);
  assert.match(filesView, /searchMode === 'log'[\s\S]*?<input[\s\S]*?: \([\s\S]*?<SearchTokenEditor/);
  assert.doesNotMatch(filesView, /allowOperators=\{searchMode === 'detailed'\}/);
} finally {
  await server.close();
}

console.log('filename search tests passed');
