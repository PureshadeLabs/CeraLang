//! The linker: loads every module a package uses and merges them into one
//! `Program` that the unchanged checker and backends can process.
//!
//! Each non-root module's top-level names are renamed to `path·name` (the first
//! letter keeps its case, so type and constructor names stay upper-case), and
//! every reference is rewritten to match. Spans are moved into one virtual text
//! (see [`SourceMap`]) so they stay unique.

use crate::resolve::Graph;
use crate::srcmap::SourceMap;
use cera_syntax::ast::*;
use cera_syntax::{closest, parse, Diagnostic, Span, MODULE_SEP};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Where linking starts.
pub enum Root {
    /// A file of the root package (its names are not renamed).
    File(PathBuf),
    /// Generated source, for example the test harness.
    Text { name: String, text: String },
}

pub struct Linked {
    pub program: Program,
    pub map: SourceMap,
    pub diagnostics: Vec<Diagnostic>,
    /// Every loaded module (for editors).
    pub modules: Vec<ModuleInfo>,
}

/// What an `import` names.
#[derive(Clone, Debug)]
pub enum AliasTarget {
    /// Index into `Linked::modules`.
    Module(usize),
    Std(String),
}

#[derive(Clone, Debug)]
pub struct ModuleInfo {
    /// The source file (None for the built-in `kote.test`).
    pub file: Option<PathBuf>,
    pub aliases: Vec<(String, AliasTarget)>,
}

const TEST_SUPPORT: &str = include_str!("test_support.cera");

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum ModKey {
    Root,
    Pkg(usize, Vec<String>),
    Builtin,
}

#[derive(Clone, Debug)]
enum Target {
    /// A standard-library module, by the name it is known as in a program.
    Std(String),
    Mod(usize),
}

struct Module {
    file: Option<PathBuf>,
    prefix: Vec<String>,
    program: Program,
    top: HashSet<String>,
    /// The names other modules may use (`pub`).
    public: HashSet<String>,
    /// Where each top-level name is defined.
    defs: HashMap<String, Span>,
    aliases: HashMap<String, Target>,
}

/// What the renamer needs to know about every module.
struct Table {
    prefix: Vec<String>,
    top: HashSet<String>,
    public: HashSet<String>,
    defs: HashMap<String, Span>,
    name: String,
}

fn mangle(prefix: &[String], name: &str) -> String {
    if prefix.is_empty() {
        return name.to_string();
    }
    let mut s = prefix.join(&MODULE_SEP.to_string());
    s.push(MODULE_SEP);
    s.push_str(name);
    if is_upper_name(name) {
        let mut cs = s.chars();
        if let Some(c) = cs.next() {
            s = c.to_ascii_uppercase().to_string() + cs.as_str();
        }
    }
    s
}

/// `(all top-level names, the public ones, where each is defined)`.
fn top_names(p: &Program) -> (HashSet<String>, HashSet<String>, HashMap<String, Span>) {
    let (mut t, mut public, mut defs) = (HashSet::new(), HashSet::new(), HashMap::new());
    for item in &p.items {
        if let Item::Decl(d) = item {
            let mut add = |n: &Ident| {
                t.insert(n.name.clone());
                defs.insert(n.name.clone(), n.span);
                if d.public {
                    public.insert(n.name.clone());
                }
            };
            add(&d.name);
            match &d.kind {
                DeclKind::Type(TypeDef::Variants(vs)) => {
                    for v in vs {
                        add(&v.name);
                    }
                }
                DeclKind::Trait(tr) => {
                    for m in &tr.methods {
                        add(&m.name);
                    }
                }
                _ => {}
            }
        }
    }
    (t, public, defs)
}

struct Loader<'g> {
    graph: &'g Graph,
    overlay: HashMap<PathBuf, String>,
    map: SourceMap,
    mods: Vec<Module>,
    index: HashMap<ModKey, usize>,
    diags: Vec<Diagnostic>,
    stack: Vec<(ModKey, String)>,
    order: Vec<usize>,
    /// Imports of the standard library to keep: joined path -> import.
    std_imports: Vec<Import>,
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).map(|r| r.display().to_string()).unwrap_or_else(|_| p.display().to_string())
}

