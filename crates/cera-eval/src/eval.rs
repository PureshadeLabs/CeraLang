//! The evaluator.

use crate::rt::{make_caps, Host, Rt};
use crate::value::*;
use cera_sema::Analysis;
use cera_syntax::ast::*;
use cera_syntax::Span;
use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::io::Write;
use std::rc::Rc;

/// Stack size of the interpreter thread. Non-tail recursion is limited by this
/// (the interpreter reports a Cera stack overflow before the real one).
#[cfg(not(target_arch = "wasm32"))]
pub const STACK_BYTES: usize = 512 << 20;
#[cfg(not(target_arch = "wasm32"))]
const STACK_MARGIN: usize = 32 << 20;
/// The wasm linear-memory stack is set by the linker (see `.cargo/config.toml`).
#[cfg(target_arch = "wasm32")]
pub const STACK_BYTES: usize = 16 << 20;
#[cfg(target_arch = "wasm32")]
const STACK_MARGIN: usize = 2 << 20;

pub enum Tail<'a> {
    Done(Value<'a>),
    Call(Value<'a>, Vec<Value<'a>>, Span),
}

pub struct Interp<'a> {
    pub(crate) fns: HashMap<String, &'a FnDef>,
    consts: HashMap<String, &'a Expr>,
    const_cache: RefCell<HashMap<String, Value<'a>>>,
    ctors: HashMap<String, (Rc<str>, usize)>,
    records: HashMap<String, Vec<String>>,
    trait_methods: HashMap<String, Rc<str>>,
    pub(crate) impls: HashMap<(String, String), HashMap<String, &'a FnDef>>,
    aliases: HashMap<String, String>,
    imported: Vec<String>,
    lit_types: HashMap<(usize, usize), String>,
    root: Rc<Env<'a>>,
    pub(crate) host: Host,
    stack_base: Cell<usize>,
}

pub fn type_head_ast(t: &Type) -> String {
    match &t.kind {
        TypeKind::Path { path, .. } => path.last().map(|i| i.name.clone()).unwrap_or_default(),
        TypeKind::List(_) => "[]".to_string(),
        TypeKind::Tuple(v) => format!("tuple{}", v.len()),
        TypeKind::Fn { .. } => "fn".to_string(),
        TypeKind::Ref(inner) => type_head_ast(inner),
    }
}

pub fn parse_int(s: &str) -> Option<i128> {
    let s: String = s.chars().filter(|c| *c != '_').collect();
    if let Some(h) = s.strip_prefix("0x") {
        i128::from_str_radix(h, 16).ok()
    } else if let Some(b) = s.strip_prefix("0b") {
        i128::from_str_radix(b, 2).ok()
    } else if let Some(o) = s.strip_prefix("0o") {
        i128::from_str_radix(o, 8).ok()
    } else {
        s.parse().ok()
    }
}

pub(crate) fn parse_float(s: &str) -> Option<f64> {
    s.chars().filter(|c| *c != '_').collect::<String>().parse().ok()
}

pub(crate) const PRELUDE_CTORS: &[(&str, &str, usize)] = &[
    ("Some", "Option", 1),
    ("None", "Option", 0),
    ("Ok", "Result", 1),
    ("Err", "Result", 1),
    ("Less", "Ordering", 0),
    ("Equal", "Ordering", 0),
    ("Greater", "Ordering", 0),
];

pub(crate) const PRELUDE_TRAITS: &[(&str, &str)] = &[
    ("show", "Show"),
    ("compare", "Ord"),
    ("add", "Add"),
    ("sub", "Sub"),
    ("mul", "Mul"),
    ("div", "Div"),
    ("rem", "Rem"),
    ("neg", "Neg"),
];

