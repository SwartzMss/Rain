export type SearchOperator = 'AND' | 'OR' | 'NOT';

export type SearchToken =
  | { kind: 'term'; value: string }
  | { kind: 'operator'; value: SearchOperator };

export type SearchTokenValidation =
  | { valid: true }
  | { valid: false; message: string };

export function expectsSearchTerm(tokens: SearchToken[]): boolean {
  if (tokens.length === 0) return true;
  return tokens[tokens.length - 1].kind === 'operator';
}

export function appendSearchTerm(
  tokens: SearchToken[],
  value: string,
  allowOperators = true
): SearchToken[] {
  const term = value.trim();
  if (!term) return tokens;
  if (!allowOperators) return [{ kind: 'term', value: term }];

  const next = [...tokens];
  if (!expectsSearchTerm(next)) {
    next.push({ kind: 'operator', value: 'AND' });
  }
  next.push({ kind: 'term', value: term });
  return next;
}

export function appendSearchOperator(
  tokens: SearchToken[],
  operator: SearchOperator
): SearchToken[] {
  if (operator === 'NOT') {
    if (!expectsSearchTerm(tokens) || tokens[tokens.length - 1]?.value === 'NOT') return tokens;
    return [...tokens, { kind: 'operator', value: operator }];
  }

  if (tokens.length === 0) return tokens;
  const last = tokens[tokens.length - 1];
  if (last.kind === 'term') {
    return [...tokens, { kind: 'operator', value: operator }];
  }
  if (last.value === 'AND' || last.value === 'OR') {
    return [...tokens.slice(0, -1), { kind: 'operator', value: operator }];
  }
  return tokens;
}

export function replaceSearchOperator(
  tokens: SearchToken[],
  index: number,
  operator: 'AND' | 'OR'
): SearchToken[] {
  const current = tokens[index];
  if (!current || current.kind !== 'operator' || current.value === 'NOT') return tokens;
  return tokens.map((token, tokenIndex) =>
    tokenIndex === index ? { kind: 'operator', value: operator } : token
  );
}

export function replaceSearchTerm(
  tokens: SearchToken[],
  index: number,
  value: string
): SearchToken[] {
  const term = value.trim();
  if (!term || tokens[index]?.kind !== 'term') return tokens;
  return tokens.map((token, tokenIndex) =>
    tokenIndex === index ? { kind: 'term', value: term } : token
  );
}

export function removeSearchToken(tokens: SearchToken[], index: number): SearchToken[] {
  const token = tokens[index];
  if (!token) return tokens;
  if (token.kind === 'operator' && token.value === 'NOT') {
    return tokens.filter((_, tokenIndex) => tokenIndex !== index);
  }

  let start = index;
  let end = index;
  if (token.kind === 'operator') {
    while (end + 1 < tokens.length && tokens[end + 1].kind === 'operator') end += 1;
    if (end + 1 < tokens.length) end += 1;
  } else {
    while (start > 0 && tokens[start - 1].kind === 'operator' && tokens[start - 1].value === 'NOT') {
      start -= 1;
    }
    if (start > 0) {
      start -= 1;
    } else if (end + 1 < tokens.length && tokens[end + 1].kind === 'operator') {
      end += 1;
    }
  }
  return tokens.filter((_, tokenIndex) => tokenIndex < start || tokenIndex > end);
}

export function validateSearchTokens(tokens: SearchToken[]): SearchTokenValidation {
  if (tokens.length === 0) return { valid: false, message: '请添加搜索关键词' };

  let expectsTerm = true;
  for (const token of tokens) {
    if (expectsTerm) {
      if (token.kind === 'term') {
        if (!token.value.trim()) return { valid: false, message: '关键词不能为空' };
        expectsTerm = false;
        continue;
      }
      if (token.value === 'NOT') continue;
      return { valid: false, message: `${token.value} 前缺少关键词` };
    }

    if (token.kind === 'operator' && (token.value === 'AND' || token.value === 'OR')) {
      expectsTerm = true;
      continue;
    }
    return { valid: false, message: '关键词之间需要 AND 或 OR' };
  }

  return expectsTerm
    ? { valid: false, message: 'AND、OR 或 NOT 后缺少关键词' }
    : { valid: true };
}

export function finalizeSearchTokens(
  tokens: SearchToken[],
  draft: string,
  allowOperators = true
): SearchToken[] {
  const finalized = appendSearchTerm(tokens, draft, allowOperators);
  const validation = validateSearchTokens(finalized);
  if (!validation.valid) throw new Error(validation.message);
  return finalized;
}

export function quoteSearchTerm(value: string): string {
  return `"${value.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`;
}

export function serializeSearchTokens(tokens: SearchToken[]): string {
  const validation = validateSearchTokens(tokens);
  if (!validation.valid) throw new Error(validation.message);
  return tokens
    .map((token) => token.kind === 'term' ? quoteSearchTerm(token.value) : token.value)
    .join(' ');
}

export function formatSearchTokens(tokens: SearchToken[]): string {
  return tokens.map((token) => token.value).join(' ');
}

export function getSearchTerms(tokens: SearchToken[]): string[] {
  return tokens.flatMap((token) => token.kind === 'term' ? [token.value] : []);
}

export function canFinalizeSearch(tokens: SearchToken[], draft: string): boolean {
  try {
    finalizeSearchTokens(tokens, draft);
    return true;
  } catch {
    return false;
  }
}

export function combineSearchExpressions(previous: string, next: string): string {
  return `(${previous}) AND (${next})`;
}