impl<'g> Loader<'g> {
    fn pkg_of(&self, key: &ModKey) -> Option<usize> {
        match key {
            ModKey::Root => Some(0),
            ModKey::Pkg(i, _) => Some(*i),
            ModKey::Builtin => None,
        }
    }

    fn prefix_of(&self, key: &ModKey) -> Vec<String> {
        match key {
            ModKey::Root => vec![],
            ModKey::Builtin => vec!["kote".into(), "test".into()],
            ModKey::Pkg(0, path) => path.clone(),
            ModKey::Pkg(i, path) => {
                let mut v = vec![self.graph.pkgs[*i].link.clone()];
                if path != &["lib".to_string()] {
                    v.extend(path.iter().cloned());
                }
                v
            }
        }
    }

    /// Parse `text` as a module, register it, and load what it imports.
    fn read(&self, path: &Path) -> std::io::Result<String> {
        match self.overlay.get(path) {
            Some(t) => Ok(t.clone()),
            None => std::fs::read_to_string(path),
        }
    }

    fn load(&mut self, key: ModKey, display: String, text: String, file: Option<PathBuf>) -> usize {
        let base = self.map.add(display.clone(), text.clone());
        let parsed = parse(&format!("{}{}", " ".repeat(base), text));
        self.diags.extend(parsed.diagnostics.iter().cloned());
        let program = parsed.program;
        let idx = self.mods.len();
        let prefix = self.prefix_of(&key);
        let (top, public, defs) = top_names(&program);
        self.mods.push(Module { file, prefix, top, public, defs, program: Program { items: vec![] }, aliases: HashMap::new() });
        self.index.insert(key.clone(), idx);
        self.stack.push((key.clone(), display));
        let mut aliases: HashMap<String, Target> = HashMap::new();
        let mut kept = vec![];
        for item in program.items {
            let Item::Import(imp) = item else {
                kept.push(item);
                continue;
            };
            let alias_name = imp.alias.as_ref().map_or_else(|| imp.path.last().map(|i| i.name.clone()).unwrap_or_default(), |a| a.name.clone());
            let target = match self.resolve_import(&key, &imp) {
                Ok(t) => t,
                Err(d) => {
                    self.diags.push(d);
                    continue;
                }
            };
            if aliases.contains_key(&alias_name) {
                self.diags.push(
                    Diagnostic::new(imp.span, format!("`{alias_name}` is already imported"))
                        .with_label("duplicate import")
                        .with_help("rename one of them with `import path as other_name`"),
                );
                continue;
            }
            aliases.insert(alias_name, target);
        }
        self.mods[idx].program = Program { items: kept };
        self.mods[idx].aliases = aliases;
        self.stack.pop();
        self.order.push(idx);
        idx
    }

