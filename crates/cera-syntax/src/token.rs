//! Token definitions.

use crate::span::Span;

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    // literals and names
    Ident(String),
    Int(String),
    Float(String),
    Str(String),

    // keywords
    Import,
    As,
    Fn,
    Type,
    Do,
    End,
    If,
    Else,
    Match,
    Region,
    With,
    Defer,
    Not,
    And,
    Or,
    True,
    False,
    Ref,
    While,
    For,
    In,
    Break,
    Continue,

    // delimiters
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,

    // punctuation and operators
    Comma,
    Semi,
    Colon,
    ColonColon,
    ColonEq, // :=
    Arrow,  // ->
    PipeGt, // |>
    Bar,    // |
    Eq,     // =
    EqEq,   // ==
    NotEq,  // !=
    Lt,
    LtEq,
    Gt,
    GtEq,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Amp,
    Caret,
    Tilde,
    Dot,
    Question,

    // structure
    Newline,
    Eof,
}

impl TokenKind {
    /// Human-readable description used in diagnostics.
    pub fn describe(&self) -> String {
        use TokenKind::*;
        match self {
            Ident(s) => format!("identifier `{s}`"),
            Int(s) | Float(s) => format!("number `{s}`"),
            Str(_) => "string".to_string(),
            Newline => "end of line".to_string(),
            Eof => "end of file".to_string(),
            other => format!("`{}`", other.text()),
        }
    }

    /// Source text of payload-free tokens.
    pub fn text(&self) -> &'static str {
        use TokenKind::*;
        match self {
            Import => "import",
            As => "as",
            Fn => "fn",
            Type => "type",
            Do => "do",
            End => "end",
            If => "if",
            Else => "else",
            Match => "match",
            Region => "region",
            With => "with",
            Defer => "defer",
            Not => "not",
            And => "and",
            Or => "or",
            True => "true",
            False => "false",
            Ref => "ref",
            While => "while",
            For => "for",
            In => "in",
            Break => "break",
            Continue => "continue",
            LParen => "(",
            RParen => ")",
            LBracket => "[",
            RBracket => "]",
            LBrace => "{",
            RBrace => "}",
            Comma => ",",
            Semi => ";",
            Colon => ":",
            ColonColon => "::",
            ColonEq => ":=",
            Arrow => "->",
            PipeGt => "|>",
            Bar => "|",
            Eq => "=",
            EqEq => "==",
            NotEq => "!=",
            Lt => "<",
            LtEq => "<=",
            Gt => ">",
            GtEq => ">=",
            Plus => "+",
            Minus => "-",
            Star => "*",
            Slash => "/",
            Percent => "%",
            Amp => "&",
            Caret => "^",
            Tilde => "~",
            Dot => ".",
            Question => "?",
            Ident(_) | Int(_) | Float(_) | Str(_) | Newline | Eof => "",
        }
    }

    /// Whether this token is a keyword.
    pub fn is_keyword(&self) -> bool {
        use TokenKind::*;
        matches!(
            self,
            Import
                | As
                | Fn
                | Type
                | Do
                | End
                | If
                | Else
                | Match
                | Region
                | With
                | Defer
                | Not
                | And
                | Or
                | True
                | False
                | Ref
                | While
                | For
                | In
                | Break
                | Continue
        )
    }
}

/// All reserved words, in source form.
pub const KEYWORDS: &[&str] = &[
    "import", "as", "fn", "type", "do", "end", "if", "else", "match", "region", "with", "defer",
    "not", "and", "or", "true", "false", "ref", "while", "for", "in", "break", "continue",
];

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}
