import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
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

  const apiClient = await readFile(
    new URL('../src/api/client.ts', import.meta.url),
    'utf8'
  );
  assert.match(apiClient, /credentials:\s*'include'/);
  assert.match(apiClient, /xhr\.withCredentials\s*=\s*true/);
  assert.match(apiClient, /payload\.message/);
  assert.match(apiClient, /rain:authentication-required/);
  assert.equal(
    apiClient.match(/shouldRevalidateAuthentication\(/g)?.length,
    3,
    'fetch and upload XHR must share the authentication revalidation predicate'
  );
  assert.doesNotMatch(apiClient, /VITE_API_BASE_URL/);
  assert.match(apiClient, /const API_BASE_URL = ''/);
  assert.doesNotMatch(apiClient, /logoutAll|logout-all/);

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

  const authContext = await readFile(
    new URL('../src/auth/AuthContext.tsx', import.meta.url),
    'utf8'
  );
  assert.match(authContext, /useRef\(new AuthOperationGeneration\(\)\)/);
  assert.match(authContext, /isCurrent\(refreshGeneration\)/);
  assert.match(authContext, /setState\(authStateAfterRefreshFailure\)/);
  assert.match(authContext, /const revalidateAuthentication = \(\) => \{\s*void refresh\(\);/);
  assert.match(
    authContext,
    /const changePassword = useCallback\([\s\S]*beginMutation\(\)[\s\S]*await rainApi\.changePassword\(payload\);[\s\S]*finishMutation\(\);[\s\S]*await refresh\(\);/
  );
  assert.doesNotMatch(authContext, /logoutAll/);

  const accountPage = await readFile(
    new URL('../src/features/auth/AccountPage.tsx', import.meta.url),
    'utf8'
  );
  assert.match(accountPage, /await auth\.changePassword\(/);
  assert.doesNotMatch(accountPage, /rainApi\.changePassword\(/);
  assert.doesNotMatch(accountPage, /退出所有设备|logoutAll/);

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
  assert.doesNotMatch(issueSelector, /登录后可新建/);

  const uploadPanel = await readFile(
    new URL('../src/features/files/components/UploadPanel.tsx', import.meta.url),
    'utf8'
  );
  assert.doesNotMatch(uploadPanel, /登录后可上传|需要登录|游客可以/);

  const uploadFileTable = await readFile(
    new URL('../src/features/files/components/UploadFileTable.tsx', import.meta.url),
    'utf8'
  );
  assert.match(uploadFileTable, /canWrite && row\.stage !== 'UPLOADING'/);
  assert.doesNotMatch(uploadFileTable, /登录后可删除/);

  const filesView = await readFile(
    new URL('../src/features/files/FilesView.tsx', import.meta.url),
    'utf8'
  );
  assert.match(filesView, /takePendingSavedSearch/);
  assert.match(filesView, /保存搜索条件/);
  assert.match(filesView, /我的搜索条件/);
  assert.match(
    filesView,
    /auth\.state\.status === 'AUTHENTICATED' && searchMode === 'detailed'[\s\S]*?保存条件/
  );
  assert.doesNotMatch(filesView, />搜索类型</);
  assert.doesNotMatch(filesView, /<option value="FILENAME">/);
  assert.match(filesView, /markSavedSearchUsed/);
  assert.match(filesView, /编辑搜索条件/);
  assert.match(filesView, /搜索表达式/);
  assert.match(filesView, /is_pinned/);
  assert.doesNotMatch(filesView, /savedSearchScope|scope_type|scope_key|sort_order/);
  assert.doesNotMatch(filesView, /fetchSavedSearches\(issueCode/);
  assert.match(filesView, /detailRawExpression/);
  assert.match(filesView, /日志内容原始搜索表达式/);
  assert.match(filesView, /const hasFileContext = Boolean\(issueCode \|\| bundleId\)/);
  assert.doesNotMatch(filesView, /选择左侧 Issue \/ Bundle 后自动加载文件树。/);
} finally {
  await server.close();
}

console.log('auth state tests passed');
