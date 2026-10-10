import { createRef } from 'react';
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { SearchResultViewer } from '../src/features/files/components/SearchResultViewer';
import type { SearchViewerTab } from '../src/features/files/viewerTabs';

const tab: SearchViewerTab = {
  id: 'tab', resultId: 'old', kind: 'search', title: 'ERROR', pinned: true,
  scrollTop: 0, expression: 'ERROR', hits: [], total: 100, from: 0,
  pageSize: 10, pageHistory: [], source: { kind: 'issue', issueCode: 'TEST' }
};

describe('unavailable search snapshot', () => {
  it('keeps loaded content, disables stale operations and offers explicit replay', () => {
    const replay = vi.fn();
    render(<SearchResultViewer activeViewerTab={tab}
      results={[{ file_id: '1', path: 'app.log', snippet: 'previous content' }]}
      resultFilterTokens={[]} resultFilterDraft="ERROR" onResultFilterTokensChange={vi.fn()}
      onResultFilterDraftChange={vi.fn()} onClearResultFilter={vi.fn()} onSearchWithinResults={vi.fn()}
      canRunResultFilter searchLoading={false} contentRef={createRef()}
      pageSizeOptions={[10]} onLoadPage={vi.fn()} highlightTerm="" renderHighlightedText={(text) => text}
      unavailable onReplay={replay} />);
    expect(screen.getByText('previous content')).toBeInTheDocument();
    expect(screen.getByText(/搜索结果已过期或被删除/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '搜索', exact: true })).toBeDisabled();
    expect(screen.getByRole('button', { name: '下一页' })).toBeDisabled();
    expect(screen.getByRole('combobox')).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: '重新搜索' }));
    expect(replay).toHaveBeenCalledOnce();
  });

  it('shows expiry and recovery even when the original search had no hits', () => {
    render(<SearchResultViewer activeViewerTab={{ ...tab, total: 0 }} results={[]}
      resultFilterTokens={[]} resultFilterDraft="" onResultFilterTokensChange={vi.fn()}
      onResultFilterDraftChange={vi.fn()} onClearResultFilter={vi.fn()} onSearchWithinResults={vi.fn()}
      canRunResultFilter={false} searchLoading={false} contentRef={createRef()}
      pageSizeOptions={[10]} onLoadPage={vi.fn()} highlightTerm="" renderHighlightedText={(text) => text}
      unavailable onReplay={vi.fn()} replayError="来源不可用，请检查原始文件" />);
    expect(screen.getByRole('button', { name: '重新搜索' })).toBeInTheDocument();
    expect(screen.getByText('来源不可用，请检查原始文件')).toBeInTheDocument();
  });
});
