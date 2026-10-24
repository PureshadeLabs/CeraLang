//! The Cera lexer.
//!
//! Newlines are significant: they end statements. The lexer removes the ones
//! that cannot end a statement, so the parser never sees them:
//!
//! * a newline directly before a line that starts with `|>`
//! * a newline directly after an operator, `,`, `(`, `[`, `{`, `->`, `=`, `::`,
//!   `:`, `.` or `|>`
//!
//! Inside `(...)` and `[...]` the parser additionally ignores all newlines.

use crate::span::{Diagnostic, Span};
use crate::token::{Token, TokenKind};

pub fn lex(src: &str) -> (Vec<Token>, Vec<Diagnostic>) {
    let b = src.as_bytes();
    let mut i = 0;
    let mut toks: Vec<Token> = Vec::new();
    let mut diags = Vec::new();

    let push = |toks: &mut Vec<Token>, kind: TokenKind, start: usize, end: usize| {
        toks.push(Token {
            kind,
            span: Span::new(start, end),
        });
    };

    while i < b.len() {
        let c = b[i];
        match c {
            b' ' | b'\t' | b'\r' => i += 1,
            b'\n' => {
                push(&mut toks, TokenKind::Newline, i, i + 1);
                i += 1;
            }
            b'#' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'"' => {
                let start = i;
                i += 1;
                let mut buf: Vec<u8> = Vec::new();
                let mut closed = false;
                while i < b.len() {
                    match b[i] {
                        b'"' => {
                            i += 1;
                            closed = true;
                            break;
                        }
                        b'\\' => {
                            if i + 1 >= b.len() {
                                i += 1;
                                break;
                            }
                            let esc = b[i + 1];
                            let mut adv = 2;
                            match esc {
                                b'x' => {
                                    let hex = |k: usize| b.get(i + k).and_then(|c| (*c as char).to_digit(16));
                                    match (hex(2), hex(3)) {
                                        (Some(h), Some(l)) if h < 8 => {
                                            buf.push((h * 16 + l) as u8);
                                            adv = 4;
                                        }
                                        (Some(_), Some(_)) => {
                                            diags.push(Diagnostic::new(Span::new(i, i + 4), "`\\x` escapes stop at `\\x7f`; write other characters as `\\u{...}`"));
                                            adv = 4;
                                        }
                                        _ => diags.push(Diagnostic::new(Span::new(i, i + 2), "`\\x` needs two hexadecimal digits")),
                                    }
                                }
                                b'u' => {
                                    let mut j = i + 2;
                                    let mut v: u32 = 0;
                                    let mut digits = 0;
                                    if b.get(j) == Some(&b'{') {
                                        j += 1;
                                        while let Some(d) = b.get(j).and_then(|c| (*c as char).to_digit(16)) {
                                            v = v.saturating_mul(16).saturating_add(d);
                                            digits += 1;
                                            j += 1;
                                        }
                                    }
                                    match (b.get(j), char::from_u32(v)) {
                                        (Some(b'}'), Some(ch)) if (1..=6).contains(&digits) => {
                                            let mut tmp = [0u8; 4];
                                            buf.extend_from_slice(ch.encode_utf8(&mut tmp).as_bytes());
                                            adv = j + 1 - i;
                                        }
                                        _ => diags.push(Diagnostic::new(Span::new(i, (j + 1).min(b.len())), "`\\u{...}` needs 1 to 6 hexadecimal digits naming a character")),
                                    }
                                }
                                b'n' => buf.push(b'\n'),
                                b't' => buf.push(b'\t'),
                                b'r' => buf.push(b'\r'),
                                b'0' => buf.push(0),
                                b'e' => buf.push(27),
                                b'\\' => buf.push(b'\\'),
                                b'"' => buf.push(b'"'),
                                other => {
                                    diags.push(Diagnostic::new(
                                        Span::new(i, i + 2),
                                        format!("unknown escape `\\{}`", other as char),
                                    ));
                                }
                            }
                            i += adv;
                        }
                        other => {
                            buf.push(other);
                            i += 1;
                        }
                    }
                }
                if !closed {
                    diags.push(Diagnostic::new(
                        Span::new(start, start + 1),
                        "unterminated string literal",
                    ));
                }
                let text = String::from_utf8_lossy(&buf).into_owned();
                push(&mut toks, TokenKind::Str(text), start, i);
            }
            b'0'..=b'9' => {
                let start = i;
                let mut is_float = false;
                if c == b'0' && i + 1 < b.len() && (b[i + 1] == b'x' || b[i + 1] == b'X') {
                    i += 2;
                    while i < b.len() && (b[i].is_ascii_hexdigit() || b[i] == b'_') {
                        i += 1;
                    }
                } else {
                    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                        i += 1;
                    }
                    if i + 1 < b.len() && b[i] == b'.' && b[i + 1].is_ascii_digit() {
                        is_float = true;
                        i += 1;
                        while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                            i += 1;
                        }
                    }
                    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                        let mut j = i + 1;
                        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
                            j += 1;
                        }
                        if j < b.len() && b[j].is_ascii_digit() {
                            is_float = true;
                            while j < b.len() && b[j].is_ascii_digit() {
                                j += 1;
                            }
                            i = j;
                        }
                    }
                }
                let text: String = src[start..i].chars().filter(|c| *c != '_').collect();
                let kind = if is_float {
                    TokenKind::Float(text)
                } else {
                    TokenKind::Int(text)
                };
                push(&mut toks, kind, start, i);
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                let word = &src[start..i];
                let kind = match word {
                    "import" => TokenKind::Import,
                    "as" => TokenKind::As,
                    "fn" => TokenKind::Fn,
                    "type" => TokenKind::Type,
                    "do" => TokenKind::Do,
                    "end" => TokenKind::End,
                    "if" => TokenKind::If,
                    "else" => TokenKind::Else,
                    "match" => TokenKind::Match,
                    "region" => TokenKind::Region,
                    "with" => TokenKind::With,
                    "defer" => TokenKind::Defer,
                    "not" => TokenKind::Not,
                    "and" => TokenKind::And,
                    "or" => TokenKind::Or,
                    "true" => TokenKind::True,
                    "false" => TokenKind::False,
                    "ref" => TokenKind::Ref,
                    "while" => TokenKind::While,
                    "for" => TokenKind::For,
                    "in" => TokenKind::In,
                    "break" => TokenKind::Break,
                    "continue" => TokenKind::Continue,
                    _ => TokenKind::Ident(word.to_string()),
                };
                push(&mut toks, kind, start, i);
            }
            _ => {
                let next = b.get(i + 1).copied();
                let (kind, len) = match (c, next) {
                    (b':', Some(b':')) => (Some(TokenKind::ColonColon), 2),
                    (b':', Some(b'=')) => (Some(TokenKind::ColonEq), 2),
                    (b'-', Some(b'>')) => (Some(TokenKind::Arrow), 2),
                    (b'|', Some(b'>')) => (Some(TokenKind::PipeGt), 2),
                    (b'=', Some(b'=')) => (Some(TokenKind::EqEq), 2),
                    (b'!', Some(b'=')) => (Some(TokenKind::NotEq), 2),
                    (b'<', Some(b'=')) => (Some(TokenKind::LtEq), 2),
                    (b'>', Some(b'=')) => (Some(TokenKind::GtEq), 2),
                    (b'(', _) => (Some(TokenKind::LParen), 1),
                    (b')', _) => (Some(TokenKind::RParen), 1),
                    (b'[', _) => (Some(TokenKind::LBracket), 1),
                    (b']', _) => (Some(TokenKind::RBracket), 1),
                    (b'{', _) => (Some(TokenKind::LBrace), 1),
                    (b'}', _) => (Some(TokenKind::RBrace), 1),
                    (b',', _) => (Some(TokenKind::Comma), 1),
                    (b';', _) => (Some(TokenKind::Semi), 1),
                    (b':', _) => (Some(TokenKind::Colon), 1),
                    (b'|', _) => (Some(TokenKind::Bar), 1),
                    (b'=', _) => (Some(TokenKind::Eq), 1),
                    (b'<', _) => (Some(TokenKind::Lt), 1),
                    (b'>', _) => (Some(TokenKind::Gt), 1),
                    (b'+', _) => (Some(TokenKind::Plus), 1),
                    (b'-', _) => (Some(TokenKind::Minus), 1),
                    (b'*', _) => (Some(TokenKind::Star), 1),
                    (b'/', _) => (Some(TokenKind::Slash), 1),
                    (b'%', _) => (Some(TokenKind::Percent), 1),
                    (b'&', _) => (Some(TokenKind::Amp), 1),
                    (b'^', _) => (Some(TokenKind::Caret), 1),
                    (b'~', _) => (Some(TokenKind::Tilde), 1),
                    (b'.', _) => (Some(TokenKind::Dot), 1),
                    (b'?', _) => (Some(TokenKind::Question), 1),
                    _ => (None, 1),
                };
                match kind {
                    Some(k) => {
                        push(&mut toks, k, i, i + len);
                        i += len;
                    }
                    None => {
                        // skip a whole UTF-8 character
                        let ch = src[i..].chars().next().unwrap_or('?');
                        diags.push(Diagnostic::new(
                            Span::new(i, i + ch.len_utf8()),
                            format!("unexpected character `{ch}`"),
                        ));
                        i += ch.len_utf8();
                    }
                }
            }
        }
    }

    push(&mut toks, TokenKind::Eof, src.len(), src.len());
    (filter_newlines(toks), diags)
}

