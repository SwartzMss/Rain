import assert from 'node:assert/strict';
import { createServer } from 'vite';

const server = await createServer({
  appType: 'custom',
  logLevel: 'silent',
  server: { middlewareMode: true }
});

try {
  const { authStateAfterRefreshFailure, postLoginPath, safeReturnPath, toAuthState } =
    await server.ssrLoadModule(
    '/src/auth/authState.ts'
  );
  const { AuthOperationGeneration } = await server.ssrLoadModule(
    '/src/auth/AuthOperationGeneration.ts'
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
  assert.equal(postLoginPath({ role: 'USER' }, '/admin/users'), '/');
  assert.equal(postLoginPath({ role: 'USER' }, '/issue/ABC'), '/issue/ABC');
  assert.equal(postLoginPath({ role: 'ADMIN' }, '/'), '/admin/users');
  const authenticatedState = {
    status: 'AUTHENTICATED',
    user: { id: '1', username: 'swartz' }
  };
  assert.equal(authStateAfterRefreshFailure(authenticatedState), authenticatedState);
  assert.deepEqual(authStateAfterRefreshFailure({ status: 'GUEST' }), { status: 'GUEST' });
  assert.deepEqual(authStateAfterRefreshFailure({ status: 'LOADING' }), { status: 'GUEST' });

  const generation = new AuthOperationGeneration();
  const staleRefresh = generation.begin();
  let authState = { status: 'LOADING' };
  let resolveRefresh;
  const refreshResponse = new Promise((resolve) => {
    resolveRefresh = resolve;
  });
  const refresh = refreshResponse.then((nextState) => {
    if (generation.isCurrent(staleRefresh)) authState = nextState;
  });

  generation.invalidate();
  authState = {
    status: 'AUTHENTICATED',
    user: { id: '1', username: 'swartz' }
  };
  resolveRefresh({ status: 'GUEST' });
  await refresh;

  assert.equal(
    authState.status,
    'AUTHENTICATED',
    'a stale refresh must not overwrite a successful login'
  );

  const finishMutation = generation.beginMutation();
  const refreshDuringMutation = generation.begin();
  assert.equal(
    generation.isCurrent(refreshDuringMutation),
    false,
    'refreshes started during an authentication mutation must not commit'
  );
  finishMutation();
  const refreshAfterMutation = generation.begin();
  assert.equal(generation.isCurrent(refreshAfterMutation), true);

  const { shouldRevalidateAuthentication } = await server.ssrLoadModule(
    '/src/api/client.ts'
  );
  assert.equal(
    shouldRevalidateAuthentication(
      401,
      JSON.stringify({ code: 'AUTHENTICATION_REQUIRED' })
    ),
    true
  );
  assert.equal(
    shouldRevalidateAuthentication(403, JSON.stringify({ code: 'ACCOUNT_DISABLED' })),
    true
  );
  assert.equal(
    shouldRevalidateAuthentication(403, JSON.stringify({ code: 'ADMIN_REQUIRED' })),
    true
  );
  assert.equal(
    shouldRevalidateAuthentication(403, JSON.stringify({ code: 'FORBIDDEN' })),
    false
  );

} finally {
  await server.close();
}

console.log('auth state tests passed');
