//! Recursive-descent parser for Cera.
//!
//! Newlines are statement terminators. The lexer already removes newlines that
//! continue an expression; on top of that the parser ignores *all* newlines
//! while it is inside `(...)`, `[...]`, `{...}` or `<...>`, and switches them
//! back on inside `do ... end` blocks.
//!
//! Errors are collected per top-level item: when an item fails to parse the
//! parser records a diagnostic, skips to the next item that starts in the first
//! column, and keeps going. That keeps editors useful while a file is broken.

use crate::ast::*;
use crate::lexer::lex;
use crate::span::{Diagnostic, Span};
use crate::token::{Token, TokenKind as T};

type PResult<X> = Result<X, Diagnostic>;

/// The result of parsing a file: a (possibly partial) tree and all problems found.
#[derive(Clone, Debug)]
pub struct Parse {
    pub program: Program,
    pub diagnostics: Vec<Diagnostic>,
}

impl Parse {
    pub fn is_ok(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

/// Switches for parsing special kinds of source.
#[derive(Clone, Copy, Debug, Default)]
pub struct ParseOptions {
    /// Allow *intrinsic* declarations: functions without a body
    /// (`println :: fn<T>(value: T)`) and opaque types (`Allocator :: type`).
    /// Only the compiler's own standard-library stubs are parsed this way;
    /// ordinary programs must give every function a body.
    pub intrinsics: bool,
}

/// Parse a complete source file.
pub fn parse(src: &str) -> Parse {
    parse_with(src, ParseOptions::default())
}

/// Parse a complete source file with explicit options.
pub fn parse_with(src: &str, options: ParseOptions) -> Parse {
    let (toks, mut diags) = lex(src);
    let mut p = Parser {
        src,
        toks,
        pos: 0,
        nl_depth: 0,
        last_end: 0,
        diags: Vec::new(),
        intrinsics: options.intrinsics,
        layouts: Default::default(),
        synth: src.len() + 1,
    };
    let program = p.program();
    diags.extend(p.diags);
    diags.sort_by_key(|d| (d.span.start, d.span.end));
    Parse {
        program,
        diagnostics: diags,
    }
}

struct Parser<'a> {
    src: &'a str,
    toks: Vec<Token>,
    pos: usize,
    nl_depth: u32,
    last_end: usize,
    diags: Vec<Diagnostic>,
    intrinsics: bool,
    layouts: crate::repr::Layouts,
    /// Next unused offset for synthetic spans (past the end of the source).
    synth: usize,
}

impl<'a> Parser<'a> {
    // ---------------------------------------------------------------- cursor

    fn cur(&mut self) -> &Token {
        if self.nl_depth > 0 {
            while self.toks[self.pos].kind == T::Newline {
                self.pos += 1;
            }
        }
        &self.toks[self.pos]
    }

    fn kind(&mut self) -> T {
        self.cur().kind.clone()
    }

    fn at(&mut self, k: &T) -> bool {
        self.cur().kind == *k
    }

    fn span(&mut self) -> Span {
        self.cur().span
    }

    /// Kind of the token `n` places after the current one, ignoring newline rules.
    fn peek_raw(&mut self, n: usize) -> T {
        self.cur();
        self.toks
            .get(self.pos + n)
            .map(|t| t.kind.clone())
            .unwrap_or(T::Eof)
    }

    fn bump(&mut self) -> Token {
        let t = self.cur().clone();
        if t.kind != T::Eof {
            self.pos += 1;
        }
        if t.kind != T::Newline && t.kind != T::Eof {
            self.last_end = t.span.end;
        }
        t
    }

    fn eat(&mut self, k: &T) -> bool {
        if self.at(k) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn skip_newlines(&mut self) {
        while self.toks[self.pos].kind == T::Newline {
            self.pos += 1;
        }
    }

    fn unexpected(&mut self, expected: &str) -> Diagnostic {
        let t = self.cur().clone();
        let found = t.kind.describe();
        let mut d = Diagnostic::new(t.span, format!("expected {}, found {}", expected, found))
            .with_label(format!("expected {expected}"));
        if found == "`:`" && expected.contains("end of line") {
            d = d.with_help("there is no typed `let`; write `name = value` and let the type be inferred");
        } else if found.contains("end of file") && expected.contains("`end`") {
            d = d.with_help("a block opened with `do` needs a matching `end`");
        } else if expected.contains("`end`") && found != "`end`" {
            d = d.with_help("every `do` block must be closed with `end`; a `match` arm with several statements needs its own `do ... end`");
        }
        d
    }

    fn expect(&mut self, k: &T) -> PResult<Token> {
        if self.at(k) {
            Ok(self.bump())
        } else {
            Err(self.unexpected(&format!("`{}`", k.text())))
        }
    }

    fn ident(&mut self, what: &str) -> PResult<Ident> {
        match self.kind() {
            T::Ident(name) => {
                let t = self.bump();
                Ok(Ident { name, span: t.span })
            }
            _ => Err(self.unexpected(what)),
        }
    }

    /// An identifier, optionally qualified with module aliases: `shapes.Circle`.
    /// A qualified name is stored as one `Ident` whose name contains dots.
    fn qual_ident(&mut self, what: &str) -> PResult<Ident> {
        let mut id = self.ident(what)?;
        while self.at(&T::Dot) && matches!(self.peek_raw(1), T::Ident(_)) {
            self.bump();
            let next = self.ident(what)?;
            id = Ident {
                name: format!("{}.{}", id.name, next.name),
                span: id.span.to(next.span),
            };
        }
        Ok(id)
    }

    fn since(&self, start: usize) -> Span {
        Span::new(start, self.last_end.max(start))
    }

    /// `open item, item, ... close` with newlines ignored inside.
    fn delimited<X>(
        &mut self,
        open: &T,
        close: &T,
        mut f: impl FnMut(&mut Self) -> PResult<X>,
    ) -> PResult<Vec<X>> {
        self.expect(open)?;
        self.nl_depth += 1;
        let mut items = Vec::new();
        while !self.at(close) {
            items.push(f(self)?);
            if !self.eat(&T::Comma) {
                break;
            }
        }
        self.expect(close)?;
        self.nl_depth -= 1;
        Ok(items)
    }

    // ------------------------------------------------------------- recovery

    fn at_item_start_raw(&self) -> bool {
        let t = &self.toks[self.pos];
        let at_line_start = t.span.start == 0 || self.src.as_bytes()[t.span.start - 1] == b'\n';
        if !at_line_start {
            return false;
        }
        match &t.kind {
            T::Import => true,
            T::Ident(s) => {
                let next = self.toks.get(self.pos + 1).map(|n| &n.kind);
                let after = self.toks.get(self.pos + 2).map(|n| &n.kind);
                matches!(next, Some(T::ColonColon) | Some(T::Lt))
                    || (s == "impl" && matches!(next, Some(T::Ident(_))))
                    || (s == "pub" && matches!(next, Some(T::Ident(_))) && matches!(after, Some(T::ColonColon) | Some(T::Lt)))
            }
            _ => false,
        }
    }

    fn recover(&mut self, start: usize) {
        self.nl_depth = 0;
        if self.pos == start && self.toks[self.pos].kind != T::Eof {
            self.pos += 1;
        }
        while self.toks[self.pos].kind != T::Eof && !self.at_item_start_raw() {
            self.pos += 1;
        }
    }

    // ---------------------------------------------------------------- items

    fn program(&mut self) -> Program {
        let mut items = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&T::Eof) {
                break;
            }
            let start = self.pos;
            match self.item() {
                Ok(item) => {
                    // The lexer drops a newline after `>`, so a bodiless intrinsic
                    // signature ending in `Option<T>` runs into the next line.
                    let bodiless = matches!(
                        &item,
                        Item::Decl(Decl {
                            kind: DeclKind::Fn(FnDef { body: None, .. }),
                            ..
                        })
                    );
                    let generated = match &item {
                        Item::Decl(d) if d.repr => self.generate_repr(d),
                        _ => Vec::new(),
                    };
                    items.push(item);
                    items.extend(generated);
                    if !(self.at(&T::Newline) || self.at(&T::Eof) || (self.intrinsics && bodiless)) {
                        let d = self.unexpected("end of line after this declaration");
                        self.diags.push(d);
                        self.recover(start);
                    }
                }
                Err(d) => {
                    self.diags.push(d);
                    self.recover(start);
                }
            }
        }
        Program { items }
    }