    fn resolve_import(&mut self, from: &ModKey, imp: &Import) -> Result<Target, Diagnostic> {
        let path: Vec<&str> = imp.path.iter().map(|i| i.name.as_str()).collect();
        let joined = path.join(".");
        if path.first() == Some(&"std") {
            if path.len() < 2 {
                return Err(Diagnostic::new(imp.span, "`std` is a package of modules; import one of them").with_help("for example `import std.io`"));
            }
            let canon = path.last().unwrap().to_string();
            let key = path.join(".");
            if !self.std_imports.iter().any(|i| i.path.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(".") == key) {
                self.std_imports.push(Import { path: imp.path.clone(), alias: None, span: imp.span });
            }
            return Ok(Target::Std(canon));
        }
        if path == ["kote", "test"] {
            let key = ModKey::Builtin;
            if let Some(&i) = self.index.get(&key) {
                return Ok(Target::Mod(i));
            }
            return Ok(Target::Mod(self.load(key, "<kote::test>".into(), TEST_SUPPORT.into(), None)));
        }
        let Some(p) = self.pkg_of(from) else {
            return Err(Diagnostic::new(imp.span, format!("unknown module `{joined}`")));
        };
        let pkg = &self.graph.pkgs[p];
        // 1. the package's own library, 2. a dependency, 3. a module of this package.
        let (target_pkg, rest): (usize, Vec<String>) = if path[0] == pkg.name {
            (p, if path.len() == 1 { vec!["lib".into()] } else { path[1..].iter().map(|s| s.to_string()).collect() })
        } else if let Some((_, d)) = pkg.deps.iter().find(|(n, _)| n == path[0]) {
            (*d, if path.len() == 1 { vec!["lib".into()] } else { path[1..].iter().map(|s| s.to_string()).collect() })
        } else {
            (p, path.iter().map(|s| s.to_string()).collect())
        };
        let tp = &self.graph.pkgs[target_pkg];
        let mut candidates: Vec<PathBuf> = vec![];
        let sub: PathBuf = rest.iter().collect();
        if target_pkg == 0 && (rest[0] == "tests" || rest[0] == "benches") && rest.len() > 1 {
            let t: PathBuf = rest[1..].iter().collect();
            candidates.push(tp.dir.join(&rest[0]).join(&t).with_extension("cera"));
        } else {
            if target_pkg == 0 && rest == ["main"] {
                return Err(Diagnostic::new(imp.span, "the entry module `main` cannot be imported")
                    .with_label("not importable")
                    .with_help("move the shared code into another file under `src/`"));
            }
            candidates.push(tp.dir.join("src").join(&sub).with_extension("cera"));
            candidates.push(tp.dir.join("src").join(&sub).join("mod.cera"));
        }
        let Some(file) = candidates.iter().find(|c| c.is_file()).cloned() else {
            let mut d = Diagnostic::new(imp.span, format!("unknown module `{joined}`")).with_label("module not found");
            d = d.with_note(format!("looked for {}", candidates.iter().map(|c| format!("`{}`", rel(&self.graph.pkgs[0].dir, c))).collect::<Vec<_>>().join(" and ")));
            if path.len() == 1 && target_pkg == p && !pkg.deps.iter().any(|(n, _)| n == path[0]) {
                let names: Vec<&str> = pkg.deps.iter().map(|(n, _)| n.as_str()).collect();
                if let Some(c) = closest(path[0], names.iter().copied()) {
                    d = d.with_help(format!("a dependency with a similar name exists: `{c}`"));
                } else {
                    d = d.with_help(format!("to use a package, add it with `kote add {} --path <dir>` or `kote add {} --git <url>`", path[0], path[0]));
                }
            }
            return Err(d);
        };
        let key = if target_pkg == 0 && (rest[0] == "tests" || rest[0] == "benches") { ModKey::Pkg(0, rest.clone()) } else { ModKey::Pkg(target_pkg, rest.clone()) };
        if let Some(&i) = self.index.get(&key) {
            if let Some(pos) = self.stack.iter().position(|(k, _)| *k == key) {
                let mut chain: Vec<String> = self.stack[pos..].iter().map(|(_, n)| n.clone()).collect();
                chain.push(self.stack[pos].1.clone());
                return Err(Diagnostic::new(imp.span, "circular import")
                    .with_label("this import closes a cycle")
                    .with_note(format!("{}", chain.join(" -> ")))
                    .with_help("move the shared definitions into a third module that both can import"));
            }
            return Ok(Target::Mod(i));
        }
        let text = self.read(&file).map_err(|e| Diagnostic::new(imp.span, format!("cannot read {}: {e}", file.display())))?;
        let display = if target_pkg == 0 { rel(&self.graph.pkgs[0].dir, &file) } else { format!("{}/{}", tp.link, rel(&tp.dir, &file)) };
        Ok(Target::Mod(self.load(key, display, text, Some(file))))
    }
}

// ------------------------------------------------------------------ renaming

