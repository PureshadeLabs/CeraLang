//! The bytecode compiler: checked syntax tree to `Bytecode`.
//!
//! Names are resolved here, once: locals become frame slots, captured
//! variables become closure slots, and everything else (functions, constants,
//! constructors, built-ins, module members) becomes a constant or global.
//! Errors the tree-walking interpreter reports at run time (an unknown module
//! member, say) become `Fail` instructions, so they are still raised only when
//! the code runs.

use crate::bytecode::*;
use crate::eval::{parse_float, parse_int, type_head_ast, PRELUDE_CTORS, PRELUDE_TRAITS};
use crate::value::*;
use cera_sema::Analysis;
use cera_syntax::ast::*;
use cera_syntax::Span;
use std::collections::HashMap;
use std::rc::Rc;

struct LoopCx {
    /// Local holding the stack height at the start of the loop.
    sp: usize,
    /// Where `continue` jumps.
    start: usize,
    /// `break` jumps still to be patched.
    breaks: Vec<usize>,
}

struct FnCtx {
    loops: Vec<LoopCx>,
    scopes: Vec<HashMap<String, usize>>,
    nlocals: usize,
    captures: Vec<String>,
    code: Vec<Op>,
    spans: Vec<Span>,
}

#[derive(Clone, Copy)]
enum Loc {
    Local(usize),
    Capture(usize),
}

struct Compiler {
    ctxs: Vec<FnCtx>,
    chunks: Vec<Option<Chunk>>,
    consts: Vec<Value<'static>>,
    pats: Vec<Pat>,
    records: Vec<RecordDesc>,
    messages: Vec<String>,
    names: Vec<Rc<str>>,
    globals: Vec<usize>,
    const_ids: HashMap<String, usize>,
    fn_ids: HashMap<String, usize>,
    fn_consts: HashMap<usize, usize>,
    ctors: HashMap<String, (Rc<str>, usize)>,
    record_types: HashMap<String, Vec<String>>,
    trait_methods: HashMap<String, Rc<str>>,
    aliases: HashMap<String, String>,
    imported: Vec<String>,
    lit_types: HashMap<(usize, usize), String>,
}

pub fn compile(program: &Program, analysis: &Analysis) -> Bytecode {
    let mut c = Compiler {
        ctxs: Vec::new(),
        chunks: Vec::new(),
        consts: Vec::new(),
        pats: Vec::new(),
        records: Vec::new(),
        messages: Vec::new(),
        names: Vec::new(),
        globals: Vec::new(),
        const_ids: HashMap::new(),
        fn_ids: HashMap::new(),
        fn_consts: HashMap::new(),
        ctors: HashMap::new(),
        record_types: HashMap::new(),
        trait_methods: HashMap::new(),
        aliases: HashMap::new(),
        imported: Vec::new(),
        lit_types: HashMap::new(),
    };
    for (ctor, ty, arity) in PRELUDE_CTORS {
        c.ctors.insert(ctor.to_string(), (Rc::from(*ty), *arity));
    }
    for (m, t) in PRELUDE_TRAITS {
        c.trait_methods.insert(m.to_string(), Rc::from(*t));
    }
    for t in &analysis.types {
        c.lit_types
            .entry((t.span.start, t.span.end))
            .or_insert_with(|| t.text.clone());
    }

    // Pass 1: declarations, and reserved chunk numbers for everything named.
    let mut fn_defs: Vec<(usize, &str, &FnDef)> = Vec::new();
    let mut const_defs: Vec<(usize, &str, &Expr)> = Vec::new();
    let mut impl_defs: Vec<(String, String, String, usize, &FnDef)> = Vec::new();
    for item in &program.items {
        match item {
            Item::Import(i) => {
                let path: Vec<&str> = i.path.iter().map(|p| p.name.as_str()).collect();
                let key = path.join(".");
                let alias = i
                    .alias
                    .as_ref()
                    .map(|a| a.name.clone())
                    .unwrap_or_else(|| path.last().unwrap().to_string());
                c.aliases.insert(alias, key.clone());
                c.imported.push(key);
            }
            Item::Impl(im) => {
                let head = type_head_ast(&im.ty);
                for m in &im.methods {
                    if let DeclKind::Fn(f) = &m.kind {
                        let idx = c.reserve();
                        impl_defs.push((im.trait_name.name.clone(), head.clone(), m.name.name.clone(), idx, f));
                    }
                }
            }
            Item::Decl(d) => match &d.kind {
                DeclKind::Fn(f) => {
                    if !c.fn_ids.contains_key(&d.name.name) {
                        let idx = c.reserve();
                        c.fn_ids.insert(d.name.name.clone(), idx);
                        fn_defs.push((idx, &d.name.name, f));
                    }
                }
                DeclKind::Const(e) => {
                    if !c.const_ids.contains_key(&d.name.name) {
                        let idx = c.reserve();
                        c.const_ids.insert(d.name.name.clone(), c.globals.len());
                        c.globals.push(idx);
                        const_defs.push((idx, &d.name.name, e));
                    }
                }
                DeclKind::Trait(t) => {
                    for m in &t.methods {
                        c.trait_methods
                            .insert(m.name.name.clone(), Rc::from(d.name.name.as_str()));
                    }
                }
                DeclKind::Type(TypeDef::Variants(vs)) => {
                    for v in vs {
                        c.ctors
                            .insert(v.name.name.clone(), (Rc::from(d.name.name.as_str()), v.fields.len()));
                    }
                }
                DeclKind::Type(TypeDef::Record(fs)) => {
                    c.record_types
                        .insert(d.name.name.clone(), fs.iter().map(|f| f.name.name.clone()).collect());
                }
                DeclKind::Type(TypeDef::Opaque) => {}
            },
        }
    }

    // Pass 2: bodies.
    for (idx, name, f) in fn_defs {
        let chunk = c.function(name, f);
        c.chunks[idx] = Some(chunk);
    }
    for (idx, name, e) in const_defs {
        c.begin_ctx(&[]);
        c.expr(e, true);
        let chunk = c.end_ctx(name, 0).0;
        c.chunks[idx] = Some(chunk);
    }
    let mut impls: HashMap<(String, String), HashMap<String, usize>> = HashMap::new();
    for (tr, head, method, idx, f) in impl_defs {
        let chunk = c.function(&format!("{tr}.{head}.{method}"), f);
        c.chunks[idx] = Some(chunk);
        impls.entry((tr, head)).or_default().insert(method, idx);
    }

    Bytecode {
        chunks: c.chunks.into_iter().map(|c| c.expect("unfinished chunk")).collect(),
        consts: c.consts,
        pats: c.pats,
        records: c.records,
        messages: c.messages,
        names: c.names,
        globals: c.globals,
        fns: c.fn_ids,
        impls,
    }
}

