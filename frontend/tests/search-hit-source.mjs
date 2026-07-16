import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';

const server = await createServer({
  appType: 'custom',
  logLevel: 'silent',
  server: { middlewareMode: true }
});

try {
  const { getSearchHitSource, placeContextMenu } = await server.ssrLoadModule(
    '/src/features/files/searchHitSource.ts'
  );

  assert.deepEqual(
    getSearchHitSource({
      bundle_hash: 'bundle-a',
      file_id: 42,
      path: '/bundle-a/app.log',
      snippet: 'ERROR',
      line_number: 17
    }),
    {
      bundleHash: 'bundle-a',
      fileId: '42',
      nodeId: 'bundle-a:42',
      path: '/bundle-a/app.log',
      line: 17
    }
  );
  assert.equal(
    getSearchHitSource({
      file_id: 42,
      path: 'app.log',
      snippet: 'ERROR',
      line_number: 17
    }),
    null
  );
  assert.deepEqual(
    placeContextMenu(
      { x: 990, y: 790 },
      { width: 180, height: 88 },
      { width: 1000, height: 800 }
    ),
    { left: 812, top: 704 }
  );

  const { SearchResultViewer } = await server.ssrLoadModule(
    '/src/features/files/components/SearchResultViewer.tsx'
  );
  const { SearchHitContextMenu } = await server.ssrLoadModule(
    '/src/features/files/components/SearchHitContextMenu.tsx'
  );
  const hit = {
    bundle_hash: 'bundle-a',
    file_id: 42,
    path: 'app.log',
    snippet: 'ERROR connection failed',
    line_number: 17
  };
  const viewerProps = {
    activeViewerTab: {
      id: 'search:1', kind: 'search', resultId: 'result-1', expression: 'ERROR',
      title: 'ERROR', pinned: false, scrollTop: 0, hits: [hit], total: 2000,
      from: 0, pageSize: 1000, source: { kind: 'issue', issueCode: 'ISSUE' }
    },
    results: [hit],
    resultFilterTokens: [],
    resultFilterDraft: '',
    onResultFilterTokensChange: () => undefined,
    onResultFilterDraftChange: () => undefined,
    onClearResultFilter: () => undefined,
    onSearchWithinResults: () => undefined,
    canRunResultFilter: false,
    searchLoading: false,
    contentRef: { current: null },
    pageSizeOptions: [1000],
    onLoadPage: () => undefined,
    highlightTerm: 'ERROR',
    renderHighlightedText: (text) => text,
    onOpenSource: () => undefined
  };
  const markup = renderToStaticMarkup(React.createElement(SearchResultViewer, viewerProps));
  assert.match(markup, /data-search-results-log="true"/);
  assert.match(markup, /data-source-line="17"/);
  assert.match(markup, />1<\/span>/);
  assert.doesNotMatch(markup, />18<\/span>/);
  assert.match(markup, /select-text/);
  assert.doesNotMatch(markup, /aria-label="打开原文件/);
  assert.doesNotMatch(markup, /title="打开原文件"/);

  const secondPageMarkup = renderToStaticMarkup(
    React.createElement(SearchResultViewer, {
      ...viewerProps,
      activeViewerTab: { ...viewerProps.activeViewerTab, from: 1000 }
    })
  );
  assert.match(secondPageMarkup, /data-source-line="17"/);
  assert.match(secondPageMarkup, />1001<\/span>/);
  assert.doesNotMatch(secondPageMarkup, />18<\/span>/);

  const missingSourceMarkup = renderToStaticMarkup(
    React.createElement(SearchResultViewer, {
      activeViewerTab: {
        id: 'search:2', kind: 'search', resultId: 'result-2', expression: 'ERROR',
        title: 'ERROR', pinned: false, scrollTop: 0, hits: [], total: 1,
        from: 0, pageSize: 1000, source: { kind: 'issue', issueCode: 'ISSUE' }
      },
      results: [{ file_id: 42, path: 'orphan.log', snippet: 'ERROR' }],
      resultFilterTokens: [], resultFilterDraft: '',
      onResultFilterTokensChange: () => undefined,
      onResultFilterDraftChange: () => undefined,
      onClearResultFilter: () => undefined,
      onSearchWithinResults: () => undefined,
      canRunResultFilter: false, searchLoading: false,
      contentRef: { current: null }, pageSizeOptions: [1000],
      onLoadPage: () => undefined, highlightTerm: '',
      renderHighlightedText: (text) => text,
      onOpenSource: () => undefined
    })
  );
  assert.match(missingSourceMarkup, /orphan\.log|ERROR/);
  assert.doesNotMatch(missingSourceMarkup, /aria-disabled=/);
  assert.doesNotMatch(missingSourceMarkup, /<button[^>]*title="来源文件信息不可用"/);

  const menuMarkup = renderToStaticMarkup(
    React.createElement(SearchHitContextMenu, {
      x: 100,
      y: 100,
      onOpen: () => undefined,
      onClose: () => undefined
    })
  );
  assert.match(menuMarkup, /role="menu"/);
  assert.match(menuMarkup, /在原文件中打开/);
  assert.doesNotMatch(menuMarkup, /复制文件路径/);

  const filesView = await readFile(
    new URL('../src/features/files/FilesView.tsx', import.meta.url),
    'utf8'
  );
  const codeLinesPane = await readFile(
    new URL('../src/features/files/components/CodeLinesPane.tsx', import.meta.url),
    'utf8'
  );
  assert.match(filesView, /const openSearchHitSource = async/);
  assert.match(filesView, /getSearchHitSource\(hit\)/);
  assert.match(filesView, /handleNodeClick\(source\.nodeId, source\.line, \{ preserveSearch: true \}\)/);
  assert.doesNotMatch(filesView, /navigator\.clipboard\.writeText\(hit\.path\)/);
  assert.match(
    filesView,
    /const hits = response\.lines\.map\(\(line\) => \(\{\s+bundle_hash: selectedBundleId,/
  );
  assert.match(filesView, /onOpenSource=\{openSearchHitSource\}/);
  assert.doesNotMatch(filesView, /onCopySourcePath=/);
  assert.match(filesView, /scrollIntoView\(\{ block: 'center' \}\)/);
  assert.match(codeLinesPane, /targetLine/);
  assert.match(codeLinesPane, /data-source-line/);
  assert.match(codeLinesPane, /bg-amber-100/);
} finally {
  await server.close();
}

console.log('search hit source tests passed');
