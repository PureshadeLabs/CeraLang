//! The Cera source formatter.
//!
//! `format_source` parses a file and prints it again in the canonical style:
//! two-space indentation, lines of at most 100 columns, one blank line between
//! declarations, and parentheses only where precedence needs them. Comments are
//! kept where they stand (above a statement, after it, or between the elements
//! of a list that is broken over several lines). A comment in the middle of an
//! expression cannot be placed safely, so formatting such a file is refused
//! rather than risking a lost or moved comment.
//!
//! Formatting never changes the meaning of a program: the formatted text parses
//! to the same syntax tree (spans aside), and formatting it again changes nothing.

use crate::ast::*;
use crate::lexer::lex;
use crate::parser::parse;
use crate::span::{LineIndex, Span};
use crate::token::TokenKind as T;

pub const WIDTH: usize = 100;
const INDENT: usize = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatError {
    /// One-based line, or 0 when unknown.
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line > 0 {
            write!(f, "{}:{}: {}", self.line, self.column, self.message)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

type R<X> = Result<X, FormatError>;

// ------------------------------------------------------------------------ docs

#[derive(Clone, Debug)]
enum Doc {
    Nil,
    Text(String),
    /// A space when flat, a newline when broken.
    Line,
    /// Nothing when flat, a newline when broken.
    Soft,
    Hard,
    Cat(Vec<Doc>),
    Nest(usize, Box<Doc>),
    Group(Box<Doc>),
    /// A comment printed at the end of the current line.
    Suffix(String),
    IfBreak(Box<Doc>, Box<Doc>),
}

use Doc::*;

fn text(s: impl Into<String>) -> Doc {
    Text(s.into())
}

fn cat(v: Vec<Doc>) -> Doc {
    Cat(v)
}

fn nest(d: Doc) -> Doc {
    Nest(INDENT, Box::new(d))
}

fn group(d: Doc) -> Doc {
    Group(Box::new(d))
}

fn forced(d: &Doc) -> bool {
    match d {
        Hard | Suffix(_) => true,
        Cat(v) => v.iter().any(forced),
        Nest(_, x) | Group(x) => forced(x),
        _ => false,
    }
}

type Item3<'a> = (usize, bool, &'a Doc);

fn fits(mut rem: isize, first: Item3, rest: &[Item3]) -> bool {
    let mut stack: Vec<Item3> = vec![first];
    let mut ri = rest.len();
    loop {
        if rem < 0 {
            return false;
        }
        let (ind, flat, d) = match stack.pop() {
            Some(i) => i,
            None => {
                if ri == 0 {
                    return true;
                }
                ri -= 1;
                rest[ri]
            }
        };
        match d {
            Nil | Suffix(_) => {}
            Text(s) => rem -= s.chars().count() as isize,
            Cat(v) => {
                for x in v.iter().rev() {
                    stack.push((ind, flat, x));
                }
            }
            Nest(n, x) => stack.push((ind + n, flat, x)),
            Line => {
                if flat {
                    rem -= 1;
                } else {
                    return true;
                }
            }
            Soft => {
                if !flat {
                    return true;
                }
            }
            Hard => return true,
            IfBreak(b, f) => stack.push((ind, flat, if flat { f } else { b })),
            Group(x) => stack.push((ind, flat && !forced(x), x)),
        }
    }
}

struct Printer {
    out: String,
    col: usize,
    pending: Option<usize>,
    suffix: Vec<String>,
}

impl Printer {
    fn write(&mut self, s: &str) {
        if let Some(n) = self.pending.take() {
            self.out.push_str(&" ".repeat(n));
        }
        self.out.push_str(s);
        self.col += s.chars().count();
    }

    fn newline(&mut self, indent: usize) {
        while self.out.ends_with(' ') {
            self.out.pop();
        }
        for s in std::mem::take(&mut self.suffix) {
            self.out.push(' ');
            self.out.push_str(&s);
        }
        self.out.push('\n');
        self.pending = Some(indent);
        self.col = indent;
    }
}

fn render(doc: &Doc) -> String {
    let mut p = Printer { out: String::new(), col: 0, pending: None, suffix: vec![] };
    let mut stack: Vec<Item3> = vec![(0, false, doc)];
    while let Some((ind, flat, d)) = stack.pop() {
        match d {
            Nil => {}
            Text(s) => p.write(s),
            Cat(v) => {
                for x in v.iter().rev() {
                    stack.push((ind, flat, x));
                }
            }
            Nest(n, x) => stack.push((ind + n, flat, x)),
            Line => {
                if flat {
                    p.write(" ");
                } else {
                    p.newline(ind);
                }
            }
            Soft => {
                if !flat {
                    p.newline(ind);
                }
            }
            Hard => p.newline(ind),
            Suffix(s) => p.suffix.push(s.clone()),
            IfBreak(b, f) => stack.push((ind, flat, if flat { f } else { b })),
            Group(x) => {
                let go_flat = flat || (!forced(x) && fits(WIDTH as isize - p.col as isize, (ind, true, x), &stack));
                stack.push((ind, go_flat, x));
            }
        }
    }
    while p.out.ends_with(' ') {
        p.out.pop();
    }
    for s in std::mem::take(&mut p.suffix) {
        p.out.push(' ');
        p.out.push_str(&s);
    }
    p.out
}

// -------------------------------------------------------------------- comments

#[derive(Clone, Debug)]
struct Comment {
    start: usize,
    end: usize,
    text: String,
}

fn scan_comments(src: &str) -> Vec<Comment> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut out = vec![];
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() {
                    match b[i] {
                        b'"' => {
                            i += 1;
                            break;
                        }
                        b'\\' => i += 2,
                        _ => i += 1,
                    }
                }
            }
            b'#' => {
                let s = i;
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                let text = src[s..i].trim_end().to_string();
                out.push(Comment { start: s, end: s + text.len(), text });
            }
            _ => i += 1,
        }
    }
    out
}