fn flatten(e: &Expr) -> Option<Vec<&str>> {
    match &e.kind {
        ExprKind::Var(n) => Some(vec![n.as_str()]),
        ExprKind::Field { expr, name } => {
            let mut v = flatten(expr)?;
            v.push(&name.name);
            Some(v)
        }
        _ => None,
    }
}

impl Compiler {
    // ----- plumbing --------------------------------------------------------------

    fn reserve(&mut self) -> usize {
        self.chunks.push(None);
        self.chunks.len() - 1
    }

    fn ctx(&mut self) -> &mut FnCtx {
        self.ctxs.last_mut().expect("no function context")
    }

    fn emit(&mut self, op: Op, span: Span) -> usize {
        let c = self.ctx();
        c.code.push(op);
        c.spans.push(span);
        c.code.len() - 1
    }

    fn here(&mut self) -> usize {
        self.ctx().code.len()
    }

    /// Point the jump at `at` to the current position.
    fn patch(&mut self, at: usize) {
        let target = self.here();
        let c = self.ctx();
        match &mut c.code[at] {
            Op::Jump(t) | Op::JumpIfFalse(t) | Op::AndJump(t) | Op::OrJump(t) | Op::CmpJump(_, t) | Op::JumpSp(_, t) => *t = target,
            Op::ForNext { exit, .. } => *exit = target as u32,
            Op::CmpJumpK(_, _, t) => *t = target as u32,
            Op::LocalCmpJumpK(_, _, _, t) => *t = target as u32,
            Op::MatchPat { fail, .. } => *fail = target as u32,
            other => unreachable!("cannot patch {other:?}"),
        }
    }

    fn constant(&mut self, v: Value<'static>) -> usize {
        self.consts.push(v);
        self.consts.len() - 1
    }

    fn message(&mut self, m: impl Into<String>) -> usize {
        self.messages.push(m.into());
        self.messages.len() - 1
    }

    fn name_id(&mut self, n: &str) -> usize {
        if let Some(i) = self.names.iter().position(|x| &**x == n) {
            return i;
        }
        self.names.push(Rc::from(n));
        self.names.len() - 1
    }

