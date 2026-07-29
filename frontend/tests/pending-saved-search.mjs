import assert from 'node:assert/strict';
import { createServer } from 'vite';

const server = await createServer({
  appType: 'custom',
  logLevel: 'silent',
  server: { middlewareMode: true }
});

try {
  const { PENDING_SAVED_SEARCH_KEY, takePendingSavedSearch } =
    await server.ssrLoadModule('/src/features/files/pendingSavedSearch.ts');
  const values = new Map();
  const storage = {
    getItem(key) { return values.get(key) ?? null; },
    removeItem(key) { values.delete(key); }
  };
  const pending = {
    name: '',
    search_type: 'DETAIL',
    query_text: '"ERROR"',
    scope_type: 'ISSUE',
    scope_key: 'CN013',
    options: { version: 1, tokens: [{ kind: 'term', value: 'ERROR' }] }
  };
  values.set(PENDING_SAVED_SEARCH_KEY, JSON.stringify(pending));

  assert.equal(takePendingSavedSearch(storage, false, 'CN013'), null);
  assert.ok(values.has(PENDING_SAVED_SEARCH_KEY), 'guest state must retain the pending condition');

  const restored = takePendingSavedSearch(storage, true, 'CN013');
  assert.deepEqual(restored, pending);
  assert.equal(values.has(PENDING_SAVED_SEARCH_KEY), false);
  assert.equal(restored.options.tokens[0].value, 'ERROR');
} finally {
  await server.close();
}

console.log('pending saved search flow tests passed');