// ------------------------------------------------------------------- formatter

struct Elem {
    lead: Vec<Comment>,
    doc: Doc,
    trail: Option<Comment>,
}

struct Entry {
    start: usize,
    end: usize,
    doc: Doc,
    comment: bool,
    import: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Sep {
    /// Imports stay together; declarations are separated by a blank line.
    Items,
    /// Multi-line entries are separated by a blank line.
    Methods,
    /// Keep the blank lines the author wrote.
    Plain,
}

struct F<'a> {
    src: &'a str,
    index: LineIndex,
    cs: Vec<Comment>,
    cur: usize,
    /// How many `(`, `[` or `{` the construct being printed is inside; the parser
    /// ignores newlines there, which allows more layouts.
    depth: u32,
}

fn gen_text(generics: &[Ident], bounds: &[Bound]) -> String {
    if generics.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = generics
        .iter()
        .map(|g| match bounds.iter().find(|b| b.param.name == g.name) {
            Some(b) => format!("{}: {}", g.name, b.traits.iter().map(|t| t.name.clone()).collect::<Vec<_>>().join(" + ")),
            None => g.name.clone(),
        })
        .collect();
    format!("<{}>", parts.join(", "))
}

fn type_text(t: &Type) -> String {
    match &t.kind {
        TypeKind::Path { path, args } => {
            let mut s = path.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(".");
            if !args.is_empty() {
                s.push('<');
                s.push_str(&args.iter().map(type_text).collect::<Vec<_>>().join(", "));
                s.push('>');
            }
            s
        }
        TypeKind::Fn { params, ret } => format!("fn({}) -> {}", params.iter().map(type_text).collect::<Vec<_>>().join(", "), type_text(ret)),
        TypeKind::List(e) => format!("[{}]", type_text(e)),
        TypeKind::Tuple(v) => format!("({})", v.iter().map(type_text).collect::<Vec<_>>().join(", ")),
        TypeKind::Ref(e) => format!("ref {}", type_text(e)),
    }
}

fn binop_info(op: BinOp) -> (u8, &'static str) {
    match op {
        BinOp::Or => (1, "or"),
        BinOp::And => (2, "and"),
        BinOp::Eq => (3, "=="),
        BinOp::NotEq => (3, "!="),
        BinOp::Lt => (4, "<"),
        BinOp::LtEq => (4, "<="),
        BinOp::Gt => (4, ">"),
        BinOp::GtEq => (4, ">="),
        BinOp::BitXor => (5, "^"),
        BinOp::BitAnd => (6, "&"),
        BinOp::Shl => (7, "<<"),
        BinOp::Shr => (7, ">>"),
        BinOp::Add => (8, "+"),
        BinOp::Sub => (8, "-"),
        BinOp::Mul => (9, "*"),
        BinOp::Div => (9, "/"),
        BinOp::Rem => (9, "%"),
    }
}

fn prec(e: &Expr) -> u8 {
    match &e.kind {
        ExprKind::Pipe { .. } => 0,
        ExprKind::Binary { op, .. } => binop_info(*op).0,
        ExprKind::Unary { .. } => 10,
        ExprKind::Call { .. } | ExprKind::Field { .. } | ExprKind::Index { .. } | ExprKind::Try(_) => 11,
        _ => 12,
    }
}

fn item_span(i: &Item) -> Span {
    match i {
        Item::Import(x) => x.span,
        Item::Decl(x) => x.span,
        Item::Impl(x) => x.span,
    }
}

impl<'a> F<'a> {
    fn err(&self, pos: usize, msg: impl Into<String>) -> FormatError {
        let (l, c) = self.index.line_col(self.src, pos);
        FormatError { line: l + 1, column: c + 1, message: msg.into() }
    }

    fn take_leading(&mut self, before: usize) -> Vec<Comment> {
        let mut v = vec![];
        while self.cur < self.cs.len() && self.cs[self.cur].start < before {
            v.push(self.cs[self.cur].clone());
            self.cur += 1;
        }
        v
    }

    fn take_trailing(&mut self, after: usize) -> Option<Comment> {
        let c = self.cs.get(self.cur)?;
        if c.start >= after && self.src[after..c.start].chars().all(|ch| matches!(ch, ' ' | '\t' | ',' | '\r')) {
            self.cur += 1;
            return Some(c.clone());
        }
        None
    }

    fn check_inside(&self, span: Span) -> R<()> {
        match self.cs.get(self.cur) {
            Some(c) if c.start < span.end => Err(self.err(c.start, "a comment in the middle of an expression cannot be formatted safely; move it onto its own line")),
            _ => Ok(()),
        }
    }

    fn multi_src(&self, span: Span) -> bool {
        self.src.get(span.start..span.end).is_some_and(|s| s.contains('\n'))
    }