fn continues_line(kind: &TokenKind) -> bool {
    use TokenKind::*;
    matches!(
        kind,
        LParen
            | LBracket
            | LBrace
            | Comma
            | Semi
            | Colon
            | ColonColon
            | ColonEq
            | Arrow
            | PipeGt
            | Eq
            | EqEq
            | NotEq
            | Lt
            | LtEq
            | Gt
            | GtEq
            | Plus
            | Minus
            | Star
            | Slash
            | Percent
            | Amp
            | Caret
            | Dot
            | And
            | Or
            | Not
    )
}

/// Collapse blank lines and drop newlines that continue an expression.
fn filter_newlines(toks: Vec<Token>) -> Vec<Token> {
    // first collapse runs of newlines and leading newlines
    let mut collapsed: Vec<Token> = Vec::with_capacity(toks.len());
    for t in toks {
        if t.kind == TokenKind::Newline {
            match collapsed.last() {
                None => continue,
                Some(prev) if prev.kind == TokenKind::Newline => continue,
                _ => {}
            }
        }
        collapsed.push(t);
    }

    let mut out: Vec<Token> = Vec::with_capacity(collapsed.len());
    for idx in 0..collapsed.len() {
        let t = &collapsed[idx];
        if t.kind == TokenKind::Newline {
            if let Some(prev) = out.last() {
                if continues_line(&prev.kind) {
                    continue;
                }
            }
            if let Some(next) = collapsed.get(idx + 1) {
                if next.kind == TokenKind::PipeGt {
                    continue;
                }
            }
        }
        out.push(t.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use TokenKind::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        let (toks, diags) = lex(src);
        assert!(diags.is_empty(), "{diags:?}");
        toks.into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn basic_tokens() {
        assert_eq!(
            kinds("main :: fn() do end"),
            vec![
                Ident("main".into()),
                ColonColon,
                Fn,
                LParen,
                RParen,
                Do,
                End,
                Eof
            ]
        );
    }

    #[test]
    fn numbers_and_strings() {
        assert_eq!(
            kinds("1_000 0xFF 3.14 2e3 \"a\\nb\""),
            vec![
                Int("1000".into()),
                Int("0xFF".into()),
                Float("3.14".into()),
                Float("2e3".into()),
                Str("a\nb".into()),
                Eof
            ]
        );
    }

    #[test]
    fn method_call_is_not_a_float() {
        assert_eq!(
            kinds("1.foo"),
            vec![Int("1".into()), Dot, Ident("foo".into()), Eof]
        );
    }

    #[test]
    fn comments_are_skipped() {
        assert_eq!(
            kinds("x # comment\ny"),
            vec![Ident("x".into()), Newline, Ident("y".into()), Eof]
        );
    }

    #[test]
    fn pipe_lines_continue() {
        let k = kinds("a\n  |> b\n  |> c\nd");
        assert_eq!(
            k,
            vec![
                Ident("a".into()),
                PipeGt,
                Ident("b".into()),
                PipeGt,
                Ident("c".into()),
                Newline,
                Ident("d".into()),
                Eof
            ]
        );
    }

    #[test]
    fn trailing_operator_continues() {
        let k = kinds("a +\n b");
        assert_eq!(k, vec![Ident("a".into()), Plus, Ident("b".into()), Eof]);
    }

    #[test]
    fn blank_lines_collapse() {
        let k = kinds("\n\na\n\n\nb\n");
        assert_eq!(
            k,
            vec![Ident("a".into()), Newline, Ident("b".into()), Newline, Eof]
        );
    }

    #[test]
    fn reports_bad_characters() {
        let (_, diags) = lex("a @ b");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains('@'));
    }

    #[test]
    fn reports_unterminated_string() {
        let (_, diags) = lex("\"abc");
        assert_eq!(diags.len(), 1);
    }
}