    fn fail(&mut self, msg: impl Into<String>, span: Span) {
        let m = self.message(msg);
        self.emit(Op::Fail(m), span);
    }

    fn begin_ctx(&mut self, params: &[String]) {
        let mut scope = HashMap::new();
        for (i, p) in params.iter().enumerate() {
            scope.insert(p.clone(), i);
        }
        self.ctxs.push(FnCtx {
            loops: Vec::new(),
            scopes: vec![scope],
            nlocals: params.len(),
            captures: Vec::new(),
            code: Vec::new(),
            spans: Vec::new(),
        });
    }

    fn end_ctx(&mut self, name: &str, arity: usize) -> (Chunk, Vec<String>) {
        let c = self.ctxs.pop().expect("no function context");
        (
            Chunk {
                name: name.to_string(),
                arity,
                nlocals: c.nlocals,
                code: c.code,
                spans: c.spans,
            },
            c.captures,
        )
    }

    fn push_scope(&mut self) {
        self.ctx().scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.ctx().scopes.pop();
    }

    fn new_local(&mut self, name: &str) -> usize {
        let c = self.ctx();
        let slot = c.nlocals;
        c.nlocals += 1;
        c.scopes.last_mut().unwrap().insert(name.to_string(), slot);
        slot
    }

    fn resolve_at(&mut self, depth: usize, name: &str) -> Option<Loc> {
        let ctx = &self.ctxs[depth];
        for s in ctx.scopes.iter().rev() {
            if let Some(slot) = s.get(name) {
                return Some(Loc::Local(*slot));
            }
        }
        if let Some(i) = ctx.captures.iter().position(|c| c == name) {
            return Some(Loc::Capture(i));
        }
        if depth == 0 {
            return None;
        }
        self.resolve_at(depth - 1, name)?;
        let ctx = &mut self.ctxs[depth];
        ctx.captures.push(name.to_string());
        Some(Loc::Capture(ctx.captures.len() - 1))
    }

    fn resolve(&mut self, name: &str) -> Option<Loc> {
        let depth = self.ctxs.len().checked_sub(1)?;
        self.resolve_at(depth, name)
    }

    /// Whether `name` is visible as a local here, without capturing it.
    fn has_local(&self, name: &str) -> bool {
        self.ctxs.iter().any(|c| {
            c.scopes.iter().any(|s| s.contains_key(name)) || c.captures.iter().any(|x| x == name)
        })
    }

    /// Whether `name` is not shadowed by a local here.
    fn resolve_is_global(&self, name: &str) -> bool {
        !self.has_local(name)
    }

    fn fn_const(&mut self, idx: usize) -> usize {
        if let Some(c) = self.fn_consts.get(&idx) {
            return *c;
        }
        let c = self.constant(Value::Fun(Rc::new(FunVal {
            func: idx,
            captures: vec![],
        })));
        self.fn_consts.insert(idx, c);
        c
    }

    // ----- functions --------------------------------------------------------------

    fn function(&mut self, name: &str, f: &FnDef) -> Chunk {
        let params: Vec<String> = f.params.iter().map(|p| p.name.name.clone()).collect();
        self.begin_ctx(&params);
        match &f.body {
            Some(body) => self.block(body, true),
            None => self.fail("this function has no body", Span::new(0, 0)),
        }
        self.end_ctx(name, params.len()).0
    }

    /// Compile a nested function and emit the code that builds the closure.
    fn closure(&mut self, name: &str, params: Vec<String>, span: Span, body: impl FnOnce(&mut Self)) {
        self.begin_ctx(&params);
        body(self);
        let (chunk, caps) = self.end_ctx(name, params.len());
        self.chunks.push(Some(chunk));
        let func = self.chunks.len() - 1;
        for c in &caps {
            self.load_name(c, span);
        }
        self.emit(Op::Closure { func: func as u32, ncap: caps.len() as u32 }, span);
    }

    fn load_name(&mut self, name: &str, span: Span) {
        match self.resolve(name) {
            Some(Loc::Local(s)) => self.emit(Op::Local(s), span),
            Some(Loc::Capture(i)) => self.emit(Op::Capture(i), span),
            None => unreachable!("captured name `{name}` is not visible in the parent"),
        };
    }

    // ----- names -----------------------------------------------------------------