    fn user_blank(&self, a_end: usize, b_start: usize) -> bool {
        a_end <= b_start && self.src[a_end..b_start].matches('\n').count() >= 2
    }

    fn comment_entry(c: &Comment) -> Entry {
        Entry { start: c.start, end: c.end, doc: text(c.text.clone()), comment: true, import: false }
    }

    /// Source text of a literal token inside `span` (keeps `1_000`, raw newlines in strings).
    fn literal(&self, span: Span, fallback: String) -> String {
        let Some(slice) = self.src.get(span.start..span.end) else { return fallback };
        let (toks, _) = lex(slice);
        let real: Vec<_> = toks.iter().filter(|t| !matches!(t.kind, T::LParen | T::RParen | T::Eof | T::Newline)).collect();
        let ok = match real.as_slice() {
            [t] => matches!(t.kind, T::Int(_) | T::Float(_) | T::Str(_)),
            [m, t] => m.kind == T::Minus && matches!(t.kind, T::Int(_) | T::Float(_)),
            _ => false,
        };
        if !ok {
            return fallback;
        }
        let first = real[0].span.start;
        let last = real[real.len() - 1].span.end;
        slice[first..last].to_string()
    }

    // ------------------------------------------------------------ sequences

    fn join_entries(&self, es: Vec<Entry>, sep: Sep) -> Doc {
        let mut parts: Vec<Doc> = vec![];
        let mut prev: Option<Entry> = None;
        for e in es {
            if let Some(p) = &prev {
                let blank = self.user_blank(p.end, e.start);
                let force = match sep {
                    Sep::Plain => blank,
                    Sep::Items | Sep::Methods => {
                        if p.comment && e.comment {
                            blank
                        } else if p.comment {
                            blank
                        } else if sep == Sep::Items && p.import && e.import {
                            blank
                        } else if sep == Sep::Items && p.import != e.import {
                            true
                        } else {
                            blank || forced(&p.doc) || (!e.comment && forced(&e.doc))
                        }
                    }
                };
                parts.push(Hard);
                if force {
                    parts.push(Hard);
                }
            }
            parts.push(match &e.doc {
                d => d.clone(),
            });
            prev = Some(e);
        }
        cat(parts)
    }

    /// Elements separated by commas inside `open`/`close`, broken one per line when
    /// they do not fit or when comments are among them.
    fn seq(&self, open: &str, close: &str, elems: Vec<Elem>, dangling: Vec<Comment>, pad: bool) -> Doc {
        if elems.is_empty() && dangling.is_empty() {
            return text(format!("{open}{close}"));
        }
        let edge = if pad { Line } else { Soft };
        let has_comments = !dangling.is_empty() || elems.iter().any(|e| !e.lead.is_empty() || e.trail.is_some());
        let n = elems.len();
        let mut inner = vec![];
        for (i, e) in elems.into_iter().enumerate() {
            for c in &e.lead {
                inner.push(text(c.text.clone()));
                inner.push(Hard);
            }
            inner.push(e.doc);
            if i + 1 < n {
                inner.push(text(","));
            } else {
                inner.push(IfBreak(Box::new(text(",")), Box::new(Nil)));
            }
            if let Some(t) = e.trail {
                inner.push(Suffix(t.text));
            }
            if i + 1 < n {
                inner.push(if has_comments { Hard } else { Line });
            }
        }
        for c in &dangling {
            inner.push(Hard);
            inner.push(text(c.text.clone()));
        }
        group(cat(vec![text(open), nest(cat(vec![edge.clone(), cat(inner)])), edge, text(close)]))
    }

    // ---------------------------------------------------------------- items

    fn program(&mut self, p: &Program) -> R<Doc> {
        let mut es: Vec<Entry> = vec![];
        for item in &p.items {
            let span = item_span(item);
            if span.start > self.src.len() {
                continue; // generated for a `repr type`
            }
            for c in self.take_leading(span.start) {
                es.push(Self::comment_entry(&c));
            }
            let doc = match item {
                Item::Import(i) => self.import(i),
                Item::Decl(d) => self.decl(d)?,
                Item::Impl(i) => self.impl_def(i)?,
            };
            self.check_inside(span)?;
            let mut end = span.end;
            let mut doc = doc;
            if let Some(t) = self.take_trailing(span.end) {
                end = t.end;
                doc = cat(vec![doc, Suffix(t.text)]);
            }
            es.push(Entry { start: span.start, end, doc, comment: false, import: matches!(item, Item::Import(_)) });
        }
        for c in self.take_leading(usize::MAX) {
            es.push(Self::comment_entry(&c));
        }
        Ok(self.join_entries(es, Sep::Items))
    }

    fn import(&self, i: &Import) -> Doc {
        let path = i.path.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(".");
        match &i.alias {
            Some(a) => text(format!("import {path} as {}", a.name)),
            None => text(format!("import {path}")),
        }
    }

    fn params(&mut self, ps: &[Param], region_end: usize) -> R<Doc> {
        let mut elems = vec![];
        for p in ps {
            let start = p.name.span.start;
            let end = p.ty.as_ref().map_or(p.name.span.end, |t| t.span.end);
            let lead = self.take_leading(start);
            let doc = match &p.ty {
                Some(t) => text(format!("{}: {}", p.name.name, type_text(t))),
                None => text(p.name.name.clone()),
            };
            let trail = self.take_trailing(end);
            elems.push(Elem { lead, doc, trail });
        }
        let dangling = if elems.is_empty() { vec![] } else { self.take_leading(region_end) };
        Ok(self.seq("(", ")", elems, dangling, false))
    }

