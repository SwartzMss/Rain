import React from 'react';
import { SearchTokenEditor } from './SearchTokenEditor';
import type { SearchToken } from './searchTokens';

type SearchExpressionEditorProps = {
  tokens: SearchToken[];
  draft: string;
  onTokensChange: (tokens: SearchToken[]) => void;
  onDraftChange: (draft: string) => void;
  placeholder: string;
  ariaLabel: string;
  disabled?: boolean;
  className?: string;
};

export function SearchExpressionEditor({
  tokens,
  draft,
  onTokensChange,
  onDraftChange,
  placeholder,
  ariaLabel,
  disabled = false,
  className = ''
}: SearchExpressionEditorProps) {
  return (
    <div className={`flex min-w-0 flex-1 flex-col gap-1 ${className}`}>
      <SearchTokenEditor
        tokens={tokens}
        draft={draft}
        onTokensChange={onTokensChange}
        onDraftChange={onDraftChange}
        placeholder={placeholder}
        ariaLabel={ariaLabel}
        disabled={disabled}
      />
    </div>
  );
}
