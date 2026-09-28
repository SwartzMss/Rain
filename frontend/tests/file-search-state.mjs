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
  assert.doesNotMatch(filesView, /setSearchError\\(issueSearchExecution\\.snapshot\\.errorMessage\\)/);
  assert.doesNotMatch(filesView, /setSearchError\\(viewerSearchExecution\\.snapshot\\.errorMessage\\)/);
  assert.match(filesView, /status === 'RUNNING'.*status === 'CANCELLING'.*status === 'CANCELLED'.*status === 'SUCCEEDED'/s);
  assert.match(filesView, /setSearchDraft\\(''\\);\\s*setSearchError\\(null\\);\\s*setSearchLoading\\(true\\);/);

} finally {
  await server.close();
}

console.log('file search state tests passed');
