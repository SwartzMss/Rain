import React, { useEffect, useRef, useState } from 'react';
import {
  appendSearchOperator,
  appendSearchTerm,
  expectsSearchTerm,
  removeSearchToken,
  replaceSearchOperator,
  replaceSearchTerm,
  type SearchToken
} from './searchTokens';

type SearchTokenEditorProps = {
  tokens: SearchToken[];
  draft: string;
  onTokensChange: (tokens: SearchToken[]) => void;
  onDraftChange: (draft: string) => void;
  placeholder: string;
  ariaLabel: string;
  allowOperators?: boolean;
  disabled?: boolean;
  className?: string;
};

export function SearchTokenEditor({
  tokens,
  draft,
  onTokensChange,
  onDraftChange,
  placeholder,
  ariaLabel,
  allowOperators = true,
  disabled = false,
  className = ''
}: SearchTokenEditorProps) {
  const [editingIndex, setEditingIndex] = useState<number | null>(null);
  const [editingValue, setEditingValue] = useState('');
  const inputRef = useRef<HTMLInputElement | null>(null);
  const tokenRefs = useRef<Array<HTMLButtonElement | null>>([]);

  useEffect(() => {
    if (editingIndex !== null && tokens[editingIndex]?.kind !== 'term') {
      setEditingIndex(null);
    }
  }, [editingIndex, tokens]);

  const commitDraft = () => {
    const next = appendSearchTerm(tokens, draft, allowOperators);
    if (next === tokens) return;
    onTokensChange(next);
    onDraftChange('');
  };

  const commitEdit = () => {
    if (editingIndex === null) return;
    onTokensChange(replaceSearchTerm(tokens, editingIndex, editingValue));
    setEditingIndex(null);
    setEditingValue('');
  };

  const focusToken = (index: number) => {
    tokenRefs.current[index]?.focus();
  };

  const removeAt = (index: number) => {
    onTokensChange(removeSearchToken(tokens, index));
    window.setTimeout(() => {
      const nextIndex = Math.min(index - 1, tokens.length - 2);
      if (nextIndex >= 0) focusToken(nextIndex);
      else inputRef.current?.focus();
    });
  };

  const canAddOperator = allowOperators && !expectsSearchTerm(tokens) && !draft.trim();

  return (
    <div
      className={`flex min-w-0 flex-1 flex-wrap items-center gap-1.5 ${className}`}
      role="group"
      aria-label={ariaLabel}
    >
      {tokens.map((token, index) => (
        <span
          key={`${token.kind}:${token.value}:${index}`}
          className={`inline-flex h-7 max-w-full items-center overflow-hidden rounded border text-xs ${
            token.kind === 'operator'
              ? 'border-cyan-500/50 bg-cyan-500/15 font-semibold text-cyan-100'
              : 'border-slate-600 bg-slate-800 text-slate-100'
          }`}
        >
          {editingIndex === index && token.kind === 'term' ? (
            <input
              autoFocus
              className="h-full min-w-24 max-w-56 bg-slate-950 px-2 text-xs text-white outline-none"
              aria-label={`编辑关键词 ${token.value}`}
              value={editingValue}
              onChange={(event) => setEditingValue(event.target.value)}
              onBlur={commitEdit}
              onKeyDown={(event) => {
                if (event.key === 'Enter') {
                  event.preventDefault();
                  commitEdit();
                  inputRef.current?.focus();
                } else if (event.key === 'Escape') {
                  event.preventDefault();
                  setEditingIndex(null);
                }
              }}
            />
          ) : (
            <button
              ref={(element) => { tokenRefs.current[index] = element; }}
              type="button"
              className="min-w-0 truncate px-2 py-1"
              title={token.kind === 'operator' && token.value !== 'NOT' ? '点击切换 AND / OR' : token.value}
              aria-label={token.kind === 'term' ? `编辑关键词 ${token.value}` : `${token.value} 运算符`}
              disabled={disabled}
              onClick={() => {
                if (token.kind === 'term') {
                  setEditingIndex(index);
                  setEditingValue(token.value);
                } else if (token.value !== 'NOT') {
                  onTokensChange(replaceSearchOperator(tokens, index, token.value === 'AND' ? 'OR' : 'AND'));
                }
              }}
              onKeyDown={(event) => {
                if (event.key === 'ArrowLeft') {
                  event.preventDefault();
                  focusToken(index - 1);
                } else if (event.key === 'ArrowRight') {
                  event.preventDefault();
                  if (index + 1 < tokens.length) focusToken(index + 1);
                  else inputRef.current?.focus();
                } else if (event.key === 'Backspace' || event.key === 'Delete') {
                  event.preventDefault();
                  removeAt(index);
                }
              }}
            >
              {token.value}
            </button>
          )}
          <button
            type="button"
            className="flex h-full w-6 shrink-0 items-center justify-center border-l border-current/20 opacity-60 hover:opacity-100"
            title={`删除 ${token.value}`}
            aria-label={`删除${token.kind === 'term' ? '关键词' : '运算符'} ${token.value}`}
            disabled={disabled}
            onClick={() => removeAt(index)}
          >
            ×
          </button>
        </span>
      ))}

      <input
        ref={inputRef}
        className="h-8 min-w-32 flex-1 bg-transparent px-1 text-sm text-white outline-none placeholder:text-slate-500"
        placeholder={placeholder}
        aria-label={ariaLabel}
        value={draft}
        disabled={disabled}
        onChange={(event) => onDraftChange(event.target.value)}
        onKeyDown={(event) => {
          if ((event.key === 'Enter' || event.key === 'Tab') && draft.trim()) {
            event.preventDefault();
            commitDraft();
          } else if (event.key === 'Backspace' && !draft && tokens.length > 0) {
            event.preventDefault();
            removeAt(tokens.length - 1);
          } else if (event.key === 'ArrowLeft' && !draft && tokens.length > 0) {
            event.preventDefault();
            focusToken(tokens.length - 1);
          }
        }}
      />

      {draft.trim() ? (
        <button
          type="button"
          className="flex h-7 w-7 shrink-0 items-center justify-center rounded border border-slate-600 text-slate-300 hover:border-slate-400 hover:text-white"
          title="添加关键词"
          aria-label="添加关键词"
          disabled={disabled}
          onClick={commitDraft}
        >
          +
        </button>
      ) : null}
      {canAddOperator ? (
        <>
          <button
            type="button"
            className="h-7 rounded border border-cyan-500/40 px-2 text-xs font-semibold text-cyan-200 hover:bg-cyan-500/15"
            disabled={disabled}
            onClick={() => onTokensChange(appendSearchOperator(tokens, 'AND'))}
          >
            AND
          </button>
          <button
            type="button"
            className="h-7 rounded border border-cyan-500/40 px-2 text-xs font-semibold text-cyan-200 hover:bg-cyan-500/15"
            disabled={disabled}
            onClick={() => onTokensChange(appendSearchOperator(tokens, 'OR'))}
          >
            OR
          </button>
          <button
            type="button"
            className="h-7 rounded border border-cyan-500/40 px-2 text-xs font-semibold text-cyan-200 hover:bg-cyan-500/15"
            disabled={disabled}
            onClick={() => {
              const withAnd = appendSearchOperator(tokens, 'AND');
              onTokensChange(appendSearchOperator(withAnd, 'NOT'));
            }}
          >
            NOT
          </button>
        </>
      ) : null}
    </div>
  );
}