    fn var(&mut self, name: &str, span: Span) {
        match self.resolve(name) {
            Some(Loc::Local(s)) => {
                self.emit(Op::Local(s), span);
                return;
            }
            Some(Loc::Capture(i)) => {
                self.emit(Op::Capture(i), span);
                return;
            }
            None => {}
        }
        if let Some(idx) = self.fn_ids.get(name).copied() {
            let c = self.fn_const(idx);
            self.emit(Op::Const(c), span);
        } else if let Some(g) = self.const_ids.get(name).copied() {
            self.emit(Op::Global(g), span);
        } else if let Some((ty, arity)) = self.ctors.get(name).cloned() {
            let v = if arity == 0 {
                Value::Ctor {
                    tag: tag_of(&ty, name),
                    fields: Rc::new(vec![]),
                }
            } else {
                Value::CtorFn {
                    tag: tag_of(&ty, name),
                    arity,
                }
            };
            let c = self.constant(v);
            self.emit(Op::Const(c), span);
        } else if let Some(t) = self.trait_methods.get(name).cloned() {
            let c = self.constant(Value::trait_method(&t, name));
            self.emit(Op::Const(c), span);
        } else if let Some(n) = crate::native::prelude_native(name) {
            let c = self.constant(Value::Native(n));
            self.emit(Op::Const(c), span);
        } else {
            self.fail(format!("unknown name `{name}`"), span);
        }
    }

    fn module_member(&mut self, chain: &[&str], span: Span) {
        let (m, member) = chain.split_at(chain.len() - 1);
        let joined = m.join(".");
        let key = if m.len() == 1 {
            match self.aliases.get(&joined) {
                Some(k) => k.clone(),
                None => match joined.as_str() {
                    "String" => "std.string".to_string(),
                    "List" => "std.list".to_string(),
                    "Option" => "std.option".to_string(),
                    "Result" => "std.result".to_string(),
                    "Num" => "std.num".to_string(),
                    "Map" => "std.map".to_string(),
                    "Set" => "std.set".to_string(),
                    "Bits" => "std.bits".to_string(),
                    "Array" => "std.array".to_string(),
                    _ => return self.fail(format!("unknown module `{joined}`"), span),
                },
            }
        } else if self.imported.contains(&joined) {
            joined.clone()
        } else {
            return self.fail(format!("unknown module `{joined}`"), span);
        };
        match crate::native::module_native(&key, member[0]) {
            Some(n) => {
                let c = self.constant(Value::Native(n));
                self.emit(Op::Const(c), span);
            }
            None => self.fail(format!("module `{key}` has no member `{}`", member[0]), span),
        }
    }

    // ----- literals ---------------------------------------------------------------

    /// Whether the literal at `span` is typed as an integer (not a float).
    fn neg_lit_ok(&self, span: Span) -> bool {
        match self.lit_types.get(&(span.start, span.end)) {
            Some(t) => IntTy::from_name(t).is_some(),
            None => true,
        }
    }

    fn int_lit(&mut self, s: &str, span: Span) {
        let Some(v) = parse_int(s) else {
            return self.fail(format!("invalid integer literal `{s}`"), span);
        };
        let ty = self
            .lit_types
            .get(&(span.start, span.end))
            .and_then(|t| IntTy::from_name(t))
            .unwrap_or(IntTy::I32);
        if !ty.fits(v) {
            return self.fail(format!("integer literal `{s}` does not fit in `{}`", ty.name()), span);
        }
        let c = self.constant(Value::Int(v, ty));
        self.emit(Op::Const(c), span);
    }

    fn float_lit(&mut self, s: &str, span: Span) {
        let Some(v) = parse_float(s) else {
            return self.fail(format!("invalid float literal `{s}`"), span);
        };
        let f32 = self.lit_types.get(&(span.start, span.end)).is_some_and(|t| t == "f32");
        let c = self.constant(Value::Float(if f32 { v as f32 as f64 } else { v }, f32));
        self.emit(Op::Const(c), span);
    }

    // ----- blocks and statements --------------------------------------------------