    fn fn_header(&mut self, f: &FnDef, upto: usize) -> R<Doc> {
        let params_end = f.ret.as_ref().map_or(upto, |r| r.span.start);
        let p = self.params(&f.params, params_end)?;
        let mut v = vec![text(format!("fn{}", gen_text(&f.generics, &f.bounds))), p];
        if let Some(r) = &f.ret {
            v.push(text(format!(" -> {}", type_text(r))));
        }
        Ok(cat(v))
    }

    fn decl(&mut self, d: &Decl) -> R<Doc> {
        let head = format!("{}{}{} :: ", if d.public { "pub " } else { "" }, d.name.name, if d.generics.is_empty() { String::new() } else { format!("<{}>", d.generics.iter().map(|g| g.name.clone()).collect::<Vec<_>>().join(", ")) });
        let end_kw = d.span.end.saturating_sub(3);
        match &d.kind {
            DeclKind::Fn(f) => {
                let Some(body) = &f.body else {
                    return Err(self.err(d.span.start, "intrinsic declarations (functions without a body) cannot be formatted"));
                };
                let body_start = body.stmts.first().map_or(end_kw, |s| s.span().start);
                let header = self.fn_header(f, body_start)?;
                let head = if f.unsafe_ { format!("{head}unsafe ") } else { head };
                self.do_block(cat(vec![text(head), header, text(" ")]), body, end_kw, true)
            }
            DeclKind::Type(TypeDef::Variants(vs)) => {
                let mut es = vec![];
                for v in vs {
                    for c in self.take_leading(v.span.start) {
                        es.push(Self::comment_entry(&c));
                    }
                    let mut s = format!("| {}", v.name.name);
                    if !v.fields.is_empty() {
                        let fs: Vec<String> = v
                            .fields
                            .iter()
                            .map(|f| match &f.name {
                                Some(n) => format!("{}: {}", n.name, type_text(&f.ty)),
                                None => type_text(&f.ty),
                            })
                            .collect();
                        s.push_str(&format!("({})", fs.join(", ")));
                    }
                    let mut doc = text(s);
                    let mut end = v.span.end;
                    if let Some(t) = self.take_trailing(v.span.end) {
                        end = t.end;
                        doc = cat(vec![doc, Suffix(t.text)]);
                    }
                    es.push(Entry { start: v.span.start, end, doc, comment: false, import: false });
                }
                for c in self.take_leading(end_kw) {
                    es.push(Self::comment_entry(&c));
                }
                Ok(cat(vec![text(head), text(if d.repr { "repr type" } else { "type" }), nest(cat(vec![Hard, self.join_entries(es, Sep::Plain)])), Hard, text("end")]))
            }
            DeclKind::Type(TypeDef::Record(fields)) => {
                let mut elems = vec![];
                for f in fields {
                    let lead = self.take_leading(f.name.span.start);
                    let ty = match (&f.array, &f.ty.kind) {
                        (Some(n), TypeKind::List(inner)) => format!("[{}; {n}]", type_text(inner)),
                        _ => type_text(&f.ty),
                    };
                    let doc = text(format!("{}: {}", f.name.name, ty));
                    let trail = self.take_trailing(f.ty.span.end);
                    elems.push(Elem { lead, doc, trail });
                }
                let dangling = if elems.is_empty() { vec![] } else { self.take_leading(d.span.end) };
                Ok(cat(vec![text(head), text(if d.repr { "repr type " } else { "type " }), self.seq("{", "}", elems, dangling, true)]))
            }
            DeclKind::Type(TypeDef::Opaque) => Err(self.err(d.span.start, "opaque types cannot be formatted")),
            DeclKind::Trait(t) => {
                let mut es = vec![];
                for m in &t.methods {
                    for c in self.take_leading(m.span.start) {
                        es.push(Self::comment_entry(&c));
                    }
                    let ret_start = m.ret.as_ref().map_or(m.span.end, |r| r.span.start);
                    let params = self.params(&m.params, ret_start)?;
                    let mut v = vec![text(format!("{} :: fn", m.name.name)), params];
                    if let Some(r) = &m.ret {
                        v.push(text(format!(" -> {}", type_text(r))));
                    }
                    let mut doc = cat(v);
                    let mut end = m.span.end;
                    if let Some(t) = self.take_trailing(m.span.end) {
                        end = t.end;
                        doc = cat(vec![doc, Suffix(t.text)]);
                    }
                    es.push(Entry { start: m.span.start, end, doc, comment: false, import: false });
                }
                for c in self.take_leading(end_kw) {
                    es.push(Self::comment_entry(&c));
                }
                let body = if es.is_empty() { Nil } else { nest(cat(vec![Hard, self.join_entries(es, Sep::Plain)])) };
                Ok(cat(vec![text(head), text("trait"), body, Hard, text("end")]))
            }
            DeclKind::Const(e) => {
                let v = self.expr(e)?;
                Ok(cat(vec![text(head), v]))
            }
        }
    }

