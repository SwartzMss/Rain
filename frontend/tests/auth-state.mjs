import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createServer } from 'vite';

const server = await createServer({
  appType: 'custom',
  logLevel: 'silent',
  server: { middlewareMode: true }
});

try {
  const { safeReturnPath, toAuthState } = await server.ssrLoadModule(
    '/src/auth/authState.ts'
  );

  assert.equal(
    toAuthState({ authenticated: false, user: null }).status,
    'GUEST'
  );
  assert.equal(
    toAuthState({
      authenticated: true,
      user: { id: '1', username: 'swartz' }
    }).status,
    'AUTHENTICATED'
  );
  assert.equal(safeReturnPath('/issue/CN013'), '/issue/CN013');
  assert.equal(safeReturnPath('https://evil.example'), '/');
  assert.equal(safeReturnPath('//evil.example'), '/');

  const apiClient = await readFile(
    new URL('../src/api/client.ts', import.meta.url),
    'utf8'
  );
  assert.match(apiClient, /credentials:\s*'include'/);
  assert.match(apiClient, /xhr\.withCredentials\s*=\s*true/);
  assert.match(apiClient, /xhr\.status === 401/);
  assert.match(apiClient, /payload\.message/);
  assert.match(apiClient, /rain:authentication-required/);

  const app = await readFile(
    new URL('../src/App.tsx', import.meta.url),
    'utf8'
  );
  assert.match(app, /path="\/login"/);
  assert.match(app, /path="\/register"/);
  assert.match(app, /path="\/account"/);
  assert.match(app, /只读模式/);
  assert.match(app, /退出登录/);

  const main = await readFile(
    new URL('../src/main.tsx', import.meta.url),
    'utf8'
  );
  assert.match(main, /<AuthProvider>/);

  const homeView = await readFile(
    new URL('../src/features/files/HomeView.tsx', import.meta.url),
    'utf8'
  );
  assert.match(homeView, /const canWrite = auth\.state\.status === 'AUTHENTICATED'/);
  assert.match(homeView, /canWrite={canWrite}/);

  const issueSelector = await readFile(
    new URL('../src/features/files/components/IssueSelector.tsx', import.meta.url),
    'utf8'
  );
  assert.match(issueSelector, /canWrite \? \(/);
  assert.match(issueSelector, /登录后可新建/);

  const uploadPanel = await readFile(
    new URL('../src/features/files/components/UploadPanel.tsx', import.meta.url),
    'utf8'
  );
  assert.match(uploadPanel, /只读模式：登录后可上传/);

  const uploadFileTable = await readFile(
    new URL('../src/features/files/components/UploadFileTable.tsx', import.meta.url),
    'utf8'
  );
  assert.match(uploadFileTable, /canWrite && row\.stage !== 'UPLOADING'/);
  assert.match(uploadFileTable, /登录后可删除/);

  const filesView = await readFile(
    new URL('../src/features/files/FilesView.tsx', import.meta.url),
    'utf8'
  );
  assert.match(filesView, /takePendingSavedSearch/);
  assert.match(filesView, /保存搜索条件/);
  assert.match(filesView, /我的搜索条件/);
  assert.match(filesView, /markSavedSearchUsed/);
  assert.match(filesView, /编辑搜索条件/);
  assert.match(filesView, /搜索表达式/);
  assert.match(filesView, /is_pinned/);
  assert.match(filesView, /sort_order/);
  assert.match(filesView, /detailRawExpression/);
  assert.match(filesView, /日志内容原始搜索表达式/);
} finally {
  await server.close();
}

console.log('auth state tests passed');
