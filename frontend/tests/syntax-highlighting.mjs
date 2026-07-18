import assert from 'node:assert/strict';
import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';

const server = await createServer({ appType: 'custom', logLevel: 'silent', server: { middlewareMode: true } });

try {
  const { getSyntaxLanguage, SyntaxHighlightedLine } = await server.ssrLoadModule('/src/features/files/syntaxHighlight.tsx');
  assert.equal(getSyntaxLanguage('main.ts'), 'typescript');
  assert.equal(getSyntaxLanguage('settings.JSON'), 'json');
  assert.equal(getSyntaxLanguage('application.log'), null);
  assert.equal(getSyntaxLanguage('README.md'), 'markdown');

  const markup = renderToStaticMarkup(
    React.createElement(SyntaxHighlightedLine, {
      content: 'const answer: number = 42;',
      language: 'typescript'
    })
  );
  assert.match(markup, /hljs-keyword/);
  assert.match(markup, /hljs-number/);
  assert.doesNotMatch(markup, /<script/);
} finally {
  await server.close();
}

console.log('syntax highlighting tests passed');
