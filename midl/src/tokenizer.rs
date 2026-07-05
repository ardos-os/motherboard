use crate::ast::Diagnostic;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Ident(String),
    Doc(String),
    Arrow,
    Symbol(char),
    Eof,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub line: usize,
    pub column: usize,
}

pub fn tokenize(source: &str) -> Result<Vec<Token>, Diagnostic> {
    Tokenizer::new(source).tokenize()
}

struct Tokenizer<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    line: usize,
    column: usize,
}

impl<'a> Tokenizer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            chars: source.chars().peekable(),
            line: 1,
            column: 1,
        }
    }

    fn tokenize(mut self) -> Result<Vec<Token>, Diagnostic> {
        let mut tokens = Vec::new();
        while let Some(ch) = self.peek() {
            match ch {
                ' ' | '\t' | '\r' | '\n' => {
                    self.bump();
                }
                '/' => {
                    let line = self.line;
                    let column = self.column;
                    self.bump();
                    if self.peek() != Some('/') {
                        return Err(Diagnostic::new("expected comment after `/`", line, column));
                    }
                    self.bump();
                    if self.peek() == Some('/') {
                        self.bump();
                        let doc = self.read_until_newline().trim_start().to_string();
                        tokens.push(Token {
                            kind: TokenKind::Doc(doc),
                            line,
                            column,
                        });
                    } else {
                        self.read_until_newline();
                    }
                }
                '-' => {
                    let line = self.line;
                    let column = self.column;
                    self.bump();
                    if self.peek() != Some('>') {
                        return Err(Diagnostic::new("expected `>` after `-`", line, column));
                    }
                    self.bump();
                    tokens.push(Token {
                        kind: TokenKind::Arrow,
                        line,
                        column,
                    });
                }
                '{' | '}' | '(' | ')' | '[' | ']' | '<' | '>' | ':' | ';' | ',' | '=' | '?' => {
                    let line = self.line;
                    let column = self.column;
                    self.bump();
                    tokens.push(Token {
                        kind: TokenKind::Symbol(ch),
                        line,
                        column,
                    });
                }
                _ if is_ident_start(ch) => {
                    let line = self.line;
                    let column = self.column;
                    let mut ident = String::new();
                    while let Some(ch) = self.peek() {
                        if is_ident_continue(ch) {
                            ident.push(ch);
                            self.bump();
                        } else {
                            break;
                        }
                    }
                    tokens.push(Token {
                        kind: TokenKind::Ident(ident),
                        line,
                        column,
                    });
                }
                _ => {
                    return Err(Diagnostic::new(
                        format!("unexpected character `{ch}`"),
                        self.line,
                        self.column,
                    ));
                }
            }
        }

        tokens.push(Token {
            kind: TokenKind::Eof,
            line: self.line,
            column: self.column,
        });
        Ok(tokens)
    }

    fn peek(&mut self) -> Option<char> {
        self.chars.peek().copied()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.chars.next()?;
        if ch == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(ch)
    }

    fn read_until_newline(&mut self) -> String {
        let mut out = String::new();
        while let Some(ch) = self.peek() {
            if ch == '\n' {
                break;
            }
            out.push(ch);
            self.bump();
        }
        out
    }
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}