impl<'a> Interp<'a> {
    pub fn new(program: &'a Program, analysis: &Analysis, host: Host) -> Interp<'a> {
        let mut it = Interp {
            fns: HashMap::new(),
            consts: HashMap::new(),
            const_cache: RefCell::new(HashMap::new()),
            ctors: HashMap::new(),
            records: HashMap::new(),
            trait_methods: HashMap::new(),
            impls: HashMap::new(),
            aliases: HashMap::new(),
            imported: Vec::new(),
            lit_types: HashMap::new(),
            root: Env::root(),
            host,
            stack_base: Cell::new(0),
        };
        for (c, ty, arity) in PRELUDE_CTORS {
            it.ctors.insert(c.to_string(), (Rc::from(*ty), *arity));
        }
        for (m, t) in PRELUDE_TRAITS {
            it.trait_methods.insert(m.to_string(), Rc::from(*t));
        }
        for t in &analysis.types {
            it.lit_types
                .entry((t.span.start, t.span.end))
                .or_insert_with(|| t.text.clone());
        }
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
                    it.aliases.insert(alias, key.clone());
                    it.imported.push(key);
                }
                Item::Impl(im) => {
                    let head = type_head_ast(&im.ty);
                    let entry = it
                        .impls
                        .entry((im.trait_name.name.clone(), head))
                        .or_default();
                    for m in &im.methods {
                        if let DeclKind::Fn(f) = &m.kind {
                            entry.insert(m.name.name.clone(), f);
                        }
                    }
                }
                Item::Decl(d) => match &d.kind {
                    DeclKind::Fn(f) => {
                        it.fns.entry(d.name.name.clone()).or_insert(f);
                    }
                    DeclKind::Const(e) => {
                        it.consts.entry(d.name.name.clone()).or_insert(e);
                    }
                    DeclKind::Trait(t) => {
                        for m in &t.methods {
                            it.trait_methods
                                .insert(m.name.name.clone(), Rc::from(d.name.name.as_str()));
                        }
                    }
                    DeclKind::Type(TypeDef::Variants(vs)) => {
                        for v in vs {
                            it.ctors.insert(
                                v.name.name.clone(),
                                (Rc::from(d.name.name.as_str()), v.fields.len()),
                            );
                        }
                    }
                    DeclKind::Type(TypeDef::Record(fs)) => {
                        it.records.insert(
                            d.name.name.clone(),
                            fs.iter().map(|f| f.name.name.clone()).collect(),
                        );
                    }
                    DeclKind::Type(TypeDef::Opaque) => {}
                },
            }
        }
        it
    }

    /// Run `main`. Returns the process exit code.
    pub fn run_main(&self) -> Result<i32, RuntimeError> {
        let Some(main) = self.fns.get("main") else {
            return Err(RuntimeError::new(None, "this program has no `main` function"));
        };
        let args = match main.params.len() {
            0 => vec![],
            1 => vec![make_caps(&self.host)],
            _ => return Err(RuntimeError::new(None, "`main` takes no arguments or just `caps: Caps`")),
        };
        let span = Span::new(0, 0);
        let marker = 0u8;
        self.stack_base.set(&marker as *const u8 as usize);
        let result = self.apply(Value::FnItem(Rc::from("main")), args, span);
        let _ = self.host.out.borrow_mut().flush();
        let _ = self.host.err.borrow_mut().flush();
        match result {
            Ok(Value::Ctor { tag, fields }) if &*tag.name == "Err" => {
                let shown = self
                    .show_value(&fields[0], false)
                    .unwrap_or_else(|_| "<error>".to_string());
                Err(RuntimeError::new(None, format!("main returned an error: {shown}")))
            }
            Ok(_) => Ok(0),
            Err(Ctl::Error(e)) => Err(e),
            Err(Ctl::Return(_)) => Ok(0),
            Err(Ctl::Exit(c)) => Ok(c),
            Err(Ctl::Break) | Err(Ctl::Continue) => Ok(0),
        }
    }

    // ----- literals -------------------------------------------------------------

    /// The integer type of the literal at `span`, if it is an integer literal.
    fn neg_lit_type(&self, span: Span) -> Option<IntTy> {
        match self.lit_types.get(&(span.start, span.end)) {
            Some(t) => IntTy::from_name(t),
            None => Some(IntTy::I32),
        }
    }

    fn int_lit(&self, s: &str, span: Span) -> Res<'a, Value<'a>> {
        let v = match parse_int(s) {
            Some(v) => v,
            None => return rt_err(span, format!("invalid integer literal `{s}`")),
        };
        let ty = self
            .lit_types
            .get(&(span.start, span.end))
            .and_then(|t| IntTy::from_name(t))
            .unwrap_or(IntTy::I32);
        if !ty.fits(v) {
            return rt_err(span, format!("integer literal `{s}` does not fit in `{}`", ty.name()));
        }
        Ok(Value::Int(v, ty))
    }

    fn float_lit(&self, s: &str, span: Span) -> Res<'a, Value<'a>> {
        let v = match parse_float(s) {
            Some(v) => v,
            None => return rt_err(span, format!("invalid float literal `{s}`")),
        };
        let f32 = self
            .lit_types
            .get(&(span.start, span.end))
            .is_some_and(|t| t == "f32");
        Ok(Value::Float(if f32 { v as f32 as f64 } else { v }, f32))
    }

    // ----- names ----------------------------------------------------------------

    fn lookup_var(&self, name: &str, env: &Rc<Env<'a>>, span: Span) -> Res<'a, Value<'a>> {
        if let Some(v) = env.get(name) {
            return Ok(v);
        }
        if self.fns.contains_key(name) {
            return Ok(Value::FnItem(Rc::from(name)));
        }
        if let Some(e) = self.consts.get(name) {
            if let Some(v) = self.const_cache.borrow().get(name) {
                return Ok(v.clone());
            }
            let v = self.eval(e, &self.root)?;
            self.const_cache.borrow_mut().insert(name.to_string(), v.clone());
            return Ok(v);
        }
        if let Some((ty, arity)) = self.ctors.get(name) {
            return Ok(if *arity == 0 {
                Value::Ctor {
                    tag: tag_of(ty, name),
                    fields: Rc::new(vec![]),
                }
            } else {
                Value::CtorFn {
                    tag: tag_of(ty, name),
                    arity: *arity,
                }
            });
        }
        if let Some(t) = self.trait_methods.get(name) {
            return Ok(Value::trait_method(t, name));
        }
        if let Some(n) = crate::native::prelude_native(name) {
            return Ok(Value::Native(n));
        }
        if name == "asm" {
            return rt_err(span, "`asm` needs the native backend (run with `--native`, or build an executable)");
        }
        rt_err(span, format!("unknown name `{name}`"))
    }

    fn module_member(&self, chain: &[&str], span: Span) -> Res<'a, Value<'a>> {
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
                    _ => return rt_err(span, format!("unknown module `{joined}`")),
                },
            }
        } else if self.imported.contains(&joined) {
            joined.clone()
        } else {
            return rt_err(span, format!("unknown module `{joined}`"));
        };
        match crate::native::module_native(&key, member[0]) {
            Some(n) => Ok(Value::Native(n)),
            None => rt_err(span, format!("module `{key}` has no member `{}`", member[0])),
        }
    }

    // ----- evaluation -----------------------------------------------------------

    pub fn eval(&self, e: &'a Expr, env: &Rc<Env<'a>>) -> Res<'a, Value<'a>> {
        match self.eval_tail(e, env)? {
            Tail::Done(v) => Ok(v),
            Tail::Call(f, args, span) => self.apply(f, args, span),
        }
    }

    /// One iteration of a loop body. `false` means `break`.
    fn loop_body(&self, body: &'a Block, env: &Rc<Env<'a>>) -> Res<'a, bool> {
        match self.eval_block_tail(body, env) {
            Ok(Tail::Done(_)) => Ok(true),
            Ok(Tail::Call(f, args, span)) => self.apply(f, args, span).map(|_| true),
            Err(Ctl::Break) => Ok(false),
            Err(Ctl::Continue) => Ok(true),
            Err(e) => Err(e),
        }
    }

    fn eval_block_tail(&self, b: &'a Block, env: &Rc<Env<'a>>) -> Res<'a, Tail<'a>> {
        let scope = Env::child(env);
        let has_defer = b.stmts.iter().any(|s| matches!(s, Stmt::Defer(..)));
        let mut defers: Vec<(&'a Expr, Rc<Env<'a>>)> = Vec::new();
        let result = self.run_stmts(b, &scope, &mut defers, has_defer);
        let mut result = result;
        for (d, dscope) in defers.iter().rev() {
            if let Err(Ctl::Error(e)) = self.eval(d, dscope) {
                if result.is_ok() {
                    result = Err(Ctl::Error(e));
                }
            }
        }
        result
    }

    fn run_stmts(
        &self,
        b: &'a Block,
        scope: &Rc<Env<'a>>,
        defers: &mut Vec<(&'a Expr, Rc<Env<'a>>)>,
        has_defer: bool,
    ) -> Res<'a, Tail<'a>> {
        // Each `let` opens a new scope, so a closure made earlier keeps seeing the
        // binding it was made with even when a later `let` shadows the name.
        let mut scope = scope.clone();
        let scope = &mut scope;
        let n = b.stmts.len();
        let mut last = Tail::Done(Value::Unit);
        for (i, s) in b.stmts.iter().enumerate() {
            match s {
                Stmt::Let { name, value, .. } => {
                    let v = self.eval(value, scope)?;
                    let next = Env::child(scope);
                    next.bind(&name.name, v);
                    *scope = next;
                    last = Tail::Done(Value::Unit);
                }
                Stmt::Assign { name, value, .. } => {
                    let v = self.eval(value, scope)?;
                    scope.set(&name.name, v);
                    last = Tail::Done(Value::Unit);
                }
                Stmt::Defer(e, _) => {
                    defers.push((e, scope.clone()));
                    last = Tail::Done(Value::Unit);
                }
                Stmt::Expr(e) => {
                    if i == n - 1 && !has_defer {
                        last = self.eval_tail(e, scope)?;
                    } else {
                        last = Tail::Done(self.eval(e, scope)?);
                    }
                }
            }
        }
        Ok(last)
    }

    fn eval_args(&self, args: &'a [Expr], env: &Rc<Env<'a>>, first: Option<Value<'a>>) -> Res<'a, Vec<Value<'a>>> {
        let mut vs = Vec::with_capacity(args.len() + 1);
        vs.extend(first);
        for a in args {
            vs.push(self.eval(a, env)?);
        }
        Ok(vs)
    }

    pub fn eval_tail(&self, e: &'a Expr, env: &Rc<Env<'a>>) -> Res<'a, Tail<'a>> {
        match &e.kind {
            ExprKind::Call { callee, args } => {
                let f = self.eval(callee, env)?;
                let vs = self.eval_args(args, env, None)?;
                Ok(Tail::Call(f, vs, e.span))
            }
            ExprKind::Pipe { lhs, rhs } => {
                let first = self.eval(lhs, env)?;
                match &rhs.kind {
                    ExprKind::Call { callee, args } => {
                        let f = self.eval(callee, env)?;
                        let vs = self.eval_args(args, env, Some(first))?;
                        Ok(Tail::Call(f, vs, e.span))
                    }
                    _ => {
                        let f = self.eval(rhs, env)?;
                        Ok(Tail::Call(f, vec![first], e.span))
                    }
                }
            }
            ExprKind::If { cond, then, els } => match self.eval(cond, env)? {
                Value::Bool(true) => self.eval_block_tail(then, env),
                Value::Bool(false) => match els {
                    Some(b) => self.eval_block_tail(b, env),
                    None => Ok(Tail::Done(Value::Unit)),
                },
                _ => rt_err(cond.span, "condition is not a bool"),
            },
            ExprKind::While { cond, body } => {
                loop {
                    match self.eval(cond, env)? {
                        Value::Bool(true) => {}
                        Value::Bool(false) => break,
                        _ => return rt_err(cond.span, "condition is not a bool"),
                    }
                    if !self.loop_body(body, env)? {
                        break;
                    }
                }
                Ok(Tail::Done(Value::Unit))
            }
            ExprKind::For { var, iter, body } => {
                let items = self.eval(iter, env)?;
                let Some(items) = items.list_items() else {
                    return rt_err(iter.span, "`for` needs a list");
                };
                let items: Vec<Value<'a>> = items.to_vec();
                for item in items {
                    let scope = Env::child(env);
                    scope.bind(&var.name, item);
                    if !self.loop_body(body, &scope)? {
                        break;
                    }
                }
                Ok(Tail::Done(Value::Unit))
            }
            ExprKind::Break => Err(Ctl::Break),
            ExprKind::Continue => Err(Ctl::Continue),
            ExprKind::Match { scrutinee, arms } => {
                let v = self.eval(scrutinee, env)?;
                for arm in arms {
                    let mut binds = Vec::new();
                    if !self.match_pat(&arm.pat, &v, &mut binds) {
                        continue;
                    }
                    let scope = Env::child(env);
                    for (n, b) in binds {
                        scope.bind(&n, b);
                    }
                    if let Some(g) = &arm.guard {
                        match self.eval(g, &scope)? {
                            Value::Bool(true) => {}
                            _ => continue,
                        }
                    }
                    return self.eval_tail(&arm.body, &scope);
                }
                rt_err(e.span, "no match arm matched the value")
            }
            ExprKind::Region(b) | ExprKind::Block(b) | ExprKind::Unsafe(b) => self.eval_block_tail(b, env),
            ExprKind::With { arg, body, .. } => {
                self.eval(arg, env)?;
                self.eval_block_tail(body, env)
            }
            _ => Ok(Tail::Done(self.eval_simple(e, env)?)),
        }
    }

    fn eval_simple(&self, e: &'a Expr, env: &Rc<Env<'a>>) -> Res<'a, Value<'a>> {
        match &e.kind {
            ExprKind::Int(s) => self.int_lit(s, e.span),
            ExprKind::Float(s) => self.float_lit(s, e.span),
            ExprKind::Str(s) => Ok(Value::str(s)),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),
            ExprKind::Unit => Ok(Value::Unit),
            ExprKind::Var(name) => self.lookup_var(name, env, e.span),
            ExprKind::List(items) => {
                let mut vs = Vec::with_capacity(items.len());
                for it in items {
                    vs.push(self.eval(it, env)?);
                }
                Ok(Value::list(vs))
            }
            ExprKind::Cons { items, tail } => {
                let mut vs = Vec::new();
                for it in items {
                    vs.push(self.eval(it, env)?);
                }
                let t = self.eval(tail, env)?;
                match t.list_items() {
                    Some(rest) => {
                        vs.extend(rest.iter().cloned());
                        Ok(Value::list(vs))
                    }
                    None => rt_err(tail.span, "the tail of a list must be a list"),
                }
            }
            ExprKind::Tuple(items) => {
                let mut vs = Vec::with_capacity(items.len());
                for it in items {
                    vs.push(self.eval(it, env)?);
                }
                Ok(Value::Tuple(Rc::new(vs)))
            }
            ExprKind::Update { base, fields } => {
                let base_v = self.eval(base, env)?;
                let mut new = Vec::new();
                for (n, ex) in fields {
                    new.push((n.name.clone(), self.eval(ex, env)?));
                }
                let Value::Record { ty, fields: old } = base_v.clone() else {
                    return rt_err(e.span, "`{value | field: ..}` needs a record");
                };
                let mut out = (*old).clone();
                for (n, v) in new {
                    match out.iter_mut().find(|(f, _)| &**f == n.as_str()) {
                        Some(slot) => slot.1 = v,
                        None => return rt_err(e.span, format!("no field `{n}`")),
                    }
                }
                Ok(Value::Record { ty, fields: Rc::new(out) })
            }
            ExprKind::Record { name, fields } => {
                let mut given: Vec<(String, Value<'a>)> = Vec::new();
                for (n, ex) in fields {
                    given.push((n.name.clone(), self.eval(ex, env)?));
                }
                let order = match self.records.get(&name.name) {
                    Some(o) => o,
                    None => return rt_err(name.span, format!("`{}` is not a record type", name.name)),
                };
                let mut out = Vec::with_capacity(order.len());
                for f in order {
                    match given.iter().position(|(n, _)| n == f) {
                        Some(i) => out.push((Rc::from(f.as_str()), given.swap_remove(i).1)),
                        None => return rt_err(e.span, format!("missing field `{f}`")),
                    }
                }
                Ok(Value::Record {
                    ty: Rc::from(name.name.as_str()),
                    fields: Rc::new(out),
                })
            }
            ExprKind::Unary { op: UnOp::Neg, expr } if matches!(expr.kind, ExprKind::Int(_)) && self.neg_lit_type(expr.span).is_some() => {
                // `-128` is a literal of its own, so it may be the smallest value of its type.
                let ExprKind::Int(s) = &expr.kind else { unreachable!() };
                let ty = self.neg_lit_type(expr.span).unwrap();
                match parse_int(s) {
                    Some(v) if ty.fits(-v) => Ok(Value::Int(-v, ty)),
                    _ => rt_err(expr.span, format!("integer literal `-{s}` does not fit in `{}`", ty.name())),
                }
            }
            ExprKind::Unary { op, expr } => {
                let v = self.eval(expr, env)?;
                match (op, v) {
                    (UnOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
                    (UnOp::BitNot, v) => crate::native::call_native(self, "bits.bnot", vec![v], e.span),
                    (UnOp::Neg, Value::Int(x, t)) => {
                        if t.fits(-x) {
                            Ok(Value::Int(-x, t))
                        } else {
                            rt_err(e.span, "integer overflow")
                        }
                    }
                    (UnOp::Neg, Value::Float(x, f)) => Ok(Value::Float(-x, f)),
                    (UnOp::Neg, other) => {
                        self.apply(Value::trait_method("Neg", "neg"), vec![other], e.span)
                    }
                    _ => rt_err(e.span, "bad operand for unary operator"),
                }
            }
            ExprKind::Binary { op, lhs, rhs } => self.eval_binary(*op, lhs, rhs, env, e.span),
            ExprKind::Field { expr, name } => {
                if let Some(chain) = flatten(e) {
                    if env.get(chain[0]).is_none()
                        && !self.fns.contains_key(chain[0])
                        && !self.consts.contains_key(chain[0])
                        && !self.ctors.contains_key(chain[0])
                    {
                        return self.module_member(&chain, e.span);
                    }
                }
                match self.eval(expr, env)? {
                    Value::Record { fields, .. } => {
                        match fields.iter().find(|(n, _)| **n == *name.name) {
                            Some((_, v)) => Ok(v.clone()),
                            None => rt_err(name.span, format!("no field `{}`", name.name)),
                        }
                    }
                    _ => rt_err(name.span, format!("no field `{}` on this value", name.name)),
                }
            }
            ExprKind::Index { expr, index } => {
                let xs = self.eval(expr, env)?;
                let i = self.eval(index, env)?;
                if let (Value::Array(a), Value::Int(i, _)) = (&xs, &i) {
                    let a = a.borrow();
                    return if *i >= 0 && (*i as usize) < a.len() {
                        Ok(a[*i as usize].clone())
                    } else {
                        rt_err(e.span, format!("index {i} is out of range for an array of length {}", a.len()))
                    };
                }
                match (xs.list_items(), i) {
                    (Some(items), Value::Int(i, _)) => {
                        if i >= 0 && (i as usize) < items.len() {
                            Ok(items[i as usize].clone())
                        } else {
                            rt_err(
                                e.span,
                                format!("index {i} is out of range for a list of length {}", items.len()),
                            )
                        }
                    }
                    _ => rt_err(e.span, "cannot index this value"),
                }
            }
            ExprKind::Try(inner) => {
                let v = self.eval(inner, env)?;
                match &v {
                    Value::Ctor { tag, fields } => match &*tag.name {
                        "Some" | "Ok" => Ok(fields[0].clone()),
                        "None" | "Err" => Err(Ctl::Return(v.clone())),
                        _ => rt_err(e.span, "`?` needs an Option or a Result"),
                    },
                    _ => rt_err(e.span, "`?` needs an Option or a Result"),
                }
            }
            ExprKind::Lambda { params, body } => Ok(Value::Closure(Rc::new(ClosureVal {
                params,
                body,
                env: env.clone(),
            }))),
            // Handled in `eval_tail`.
            _ => unreachable!("tail expression kinds are handled in eval_tail"),
        }
    }

    fn eval_binary(
        &self,
        op: BinOp,
        lhs: &'a Expr,
        rhs: &'a Expr,
        env: &Rc<Env<'a>>,
        span: Span,
    ) -> Res<'a, Value<'a>> {
        match op {
            BinOp::And | BinOp::Or => {
                let l = self.eval(lhs, env)?;
                let Value::Bool(l) = l else {
                    return rt_err(lhs.span, "operand is not a bool");
                };
                if (op == BinOp::And && !l) || (op == BinOp::Or && l) {
                    return Ok(Value::Bool(l));
                }
                match self.eval(rhs, env)? {
                    Value::Bool(r) => Ok(Value::Bool(r)),
                    _ => rt_err(rhs.span, "operand is not a bool"),
                }
            }
            BinOp::Eq | BinOp::NotEq => {
                let l = self.eval(lhs, env)?;
                let r = self.eval(rhs, env)?;
                let eq = self.values_eq(&l, &r, span)?;
                Ok(Value::Bool(if op == BinOp::Eq { eq } else { !eq }))
            }
            BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
                let l = self.eval(lhs, env)?;
                let r = self.eval(rhs, env)?;
                let ord = self.compare(&l, &r, span)?;
                Ok(Value::Bool(match ord {
                    None => false,
                    Some(o) => match op {
                        BinOp::Lt => o == Ordering::Less,
                        BinOp::LtEq => o != Ordering::Greater,
                        BinOp::Gt => o == Ordering::Greater,
                        _ => o != Ordering::Less,
                    },
                }))
            }
            _ => {
                let l = self.eval(lhs, env)?;
                let r = self.eval(rhs, env)?;
                self.arith(op, l, r, span)
            }
        }
    }

    // ----- arithmetic and comparison (shared with the other engines) -------------

    pub(crate) fn arith(&self, op: BinOp, l: Value<'a>, r: Value<'a>, span: Span) -> Res<'a, Value<'a>> {
        crate::rt::arith(self, op, l, r, span)
    }

    /// `None` means "unordered" (a NaN was involved).
    pub(crate) fn compare(&self, a: &Value<'a>, b: &Value<'a>, span: Span) -> Res<'a, Option<Ordering>> {
        crate::rt::compare(self, a, b, span)
    }

    pub(crate) fn values_eq(&self, a: &Value<'a>, b: &Value<'a>, span: Span) -> Res<'a, bool> {
        crate::rt::values_eq(self, a, b, span)
    }

    pub fn show_value(&self, v: &Value<'a>, nested: bool) -> Res<'a, String> {
        crate::rt::show_value(self, v, nested)
    }

    // ----- patterns -------------------------------------------------------------

    fn match_pat(&self, p: &'a Pattern, v: &Value<'a>, binds: &mut Vec<(String, Value<'a>)>) -> bool {
        match &p.kind {
            PatKind::Wild => true,
            PatKind::Bind(n) => {
                binds.push((n.clone(), v.clone()));
                true
            }
            PatKind::Int(s) => matches!((v, parse_int(s)), (Value::Int(x, _), Some(y)) if *x == y),
            PatKind::Float(s) => {
                matches!((v, parse_float(s)), (Value::Float(x, _), Some(y)) if *x == y)
            }
            PatKind::Str(s) => matches!(v, Value::Str(x) if **x == **s),
            PatKind::Bool(b) => matches!(v, Value::Bool(x) if x == b),
            PatKind::Ctor { name, args } => match v {
                Value::Ctor { tag: n, fields } => {
                    *n.name == *name.name
                        && args.len() == fields.len()
                        && args.iter().zip(fields.iter()).all(|(a, f)| self.match_pat(a, f, binds))
                }
                Value::Ref(inner) => self.match_pat(p, inner, binds),
                _ => false,
            },
            PatKind::Record { name, fields: fps } => match v {
                Value::Record { ty, fields } if **ty == *name.name => {
                    for (fname, fp) in fps {
                        let Some((_, fv)) = fields.iter().find(|(n, _)| **n == *fname.name) else {
                            return false;
                        };
                        match fp {
                            Some(sub) => {
                                if !self.match_pat(sub, fv, binds) {
                                    return false;
                                }
                            }
                            None => binds.push((fname.name.clone(), fv.clone())),
                        }
                    }
                    true
                }
                Value::Ref(inner) => self.match_pat(p, inner, binds),
                _ => false,
            },
            PatKind::Tuple(ps) => match v {
                Value::Tuple(vs) => {
                    ps.len() == vs.len() && ps.iter().zip(vs.iter()).all(|(a, f)| self.match_pat(a, f, binds))
                }
                Value::Unit => ps.is_empty(),
                _ => false,
            },
            PatKind::List { items, tail } => {
                let Some(xs) = v.list_items() else { return false };
                if xs.len() < items.len() || (tail.is_none() && xs.len() != items.len()) {
                    return false;
                }
                if !items.iter().zip(xs.iter()).all(|(a, f)| self.match_pat(a, f, binds)) {
                    return false;
                }
                match tail {
                    None => true,
                    Some(t) => {
                        let rest = match v {
                            Value::List(rc, off) => Value::List(rc.clone(), off + items.len()),
                            Value::Ref(inner) => match &**inner {
                                Value::List(rc, off) => Value::List(rc.clone(), off + items.len()),
                                _ => return false,
                            },
                            _ => return false,
                        };
                        self.match_pat(t, &rest, binds)
                    }
                }
            }
        }
    }

    // ----- calls ----------------------------------------------------------------

    fn enter_fn(&self, f: &'a FnDef, args: Vec<Value<'a>>, span: Span) -> Res<'a, Tail<'a>> {
        let Some(body) = &f.body else {
            return rt_err(span, "this function has no body");
        };
        if f.params.len() != args.len() {
            return rt_err(
                span,
                format!("expected {} argument(s), got {}", f.params.len(), args.len()),
            );
        }
        let env = Env::child(&self.root);
        for (p, a) in f.params.iter().zip(args) {
            env.bind(&p.name.name, a);
        }
        self.eval_block_tail(body, &env)
    }

    pub fn apply(&self, f: Value<'a>, args: Vec<Value<'a>>, span: Span) -> Res<'a, Value<'a>> {
        let marker = 0u8;
        let here = &marker as *const u8 as usize;
        if self.stack_base.get().saturating_sub(here) > STACK_BYTES - STACK_MARGIN {
            return rt_err(span, "stack overflow: recursion is too deep (only tail calls are free)");
        }
        match self.apply_loop(f, args, span) {
            Err(Ctl::Return(v)) => Ok(v),
            other => other,
        }
    }

    fn apply_loop(&self, mut f: Value<'a>, mut args: Vec<Value<'a>>, mut span: Span) -> Res<'a, Value<'a>> {
        loop {
            let step = match f {
                Value::FnItem(name) => {
                    let Some(fd) = self.fns.get(&*name) else {
                        return rt_err(span, format!("unknown function `{name}`"));
                    };
                    self.enter_fn(fd, args, span)
                }
                Value::Closure(c) => {
                    let (params, body, env) = (c.params, c.body, &c.env);
                    if params.len() != args.len() {
                        return rt_err(
                            span,
                            format!("expected {} argument(s), got {}", params.len(), args.len()),
                        );
                    }
                    let scope = Env::child(env);
                    for (p, a) in params.iter().zip(args) {
                        scope.bind(&p.name.name, a);
                    }
                    self.eval_tail(body, &scope)
                }
                Value::CtorFn { tag, arity } => {
                    if arity != args.len() {
                        return rt_err(
                            span,
                            format!("`{}` takes {arity} argument(s), got {}", tag.name, args.len()),
                        );
                    }
                    return Ok(Value::Ctor {
                        tag,
                        fields: Rc::new(args),
                    });
                }
                Value::TraitMethod(tr) => {
                    let (trait_name, method) = (&tr.trait_name, &tr.method);
                    let Some(first) = args.first() else {
                        return rt_err(span, format!("`{method}` needs a receiver argument"));
                    };
                    let head = first.head();
                    let user = self
                        .impls
                        .get(&(trait_name.to_string(), head))
                        .and_then(|m| m.get(&**method))
                        .copied();
                    match user {
                        Some(fd) => self.enter_fn(fd, args, span),
                        None => return crate::native::native_method(self, &method, args, span),
                    }
                }
                Value::Native(name) => return crate::native::call_native(self, name, args, span),
                Value::Ref(inner) => {
                    f = (*inner).clone();
                    continue;
                }
                _ => return rt_err(span, "this value is not a function"),
            };
            match step {
                Ok(Tail::Done(v)) => return Ok(v),
                Ok(Tail::Call(f2, a2, s2)) => {
                    f = f2;
                    args = a2;
                    span = s2;
                }
                Err(Ctl::Return(v)) => return Ok(v),
                Err(e) => return Err(e),
            }
        }
    }

}

impl<'a> Rt<'a> for Interp<'a> {
    fn apply(&self, f: Value<'a>, args: Vec<Value<'a>>, span: Span) -> Res<'a, Value<'a>> {
        Interp::apply(self, f, args, span)
    }

    fn has_impl(&self, trait_name: &str, head: &str) -> bool {
        self.impls.contains_key(&(trait_name.to_string(), head.to_string()))
    }

    fn host(&self) -> &Host {
        &self.host
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
