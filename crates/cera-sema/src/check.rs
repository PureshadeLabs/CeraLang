//! Name resolution and type inference.
//!
//! The checker works in three phases: collect module-level declarations
//! (types, constructors, function signatures), check every body with
//! Hindley-Milner style unification, then run the checks that need fully
//! resolved types (literal defaulting, operator constraints, exhaustiveness).

use crate::defs::*;
use crate::exhaust::{Con, Ctx, DPat};
use crate::ty::*;
use cera_syntax::ast::*;
use cera_syntax::{closest, parse_with, Diagnostic, ParseOptions, Span};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::OnceLock;

/// Library functions that need no import.
pub const PRELUDE_LIST: &[&str] = &[
    "map", "filter", "fold", "sum", "length", "reverse", "append", "head", "tail",
];

/// Modules reachable by a capitalized name without an import: `String.length`.
const NAMESPACES: &[(&str, &str)] = &[
    ("std.list", "List"),
    ("std.string", "String"),
    ("std.option", "Option"),
    ("std.result", "Result"),
    ("std.num", "Num"),
    ("std.bits", "Bits"),
    ("std.map", "Map"),
    ("std.set", "Set"),
    ("std.array", "Array"),
];

/// Every bodiless (intrinsic) function in the standard-library stubs, as `(module, name)`.
pub fn stub_functions() -> Vec<(String, String)> {
    let mut out = vec![];
    for (module, prog) in stubs() {
        for item in &prog.items {
            if let Item::Decl(d) = item {
                if let DeclKind::Fn(f) = &d.kind {
                    if f.body.is_none() {
                        out.push((module.to_string(), d.name.name.clone()));
                    }
                }
            }
        }
    }
    out
}

/// `core.cera` plus the trait implementations for the built-in types.
fn core_source() -> String {
    let mut s = String::from(include_str!("std/core.cera"));
    let nums: Vec<&str> = INTS.iter().chain(FLOATS.iter()).copied().collect();
    let mut imp = |tr: &str, ty: &str, method: &str, params: &str, ret: &str| {
        s.push_str(&format!(
            "impl {tr} for {ty} do\n  {method} :: fn({params}) -> {ret}\nend\n"
        ));
    };
    for t in &nums {
        let ab = format!("a: {t}, b: {t}");
        for (tr, m) in [("Add", "add"), ("Sub", "sub"), ("Mul", "mul"), ("Div", "div"), ("Rem", "rem")] {
            imp(tr, t, m, &ab, t);
        }
        imp("Ord", t, "compare", &ab, "Ordering");
    }
    for t in INTS {
        imp("Bitwise", t, "bitwise", &format!("a: {t}, b: {t}"), t);
    }
    for t in ["i8", "i16", "i32", "i64", "isize", "f32", "f64"] {
        imp("Neg", t, "neg", &format!("a: {t}"), t);
    }
    for t in ["char", "String", "bool"] {
        imp("Ord", t, "compare", &format!("a: {t}, b: {t}"), "Ordering");
    }
    for t in nums.iter().copied().chain(["bool", "char", "String", "Ordering"]) {
        imp("Show", t, "show", &format!("a: {t}"), "String");
    }
    s.push_str("impl Show for () do\n  show :: fn(a: ()) -> String\nend\n");
    s.push_str("impl Show for Error do\n  show :: fn(a: Error) -> String\nend\n");
    s.push_str("impl<K: Show, V: Show> Show for Map<K, V> do\n  show :: fn(a: Map<K, V>) -> String\nend\n");
    s.push_str("impl<T: Show> Show for Array<T> do\n  show :: fn(a: Array<T>) -> String\nend\n");
    s.push_str("impl<T: Show> Show for Set<T> do\n  show :: fn(a: Set<T>) -> String\nend\n");
    s.push_str("impl<A: Ord, B: Ord> Ord for (A, B) do\n  compare :: fn(a: (A, B), b: (A, B)) -> Ordering\nend\n");
    s.push_str("impl<A: Ord, B: Ord, C: Ord> Ord for (A, B, C) do\n  compare :: fn(a: (A, B, C), b: (A, B, C)) -> Ordering\nend\n");
    s.push_str("impl<T: Ord> Ord for [T] do\n  compare :: fn(a: [T], b: [T]) -> Ordering\nend\n");
    s.push_str("impl<T: Show> Show for [T] do\n  show :: fn(a: [T]) -> String\nend\n");
    s.push_str("impl<A: Show, B: Show> Show for (A, B) do\n  show :: fn(a: (A, B)) -> String\nend\n");
    s.push_str("impl<A: Show, B: Show, C: Show> Show for (A, B, C) do\n  show :: fn(a: (A, B, C)) -> String\nend\n");
    s.push_str("impl<T: Show> Show for Option<T> do\n  show :: fn(a: Option<T>) -> String\nend\n");
    s.push_str("impl<T: Show, E: Show> Show for Result<T, E> do\n  show :: fn(a: Result<T, E>) -> String\nend\n");
    s
}

fn stubs() -> &'static Vec<(&'static str, Program)> {
    static CELL: OnceLock<Vec<(&'static str, Program)>> = OnceLock::new();
    CELL.get_or_init(|| {
        let sources: Vec<(&'static str, String)> = vec![
            ("core", core_source()),
            ("std.list", include_str!("std/list.cera").to_string()),
            ("std.string", include_str!("std/string.cera").to_string()),
            ("std.io", include_str!("std/io.cera").to_string()),
            ("std.option", include_str!("std/option.cera").to_string()),
            ("std.result", include_str!("std/result.cera").to_string()),
            ("std.num", include_str!("std/num.cera").to_string()),
            ("std.bits", include_str!("std/bits.cera").to_string()),
            ("std.array", include_str!("std/array.cera").to_string()),
            ("std.map", include_str!("std/map.cera").to_string()),
            ("std.set", include_str!("std/set.cera").to_string()),
            ("std.fs", include_str!("std/fs.cera").to_string()),
            ("std.time", include_str!("std/time.cera").to_string()),
            ("std.env", include_str!("std/env.cera").to_string()),
            ("std.process", include_str!("std/process.cera").to_string()),
            ("std.mem", include_str!("std/mem.cera").to_string()),
            ("std.sys", include_str!("std/sys.cera").to_string()),
        ];
        sources
            .into_iter()
            .map(|(name, src)| {
                let p = parse_with(&src, ParseOptions { intrinsics: true });
                assert!(p.is_ok(), "std stub `{name}` does not parse: {:?}", p.diagnostics);
                (name, p.program)
            })
            .collect()
    })
}

/// What `analyze` returns.
#[derive(Clone, Debug, Default)]
pub struct Analysis {
    pub diagnostics: Vec<Diagnostic>,
    /// The type of every expression and binding, for hover and `cerac types`.
    pub types: Vec<TypeAt>,
    /// Structured, fully resolved type of every expression and pattern, keyed by `(start, end)`.
    pub typed: HashMap<(usize, usize), Ty>,
    /// Type declarations, trait declarations, impls and function schemes (user and std stubs).
    pub type_defs: HashMap<String, TypeInfo>,
    pub trait_defs: HashMap<String, TraitInfo>,
    pub impls: Vec<ImplInfo>,
    pub fn_schemes: HashMap<String, Scheme>,
    /// Methods of user `impl` blocks.
    pub impl_fns: Vec<ImplFn>,
}

/// A method defined in a user `impl` block.
#[derive(Clone, Debug)]
pub struct ImplFn {
    pub trait_id: String,
    /// The impl's type parameters (the scheme's generics) and the type it is implemented for.
    pub self_ty: Ty,
    pub name: String,
    /// Span of the method's name, identifying its declaration.
    pub span: Span,
    pub scheme: Scheme,
}

#[derive(Clone)]
struct LocalInfo {
    ty: Ty,
    /// Declared with `var`.
    var: bool,
    fn_depth: usize,
}

#[derive(Clone, Debug)]
pub struct TypeAt {
    pub span: Span,
    pub text: String,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Any,
    Int,
    Float,
}

struct VarInfo {
    kind: Kind,
    bound: Option<Ty>,
}

enum UErr {
    Mismatch,
    Occurs,
}

type Given = Rc<Vec<(String, String)>>;

/// "this type must implement this trait", solved once types are known.
struct Obl {
    ty: Ty,
    trait_id: String,
    span: Span,
    /// The operator that caused it, for error messages.
    op: Option<&'static str>,
    given: Given,
}

enum ConstState {
    Pending,
    Active,
    Done,
}

struct ConstInfo<'a> {
    name: String,
    expr: &'a Expr,
    state: ConstState,
    ty: Ty,
}

struct FnToCheck<'a> {
    decl: &'a Decl,
    f: &'a FnDef,
    scheme: Scheme,
}

struct Rec {
    span: Span,
    label: Option<String>,
    ty: Ty,
}

pub struct Checker<'a> {
    diags: Vec<Diagnostic>,
    vars: Vec<VarInfo>,
    types: HashMap<String, TypeInfo>,
    prelude: Exports,
    traits: HashMap<String, TraitInfo>,
    impls: Vec<ImplInfo>,
    vis_traits: HashMap<String, String>,
    self_ty: Option<Ty>,
    given: Given,
    modules: HashMap<String, Exports>,
    aliases: HashMap<String, String>,
    imported: HashSet<String>,
    user: Exports,
    vis_types: HashMap<String, String>,
    consts: Vec<ConstInfo<'a>>,
    fns: Vec<FnToCheck<'a>>,
    scopes: Vec<HashMap<String, LocalInfo>>,
    /// Nesting of functions, lambdas and `defer` bodies (a `var` cannot cross one).
    fn_depth: usize,
    /// How many `unsafe do ... end` blocks enclose the code being checked (reset inside lambdas).
    unsafe_depth: usize,
    /// Enclosing loops, innermost last: the `barriers` count when the loop began.
    loops: Vec<usize>,
    /// Open `region`, `with` and `defer`-carrying blocks (a `break` cannot cross one).
    barriers: usize,
    ret_stack: Vec<Ty>,
    cur_generics: Vec<String>,
    obligations: Vec<Obl>,
    matches: Vec<(Span, Vec<Vec<DPat>>)>,
    rec: Vec<Rec>,
    impl_fns: Vec<ImplFn>,
}

/// Type-check a parsed program against the built-in standard-library stubs.
pub fn analyze(program: &Program) -> Analysis {
    let mut ck = Checker::new();
    ck.load_stubs();
    ck.run_user(program)
}

