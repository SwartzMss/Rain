import React from 'react';
import { SearchTokenEditor } from './SearchTokenEditor';
import type { SearchToken } from './searchTokens';

export type SearchExpressionMode = 'simple' | 'advanced';

type SearchExpressionEditorProps = {
  mode: SearchExpressionMode;
  tokens: SearchToken[];
  draft: string;
  rawExpression: string;
  onModeChange: (mode: SearchExpressionMode) => void;
  onTokensChange: (tokens: SearchToken[]) => void;
  onDraftChange: (draft: string) => void;
  onRawExpressionChange: (expression: string) => void;
  placeholder: string;
  ariaLabel: string;
  disabled?: boolean;
  className?: string;
};

export function SearchExpressionEditor({
  mode,
  tokens,
  draft,
  rawExpression,
  onModeChange,
  onTokensChange,
  onDraftChange,
  onRawExpressionChange,
  placeholder,
  ariaLabel,
  disabled = false,
  className = ''
}: SearchExpressionEditorProps) {
  return (
    <div className={`flex min-w-0 flex-1 flex-col gap-1 ${className}`}>
      <div className="flex items-center gap-1 text-[11px]" role="tablist" aria-label="搜索表达式模式">
        <button
          type="button"
          role="tab"
          aria-selected={mode === 'simple'}
          className={`rounded px-2 py-0.5 font-semibold ${mode === 'simple' ? 'bg-slate-200 text-slate-900' : 'text-slate-500 hover:bg-slate-100'}`}
          disabled={disabled}
          onClick={() => onModeChange('simple')}
        >
          简单模式
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={mode === 'advanced'}
          className={`rounded px-2 py-0.5 font-semibold ${mode === 'advanced' ? 'bg-cyan-100 text-cyan-900' : 'text-slate-500 hover:bg-slate-100'}`}
          disabled={disabled}
          onClick={() => onModeChange('advanced')}
        >
          高级表达式
        </button>
      </div>
      {mode === 'advanced' ? (
        <>
          <textarea
            className="min-h-16 w-full resize-y bg-transparent px-1 font-mono text-sm text-slate-950 outline-none placeholder:text-slate-500"
            aria-label={ariaLabel}
            placeholder={placeholder}
            maxLength={4096}
            value={rawExpression}
            disabled={disabled}
            onChange={(event) => onRawExpressionChange(event.target.value)}
          />
          <p className="px-1 text-[11px] text-slate-500">
            支持 AND / OR / NOT / () 和引号短语；优先级为 NOT &gt; AND &gt; OR。
          </p>
        </>
      ) : (
        <>
          <SearchTokenEditor
            tokens={tokens}
            draft={draft}
            onTokensChange={onTokensChange}
            onDraftChange={onDraftChange}
            placeholder={placeholder}
            ariaLabel={ariaLabel}
            disabled={disabled}
          />
          <p className="px-1 text-[11px] text-slate-500">
            AND 优先于 OR；需要括号或前置 NOT 时请切换高级表达式。
          </p>
        </>
      )}
    </div>
  );
}
