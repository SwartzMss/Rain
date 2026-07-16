import React, { useState } from 'react';
import type { IssueLogSearchHit } from '../../../api/types';
import type { SearchToken } from '../searchTokens';
import type { SearchViewerTab, TempViewerTab } from '../viewerTabs';
import { SearchTokenEditor } from '../SearchTokenEditor';
import { SearchHitContextMenu } from './SearchHitContextMenu';
import { getSearchHitSource } from '../searchHitSource';
import { formatHitPath } from '../treeModel';

type SearchResultViewerProps = {
  activeViewerTab: SearchViewerTab | TempViewerTab;
  results: IssueLogSearchHit[];
  resultFilterTokens: SearchToken[];
  resultFilterDraft: string;
  onResultFilterTokensChange: (tokens: SearchToken[]) => void;
  onResultFilterDraftChange: (draft: string) => void;
  onClearResultFilter: () => void;
  onSearchWithinResults: () => void;
  canRunResultFilter: boolean;
  searchLoading: boolean;
  contentRef: React.RefObject<HTMLDivElement>;
  pageSizeOptions: readonly number[];
  onLoadPage: (tab: SearchViewerTab | TempViewerTab, from: number, pageSize: number) => void;
  highlightTerm: string;
  renderHighlightedText: (text: string, keyword: string) => React.ReactNode;
  onOpenSource?: (hit: IssueLogSearchHit) => void;
  onCopySourcePath?: (hit: IssueLogSearchHit) => void;
};