fn op_str(op: BinOp) -> &'static str {
    match op {
        BinOp::Or => "or",
        BinOp::And => "and",
        BinOp::Eq => "==",
        BinOp::NotEq => "!=",
        BinOp::Lt => "<",
        BinOp::LtEq => "<=",
        BinOp::Gt => ">",
        BinOp::GtEq => ">=",
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
        BinOp::BitAnd => "&",
        BinOp::BitXor => "^",
        BinOp::Shl => "<<",
        BinOp::Shr => ">>",
    }
}

fn tail_span(b: &Block) -> Span {
    b.stmts.last().map_or(b.span, |s| s.span())
}

/// `a.b.c` as `["a", "b", "c"]` when it is a pure chain of field accesses on a name.
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

impl<'a> Checker<'a> {
    fn new() -> Self {
        Checker {
            diags: vec![],
            vars: vec![],
            types: HashMap::new(),
            prelude: Exports::default(),
            traits: HashMap::new(),
            impls: vec![],
            vis_traits: HashMap::new(),
            self_ty: None,
            given: Rc::new(vec![]),
            modules: HashMap::new(),
            aliases: HashMap::new(),
            imported: HashSet::new(),
            user: Exports::default(),
            vis_types: HashMap::new(),
            consts: vec![],
            fns: vec![],
            scopes: vec![],
            fn_depth: 0,
            unsafe_depth: 0,
            loops: vec![],
            barriers: 0,
            ret_stack: vec![],
            cur_generics: vec![],
            obligations: vec![],
            matches: vec![],
            rec: vec![],
            impl_fns: vec![],
        }
    }

    fn err(&mut self, span: Span, msg: impl Into<String>) {
        let d = enrich(Diagnostic::new(span, msg));
        self.diags.push(d);
    }

    fn err_d(&mut self, d: Diagnostic) {
        self.diags.push(d);
    }