struct Ren<'a> {
    me: usize,
    mods: &'a [Table],
    aliases: &'a HashMap<String, Target>,
    locals: Vec<HashSet<String>>,
    tparams: Vec<HashSet<String>>,
    diags: &'a mut Vec<Diagnostic>,
}

impl Ren<'_> {
    fn is_local(&self, n: &str) -> bool {
        self.locals.iter().any(|s| s.contains(n))
    }

    fn is_tparam(&self, n: &str) -> bool {
        self.tparams.iter().any(|s| s.contains(n))
    }

    fn mine(&self, n: &str) -> Option<String> {
        self.mods[self.me].top.contains(n).then(|| mangle(&self.mods[self.me].prefix, n))
    }

    fn member(&mut self, m: usize, name: &str, span: Span, alias: &str) -> Option<String> {
        // Test functions are reachable from the generated harness without `pub`.
        let test_fn = self.mods[m].top.contains(name)
            && ((self.mods[m].name.starts_with("tests.") && name.starts_with("test_")) || (self.mods[m].name.starts_with("benches.") && name.starts_with("bench_")));
        if self.mods[m].public.contains(name) || test_fn {
            return Some(mangle(&self.mods[m].prefix, name));
        }
        if self.mods[m].top.contains(name) && m != self.me {
            let mut d = Diagnostic::new(span, format!("`{name}` is private to module `{}`", self.mods[m].name))
                .with_label("private")
                .with_help(format!("make it public by writing `pub` before its definition in {}", self.mods[m].name));
            if let Some(def) = self.mods[m].defs.get(name) {
                d = d.with_secondary(*def, "defined here, without `pub`");
            }
            self.diags.push(d);
            return None;
        }
        let publics = self.mods[m].public.iter().map(String::as_str);
        let mut d = Diagnostic::new(span, format!("module `{alias}` has no member `{name}`")).with_label("not found in that module");
        if let Some(c) = closest(name, publics) {
            d = d.with_help(format!("a similar name exists: `{alias}.{c}`"));
        }
        self.diags.push(d);
        None
    }

    /// A possibly qualified type, constructor or trait name.
    fn type_name(&mut self, id: &mut Ident) {
        if let Some((a, rest)) = id.name.clone().split_once('.') {
            match self.aliases.get(a).cloned() {
                Some(Target::Mod(m)) => {
                    if let Some(n) = self.member(m, rest, id.span, a) {
                        id.name = n;
                    }
                }
                Some(Target::Std(c)) => id.name = format!("{c}.{rest}"),
                None => {
                    let mut d = Diagnostic::new(id.span, format!("`{a}` is not an imported module")).with_label("unknown module");
                    d = d.with_help(format!("add `import {a}` (or the full module path) at the top of the file"));
                    self.diags.push(d);
                }
            }
        } else if !self.is_tparam(&id.name) {
            if let Some(n) = self.mine(&id.name) {
                id.name = n;
            }
        }
    }

    fn ty(&mut self, t: &mut Type) {
        match &mut t.kind {
            TypeKind::Path { path, args } => {
                for a in args.iter_mut() {
                    self.ty(a);
                }
                if path.len() == 1 {
                    if !self.is_tparam(&path[0].name) {
                        if let Some(n) = self.mine(&path[0].name) {
                            path[0].name = n;
                        }
                    }
                } else if path[0].name != "std" {
                    let a = path[0].name.clone();
                    match self.aliases.get(&a).cloned() {
                        Some(Target::Mod(m)) if path.len() == 2 => {
                            if let Some(n) = self.member(m, &path[1].name.clone(), path[1].span, &a) {
                                let span = path[0].span.to(path[1].span);
                                *path = vec![Ident { name: n, span }];
                            }
                        }
                        Some(Target::Std(c)) => path[0].name = c,
                        _ => {}
                    }
                }
            }
            TypeKind::Fn { params, ret } => {
                for p in params {
                    self.ty(p);
                }
                self.ty(ret);
            }
            TypeKind::List(e) | TypeKind::Ref(e) => self.ty(e),
            TypeKind::Tuple(v) => {
                for e in v {
                    self.ty(e);
                }
            }
        }
    }

    fn bounds(&mut self, bs: &mut [Bound]) {
        for b in bs {
            for t in &mut b.traits {
                self.type_name(t);
            }
        }
    }

    fn bind(&mut self, n: &str) {
        if let Some(s) = self.locals.last_mut() {
            s.insert(n.to_string());
        }
    }

    fn pat(&mut self, p: &mut Pattern) {
        match &mut p.kind {
            PatKind::Bind(n) => {
                let n = n.clone();
                self.bind(&n);
            }
            PatKind::Ctor { name, args } => {
                self.type_name(name);
                for a in args {
                    self.pat(a);
                }
            }
            PatKind::Record { name, fields } => {
                self.type_name(name);
                for (f, sub) in fields {
                    match sub {
                        Some(sp) => self.pat(sp),
                        None => {
                            let n = f.name.clone();
                            self.bind(&n);
                        }
                    }
                }
            }
            PatKind::Tuple(v) => {
                for x in v {
                    self.pat(x);
                }
            }
            PatKind::List { items, tail } => {
                for x in items {
                    self.pat(x);
                }
                if let Some(t) = tail {
                    self.pat(t);
                }
            }
            _ => {}
        }
    }

    fn block(&mut self, b: &mut Block) {
        self.locals.push(HashSet::new());
        for s in &mut b.stmts {
            match s {
                Stmt::Let { name, value, .. } => {
                    self.expr(value);
                    let n = name.name.clone();
                    self.bind(&n);
                }
                Stmt::Assign { value, .. } => self.expr(value),
                Stmt::Defer(e, _) => self.expr(e),
                Stmt::Expr(e) => self.expr(e),
            }
        }
        self.locals.pop();
    }

    fn expr(&mut self, e: &mut Expr) {
        let span = e.span;
        match &mut e.kind {
            ExprKind::Var(n) => {
                if !self.is_local(n) {
                    if let Some(m) = self.mine(n) {
                        *n = m;
                    }
                }
            }
            ExprKind::Field { expr, name } => {
                if let ExprKind::Var(a) = &mut expr.kind {
                    let a = a.clone();
                    if !self.is_local(&a) && self.mine(&a).is_none() {
                        match self.aliases.get(&a).cloned() {
                            Some(Target::Mod(m)) => {
                                let nm = name.name.clone();
                                let nsp = name.span;
                                if let Some(mangled) = self.member(m, &nm, nsp, &a) {
                                    e.kind = ExprKind::Var(mangled);
                                    e.span = span;
                                }
                                return;
                            }
                            Some(Target::Std(c)) => {
                                expr.kind = ExprKind::Var(c);
                                return;
                            }
                            None => {}
                        }
                    }
                }
                self.expr(expr);
            }
            ExprKind::Update { base, fields } => {
                self.expr(base);
                for (_, v) in fields {
                    self.expr(v);
                }
            }
            ExprKind::Record { name, fields } => {
                self.type_name(name);
                for (_, v) in fields {
                    self.expr(v);
                }
            }
            ExprKind::List(v) | ExprKind::Tuple(v) => {
                for x in v {
                    self.expr(x);
                }
            }
            ExprKind::Cons { items, tail } => {
                for x in items {
                    self.expr(x);
                }
                self.expr(tail);
            }
            ExprKind::Unary { expr, .. } => self.expr(expr),
            ExprKind::Binary { lhs, rhs, .. } | ExprKind::Pipe { lhs, rhs } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            ExprKind::Call { callee, args } => {
                self.expr(callee);
                for a in args {
                    self.expr(a);
                }
            }
            ExprKind::Index { expr, index } => {
                self.expr(expr);
                self.expr(index);
            }
            ExprKind::Try(x) => self.expr(x),
            ExprKind::Lambda { params, body } => {
                self.locals.push(params.iter().map(|p| p.name.name.clone()).collect());
                for p in params.iter_mut() {
                    if let Some(t) = &mut p.ty {
                        self.ty(t);
                    }
                }
                self.expr(body);
                self.locals.pop();
            }
            ExprKind::If { cond, then, els } => {
                self.expr(cond);
                self.block(then);
                if let Some(b) = els {
                    self.block(b);
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                self.expr(scrutinee);
                for arm in arms {
                    self.locals.push(HashSet::new());
                    self.pat(&mut arm.pat);
                    if let Some(g) = &mut arm.guard {
                        self.expr(g);
                    }
                    self.expr(&mut arm.body);
                    self.locals.pop();
                }
            }
            ExprKind::Region(b) | ExprKind::Block(b) | ExprKind::Unsafe(b) => self.block(b),
            ExprKind::While { cond, body } => {
                self.expr(cond);
                self.block(body);
            }
            ExprKind::For { var, iter, body } => {
                self.expr(iter);
                self.locals.push(HashSet::new());
                let n = var.name.clone();
                self.bind(&n);
                self.block(body);
                self.locals.pop();
            }
            ExprKind::Break | ExprKind::Continue => {}
            ExprKind::With { arg, body, .. } => {
                self.expr(arg);
                self.block(body);
            }
            ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Str(_) | ExprKind::Bool(_) | ExprKind::Unit => {}
        }
    }

    fn func(&mut self, f: &mut FnDef) {
        self.tparams.push(f.generics.iter().map(|g| g.name.clone()).collect());
        self.bounds(&mut f.bounds);
        for p in &mut f.params {
            if let Some(t) = &mut p.ty {
                self.ty(t);
            }
        }
        if let Some(r) = &mut f.ret {
            self.ty(r);
        }
        if let Some(body) = &mut f.body {
            self.locals.push(f.params.iter().map(|p| p.name.name.clone()).collect());
            self.block(body);
            self.locals.pop();
        }
        self.tparams.pop();
    }

    fn decl(&mut self, d: &mut Decl, name_prefix: Option<&[String]>) {
        let prefix = name_prefix.map(<[String]>::to_vec).unwrap_or_else(|| self.mods[self.me].prefix.clone());
        d.name.name = mangle(&prefix, &d.name.name);
        self.tparams.push(d.generics.iter().map(|g| g.name.clone()).collect());
        match &mut d.kind {
            DeclKind::Fn(f) => self.func(f),
            DeclKind::Type(td) => match td {
                TypeDef::Variants(vs) => {
                    for v in vs {
                        v.name.name = mangle(&prefix, &v.name.name);
                        for f in &mut v.fields {
                            self.ty(&mut f.ty);
                        }
                    }
                }
                TypeDef::Record(fs) => {
                    for f in fs {
                        self.ty(&mut f.ty);
                    }
                }
                TypeDef::Opaque => {}
            },
            DeclKind::Trait(tr) => {
                self.tparams.push(["Self".to_string()].into_iter().collect());
                for m in &mut tr.methods {
                    m.name.name = mangle(&prefix, &m.name.name);
                    for p in &mut m.params {
                        if let Some(t) = &mut p.ty {
                            self.ty(t);
                        }
                    }
                    if let Some(r) = &mut m.ret {
                        self.ty(r);
                    }
                }
                self.tparams.pop();
            }
            DeclKind::Const(e) => {
                self.locals.push(HashSet::new());
                self.expr(e);
                self.locals.pop();
            }
        }
        self.tparams.pop();
    }

    fn imp(&mut self, i: &mut ImplDef) {
        self.tparams.push(i.generics.iter().map(|g| g.name.clone()).collect());
        self.bounds(&mut i.bounds);
        // The trait's methods carry the prefix of the module that defines the trait.
        let owner: Option<Vec<String>> = match i.trait_name.name.split_once('.') {
            Some((a, _)) => match self.aliases.get(a) {
                Some(Target::Mod(m)) => Some(self.mods[*m].prefix.clone()),
                _ => None,
            },
            None if self.mods[self.me].top.contains(&i.trait_name.name) => Some(self.mods[self.me].prefix.clone()),
            None => None,
        };
        self.type_name(&mut i.trait_name);
        self.ty(&mut i.ty);
        for m in &mut i.methods {
            match &owner {
                Some(p) => {
                    let p = p.clone();
                    self.decl(m, Some(&p));
                }
                None => {
                    self.tparams.push(m.generics.iter().map(|g| g.name.clone()).collect());
                    if let DeclKind::Fn(f) = &mut m.kind {
                        self.func(f);
                    }
                    self.tparams.pop();
                }
            }
        }
        self.tparams.pop();
    }
}