    fn impl_def(&mut self, i: &ImplDef) -> R<Doc> {
        let end_kw = i.span.end.saturating_sub(3);
        let head = format!("impl{} {} for {} do", gen_text(&i.generics, &i.bounds), i.trait_name.name, type_text(&i.ty));
        let mut es = vec![];
        for m in &i.methods {
            for c in self.take_leading(m.span.start) {
                es.push(Self::comment_entry(&c));
            }
            let doc = self.decl(m)?;
            self.check_inside(m.span)?;
            let mut end = m.span.end;
            let mut doc = doc;
            if let Some(t) = self.take_trailing(m.span.end) {
                end = t.end;
                doc = cat(vec![doc, Suffix(t.text)]);
            }
            es.push(Entry { start: m.span.start, end, doc, comment: false, import: false });
        }
        for c in self.take_leading(end_kw) {
            es.push(Self::comment_entry(&c));
        }
        let body = if es.is_empty() { Nil } else { nest(cat(vec![Hard, self.join_entries(es, Sep::Methods)])) };
        Ok(cat(vec![text(head), body, Hard, text("end")]))
    }

    // --------------------------------------------------------------- blocks

    /// The statements of a block as lines; `None` when there are none and no comments.
    fn block_body(&mut self, b: &Block, end_bound: usize) -> R<Option<Doc>> {
        let saved = std::mem::replace(&mut self.depth, 0);
        let r = self.block_body_inner(b, end_bound);
        self.depth = saved;
        r
    }

