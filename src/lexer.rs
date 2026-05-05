//! Tokenizer for the Courvux template-expression subset.
//!
//! This is intentionally tiny — only what an expression inside `{{ ... }}`,
//! `:attr="..."`, `@event="..."`, and `cv-X="..."` needs. Anything more
//! exotic (destructuring assignment, async, generators, regex literals) is
//! a parse error so we can report it cleanly at build time instead of
//! producing surprising runtime behavior.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Literals
    Number(f64),
    String(String),       // contents only, quotes stripped
    Template(String),     // backtick-quoted, contents only
    True,
    False,
    Null,
    Undefined,

    // Identifiers (also `$event`, `$store`, `$refs`, etc.)
    Ident(String),

    // Punctuation
    Dot,
    Comma,
    Colon,
    Semicolon,
    Question,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Arrow,        // =>

    // Operators
    Assign,       // =
    PlusAssign,   // +=
    MinusAssign,  // -=
    StarAssign,   // *=
    SlashAssign,  // /=
    PercentAssign,// %=
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Bang,         // !
    BangEq,       // !=
    BangEqEq,     // !==
    Eq,           // ==
    EqEq,         // ===
    Lt,
    LtEq,
    Gt,
    GtEq,
    AmpAmp,       // &&
    PipePipe,     // ||
    QuestionQuestion, // ??
    QuestionDot,  // ?.
    PlusPlus,
    MinusMinus,
    Spread,       // ...
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use Token::*;
        match self {
            Number(n)   => write!(f, "number {}", n),
            String(s)   => write!(f, "string {:?}", s),
            Template(s) => write!(f, "template {:?}", s),
            Ident(s)    => write!(f, "ident {}", s),
            True        => write!(f, "true"),
            False       => write!(f, "false"),
            Null        => write!(f, "null"),
            Undefined   => write!(f, "undefined"),
            t           => write!(f, "{:?}", t),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Spanned {
    pub token: Token,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LexError {
    pub message: String,
    pub pos: usize,
}

pub fn tokenize(src: &str) -> Result<Vec<Spanned>, LexError> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < bytes.len() {
        let start = i;
        let c = bytes[i] as char;

        // Whitespace
        if c.is_whitespace() {
            i += 1;
            continue;
        }

        // Numbers — integer or decimal, no exponent (templates almost never need it)
        if c.is_ascii_digit() || (c == '.' && i + 1 < bytes.len() && (bytes[i + 1] as char).is_ascii_digit()) {
            let mut j = i;
            let mut saw_dot = false;
            while j < bytes.len() {
                let ch = bytes[j] as char;
                if ch.is_ascii_digit() {
                    j += 1;
                } else if ch == '.' && !saw_dot && j + 1 < bytes.len() && (bytes[j + 1] as char).is_ascii_digit() {
                    saw_dot = true;
                    j += 1;
                } else {
                    break;
                }
            }
            let lexeme = &src[i..j];
            let n: f64 = lexeme.parse().map_err(|_| LexError {
                message: format!("invalid number literal `{}`", lexeme),
                pos: i,
            })?;
            out.push(Spanned { token: Token::Number(n), start, end: j });
            i = j;
            continue;
        }

        // Identifiers — also keywords (true/false/null/undefined)
        if c == '_' || c == '$' || c.is_ascii_alphabetic() {
            let mut j = i + 1;
            while j < bytes.len() {
                let ch = bytes[j] as char;
                if ch == '_' || ch == '$' || ch.is_ascii_alphanumeric() {
                    j += 1;
                } else {
                    break;
                }
            }
            let lexeme = &src[i..j];
            let token = match lexeme {
                "true"      => Token::True,
                "false"     => Token::False,
                "null"      => Token::Null,
                "undefined" => Token::Undefined,
                _           => Token::Ident(lexeme.to_string()),
            };
            out.push(Spanned { token, start, end: j });
            i = j;
            continue;
        }

        // String literals — single or double quote
        if c == '\'' || c == '"' {
            let quote = c;
            let mut j = i + 1;
            let mut s = std::string::String::new();
            while j < bytes.len() {
                let ch = bytes[j] as char;
                if ch == '\\' && j + 1 < bytes.len() {
                    let esc = bytes[j + 1] as char;
                    match esc {
                        'n'  => s.push('\n'),
                        't'  => s.push('\t'),
                        'r'  => s.push('\r'),
                        '\\' => s.push('\\'),
                        '\'' => s.push('\''),
                        '"'  => s.push('"'),
                        '`'  => s.push('`'),
                        '0'  => s.push('\0'),
                        other => { s.push('\\'); s.push(other); }
                    }
                    j += 2;
                    continue;
                }
                if ch == quote {
                    out.push(Spanned { token: Token::String(s), start, end: j + 1 });
                    i = j + 1;
                    break;
                }
                s.push(ch);
                j += 1;
            }
            if i != j + 1 {
                return Err(LexError { message: format!("unterminated string starting with {}", quote), pos: start });
            }
            continue;
        }

        // Template literal — backtick. We capture the raw contents only;
        // ${...} interpolation is preserved as-is for the codegen to embed
        // verbatim. Templates inside templates and complex escapes are not
        // a goal — keep it simple and explicit.
        if c == '`' {
            let mut j = i + 1;
            let mut s = std::string::String::new();
            let mut closed = false;
            while j < bytes.len() {
                let ch = bytes[j] as char;
                if ch == '\\' && j + 1 < bytes.len() {
                    s.push('\\');
                    s.push(bytes[j + 1] as char);
                    j += 2;
                    continue;
                }
                if ch == '`' {
                    closed = true;
                    j += 1;
                    break;
                }
                s.push(ch);
                j += 1;
            }
            if !closed {
                return Err(LexError { message: "unterminated template literal".into(), pos: start });
            }
            out.push(Spanned { token: Token::Template(s), start, end: j });
            i = j;
            continue;
        }

        // Multi-char punctuation / operators — try the longest match first
        let two  = if i + 1 < bytes.len() { &src[i..i + 2] } else { "" };
        let three = if i + 2 < bytes.len() { &src[i..i + 3] } else { "" };

        if three == "===" {
            out.push(Spanned { token: Token::EqEq, start, end: i + 3 });
            i += 3; continue;
        }
        if three == "!==" {
            out.push(Spanned { token: Token::BangEqEq, start, end: i + 3 });
            i += 3; continue;
        }
        if three == "..." {
            out.push(Spanned { token: Token::Spread, start, end: i + 3 });
            i += 3; continue;
        }

        match two {
            "==" => { out.push(Spanned { token: Token::Eq,            start, end: i + 2 }); i += 2; continue; }
            "!=" => { out.push(Spanned { token: Token::BangEq,        start, end: i + 2 }); i += 2; continue; }
            "<=" => { out.push(Spanned { token: Token::LtEq,          start, end: i + 2 }); i += 2; continue; }
            ">=" => { out.push(Spanned { token: Token::GtEq,          start, end: i + 2 }); i += 2; continue; }
            "&&" => { out.push(Spanned { token: Token::AmpAmp,        start, end: i + 2 }); i += 2; continue; }
            "||" => { out.push(Spanned { token: Token::PipePipe,      start, end: i + 2 }); i += 2; continue; }
            "??" => { out.push(Spanned { token: Token::QuestionQuestion, start, end: i + 2 }); i += 2; continue; }
            "?." => { out.push(Spanned { token: Token::QuestionDot,   start, end: i + 2 }); i += 2; continue; }
            "++" => { out.push(Spanned { token: Token::PlusPlus,      start, end: i + 2 }); i += 2; continue; }
            "--" => { out.push(Spanned { token: Token::MinusMinus,    start, end: i + 2 }); i += 2; continue; }
            "=>" => { out.push(Spanned { token: Token::Arrow,         start, end: i + 2 }); i += 2; continue; }
            "+=" => { out.push(Spanned { token: Token::PlusAssign,    start, end: i + 2 }); i += 2; continue; }
            "-=" => { out.push(Spanned { token: Token::MinusAssign,   start, end: i + 2 }); i += 2; continue; }
            "*=" => { out.push(Spanned { token: Token::StarAssign,    start, end: i + 2 }); i += 2; continue; }
            "/=" => { out.push(Spanned { token: Token::SlashAssign,   start, end: i + 2 }); i += 2; continue; }
            "%=" => { out.push(Spanned { token: Token::PercentAssign, start, end: i + 2 }); i += 2; continue; }
            _ => {}
        }

        let token = match c {
            '.' => Token::Dot,
            ',' => Token::Comma,
            ':' => Token::Colon,
            ';' => Token::Semicolon,
            '?' => Token::Question,
            '(' => Token::LParen,
            ')' => Token::RParen,
            '[' => Token::LBracket,
            ']' => Token::RBracket,
            '{' => Token::LBrace,
            '}' => Token::RBrace,
            '+' => Token::Plus,
            '-' => Token::Minus,
            '*' => Token::Star,
            '/' => Token::Slash,
            '%' => Token::Percent,
            '!' => Token::Bang,
            '=' => Token::Assign,
            '<' => Token::Lt,
            '>' => Token::Gt,
            other => return Err(LexError { message: format!("unexpected character `{}`", other), pos: i }),
        };
        out.push(Spanned { token, start, end: i + 1 });
        i += 1;
    }

    Ok(out)
}