/// Load `root` and everything it imports, and merge the modules.
pub fn link(graph: &Graph, root: Root) -> Linked {
    link_with(graph, root, HashMap::new())
}

/// Like [`link`], but files in `overlay` (unsaved editor buffers) win over the disk.
pub fn link_with(graph: &Graph, root: Root, overlay: HashMap<PathBuf, String>) -> Linked {
    let mut l = Loader {
        graph,
        overlay,
        map: SourceMap::default(),
        mods: vec![],
        index: HashMap::new(),
        diags: vec![],
        stack: vec![],
        order: vec![],
        std_imports: vec![],
    };
    let root_path = match &root {
        Root::File(p) => Some(p.clone()),
        Root::Text { .. } => None,
    };
    let (name, text) = match root {
        Root::File(ref path) => match l.read(path) {
            Ok(t) => (rel(&graph.pkgs[0].dir, path), t),
            Err(e) => {
                l.diags.push(Diagnostic::new(Span::default(), format!("cannot read {}: {e}", path.display())));
                (path.display().to_string(), String::new())
            }
        },
        Root::Text { name, text } => (name, text),
    };
    let root_file = match &root_path { Some(p) => Some(p.clone()), None => None };
    l.load(ModKey::Root, name, text, root_file);

    // Rename every module, dependencies first.
    let tables: Vec<Table> = l
        .mods
        .iter()
        .map(|m| Table {
            prefix: m.prefix.clone(),
            top: m.top.clone(),
            public: m.public.clone(),
            defs: m.defs.clone(),
            name: if m.prefix.is_empty() { "main".to_string() } else { m.prefix.join(".") },
        })
        .collect();
    let mut items: Vec<Item> = l.std_imports.iter().cloned().map(Item::Import).collect();
    let order = l.order.clone();
    let mut diags = std::mem::take(&mut l.diags);
    for &i in &order {
        let mut prog = std::mem::replace(&mut l.mods[i].program, Program { items: vec![] });
        let aliases = l.mods[i].aliases.clone();
        let mut r = Ren { me: i, mods: &tables, aliases: &aliases, locals: vec![], tparams: vec![], diags: &mut diags };
        for item in &mut prog.items {
            match item {
                Item::Decl(d) => r.decl(d, None),
                Item::Impl(im) => r.imp(im),
                Item::Import(_) => {}
            }
        }
        items.extend(prog.items);
    }
    // Names for diagnostics.
    for t in &tables {
        if t.prefix.is_empty() {
            continue;
        }
        for n in &t.top {
            l.map.names.push((mangle(&t.prefix, n), format!("{}.{n}", t.prefix.join("."))));
        }
    }
    l.map.names.sort_by_key(|(m, _)| std::cmp::Reverse(m.len()));
    let modules = l
        .mods
        .iter()
        .map(|m| ModuleInfo {
            file: m.file.clone(),
            aliases: m
                .aliases
                .iter()
                .map(|(a, t)| (a.clone(), match t { Target::Mod(i) => AliasTarget::Module(*i), Target::Std(n) => AliasTarget::Std(n.clone()) }))
                .collect(),
        })
        .collect();
    Linked { program: Program { items }, map: l.map, diagnostics: diags, modules }
}
