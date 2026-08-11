import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const homeView = await readFile(
  new URL('../src/features/files/HomeView.tsx', import.meta.url),
  'utf8'
);
assert.match(
  homeView,
  /await rainApi\.deleteBundle\(issues\.currentIssueCode, row\.bundleHash\);[\s\S]*?await bundles\.loadBundles\(issues\.currentIssueCode\);/
);
assert.doesNotMatch(homeView, /shouldResetUploadAfterBundleDeletion|upload\.uploadTask/);

console.log('upload deletion tests passed');
