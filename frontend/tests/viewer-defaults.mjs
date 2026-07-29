import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createServer } from 'vite';

const server = await createServer({
  appType: 'custom',
  logLevel: 'silent',
  server: { middlewareMode: true }
});

try {
  const { LINE_PAGE_SIZE_OPTIONS } = await server.ssrLoadModule(
    '/src/features/files/linePageSizes.ts'
  );
  assert.deepEqual([...LINE_PAGE_SIZE_OPTIONS], [5000, 10000]);

  const filesView = await readFile(
    new URL('../src/features/files/FilesView.tsx', import.meta.url),
    'utf8'
  );
  const tempResultView = await readFile(
    new URL('../src/features/files/TempResultView.tsx', import.meta.url),
    'utf8'
  );
  assert.match(filesView, /useState<'log' \| 'detailed'>\('detailed'\)/);
  assert.match(filesView, /import \{ LINE_PAGE_SIZE_OPTIONS \} from '\.\/linePageSizes';/);
  assert.match(tempResultView, /import \{ LINE_PAGE_SIZE_OPTIONS \} from '\.\/linePageSizes';/);
  assert.doesNotMatch(filesView, /const LINE_PAGE_SIZE_OPTIONS =/);
  assert.doesNotMatch(tempResultView, /const PAGE_SIZES =/);
} finally {
  await server.close();
}

console.log('viewer default tests passed');
