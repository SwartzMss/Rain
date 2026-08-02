import assert from 'node:assert/strict';
import { createServer } from 'vite';

const server = await createServer({ appType: 'custom', logLevel: 'silent', server: { middlewareMode: true } });
try {
  const { isAdmin } = await server.ssrLoadModule('/src/auth/permissions.ts');
  assert.equal(isAdmin(null), false);
  assert.equal(isAdmin({ id: 'u', username: 'user', role: 'USER' }), false);
  assert.equal(isAdmin({ id: 'a', username: 'admin', role: 'ADMIN' }), true);
  const { advanceCursor, currentCursor, retreatCursor, runAdminAction } =
    await server.ssrLoadModule('/src/features/admin/adminFlow.ts');

  const calls = [];
  await runAdminAction({
    action: async () => calls.push('action'),
    reload: async () => calls.push('reload'),
    refreshAuth: async () => calls.push('refresh')
  });
  assert.deepEqual(calls, ['action', 'reload', 'refresh']);

  let history = [undefined];
  history = advanceCursor(history, 'page-2');
  history = advanceCursor(history, 'page-3');
  assert.equal(currentCursor(history), 'page-3');
  history = retreatCursor(history);
  assert.equal(currentCursor(history), 'page-2');
  history = retreatCursor(history);
  assert.equal(currentCursor(history), undefined);
} finally {
  await server.close();
}
const { readFile } = await import('node:fs/promises');
const adminPage = await readFile(new URL('../src/features/admin/AdminPage.tsx', import.meta.url), 'utf8');
assert.doesNotMatch(adminPage, /全部角色|提升|降级|changeUserRole/);
const apiClient = await readFile(new URL('../src/api/client.ts', import.meta.url), 'utf8');
assert.doesNotMatch(apiClient, /changeUserRole|\/role/);
const homeView = await readFile(new URL('../src/features/files/HomeView.tsx', import.meta.url), 'utf8');
assert.match(homeView, /isUser\(auth\.state\.user\)/);
const app = await readFile(new URL('../src/App.tsx', import.meta.url), 'utf8');
assert.match(app, /to="\/admin\/users"/);
assert.match(app, /<Navigate to="\/admin\/users" replace \/>/);
assert.doesNotMatch(app, /isAdmin\(auth\.state\.user\) \? <Link[\s\S]*>管理<\/Link>/);
assert.match(app, /!isAdmin\(auth\.state\.user\)[\s\S]*to="\/account"/);
const accountPage = await readFile(new URL('../src/features/auth/AccountPage.tsx', import.meta.url), 'utf8');
assert.match(accountPage, /isAdmin\(auth\.state\.user\)[\s\S]*Navigate to="\/admin\/users"/);
console.log('admin permission tests passed');
