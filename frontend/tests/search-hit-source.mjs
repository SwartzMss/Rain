import assert from 'node:assert/strict';

import { createServer } from 'vite';

const server = await createServer({
  appType: 'custom',
  logLevel: 'silent',
  server: { middlewareMode: true }
});

try {
  const { getSearchHitSource, placeContextMenu } = await server.ssrLoadModule(
    '/src/features/files/searchHitSource.ts'
  );

  assert.deepEqual(
    getSearchHitSource({
      bundle_hash: 'bundle-a',
      file_id: 42,
      path: '/bundle-a/app.log',
      snippet: 'ERROR',
      line_number: 17
    }),
    {
      bundleHash: 'bundle-a',
      fileId: '42',
      nodeId: 'bundle-a:42',
      path: '/bundle-a/app.log',
      line: 17
    }
  );
  assert.equal(
    getSearchHitSource({
      file_id: 42,
      path: 'app.log',
      snippet: 'ERROR',
      line_number: 17
    }),
    null
  );
  assert.deepEqual(
    placeContextMenu(
      { x: 990, y: 790 },
      { width: 180, height: 88 },
      { width: 1000, height: 800 }
    ),
    { left: 812, top: 704 }
  );
} finally {
  await server.close();
}

console.log('search hit source tests passed');
