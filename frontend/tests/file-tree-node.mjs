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
  const { FileTreeNode } = await server.ssrLoadModule(
    '/src/features/files/components/FileTreeNode.tsx'
  );

  const archiveId = 'bundle:archive';
  const fileId = 'bundle:file';
  const treeNodes = {
    [archiveId]: {
      id: archiveId,
      rawId: 'archive',
      bundleId: 'bundle',
      parentId: null,
      name: 'very-long-diagnostic-bundle.zip',
      path: '/very-long-diagnostic-bundle.zip',
      is_dir: false,
      preview_kind: 'archive',
      size_bytes: 8192,
      mime_type: 'application/zip',
      childrenIds: [fileId],
      hasLoadedChildren: true
    },
    [fileId]: {
      id: fileId,
      rawId: 'file',
      bundleId: 'bundle',
      parentId: archiveId,
      name: 'application-production.log',
      path: '/application-production.log',
      is_dir: false,
      preview_kind: 'text',
      size_bytes: 4096,
      mime_type: 'text/plain',
      childrenIds: [],
      hasLoadedChildren: true
    }
  };

  const markup = renderToStaticMarkup(
    React.createElement(FileTreeNode, {
      nodeId: archiveId,
      treeNodes,
      expandedNodes: new Set([archiveId]),
      selectedNodeId: null,
      onNodeClick: () => undefined
    })
  );

  assert.match(markup, /very-long-diagnostic-bundle\.zip/);
  assert.match(markup, /application-production\.log/);
  assert.match(markup, /title="very-long-diagnostic-bundle\.zip"/);
  assert.match(markup, /aria-label="very-long-diagnostic-bundle\.zip"/);
  assert.match(markup, /text-violet-500/);
  assert.match(markup, /text-slate-500/);
  assert.match(markup, /rotate-90/);
  assert.doesNotMatch(markup, />ZIP<|>TXT<|□|▣/);
  assert.doesNotMatch(markup, /1 子节点/);
  assert.doesNotMatch(markup, /text\/plain/);
  assert.doesNotMatch(markup, /4\.0 KB/);
  assert.doesNotMatch(markup, /展开|收起|暂无子节点/);
} finally {
  await server.close();
}

console.log('file tree node tests passed');