    fn item(&mut self) -> PResult<Item> {
        match self.kind() {
            T::Import => self.import().map(Item::Import),
            T::Ident(ref s)
                if s == "impl" && matches!(self.peek_raw(1), T::Ident(_) | T::Lt) =>
            {
                self.impl_def().map(Item::Impl)
            }
            T::Ident(_) => self.decl().map(Item::Decl),
            _ => Err(self.unexpected("`import` or a declaration like `name :: ...`")),
        }
    }

    fn import(&mut self) -> PResult<Import> {
        let start = self.expect(&T::Import)?.span.start;
        let mut path = vec![self.ident("a module name")?];
        while self.eat(&T::Dot) {
            path.push(self.ident("a module name")?);
        }
        let alias = if self.eat(&T::As) {
            Some(self.ident("an alias")?)
        } else {
            None
        };
        Ok(Import {
            path,
            alias,
            span: self.since(start),
        })
    }

    fn generics(&mut self) -> PResult<Vec<Ident>> {
        self.delimited(&T::Lt, &T::Gt, |p| p.ident("a type parameter name"))
    }

    /// `<A, B: Ord, C: Show + Ord>`
    fn generics_bounded(&mut self) -> PResult<(Vec<Ident>, Vec<Bound>)> {
        let items = self.delimited(&T::Lt, &T::Gt, |p| {
            let name = p.ident("a type parameter name")?;
            let mut traits = Vec::new();
            if p.eat(&T::Colon) {
                traits.push(p.qual_ident("a trait name")?);
                while p.eat(&T::Plus) {
                    traits.push(p.qual_ident("a trait name")?);
                }
            }
            Ok((name, traits))
        })?;
        let mut generics = Vec::new();
        let mut bounds = Vec::new();
        for (name, traits) in items {
            if !traits.is_empty() {
                bounds.push(Bound {
                    param: name.clone(),
                    traits,
                });
            }
            generics.push(name);
        }
        Ok((generics, bounds))
    }