    fn value_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.scopes.iter().flat_map(|s| s.keys().cloned()).collect();
        v.extend(self.user.values.keys().cloned());
        v.extend(self.prelude.values.keys().cloned());
        v
    }

    fn unknown_name(&mut self, span: Span, name: &str) {
        let mut d = Diagnostic::new(span, format!("unknown name `{name}`")).with_label("not found in this scope");
        let names = self.value_names();
        if let Some(c) = closest(name, names.iter().map(String::as_str)) {
            d = d.with_help(format!("a similar name exists: `{c}`"));
        } else if STD_MODULES.contains(&name) {
            d = d.with_help(format!("`{name}` is a standard module; add `import std.{name}` at the top of the file"));
        }
        self.err_d(d);
    }

    fn unknown_type(&mut self, span: Span, name: &str) {
        let mut d = Diagnostic::new(span, format!("unknown type `{name}`")).with_label("not found");
        let mut names: Vec<&str> = self.vis_types.keys().map(String::as_str).collect();
        names.extend(["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "usize", "f32", "f64", "bool", "String"]);
        if let Some(c) = closest(name, names) {
            d = d.with_help(format!("a similar type exists: `{c}`"));
        }
        self.err_d(d);
    }

    fn no_field(&mut self, span: Span, ty: &str, field: &str, available: &[String]) {
        let mut d = Diagnostic::new(span, format!("type `{ty}` has no field `{field}`")).with_label("unknown field");
        if let Some(c) = closest(field, available.iter().map(String::as_str)) {
            d = d.with_help(format!("a field with a similar name exists: `{c}`"));
        } else if !available.is_empty() {
            d = d.with_note(format!("available fields: {}", available.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ")));
        }
        self.err_d(d);
    }

    // ----- inference variables -------------------------------------------------

    fn fresh(&mut self, kind: Kind) -> Ty {
        self.vars.push(VarInfo { kind, bound: None });
        Ty::Var(self.vars.len() as u32 - 1)
    }

    fn shallow(&self, t: &Ty) -> Ty {
        let mut t = t.clone();
        while let Ty::Var(v) = t {
            match &self.vars[v as usize].bound {
                Some(b) => t = b.clone(),
                None => return Ty::Var(v),
            }
        }
        t
    }

    fn zonk(&self, t: &Ty) -> Ty {
        match self.shallow(t) {
            Ty::Con(n, a) => Ty::Con(n, a.iter().map(|x| self.zonk(x)).collect()),
            Ty::Fn(p, r) => Ty::Fn(
                p.iter().map(|x| self.zonk(x)).collect(),
                Box::new(self.zonk(&r)),
            ),
            Ty::List(e) => Ty::List(Box::new(self.zonk(&e))),
            Ty::Tuple(v) => Ty::Tuple(v.iter().map(|x| self.zonk(x)).collect()),
            Ty::Ref(e) => Ty::Ref(Box::new(self.zonk(&e))),
            other => other,
        }
    }

    fn occurs(&self, v: u32, t: &Ty) -> bool {
        match self.shallow(t) {
            Ty::Var(w) => v == w,
            Ty::Con(_, a) | Ty::Tuple(a) => a.iter().any(|x| self.occurs(v, x)),
            Ty::Fn(p, r) => p.iter().any(|x| self.occurs(v, x)) || self.occurs(v, &r),
            Ty::List(e) | Ty::Ref(e) => self.occurs(v, &e),
            _ => false,
        }
    }

    fn bind_var(&mut self, v: u32, t: &Ty) -> Result<(), UErr> {
        let ok = match (self.vars[v as usize].kind, t) {
            (Kind::Any, _) => true,
            (Kind::Int, Ty::Con(n, a)) => a.is_empty() && INTS.contains(&n.as_str()),
            (Kind::Float, Ty::Con(n, a)) => a.is_empty() && FLOATS.contains(&n.as_str()),
            _ => false,
        };
        if !ok {
            return Err(UErr::Mismatch);
        }
        if self.occurs(v, t) {
            return Err(UErr::Occurs);
        }
        self.vars[v as usize].bound = Some(t.clone());
        Ok(())
    }

    fn unify(&mut self, a: &Ty, b: &Ty) -> Result<(), UErr> {
        let a = self.shallow(a);
        let b = self.shallow(b);
        match (&a, &b) {
            (Ty::Error, _) | (_, Ty::Error) => Ok(()),
            (Ty::Var(x), Ty::Var(y)) if x == y => Ok(()),
            (Ty::Var(x), Ty::Var(y)) => {
                let kx = self.vars[*x as usize].kind;
                let ky = self.vars[*y as usize].kind;
                let k = match (kx, ky) {
                    (Kind::Any, k) | (k, Kind::Any) => k,
                    (p, q) if p == q => p,
                    _ => return Err(UErr::Mismatch),
                };
                self.vars[*y as usize].kind = k;
                self.vars[*x as usize].bound = Some(Ty::Var(*y));
                Ok(())
            }
            (Ty::Var(x), t) | (t, Ty::Var(x)) => self.bind_var(*x, t),
            (Ty::Con(n1, a1), Ty::Con(n2, a2)) if n1 == n2 && a1.len() == a2.len() => {
                for (x, y) in a1.iter().zip(a2) {
                    self.unify(x, y)?;
                }
                Ok(())
            }
            (Ty::Fn(p1, r1), Ty::Fn(p2, r2)) if p1.len() == p2.len() => {
                for (x, y) in p1.iter().zip(p2) {
                    self.unify(x, y)?;
                }
                self.unify(r1, r2)
            }
            (Ty::List(x), Ty::List(y)) | (Ty::Ref(x), Ty::Ref(y)) => self.unify(x, y),
            (Ty::Tuple(x), Ty::Tuple(y)) if x.len() == y.len() => {
                for (p, q) in x.iter().zip(y) {
                    self.unify(p, q)?;
                }
                Ok(())
            }
            (Ty::Param(x), Ty::Param(y)) if x == y => Ok(()),
            _ => Err(UErr::Mismatch),
        }
    }

    fn show(&self, t: &Ty) -> String {
        match self.shallow(t) {
            Ty::Con(id, args) => {
                if args.is_empty() {
                    short_name(&id).to_string()
                } else {
                    format!("{}<{}>", short_name(&id), self.show_all(&args))
                }
            }
            Ty::Fn(ps, r) => format!("fn({}) -> {}", self.show_all(&ps), self.show(&r)),
            Ty::List(e) => format!("[{}]", self.show(&e)),
            Ty::Tuple(v) => format!("({})", self.show_all(&v)),
            Ty::Ref(e) => format!("ref {}", self.show(&e)),
            Ty::Param(n) => n,
            Ty::Var(v) => match self.vars[v as usize].kind {
                Kind::Any => "_".to_string(),
                Kind::Int => "{integer}".to_string(),
                Kind::Float => "{float}".to_string(),
            },
            Ty::Error => "{error}".to_string(),
        }
    }

    fn show_all(&self, ts: &[Ty]) -> String {
        ts.iter().map(|t| self.show(t)).collect::<Vec<_>>().join(", ")
    }

    /// Unify, reporting a mismatch at `span`.
    fn expect(&mut self, expected: &Ty, found: &Ty, span: Span) {
        if let Err(e) = self.unify(expected, found) {
            let msg = match e {
                UErr::Mismatch => format!(
                    "type mismatch: expected `{}`, found `{}`",
                    self.show(expected),
                    self.show(found)
                ),
                UErr::Occurs => "cannot construct an infinite type".to_string(),
            };
            self.err(span, msg);
        }
    }

    fn subst(&self, t: &Ty, m: &HashMap<String, Ty>) -> Ty {
        match t {
            Ty::Param(n) => m.get(n).cloned().unwrap_or_else(|| t.clone()),
            Ty::Con(n, a) => Ty::Con(n.clone(), a.iter().map(|x| self.subst(x, m)).collect()),
            Ty::Fn(p, r) => Ty::Fn(
                p.iter().map(|x| self.subst(x, m)).collect(),
                Box::new(self.subst(r, m)),
            ),
            Ty::List(e) => Ty::List(Box::new(self.subst(e, m))),
            Ty::Tuple(v) => Ty::Tuple(v.iter().map(|x| self.subst(x, m)).collect()),
            Ty::Ref(e) => Ty::Ref(Box::new(self.subst(e, m))),
            _ => t.clone(),
        }
    }

    fn fresh_map(&mut self, generics: &[String]) -> HashMap<String, Ty> {
        generics
            .iter()
            .map(|g| (g.clone(), self.fresh(Kind::Any)))
            .collect()
    }

    fn instantiate(&mut self, s: &Scheme, span: Span) -> Ty {
        if s.unsafe_ && self.unsafe_depth == 0 {
            self.err(span, "this function is `unsafe`: call it inside an `unsafe do ... end` block".to_string());
        }
        let m = self.fresh_map(&s.generics);
        for (g, tr) in &s.bounds {
            if let Some(t) = m.get(g) {
                self.oblige(t.clone(), tr.clone(), span, None);
            }
        }
        self.subst(&s.ty, &m)
    }

    fn oblige(&mut self, ty: Ty, trait_id: String, span: Span, op: Option<&'static str>) {
        let given = self.given.clone();
        self.obligations.push(Obl {
            ty,
            trait_id,
            span,
            op,
            given,
        });
    }

    fn oblige_core(&mut self, ty: Ty, trait_name: &str, span: Span, op: &'static str) {
        let id = self.prelude.traits[trait_name].clone();
        self.oblige(ty, id, span, Some(op));
    }

    /// Does `pat` (whose `gens` are pattern variables) match `ty`? Unknown parts match anything.
    fn match_ty(&self, pat: &Ty, ty: &Ty, gens: &[String], b: &mut HashMap<String, Ty>) -> bool {
        let ty = self.shallow(ty);
        match (pat, &ty) {
            (Ty::Param(g), _) if gens.contains(g) => match b.get(g) {
                Some(prev) => *prev == self.zonk(&ty),
                None => {
                    b.insert(g.clone(), ty.clone());
                    true
                }
            },
            (_, Ty::Var(_) | Ty::Error) => true,
            (Ty::Con(x, xa), Ty::Con(y, ya)) => {
                x == y
                    && xa.len() == ya.len()
                    && xa.iter().zip(ya).all(|(p, t)| self.match_ty(p, t, gens, b))
            }
            (Ty::Fn(xp, xr), Ty::Fn(yp, yr)) => {
                xp.len() == yp.len()
                    && xp.iter().zip(yp).all(|(p, t)| self.match_ty(p, t, gens, b))
                    && self.match_ty(xr, yr, gens, b)
            }
            (Ty::List(x), Ty::List(y)) | (Ty::Ref(x), Ty::Ref(y)) => self.match_ty(x, y, gens, b),
            (Ty::Tuple(xs), Ty::Tuple(ys)) => {
                xs.len() == ys.len() && xs.iter().zip(ys).all(|(p, t)| self.match_ty(p, t, gens, b))
            }
            (Ty::Param(x), Ty::Param(y)) => x == y,
            _ => false,
        }
    }

    /// Does `ty` implement the trait? `Err` carries the reason.
    fn holds(&self, ty: &Ty, trait_id: &str, given: &Given, depth: u32) -> Result<(), String> {
        if depth > 16 {
            return Ok(());
        }
        let ty = self.zonk(ty);
        let tname = self.traits.get(trait_id).map_or(short_name(trait_id), |t| t.name.as_str());
        match &ty {
            Ty::Var(_) | Ty::Error => return Ok(()),
            Ty::Param(p) => {
                return if given.iter().any(|(g, t)| g == p && t == trait_id) {
                    Ok(())
                } else {
                    Err(format!(
                        "type parameter `{p}` does not implement `{tname}` (add `{p}: {tname}` to the type parameters)"
                    ))
                };
            }
            _ => {}
        }
        for imp in &self.impls {
            if imp.trait_id != trait_id {
                continue;
            }
            let mut b = HashMap::new();
            if self.match_ty(&imp.self_ty, &ty, &imp.generics, &mut b) {
                for (g, tr) in &imp.bounds {
                    let bt = b.get(g).cloned().unwrap_or(Ty::Error);
                    self.holds(&bt, tr, given, depth + 1)?;
                }
                return Ok(());
            }
        }
        Err(format!("the trait `{tname}` is not implemented for `{}`", self.show(&ty)))
    }

    fn ctor_type(&mut self, c: &CtorInfo) -> Ty {
        let m = self.fresh_map(&c.generics);
        let args: Vec<Ty> = c.generics.iter().map(|g| m[g].clone()).collect();
        let result = Ty::Con(c.type_id.clone(), args);
        if c.fields.is_empty() {
            result
        } else {
            let ps = c.fields.iter().map(|f| self.subst(f, &m)).collect();
            Ty::Fn(ps, Box::new(result))
        }
    }

    fn core_con(&self, name: &str, args: Vec<Ty>) -> Ty {
        Ty::Con(self.prelude.types[name].clone(), args)
    }

    // ----- scopes and lookup ---------------------------------------------------

    fn lookup_info(&self, name: &str) -> Option<&LocalInfo> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }

    fn lookup_local(&self, name: &str) -> Option<Ty> {
        self.lookup_info(name).map(|i| i.ty.clone())
    }

    fn lookup_global(&self, name: &str) -> Option<Value> {
        self.user
            .values
            .get(name)
            .or_else(|| self.prelude.values.get(name))
            .cloned()
    }

    fn bind_local(&mut self, name: &Ident, ty: Ty) {
        self.bind_info(name, ty, false);
    }

    fn bind_info(&mut self, name: &Ident, ty: Ty, var: bool) {
        self.rec.push(Rec {
            span: name.span,
            label: Some(name.name.clone()),
            ty: ty.clone(),
        });
        let info = LocalInfo { ty, var, fn_depth: self.fn_depth };
        if let Some(s) = self.scopes.last_mut() {
            s.insert(name.name.clone(), info);
        }
    }

    fn bind_param(&mut self, name: &Ident, ty: Ty) {
        if self.scopes.last().is_some_and(|s| s.contains_key(&name.name)) {
            self.err(name.span, format!("duplicate parameter `{}`", name.name));
        }
        self.bind_local(name, ty);
    }

    /// Whether the module named by `segs` is one whose functions need `unsafe` (`std.mem`, `std.sys`).
    fn is_unsafe_module(&self, segs: &[&str]) -> bool {
        let joined = segs.join(".");
        let key = if segs.len() == 1 { self.aliases.get(&joined).cloned().unwrap_or(joined) } else { joined };
        matches!(key.as_str(), "std.mem" | "std.sys")
    }

    fn find_module(&self, segs: &[&str]) -> Option<&Exports> {
        let joined = segs.join(".");
        if segs.len() == 1 {
            if let Some(key) = self.aliases.get(&joined) {
                return self.modules.get(key);
            }
            if NAMESPACES.iter().any(|(_, ns)| *ns == joined) {
                return self.modules.get(&joined);
            }
            None
        } else if self.imported.contains(&joined) {
            self.modules.get(&joined)
        } else {
            None
        }
    }

    // ----- type expressions ----------------------------------------------------

    fn resolve_type(&mut self, t: &cera_syntax::ast::Type, generics: &[String]) -> Ty {
        match &t.kind {
            TypeKind::Path { path, args } => {
                let argtys: Vec<Ty> = args.iter().map(|a| self.resolve_type(a, generics)).collect();
                let last = &path[path.len() - 1];
                if path.len() == 1 {
                    let name = &last.name;
                    if generics.contains(name) {
                        if !args.is_empty() {
                            self.err(t.span, format!("type parameter `{name}` takes no type arguments"));
                        }
                        return Ty::Param(name.clone());
                    }
                    if name == "Self" {
                        if let Some(t) = &self.self_ty {
                            return t.clone();
                        }
                    }
                    if is_primitive(name) {
                        if !args.is_empty() {
                            self.err(t.span, format!("type `{name}` takes no type arguments"));
                        }
                        return Ty::prim(name);
                    }
                    return match self.vis_types.get(name).cloned() {
                        Some(id) => self.apply_type(id, argtys, t.span),
                        None => {
                            self.unknown_type(t.span, name);
                            Ty::Error
                        }
                    };
                }
                let prefix: Vec<&str> = path[..path.len() - 1].iter().map(|i| i.name.as_str()).collect();
                let found = self
                    .find_module(&prefix)
                    .map(|ex| ex.types.get(&last.name).cloned());
                match found {
                    Some(Some(id)) => self.apply_type(id, argtys, t.span),
                    Some(None) => {
                        self.err(t.span, format!("module `{}` has no type `{}`", prefix.join("."), last.name));
                        Ty::Error
                    }
                    None => {
                        self.err(t.span, format!("unknown module `{}`", prefix.join(".")));
                        Ty::Error
                    }
                }
            }
            TypeKind::Fn { params, ret } => Ty::Fn(
                params.iter().map(|p| self.resolve_type(p, generics)).collect(),
                Box::new(self.resolve_type(ret, generics)),
            ),
            TypeKind::List(e) => Ty::List(Box::new(self.resolve_type(e, generics))),
            TypeKind::Tuple(v) => Ty::Tuple(v.iter().map(|x| self.resolve_type(x, generics)).collect()),
            TypeKind::Ref(e) => Ty::Ref(Box::new(self.resolve_type(e, generics))),
        }
    }

    fn apply_type(&mut self, id: String, args: Vec<Ty>, span: Span) -> Ty {
        let (name, n) = match self.types.get(&id) {
            Some(i) => (i.name.clone(), i.generics.len()),
            None => return Ty::Error,
        };
        if n != args.len() {
            self.err(
                span,
                format!("type `{name}` expects {n} type argument(s), found {}", args.len()),
            );
            return Ty::Error;
        }
        Ty::Con(id, args)
    }

    // ----- declaration collection ----------------------------------------------

    fn define_value(&mut self, ex: &mut Exports, name: &Ident, v: Value, user: bool) {
        if ex.values.contains_key(&name.name) {
            if user {
                self.err(name.span, format!("`{}` is already defined", name.name));
            }
            return;
        }
        ex.values.insert(name.name.clone(), v);
    }

    fn check_generics(&mut self, gs: &[Ident]) -> Vec<String> {
        let mut seen = HashSet::new();
        for g in gs {
            if !seen.insert(g.name.clone()) {
                self.err(g.span, format!("type parameter `{}` is declared twice", g.name));
            }
        }
        gs.iter().map(|g| g.name.clone()).collect()
    }

    fn collect(&mut self, prog: &'a Program, module: &str, user: bool) -> Exports {
        let mut ex = Exports::default();

        // Pass 1: type and trait names, so declarations can refer to each other.
        for item in &prog.items {
            if let Item::Decl(d) = item {
                if let DeclKind::Trait(_) = &d.kind {
                    let id = if module.is_empty() {
                        d.name.name.clone()
                    } else {
                        format!("{module}::{}", d.name.name)
                    };
                    if ex.traits.contains_key(&d.name.name) {
                        self.err(d.name.span, format!("trait `{}` is already defined", d.name.name));
                        continue;
                    }
                    self.traits.insert(
                        id.clone(),
                        TraitInfo {
                            name: d.name.name.clone(),
                            methods: vec![],
                        },
                    );
                    ex.traits.insert(d.name.name.clone(), id);
                }
                if let DeclKind::Type(_) = &d.kind {
                    let id = if module.is_empty() {
                        d.name.name.clone()
                    } else {
                        format!("{module}::{}", d.name.name)
                    };
                    if ex.types.contains_key(&d.name.name) {
                        self.err(d.name.span, format!("type `{}` is already defined", d.name.name));
                        continue;
                    }
                    let generics = self.check_generics(&d.generics);
                    self.types.insert(
                        id.clone(),
                        TypeInfo {
                            name: d.name.name.clone(),
                            generics,
                            def: TDef::Opaque,
                        },
                    );
                    ex.types.insert(d.name.name.clone(), id);
                }
            }
        }
        self.vis_types = self.prelude.types.clone();
        self.vis_types.extend(ex.types.clone());
        self.vis_traits = self.prelude.traits.clone();
        self.vis_traits.extend(ex.traits.clone());

        // Pass 2: type bodies and constructors.
        for item in &prog.items {
            let Item::Decl(d) = item else { continue };
            let DeclKind::Type(td) = &d.kind else { continue };
            let Some(id) = ex.types.get(&d.name.name).cloned() else { continue };
            if self.types[&id].generics.len() != d.generics.len() {
                continue; // a duplicate declaration that was skipped above
            }
            let generics = self.types[&id].generics.clone();
            let def = match td {
                TypeDef::Opaque => TDef::Opaque,
                TypeDef::Record(fields) => {
                    let mut seen = HashSet::new();
                    let mut out = vec![];
                    for f in fields {
                        if !seen.insert(f.name.name.clone()) {
                            self.err(f.name.span, format!("field `{}` is declared twice", f.name.name));
                        }
                        out.push((f.name.name.clone(), self.resolve_type(&f.ty, &generics)));
                    }
                    TDef::Record(out)
                }
                TypeDef::Variants(vs) => {
                    let mut out = vec![];
                    for (index, v) in vs.iter().enumerate() {
                        let fields: Vec<Ty> =
                            v.fields.iter().map(|f| self.resolve_type(&f.ty, &generics)).collect();
                        let info = CtorInfo {
                            type_id: id.clone(),
                            index,
                            generics: generics.clone(),
                            fields: fields.clone(),
                        };
                        self.define_value(&mut ex, &v.name, Value::Ctor(info), user);
                        out.push(VariantInfo {
                            name: v.name.name.clone(),
                            fields,
                        });
                    }
                    TDef::Variants(out)
                }
            };
            self.types.get_mut(&id).unwrap().def = def;
        }

        // Pass 3: function signatures and constants.
        for item in &prog.items {
            let Item::Decl(d) = item else { continue };
            match &d.kind {
                DeclKind::Type(_) => {}
                DeclKind::Trait(td) => {
                    let Some(id) = ex.traits.get(&d.name.name).cloned() else { continue };
                    if !self.traits[&id].methods.is_empty() {
                        continue; // duplicate declaration
                    }
                    self.collect_trait(&mut ex, &id, td, user);
                }
                DeclKind::Fn(f) => {
                    let generics = self.check_generics(&f.generics);
                    let bounds = self.resolve_bounds(&f.bounds, &generics);
                    let mut params = vec![];
                    for p in &f.params {
                        let t = match &p.ty {
                            Some(t) => self.resolve_type(t, &generics),
                            None => {
                                self.err(
                                    p.name.span,
                                    format!("parameter `{}` needs a type annotation", p.name.name),
                                );
                                Ty::Error
                            }
                        };
                        params.push(t);
                    }
                    let ret = match (&f.ret, &f.body) {
                        (Some(t), _) => self.resolve_type(t, &generics),
                        (None, None) => Ty::unit(),
                        (None, Some(_)) => self.fresh(Kind::Any),
                    };
                    let scheme = Scheme {
                        generics,
                        bounds,
                        ty: Ty::Fn(params, Box::new(ret)),
                        unsafe_: f.unsafe_,
                    };
                    self.define_value(&mut ex, &d.name, Value::Fn(scheme.clone()), user);
                    if user {
                        self.fns.push(FnToCheck { decl: d, f, scheme });
                    }
                }
                DeclKind::Const(e) => {
                    if !user {
                        continue;
                    }
                    let idx = self.consts.len();
                    let ty = self.fresh(Kind::Any);
                    self.consts.push(ConstInfo {
                        name: d.name.name.clone(),
                        expr: e,
                        state: ConstState::Pending,
                        ty,
                    });
                    self.define_value(&mut ex, &d.name, Value::Const(idx), user);
                }
            }
        }

        // Pass 4: implementations.
        for item in &prog.items {
            if let Item::Impl(im) = item {
                self.collect_impl(im, user);
            }
        }
        ex
    }

    fn resolve_bounds(&mut self, bounds: &[Bound], generics: &[String]) -> Vec<(String, String)> {
        let mut out = vec![];
        for b in bounds {
            if !generics.contains(&b.param.name) {
                self.err(b.param.span, format!("unknown type parameter `{}`", b.param.name));
                continue;
            }
            for t in &b.traits {
                match self.vis_traits.get(&t.name).cloned() {
                    Some(id) => out.push((b.param.name.clone(), id)),
                    None => self.err(t.span, format!("unknown trait `{}`", t.name)),
                }
            }
        }
        out
    }

    fn collect_trait(&mut self, ex: &mut Exports, id: &str, td: &TraitDef, user: bool) {
        let gens = vec!["Self".to_string()];
        for m in &td.methods {
            let first_is_self = m
                .params
                .first()
                .is_some_and(|p| p.name.name == "self" && p.ty.is_none());
            if !first_is_self {
                self.err(
                    m.name.span,
                    format!("trait method `{}` must take `self` as its first parameter", m.name.name),
                );
            }
            let mut params = vec![];
            for (i, p) in m.params.iter().enumerate() {
                let t = match &p.ty {
                    Some(t) => self.resolve_type(t, &gens),
                    None if i == 0 && p.name.name == "self" => Ty::Param("Self".to_string()),
                    None => {
                        self.err(
                            p.name.span,
                            format!("parameter `{}` needs a type annotation", p.name.name),
                        );
                        Ty::Error
                    }
                };
                params.push(t);
            }
            let ret = match &m.ret {
                Some(t) => self.resolve_type(t, &gens),
                None => Ty::unit(),
            };
            let ty = Ty::Fn(params, Box::new(ret));
            let scheme = Scheme {
                generics: gens.clone(),
                bounds: vec![("Self".to_string(), id.to_string())],
                ty: ty.clone(),
                unsafe_: false,
            };
            self.define_value(ex, &m.name, Value::Fn(scheme), user);
            self.traits
                .get_mut(id)
                .unwrap()
                .methods
                .push((m.name.name.clone(), ty));
        }
    }

    fn type_head(t: &Ty) -> String {
        match t {
            Ty::Con(id, _) => id.clone(),
            Ty::List(_) => "[]".to_string(),
            Ty::Tuple(v) => format!("tuple{}", v.len()),
            Ty::Fn(..) => "fn".to_string(),
            Ty::Ref(_) => "ref".to_string(),
            Ty::Param(_) => "*".to_string(),
            _ => "?".to_string(),
        }
    }

    fn collect_impl(&mut self, im: &'a ImplDef, user: bool) {
        let generics = self.check_generics(&im.generics);
        let bounds = self.resolve_bounds(&im.bounds, &generics);
        let Some(trait_id) = self.vis_traits.get(&im.trait_name.name).cloned() else {
            self.err(im.trait_name.span, format!("unknown trait `{}`", im.trait_name.name));
            return;
        };
        let self_ty = self.resolve_type(&im.ty, &generics);
        if self_ty == Ty::Error {
            return;
        }
        let head = Self::type_head(&self_ty);
        if head == "*" {
            self.err(im.ty.span, "an impl for a bare type parameter is not supported");
            return;
        }
        if self
            .impls
            .iter()
            .any(|o| o.trait_id == trait_id && Self::type_head(&o.self_ty) == head)
        {
            let msg = format!(
                "conflicting implementations of `{}` for `{}`",
                im.trait_name.name,
                self.show(&self_ty)
            );
            self.err(im.ty.span, msg);
            return;
        }
        let tinfo = self.traits[&trait_id].clone();
        let mut seen = HashSet::new();
        let mut selfm = HashMap::new();
        selfm.insert("Self".to_string(), self_ty.clone());
        for d in &im.methods {
            let DeclKind::Fn(f) = &d.kind else { continue };
            if !seen.insert(d.name.name.clone()) {
                self.err(d.name.span, format!("method `{}` is defined twice", d.name.name));
                continue;
            }
            let Some((_, tty)) = tinfo.methods.iter().find(|(n, _)| *n == d.name.name) else {
                self.err(
                    d.name.span,
                    format!("trait `{}` has no method `{}`", tinfo.name, d.name.name),
                );
                continue;
            };
            if !f.generics.is_empty() {
                self.err(d.name.span, "methods cannot have their own type parameters yet");
                continue;
            }
            let expected = self.subst(tty, &selfm);
            self.self_ty = Some(self_ty.clone());
            let mut params = vec![];
            for p in &f.params {
                let t = match &p.ty {
                    Some(t) => self.resolve_type(t, &generics),
                    None => {
                        self.err(
                            p.name.span,
                            format!("parameter `{}` needs a type annotation", p.name.name),
                        );
                        Ty::Error
                    }
                };
                params.push(t);
            }
            let ret = match (&f.ret, &expected) {
                (Some(t), _) => self.resolve_type(t, &generics),
                (None, Ty::Fn(_, r)) => (**r).clone(),
                (None, _) => Ty::unit(),
            };
            self.self_ty = None;
            let declared = Ty::Fn(params, Box::new(ret));
            if self.unify(&expected, &declared).is_err() {
                let msg = format!(
                    "method `{}` has type `{}`, but the trait requires `{}`",
                    d.name.name,
                    self.show(&declared),
                    self.show(&expected)
                );
                self.err(d.name.span, msg);
                continue;
            }
            if user && f.body.is_some() {
                self.impl_fns.push(ImplFn {
                    trait_id: trait_id.clone(),
                    self_ty: self_ty.clone(),
                    name: d.name.name.clone(),
                    span: d.name.span,
                    scheme: Scheme { generics: generics.clone(), bounds: bounds.clone(), ty: declared.clone(), unsafe_: false },
                });
                self.fns.push(FnToCheck {
                    decl: d,
                    f,
                    scheme: Scheme {
                        generics: generics.clone(),
                        bounds: bounds.clone(),
                        ty: declared,
                        unsafe_: false,
                    },
                });
            }
        }
        let missing: Vec<String> = tinfo
            .methods
            .iter()
            .filter(|(n, _)| !seen.contains(n))
            .map(|(n, _)| format!("`{n}`"))
            .collect();
        if !missing.is_empty() {
            self.err(
                im.trait_name.span,
                format!("missing method(s) {} in impl of `{}`", missing.join(", "), tinfo.name),
            );
        }
        self.impls.push(ImplInfo {
            trait_id,
            generics,
            bounds,
            self_ty,
        });
    }

    fn load_stubs(&mut self) {
        for (name, prog) in stubs() {
            let ex = self.collect(prog, name, false);
            match *name {
                "core" => self.prelude = ex,
                "std.list" => {
                    for (k, v) in &ex.values {
                        if PRELUDE_LIST.contains(&k.as_str()) {
                            self.prelude.values.insert(k.clone(), v.clone());
                        }
                    }
                    self.modules.insert("List".to_string(), ex.clone());
                    self.modules.insert(name.to_string(), ex);
                }
                other => {
                    if let Some(ns) = NAMESPACES.iter().find(|(m, _)| *m == other) {
                        self.modules.insert(ns.1.to_string(), ex.clone());
                    }
                    self.modules.insert(name.to_string(), ex);
                }
            }
        }
    }

    fn process_imports(&mut self, prog: &Program) {
        for item in &prog.items {
            let Item::Import(i) = item else { continue };
            let joined = i.path.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(".");
            if !joined.contains('.') || !self.modules.contains_key(&joined) {
                let mut avail: Vec<&str> = self
                    .modules
                    .keys()
                    .filter(|k| k.contains('.'))
                    .map(String::as_str)
                    .collect();
                avail.sort();
                self.err(
                    i.span,
                    format!("unknown module `{joined}` (available so far: {})", avail.join(", ")),
                );
                continue;
            }
            let alias = match &i.alias {
                Some(a) => a.name.clone(),
                None => i.path.last().unwrap().name.clone(),
            };
            if self.aliases.contains_key(&alias) {
                self.err(i.span, format!("`{alias}` is already imported"));
                continue;
            }
            self.aliases.insert(alias, joined.clone());
            self.imported.insert(joined);
        }
    }

    // ----- driver --------------------------------------------------------------

    fn run_user(mut self, prog: &'a Program) -> Analysis {
        self.process_imports(prog);
        let user = self.collect(prog, "", true);
        self.user = user;

        for i in 0..self.consts.len() {
            self.const_type(i, None);
        }
        // Record constant names for hover.
        for item in &prog.items {
            if let Item::Decl(d) = item {
                if let DeclKind::Const(_) = d.kind {
                    if let Some(Value::Const(i)) = self.user.values.get(&d.name.name).cloned() {
                        let ty = self.consts[i].ty.clone();
                        self.rec.push(Rec {
                            span: d.name.span,
                            label: Some(d.name.name.clone()),
                            ty,
                        });
                    }
                }
            }
        }

        let fns = std::mem::take(&mut self.fns);
        for f in &fns {
            self.check_fn(f);
        }
        self.finish()
    }

    fn check_fn(&mut self, item: &FnToCheck<'a>) {
        let Ty::Fn(params, ret) = &item.scheme.ty else { return };
        let ret = (**ret).clone();
        self.cur_generics = item.scheme.generics.clone();
        self.unsafe_depth = usize::from(item.f.unsafe_);
        self.given = Rc::new(item.scheme.bounds.clone());
        self.scopes = vec![HashMap::new()];
        self.ret_stack = vec![ret.clone()];
        self.rec.push(Rec {
            span: item.decl.name.span,
            label: Some(item.decl.name.name.clone()),
            ty: item.scheme.ty.clone(),
        });
        for (p, t) in item.f.params.iter().zip(params) {
            self.bind_param(&p.name, t.clone());
        }
        if let Some(body) = &item.f.body {
            let t = self.infer_block(body);
            if self.unify(&ret, &t).is_err() {
                let msg = format!(
                    "function `{}` should return `{}`, but its body has type `{}`",
                    item.decl.name.name,
                    self.show(&ret),
                    self.show(&t)
                );
                self.err(tail_span(body), msg);
            }
        }
        self.scopes.clear();
        self.ret_stack.clear();
        self.cur_generics.clear();
        self.given = Rc::new(vec![]);
    }

    fn const_type(&mut self, i: usize, use_span: Option<Span>) -> Ty {
        match self.consts[i].state {
            ConstState::Done => return self.consts[i].ty.clone(),
            ConstState::Active => {
                let name = self.consts[i].name.clone();
                let span = use_span.unwrap_or(self.consts[i].expr.span);
                self.err(span, format!("constant `{name}` depends on itself"));
                return Ty::Error;
            }
            ConstState::Pending => {}
        }
        self.consts[i].state = ConstState::Active;
        let expr: &'a Expr = self.consts[i].expr;
        let saved_scopes = std::mem::replace(&mut self.scopes, vec![HashMap::new()]);
        let saved_ret = std::mem::take(&mut self.ret_stack);
        let saved_gen = std::mem::take(&mut self.cur_generics);
        let t = self.infer(expr);
        self.scopes = saved_scopes;
        self.ret_stack = saved_ret;
        self.cur_generics = saved_gen;
        let slot = self.consts[i].ty.clone();
        let _ = self.unify(&slot, &t);
        self.consts[i].state = ConstState::Done;
        self.consts[i].ty.clone()
    }

    fn finish(mut self) -> Analysis {
        // Literal defaulting.
        for i in 0..self.vars.len() {
            if self.vars[i].bound.is_none() {
                match self.vars[i].kind {
                    Kind::Int => self.vars[i].bound = Some(Ty::prim("i32")),
                    Kind::Float => self.vars[i].bound = Some(Ty::prim("f64")),
                    Kind::Any => {}
                }
            }
        }

        // Trait obligations (operators and bounds).
        let obligations = std::mem::take(&mut self.obligations);
        let mut failures = vec![];
        for o in &obligations {
            if let Err(reason) = self.holds(&o.ty, &o.trait_id, &o.given, 0) {
                let msg = match o.op {
                    Some(op) => {
                        let z = self.zonk(&o.ty);
                        let what = if matches!(z, Ty::Param(_)) { "type parameter" } else { "type" };
                        let tname = self.traits.get(&o.trait_id).map_or("?", |t| t.name.as_str());
                        format!(
                            "operator `{op}` cannot be applied to {what} `{}` (requires `{tname}`)",
                            self.show(&z)
                        )
                    }
                    None => reason,
                };
                failures.push((o.span, msg));
            }
        }
        for (span, msg) in failures {
            self.err(span, msg);
        }

        // Exhaustiveness.
        let matches = std::mem::take(&mut self.matches);
        let mut found = vec![];
        {
            let cx = Ctx { types: &self.types };
            for (span, rows) in &matches {
                if let Some(w) = cx.witness(rows, 1) {
                    found.push((*span, format!("non-exhaustive match: `{}` is not covered", cx.show(&w[0]))));
                }
            }
        }
        for (span, msg) in found {
            self.err(span, msg);
        }

        // Hover data.
        let recs = std::mem::take(&mut self.rec);
        let types = recs
            .iter()
            .map(|r| {
                let t = self.show(&self.zonk(&r.ty));
                TypeAt {
                    span: r.span,
                    text: match &r.label {
                        Some(n) => format!("{n}: {t}"),
                        None => t,
                    },
                }
            })
            .collect();

        let mut typed = HashMap::new();
        for r in &recs {
            if r.label.is_none() {
                typed.insert((r.span.start, r.span.end), self.zonk(&r.ty));
            }
        }
        let fn_schemes = self
            .user
            .values
            .iter()
            .filter_map(|(k, v)| match v {
                Value::Fn(s) => Some((k.clone(), s.clone())),
                _ => None,
            })
            .collect();
        let mut diagnostics = self.diags;
        diagnostics.sort_by_key(|d| d.span.start);
        Analysis {
            diagnostics,
            types,
            typed,
            type_defs: self.types,
            trait_defs: self.traits,
            impls: self.impls,
            fn_schemes,
            impl_fns: self.impl_fns,
        }
    }

    // ----- expressions ---------------------------------------------------------

    fn infer(&mut self, e: &Expr) -> Ty {
        let t = self.infer_inner(e);
        self.rec.push(Rec {
            span: e.span,
            label: None,
            ty: t.clone(),
        });
        t
    }

    fn check(&mut self, e: &Expr, expected: &Ty) {
        if let ExprKind::Lambda { params, body } = &e.kind {
            if let Ty::Fn(ps, r) = self.shallow(expected) {
                if ps.len() == params.len() {
                    let t = self.infer_lambda(params, body, Some((ps, *r)));
                    self.rec.push(Rec {
                        span: e.span,
                        label: None,
                        ty: t.clone(),
                    });
                    self.expect(expected, &t, e.span);
                    return;
                }
            }
        }
        let t = self.infer(e);
        self.expect(expected, &t, e.span);
    }

    fn infer_inner(&mut self, e: &Expr) -> Ty {
        match &e.kind {
            ExprKind::Int(_) => self.fresh(Kind::Int),
            ExprKind::Float(_) => self.fresh(Kind::Float),
            ExprKind::Str(_) => Ty::string(),
            ExprKind::Bool(_) => Ty::bool(),
            ExprKind::Unit => Ty::unit(),
            ExprKind::Var(name) => self.infer_var(name, e.span),
            ExprKind::List(items) => {
                let elem = self.fresh(Kind::Any);
                for it in items {
                    self.check(it, &elem);
                }
                Ty::List(Box::new(elem))
            }
            ExprKind::Cons { items, tail } => {
                let elem = self.fresh(Kind::Any);
                for it in items {
                    self.check(it, &elem);
                }
                let list = Ty::List(Box::new(elem));
                self.check(tail, &list);
                list
            }
            ExprKind::Tuple(items) => Ty::Tuple(items.iter().map(|i| self.infer(i)).collect()),
            ExprKind::Record { name, fields } => self.infer_record(name, fields, e.span),
            ExprKind::Update { base, fields } => self.infer_update(base, fields),
            ExprKind::Unary { op, expr } => match op {
                UnOp::Neg => {
                    let t = self.infer(expr);
                    self.oblige_core(t.clone(), "Neg", e.span, "-");
                    t
                }
                UnOp::Not => {
                    self.check(expr, &Ty::bool());
                    Ty::bool()
                }
                UnOp::BitNot => {
                    let t = self.infer(expr);
                    self.oblige_core(t.clone(), "Bitwise", e.span, "~");
                    t
                }
            },
            ExprKind::Binary { op, lhs, rhs } => match op {
                BinOp::And | BinOp::Or => {
                    self.check(lhs, &Ty::bool());
                    self.check(rhs, &Ty::bool());
                    Ty::bool()
                }
                BinOp::Eq | BinOp::NotEq => {
                    let a = self.infer(lhs);
                    self.check(rhs, &a);
                    Ty::bool()
                }
                BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
                    let a = self.infer(lhs);
                    self.check(rhs, &a);
                    self.oblige_core(a, "Ord", e.span, op_str(*op));
                    Ty::bool()
                }
                BinOp::BitAnd | BinOp::BitXor => {
                    let a = self.infer(lhs);
                    self.check(rhs, &a);
                    self.oblige_core(a.clone(), "Bitwise", e.span, op_str(*op));
                    a
                }
                BinOp::Shl | BinOp::Shr => {
                    let a = self.infer(lhs);
                    self.check(rhs, &Ty::prim("u32"));
                    self.oblige_core(a.clone(), "Bitwise", e.span, op_str(*op));
                    a
                }
                _ => {
                    let a = self.infer(lhs);
                    self.check(rhs, &a);
                    let tr = match op {
                        BinOp::Add => "Add",
                        BinOp::Sub => "Sub",
                        BinOp::Mul => "Mul",
                        BinOp::Div => "Div",
                        _ => "Rem",
                    };
                    self.oblige_core(a.clone(), tr, e.span, op_str(*op));
                    a
                }
            },
            ExprKind::Pipe { lhs, rhs } => match &rhs.kind {
                ExprKind::Call { callee, args } => {
                    let mut v: Vec<&Expr> = vec![lhs];
                    v.extend(args.iter());
                    self.infer_call(callee, v, e.span)
                }
                _ => self.infer_call(rhs, vec![lhs], e.span),
            },
            ExprKind::Call { callee, args } => {
                let v: Vec<&Expr> = args.iter().collect();
                self.infer_call(callee, v, e.span)
            }
            ExprKind::Field { expr, name } => self.infer_field(e, expr, name),
            ExprKind::Index { expr, index } => {
                let t = self.infer(expr);
                let elem = self.fresh(Kind::Any);
                let container = match self.zonk(&t) {
                    Ty::Con(n, _) if n.ends_with("Array") => Ty::Con(n, vec![elem.clone()]),
                    _ => Ty::List(Box::new(elem.clone())),
                };
                self.expect(&container, &t, expr.span);
                self.check(index, &Ty::prim("usize"));
                elem
            }
            ExprKind::Try(inner) => self.infer_try(inner, e.span),
            ExprKind::Lambda { params, body } => self.infer_lambda(params, body, None),
            ExprKind::If { cond, then, els } => {
                self.check(cond, &Ty::bool());
                let t = self.infer_block(then);
                match els {
                    Some(b) => {
                        let t2 = self.infer_block(b);
                        if self.unify(&t, &t2).is_err() {
                            let msg = format!(
                                "`if` and `else` have different types: `{}` and `{}`",
                                self.show(&t),
                                self.show(&t2)
                            );
                            self.err(tail_span(b), msg);
                        }
                        t
                    }
                    None => {
                        if self.unify(&Ty::unit(), &t).is_err() {
                            let msg =
                                format!("`if` without `else` must have type `()`, found `{}`", self.show(&t));
                            self.err(tail_span(then), msg);
                        }
                        Ty::unit()
                    }
                }
            }
            ExprKind::Match { scrutinee, arms } => self.infer_match(scrutinee, arms),
            ExprKind::Block(b) => self.infer_block(b),
            ExprKind::Unsafe(b) => {
                self.unsafe_depth += 1;
                let t = self.infer_block(b);
                self.unsafe_depth -= 1;
                t
            }
            ExprKind::Region(b) => {
                self.barriers += 1;
                let t = self.infer_block(b);
                self.barriers -= 1;
                t
            }
            ExprKind::While { cond, body } => {
                self.check(cond, &Ty::bool());
                self.loop_body(body);
                Ty::unit()
            }
            ExprKind::For { var, iter, body } => {
                let elem = self.fresh(Kind::Any);
                self.check(iter, &Ty::List(Box::new(elem.clone())));
                self.scopes.push(HashMap::new());
                self.bind_local(var, elem);
                self.loop_body(body);
                self.scopes.pop();
                Ty::unit()
            }
            ExprKind::Break => {
                self.jump_allowed("break", e.span);
                Ty::unit()
            }
            ExprKind::Continue => {
                self.jump_allowed("continue", e.span);
                Ty::unit()
            }
            ExprKind::With { what, arg, body } => {
                if what.name == "allocator" {
                    let alloc = self.core_con("Allocator", vec![]);
                    self.check(arg, &alloc);
                } else {
                    self.err(
                        what.span,
                        format!("unknown `with` form `{}`; expected `allocator`", what.name),
                    );
                    self.infer(arg);
                }
                self.barriers += 1;
                let t = self.infer_block(body);
                self.barriers -= 1;
                t
            }
        }
    }

    fn infer_var(&mut self, name: &str, span: Span) -> Ty {
        if let Some(info) = self.lookup_info(name).cloned() {
            if info.var && info.fn_depth < self.fn_depth {
                self.capture_error(name, span);
            }
            return info.ty;
        }
        match self.lookup_global(name) {
            Some(Value::Fn(s)) => self.instantiate(&s, span),
            Some(Value::Const(i)) => self.const_type(i, Some(span)),
            Some(Value::Ctor(c)) => self.ctor_type(&c),
            None => {
                if self.find_module(&[name]).is_some() {
                    self.err(span, format!("`{name}` is a module, not a value"));
                } else {
                    self.unknown_name(span, name);
                }
                Ty::Error
            }
        }
    }

    fn capture_error(&mut self, name: &str, span: Span) {
        let d = Diagnostic::new(span, format!("`{name}` is a `var` and cannot be used inside a function, closure or `defer`"))
            .with_label("mutable variable used here")
            .with_help(format!("copy it into an immutable binding first, for example `{name}_now = {name}`, and use that"));
        self.diags.push(d);
    }

    fn infer_block(&mut self, b: &Block) -> Ty {
        self.scopes.push(HashMap::new());
        let has_defer = b.stmts.iter().any(|s| matches!(s, Stmt::Defer(..)));
        if has_defer {
            self.barriers += 1;
        }
        let mut last = Ty::unit();
        for s in &b.stmts {
            match s {
                Stmt::Let { name, value, mutable, .. } => {
                    let t = self.infer(value);
                    self.bind_info(name, t, *mutable);
                    last = Ty::unit();
                }
                Stmt::Assign { name, value, span } => {
                    self.assign(name, value, *span);
                    last = Ty::unit();
                }
                Stmt::Defer(e, _) => {
                    self.fn_depth += 1;
                    self.infer(e);
                    self.fn_depth -= 1;
                    last = Ty::unit();
                }
                Stmt::Expr(e) => last = self.infer(e),
            }
        }
        if has_defer {
            self.barriers -= 1;
        }
        self.scopes.pop();
        last
    }

    fn assign(&mut self, name: &Ident, value: &Expr, _span: Span) {
        let Some(info) = self.lookup_info(&name.name).cloned() else {
            self.infer(value);
            let d = Diagnostic::new(name.span, format!("cannot assign to `{}`: it is not defined", name.name))
                .with_label("not defined")
                .with_help(format!("declare it first with `var {} = ...`", name.name));
            self.diags.push(d);
            return;
        };
        if !info.var {
            self.infer(value);
            let d = Diagnostic::new(name.span, format!("cannot assign to `{}`: it is not a `var`", name.name))
                .with_label("immutable binding")
                .with_help(format!("declare it with `var {} = ...` to be able to change it", name.name));
            self.diags.push(d);
            return;
        }
        if info.fn_depth < self.fn_depth {
            self.capture_error(&name.name, name.span);
        }
        self.check(value, &info.ty);
    }

    fn loop_body(&mut self, body: &Block) {
        self.loops.push(self.barriers);
        let t = self.infer_block(body);
        self.loops.pop();
        if self.unify(&Ty::unit(), &t).is_err() {
            let msg = format!("the body of a loop must have type `()`, found `{}`", self.show(&t));
            self.err(tail_span(body), msg);
        }
    }

    fn jump_allowed(&mut self, what: &str, span: Span) {
        match self.loops.last() {
            None => {
                let d = Diagnostic::new(span, format!("`{what}` outside of a loop")).with_label("not inside a `while` or `for`");
                self.diags.push(d);
            }
            Some(&b) if b != self.barriers => {
                let d = Diagnostic::new(span, format!("`{what}` cannot leave a `region`, `with` or a block that has `defer`"))
                    .with_label("crosses a cleanup boundary")
                    .with_help(format!("end that block first, then `{what}` after it (for example by setting a `var` flag)"));
                self.diags.push(d);
            }
            _ => {}
        }
    }

    fn infer_lambda(&mut self, params: &[Param], body: &Expr, hint: Option<(Vec<Ty>, Ty)>) -> Ty {
        self.fn_depth += 1;
        let saved_loops = std::mem::take(&mut self.loops);
        let saved_barriers = std::mem::replace(&mut self.barriers, 0);
        let saved_unsafe = std::mem::replace(&mut self.unsafe_depth, 0);
        let t = self.infer_lambda_inner(params, body, hint);
        self.unsafe_depth = saved_unsafe;
        self.barriers = saved_barriers;
        self.loops = saved_loops;
        self.fn_depth -= 1;
        t
    }

    fn infer_lambda_inner(&mut self, params: &[Param], body: &Expr, hint: Option<(Vec<Ty>, Ty)>) -> Ty {
        self.scopes.push(HashMap::new());
        let gens = self.cur_generics.clone();
        let mut ptys = vec![];
        for (i, p) in params.iter().enumerate() {
            let hinted = hint.as_ref().map(|h| h.0[i].clone());
            let t = match (&p.ty, hinted) {
                (Some(a), Some(h)) => {
                    let t = self.resolve_type(a, &gens);
                    self.expect(&h, &t, p.name.span);
                    t
                }
                (Some(a), None) => self.resolve_type(a, &gens),
                (None, Some(h)) => h,
                (None, None) => self.fresh(Kind::Any),
            };
            self.bind_param(&p.name, t.clone());
            ptys.push(t);
        }
        let ret = match hint {
            Some((_, r)) => r,
            None => self.fresh(Kind::Any),
        };
        self.ret_stack.push(ret.clone());
        self.check(body, &ret);
        self.ret_stack.pop();
        self.scopes.pop();
        Ty::Fn(ptys, Box::new(ret))
    }

    /// `asm("text", a, b, ...)`: inline assembly. Needs `unsafe`; the inputs are integers placed in
    /// `rdi, rsi, rdx, rcx, r8, r9`, and the result (`rax`) is an `i64`.
    fn infer_asm(&mut self, args: Vec<&Expr>, span: Span) -> Ty {
        if self.unsafe_depth == 0 {
            self.err(span, "`asm` is unsafe: call it inside an `unsafe do ... end` block".to_string());
        }
        match args.first().map(|a| &a.kind) {
            Some(ExprKind::Str(text)) => {
                if let Err(m) = cera_syntax::asm::assemble(text) {
                    self.err(args[0].span, m);
                }
            }
            _ => self.err(span, "`asm` takes a string literal first: asm(\"nop\", input, ...)".to_string()),
        }
        if args.len() > 7 {
            self.err(span, "`asm` takes at most six inputs".to_string());
        }
        for a in args.iter().skip(1) {
            let t = self.infer(a);
            self.oblige_core(t, "Bitwise", a.span, "asm input");
        }
        Ty::prim("i64")
    }

    fn infer_call(&mut self, callee: &Expr, args: Vec<&Expr>, span: Span) -> Ty {
        if let ExprKind::Var(n) = &callee.kind {
            if n == "asm" && self.lookup_info("asm").is_none() && self.lookup_global("asm").is_none() {
                return self.infer_asm(args, span);
            }
        }
        let ct = self.infer(callee);
        match self.shallow(&ct) {
            Ty::Fn(params, ret) => {
                if params.len() != args.len() {
                    self.err(
                        span,
                        format!(
                            "this function takes {} argument(s) but {} were supplied",
                            params.len(),
                            args.len()
                        ),
                    );
                    for a in args {
                        self.infer(a);
                    }
                    return *ret;
                }
                for (p, a) in params.iter().zip(args) {
                    self.check(a, p);
                }
                *ret
            }
            Ty::Var(_) => {
                let ps: Vec<Ty> = args.iter().map(|a| self.infer(a)).collect();
                let r = self.fresh(Kind::Any);
                self.expect(&ct, &Ty::Fn(ps, Box::new(r.clone())), callee.span);
                r
            }
            Ty::Error => {
                for a in args {
                    self.infer(a);
                }
                Ty::Error
            }
            other => {
                let msg = format!("cannot call a value of type `{}`", self.show(&other));
                self.err(callee.span, msg);
                for a in args {
                    self.infer(a);
                }
                Ty::Error
            }
        }
    }

    fn infer_field(&mut self, whole: &Expr, inner: &Expr, name: &Ident) -> Ty {
        if let Some(chain) = flatten(whole) {
            if self.lookup_local(chain[0]).is_none() && self.lookup_global(chain[0]).is_none() {
                return self.module_member(&chain, whole.span);
            }
        }
        let t = self.infer(inner);
        match self.shallow(&t) {
            Ty::Error => Ty::Error,
            Ty::Var(_) => {
                self.err(
                    name.span,
                    format!(
                        "cannot access field `{}`: the type is not known here (add a type annotation)",
                        name.name
                    ),
                );
                Ty::Error
            }
            Ty::Con(id, args) => {
                let info = self.types.get(&id).cloned();
                if let Some(TypeInfo {
                    generics,
                    def: TDef::Record(fields),
                    ..
                }) = info
                {
                    if let Some((_, fty)) = fields.iter().find(|(n, _)| *n == name.name) {
                        let m: HashMap<String, Ty> = generics.into_iter().zip(args).collect();
                        return self.subst(fty, &m);
                    }
                }
                let avail: Vec<String> = match self.types.get(&id) {
                    Some(TypeInfo { def: TDef::Record(f), .. }) => f.iter().map(|(n, _)| n.clone()).collect(),
                    _ => vec![],
                };
                self.no_field(name.span, &short_name(&id), &name.name, &avail);
                Ty::Error
            }
            other => {
                let shown = self.show(&other);
                self.no_field(name.span, &shown, &name.name, &[]);
                Ty::Error
            }
        }
    }

    fn module_member(&mut self, chain: &[&str], span: Span) -> Ty {
        let (m, member) = chain.split_at(chain.len() - 1);
        let member = member[0];
        let found = self.find_module(m).map(|ex| ex.values.get(member).cloned());
        match found {
            Some(Some(Value::Fn(s))) => {
                if self.is_unsafe_module(m) && self.unsafe_depth == 0 {
                    self.err(span, format!("`{}` is unsafe: call it inside an `unsafe do ... end` block", chain.join(".")));
                }
                self.instantiate(&s, span)
            }
            Some(Some(Value::Ctor(c))) => self.ctor_type(&c),
            Some(Some(Value::Const(_))) => Ty::Error,
            Some(None) => {
                self.err(span, format!("module `{}` has no member `{member}`", m.join(".")));
                Ty::Error
            }
            None => {
                if chain.len() == 2 {
                    self.unknown_name(span, chain[0]);
                } else {
                    self.err(span, format!("unknown module `{}`", m.join(".")));
                }
                Ty::Error
            }
        }
    }

    fn infer_record(&mut self, name: &Ident, fields: &[(Ident, Expr)], span: Span) -> Ty {
        let Some(id) = self.vis_types.get(&name.name).cloned() else {
            self.unknown_type(name.span, &name.name);
            for (_, e) in fields {
                self.infer(e);
            }
            return Ty::Error;
        };
        let info = self.types[&id].clone();
        let TDef::Record(decl) = info.def else {
            self.err(name.span, format!("`{}` is not a record type", name.name));
            for (_, e) in fields {
                self.infer(e);
            }
            return Ty::Error;
        };
        let m = self.fresh_map(&info.generics);
        let mut seen = HashSet::new();
        for (fname, fexpr) in fields {
            if !seen.insert(fname.name.clone()) {
                self.err(fname.span, format!("field `{}` is given twice", fname.name));
            }
            match decl.iter().find(|(n, _)| *n == fname.name) {
                Some((_, fty)) => {
                    let expected = self.subst(fty, &m);
                    self.check(fexpr, &expected);
                }
                None => {
                    let avail: Vec<String> = decl.iter().map(|(n, _)| n.clone()).collect();
                    self.no_field(fname.span, &name.name, &fname.name, &avail);
                    self.infer(fexpr);
                }
            }
        }
        let missing: Vec<String> = decl
            .iter()
            .filter(|(n, _)| !seen.contains(n))
            .map(|(n, _)| format!("`{n}`"))
            .collect();
        if !missing.is_empty() {
            self.err(
                span,
                format!("missing field(s) {} in `{}`", missing.join(", "), name.name),
            );
        }
        Ty::Con(id, info.generics.iter().map(|g| m[g].clone()).collect())
    }

    /// `{p | x: 3}`: the type of `p`, with the named fields checked against it.
    fn infer_update(&mut self, base: &Expr, fields: &[(Ident, Expr)]) -> Ty {
        let t = self.infer(base);
        let shallow = self.shallow(&t);
        let decl = match &shallow {
            Ty::Con(id, args) => match self.types.get(id).cloned() {
                Some(TypeInfo { generics, def: TDef::Record(decl), .. }) => {
                    Some((id.clone(), decl, generics.into_iter().zip(args.iter().cloned()).collect::<HashMap<String, Ty>>()))
                }
                _ => None,
            },
            _ => None,
        };
        let Some((id, decl, m)) = decl else {
            if !matches!(shallow, Ty::Error) {
                let shown = self.show(&shallow);
                let span = base.span;
                self.err(span, format!("`{{value | field: ..}}` needs a record, found `{shown}`"));
            }
            for (_, e) in fields {
                self.infer(e);
            }
            return Ty::Error;
        };
        let mut seen = HashSet::new();
        for (fname, fexpr) in fields {
            if !seen.insert(fname.name.clone()) {
                self.err(fname.span, format!("field `{}` is given twice", fname.name));
            }
            match decl.iter().find(|(n, _)| *n == fname.name) {
                Some((_, fty)) => {
                    let expected = self.subst(fty, &m);
                    self.check(fexpr, &expected);
                }
                None => {
                    let avail: Vec<String> = decl.iter().map(|(n, _)| n.clone()).collect();
                    self.no_field(fname.span, &short_name(&id), &fname.name, &avail);
                    self.infer(fexpr);
                }
            }
        }
        shallow
    }

    fn infer_try(&mut self, inner: &Expr, span: Span) -> Ty {
        let t = self.infer(inner);
        let opt = self.prelude.types["Option"].clone();
        let res = self.prelude.types["Result"].clone();
        let Some(ret) = self.ret_stack.last().cloned() else {
            self.err(span, "`?` can only be used inside a function");
            return Ty::Error;
        };
        match self.shallow(&t) {
            Ty::Con(id, args) if id == opt => {
                let want = Ty::Con(opt, vec![self.fresh(Kind::Any)]);
                if self.unify(&want, &ret).is_err() {
                    let msg = format!(
                        "`?` on an Option needs the enclosing function to return an Option, but it returns `{}`",
                        self.show(&ret)
                    );
                    self.err(span, msg);
                }
                args[0].clone()
            }
            Ty::Con(id, args) if id == res => {
                let want = Ty::Con(res, vec![self.fresh(Kind::Any), args[1].clone()]);
                if self.unify(&want, &ret).is_err() {
                    let msg = format!(
                        "`?` on a Result needs the enclosing function to return a Result with error type `{}`, but it returns `{}`",
                        self.show(&args[1]),
                        self.show(&ret)
                    );
                    self.err(span, msg);
                }
                args[0].clone()
            }
            Ty::Var(_) => {
                self.err(span, "the type before `?` is not known here (add a type annotation)");
                Ty::Error
            }
            Ty::Error => Ty::Error,
            other => {
                let msg = format!("`?` can only be used on Option or Result, found `{}`", self.show(&other));
                self.err(span, msg);
                Ty::Error
            }
        }
    }

    // ----- patterns ------------------------------------------------------------

    fn infer_match(&mut self, scrutinee: &Expr, arms: &[Arm]) -> Ty {
        let st = self.infer(scrutinee);
        let result = self.fresh(Kind::Any);
        let before = self.diags.len();
        let mut rows = vec![];
        for arm in arms {
            self.scopes.push(HashMap::new());
            let mut binds = HashSet::new();
            let dp = self.check_pattern(&arm.pat, &st, &mut binds);
            if let Some(g) = &arm.guard {
                self.check(g, &Ty::bool());
            }
            self.check(&arm.body, &result);
            self.scopes.pop();
            if arm.guard.is_none() {
                rows.push(vec![dp]);
            }
        }
        if self.diags.len() == before {
            self.matches.push((scrutinee.span, rows));
        }
        result
    }

    fn check_pattern(&mut self, p: &Pattern, expected: &Ty, binds: &mut HashSet<String>) -> DPat {
        self.rec.push(Rec {
            span: p.span,
            label: None,
            ty: expected.clone(),
        });
        match &p.kind {
            PatKind::Wild => DPat::Wild,
            PatKind::Bind(name) => {
                if !binds.insert(name.clone()) {
                    self.err(p.span, format!("`{name}` is bound more than once in this pattern"));
                }
                let id = Ident {
                    name: name.clone(),
                    span: p.span,
                };
                self.bind_local(&id, expected.clone());
                DPat::Wild
            }
            PatKind::Int(s) => {
                let v = self.fresh(Kind::Int);
                self.expect(expected, &v, p.span);
                DPat::Ctor(Con::Lit(s.clone()), vec![])
            }
            PatKind::Float(s) => {
                let v = self.fresh(Kind::Float);
                self.expect(expected, &v, p.span);
                DPat::Ctor(Con::Lit(s.clone()), vec![])
            }
            PatKind::Str(s) => {
                self.expect(expected, &Ty::string(), p.span);
                DPat::Ctor(Con::Lit(format!("{s:?}")), vec![])
            }
            PatKind::Bool(b) => {
                self.expect(expected, &Ty::bool(), p.span);
                DPat::Ctor(Con::Bool(*b), vec![])
            }
            PatKind::Ctor { name, args } => {
                let c = match self.lookup_global(&name.name) {
                    Some(Value::Ctor(c)) => c,
                    Some(_) => {
                        self.err(name.span, format!("`{}` is not a constructor", name.name));
                        return self.pattern_error(args, binds);
                    }
                    None => {
                        let mut d = Diagnostic::new(name.span, format!("unknown constructor `{}`", name.name)).with_label("not found");
                        let ctors: Vec<String> = self
                            .user
                            .values
                            .iter()
                            .chain(self.prelude.values.iter())
                            .filter(|(_, v)| matches!(v, Value::Ctor(_)))
                            .map(|(k, _)| k.clone())
                            .collect();
                        if let Some(c) = closest(&name.name, ctors.iter().map(String::as_str)) {
                            d = d.with_help(format!("a similar constructor exists: `{c}`"));
                        }
                        self.err_d(d);
                        return self.pattern_error(args, binds);
                    }
                };
                let m = self.fresh_map(&c.generics);
                let result = Ty::Con(
                    c.type_id.clone(),
                    c.generics.iter().map(|g| m[g].clone()).collect(),
                );
                self.expect(expected, &result, p.span);
                if args.len() != c.fields.len() {
                    self.err(
                        p.span,
                        format!(
                            "constructor `{}` has {} field(s), but the pattern has {}",
                            name.name,
                            c.fields.len(),
                            args.len()
                        ),
                    );
                    return self.pattern_error(args, binds);
                }
                let subs = args
                    .iter()
                    .zip(&c.fields)
                    .map(|(a, f)| {
                        let ft = self.subst(f, &m);
                        self.check_pattern(a, &ft, binds)
                    })
                    .collect();
                DPat::Ctor(
                    Con::Variant {
                        ty: c.type_id.clone(),
                        index: c.index,
                    },
                    subs,
                )
            }
            PatKind::Record { name, fields } => {
                let Some(id) = self.vis_types.get(&name.name).cloned() else {
                    self.unknown_type(name.span, &name.name);
                    return DPat::Wild;
                };
                let info = self.types[&id].clone();
                let TDef::Record(decl) = info.def else {
                    self.err(name.span, format!("`{}` is not a record type", name.name));
                    return DPat::Wild;
                };
                let m = self.fresh_map(&info.generics);
                let result = Ty::Con(
                    id.clone(),
                    info.generics.iter().map(|g| m[g].clone()).collect(),
                );
                self.expect(expected, &result, p.span);
                let mut subs = vec![DPat::Wild; decl.len()];
                for (fname, fpat) in fields {
                    let Some(pos) = decl.iter().position(|(n, _)| *n == fname.name) else {
                        let avail: Vec<String> = decl.iter().map(|(n, _)| n.clone()).collect();
                        self.no_field(fname.span, &name.name, &fname.name, &avail);
                        continue;
                    };
                    let ft = self.subst(&decl[pos].1, &m);
                    subs[pos] = match fpat {
                        Some(sp) => self.check_pattern(sp, &ft, binds),
                        None => {
                            if !binds.insert(fname.name.clone()) {
                                self.err(
                                    fname.span,
                                    format!("`{}` is bound more than once in this pattern", fname.name),
                                );
                            }
                            self.bind_local(fname, ft);
                            DPat::Wild
                        }
                    };
                }
                DPat::Ctor(Con::Record(id, decl.len()), subs)
            }
            PatKind::Tuple(items) => {
                let vars: Vec<Ty> = items.iter().map(|_| self.fresh(Kind::Any)).collect();
                self.expect(expected, &Ty::Tuple(vars.clone()), p.span);
                let subs = items
                    .iter()
                    .zip(&vars)
                    .map(|(it, v)| self.check_pattern(it, v, binds))
                    .collect();
                DPat::Ctor(Con::Tuple(items.len()), subs)
            }
            PatKind::List { items, tail } => {
                let elem = self.fresh(Kind::Any);
                let list = Ty::List(Box::new(elem.clone()));
                self.expect(expected, &list, p.span);
                let heads: Vec<DPat> = items
                    .iter()
                    .map(|it| self.check_pattern(it, &elem, binds))
                    .collect();
                let mut acc = match tail {
                    Some(t) => self.check_pattern(t, &list, binds),
                    None => DPat::Ctor(Con::Nil, vec![]),
                };
                for h in heads.into_iter().rev() {
                    acc = DPat::Ctor(Con::Cons, vec![h, acc]);
                }
                acc
            }
        }
    }

    /// After a bad constructor pattern, still bind the names in its sub-patterns
    /// (with error types) so later uses do not report spurious unknown names.
    fn pattern_error(&mut self, args: &[Pattern], binds: &mut HashSet<String>) -> DPat {
        for a in args {
            self.check_pattern(a, &Ty::Error, binds);
        }
        DPat::Wild
    }
}


const STD_MODULES: &[&str] = &["io", "list", "string", "option", "result", "num", "bits", "array", "map", "set", "env", "time", "fs", "mem", "sys", "process"];

/// Attach a label and advice to a message-only diagnostic from its wording.
fn enrich(d: Diagnostic) -> Diagnostic {
    let m = d.message.clone();
    let tick = |s: &str| -> Option<String> {
        let start = s.find('`')? + 1;
        let end = s[start..].find('`')? + start;
        Some(s[start..end].to_string())
    };
    if let Some(rest) = m.strip_prefix("type mismatch: expected `") {
        if let Some((exp, found)) = rest.split_once("`, found `") {
            let found = found.trim_end_matches('`');
            let d = d.with_label(format!("expected `{exp}`, found `{found}`"));
            const INTS: &[&str] = &["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "usize"];
            return if INTS.contains(&exp) && INTS.contains(&found) || (exp == "f64" && INTS.contains(&found)) || (INTS.contains(&exp) && found == "f64") {
                d.with_help(format!("convert explicitly with `num.to_{exp}(..)` (add `import std.num`); numeric types are never converted implicitly"))
            } else if exp == "String" && (INTS.contains(&found) || found == "f64" || found == "bool") {
                d.with_help("convert the value with `show(..)`")
            } else if let Some(inner) = exp.strip_prefix("Option<") {
                if inner.trim_end_matches('>') == found {
                    d.with_help(format!("wrap the value in `Some(..)`"))
                } else {
                    d
                }
            } else {
                d
            };
        }
    }
    if m.starts_with("function `") && m.contains("should return `") {
        let ret = m.split("should return `").nth(1).and_then(|r| r.split('`').next()).unwrap_or("").to_string();
        return d.with_label("the body's value has the wrong type").with_help(format!("change the return type, or convert the result to `{ret}`"));
    }
    if m.starts_with("non-exhaustive match") {
        let pat = tick(&m).unwrap_or_default();
        return d
            .with_label(format!("pattern `{pat}` not covered"))
            .with_help(format!("add an arm for `{pat}`, or a wildcard arm `_ -> ...`"));
    }
    if m.starts_with("the trait `") && m.contains("is not implemented for") {
        let tr = tick(&m).unwrap_or_default();
        let ty = m.rsplit('`').nth(1).unwrap_or("").to_string();
        return d.with_label(format!("`{ty}` does not implement `{tr}`")).with_help(format!("implement it with `impl {tr} for {ty} do ... end`"));
    }
    if m.contains("needs a type annotation") {
        let p = tick(&m).unwrap_or_default();
        return d.with_label("type not given").with_help(format!("write the parameter as `{p}: Type`"));
    }
    if m.starts_with("`if` without `else`") {
        return d
            .with_label("this branch produces a value, but there is no `else`")
            .with_help("add an `else` branch, or make the body end in `()`");
    }
    if m.starts_with("`?` can only be used") {
        return d.with_help("`?` unwraps an `Option` or `Result` and returns early on `None` or `Err`; apply it to one of those");
    }
    if m.starts_with("unknown module `") {
        let name = tick(&m).unwrap_or_default();
        let first = name.split('.').next().unwrap_or("").to_string();
        let d = d.with_label("module not found");
        return if STD_MODULES.contains(&first.as_str()) {
            d.with_help(format!("add `import std.{first}` at the top of the file"))
        } else {
            d
        };
    }
    if m.contains(" is a module, not a value") {
        return d.with_help("call one of its functions, for example `module.function(..)`");
    }
    if m.starts_with("missing field(s)") {
        return d.with_label("fields missing here").with_help("give a value for every field of the record");
    }
    if m.contains("is already defined") || m.contains("is defined twice") || m.contains("is declared twice") {
        return d.with_label("duplicate definition").with_help("rename one of them, or remove the duplicate");
    }
    if m.starts_with("cannot call a value of type") {
        return d.with_label("not a function");
    }
    if m.starts_with("missing method(s)") {
        return d.with_label("incomplete impl").with_help("define every method the trait declares");
    }
    d
}