    /// With `tail`, control always leaves the chunk (`Return` or `TailCall`);
    /// without it exactly one value is left on the stack.
    fn block(&mut self, b: &Block, tail: bool) {
        self.push_scope();
        let has_defer = b.stmts.iter().any(|s| matches!(s, Stmt::Defer(..)));
        let tail_here = tail && !has_defer;
        let n = b.stmts.len();
        let mut ndefers = 0;
        for (i, s) in b.stmts.iter().enumerate() {
            match s {
                Stmt::Let { name, value, span, .. } => {
                    self.expr(value, false);
                    let slot = self.new_local(&name.name);
                    self.emit(Op::SetLocal(slot), *span);
                }
                Stmt::Assign { name, value, span } => {
                    self.expr(value, false);
                    match self.resolve(&name.name) {
                        Some(Loc::Local(slot)) => {
                            self.emit(Op::SetLocal(slot), *span);
                        }
                        _ => self.fail(format!("cannot assign to `{}`", name.name), *span),
                    }
                }
                Stmt::Defer(e, span) => {
                    self.closure("defer", vec![], *span, |c| c.expr(e, true));
                    self.emit(Op::PushDefer, *span);
                    ndefers += 1;
                }
                Stmt::Expr(e) => {
                    if i == n - 1 {
                        self.expr(e, tail_here);
                    } else {
                        self.expr(e, false);
                        self.emit(Op::Pop, e.span);
                    }
                }
            }
        }
        let last_is_expr = matches!(b.stmts.last(), Some(Stmt::Expr(_)));
        if !last_is_expr {
            self.emit(Op::Unit, b.span);
        }
        if has_defer {
            self.emit(Op::RunDefers(ndefers), b.span);
        }
        if tail && !(tail_here && last_is_expr) {
            self.emit(Op::Return, b.span);
        }
        self.pop_scope();
    }

    // ----- expressions ------------------------------------------------------------

    fn expr(&mut self, e: &Expr, tail: bool) {
        let sp = e.span;
        match &e.kind {
            ExprKind::Call { callee, args } if matches!(&callee.kind, ExprKind::Var(n) if n == "asm" && !self.has_local("asm") && !self.fn_ids.contains_key("asm")) => {
                self.fail("`asm` needs the native backend (run with `--native`, or build an executable)", sp);
                self.emit(Op::Unit, sp);
                let _ = args;
            }
            ExprKind::Call { callee, args } => {
                let known = match &callee.kind {
                    ExprKind::Var(n) if self.resolve_is_global(n) => self.fn_ids.get(n.as_str()).copied(),
                    _ => None,
                };
                if let Some(func) = known {
                    self.emit(Op::Unit, callee.span);
                    for a in args {
                        self.expr(a, false);
                    }
                    self.emit(if tail { Op::TailCallFn(func as u32, args.len() as u32) } else { Op::CallFn(func as u32, args.len() as u32) }, sp);
                    return;
                }
                self.expr(callee, false);
                for a in args {
                    self.expr(a, false);
                }
                self.emit(if tail { Op::TailCall(args.len()) } else { Op::Call(args.len()) }, sp);
            }
            ExprKind::Pipe { lhs, rhs } => {
                self.expr(lhs, false);
                let argc = match &rhs.kind {
                    ExprKind::Call { callee, args } => {
                        self.expr(callee, false);
                        self.emit(Op::Swap, sp);
                        for a in args {
                            self.expr(a, false);
                        }
                        args.len() + 1
                    }
                    _ => {
                        self.expr(rhs, false);
                        self.emit(Op::Swap, sp);
                        1
                    }
                };
                self.emit(if tail { Op::TailCall(argc) } else { Op::Call(argc) }, sp);
            }
            ExprKind::If { cond, then, els } => {
                self.expr(cond, false);
                // `if a < b`: fuse the comparison with the branch.
                let fuse = matches!(&cond.kind, ExprKind::Binary { op, .. }
                    if matches!(op, BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq));
                let jf = match (fuse, self.ctx().code.last().copied()) {
                    (true, Some(Op::Cmp(o))) => {
                        let c = self.ctx();
                        c.code.pop();
                        c.spans.pop();
                        // `n < 2`: an integer-literal right side stays in the instruction.
                        let k = match (&cond.kind, self.ctx().code.last().copied()) {
                            (ExprKind::Binary { rhs, .. }, Some(Op::Const(k)))
                                if matches!(rhs.kind, ExprKind::Int(_)) && matches!(self.consts[k], Value::Int(..)) =>
                            {
                                Some(k)
                            }
                            _ => None,
                        };
                        match k {
                            Some(k) => {
                                let c = self.ctx();
                                c.code.pop();
                                c.spans.pop();
                                let lhs_local = match &cond.kind {
                                    ExprKind::Binary { lhs, .. } if matches!(lhs.kind, ExprKind::Var(_)) => {
                                        match self.ctx().code.last().copied() {
                                            Some(Op::Local(s)) => Some(s),
                                            _ => None,
                                        }
                                    }
                                    _ => None,
                                };
                                match lhs_local {
                                    Some(s) => {
                                        let c = self.ctx();
                                        c.code.pop();
                                        c.spans.pop();
                                        self.emit(Op::LocalCmpJumpK(o, s as u32, k as u32, 0), cond.span)
                                    }
                                    None => self.emit(Op::CmpJumpK(o, k as u32, 0), cond.span),
                                }
                            }
                            None => self.emit(Op::CmpJump(o, 0), cond.span),
                        }
                    }
                    _ => self.emit(Op::JumpIfFalse(0), cond.span),
                };
                self.block(then, tail);
                let jend = if tail { None } else { Some(self.emit(Op::Jump(0), sp)) };
                self.patch(jf);
                match els {
                    Some(b) => self.block(b, tail),
                    None => {
                        self.emit(Op::Unit, sp);
                        if tail {
                            self.emit(Op::Return, sp);
                        }
                    }
                }
                if let Some(j) = jend {
                    self.patch(j);
                }
            }
            ExprKind::Match { scrutinee, arms } => self.match_expr(scrutinee, arms, tail, sp),
            ExprKind::Region(b) | ExprKind::Block(b) | ExprKind::Unsafe(b) => self.block(b, tail),
            ExprKind::With { arg, body, .. } => {
                self.expr(arg, false);
                self.emit(Op::Pop, sp);
                self.block(body, tail);
            }
            _ => {
                self.simple(e);
                if tail {
                    match (&e.kind, self.ctx().code.last().copied()) {
                        (ExprKind::Var(_), Some(Op::Local(s))) => {
                            let c = self.ctx();
                            c.code.pop();
                            c.spans.pop();
                            self.emit(Op::ReturnLocal(s as u32), sp);
                        }
                        _ => {
                            self.emit(Op::Return, sp);
                        }
                    }
                }
            }
        }
    }

