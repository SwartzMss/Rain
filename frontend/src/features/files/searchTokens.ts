export type SearchOperator = 'AND' | 'OR' | 'NOT';
export type SearchParen = '(' | ')';

export type SearchToken =
  | { kind: 'term'; value: string }
  | { kind: 'operator'; value: SearchOperator }
  | { kind: 'paren'; value: SearchParen };

export type SearchTokenValidation =
  | { valid: true }
  | { valid: false; message: string };

function isBinaryOperator(token: SearchToken | undefined): boolean {
  return token?.kind === 'operator' && (token.value === 'AND' || token.value === 'OR');
}

function isOperandEnd(token: SearchToken | undefined): boolean {
  return token?.kind === 'term' || (token?.kind === 'paren' && token.value === ')');
}

function isOperandStart(token: SearchToken | undefined): boolean {
  return token?.kind === 'term'
    || (token?.kind === 'paren' && token.value === '(')
    || (token?.kind === 'operator' && token.value === 'NOT');
}

export function expectsSearchTerm(tokens: SearchToken[]): boolean {
  if (tokens.length === 0) return true;
  return !isOperandEnd(tokens[tokens.length - 1]);
}

export function hasUnclosedSearchParens(tokens: SearchToken[]): number {
  let depth = 0;
  for (const token of tokens) {
    if (token.kind === 'paren') depth += token.value === '(' ? 1 : -1;
  }
  return Math.max(depth, 0);
}

function canAppendLeftParen(tokens: SearchToken[]): boolean {
  return tokens.length === 0 || expectsSearchTerm(tokens);
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
  if (isOperandEnd(next[next.length - 1])) {
    next.push({ kind: 'operator', value: 'AND' });
  }
  next.push({ kind: 'term', value: term });
  return next;
}

export function appendSearchOperator(
  tokens: SearchToken[],
  operator: SearchOperator
): SearchToken[] {
  const next = [...tokens];
  if (operator === 'NOT') {
    if (isOperandEnd(next[next.length - 1])) next.push({ kind: 'operator', value: 'AND' });
    if (!next.length || expectsSearchTerm(next)) next.push({ kind: 'operator', value: 'NOT' });
    return next;
  }

  const last = next[next.length - 1];
  if (isOperandEnd(last)) return [...next, { kind: 'operator', value: operator }];
  if (isBinaryOperator(last)) {
    return [...next.slice(0, -1), { kind: 'operator', value: operator }];
  }
  return next;
}

