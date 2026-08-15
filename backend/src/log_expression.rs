use caseless::default_case_fold_str;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expression {
    Term(String),
    Not(Box<Expression>),
    And(Box<Expression>, Box<Expression>),
    Or(Box<Expression>, Box<Expression>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub offset: usize,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TokenKind {
    Term(String),
    And,
    Or,
    Not,
    LeftParen,
    RightParen,
}

#[derive(Debug, Clone)]
struct Token {
    kind: TokenKind,
    offset: usize,
}

impl Expression {
    pub fn matches(&self, line: &str) -> bool {
        let normalized = default_case_fold_str(line);
        self.matches_normalized(&normalized)
    }

    fn matches_normalized(&self, line: &str) -> bool {
        match self {
            Self::Term(term) => line.contains(term),
            Self::Not(expression) => !expression.matches_normalized(line),
            Self::And(left, right) => {
                left.matches_normalized(line) && right.matches_normalized(line)
            }
            Self::Or(left, right) => {
                left.matches_normalized(line) || right.matches_normalized(line)
            }
        }
    }

    pub(crate) fn chunk_matcher(&self) -> ExpressionChunkMatcher {
        let mut terms = Vec::new();
        collect_terms(self, &mut terms);
        ExpressionChunkMatcher {
            terms: terms
                .into_iter()
                .map(|term| ChunkTerm {
                    keep_chars: term.chars().count().saturating_sub(1),
                    term,
                    tail: String::new(),
                    found: false,
                })
                .collect(),
            pending: Vec::new(),
        }
    }
}

pub const MAX_EXPRESSION_CHARS: usize = 4_096;
pub const MAX_EXPRESSION_BYTES: usize = 16 * 1024;
const MAX_EXPRESSION_TOKENS: usize = 128;
const MAX_EXPRESSION_NESTING_DEPTH: usize = 32;
const MAX_EXPRESSION_AST_NODES: usize = 128;

pub(crate) struct ExpressionChunkMatcher {
    terms: Vec<ChunkTerm>,
    pending: Vec<u8>,
}

struct ChunkTerm {
    term: String,
    keep_chars: usize,
    tail: String,
    found: bool,
}

impl ExpressionChunkMatcher {
    pub(crate) fn reset(&mut self) {
        self.pending.clear();
        for term in &mut self.terms {
            term.tail.clear();
            term.found = false;
        }
    }

    pub(crate) fn feed_bytes(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
        self.decode_pending(false);
    }

    pub(crate) fn finish(&mut self) {
        self.decode_pending(true);
    }

    pub(crate) fn matches(&self, expression: &Expression) -> bool {
        let mut cursor = 0;
        let result = evaluate_chunk_matches(expression, &self.terms, &mut cursor);
        debug_assert_eq!(cursor, self.terms.len());
        result
    }

    fn decode_pending(&mut self, flush: bool) {
        if self.pending.is_empty() {
            return;
        }

        let mut decoded = String::new();
        let mut consumed = 0;
        while consumed < self.pending.len() {
            match std::str::from_utf8(&self.pending[consumed..]) {
                Ok(text) => {
                    decoded.push_str(text);
                    consumed = self.pending.len();
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    if valid > 0 {
                        decoded.push_str(
                            std::str::from_utf8(&self.pending[consumed..consumed + valid])
                                .expect("valid UTF-8 prefix"),
                        );
                        consumed += valid;
                    }
                    if let Some(invalid_length) = error.error_len() {
                        decoded.push('\u{fffd}');
                        consumed += invalid_length;
                    } else if flush {
                        decoded.push('\u{fffd}');
                        consumed = self.pending.len();
                    } else {
                        break;
                    }
                }
            }
        }

        if !decoded.is_empty() {
            self.feed_text(&decoded);
        }
        if consumed > 0 {
            self.pending.drain(..consumed);
        }
    }

    fn feed_text(&mut self, text: &str) {
        self.feed_normalized(&default_case_fold_str(text));
    }

    fn feed_normalized(&mut self, normalized: &str) {
        for term in &mut self.terms {
            if term.found {
                continue;
            }
            let combined = format!("{}{}", term.tail, normalized);
            if combined.contains(&term.term) {
                term.found = true;
            } else {
                term.tail = keep_suffix(&combined, term.keep_chars);
            }
        }
    }
}

fn collect_terms(expression: &Expression, terms: &mut Vec<String>) {
    match expression {
        Expression::Term(term) => terms.push(term.clone()),
        Expression::Not(expression) => collect_terms(expression, terms),
        Expression::And(left, right) | Expression::Or(left, right) => {
            collect_terms(left, terms);
            collect_terms(right, terms);
        }
    }
}

fn evaluate_chunk_matches(
    expression: &Expression,
    terms: &[ChunkTerm],
    cursor: &mut usize,
) -> bool {
    match expression {
        Expression::Term(_) => {
            let result = terms[*cursor].found;
            *cursor += 1;
            result
        }
        Expression::Not(expression) => !evaluate_chunk_matches(expression, terms, cursor),
        Expression::And(left, right) => {
            let left = evaluate_chunk_matches(left, terms, cursor);
            let right = evaluate_chunk_matches(right, terms, cursor);
            left && right
        }
        Expression::Or(left, right) => {
            let left = evaluate_chunk_matches(left, terms, cursor);
            let right = evaluate_chunk_matches(right, terms, cursor);
            left || right
        }
    }
}

fn keep_suffix(value: &str, chars: usize) -> String {
    value
        .chars()
        .rev()
        .take(chars)
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

pub fn parse(input: &str) -> Result<Expression, ParseError> {
    if input.len() > MAX_EXPRESSION_BYTES {
        return Err(ParseError {
            offset: MAX_EXPRESSION_BYTES,
            message: "expression exceeds the byte limit".into(),
        });
    }
    if input.chars().count() > MAX_EXPRESSION_CHARS {
        let offset = input
            .char_indices()
            .nth(MAX_EXPRESSION_CHARS)
            .map_or(input.len(), |(offset, _)| offset);
        return Err(ParseError {
            offset,
            message: "expression exceeds the character limit".into(),
        });
    }
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Err(ParseError {
            offset: 0,
            message: "expression is required".into(),
        });
    }
    if tokens.len() > MAX_EXPRESSION_TOKENS {
        return Err(ParseError {
            offset: tokens[MAX_EXPRESSION_TOKENS].offset,
            message: "expression has too many tokens".into(),
        });
    }
    let mut parser = Parser {
        tokens,
        cursor: 0,
        nesting_depth: 0,
    };
    let expression = parser.parse_or()?;
    if let Some(token) = parser.peek() {
        return Err(ParseError {
            offset: token.offset,
            message: "unexpected token".into(),
        });
    }
    if ast_node_count(&expression) > MAX_EXPRESSION_AST_NODES {
        return Err(ParseError {
            offset: input.len(),
            message: "expression has too many AST nodes".into(),
        });
    }
    Ok(expression)
}

fn tokenize(input: &str) -> Result<Vec<Token>, ParseError> {
    let mut tokens = Vec::new();
    let mut cursor = 0;
    while cursor < input.len() {
        let character = input[cursor..].chars().next().expect("character");
        if character.is_whitespace() {
            cursor += character.len_utf8();
            continue;
        }
        if character == '(' || character == ')' {
            tokens.push(Token {
                kind: if character == '(' {
                    TokenKind::LeftParen
                } else {
                    TokenKind::RightParen
                },
                offset: cursor,
            });
            cursor += 1;
            continue;
        }
        if character == '"' {
            let offset = cursor;
            cursor += 1;
            let mut phrase = String::new();
            let mut closed = false;
            while cursor < input.len() {
                let quoted = input[cursor..].chars().next().expect("quoted character");
                if quoted == '"' {
                    cursor += quoted.len_utf8();
                    closed = true;
                    break;
                }
                if quoted == '\\' {
                    cursor += quoted.len_utf8();
                    if cursor >= input.len() {
                        return Err(ParseError {
                            offset,
                            message: "unterminated quoted phrase".into(),
                        });
                    }
                    let escaped = input[cursor..].chars().next().expect("escaped character");
                    if escaped != '"' && escaped != '\\' {
                        phrase.push('\\');
                    }
                    phrase.push(escaped);
                    cursor += escaped.len_utf8();
                    continue;
                }
                phrase.push(quoted);
                cursor += quoted.len_utf8();
            }
            if !closed {
                return Err(ParseError {
                    offset,
                    message: "unterminated quoted phrase".into(),
                });
            }
            let phrase = phrase.trim();
            if phrase.is_empty() {
                return Err(ParseError {
                    offset,
                    message: "quoted phrase cannot be empty".into(),
                });
            }
            tokens.push(Token {
                kind: TokenKind::Term(default_case_fold_str(phrase)),
                offset,
            });
            continue;
        }

        let offset = cursor;
        while cursor < input.len() {
            let next = input[cursor..].chars().next().expect("term character");
            if next.is_whitespace() || next == '(' || next == ')' {
                break;
            }
            cursor += next.len_utf8();
        }
        let word = &input[offset..cursor];
        let kind = match word.to_ascii_uppercase().as_str() {
            "AND" => TokenKind::And,
            "OR" => TokenKind::Or,
            "NOT" => TokenKind::Not,
            _ => TokenKind::Term(default_case_fold_str(word)),
        };
        tokens.push(Token { kind, offset });
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    cursor: usize,
    nesting_depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.cursor)
    }

    fn take(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.cursor).cloned();
        self.cursor += usize::from(token.is_some());
        token
    }

    fn parse_or(&mut self) -> Result<Expression, ParseError> {
        let mut expression = self.parse_and()?;
        while matches!(self.peek().map(|token| &token.kind), Some(TokenKind::Or)) {
            self.take();
            expression = Expression::Or(Box::new(expression), Box::new(self.parse_and()?));
        }
        Ok(expression)
    }

    fn parse_and(&mut self) -> Result<Expression, ParseError> {
        let mut expression = self.parse_not()?;
        while matches!(self.peek().map(|token| &token.kind), Some(TokenKind::And)) {
            self.take();
            expression = Expression::And(Box::new(expression), Box::new(self.parse_not()?));
        }
        Ok(expression)
    }

    fn parse_not(&mut self) -> Result<Expression, ParseError> {
        if matches!(self.peek().map(|token| &token.kind), Some(TokenKind::Not)) {
            let token = self.take().expect("NOT token");
            self.enter_nesting(token.offset)?;
            let expression = self.parse_not();
            self.nesting_depth -= 1;
            return Ok(Expression::Not(Box::new(expression?)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expression, ParseError> {
        let Some(token) = self.take() else {
            return Err(ParseError {
                offset: self.tokens.last().map_or(0, |token| token.offset + 1),
                message: "expected a term or parenthesized expression".into(),
            });
        };
        match token.kind {
            TokenKind::Term(term) => Ok(Expression::Term(term)),
            TokenKind::LeftParen => {
                self.enter_nesting(token.offset)?;
                let expression = self.parse_or();
                self.nesting_depth -= 1;
                let expression = expression?;
                let Some(closing) = self.take() else {
                    return Err(ParseError {
                        offset: token.offset,
                        message: "missing closing parenthesis".into(),
                    });
                };
                if closing.kind != TokenKind::RightParen {
                    return Err(ParseError {
                        offset: closing.offset,
                        message: "expected closing parenthesis".into(),
                    });
                }
                Ok(expression)
            }
            _ => Err(ParseError {
                offset: token.offset,
                message: "expected a term or parenthesized expression".into(),
            }),
        }
    }

    fn enter_nesting(&mut self, offset: usize) -> Result<(), ParseError> {
        if self.nesting_depth >= MAX_EXPRESSION_NESTING_DEPTH {
            return Err(ParseError {
                offset,
                message: "expression nesting is too deep".into(),
            });
        }
        self.nesting_depth += 1;
        Ok(())
    }
}

fn ast_node_count(expression: &Expression) -> usize {
    match expression {
        Expression::Term(_) => 1,
        Expression::Not(expression) => 1 + ast_node_count(expression),
        Expression::And(left, right) | Expression::Or(left, right) => {
            1 + ast_node_count(left) + ast_node_count(right)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn applies_not_then_and_then_or_precedence() {
        let expression = parse("error OR warn AND NOT timeout").expect("parse expression");
        assert!(expression.matches("ERROR started"));
        assert!(expression.matches("WARN connected"));
        assert!(!expression.matches("WARN timeout"));
    }

    #[test]
    fn supports_parentheses_and_quoted_phrases() {
        let expression = parse("(ERROR OR WARN) AND \"tracking point\"").expect("parse expression");
        assert!(expression.matches("warn Interaction Tracking Point moved"));
        assert!(!expression.matches("warn tracking stopped"));
    }

    #[test]
    fn supports_escaped_quotes_and_backslashes_in_quoted_phrases() {
        let expression = parse(r#""disk \"primary\" at C:\\logs""#).expect("parse expression");
        assert!(expression.matches(r#"mounted disk "primary" at C:\logs"#));
        assert!(!expression.matches(r#"mounted disk "secondary" at C:\logs"#));
    }

    #[test]
    fn treats_operator_words_as_literal_text_inside_quotes() {
        let expression = parse(r#""AND OR NOT (vds): mounted""#).expect("parse expression");
        assert!(expression.matches("and or not (vds): mounted successfully"));
    }

    #[test]
    fn supports_three_levels_of_safely_nested_filters() {
        let expression = parse(
            r#"(("EXT4-fs (vds): mounted") AND ("ordered data mode")) AND ("AND remains literal")"#,
        )
        .expect("parse nested expression");
        assert!(
            expression
                .matches("EXT4-fs (vds): mounted with ordered data mode; AND remains literal")
        );
    }

    #[test]
    fn chunk_matcher_finds_terms_across_chunks_and_utf8_boundaries() {
        let expression = parse("ERROR AND 中").expect("expression");
        let mut matcher = expression.chunk_matcher();
        matcher.feed_bytes(b"prefix er");
        matcher.feed_bytes(b"ror ");
        matcher.feed_bytes(&[0xe4]);
        matcher.feed_bytes(&[0xb8, 0xad]);
        matcher.finish();

        assert!(matcher.matches(&expression));
    }

    #[test]
    fn chunk_matcher_matches_contextual_sigma_across_chunks() {
        let expression = parse("ΟΣ").expect("expression");
        assert!(expression.matches("ΟΣ"));

        let mut matcher = expression.chunk_matcher();
        matcher.feed_bytes("Ο".as_bytes());
        matcher.feed_bytes("Σ".as_bytes());
        matcher.finish();
        assert!(matcher.matches(&expression));
    }

    #[test]
    fn chunk_matcher_uses_case_folding_for_caseless_variants() {
        let expression = parse("Σ").expect("expression");
        for source in ["Σ", "σ", "ς"] {
            assert!(expression.matches(source), "source: {source:?}");
            let mut matcher = expression.chunk_matcher();
            matcher.feed_bytes(source.as_bytes());
            matcher.finish();
            assert!(matcher.matches(&expression), "source: {source:?}");
        }

        let expression = parse("ß").expect("expression");
        assert!(expression.matches("SS"));
        let mut matcher = expression.chunk_matcher();
        matcher.feed_bytes(b"S");
        matcher.feed_bytes(b"S");
        matcher.finish();
        assert!(matcher.matches(&expression));
    }

    #[test]
    fn chunk_matcher_uses_complete_unicode_sigma_context() {
        for source in ["AΣ\u{0301}B", "ǅΣ"] {
            let expression = parse(source).expect("expression");
            assert!(expression.matches(source));

            for split in 0..=source.len() {
                let mut matcher = expression.chunk_matcher();
                matcher.feed_bytes(&source.as_bytes()[..split]);
                matcher.feed_bytes(&source.as_bytes()[split..]);
                matcher.finish();
                assert!(
                    matcher.matches(&expression),
                    "source: {source:?}, split: {split}"
                );
            }
        }
    }

    #[test]
    fn chunk_matcher_decodes_invalid_bytes_in_one_pass() {
        let expression = parse("\u{fffd}").expect("replacement expression");
        let mut matcher = expression.chunk_matcher();
        matcher.feed_bytes(&[0xff; 8_192]);
        matcher.finish();
        assert!(matcher.matches(&expression));
    }

    #[test]
    fn chunk_matcher_matches_whole_line_for_all_byte_splits() {
        let source = "prefix ΟΣ 中 suffix";
        let expression = parse("ΟΣ").expect("expression");
        let expected = expression.matches(source);

        for split in 0..=source.len() {
            let mut matcher = expression.chunk_matcher();
            matcher.feed_bytes(&source.as_bytes()[..split]);
            matcher.feed_bytes(&source.as_bytes()[split..]);
            matcher.finish();
            assert_eq!(matcher.matches(&expression), expected, "split at {split}");
        }
    }

    #[test]
    fn rejects_excessive_expression_complexity() {
        assert!(parse(&"中".repeat(4_096)).is_ok());
        assert!(parse(&"x".repeat(4_097)).is_err());
        assert!(parse(&"😀".repeat(4_097)).is_err());
        assert!(parse(&format!("{}x", "x OR ".repeat(64))).is_err());
        assert!(parse(&format!("{}x", "NOT ".repeat(33))).is_err());
        assert!(parse(&format!("{}x{}", "(".repeat(33), ")".repeat(33))).is_err());
        assert!(parse(&(0..65).map(|_| "x").collect::<Vec<_>>().join(" OR ")).is_err());
    }

    #[test]
    fn reports_the_offset_of_invalid_syntax() {
        let error = parse("ERROR AND OR WARN").expect_err("invalid expression");
        assert_eq!(error.offset, 10);
        let error = parse("ERROR WARN").expect_err("missing operator");
        assert_eq!(error.offset, 6);
    }
}
