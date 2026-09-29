import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createServer } from 'vite';

const server = await createServer({
  appType: 'custom',
  logLevel: 'silent',
  server: { middlewareMode: true }
});

try {
  const { isFileSearchConditionEmpty } = await server.ssrLoadModule(
    '/src/features/files/fileSearchState.ts'
  );

  assert.equal(isFileSearchConditionEmpty([], ''), true);
  assert.equal(
    isFileSearchConditionEmpty([{ kind: 'term', value: 'ERROR' }], ''),
    false
  );
  assert.equal(isFileSearchConditionEmpty([], 'ERROR'), false);

  const filesView = await readFile(
    new URL('../src/features/files/FilesView.tsx', import.meta.url),
    'utf8'
  );
  assert.match(filesView, /useIssueSearchController/);
  assert.match(filesView, /useFileSearchController/);
  assert.match(filesView, /useViewerSearchController/);
  assert.match(filesView, /useViewerPaginationController/);
  assert.match(filesView, /useSavedSearchController/);
  assert.equal(filesView.includes('const [searchLoading'), false);
  assert.equal(filesView.includes('const [searchError'), false);
  assert.equal(filesView.includes('searchRequestGenerationRef'), false);
  assert.equal(
    (filesView.match(/<SearchExecutionStatus/g) ?? []).length,
    1,
    'only the global Issue search should render detailed execution status'
  );

  const tempResultView = await readFile(
    new URL('../src/features/files/TempResultView.tsx', import.meta.url),
    'utf8'
  );
  assert.equal(
    (tempResultView.match(/<SearchExecutionStatus/g) ?? []).length,
    0,
    'temporary-result search should not render detailed execution status'
  );

} finally {
  await server.close();
}

console.log('file search state tests passed');