    fn trait_def(&mut self) -> PResult<TraitDef> {
        self.bump(); // `trait`
        let mut methods = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&T::End) {
                break;
            }
            let start = self.span().start;
            let name = self.ident("a method name or `end`")?;
            self.expect(&T::ColonColon)?;
            self.expect(&T::Fn)?;
            let params = self.params(false)?;
            let ret = if self.eat(&T::Arrow) {
                Some(self.ty()?)
            } else {
                None
            };
            // No newline check: a signature ending in `>` swallows its newline.
            methods.push(TraitMethod {
                name,
                params,
                ret,
                span: self.since(start),
            });
        }
        self.expect(&T::End)?;
        Ok(TraitDef { methods })
    }

    fn impl_def(&mut self) -> PResult<ImplDef> {
        let start = self.span().start;
        self.bump(); // `impl`
        let (generics, bounds) = if self.at(&T::Lt) {
            self.generics_bounded()?
        } else {
            (Vec::new(), Vec::new())
        };
        let trait_name = self.qual_ident("a trait name")?;
        if self.at(&T::For) {
            self.bump();
        } else {
            return Err(self.unexpected("`for`"));
        }
        let ty = self.ty()?;
        self.expect(&T::Do)?;
        let mut methods = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&T::End) {
                break;
            }
            let d = self.decl()?;
            if d.public {
                return Err(Diagnostic::new(d.name.span, "methods in an `impl` cannot be marked `pub`")
                    .with_label("remove `pub`")
                    .with_help("a method is as visible as the trait and the type it implements"));
            }
            if !matches!(d.kind, DeclKind::Fn(_)) {
                return Err(Diagnostic::new(
                    d.span,
                    "only functions can appear inside an `impl`",
                ));
            }
            methods.push(d);
        }
        self.expect(&T::End)?;
        Ok(ImplDef {
            generics,
            bounds,
            trait_name,
            ty,
            methods,
            span: self.since(start),
        })
    }

    fn decl(&mut self) -> PResult<Decl> {
        let start = self.span().start;
        let public = matches!(self.kind(), T::Ident(ref s) if s == "pub") && matches!(self.peek_raw(1), T::Ident(_));
        if public {
            self.bump();
        }
        let name = self.ident("a declaration name")?;
        let generics = if self.at(&T::Lt) {
            self.generics()?
        } else {
            Vec::new()
        };
        self.expect(&T::ColonColon)?;
        let unsafe_fn = matches!(self.kind(), T::Ident(ref s) if s == "unsafe") && self.peek_raw(1) == T::Fn;
        if unsafe_fn {
            self.bump();
        }
        let repr = matches!(self.kind(), T::Ident(ref s) if s == "repr") && self.peek_raw(1) == T::Type;
        if repr {
            self.bump();
        }
        let kind = match self.kind() {
            T::Fn => DeclKind::Fn(self.fn_def(unsafe_fn)?),
            T::Type => DeclKind::Type(self.type_def(repr)?),
            T::Ident(ref s) if s == "trait" => DeclKind::Trait(self.trait_def()?),
            _ => DeclKind::Const(self.expr()?),
        };
        Ok(Decl {
            public,
            name,
            generics,
            kind,
            repr,
            span: self.since(start),
        })
    }

    /// The accessor functions of a `repr type`, parsed from generated source with synthetic spans.
    fn generate_repr(&mut self, d: &Decl) -> Vec<Item> {
        let DeclKind::Type(TypeDef::Record(fields)) = &d.kind else {
            self.diags.push(Diagnostic::new(d.name.span, "a `repr type` must be a record"));
            return Vec::new();
        };
        let src = match crate::repr::generate(d, fields, &mut self.layouts) {
            Ok(s) => s,
            Err(m) => {
                self.diags.push(Diagnostic::new(d.name.span, m));
                return Vec::new();
            }
        };
        let src = if self.layouts.len() == 1 {
            format!("import std.mem as repr_mem\nimport std.bits as repr_bits\n{src}")
        } else {
            src
        };
        let (mut toks, _) = lex(&src);
        // Each token gets its own span past the end of the source: the checker and the native
        // backend key types by span, so generated expressions must not share one.
        let first = self.synth;
        for t in &mut toks {
            t.span = Span::new(self.synth, self.synth + 1);
            self.synth += 2;
        }
        let mut p = Parser { src: self.src, toks, pos: 0, nl_depth: 0, last_end: first, diags: Vec::new(), intrinsics: false, layouts: Default::default(), synth: 0 };
        let prog = p.program();
        self.diags.extend(p.diags);
        prog.items
    }

    fn params(&mut self, require_types: bool) -> PResult<Vec<Param>> {
        self.delimited(&T::LParen, &T::RParen, |p| {
            let name = p.ident("a parameter name")?;
            let ty = if p.eat(&T::Colon) {
                Some(p.ty()?)
            } else if require_types {
                return Err(p.unexpected("`:` and a type"));
            } else {
                None
            };
            Ok(Param { name, ty })
        })
    }

    fn fn_def(&mut self, unsafe_: bool) -> PResult<FnDef> {
        self.expect(&T::Fn)?;
        let (generics, bounds) = if self.at(&T::Lt) {
            self.generics_bounded()?
        } else {
            (Vec::new(), Vec::new())
        };
        let params = self.params(true)?;
        let ret = if self.eat(&T::Arrow) {
            Some(self.ty()?)
        } else {
            None
        };
        if self.intrinsics && !self.at(&T::Do) {
            return Ok(FnDef {
                unsafe_,
                generics,
                bounds,
                params,
                ret,
                body: None,
            });
        }
        self.expect(&T::Do)?;
        let body = self.block()?;
        self.expect(&T::End)?;
        Ok(FnDef {
            unsafe_,
            generics,
            bounds,
            params,
            ret,
            body: Some(body),
        })
    }

    fn type_def(&mut self, repr: bool) -> PResult<TypeDef> {
        self.expect(&T::Type)?;
        if self.intrinsics && matches!(self.toks[self.pos].kind, T::Newline | T::Eof) {
            // Opaque unless a variant list or record body starts on a later line.
            let next = self.toks[self.pos..]
                .iter()
                .find(|t| t.kind != T::Newline)
                .map(|t| t.kind.clone());
            if !matches!(next, Some(T::Bar | T::LBrace)) {
                return Ok(TypeDef::Opaque);
            }
        }
        self.skip_newlines();
        if self.at(&T::LBrace) {
            let fields = self.delimited(&T::LBrace, &T::RBrace, |p| {
                let name = p.ident("a field name")?;
                p.expect(&T::Colon)?;
                if repr && p.at(&T::LBracket) {
                    let lb = p.bump().span.start;
                    let elem = p.ty()?;
                    let semi = p.eat(&T::Semi);
                    let (n, ok) = match p.kind() {
                        T::Int(v) => (v.replace('_', "").parse::<u64>().unwrap_or(0), semi),
                        _ => (0, false),
                    };
                    if !ok {
                        return Err(p.unexpected("`;` and an element count in `[T; N]`"));
                    }
                    p.bump();
                    let rb = p.expect(&T::RBracket)?.span.end;
                    let span = Span::new(lb, rb);
                    return Ok(Field { name, ty: Type { kind: TypeKind::List(Box::new(elem)), span }, array: Some(n) });
                }
                let ty = p.ty()?;
                Ok(Field { name, ty, array: None })
            })?;
            return Ok(TypeDef::Record(fields));
        }
        let mut variants = Vec::new();
        loop {
            self.skip_newlines();
            if !self.at(&T::Bar) {
                break;
            }
            let start = self.bump().span.start;
            let name = self.ident("a variant name")?;
            let fields = if self.at(&T::LParen) {
                self.delimited(&T::LParen, &T::RParen, |p| p.variant_field())?
            } else {
                Vec::new()
            };
            variants.push(Variant {
                name,
                fields,
                span: self.since(start),
            });
        }
        if variants.is_empty() {
            return Err(self.unexpected("`|` to start a variant, or `{` to start a record"));
        }
        self.expect(&T::End)?;
        Ok(TypeDef::Variants(variants))
    }

    fn variant_field(&mut self) -> PResult<VariantField> {
        if matches!(self.kind(), T::Ident(_)) && self.peek_raw(1) == T::Colon {
            let name = self.ident("a field name")?;
            self.expect(&T::Colon)?;
            let ty = self.ty()?;
            Ok(VariantField {
                name: Some(name),
                ty,
            })
        } else {
            let ty = self.ty()?;
            Ok(VariantField { name: None, ty })
        }
    }

    // ---------------------------------------------------------------- types

    fn ty(&mut self) -> PResult<Type> {
        let start = self.span().start;
        match self.kind() {
            T::Ident(_) => {
                let mut path = vec![self.ident("a type name")?];
                while self.eat(&T::Dot) {
                    path.push(self.ident("a type name")?);
                }
                let args = if self.at(&T::Lt) {
                    self.delimited(&T::Lt, &T::Gt, |p| p.ty())?
                } else {
                    Vec::new()
                };
                Ok(Type {
                    kind: TypeKind::Path { path, args },
                    span: self.since(start),
                })
            }
            T::Fn => {
                self.bump();
                let params = self.delimited(&T::LParen, &T::RParen, |p| p.ty())?;
                self.expect(&T::Arrow)?;
                let ret = self.ty()?;
                Ok(Type {
                    kind: TypeKind::Fn {
                        params,
                        ret: Box::new(ret),
                    },
                    span: self.since(start),
                })
            }
            T::LBracket => {
                self.bump();
                let inner = self.ty()?;
                self.expect(&T::RBracket)?;
                Ok(Type {
                    kind: TypeKind::List(Box::new(inner)),
                    span: self.since(start),
                })
            }
            T::LParen => {
                let mut items = self.delimited(&T::LParen, &T::RParen, |p| p.ty())?;
                let span = self.since(start);
                if items.len() == 1 {
                    let mut t = items.pop().unwrap();
                    t.span = span;
                    Ok(t)
                } else {
                    Ok(Type {
                        kind: TypeKind::Tuple(items),
                        span,
                    })
                }
            }
            T::Ref => {
                self.bump();
                let inner = self.ty()?;
                Ok(Type {
                    kind: TypeKind::Ref(Box::new(inner)),
                    span: self.since(start),
                })
            }
            _ => Err(self.unexpected("a type")),
        }
    }

    // --------------------------------------------------------------- blocks

    /// Parses statements up to (not including) `end`, `else` or end of file.
    fn block(&mut self) -> PResult<Block> {
        let saved = std::mem::replace(&mut self.nl_depth, 0);
        let result = self.block_inner();
        self.nl_depth = saved;
        result
    }

    fn block_inner(&mut self) -> PResult<Block> {
        let start = self.span().start;
        let mut stmts = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&T::End) || self.at(&T::Else) || self.at(&T::Eof) {
                break;
            }
            stmts.push(self.stmt()?);
            if !(self.at(&T::Newline) || self.at(&T::End) || self.at(&T::Else) || self.at(&T::Eof))
            {
                return Err(self.unexpected("end of line or `end`"));
            }
        }
        Ok(Block {
            stmts,
            span: self.since(start),
        })
    }

    fn stmt(&mut self) -> PResult<Stmt> {
        let start = self.span().start;
        if self.at(&T::Defer) {
            self.bump();
            let e = self.expr()?;
            return Ok(Stmt::Defer(e, self.since(start)));
        }
        // `var x = e`: `var` is only special in this position, so `env.var` still works.
        if matches!(self.kind(), T::Ident(ref s) if s == "var") && matches!(self.peek_raw(1), T::Ident(_)) && self.peek_raw(2) == T::Eq {
            self.bump();
            let name = self.ident("a name")?;
            self.expect(&T::Eq)?;
            let value = self.expr()?;
            return Ok(Stmt::Let {
                name,
                value,
                mutable: true,
                span: self.since(start),
            });
        }
        if matches!(self.kind(), T::Ident(_)) && self.peek_raw(1) == T::ColonEq {
            let name = self.ident("a name")?;
            self.expect(&T::ColonEq)?;
            let value = self.expr()?;
            return Ok(Stmt::Assign {
                name,
                value,
                span: self.since(start),
            });
        }
        if matches!(self.kind(), T::Ident(_)) && self.peek_raw(1) == T::Eq {
            let name = self.ident("a name")?;
            self.expect(&T::Eq)?;
            let value = self.expr()?;
            return Ok(Stmt::Let {
                name,
                value,
                mutable: false,
                span: self.since(start),
            });
        }
        Ok(Stmt::Expr(self.expr()?))
    }

    // ---------------------------------------------------------- expressions

    fn expr(&mut self) -> PResult<Expr> {
        self.pipe()
    }

    fn pipe(&mut self) -> PResult<Expr> {
        let mut lhs = self.binary(1)?;
        while self.at(&T::PipeGt) {
            self.bump();
            let rhs = self.binary(1)?;
            let span = lhs.span.to(rhs.span);
            lhs = Expr {
                kind: ExprKind::Pipe {
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(lhs)
    }

    /// The binary operator at the cursor: (precedence, operator, tokens it spans).
    fn binop(&mut self) -> Option<(u8, BinOp, usize)> {
        // `<<` and `>>` are two adjacent `<` or `>` tokens, so `Option<Option<T>>` still lexes.
        let adjacent = |s: &Self, k: T| {
            s.toks.get(s.pos + 1).is_some_and(|n| n.kind == k && n.span.start == s.toks[s.pos].span.end)
        };
        Some(match self.kind() {
            T::Or => (1, BinOp::Or, 1),
            T::And => (2, BinOp::And, 1),
            T::EqEq => (3, BinOp::Eq, 1),
            T::NotEq => (3, BinOp::NotEq, 1),
            T::Lt if adjacent(self, T::Lt) => (7, BinOp::Shl, 2),
            T::Gt if adjacent(self, T::Gt) => (7, BinOp::Shr, 2),
            T::Lt => (4, BinOp::Lt, 1),
            T::LtEq => (4, BinOp::LtEq, 1),
            T::Gt => (4, BinOp::Gt, 1),
            T::GtEq => (4, BinOp::GtEq, 1),
            T::Caret => (5, BinOp::BitXor, 1),
            T::Amp => (6, BinOp::BitAnd, 1),
            T::Plus => (8, BinOp::Add, 1),
            T::Minus => (8, BinOp::Sub, 1),
            T::Star => (9, BinOp::Mul, 1),
            T::Slash => (9, BinOp::Div, 1),
            T::Percent => (9, BinOp::Rem, 1),
            _ => return None,
        })
    }

    fn binary(&mut self, min_prec: u8) -> PResult<Expr> {
        let mut lhs = self.unary()?;
        while let Some((prec, op, ntoks)) = self.binop() {
            if prec < min_prec {
                break;
            }
            for _ in 0..ntoks {
                self.bump();
            }
            let rhs = self.binary(prec + 1)?;
            let span = lhs.span.to(rhs.span);
            lhs = Expr {
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> PResult<Expr> {
        let start = self.span().start;
        let op = match self.kind() {
            T::Minus => UnOp::Neg,
            T::Not => UnOp::Not,
            T::Tilde => UnOp::BitNot,
            _ => return self.postfix(),
        };
        self.bump();
        let inner = self.unary()?;
        Ok(Expr {
            kind: ExprKind::Unary {
                op,
                expr: Box::new(inner),
            },
            span: self.since(start),
        })
    }

    fn postfix(&mut self) -> PResult<Expr> {
        let mut e = self.primary()?;
        loop {
            let start = e.span.start;
            match self.kind() {
                T::LParen => {
                    let args = self.delimited(&T::LParen, &T::RParen, |p| p.expr())?;
                    e = Expr {
                        kind: ExprKind::Call {
                            callee: Box::new(e),
                            args,
                        },
                        span: self.since(start),
                    };
                }
                T::Dot => {
                    self.bump();
                    let name = self.ident("a field or function name after `.`")?;
                    if is_upper_name(&name.name) && self.at(&T::LBrace) {
                        if let ExprKind::Var(alias) = &e.kind {
                            let qualified = Ident {
                                name: format!("{alias}.{}", name.name),
                                span: e.span.to(name.span),
                            };
                            let fields = self.delimited(&T::LBrace, &T::RBrace, |p| {
                                let n = p.ident("a field name")?;
                                p.expect(&T::Colon)?;
                                let v = p.expr()?;
                                Ok((n, v))
                            })?;
                            e = Expr {
                                kind: ExprKind::Record { name: qualified, fields },
                                span: self.since(start),
                            };
                            continue;
                        }
                    }
                    e = Expr {
                        kind: ExprKind::Field {
                            expr: Box::new(e),
                            name,
                        },
                        span: self.since(start),
                    };
                }
                T::LBracket => {
                    self.bump();
                    self.nl_depth += 1;
                    let index = self.expr()?;
                    self.expect(&T::RBracket)?;
                    self.nl_depth -= 1;
                    e = Expr {
                        kind: ExprKind::Index {
                            expr: Box::new(e),
                            index: Box::new(index),
                        },
                        span: self.since(start),
                    };
                }
                T::Question => {
                    self.bump();
                    e = Expr {
                        kind: ExprKind::Try(Box::new(e)),
                        span: self.since(start),
                    };
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn primary(&mut self) -> PResult<Expr> {
        let tok = self.cur().clone();
        let start = tok.span.start;
        if matches!(&tok.kind, T::Ident(s) if s == "unsafe") && self.peek_raw(1) == T::Do {
            self.bump();
            self.bump();
            let body = self.block()?;
            self.expect(&T::End)?;
            return Ok(Expr {
                kind: ExprKind::Unsafe(body),
                span: self.since(start),
            });
        }
        match tok.kind {
            T::Int(s) => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Int(s),
                    span: tok.span,
                })
            }
            T::Float(s) => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Float(s),
                    span: tok.span,
                })
            }
            T::Str(s) => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Str(s),
                    span: tok.span,
                })
            }
            T::True | T::False => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Bool(tok.kind == T::True),
                    span: tok.span,
                })
            }
            T::Ident(name) => {
                if is_upper_name(&name) && self.peek_raw(1) == T::LBrace {
                    let id = self.ident("a type name")?;
                    let fields = self.delimited(&T::LBrace, &T::RBrace, |p| {
                        let n = p.ident("a field name")?;
                        p.expect(&T::Colon)?;
                        let v = p.expr()?;
                        Ok((n, v))
                    })?;
                    return Ok(Expr {
                        kind: ExprKind::Record { name: id, fields },
                        span: self.since(start),
                    });
                }
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Var(name),
                    span: tok.span,
                })
            }
            T::LParen => {
                self.bump();
                self.nl_depth += 1;
                if self.at(&T::RParen) {
                    self.bump();
                    self.nl_depth -= 1;
                    return Ok(Expr {
                        kind: ExprKind::Unit,
                        span: self.since(start),
                    });
                }
                let first = self.expr()?;
                if self.at(&T::Comma) {
                    let mut items = vec![first];
                    while self.eat(&T::Comma) {
                        if self.at(&T::RParen) {
                            break;
                        }
                        items.push(self.expr()?);
                    }
                    self.expect(&T::RParen)?;
                    self.nl_depth -= 1;
                    Ok(Expr {
                        kind: ExprKind::Tuple(items),
                        span: self.since(start),
                    })
                } else {
                    self.expect(&T::RParen)?;
                    self.nl_depth -= 1;
                    let mut inner = first;
                    inner.span = self.since(start);
                    Ok(inner)
                }
            }
            T::LBracket => {
                self.bump();
                self.nl_depth += 1;
                let mut items = Vec::new();
                let mut tail = None;
                while !self.at(&T::RBracket) {
                    items.push(self.expr()?);
                    if self.eat(&T::Bar) {
                        tail = Some(Box::new(self.expr()?));
                        break;
                    }
                    if !self.eat(&T::Comma) {
                        break;
                    }
                }
                self.expect(&T::RBracket)?;
                self.nl_depth -= 1;
                let kind = match tail {
                    Some(tail) => ExprKind::Cons { items, tail },
                    None => ExprKind::List(items),
                };
                Ok(Expr {
                    kind,
                    span: self.since(start),
                })
            }
            T::LBrace => {
                self.bump();
                self.nl_depth += 1;
                let base = Box::new(self.expr()?);
                self.expect(&T::Bar)?;
                let mut fields = vec![];
                loop {
                    if self.at(&T::RBrace) {
                        break;
                    }
                    let n = self.ident("a field name")?;
                    self.expect(&T::Colon)?;
                    let v = self.expr()?;
                    fields.push((n, v));
                    if !self.eat(&T::Comma) {
                        break;
                    }
                }
                self.expect(&T::RBrace)?;
                self.nl_depth -= 1;
                Ok(Expr {
                    kind: ExprKind::Update { base, fields },
                    span: self.since(start),
                })
            }
            T::Fn => self.lambda(),
            T::If => self.if_expr(),
            T::Match => self.match_expr(),
            T::Region => {
                self.bump();
                self.expect(&T::Do)?;
                let body = self.block()?;
                self.expect(&T::End)?;
                Ok(Expr {
                    kind: ExprKind::Region(body),
                    span: self.since(start),
                })
            }
            T::While => {
                self.bump();
                let cond = self.expr()?;
                self.expect(&T::Do)?;
                let body = self.block()?;
                self.expect(&T::End)?;
                Ok(Expr {
                    kind: ExprKind::While { cond: Box::new(cond), body },
                    span: self.since(start),
                })
            }
            T::For => {
                self.bump();
                let var = self.ident("a loop variable name")?;
                self.expect(&T::In)?;
                let iter = self.expr()?;
                self.expect(&T::Do)?;
                let body = self.block()?;
                self.expect(&T::End)?;
                Ok(Expr {
                    kind: ExprKind::For { var, iter: Box::new(iter), body },
                    span: self.since(start),
                })
            }
            T::Break => {
                self.bump();
                Ok(Expr { kind: ExprKind::Break, span: self.since(start) })
            }
            T::Continue => {
                self.bump();
                Ok(Expr { kind: ExprKind::Continue, span: self.since(start) })
            }
            T::With => {
                self.bump();
                let what = self.ident("an allocator or context name after `with`")?;
                self.expect(&T::LParen)?;
                self.nl_depth += 1;
                let arg = self.expr()?;
                self.expect(&T::RParen)?;
                self.nl_depth -= 1;
                self.expect(&T::Do)?;
                let body = self.block()?;
                self.expect(&T::End)?;
                Ok(Expr {
                    kind: ExprKind::With {
                        what,
                        arg: Box::new(arg),
                        body,
                    },
                    span: self.since(start),
                })
            }
            T::Do => {
                self.bump();
                let body = self.block()?;
                self.expect(&T::End)?;
                Ok(Expr {
                    kind: ExprKind::Block(body),
                    span: self.since(start),
                })
            }
            _ => Err(self.unexpected("an expression")),
        }
    }

    fn lambda(&mut self) -> PResult<Expr> {
        let start = self.expect(&T::Fn)?.span.start;
        let params = self.params(false)?;
        let body = if self.eat(&T::Arrow) {
            let e = self.expr()?;
            self.expect(&T::End)?;
            e
        } else if self.at(&T::Do) {
            let dstart = self.bump().span.start;
            let b = self.block()?;
            self.expect(&T::End)?;
            Expr {
                kind: ExprKind::Block(b),
                span: self.since(dstart),
            }
        } else {
            return Err(self.unexpected("`->` or `do` after the lambda parameters"));
        };
        Ok(Expr {
            kind: ExprKind::Lambda {
                params,
                body: Box::new(body),
            },
            span: self.since(start),
        })
    }

    fn if_expr(&mut self) -> PResult<Expr> {
        let start = self.expect(&T::If)?.span.start;
        let cond = self.expr()?;
        self.expect(&T::Do)?;
        let then = self.block()?;
        let els = if self.eat(&T::Else) {
            if self.at(&T::If) {
                // `else if` chains share a single `end`
                let nested = self.if_expr()?;
                let span = nested.span;
                let els = Block {
                    stmts: vec![Stmt::Expr(nested)],
                    span,
                };
                return Ok(Expr {
                    kind: ExprKind::If {
                        cond: Box::new(cond),
                        then,
                        els: Some(els),
                    },
                    span: self.since(start),
                });
            }
            Some(self.block()?)
        } else {
            None
        };
        self.expect(&T::End)?;
        Ok(Expr {
            kind: ExprKind::If {
                cond: Box::new(cond),
                then,
                els,
            },
            span: self.since(start),
        })
    }

    fn match_expr(&mut self) -> PResult<Expr> {
        let start = self.expect(&T::Match)?.span.start;
        let scrutinee = self.expr()?;
        self.expect(&T::Do)?;
        let saved = std::mem::replace(&mut self.nl_depth, 0);
        let arms = self.arms();
        self.nl_depth = saved;
        let arms = arms?;
        self.expect(&T::End)?;
        Ok(Expr {
            kind: ExprKind::Match {
                scrutinee: Box::new(scrutinee),
                arms,
            },
            span: self.since(start),
        })
    }

    fn arms(&mut self) -> PResult<Vec<Arm>> {
        let mut arms = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&T::End) || self.at(&T::Eof) {
                break;
            }
            let start = self.span().start;
            let pat = self.pattern()?;
            let guard = if self.eat(&T::If) {
                Some(self.expr()?)
            } else {
                None
            };
            self.expect(&T::Arrow)?;
            let body = self.expr()?;
            arms.push(Arm {
                pat,
                guard,
                body,
                span: self.since(start),
            });
            if !(self.at(&T::Newline) || self.at(&T::End)) {
                return Err(self.unexpected("end of line or `end` after a match arm"));
            }
        }
        Ok(arms)
    }

    // ------------------------------------------------------------- patterns

    fn pattern(&mut self) -> PResult<Pattern> {
        let tok = self.cur().clone();
        let start = tok.span.start;
        match tok.kind {
            T::Ident(name) => {
                if name == "_" {
                    self.bump();
                    return Ok(Pattern {
                        kind: PatKind::Wild,
                        span: tok.span,
                    });
                }
                let qualified = !is_upper_name(&name)
                    && self.peek_raw(1) == T::Dot
                    && matches!(self.peek_raw(2), T::Ident(ref n) if is_upper_name(n));
                if is_upper_name(&name) || qualified {
                    let id = self.qual_ident("a constructor name")?;
                    if self.at(&T::LParen) {
                        let args = self.delimited(&T::LParen, &T::RParen, |p| p.pattern())?;
                        return Ok(Pattern {
                            kind: PatKind::Ctor { name: id, args },
                            span: self.since(start),
                        });
                    }
                    if self.at(&T::LBrace) {
                        let fields = self.delimited(&T::LBrace, &T::RBrace, |p| {
                            let n = p.ident("a field name")?;
                            let pat = if p.eat(&T::Colon) {
                                Some(p.pattern()?)
                            } else {
                                None
                            };
                            Ok((n, pat))
                        })?;
                        return Ok(Pattern {
                            kind: PatKind::Record { name: id, fields },
                            span: self.since(start),
                        });
                    }
                    return Ok(Pattern {
                        kind: PatKind::Ctor {
                            name: id,
                            args: Vec::new(),
                        },
                        span: self.since(start),
                    });
                }
                self.bump();
                Ok(Pattern {
                    kind: PatKind::Bind(name),
                    span: tok.span,
                })
            }
            T::Int(s) => {
                self.bump();
                Ok(Pattern {
                    kind: PatKind::Int(s),
                    span: tok.span,
                })
            }
            T::Float(s) => {
                self.bump();
                Ok(Pattern {
                    kind: PatKind::Float(s),
                    span: tok.span,
                })
            }
            T::Str(s) => {
                self.bump();
                Ok(Pattern {
                    kind: PatKind::Str(s),
                    span: tok.span,
                })
            }
            T::True | T::False => {
                self.bump();
                Ok(Pattern {
                    kind: PatKind::Bool(tok.kind == T::True),
                    span: tok.span,
                })
            }
            T::Minus => {
                self.bump();
                match self.kind() {
                    T::Int(s) => {
                        self.bump();
                        Ok(Pattern {
                            kind: PatKind::Int(format!("-{s}")),
                            span: self.since(start),
                        })
                    }
                    T::Float(s) => {
                        self.bump();
                        Ok(Pattern {
                            kind: PatKind::Float(format!("-{s}")),
                            span: self.since(start),
                        })
                    }
                    _ => Err(self.unexpected("a number after `-` in a pattern")),
                }
            }
            T::LParen => {
                let mut items = self.delimited(&T::LParen, &T::RParen, |p| p.pattern())?;
                let span = self.since(start);
                if items.len() == 1 {
                    let mut p = items.pop().unwrap();
                    p.span = span;
                    Ok(p)
                } else {
                    Ok(Pattern {
                        kind: PatKind::Tuple(items),
                        span,
                    })
                }
            }
            T::LBracket => {
                self.bump();
                self.nl_depth += 1;
                let mut items = Vec::new();
                let mut tail = None;
                while !self.at(&T::RBracket) {
                    items.push(self.pattern()?);
                    if self.eat(&T::Bar) {
                        tail = Some(Box::new(self.pattern()?));
                        break;
                    }
                    if !self.eat(&T::Comma) {
                        break;
                    }
                }
                self.expect(&T::RBracket)?;
                self.nl_depth -= 1;
                Ok(Pattern {
                    kind: PatKind::List { items, tail },
                    span: self.since(start),
                })
            }
            _ => Err(self.unexpected("a pattern")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(src: &str) -> Program {
        let p = parse(src);
        assert!(p.is_ok(), "unexpected diagnostics: {:#?}", p.diagnostics);
        p.program
    }

    fn first_fn(src: &str) -> FnDef {
        match ok(src).items.into_iter().next().unwrap() {
            Item::Decl(Decl {
                kind: DeclKind::Fn(f),
                ..
            }) => f,
            other => panic!("expected fn, got {other:?}"),
        }
    }

    fn last_expr(f: &FnDef) -> &Expr {
        match f.body.as_ref().unwrap().stmts.last().unwrap() {
            Stmt::Expr(e) => e,
            other => panic!("expected expression statement, got {other:?}"),
        }
    }

    #[test]
    fn hello_world() {
        let prog = ok("import std.io\n\nmain :: fn() do\n  io.println(\"Hello, world!\")\nend\n");
        assert_eq!(prog.items.len(), 2);
        match &prog.items[0] {
            Item::Import(i) => {
                let names: Vec<_> = i.path.iter().map(|p| p.name.as_str()).collect();
                assert_eq!(names, ["std", "io"]);
            }
            _ => panic!("expected import"),
        }
    }

    #[test]
    fn import_alias() {
        let prog = ok("import std.collections.map as m\n");
        match &prog.items[0] {
            Item::Import(i) => assert_eq!(i.alias.as_ref().unwrap().name, "m"),
            _ => panic!(),
        }
    }

    #[test]
    fn pipe_hello() {
        let f = first_fn("main :: fn() do\n  \"hi\" |> io.println\nend\n");
        assert!(matches!(last_expr(&f).kind, ExprKind::Pipe { .. }));
    }

    #[test]
    fn spec_example_parses() {
        let src = "\
import std.io

Shape :: type
  | Circle(r: f64)
  | Rect(w: f64, h: f64)
end

area :: fn(s: Shape) -> f64 do
  match s do
    Circle(r)  -> 3.14159 * r * r
    Rect(w, h) -> w * h
  end
end

main :: fn() do
  region do
    shapes = [Circle(2.0), Rect(3.0, 4.0)]
    shapes
      |> map(area)
      |> sum
      |> io.println
  end
end
";
        let prog = ok(src);
        assert_eq!(prog.items.len(), 4);
        match &prog.items[1] {
            Item::Decl(Decl {
                kind: DeclKind::Type(TypeDef::Variants(vs)),
                ..
            }) => {
                assert_eq!(vs.len(), 2);
                assert_eq!(vs[0].name.name, "Circle");
                assert_eq!(vs[1].fields.len(), 2);
            }
            other => panic!("expected variants, got {other:?}"),
        }
    }

    #[test]
    fn multiline_pipeline_is_one_expression() {
        let f = first_fn("f :: fn() do\n  xs\n    |> map(g)\n    |> sum\nend\n");
        assert_eq!(f.body.as_ref().unwrap().stmts.len(), 1);
        match &last_expr(&f).kind {
            ExprKind::Pipe { lhs, .. } => assert!(matches!(lhs.kind, ExprKind::Pipe { .. })),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn operator_precedence() {
        let f = first_fn("f :: fn() do\n  1 + 2 * 3 == 7 and not false\nend\n");
        match &last_expr(&f).kind {
            ExprKind::Binary {
                op: BinOp::And,
                lhs,
                rhs,
            } => {
                assert!(matches!(lhs.kind, ExprKind::Binary { op: BinOp::Eq, .. }));
                assert!(matches!(rhs.kind, ExprKind::Unary { op: UnOp::Not, .. }));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn bindings_and_defer() {
        let f = first_fn("f :: fn() do\n  x = 1\n  defer close(x)\n  x\nend\n");
        assert!(matches!(f.body.as_ref().unwrap().stmts[0], Stmt::Let { .. }));
        assert!(matches!(f.body.as_ref().unwrap().stmts[1], Stmt::Defer(..)));
        assert!(matches!(f.body.as_ref().unwrap().stmts[2], Stmt::Expr(..)));
    }

    #[test]
    fn generics_and_recursive_types() {
        let prog = ok("\
List<T> :: type
  | Nil
  | Cons(T, List<T>)
end

map :: fn<A, B>(xs: List<A>, f: fn(A) -> B) -> List<B> do
  match xs do
    Nil -> Nil
    Cons(h, t) -> Cons(f(h), map(t, f))
  end
end
");
        match &prog.items[0] {
            Item::Decl(d) => assert_eq!(d.generics[0].name, "T"),
            _ => panic!(),
        }
        match &prog.items[1] {
            Item::Decl(Decl {
                kind: DeclKind::Fn(f),
                ..
            }) => {
                assert_eq!(f.generics.len(), 2);
                assert_eq!(f.params.len(), 2);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn records() {
        let prog = ok("\
Point :: type { x: f64, y: f64 }

origin :: fn() -> Point do
  Point{x: 0.0, y: 0.0}
end

norm2 :: fn(p: Point) -> f64 do
  match p do
    Point{x, y} -> x * x + y * y
  end
end
");
        assert_eq!(prog.items.len(), 3);
    }

    #[test]
    fn if_else_chain_shares_one_end() {
        let f = first_fn(
            "f :: fn(n: i32) -> i32 do\n  if n < 0 do\n    -1\n  else if n == 0 do\n    0\n  else\n    1\n  end\nend\n",
        );
        match &last_expr(&f).kind {
            ExprKind::If { els: Some(b), .. } => {
                assert!(matches!(
                    b.stmts[0],
                    Stmt::Expr(Expr {
                        kind: ExprKind::If { .. },
                        ..
                    })
                ));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn single_line_if() {
        ok("f :: fn(b: bool) -> i32 do\n  if b do 1 else 2 end\nend\n");
    }

    #[test]
    fn lambdas() {
        ok("f :: fn() do\n  xs |> map(fn(x) -> x * 2 end)\nend\n");
        ok("f :: fn() do\n  xs |> map(fn(x: i32) do\n    y = x * 2\n    y + 1\n  end)\nend\n");
    }

    #[test]
    fn patterns() {
        ok("\
f :: fn(xs: [i32]) -> i32 do
  match xs do
    [] -> 0
    [a] -> a
    [a, b | rest] if a > b -> a
    (x, _) -> x
    0 -> 1
    -1 -> 2
    \"s\" -> 3
    true -> 4
    Some(Pair(a, _)) -> a
    _ -> 5
  end
end
");
    }

    #[test]
    fn regions_and_allocators() {
        ok("\
main :: fn() do
  region do
    xs = [1, 2, 3]
    ys = promote(xs)
    with allocator(arena) do
      zs = alloc_list(4)
    end
  end
end
");
    }

    #[test]
    fn list_cons_expressions() {
        let f = first_fn("f :: fn() do\n  [1, 2 | rest]\nend\n");
        match &last_expr(&f).kind {
            ExprKind::Cons { items, tail } => {
                assert_eq!(items.len(), 2);
                assert!(matches!(tail.kind, ExprKind::Var(_)));
            }
            other => panic!("{other:?}"),
        }
        let f = first_fn("f :: fn() do\n  [g(x) | map(rest, g)]\nend\n");
        assert!(matches!(last_expr(&f).kind, ExprKind::Cons { .. }));
        // plain lists still work
        let f = first_fn("f :: fn() do\n  [1, 2]\nend\n");
        assert!(matches!(last_expr(&f).kind, ExprKind::List(_)));
    }

    #[test]
    fn postfix_chains() {
        let f = first_fn("f :: fn() do\n  a.b(1).c[2]?\nend\n");
        assert!(matches!(last_expr(&f).kind, ExprKind::Try(..)));
    }

    #[test]
    fn newlines_inside_parens_are_ignored() {
        ok("f :: fn() do\n  g(\n    1,\n    2\n  )\n  [\n    1,\n    2,\n  ]\nend\n");
    }

    #[test]
    fn constants() {
        let prog = ok("PI :: 3.14159\nanswer :: 40 + 2\n");
        assert_eq!(prog.items.len(), 2);
    }

    #[test]
    fn comments_everywhere() {
        ok("# top\nf :: fn() do # trailing\n  # inside\n  1 # one\nend # done\n");
    }

    #[test]
    fn missing_end_is_reported() {
        let p = parse("f :: fn() do\n  1\n");
        assert_eq!(p.diagnostics.len(), 1);
        assert!(
            p.diagnostics[0].message.contains("`end`"),
            "{:?}",
            p.diagnostics
        );
    }

    #[test]
    fn errors_do_not_stop_later_items() {
        let p = parse("a :: fn() do\n  )\nend\n\nb :: fn() do\n  1\nend\n\nc :: fn( do\nend\n");
        assert_eq!(p.diagnostics.len(), 2, "{:#?}", p.diagnostics);
        let names: Vec<_> = p
            .program
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Decl(d) => Some(d.name.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(names, ["b"]);
    }

    #[test]
    fn garbage_at_top_level() {
        let p = parse("1 + 2\n");
        assert_eq!(p.diagnostics.len(), 1);
    }

    #[test]
    fn missing_param_type() {
        let p = parse("f :: fn(x) do\nend\n");
        assert_eq!(p.diagnostics.len(), 1);
        assert!(p.diagnostics[0].message.contains("type"));
    }

    #[test]
    fn two_statements_on_one_line_are_rejected() {
        let p = parse("f :: fn() do\n  a b\nend\n");
        assert_eq!(p.diagnostics.len(), 1);
    }

    #[test]
    fn empty_file_is_ok() {
        assert!(parse("").is_ok());
        assert!(parse("\n\n# only a comment\n").is_ok());
    }

    #[test]
    fn intrinsics_mode_allows_bodyless_fns_and_opaque_types() {
        let src = "\
Allocator :: type
Map<K, V> :: type

println :: fn<T>(value: T)
map :: fn<A, B>(xs: [A], f: fn(A) -> B) -> [B]
";
        let p = parse_with(src, ParseOptions { intrinsics: true });
        assert!(p.is_ok(), "{:?}", p.diagnostics);
        assert_eq!(p.program.items.len(), 4);
        match &p.program.items[0] {
            Item::Decl(Decl {
                kind: DeclKind::Type(TypeDef::Opaque),
                ..
            }) => {}
            other => panic!("{other:?}"),
        }
        match &p.program.items[2] {
            Item::Decl(Decl {
                kind: DeclKind::Fn(f),
                ..
            }) => assert!(f.body.is_none()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn ordinary_programs_still_require_bodies() {
        assert!(!parse("println :: fn<T>(value: T)\n").is_ok());
        assert!(!parse("Allocator :: type\n").is_ok());
    }

    #[test]
    fn spans_cover_declarations() {
        let src = "f :: fn() do\n  1\nend\n";
        let prog = ok(src);
        match &prog.items[0] {
            Item::Decl(d) => {
                assert_eq!(&src[d.span.start..d.span.end], "f :: fn() do\n  1\nend");
                assert_eq!(&src[d.name.span.start..d.name.span.end], "f");
            }
            _ => panic!(),
        }
    }
    #[test]
    fn trait_and_impl_parse() {
        let prog = ok("\
Show :: trait
  show :: fn(self) -> String
end

Shape :: type
  | Dot
end

impl Show for Shape do
  show :: fn(s: Shape) -> String do
    \"dot\"
  end
end

impl<T: Show> Show for [T] do
  show :: fn(xs: [T]) -> String do
    \"list\"
  end
end

largest :: fn<T: Ord + Show, U>(a: T, b: U) -> T do
  a
end
");
        assert_eq!(prog.items.len(), 5);
        match &prog.items[0] {
            Item::Decl(Decl {
                kind: DeclKind::Trait(t),
                ..
            }) => {
                assert_eq!(t.methods.len(), 1);
                assert_eq!(t.methods[0].params[0].name.name, "self");
            }
            other => panic!("{other:?}"),
        }
        match &prog.items[2] {
            Item::Impl(i) => {
                assert_eq!(i.trait_name.name, "Show");
                assert_eq!(i.methods.len(), 1);
            }
            other => panic!("{other:?}"),
        }
        match &prog.items[3] {
            Item::Impl(i) => {
                assert_eq!(i.generics.len(), 1);
                assert_eq!(i.bounds[0].traits[0].name, "Show");
            }
            other => panic!("{other:?}"),
        }
        match &prog.items[4] {
            Item::Decl(Decl {
                kind: DeclKind::Fn(f),
                ..
            }) => {
                assert_eq!(f.generics.len(), 2);
                assert_eq!(f.bounds.len(), 1);
                assert_eq!(f.bounds[0].traits.len(), 2);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn trait_method_returning_generic_type() {
        // `>` at the end of a line swallows the newline; the next method must still parse.
        let prog = ok("\
Seq :: trait
  first :: fn(self) -> Option<i32>
  second :: fn(self) -> Option<i32>
end
");
        match &prog.items[0] {
            Item::Decl(Decl {
                kind: DeclKind::Trait(t),
                ..
            }) => assert_eq!(t.methods.len(), 2),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn impl_errors_recover() {
        let p = parse("impl Show Shape do\nend\nf :: fn() do\nend\n");
        assert!(!p.is_ok());
        assert!(p.program.items.iter().any(|i| matches!(i, Item::Decl(d) if d.name.name == "f")));
    }
}

#[cfg(test)]
mod qualified_tests {
    use super::*;

    #[test]
    fn qualified_names_parse() {
        let src = "f :: fn() do\n  p = shapes.Point { x: 1 }\n  match p do\n    shapes.Point { x } -> x\n    shapes.Circle(r) -> r\n    _ -> 0\n  end\nend\nimpl a.Show for b.T do\nend\n";
        let p = parse(src);
        assert!(p.is_ok(), "{:?}", p.diagnostics);
    }
}
