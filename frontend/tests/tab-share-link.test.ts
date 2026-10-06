import { describe, expect, it } from 'vitest';
import { buildTabShareUrl, parseSharedTabSearch } from '../src/features/files/tabShareLink';
import type { FileViewerTab, SearchViewerTab } from '../src/features/files/viewerTabs';

const searchTab = (queryPlan: SearchViewerTab['queryPlan']): SearchViewerTab => ({
  id: 'search:result',
  kind: 'search',
  resultId: 'result',
  title: '错误',
  pinned: true,
  scrollTop: 0,
  expression: '"错误"',
  hits: [],
  total: 0,
  from: 0,
  pageSize: 50,
  pageHistory: [],
  source: { kind: 'issue', issueCode: 'CN013' },
  queryPlan
});

const fileTab: FileViewerTab = {
  id: 'file:bundle-1:42',
  kind: 'file',
  title: 'application.log',
  pinned: true,
  scrollTop: 300,
  nodeId: 'bundle-1:42',
  lineStart: 0,
  pageSize: 100,
  pageHistory: [],
  targetLine: null
};

describe('tab share links', () => {
  it('shares a pinned file tab without including transient tab state', () => {
    const url = buildTabShareUrl(fileTab, 'CN013', 'http://rain.local:8078');
    expect(url).toBe('http://rain.local:8078/issue/CN013/bundle/bundle-1?share=1&v=1&view=file&bundle=bundle-1&file=42');
    expect(parseSharedTabSearch(new URL(url ?? '').search, 'CN013')).toEqual({
      kind: 'file',
      issueCode: 'CN013',
      bundleHash: 'bundle-1',
      fileId: '42'
    });
    expect(url).not.toContain('pinned');
    expect(url).not.toContain('scrollTop');
  });

  it('round trips an issue search and preserves ordered nested filters', () => {
    const tab = searchTab({
      root: { kind: 'issue', issueCode: 'CN013' },
      expressions: ['"错误"', '"超时" AND NOT "忽略"']
    });
    const url = buildTabShareUrl(tab, 'CN013', 'http://rain.local:8078');
    expect(url).toContain('/issue/CN013?');
    expect(parseSharedTabSearch(new URL(url ?? '').search, 'CN013')).toEqual({
      kind: 'search',
      issueCode: 'CN013',
      plan: {
        root: { kind: 'issue', issueCode: 'CN013' },
        expressions: ['"错误"', '"超时" AND NOT "忽略"']
      }
    });
  });

  it('round trips file search scope and does not encode pin state', () => {
    const tab = searchTab({
      root: { kind: 'file', bundleHash: 'bundle/hash', fileId: '42' },
      expressions: ['"timeout"']
    });
    const url = buildTabShareUrl(tab, 'CN013', 'http://rain.local:8078');
    expect(url).toContain('/issue/CN013/bundle/bundle%2Fhash?');
    expect(parseSharedTabSearch(new URL(url ?? '').search, 'CN013')?.kind).toBe('search');
    expect(url).not.toContain('pinned');
  });
});