export function appendSearchParen(tokens: SearchToken[], paren: SearchParen): SearchToken[] {
  if (paren === '(') {
    const next = [...tokens];
    if (isOperandEnd(next[next.length - 1])) next.push({ kind: 'operator', value: 'AND' });
    if (canAppendLeftParen(next)) next.push({ kind: 'paren', value: '(' });
    return next;
  }
  if (!tokens.length || expectsSearchTerm(tokens) || hasUnclosedSearchParens(tokens) === 0) return tokens;
  return [...tokens, { kind: 'paren', value: ')' }];
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

function matchingParen(tokens: SearchToken[], index: number): number | null {
  const token = tokens[index];
  if (token?.kind !== 'paren') return null;
  let depth = 0;
  if (token.value === '(') {
    for (let cursor = index; cursor < tokens.length; cursor += 1) {
      const current = tokens[cursor];
      if (current.kind !== 'paren') continue;
      depth += current.value === '(' ? 1 : -1;
      if (depth === 0) return cursor;
    }
  } else {
    for (let cursor = index; cursor >= 0; cursor -= 1) {
      const current = tokens[cursor];
      if (current.kind !== 'paren') continue;
      depth += current.value === ')' ? 1 : -1;
      if (depth === 0) return cursor;
    }
  }
  return null;
}

function operandStart(tokens: SearchToken[], index: number): number {
  let start = index;
  while (start > 0 && tokens[start - 1].kind === 'operator' && tokens[start - 1].value === 'NOT') start -= 1;
  return start;
}

function operandEnd(tokens: SearchToken[], index: number): number {
  let cursor = index;
  while (tokens[cursor]?.kind === 'operator' && tokens[cursor].value === 'NOT') cursor += 1;
  if (tokens[cursor]?.kind === 'paren' && tokens[cursor].value === '(') {
    return matchingParen(tokens, cursor) ?? tokens.length - 1;
  }
  return cursor;
}

function cleanupSearchTokens(tokens: SearchToken[]): SearchToken[] {
  let next = [...tokens];
  let changed = true;
  while (changed) {
    changed = false;
    for (let index = 0; index < next.length; index += 1) {
      const token = next[index];
      if (token.kind === 'paren' && token.value === '(') {
        const close = matchingParen(next, index);
        if (close === index + 1) {
          let start = index;
          while (start > 0 && next[start - 1].kind === 'operator' && next[start - 1].value === 'NOT') start -= 1;
          next.splice(start, close - start + 1);
          changed = true;
          break;
        }
      }
      if (isBinaryOperator(token) && (!isOperandEnd(next[index - 1]) || !isOperandStart(next[index + 1]))) {
        next.splice(index, 1);
        changed = true;
        break;
      }
    }
  }
  return next;
}

export function removeSearchToken(tokens: SearchToken[], index: number): SearchToken[] {
  const token = tokens[index];
  if (!token) return tokens;

  let start = index;
  let end = index;
  if (token.kind === 'paren') {
    const pair = matchingParen(tokens, index);
    if (pair !== null) {
      start = Math.min(index, pair);
      end = Math.max(index, pair);
    }
  } else if (token.kind === 'operator' && token.value === 'NOT') {
    return cleanupSearchTokens(tokens.filter((_, tokenIndex) => tokenIndex !== index));
  } else if (token.kind === 'operator') {
    const right = index + 1;
    start = index;
    end = right < tokens.length ? operandEnd(tokens, operandStart(tokens, right)) : index;
  } else {
    start = operandStart(tokens, index);
    end = operandEnd(tokens, index);
  }

  return cleanupSearchTokens(tokens.filter((_, tokenIndex) => tokenIndex < start || tokenIndex > end));
}

export function validateSearchTokens(tokens: SearchToken[]): SearchTokenValidation {
  if (tokens.length === 0) return { valid: false, message: '请添加搜索关键词' };

  let expectsOperand = true;
  let depth = 0;
  for (const token of tokens) {
    if (expectsOperand) {
      if (token.kind === 'term') {
        if (!token.value.trim()) return { valid: false, message: '关键词不能为空' };
        expectsOperand = false;
      } else if (token.kind === 'operator' && token.value === 'NOT') {
        continue;
      } else if (token.kind === 'paren' && token.value === '(') {
        depth += 1;
      } else {
        return { valid: false, message: `${token.value} 前缺少关键词` };
      }
      continue;
    }

    if (token.kind === 'operator' && (token.value === 'AND' || token.value === 'OR')) {
      expectsOperand = true;
    } else if (token.kind === 'paren' && token.value === ')') {
      if (depth === 0) return { valid: false, message: '右括号前缺少左括号' };
      depth -= 1;
    } else {
      return { valid: false, message: '关键词之间需要 AND 或 OR' };
    }
  }

  if (expectsOperand) return { valid: false, message: '运算符或左括号后缺少关键词' };
  if (depth > 0) return { valid: false, message: '缺少右括号' };
  return { valid: true };
}

export function finalizeSearchTokens(tokens: SearchToken[], draft: string, allowOperators = true): SearchToken[] {
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
  return tokens.map((token) => token.kind === 'term' ? quoteSearchTerm(token.value) : token.value).join(' ');
}

export function deserializeSearchTokens(expression: string): SearchToken[] {
  const tokens: SearchToken[] = [];
  let index = 0;
  while (index < expression.length) {
    while (/\s/.test(expression[index] ?? '')) index += 1;
    if (index >= expression.length) break;
    if (expression[index] === '(' || expression[index] === ')') {
      tokens.push({ kind: 'paren', value: expression[index] as SearchParen });
      index += 1;
      continue;
    }
    if (expression[index] === '"') {
      index += 1;
      let value = '';
      let closed = false;
      while (index < expression.length) {
        const character = expression[index];
        index += 1;
        if (character === '"') {
          closed = true;
          break;
        }
        if (character === '\\') {
          if (index >= expression.length) throw new Error('搜索表达式转义不完整');
          const escaped = expression[index];
          value += escaped === '"' || escaped === '\\' ? escaped : `\\${escaped}`;
          index += 1;
        } else {
          value += character;
        }
      }
      if (!closed || !value.trim()) throw new Error('搜索表达式中的关键词无效');
      tokens.push({ kind: 'term', value });
      continue;
    }
    const start = index;
    while (index < expression.length && !/\s/.test(expression[index]) && expression[index] !== '(' && expression[index] !== ')') index += 1;
    const fragment = expression.slice(start, index);
    const normalizedFragment = fragment.toUpperCase();
    if (['AND', 'OR', 'NOT'].includes(normalizedFragment)) {
      tokens.push({ kind: 'operator', value: normalizedFragment as SearchOperator });
    } else if (fragment) {
      tokens.push({ kind: 'term', value: fragment });
    }
  }
  const validation = validateSearchTokens(tokens);
  if (!validation.valid) throw new Error(validation.message);
  return tokens;
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
