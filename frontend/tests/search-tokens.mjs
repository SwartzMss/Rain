import assert from 'node:assert/strict';

import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';

const server = await createServer({
  appType: 'custom',
  logLevel: 'silent',
  server: { middlewareMode: true }
});

try {
  const tokensModule = await server.ssrLoadModule('/src/features/files/searchTokens.ts');
  const { SearchTokenEditor } = await server.ssrLoadModule(
    '/src/features/files/SearchTokenEditor.tsx'
  );
  const {
    appendSearchOperator,
    appendSearchTerm,
    combineSearchExpressions,
    finalizeSearchTokens,
    removeSearchToken,
    serializeSearchTokens,
    validateSearchTokens
  } = tokensModule;

  const ext4 = finalizeSearchTokens(
    [],
    'EXT4-fs (vds): mounted filesystem with ordered data mode'
  );
  assert.equal(
    serializeSearchTokens(ext4),
    '"EXT4-fs (vds): mounted filesystem with ordered data mode"'
  );

  const reservedWords = finalizeSearchTokens([], 'AND OR NOT (worker): ready');
  assert.equal(serializeSearchTokens(reservedWords), '"AND OR NOT (worker): ready"');

  const escaped = finalizeSearchTokens([], 'disk "primary" at C:\\logs');
  assert.equal(serializeSearchTokens(escaped), '"disk \\"primary\\" at C:\\\\logs"');

  let booleanTokens = appendSearchTerm([], 'ERROR');
  booleanTokens = appendSearchOperator(booleanTokens, 'OR');
  booleanTokens = appendSearchOperator(booleanTokens, 'NOT');
  booleanTokens = appendSearchTerm(booleanTokens, 'request timeout');
  assert.equal(serializeSearchTokens(booleanTokens), '"ERROR" OR NOT "request timeout"');

  assert.deepEqual(validateSearchTokens([{ kind: 'operator', value: 'AND' }]), {
    valid: false,
    message: 'AND 前缺少关键词'
  });
  assert.equal(validateSearchTokens([...booleanTokens, { kind: 'operator', value: 'AND' }]).valid, false);

  const repaired = removeSearchToken(booleanTokens, 3);
  assert.equal(serializeSearchTokens(repaired), '"ERROR"');

  const level1 = serializeSearchTokens(finalizeSearchTokens([], 'EXT4-fs (vds): mounted'));
  const level2 = combineSearchExpressions(
    level1,
    serializeSearchTokens(finalizeSearchTokens([], 'ordered data mode'))
  );
  const level3 = combineSearchExpressions(
    level2,
    serializeSearchTokens(finalizeSearchTokens([], 'AND remains literal'))
  );
  assert.equal(
    level3,
    '(("EXT4-fs (vds): mounted") AND ("ordered data mode")) AND ("AND remains literal")'
  );

  const markup = renderToStaticMarkup(
    React.createElement(SearchTokenEditor, {
      tokens: booleanTokens,
      draft: '',
      onTokensChange: () => undefined,
      onDraftChange: () => undefined,
      placeholder: '输入关键词',
      ariaLabel: '测试搜索条件'
    })
  );
  assert.match(markup, /aria-label="测试搜索条件"/);
  assert.match(markup, /AND|OR/);
  assert.match(markup, /NOT 运算符/);
  assert.match(markup, /删除关键词 request timeout/);
} finally {
  await server.close();
}

console.log('search token editor tests passed');