    fn block_body_inner(&mut self, b: &Block, end_bound: usize) -> R<Option<Doc>> {
        let mut es: Vec<Entry> = vec![];
        for s in &b.stmts {
            let span = s.span();
            for c in self.take_leading(span.start) {
                es.push(Self::comment_entry(&c));
            }
            let doc = match s {
                Stmt::Let { name, value, mutable, .. } => {
                    let v = self.expr(value)?;
                    cat(vec![text(format!("{}{} = ", if *mutable { "var " } else { "" }, name.name)), v])
                }
                Stmt::Assign { name, value, .. } => {
                    let v = self.expr(value)?;
                    cat(vec![text(format!("{} := ", name.name)), v])
                }
                Stmt::Defer(e, _) => {
                    let v = self.expr(e)?;
                    cat(vec![text("defer "), v])
                }
                Stmt::Expr(e) => self.expr(e)?,
            };
            self.check_inside(span)?;
            let mut end = span.end;
            let mut doc = doc;
            if let Some(t) = self.take_trailing(span.end) {
                end = t.end;
                doc = cat(vec![doc, Suffix(t.text)]);
            }
            es.push(Entry { start: span.start, end, doc, comment: false, import: false });
        }
        for c in self.take_leading(end_bound) {
            es.push(Self::comment_entry(&c));
        }
        if es.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.join_entries(es, Sep::Plain)))
    }

    /// `header` `do` body `end`. The body goes on one line only when `multi` is false
    /// and it is a single short statement.
    fn do_block(&mut self, header: Doc, block: &Block, end_bound: usize, multi: bool) -> R<Doc> {
        let body = self.block_body(block, end_bound)?;
        let ln = if multi { Hard } else { Line };
        Ok(match body {
            None => group(cat(vec![header, text("do"), ln, text("end")])),
            Some(b) => group(cat(vec![header, text("do"), nest(cat(vec![ln.clone(), b])), ln, text("end")])),
        })
    }

    // ---------------------------------------------------------- expressions

    fn operand(&mut self, e: &Expr, min: u8) -> R<Doc> {
        let d = self.expr(e)?;
        Ok(if prec(e) < min { cat(vec![text("("), d, text(")")]) } else { d })
    }

    /// A receiver or callee: anything that is not self-delimiting gets parentheses.
    fn receiver(&mut self, e: &Expr) -> R<Doc> {
        let d = self.expr(e)?;
        let wrap = prec(e) < 11
            || matches!(e.kind, ExprKind::Lambda { .. } | ExprKind::If { .. } | ExprKind::Match { .. } | ExprKind::Region(_) | ExprKind::Unsafe(_) | ExprKind::With { .. } | ExprKind::Block(_) | ExprKind::While { .. } | ExprKind::For { .. });
        Ok(if wrap { cat(vec![text("("), d, text(")")]) } else { d })
    }

    fn elems_of(&mut self, es: &[Expr]) -> R<Vec<Elem>> {
        let mut out = vec![];
        self.depth += 1;
        for e in es {
            let lead = self.take_leading(e.span.start);
            let doc = self.expr(e);
            let doc = match doc {
                Ok(d) => d,
                Err(err) => {
                    self.depth -= 1;
                    return Err(err);
                }
            };
            let trail = self.take_trailing(e.span.end);
            out.push(Elem { lead, doc, trail });
        }
        self.depth -= 1;
        Ok(out)
    }

    fn expr(&mut self, e: &Expr) -> R<Doc> {
        Ok(match &e.kind {
            ExprKind::Int(s) | ExprKind::Float(s) => text(self.literal(e.span, s.clone())),
            ExprKind::Str(s) => text(self.literal(e.span, escape(s))),
            ExprKind::Bool(b) => text(if *b { "true" } else { "false" }),
            ExprKind::Unit => text("()"),
            ExprKind::Var(n) => text(n.clone()),
            ExprKind::List(items) => {
                let elems = self.elems_of(items)?;
                let dangling = if elems.is_empty() { vec![] } else { self.take_leading(e.span.end.saturating_sub(1)) };
                self.seq("[", "]", elems, dangling, false)
            }
            ExprKind::Cons { items, tail } => {
                let mut v = vec![text("[")];
                self.depth += 1;
                for (i, x) in items.iter().enumerate() {
                    if i > 0 {
                        v.push(text(", "));
                    }
                    v.push(self.expr(x)?);
                }
                v.push(text(" | "));
                v.push(self.expr(tail)?);
                self.depth -= 1;
                v.push(text("]"));
                cat(v)
            }
            ExprKind::Update { base, fields } => {
                let mut v = vec![text("{")];
                self.depth += 1;
                v.push(self.expr(base)?);
                v.push(text(" | "));
                for (i, (n, x)) in fields.iter().enumerate() {
                    if i > 0 {
                        v.push(text(", "));
                    }
                    v.push(text(&format!("{}: ", n.name)));
                    v.push(self.expr(x)?);
                }
                self.depth -= 1;
                v.push(text("}"));
                cat(v)
            }
            ExprKind::Tuple(items) => {
                let elems = self.elems_of(items)?;
                let dangling = if elems.is_empty() { vec![] } else { self.take_leading(e.span.end.saturating_sub(1)) };
                self.seq("(", ")", elems, dangling, false)
            }
            ExprKind::Record { name, fields } => {
                let mut elems = vec![];
                self.depth += 1;
                for (n, v) in fields {
                    let lead = self.take_leading(n.span.start);
                    let d = self.expr(v);
                    let d = match d {
                        Ok(d) => d,
                        Err(err) => {
                            self.depth -= 1;
                            return Err(err);
                        }
                    };
                    let trail = self.take_trailing(v.span.end);
                    elems.push(Elem { lead, doc: cat(vec![text(format!("{}: ", n.name)), d]), trail });
                }
                self.depth -= 1;
                let dangling = if elems.is_empty() { vec![] } else { self.take_leading(e.span.end.saturating_sub(1)) };
                cat(vec![text(format!("{} ", name.name)), self.seq("{", "}", elems, dangling, true)])
            }
            ExprKind::Unary { op, expr } => {
                let o = self.operand(expr, 11)?;
                match op {
                    UnOp::Neg => cat(vec![text("-"), o]),
                    UnOp::Not => cat(vec![text("not "), o]),
                    UnOp::BitNot => cat(vec![text("~"), o]),
                }
            }
            ExprKind::Binary { .. } => {
                // Flatten a left-leaning chain of one precedence level.
                let (p, _) = match &e.kind {
                    ExprKind::Binary { op, .. } => binop_info(*op),
                    _ => unreachable!(),
                };
                let mut rights: Vec<(&'static str, &Expr)> = vec![];
                let mut cur = e;
                loop {
                    match &cur.kind {
                        ExprKind::Binary { op, lhs, rhs } if binop_info(*op).0 == p => {
                            rights.push((binop_info(*op).1, rhs));
                            cur = lhs;
                        }
                        _ => break,
                    }
                }
                rights.reverse();
                let first = self.operand(cur, p)?;
                let mut rest = vec![];
                for (op, r) in rights {
                    let d = self.operand(r, p + 1)?;
                    rest.push(text(format!(" {op}")));
                    rest.push(Line);
                    rest.push(d);
                }
                group(cat(vec![first, nest(cat(rest))]))
            }
            ExprKind::Pipe { .. } => {
                let mut stages: Vec<&Expr> = vec![];
                let mut cur = e;
                while let ExprKind::Pipe { lhs, rhs } = &cur.kind {
                    stages.push(rhs);
                    cur = lhs;
                }
                stages.push(cur);
                stages.reverse();
                let mut multi = false;
                for w in stages.windows(2) {
                    if self.src.get(w[0].span.end..w[1].span.start).is_some_and(|s| s.contains('\n')) {
                        multi = true;
                    }
                }
                let ln = if multi { Hard } else { Line };
                let first = self.operand(stages[0], 1)?;
                let mut docs = vec![];
                for s in &stages[1..] {
                    docs.push(self.operand(s, 1)?);
                }
                // `xs |> f(a, fn(x) do ... end)` stays on one line: only the last stage is tall.
                let hug = !multi && !forced(&first) && docs.last().is_some_and(forced) && docs[..docs.len() - 1].iter().all(|d| !forced(d));
                let mut rest = vec![];
                for d in docs {
                    if hug {
                        rest.push(text(" |> "));
                    } else {
                        rest.push(ln.clone());
                        rest.push(text("|> "));
                    }
                    rest.push(d);
                }
                if hug {
                    cat(vec![first, cat(rest)])
                } else {
                    group(cat(vec![first, nest(cat(rest))]))
                }
            }
            ExprKind::Call { callee, args } => {
                let c = self.receiver(callee)?;
                let elems = self.elems_of(args)?;
                let dangling = if elems.is_empty() { vec![] } else { self.take_leading(e.span.end.saturating_sub(1)) };
                let plain = dangling.is_empty() && elems.iter().all(|x| x.lead.is_empty() && x.trail.is_none());
                let hug = plain
                    && matches!(args.last().map(|a| &a.kind), Some(ExprKind::Lambda { .. }))
                    && elems.last().is_some_and(|l| forced(&l.doc))
                    && elems[..elems.len() - 1].iter().all(|x| !forced(&x.doc));
                if hug {
                    let mut v = vec![c, text("(")];
                    for (i, x) in elems.into_iter().enumerate() {
                        if i > 0 {
                            v.push(text(", "));
                        }
                        v.push(x.doc);
                    }
                    v.push(text(")"));
                    cat(v)
                } else {
                    cat(vec![c, self.seq("(", ")", elems, dangling, false)])
                }
            }
            ExprKind::Field { expr, name } => {
                let r = self.receiver(expr)?;
                cat(vec![r, text(format!(".{}", name.name))])
            }
            ExprKind::Index { expr, index } => {
                let r = self.receiver(expr)?;
                self.depth += 1;
                let i = self.expr(index);
                self.depth -= 1;
                cat(vec![r, text("["), i?, text("]")])
            }
            ExprKind::Try(x) => {
                let r = self.receiver(x)?;
                cat(vec![r, text("?")])
            }
            ExprKind::Lambda { params, body } => {
                let body_start = body.span.start;
                let p = self.params(params, body_start)?;
                match &body.kind {
                    ExprKind::Block(b) => {
                        let multi = self.multi_src(e.span);
                        self.do_block(cat(vec![text("fn"), p, text(" ")]), b, e.span.end.saturating_sub(3), multi)?
                    }
                    _ => {
                        let breakable = self.depth > 0;
                        let b = self.expr(body)?;
                        if breakable {
                            let ln = if self.multi_src(e.span) { Hard } else { Line };
                            group(cat(vec![text("fn"), p, text(" ->"), nest(cat(vec![ln.clone(), b])), ln, text("end")]))
                        } else {
                            cat(vec![text("fn"), p, text(" -> "), b, text(" end")])
                        }
                    }
                }
            }
            ExprKind::If { .. } => self.if_expr(e)?,
            ExprKind::Match { scrutinee, arms } => {
                let s = self.expr(scrutinee)?;
                let mut es = vec![];
                let saved_depth = std::mem::replace(&mut self.depth, 0);
                for a in arms {
                    for c in self.take_leading(a.span.start) {
                        es.push(Self::comment_entry(&c));
                    }
                    let mut v = vec![self.pattern(&a.pat)];
                    if let Some(g) = &a.guard {
                        v.push(text(" if "));
                        v.push(self.expr(g)?);
                    }
                    let body = self.expr(&a.body)?;
                    if forced(&body) {
                        let opens = matches!(a.body.kind, ExprKind::Block(_) | ExprKind::Unsafe(_) | ExprKind::Lambda { .. } | ExprKind::Region(_) | ExprKind::With { .. });
                        if opens {
                            v.push(text(" -> "));
                            v.push(body);
                        } else {
                            v.push(text(" ->"));
                            v.push(nest(cat(vec![Hard, body])));
                        }
                    } else {
                        v.push(text(" ->"));
                        v.push(group(nest(cat(vec![Line, body]))));
                    }
                    self.check_inside(a.span)?;
                    let mut doc = cat(v);
                    let mut end = a.span.end;
                    if let Some(t) = self.take_trailing(a.span.end) {
                        end = t.end;
                        doc = cat(vec![doc, Suffix(t.text)]);
                    }
                    es.push(Entry { start: a.span.start, end, doc, comment: false, import: false });
                }
                for c in self.take_leading(e.span.end.saturating_sub(3)) {
                    es.push(Self::comment_entry(&c));
                }
                self.depth = saved_depth;
                let body = if es.is_empty() { Nil } else { nest(cat(vec![Hard, self.join_entries(es, Sep::Plain)])) };
                cat(vec![text("match "), s, text(" do"), body, Hard, text("end")])
            }
            ExprKind::Region(b) => {
                let multi = self.multi_src(e.span);
                self.do_block(text("region "), b, e.span.end.saturating_sub(3), multi)?
            }
            ExprKind::Unsafe(b) => {
                let multi = self.multi_src(e.span);
                self.do_block(text("unsafe "), b, e.span.end.saturating_sub(3), multi)?
            }
            ExprKind::With { what, arg, body } => {
                self.depth += 1;
                let a = self.expr(arg);
                self.depth -= 1;
                let a = a?;
                let multi = self.multi_src(e.span);
                self.do_block(cat(vec![text(format!("with {}(", what.name)), a, text(") ")]), body, e.span.end.saturating_sub(3), multi)?
            }
            ExprKind::Block(b) => {
                let multi = self.multi_src(e.span);
                self.do_block(Nil, b, e.span.end.saturating_sub(3), multi)?
            }
            ExprKind::While { cond, body } => {
                let multi = self.multi_src(e.span);
                self.depth += 1;
                let c = self.expr(cond);
                self.depth -= 1;
                let c = c?;
                self.do_block(cat(vec![text("while "), c, text(" ")]), body, e.span.end.saturating_sub(3), multi)?
            }
            ExprKind::For { var, iter, body } => {
                let multi = self.multi_src(e.span);
                self.depth += 1;
                let i = self.expr(iter);
                self.depth -= 1;
                let i = i?;
                self.do_block(cat(vec![text(format!("for {} in ", var.name)), i, text(" ")]), body, e.span.end.saturating_sub(3), multi)?
            }
            ExprKind::Break => text("break"),
            ExprKind::Continue => text("continue"),
        })
    }

    /// Position of the keyword `kw` between `from` and `to`, skipping comments.
    fn find_keyword(&self, kw: &str, from: usize, to: usize) -> usize {
        let mut i = from;
        while i < to {
            if let Some(c) = self.cs.iter().find(|c| c.start <= i && i < c.end) {
                i = c.end;
                continue;
            }
            if self.src[i..].starts_with(kw) {
                return i;
            }
            i += 1;
        }
        to
    }

    fn if_expr(&mut self, e: &Expr) -> R<Doc> {
        let multi = self.multi_src(e.span);
        let ln = if multi { Hard } else { Line };
        let end_kw = e.span.end.saturating_sub(3);
        let mut parts: Vec<Doc> = vec![];
        let mut cur = e;
        let mut first = true;
        loop {
            let ExprKind::If { cond, then, els } = &cur.kind else { unreachable!() };
            let c = self.expr(cond)?;
            parts.push(text(if first { "if " } else { "if " }));
            parts.push(c);
            parts.push(text(" do"));
            first = false;
            let then_bound = match els {
                Some(b) => self.find_keyword("else", then.stmts.last().map_or(cur.span.start, |s| s.span().end), b.span.start.max(1)),
                None => end_kw,
            };
            match self.block_body(then, then_bound)? {
                Some(b) => {
                    parts.push(nest(cat(vec![ln.clone(), b])));
                }
                None => {}
            }
            let Some(els) = els else {
                break;
            };
            parts.push(ln.clone());
            parts.push(text("else"));
            // `else if` chains share one `end`.
            let chain = match els.stmts.as_slice() {
                [Stmt::Expr(inner)] if matches!(inner.kind, ExprKind::If { .. }) && inner.span == els.span => Some(inner),
                _ => None,
            };
            if let Some(inner) = chain {
                // Comments between `else` and `if` are rare; they surface at the next slot.
                parts.push(text(" "));
                cur = inner;
                continue;
            }
            match self.block_body(els, end_kw)? {
                Some(b) => parts.push(nest(cat(vec![ln.clone(), b]))),
                None => {}
            }
            break;
        }
        parts.push(ln);
        parts.push(text("end"));
        Ok(group(cat(parts)))
    }

    // ------------------------------------------------------------- patterns

    fn pattern(&self, p: &Pattern) -> Doc {
        text(self.pattern_text(p))
    }

    fn pattern_text(&self, p: &Pattern) -> String {
        match &p.kind {
            PatKind::Wild => "_".into(),
            PatKind::Bind(n) => n.clone(),
            PatKind::Int(s) | PatKind::Float(s) => self.literal(p.span, s.clone()),
            PatKind::Str(s) => self.literal(p.span, escape(s)),
            PatKind::Bool(b) => if *b { "true" } else { "false" }.into(),
            PatKind::Ctor { name, args } => {
                if args.is_empty() {
                    name.name.clone()
                } else {
                    format!("{}({})", name.name, args.iter().map(|a| self.pattern_text(a)).collect::<Vec<_>>().join(", "))
                }
            }
            PatKind::Record { name, fields } => {
                if fields.is_empty() {
                    format!("{} {{}}", name.name)
                } else {
                    let fs: Vec<String> = fields
                        .iter()
                        .map(|(f, sub)| match sub {
                            Some(s) => format!("{}: {}", f.name, self.pattern_text(s)),
                            None => f.name.clone(),
                        })
                        .collect();
                    format!("{} {{ {} }}", name.name, fs.join(", "))
                }
            }
            PatKind::Tuple(v) => format!("({})", v.iter().map(|a| self.pattern_text(a)).collect::<Vec<_>>().join(", ")),
            PatKind::List { items, tail } => {
                let mut s = items.iter().map(|a| self.pattern_text(a)).collect::<Vec<_>>().join(", ");
                if let Some(t) = tail {
                    s.push_str(" | ");
                    s.push_str(&self.pattern_text(t));
                }
                format!("[{s}]")
            }
        }
    }
}

