import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createServer } from 'vite';

const server = await createServer({
  appType: 'custom',
  logLevel: 'silent',
  server: { middlewareMode: true }
});

try {
  const { shouldResetUploadAfterBundleDeletion } = await server.ssrLoadModule(
    '/src/features/files/uploadDeletion.ts'
  );

  assert.equal(shouldResetUploadAfterBundleDeletion('failed-bundle', 'failed-bundle'), true);
  assert.equal(shouldResetUploadAfterBundleDeletion('other-bundle', 'failed-bundle'), false);
  assert.equal(shouldResetUploadAfterBundleDeletion('failed-bundle', undefined), false);

  const homeView = await readFile(
    new URL('../src/features/files/HomeView.tsx', import.meta.url),
    'utf8'
  );
  assert.match(
    homeView,
    /await rainApi\.deleteBundle\(issues\.currentIssueCode, row\.bundleHash\);[\s\S]*?shouldResetUploadAfterBundleDeletion\([\s\S]*?row\.bundleHash,[\s\S]*?upload\.uploadTask\?\.bundle_hash[\s\S]*?\)[\s\S]*?upload\.resetSelection\(\);[\s\S]*?await bundles\.loadBundles\(issues\.currentIssueCode\);/
  );
} finally {
  await server.close();
}

console.log('upload deletion tests passed');
