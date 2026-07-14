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
  const { BinaryFileInfo } = await server.ssrLoadModule(
    '/src/features/files/BinaryFileInfo.tsx'
  );
  const { canPreviewText, isArchiveNode, isBinaryNode } = await server.ssrLoadModule(
    '/src/features/files/filePresentation.ts'
  );

  const markup = renderToStaticMarkup(
    React.createElement(BinaryFileInfo, {
      node: {
        name: 'tool.exe',
        mime_type: 'application/x-msdownload',
        size_bytes: 4096
      }
    })
  );

  assert.match(markup, /tool\.exe/);
  assert.match(markup, /application\/x-msdownload/);
  assert.match(markup, /4\.0 KB/);
  assert.doesNotMatch(markup, /下载/);
  assert.doesNotMatch(markup, /<pre/);
  assert.doesNotMatch(markup, /搜索/);

  const docx = { name: 'report.docx', is_dir: false, preview_kind: 'binary' };
  const zip = { name: 'logs.zip', is_dir: false, preview_kind: 'archive' };
  assert.equal(isBinaryNode(docx), true);
  assert.equal(canPreviewText(docx), false);
  assert.equal(isArchiveNode(docx), false);
  assert.equal(isArchiveNode(zip), true);
} finally {
  await server.close();
}

console.log('binary file information view tests passed');