/// A string literal's contents as source text.
fn escape(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            '\x1b' => out.push_str("\\e"),
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Format a source file.
pub fn format_source(src: &str) -> Result<String, FormatError> {
    let parsed = parse(src);
    if let Some(d) = parsed.diagnostics.first() {
        let idx = LineIndex::new(src);
        let (l, c) = idx.line_col(src, d.span.start);
        return Err(FormatError { line: l + 1, column: c + 1, message: format!("syntax error: {}", d.message) });
    }
    let mut f = F { src, index: LineIndex::new(src), cs: scan_comments(src), cur: 0, depth: 0 };
    let doc = f.program(&parsed.program)?;
    if f.cur < f.cs.len() {
        let c = &f.cs[f.cur];
        return Err(f.err(c.start, "this comment could not be placed"));
    }
    let mut out = render(&doc);
    if !out.is_empty() {
        out.push('\n');
    }
    Ok(out)
}

/// The syntax tree of `src` printed without spans, for comparing programs.
pub fn tree_fingerprint(src: &str) -> String {
    let p = parse(src);
    let dbg = format!("{:?}", p.program);
    let mut out = String::new();
    let mut rest = dbg.as_str();
    while let Some(i) = rest.find("Span { start: ") {
        out.push_str(&rest[..i]);
        match rest[i..].find('}') {
            Some(j) => rest = &rest[i + j + 1..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}
