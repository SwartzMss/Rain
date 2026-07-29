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
  assert.match(apiClient, /payload\.message/);
} finally {
  await server.close();
}

console.log('auth state tests passed');