export function SearchResultViewer({
  activeViewerTab,
  results,
  resultFilterTokens,
  resultFilterDraft,
  onResultFilterTokensChange,
  onResultFilterDraftChange,
  onClearResultFilter,
  onSearchWithinResults,
  canRunResultFilter,
  searchLoading,
  contentRef,
  pageSizeOptions,
  onLoadPage,
  highlightTerm,
  renderHighlightedText,
  onOpenSource,
  onCopySourcePath
}: SearchResultViewerProps) {
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    hit: IssueLogSearchHit;
  } | null>(null);
  if (searchLoading && results.length === 0) {
    return <p className="py-8 text-center text-sm text-slate-500">正在搜索...</p>;
  }

  if (results.length === 0) {
    return <p className="py-8 text-center text-sm text-slate-500">未搜索到相关日志。</p>;
  }

  return (
    <>
      <div className="flex min-h-14 flex-wrap items-center gap-3 border-b border-slate-200 bg-white px-4 py-3 focus-within:border-sky-400">
        <span className="mt-1.5 shrink-0 self-start text-slate-500" aria-hidden="true">⌕</span>
        <SearchTokenEditor
          className="min-w-[220px]"
          tokens={resultFilterTokens}
          draft={resultFilterDraft}
          onTokensChange={onResultFilterTokensChange}
          onDraftChange={onResultFilterDraftChange}
          placeholder="在当前结果中添加关键词或短语..."
          ariaLabel="当前结果筛选条件"
          disabled={searchLoading}
        />
        <span className="shrink-0 text-xs text-slate-500">
          {`${activeViewerTab.total} 条结果`}
        </span>
        {resultFilterTokens.length > 0 || resultFilterDraft ? (
          <button
            type="button"
            className="shrink-0 rounded border border-transparent px-2 py-1 text-xs text-slate-500 transition hover:border-slate-300 hover:text-slate-950"
            onClick={onClearResultFilter}
          >
            清空
          </button>
        ) : null}
        <button
          type="button"
          className="shrink-0 rounded border border-slate-300 bg-white px-3 py-1.5 text-xs font-semibold text-slate-700 transition hover:border-slate-500 disabled:cursor-not-allowed disabled:opacity-50"
          disabled={searchLoading || !canRunResultFilter}
          onClick={onSearchWithinResults}
        >
          搜索
        </button>
      </div>

      <div className="flex min-h-0 flex-1 flex-col gap-2">
        <div ref={contentRef} className="min-h-[70vh] flex-1 overflow-auto bg-white px-3 py-3">
          <div className="space-y-2">
            {results.map((hit, index) => {
              const source = getSearchHitSource(hit);
              const displayPath = formatHitPath(hit.path) || hit.path || '未知文件';
              const displayLine = source?.line === null || source?.line === undefined
                ? '行号未知'
                : `第 ${source.line + 1} 行`;
              return (
                <button
                  key={`${hit.bundle_hash ?? 'bundle'}:${hit.file_id}:${hit.line_number ?? index}:${index}`}
                  type="button"
                  title={source ? '打开原文件' : '来源文件信息不可用'}
                  aria-label={`打开原文件：${displayPath}，${displayLine}`}
                  aria-disabled={!source}
                  className={`group w-full rounded-md border px-3 py-2 text-left transition focus:outline-none focus:ring-2 focus:ring-sky-100 ${
                    source
                      ? 'border-slate-200 bg-white hover:border-sky-200 hover:bg-sky-50/50 focus:border-sky-300'
                      : 'cursor-not-allowed border-slate-200 bg-slate-50 text-slate-500'
                  }`}
                  onClick={() => {
                    if (source) onOpenSource?.(hit);
                  }}
                  onContextMenu={(event) => {
                    event.preventDefault();
                    setContextMenu({ x: event.clientX, y: event.clientY, hit });
                  }}
                >
                  <span className="flex items-center justify-between gap-3 text-[11px] text-slate-500">
                    <span className="truncate">{displayPath}</span>
                    <span className="shrink-0">{displayLine}</span>
                  </span>
                  <span className="mt-1 block truncate font-mono text-xs text-slate-900">
                    {renderHighlightedText(hit.snippet, highlightTerm)}
                  </span>
                </button>
              );
            })}
          </div>
        </div>
        <div className="flex flex-wrap items-center justify-end gap-2 border-t border-slate-200 bg-slate-50 px-4 py-2 text-xs text-slate-500">
          <label className="flex items-center gap-2">
            <span>每页</span>
            <select
              className="rounded border border-slate-300 bg-white px-2 py-1 text-slate-700 outline-none focus:border-cyan-500/60"
              value={activeViewerTab.pageSize}
              disabled={searchLoading}
              onChange={(event) => onLoadPage(activeViewerTab, 0, Number(event.target.value))}
            >
              {pageSizeOptions.map((size) => <option key={size} value={size}>{size} 行</option>)}
            </select>
          </label>
          <span>
            第 {Math.floor(activeViewerTab.from / activeViewerTab.pageSize) + 1} / {Math.max(1, Math.ceil(activeViewerTab.total / activeViewerTab.pageSize))} 页
          </span>
          <button
            type="button"
            className="rounded border border-slate-300 px-3 py-1 hover:border-slate-500 disabled:opacity-50"
            disabled={activeViewerTab.from === 0 || searchLoading}
            onClick={() => onLoadPage(activeViewerTab, Math.max(0, activeViewerTab.from - activeViewerTab.pageSize), activeViewerTab.pageSize)}
          >
            上一页
          </button>
          <button
            type="button"
            className="rounded border border-slate-300 px-3 py-1 hover:border-slate-500 disabled:opacity-50"
            disabled={activeViewerTab.from + results.length >= activeViewerTab.total || searchLoading}
            onClick={() => onLoadPage(activeViewerTab, activeViewerTab.from + activeViewerTab.pageSize, activeViewerTab.pageSize)}
          >
            下一页
          </button>
        </div>
      </div>
      {contextMenu ? (
        <SearchHitContextMenu
          x={contextMenu.x}
          y={contextMenu.y}
          canOpen={getSearchHitSource(contextMenu.hit) !== null}
          canCopy={Boolean(contextMenu.hit.path)}
          onOpen={() => onOpenSource?.(contextMenu.hit)}
          onCopyPath={() => onCopySourcePath?.(contextMenu.hit)}
          onClose={() => setContextMenu(null)}
        />
      ) : null}
    </>
  );
}
