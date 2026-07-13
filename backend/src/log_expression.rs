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
        let normalized = line.to_lowercase();
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
}

pub fn parse(input: &str) -> Result<Expression, ParseError> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Err(ParseError {
            offset: 0,
            message: "expression is required".into(),
        });
    }
    let mut parser = Parser { tokens, cursor: 0 };
    let expression = parser.parse_or()?;
    if let Some(token) = parser.peek() {
        return Err(ParseError {
            offset: token.offset,
            message: "unexpected token".into(),
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
            let start = cursor;
            while cursor < input.len() && !input[cursor..].starts_with('"') {
                cursor += input[cursor..]
                    .chars()
                    .next()
                    .expect("quoted character")
                    .len_utf8();
            }
            if cursor >= input.len() {
                return Err(ParseError {
                    offset,
                    message: "unterminated quoted phrase".into(),
                });
            }
            let phrase = input[start..cursor].trim();
            if phrase.is_empty() {
                return Err(ParseError {
                    offset,
                    message: "quoted phrase cannot be empty".into(),
                });
            }
            tokens.push(Token {
                kind: TokenKind::Term(phrase.to_lowercase()),
                offset,
            });
            cursor += 1;
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
            _ => TokenKind::Term(word.to_lowercase()),
        };
        tokens.push(Token { kind, offset });
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    cursor: usize,
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
            self.take();
            return Ok(Expression::Not(Box::new(self.parse_not()?)));
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
                let expression = self.parse_or()?;
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
    fn reports_the_offset_of_invalid_syntax() {
        let error = parse("ERROR AND OR WARN").expect_err("invalid expression");
        assert_eq!(error.offset, 10);
        let error = parse("ERROR WARN").expect_err("missing operator");
        assert_eq!(error.offset, 6);
    }
}
