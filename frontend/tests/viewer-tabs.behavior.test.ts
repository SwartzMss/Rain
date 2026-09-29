import { describe, expect, it } from 'vitest';
import { openOrActivateTab, type FileViewerTab } from '../src/features/files/viewerTabs';

const fileTab = (overrides: Partial<FileViewerTab> = {}): FileViewerTab => ({
  id: 'file:bundle:1',
  kind: 'file',
  title: 'application.log',
  pinned: false,
  scrollTop: 240,
  nodeId: 'bundle:1',
  lineStart: 5000,
  pageSize: 5000,
  pageHistory: [0],
  targetLine: null,
  ...overrides
});

describe('viewer file tabs', () => {
  it('keeps an existing file tab browsing state when the tree opens it again', () => {
    const existing = fileTab();
    const incoming = fileTab({
      title: 'application.log (刷新后)',
      scrollTop: 0,
      lineStart: 0,
      pageHistory: [],
      targetLine: 12
    });

    expect(openOrActivateTab([existing], incoming)).toEqual([
      { ...existing, title: 'application.log (刷新后)' }
    ]);
  });
});
