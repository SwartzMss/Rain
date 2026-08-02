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
console.log('admin permission tests passed');