    fn match_expr(&mut self, scrutinee: &Expr, arms: &[Arm], tail: bool, sp: Span) {
        self.expr(scrutinee, false);
        self.push_scope();
        let hidden = self.new_local("%scrutinee");
        self.emit(Op::SetLocal(hidden), scrutinee.span);
        let mut ends = Vec::new();
        for arm in arms {
            self.push_scope();
            let pat = self.pattern(&arm.pat);
            let pi = self.pats.len();
            self.pats.push(pat);
            let test = self.emit(Op::MatchPat { pat: pi as u32, slot: hidden as u32, fail: 0 }, arm.span);
            let guard = arm.guard.as_ref().map(|g| {
                self.expr(g, false);
                self.emit(Op::JumpIfFalse(0), g.span)
            });
            self.expr(&arm.body, tail);
            if !tail {
                ends.push(self.emit(Op::Jump(0), arm.span));
            }
            self.patch(test);
            if let Some(g) = guard {
                self.patch(g);
            }
            self.pop_scope();
        }
        self.emit(Op::MatchFail, sp);
        for j in ends {
            self.patch(j);
        }
        self.pop_scope();
    }

    fn pattern(&mut self, p: &Pattern) -> Pat {
        match &p.kind {
            PatKind::Wild => Pat::Wild,
            PatKind::Bind(n) => Pat::Bind(self.new_local(n)),
            PatKind::Int(s) => match parse_int(s) {
                Some(v) => Pat::Int(v),
                None => Pat::Wild,
            },
            PatKind::Float(s) => match parse_float(s) {
                Some(v) => Pat::Float(v),
                None => Pat::Wild,
            },
            PatKind::Str(s) => Pat::Str(Rc::from(s.as_str())),
            PatKind::Bool(b) => Pat::Bool(*b),
            PatKind::Ctor { name, args } => Pat::Ctor {
                name: Rc::from(name.name.as_str()),
                args: args.iter().map(|a| self.pattern(a)).collect(),
            },
            PatKind::Record { name, fields } => Pat::Record {
                ty: Rc::from(name.name.as_str()),
                fields: fields
                    .iter()
                    .map(|(f, sub)| {
                        let sub = match sub {
                            Some(s) => self.pattern(s),
                            None => Pat::Bind(self.new_local(&f.name)),
                        };
                        (Rc::from(f.name.as_str()), sub)
                    })
                    .collect(),
            },
            PatKind::Tuple(ps) => Pat::Tuple(ps.iter().map(|a| self.pattern(a)).collect()),
            PatKind::List { items, tail } => Pat::List {
                items: items.iter().map(|a| self.pattern(a)).collect(),
                tail: tail.as_ref().map(|t| Box::new(self.pattern(t))),
            },
        }
    }

