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
    options: { version: 1, tokens: [{ kind: 'term', value: 'ERROR' }] }
  };
  values.set(PENDING_SAVED_SEARCH_KEY, JSON.stringify(pending));

  assert.equal(takePendingSavedSearch(storage, false, 'CN013'), null);
  assert.ok(values.has(PENDING_SAVED_SEARCH_KEY), 'guest state must retain the pending condition');

  const restored = takePendingSavedSearch(storage, true);
  assert.deepEqual(restored, pending);
  assert.equal(values.has(PENDING_SAVED_SEARCH_KEY), false);
  assert.equal(restored.options.tokens[0].value, 'ERROR');

  for (const corrupted of [
    { ...pending, query_text: 42 },
    { ...pending, options: null },
    { ...pending, options: {} },
    { ...pending, options: { version: 1, tokens: [null] } },
    { ...pending, options: { version: 1, tokens: [{ kind: 'operator', value: 'AND' }] } },
    { ...pending, options: [] }
  ]) {
    values.set(PENDING_SAVED_SEARCH_KEY, JSON.stringify(corrupted));
    assert.equal(takePendingSavedSearch(storage, true), null);
    assert.equal(values.has(PENDING_SAVED_SEARCH_KEY), false);
  }

  const queryOnly = { ...pending, options: { version: 1 } };
  values.set(PENDING_SAVED_SEARCH_KEY, JSON.stringify(queryOnly));
  assert.deepEqual(takePendingSavedSearch(storage, true), queryOnly);
} finally {
  await server.close();
}

console.log('pending saved search flow tests passed');
