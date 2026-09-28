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
  assert.match(
    filesView,
    /useEffect\(\(\) => \{\s*if \(fileSearchExecuted && isFileSearchConditionEmpty\(fileSearchTokens, fileSearchDraft\)\) \{\s*clearFileSearch\(\);\s*\}\s*\}, \[clearFileSearch, fileSearchDraft, fileSearchExecuted, fileSearchTokens\]\);/
  );
  assert.equal(filesView.includes('setSearchError(issueSearchExecution.snapshot.errorMessage)'), false);
  assert.equal(filesView.includes('setSearchError(viewerSearchExecution.snapshot.errorMessage)'), false);
  assert.ok(filesView.includes("status === 'RUNNING' || status === 'CANCELLING' || status === 'CANCELLED' || status === 'SUCCEEDED'"));
  assert.ok(filesView.includes("setSearchDraft('');\n    setSearchError(null);\n    setSearchLoading(true);"));
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
