import React, { useEffect, useRef, useState } from 'react';
import {
  appendSearchOperator,
  appendSearchParen,
  appendSearchTerm,
  expectsSearchTerm,
  hasUnclosedSearchParens,
  removeSearchToken,
  replaceSearchOperator,
  replaceSearchTerm,
  type SearchOperator,
  type SearchParen,
  type SearchToken
} from './searchTokens';

type SearchTokenEditorProps = {
  tokens: SearchToken[];
  draft: string;
  onTokensChange: (tokens: SearchToken[]) => void;
  onDraftChange: (draft: string) => void;
  onSubmit?: (tokens: SearchToken[], draft: string) => void;
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
  onSubmit,
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
    if (editingIndex !== null && tokens[editingIndex]?.kind !== 'term') setEditingIndex(null);
  }, [editingIndex, tokens]);

  const commitDraft = () => {
    const next = appendSearchTerm(tokens, draft, allowOperators);
    if (next === tokens) return;
    onTokensChange(next);
    onDraftChange('');
  };

  const commitEdit = () => {
    if (editingIndex === null) return;
    const next = replaceSearchTerm(tokens, editingIndex, editingValue);
    if (next !== tokens) onTokensChange(next);
    setEditingIndex(null);
    setEditingValue('');
  };

  const focusToken = (index: number) => tokenRefs.current[index]?.focus();

  const removeAt = (index: number) => {
    onTokensChange(removeSearchToken(tokens, index));
    window.setTimeout(() => {
      const nextIndex = Math.min(index - 1, tokens.length - 2);
      if (nextIndex >= 0) focusToken(nextIndex);
      else inputRef.current?.focus();
    });
  };

  const updateOperator = (operator: SearchOperator) => {
    const withDraft = draft.trim()
      ? appendSearchTerm(tokens, draft, allowOperators)
      : tokens;
    const next = appendSearchOperator(withDraft, operator);
    if (withDraft !== tokens) onDraftChange('');
    if (next !== tokens || withDraft !== tokens) onTokensChange(next);
  };

  const updateParen = (paren: SearchParen) => {
    const withDraft = draft.trim()
      ? appendSearchTerm(tokens, draft, allowOperators)
      : tokens;
    const next = appendSearchParen(withDraft, paren);
    if (withDraft !== tokens) onDraftChange('');
    if (next !== tokens || withDraft !== tokens) onTokensChange(next);
  };

  const tokensForControls = draft.trim()
    ? appendSearchTerm(tokens, draft, allowOperators)
    : tokens;
  const operandExpected = expectsSearchTerm(tokensForControls);
  const last = tokensForControls[tokensForControls.length - 1];
  const canAddBinary = allowOperators && (last?.kind === 'term' || last?.kind === 'paren' && last.value === ')' || last?.kind === 'operator' && (last.value === 'AND' || last.value === 'OR'));
  const canAddNot = allowOperators && (operandExpected || last?.kind === 'term' || last?.kind === 'paren' && last.value === ')');
  const canAddLeftParen = allowOperators && (operandExpected || Boolean(last));
  const canAddRightParen = allowOperators && !operandExpected && hasUnclosedSearchParens(tokensForControls) > 0;

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
            token.kind === 'term'
              ? 'border-slate-300 bg-slate-100 text-slate-900'
              : token.kind === 'paren'
                ? 'border-violet-500/50 bg-violet-500/10 font-semibold text-violet-800'
                : 'border-cyan-500/50 bg-cyan-500/15 font-semibold text-cyan-800'
          }`}
        >
          {editingIndex === index && token.kind === 'term' ? (
            <input
              autoFocus
              className="h-full min-w-24 max-w-56 bg-white px-2 text-xs text-slate-950 outline-none"
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
                  setEditingValue('');
                }
              }}
            />
          ) : (
            <button
              ref={(element) => { tokenRefs.current[index] = element; }}
              type="button"
              className="min-w-0 truncate px-2 py-1"
              title={token.kind === 'operator' && token.value !== 'NOT' ? '切换 AND / OR' : token.value}
              aria-label={token.kind === 'term' ? `编辑关键词 ${token.value}` : `${token.value} 运算符`}
              disabled={disabled}
              onClick={() => {
                if (token.kind === 'term') {
                  setEditingIndex(index);
                  setEditingValue(token.value);
                } else if (token.kind === 'operator' && token.value !== 'NOT') {
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
            aria-label={`删除${token.kind === 'term' ? '关键词' : '语法'} ${token.value}`}
            disabled={disabled}
            onClick={() => removeAt(index)}
          >
            ×
          </button>
        </span>
      ))}

      <input
        ref={inputRef}
        className="h-8 min-w-32 flex-1 bg-transparent px-1 text-sm text-slate-950 outline-none placeholder:text-slate-500"
        placeholder={placeholder}
        aria-label={ariaLabel}
        value={draft}
        disabled={disabled}
        onChange={(event) => onDraftChange(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === 'Enter' && onSubmit) {
            event.preventDefault();
            onSubmit(tokens, draft);
          } else if ((event.key === 'Enter' || event.key === 'Tab') && draft.trim()) {
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

      {allowOperators ? (
        <div className="flex items-center gap-1" aria-label="搜索语法">
          <button type="button" className="h-7 rounded border border-cyan-500/40 px-2 text-xs font-semibold text-cyan-700 hover:bg-cyan-500/15 disabled:opacity-40" disabled={disabled || !canAddBinary} onClick={() => updateOperator('AND')}>AND</button>
          <button type="button" className="h-7 rounded border border-cyan-500/40 px-2 text-xs font-semibold text-cyan-700 hover:bg-cyan-500/15 disabled:opacity-40" disabled={disabled || !canAddBinary} onClick={() => updateOperator('OR')}>OR</button>
          <button type="button" className="h-7 rounded border border-cyan-500/40 px-2 text-xs font-semibold text-cyan-700 hover:bg-cyan-500/15 disabled:opacity-40" disabled={disabled || !canAddNot} onClick={() => updateOperator('NOT')}>NOT</button>
          <button type="button" className="h-7 rounded border border-violet-500/40 px-2 text-xs font-semibold text-violet-700 hover:bg-violet-500/15 disabled:opacity-40" disabled={disabled || !canAddLeftParen} onClick={() => updateParen('(')}>(</button>
          <button type="button" className="h-7 rounded border border-violet-500/40 px-2 text-xs font-semibold text-violet-700 hover:bg-violet-500/15 disabled:opacity-40" disabled={disabled || !canAddRightParen} onClick={() => updateParen(')')}>)</button>
        </div>
      ) : null}
    </div>
  );
}