    /// Expressions that never end a function by themselves.
    fn simple(&mut self, e: &Expr) {
        let sp = e.span;
        match &e.kind {
            ExprKind::While { cond, body } => {
                self.push_scope();
                let sp_slot = self.new_local("%sp");
                self.emit(Op::SaveSp(sp_slot), sp);
                let start = self.here();
                self.ctx().loops.push(LoopCx { sp: sp_slot, start, breaks: vec![] });
                self.expr(cond, false);
                let jf = self.emit(Op::JumpIfFalse(0), cond.span);
                self.block(body, false);
                self.emit(Op::Pop, sp);
                self.emit(Op::Jump(start), sp);
                self.patch(jf);
                let lp = self.ctx().loops.pop().unwrap();
                for j in lp.breaks {
                    self.patch(j);
                }
                self.emit(Op::Unit, sp);
                self.pop_scope();
            }
            ExprKind::For { var, iter, body } => {
                self.expr(iter, false);
                self.push_scope();
                let list = self.new_local("%list");
                let idx = self.new_local("%idx");
                let sp_slot = self.new_local("%sp");
                self.emit(Op::SetLocal(list), iter.span);
                let zero = self.constant(Value::Int(0, IntTy::I64));
                self.emit(Op::Const(zero), sp);
                self.emit(Op::SetLocal(idx), sp);
                self.emit(Op::SaveSp(sp_slot), sp);
                let start = self.here();
                self.ctx().loops.push(LoopCx { sp: sp_slot, start, breaks: vec![] });
                self.push_scope();
                let v = self.new_local(&var.name);
                let next = self.emit(Op::ForNext { list: list as u32, idx: idx as u32, var: v as u32, exit: 0 }, iter.span);
                self.block(body, false);
                self.emit(Op::Pop, sp);
                self.emit(Op::Jump(start), sp);
                self.pop_scope();
                self.patch(next);
                let lp = self.ctx().loops.pop().unwrap();
                for j in lp.breaks {
                    self.patch(j);
                }
                self.emit(Op::Unit, sp);
                self.pop_scope();
            }
            ExprKind::Break => {
                let slot = self.ctx().loops.last().map(|l| l.sp);
                match slot {
                    Some(slot) => {
                        let j = self.emit(Op::JumpSp(slot, 0), sp);
                        self.ctx().loops.last_mut().unwrap().breaks.push(j);
                    }
                    None => self.fail("`break` outside of a loop".to_string(), sp),
                }
            }
            ExprKind::Continue => {
                let l = self.ctx().loops.last().map(|l| (l.sp, l.start));
                match l {
                    Some((slot, start)) => {
                        self.emit(Op::JumpSp(slot, start), sp);
                    }
                    None => self.fail("`continue` outside of a loop".to_string(), sp),
                }
            }
            ExprKind::Int(s) => self.int_lit(s, sp),
            ExprKind::Float(s) => self.float_lit(s, sp),
            ExprKind::Str(s) => {
                let c = self.constant(Value::str(s));
                self.emit(Op::Const(c), sp);
            }
            ExprKind::Bool(b) => {
                let c = self.constant(Value::Bool(*b));
                self.emit(Op::Const(c), sp);
            }
            ExprKind::Unit => {
                self.emit(Op::Unit, sp);
            }
            ExprKind::Var(name) => self.var(name, sp),
            ExprKind::List(items) => {
                for it in items {
                    self.expr(it, false);
                }
                self.emit(Op::MakeList(items.len()), sp);
            }
            ExprKind::Cons { items, tail } => {
                for it in items {
                    self.expr(it, false);
                }
                self.expr(tail, false);
                self.emit(Op::MakeCons(items.len()), sp);
            }
            ExprKind::Tuple(items) => {
                for it in items {
                    self.expr(it, false);
                }
                self.emit(Op::MakeTuple(items.len()), sp);
            }
            ExprKind::Update { base, fields } => {
                self.expr(base, false);
                for (_, ex) in fields {
                    self.expr(ex, false);
                }
                self.records.push(RecordDesc {
                    ty: Rc::from(""),
                    names: fields.iter().map(|(n, _)| Rc::from(n.name.as_str())).collect(),
                    perm: (0..fields.len()).collect(),
                });
                let d = self.records.len() - 1;
                self.emit(Op::UpdateRecord(d), sp);
            }
            ExprKind::Record { name, fields } => {
                for (_, ex) in fields {
                    self.expr(ex, false);
                }
                let Some(order) = self.record_types.get(&name.name).cloned() else {
                    return self.fail(format!("`{}` is not a record type", name.name), name.span);
                };
                let mut perm = Vec::with_capacity(order.len());
                for f in &order {
                    match fields.iter().position(|(n, _)| &n.name == f) {
                        Some(i) => perm.push(i),
                        None => return self.fail(format!("missing field `{f}`"), sp),
                    }
                }
                self.records.push(RecordDesc {
                    ty: Rc::from(name.name.as_str()),
                    names: order.iter().map(|f| Rc::from(f.as_str())).collect(),
                    perm,
                });
                let d = self.records.len() - 1;
                self.emit(Op::MakeRecord(d), sp);
            }
            ExprKind::Unary { op: UnOp::Neg, expr } if matches!(expr.kind, ExprKind::Int(_)) && self.neg_lit_ok(expr.span) => {
                let ExprKind::Int(s) = &expr.kind else { unreachable!() };
                let ty = self.lit_types.get(&(expr.span.start, expr.span.end)).and_then(|t| IntTy::from_name(t)).unwrap_or(IntTy::I32);
                match parse_int(s) {
                    Some(v) if ty.fits(-v) => {
                        let c = self.constant(Value::Int(-v, ty));
                        self.emit(Op::Const(c), sp);
                    }
                    _ => self.fail(format!("integer literal `-{s}` does not fit in `{}`", ty.name()), expr.span),
                }
            }
            ExprKind::Unary { op, expr } => {
                self.expr(expr, false);
                self.emit(match op { UnOp::Neg => Op::Neg, UnOp::Not => Op::Not, UnOp::BitNot => Op::BitNot }, sp);
            }
            ExprKind::Binary { op, lhs, rhs } => match op {
                BinOp::And | BinOp::Or => {
                    self.expr(lhs, false);
                    let j = self.emit(
                        if *op == BinOp::And { Op::AndJump(0) } else { Op::OrJump(0) },
                        lhs.span,
                    );
                    self.expr(rhs, false);
                    self.emit(Op::ExpectBool, rhs.span);
                    self.patch(j);
                }
                _ => {
                    self.expr(lhs, false);
                    self.expr(rhs, false);
                    // `x + 1`: fold an integer-literal right side into the instruction.
                    if matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul) {
                        if let (ExprKind::Int(_), Some(Op::Const(k))) = (&rhs.kind, self.ctx().code.last().copied()) {
                            if matches!(self.consts[k], Value::Int(..)) {
                                let c = self.ctx();
                                c.code.pop();
                                c.spans.pop();
                                if let (ExprKind::Var(_), Some(Op::Local(s))) = (&lhs.kind, self.ctx().code.last().copied()) {
                                    let c = self.ctx();
                                    c.code.pop();
                                    c.spans.pop();
                                    self.emit(Op::LocalArithK(*op, s as u32, k as u32), sp);
                                } else {
                                    self.emit(Op::ArithK(*op, k), sp);
                                }
                                return;
                            }
                        }
                    }
                    self.emit(
                        match op {
                            BinOp::Eq => Op::Eq,
                            BinOp::NotEq => Op::NotEq,
                            BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => Op::Cmp(*op),
                            _ => Op::Arith(*op),
                        },
                        sp,
                    );
                }
            },
            ExprKind::Field { expr, name } => {
                if let Some(chain) = flatten(e) {
                    if !self.has_local(chain[0])
                        && !self.fn_ids.contains_key(chain[0])
                        && !self.const_ids.contains_key(chain[0])
                        && !self.ctors.contains_key(chain[0])
                    {
                        return self.module_member(&chain, sp);
                    }
                }
                self.expr(expr, false);
                let n = self.name_id(&name.name);
                self.emit(Op::Field(n), name.span);
            }
            ExprKind::Index { expr, index } => {
                self.expr(expr, false);
                self.expr(index, false);
                self.emit(Op::Index, sp);
            }
            ExprKind::Try(inner) => {
                self.expr(inner, false);
                self.emit(Op::Try, sp);
            }
            ExprKind::Lambda { params, body } => {
                let names: Vec<String> = params.iter().map(|p| p.name.name.clone()).collect();
                self.closure("lambda", names, sp, |c| c.expr(body, true));
            }
            // Handled in `expr`.
            ExprKind::Call { .. }
            | ExprKind::Pipe { .. }
            | ExprKind::If { .. }
            | ExprKind::Match { .. }
            | ExprKind::Region(_)
            | ExprKind::With { .. }
            | ExprKind::Block(_)
            | ExprKind::Unsafe(_) => unreachable!("handled in `expr`"),
        }
    }
}
