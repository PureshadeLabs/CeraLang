//! Lowering from the checked AST to Cranelift IR, with monomorphization.
//!
//! Every value is one 64-bit word: integers (sign- or zero-extended to their width),
//! booleans, chars, floats (bit pattern), and pointers to heap objects. Tuples and
//! records are `[field...]`, variants are `[tag, field...]`, strings are `[len, bytes...]`.

use crate::backend::*;
use crate::rt::{self, FIRST_USER};
use cera_sema::defs::{TDef, TypeInfo};
use cera_sema::ty::Ty;
use cera_sema::Analysis;
use cera_syntax::ast::*;
use cranelift_codegen::ir::{self, condcodes::FloatCC, condcodes::IntCC, types::F64, types::I64, InstBuilder, MemFlags, UserFuncName};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use std::collections::HashMap;

type R<T> = Result<T, String>;
type Subst = HashMap<String, Ty>;

fn unsupported<T>(what: &str) -> R<T> {
    Err(format!("{what} is not supported by the native backend yet"))
}

struct Inst<'a> {
    id: u32,
    name: String,
    m: usize,
    decl: &'a FnDef,
    params: Vec<(String, Ty)>,
    subst: Subst,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum GlueKind {
    Show { nested: bool },
    Eq,
    Lt,
}

pub(crate) struct GlueReq {
    id: u32,
    kind: GlueKind,
    ty: Ty,
}

/// One analyzed source module: the user's program (index 0) or the native standard library (1).
pub struct Module<'a> {
    an: &'a Analysis,
    fns: HashMap<String, (&'a Decl, &'a FnDef)>,
    consts: HashMap<String, &'a Expr>,
    /// user impl methods by the start offset of their name
    impl_decls: HashMap<usize, &'a FnDef>,
    /// namespace alias -> std module short name (`list`, `io`, ...)
    ns: HashMap<String, String>,
}

impl<'a> Module<'a> {
    fn new(prog: &'a Program, an: &'a Analysis) -> Module<'a> {
        let mut fns = HashMap::new();
        let mut consts = HashMap::new();
        let mut ns: HashMap<String, String> = HashMap::new();
        let mut impl_decls = HashMap::new();
        for (a, m) in [("List", "list"), ("String", "string"), ("Option", "option"), ("Result", "result"), ("Num", "num"), ("Bits", "bits"), ("Array", "array"), ("Map", "map"), ("Set", "set")] {
            ns.insert(a.to_string(), m.to_string());
        }
        for item in &prog.items {
            match item {
                Item::Decl(d) => match &d.kind {
                    DeclKind::Fn(f) => {
                        fns.insert(d.name.name.clone(), (d, f));
                    }
                    DeclKind::Const(e) => {
                        consts.insert(d.name.name.clone(), e);
                    }
                    _ => {}
                },
                Item::Impl(im) => {
                    for d in &im.methods {
                        if let DeclKind::Fn(f) = &d.kind {
                            impl_decls.insert(d.name.span.start, f);
                        }
                    }
                }
                Item::Import(i) => {
                    let p: Vec<&str> = i.path.iter().map(|x| x.name.as_str()).collect();
                    if p.len() == 2 && p[0] == "std" {
                        let alias = i.alias.as_ref().map_or(p[1].to_string(), |a| a.name.clone());
                        ns.insert(alias, p[1].to_string());
                    }
                }
            }
        }
        Module { an, fns, consts, impl_decls, ns }
    }
}

enum BodyRef<'a> {
    Block(&'a Block),
    Expr(&'a Expr),
}

struct FnSpec<'a> {
    id: u32,
    name: String,
    m: usize,
    subst: Subst,
    params: Vec<(String, Ty)>,
    caps: Vec<(String, Ty)>,
    has_env: bool,
    self_tail: bool,
    body: BodyRef<'a>,
}

pub struct Compiler<'a> {
    pub an: &'a Analysis,
    pub be: Backend,
    mods: Vec<Module<'a>>,
    ctors: HashMap<String, (String, usize)>,
    records: HashMap<String, String>, // short type name -> type id
    instances: HashMap<String, u32>,
    queue: Vec<Inst<'a>>,
    glue_cache: HashMap<String, u32>,
    glue_queue: Vec<GlueReq>,
    next_id: u32,
    strings: HashMap<String, u64>,
    nullary: HashMap<(String, usize), u64>,
    shims: HashMap<u32, u32>,
    pub newline: u64,
    /// Renders a runtime error at a source span (for locating runtime errors).
    pub renderer: Option<std::rc::Rc<dyn Fn(&cera_syntax::Diagnostic) -> String>>,
}

pub(crate) fn norm(t: &Ty) -> Ty {
    match t {
        Ty::Con(n, a) => Ty::Con(n.clone(), a.iter().map(norm).collect()),
        Ty::Fn(p, r) => Ty::Fn(p.iter().map(norm).collect(), Box::new(norm(r))),
        Ty::List(e) => Ty::List(Box::new(norm(e))),
        Ty::Tuple(v) => Ty::Tuple(v.iter().map(norm).collect()),
        Ty::Ref(e) => norm(e),
        Ty::Var(_) | Ty::Error => Ty::unit(),
        Ty::Param(p) => Ty::Param(p.clone()),
    }
}

pub(crate) fn subst_ty(t: &Ty, s: &Subst) -> Ty {
    match t {
        Ty::Param(p) => s.get(p).cloned().unwrap_or_else(|| t.clone()),
        Ty::Con(n, a) => Ty::Con(n.clone(), a.iter().map(|x| subst_ty(x, s)).collect()),
        Ty::Fn(p, r) => Ty::Fn(p.iter().map(|x| subst_ty(x, s)).collect(), Box::new(subst_ty(r, s))),
        Ty::List(e) => Ty::List(Box::new(subst_ty(e, s))),
        Ty::Tuple(v) => Ty::Tuple(v.iter().map(|x| subst_ty(x, s)).collect()),
        Ty::Ref(e) => Ty::Ref(Box::new(subst_ty(e, s))),
        other => other.clone(),
    }
}

/// Bind the `Param`s in `pat` by matching against `conc`.
pub(crate) fn match_ty(pat: &Ty, conc: &Ty, gens: &[String], out: &mut Subst) {
    match (pat, conc) {
        (Ty::Param(g), _) if gens.contains(g) => {
            out.entry(g.clone()).or_insert_with(|| conc.clone());
        }
        (Ty::Con(_, a), Ty::Con(_, b)) | (Ty::Tuple(a), Ty::Tuple(b)) => {
            for (x, y) in a.iter().zip(b) {
                match_ty(x, y, gens, out);
            }
        }
        (Ty::Fn(p, r), Ty::Fn(q, s)) => {
            for (x, y) in p.iter().zip(q) {
                match_ty(x, y, gens, out);
            }
            match_ty(r, s, gens, out);
        }
        (Ty::List(a), Ty::List(b)) | (Ty::Ref(a), Ty::Ref(b)) => match_ty(a, b, gens, out),
        _ => {}
    }
}

pub(crate) fn int_kind(t: &Ty) -> Option<(u32, bool)> {
    match t {
        Ty::Con(n, a) if a.is_empty() => Some(match n.as_str() {
            "i8" => (8, true),
            "i16" => (16, true),
            "i32" => (32, true),
            "i64" | "isize" => (64, true),
            "u8" => (8, false),
            "u16" => (16, false),
            "u32" => (32, false),
            "u64" | "usize" => (64, false),
            _ => return None,
        }),
        _ => None,
    }
}

pub(crate) fn is_con(t: &Ty, name: &str) -> bool {
    matches!(t, Ty::Con(n, a) if n == name && a.is_empty())
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

impl<'a> Compiler<'a> {
    pub fn new(prog: &'a Program, an: &'a Analysis, std_prog: &'a Program, std_an: &'a Analysis) -> Compiler<'a> {
        let mods = vec![Module::new(prog, an), Module::new(std_prog, std_an)];
        let mut ctors = HashMap::new();
        let mut records = HashMap::new();
        for (id, ti) in &an.type_defs {
            match &ti.def {
                TDef::Variants(vs) => {
                    for (i, v) in vs.iter().enumerate() {
                        ctors.insert(v.name.clone(), (id.clone(), i));
                    }
                }
                TDef::Record(_) => {
                    records.insert(cera_sema::ty::short_name(id).to_string(), id.clone());
                }
                TDef::Opaque => {}
            }
        }
        let mut be = Backend::new();
        let newline = be.add_data(b"\n", 8);
        Compiler {
            an,
            be,
            mods,
            ctors,
            records,
            instances: HashMap::new(),
            queue: vec![],
            glue_cache: HashMap::new(),
            glue_queue: vec![],
            next_id: FIRST_USER,
            strings: HashMap::new(),
            nullary: HashMap::new(),
            shims: HashMap::new(),
            newline,
            renderer: None,
        }
    }

    pub(crate) fn ctor(&self, name: &str) -> Option<(String, usize)> {
        self.ctors.get(name).cloned()
    }

    pub(crate) fn string_addr(&mut self, s: &str) -> u64 {
        if let Some(a) = self.strings.get(s) {
            return *a;
        }
        let mut bytes = (s.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(s.as_bytes());
        let a = self.be.add_data(&bytes, 8);
        self.strings.insert(s.to_string(), a);
        a
    }

    pub(crate) fn nullary_addr(&mut self, ty_id: &str, idx: usize) -> u64 {
        let key = (ty_id.to_string(), idx);
        if let Some(a) = self.nullary.get(&key) {
            return *a;
        }
        let a = self.be.add_data(&(idx as u64).to_le_bytes(), 8);
        self.nullary.insert(key, a);
        a
    }

    /// The instance id of `name` in module `m` at the given type arguments, queueing it for compilation.
    pub(crate) fn instance(&mut self, m: usize, name: &str, args: &[Ty]) -> R<u32> {
        let key = format!("{m}:{name}<{args:?}>");
        if let Some(id) = self.instances.get(&key) {
            return Ok(*id);
        }
        let Some((_, f)) = self.mods[m].fns.get(name).copied() else {
            return unsupported(&format!("the library function `{name}`"));
        };
        if f.body.is_none() {
            return unsupported(&format!("library function `{name}`"));
        }
        let scheme = self.mods[m].an.fn_schemes.get(name).ok_or_else(|| format!("no type for `{name}`"))?;
        let subst: Subst = scheme.generics.iter().cloned().zip(args.iter().cloned()).collect();
        let Ty::Fn(ptys, _) = &scheme.ty else { return Err("bad function type".into()) };
        let params = f.params.iter().zip(ptys).map(|(p, t)| (p.name.name.clone(), norm(&subst_ty(t, &subst)))).collect();
        let id = self.next_id;
        self.next_id += 1;
        self.instances.insert(key, id);
        self.queue.push(Inst { id, name: name.to_string(), m, decl: f, params, subst });
        Ok(id)
    }

    /// The instance of a user `impl` method for the given impl type arguments.
    pub(crate) fn impl_instance(&mut self, idx: usize, subst: &Subst) -> R<u32> {
        let imf = &self.an.impl_fns[idx];
        let key = format!("impl{}<{:?}>", imf.span.start, {
            let mut v: Vec<_> = subst.iter().collect();
            v.sort_by(|a, b| a.0.cmp(b.0));
            v
        });
        if let Some(id) = self.instances.get(&key) {
            return Ok(*id);
        }
        let f = *self.mods[0].impl_decls.get(&imf.span.start).ok_or("impl method not found")?;
        let Ty::Fn(ptys, _) = &imf.scheme.ty else { return Err("bad method type".into()) };
        let params = f.params.iter().zip(ptys).map(|(p, t)| (p.name.name.clone(), norm(&subst_ty(t, subst)))).collect();
        let id = self.next_id;
        self.next_id += 1;
        self.instances.insert(key, id);
        self.queue.push(Inst { id, name: imf.name.clone(), m: 0, decl: f, params, subst: subst.clone() });
        Ok(id)
    }

    /// Find the user impl of trait method `method` for a value of type `self_ty`.
    pub(crate) fn find_impl(&self, method: &str, self_ty: &Ty) -> Option<(usize, Subst)> {
        for (i, imf) in self.an.impl_fns.iter().enumerate() {
            if imf.name != method {
                continue;
            }
            let Ty::Fn(ps, _) = &imf.scheme.ty else { continue };
            let Some(p0) = ps.first() else { continue };
            let mut sub = Subst::new();
            match_ty(p0, self_ty, &imf.scheme.generics, &mut sub);
            if norm(&subst_ty(p0, &sub)) == *self_ty {
                return Some((i, sub));
            }
        }
        None
    }

    /// The id of the show/eq glue function for a type (compiled later).
    pub(crate) fn glue(&mut self, kind: GlueKind, ty: &Ty) -> u32 {
        let key = format!("{kind:?}{ty:?}");
        if let Some(id) = self.glue_cache.get(&key) {
            return *id;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.glue_cache.insert(key, id);
        self.glue_queue.push(GlueReq { id, kind, ty: ty.clone() });
        id
    }

    /// Assemble an `asm("...")` block into its own function; returns its id.
    fn asm_block(&mut self, text: &str) -> R<u32> {
        let bytes = cera_syntax::asm::assemble(text)?;
        let id = self.next_id;
        self.next_id += 1;
        self.be.define_bytes(id, bytes);
        Ok(id)
    }

    /// A function with the closure calling convention `(env, args...)` that calls instance `id`.
    fn shim(&mut self, id: u32, nparams: usize) -> R<u32> {
        if let Some(s) = self.shims.get(&id) {
            return Ok(*s);
        }
        let sid = self.next_id;
        self.next_id += 1;
        let sig = self.be.sig(nparams + 1, 1);
        let mut func = ir::Function::with_name_signature(UserFuncName::user(0, sid), sig);
        let mut fc = FunctionBuilderContext::new();
        {
            let mut b = FunctionBuilder::new(&mut func, &mut fc);
            let e = b.create_block();
            b.append_block_params_for_function_params(e);
            b.switch_to_block(e);
            let ps = b.block_params(e).to_vec();
            let r = rt::call(&mut b, &self.be, id, nparams, &ps[1..]);
            b.ins().return_(&[r[0]]);
            b.seal_all_blocks();
            b.finalize();
        }
        self.be.define(sid, func)?;
        self.shims.insert(id, sid);
        Ok(sid)
    }

    pub(crate) fn type_info(&self, id: &str) -> R<&'a TypeInfo> {
        self.an.type_defs.get(id).ok_or_else(|| format!("unknown type `{id}`"))
    }

    /// Compile everything reachable from `main`; returns the ELF image.
    pub fn compile(mut self) -> R<Vec<u8>> {
        let (_, mainf) = *self.mods[0].fns.get("main").ok_or("no `main` function")?;
        if mainf.params.len() > 1 {
            return Err("`main` takes at most one parameter".into());
        }
        let main_id = self.instance(0, "main", &[])?;
        let scheme = &self.mods[0].an.fn_schemes["main"];
        let returns_int = matches!(&scheme.ty, Ty::Fn(_, r) if int_kind(r).is_some());
        loop {
            if let Some(inst) = self.queue.pop() {
                self.compile_instance(inst)?;
            } else if let Some(g) = self.glue_queue.pop() {
                crate::glue::compile_glue(&mut self, g.id, g.kind, g.ty)?;
            } else {
                break;
            }
        }
        rt::define_all(&mut self.be, main_id, mainf.params.len(), returns_int, self.newline)?;
        self.be.link(rt::RT_START, 0)
    }

    fn compile_instance(&mut self, inst: Inst<'a>) -> R<()> {
        let spec = FnSpec {
            id: inst.id,
            name: inst.name,
            m: inst.m,
            subst: inst.subst,
            params: inst.params,
            caps: vec![],
            has_env: false,
            self_tail: true,
            body: BodyRef::Block(inst.decl.body.as_ref().unwrap()),
        };
        compile_function(self, spec)
    }
}

/// Compile a function whose body is produced by `f` (used for generated glue).
pub(crate) fn compile_custom<'a>(
    c: &mut Compiler<'a>,
    id: u32,
    nparams: usize,
    f: impl FnOnce(&mut FnCx<'_, 'a, '_>, &[ir::Value]) -> R<ir::Value>,
) -> R<()> {
    let sig = c.be.sig(nparams, 1);
    let mut func = ir::Function::with_name_signature(UserFuncName::user(0, id), sig);
    let mut fc = FunctionBuilderContext::new();
    {
        let b = FunctionBuilder::new(&mut func, &mut fc);
        let mut cx = FnCx {
            c,
            b,
            m: 0,
            scopes: vec![HashMap::new()],
            subst: Subst::new(),
            self_id: u32::MAX,
            loop_head: None,
            loops: Vec::new(),
            param_vars: vec![],
            panics: HashMap::new(),
            nvars: 0,
            defer_stack: vec![],
            cur_span: None,
        };
        let entry = cx.b.create_block();
        cx.b.append_block_params_for_function_params(entry);
        cx.b.switch_to_block(entry);
        let vals = cx.b.block_params(entry).to_vec();
        let v = f(&mut cx, &vals)?;
        cx.b.ins().return_(&[v]);
        cx.finish_panics();
        cx.b.seal_all_blocks();
        cx.b.finalize();
    }
    c.be.define(id, func)
}

fn compile_function<'a>(c: &mut Compiler<'a>, spec: FnSpec<'a>) -> R<()> {
    let nparams = spec.params.len() + spec.has_env as usize;
    let sig = c.be.sig(nparams, 1);
    let mut func = ir::Function::with_name_signature(UserFuncName::user(0, spec.id), sig);
    let mut fc = FunctionBuilderContext::new();
    {
        let b = FunctionBuilder::new(&mut func, &mut fc);
        let mut cx = FnCx {
            c,
            b,
            m: spec.m,
            scopes: vec![HashMap::new()],
            subst: spec.subst.clone(),
            self_id: if spec.self_tail { spec.id } else { u32::MAX },
            loop_head: None,
            loops: Vec::new(),
            param_vars: vec![],
            panics: HashMap::new(),
            nvars: 0,
            defer_stack: vec![],
            cur_span: None,
        };
        let entry = cx.b.create_block();
        cx.b.append_block_params_for_function_params(entry);
        cx.b.switch_to_block(entry);
        let vals = cx.b.block_params(entry).to_vec();
        {
            let sp = cx.b.ins().get_stack_pointer(I64);
            let lc = cx.b.ins().iconst(I64, (globals() + 56) as i64);
            let limit = cx.b.ins().load(I64, MemFlags::trusted(), lc, 0);
            let over = cx.b.ins().icmp(IntCC::UnsignedLessThan, sp, limit);
            cx.panic_if(over, "stack overflow: the program recursed too deeply");
        }
        let mut k = 0;
        if spec.has_env {
            let env = vals[0];
            k = 1;
            for (i, (name, t)) in spec.caps.iter().enumerate() {
                let v = cx.load_word(env, i + 1);
                cx.bind(name, v, t.clone());
            }
        }
        let mut pv = vec![];
        for (p, v) in spec.params.iter().zip(&vals[k..]) {
            let var = cx.new_var();
            cx.b.def_var(var, *v);
            cx.param_vars.push(var);
            pv.push((p.0.clone(), var, p.1.clone()));
        }
        let head = cx.b.create_block();
        cx.b.ins().jump(head, &[]);
        cx.b.switch_to_block(head);
        cx.loop_head = Some(head);
        for (name, var, t) in pv {
            cx.scopes.last_mut().unwrap().insert(name, (var, t));
        }
        let v = match spec.body {
            BodyRef::Block(b) => cx.block(b, true)?,
            BodyRef::Expr(e) => cx.expr(e, true)?,
        };
        cx.b.ins().return_(&[v]);
        cx.finish_panics();
        cx.b.seal_all_blocks();
        cx.b.finalize();
    }
    if c.be.debug.is_some() && spec.m == 0 {
        c.be.names.insert(spec.id, cera_syntax::display_name(&spec.name).to_string());
    }
    c.be.define(spec.id, func)
}

pub(crate) type Scope = HashMap<String, (Variable, Ty)>;

pub(crate) struct DeferEntry {
    expr: &'static Expr,
    scopes: Vec<Scope>,
}

pub(crate) struct FnCx<'c, 'a, 'b> {
    pub(crate) c: &'c mut Compiler<'a>,
    pub(crate) b: FunctionBuilder<'b>,
    pub(crate) m: usize,
    pub(crate) scopes: Vec<Scope>,
    pub(crate) subst: Subst,
    pub(crate) self_id: u32,
    pub(crate) loop_head: Option<ir::Block>,
    /// Enclosing source loops, innermost last: `(continue target, break target)`.
    pub(crate) loops: Vec<(ir::Block, ir::Block)>,
    pub(crate) param_vars: Vec<Variable>,
    pub(crate) panics: HashMap<String, ir::Block>,
    pub(crate) nvars: u32,
    pub(crate) defer_stack: Vec<Vec<DeferEntry>>,
    pub(crate) cur_span: Option<cera_syntax::Span>,
}

impl<'c, 'a, 'b> FnCx<'c, 'a, 'b> {
    pub(crate) fn norm_ty(&self, t: &Ty) -> Ty {
        norm(&subst_ty(t, &self.subst))
    }

    pub(crate) fn ty_of(&self, e: &Expr) -> R<Ty> {
        match self.c.mods[self.m].an.typed.get(&(e.span.start, e.span.end)) {
            Some(t) => Ok(self.norm_ty(t)),
            None => Err(format!("no type recorded for expression at {}..{}", e.span.start, e.span.end)),
        }
    }

    pub(crate) fn new_var(&mut self) -> Variable {
        let v = Variable::from_u32(self.nvars);
        self.nvars += 1;
        self.b.declare_var(v, I64);
        v
    }

    pub(crate) fn iconst(&mut self, v: i64) -> ir::Value {
        self.b.ins().iconst(I64, v)
    }

    pub(crate) fn lookup(&self, name: &str) -> Option<(Variable, Ty)> {
        self.scopes.iter().rev().find_map(|s| s.get(name).cloned())
    }

    pub(crate) fn bind(&mut self, name: &str, v: ir::Value, t: Ty) {
        let var = self.new_var();
        self.b.def_var(var, v);
        self.scopes.last_mut().unwrap().insert(name.to_string(), (var, t));
    }

    /// The complete text printed for a runtime error raised at the current expression.
    pub(crate) fn located(&self, msg: &str) -> String {
        if let (0, Some(span), Some(render)) = (self.m, self.cur_span, self.c.renderer.as_ref()) {
            if !span.is_empty() {
                let mut d = cera_syntax::Diagnostic::new(span, format!("runtime error: {msg}"));
                if let Some(h) = cera_syntax::runtime_help(msg) {
                    d = d.with_help(h);
                }
                return render(&d) + "\n";
            }
        }
        format!("runtime error: {msg}\n")
    }

    pub(crate) fn panic_block(&mut self, msg: &str) -> ir::Block {
        let full = self.located(msg);
        let msg = full.as_str();
        if let Some(b) = self.panics.get(msg) {
            return *b;
        }
        let b = self.b.create_block();
        self.b.set_cold_block(b);
        self.panics.insert(msg.to_string(), b);
        b
    }

    pub(crate) fn finish_panics(&mut self) {
        let mut ps: Vec<(String, ir::Block)> = self.panics.drain().collect();
        ps.sort_by_key(|(_, b)| b.as_u32());
        for (msg, blk) in ps {
            self.b.switch_to_block(blk);
            let a = self.c.string_addr(&msg);
            let m = self.b.ins().iconst(I64, a as i64);
            rt::call(&mut self.b, &self.c.be, rt::RT_PANIC, 1, &[m]);
            self.b.ins().trap(ir::TrapCode::unwrap_user(3));
        }
    }

    /// Report a runtime error now; code after it is unreachable.
    pub(crate) fn emit_panic(&mut self, msg: &str) -> ir::Value {
        let full = self.located(msg);
        let a = self.c.string_addr(&full);
        let m = self.iconst(a as i64);
        rt::call(&mut self.b, &self.c.be, rt::RT_PANIC, 1, &[m]);
        self.b.ins().trap(ir::TrapCode::unwrap_user(5));
        let dead = self.b.create_block();
        self.b.switch_to_block(dead);
        self.iconst(0)
    }

    /// Branch to a panic block when `cond` is true.
    pub(crate) fn panic_if(&mut self, cond: ir::Value, msg: &str) {
        let p = self.panic_block(msg);
        let ok = self.b.create_block();
        self.b.ins().brif(cond, p, &[], ok, &[]);
        self.b.switch_to_block(ok);
    }

    pub(crate) fn alloc(&mut self, words: usize) -> ir::Value {
        let size = self.iconst((words.max(1) * 8) as i64);
        self.alloc_bytes(size)
    }

    /// Allocate `size` bytes (a multiple of 8), zeroed.
    pub(crate) fn alloc_bytes(&mut self, size: ir::Value) -> ir::Value {
        let hp = self.b.ins().iconst(I64, heap_ptr() as i64);
        let he = self.b.ins().iconst(I64, heap_end() as i64);
        let p = self.b.ins().load(I64, MemFlags::trusted(), hp, 0);
        let np = self.b.ins().iadd(p, size);
        let end = self.b.ins().load(I64, MemFlags::trusted(), he, 0);
        let over = self.b.ins().icmp(IntCC::UnsignedGreaterThan, np, end);
        let slow = self.b.create_block();
        let fast = self.b.create_block();
        let merge = self.b.create_block();
        self.b.append_block_param(merge, I64);
        self.b.set_cold_block(slow);
        self.b.ins().brif(over, slow, &[], fast, &[]);
        self.b.switch_to_block(fast);
        self.b.ins().store(MemFlags::trusted(), np, hp, 0);
        self.b.ins().jump(merge, &[p.into()]);
        self.b.switch_to_block(slow);
        let q = rt::call(&mut self.b, &self.c.be, rt::RT_GROW, 1, &[size])[0];
        self.b.ins().jump(merge, &[q.into()]);
        self.b.switch_to_block(merge);
        self.b.block_params(merge)[0]
    }

    pub(crate) fn store_words(&mut self, base: ir::Value, offset_words: usize, vals: &[ir::Value]) {
        for (i, v) in vals.iter().enumerate() {
            self.b.ins().store(MemFlags::trusted(), *v, base, ((offset_words + i) * 8) as i32);
        }
    }

    pub(crate) fn load_word(&mut self, base: ir::Value, idx: usize) -> ir::Value {
        self.b.ins().load(I64, MemFlags::trusted(), base, (idx * 8) as i32)
    }

    // ----- blocks and statements ---------------------------------------------------

    /// `region do ... end`: when nothing inside can keep a pointer to region memory alive after
    /// the `end`, rewind the heap there, which frees everything the body allocated.
    fn region(&mut self, e: &Expr, b: &Block) -> R<ir::Value> {
        let rt = self.ty_of(e)?;
        let is_str = matches!(&rt, Ty::Con(n, a) if n == "String" && a.is_empty());
        if !(is_flat(&rt) || is_str) || self.region_escapes(b) {
            return self.block(b, false);
        }
        let usize_t = Ty::prim("usize");
        let (pid, _) = self.instance_for(1, "stdin_pin", &Ty::Fn(vec![], Box::new(Ty::unit())))?;
        rt::call(&mut self.b, &self.c.be, pid, 0, &[]);
        let (mid, _) = self.instance_for(1, "mem_mark", &Ty::Fn(vec![], Box::new(usize_t.clone())))?;
        let mark = rt::call(&mut self.b, &self.c.be, mid, 0, &[])[0];
        let v = self.block(b, false)?;
        if is_str {
            let (rid, _) = self.instance_for(1, "mem_reset_str", &Ty::Fn(vec![usize_t, rt.clone()], Box::new(rt.clone())))?;
            return Ok(rt::call(&mut self.b, &self.c.be, rid, 2, &[mark, v])[0]);
        }
        let (rid, _) = self.instance_for(1, "mem_reset", &Ty::Fn(vec![usize_t], Box::new(Ty::unit())))?;
        rt::call(&mut self.b, &self.c.be, rid, 1, &[mark]);
        Ok(v)
    }

    /// Could the body of a region leave a pointer to its own memory somewhere that outlives it?
    /// That needs a write to something declared outside (a `var`, an array), a closure from
    /// outside, or raw memory handling. Anything else cannot.
    fn region_escapes(&self, b: &Block) -> bool {
        let mut declared: Vec<String> = vec![];
        let mut bad = false;
        walk_block(b, &mut |n: Node| match n {
            Node::S(Stmt::Let { name, .. }) => declared.push(name.name.clone()),
            Node::S(Stmt::Assign { name, .. }) => {
                if !declared.contains(&name.name) {
                    bad = true;
                }
            }
            Node::E(x) => match &x.kind {
                ExprKind::Unsafe(_) => bad = true,
                ExprKind::Var(name) if !declared.contains(name) => {
                    if let Some((_, t)) = self.lookup(name) {
                        if has_mutable_or_fn(&t) {
                            bad = true;
                        }
                    }
                }
                ExprKind::Call { callee, .. } => {
                    if let Some((1, f)) = self.resolve_fn(callee) {
                        const MUT: [&str; 9] = ["push", "set", "pop", "insert", "remove", "swap", "clear", "reserve", "fill_from"];
                        if f.starts_with("mem_") || f.starts_with("sys_") || MUT.iter().any(|m| f == format!("array_{m}")) {
                            bad = true;
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        });
        bad
    }

    pub(crate) fn block(&mut self, blk: &Block, tail: bool) -> R<ir::Value> {
        self.scopes.push(HashMap::new());
        self.defer_stack.push(vec![]);
        let has_defer = blk.stmts.iter().any(|s| matches!(s, Stmt::Defer(..)));
        let mut last = self.iconst(0);
        let n = blk.stmts.len();
        for (i, s) in blk.stmts.iter().enumerate() {
            let is_last = i + 1 == n;
            match s {
                Stmt::Let { name, value, .. } => {
                    let t = self.ty_of(value)?;
                    let v = self.expr(value, false)?;
                    self.bind(&name.name, v, t);
                    last = self.iconst(0);
                }
                Stmt::Assign { name, value, .. } => {
                    let v = self.expr(value, false)?;
                    let (var, _) = self.lookup(&name.name).ok_or_else(|| format!("assignment to unknown variable `{}`", name.name))?;
                    self.b.def_var(var, v);
                    last = self.iconst(0);
                }
                Stmt::Defer(e, _) => {
                    let snap = self.scopes.clone();
                    self.defer_stack.last_mut().unwrap().push(DeferEntry { expr: unsafe_extend(e), scopes: snap });
                    last = self.iconst(0);
                }
                Stmt::Expr(e) => {
                    last = self.expr(e, tail && is_last && !has_defer)?;
                }
            }
        }
        let defers = self.defer_stack.pop().unwrap();
        self.run_defers(&defers)?;
        self.scopes.pop();
        Ok(last)
    }

    fn run_defers(&mut self, defers: &[DeferEntry]) -> R<()> {
        for d in defers.iter().rev() {
            let saved = std::mem::replace(&mut self.scopes, d.scopes.clone());
            let r = self.expr(d.expr, false);
            self.scopes = saved;
            r?;
        }
        Ok(())
    }

    /// Run every pending defer of the function (innermost first), for an early return.
    fn run_all_defers(&mut self) -> R<()> {
        let stack = std::mem::take(&mut self.defer_stack);
        let mut result = Ok(());
        for level in stack.iter().rev() {
            if result.is_ok() {
                result = self.run_defers(level);
            }
        }
        self.defer_stack = stack;
        result
    }

    // ----- expressions -----------------------------------------------------------

    pub(crate) fn expr(&mut self, e: &Expr, tail: bool) -> R<ir::Value> {
        let prev = self.cur_span.replace(e.span);
        let debug = self.m == 0 && self.c.be.debug.is_some();
        if debug {
            self.b.set_srcloc(ir::SourceLoc::new(e.span.start as u32));
        }
        let r = self.expr_inner(e, tail);
        self.cur_span = prev;
        if debug {
            // Back to the enclosing expression's position for what follows.
            self.b.set_srcloc(prev.map_or(ir::SourceLoc::default(), |p| ir::SourceLoc::new(p.start as u32)));
        }
        r
    }

    fn expr_inner(&mut self, e: &Expr, tail: bool) -> R<ir::Value> {
        match &e.kind {
            ExprKind::Int(s) => {
                let ty = self.ty_of(e)?;
                if is_con(&ty, "f64") || is_con(&ty, "f32") {
                    let v = parse_int(s).ok_or("bad integer literal")? as f64;
                    return Ok(self.fconst(v));
                }
                let v = parse_int(s).ok_or("bad integer literal")?;
                if let Some((bits, signed)) = int_kind(&ty) {
                    let (lo, hi): (i128, i128) = if signed { (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1) } else { (0, (1i128 << bits) - 1) };
                    if v < lo || v > hi {
                        let tn = match &ty {
                            Ty::Con(n, _) => n.clone(),
                            _ => String::new(),
                        };
                        return Ok(self.emit_panic(&format!("integer literal `{s}` does not fit in `{tn}`")));
                    }
                }
                Ok(self.iconst(v as i64))
            }
            ExprKind::Float(s) => {
                let v: f64 = s.chars().filter(|c| *c != '_').collect::<String>().parse().map_err(|_| "bad float literal")?;
                Ok(self.fconst(v))
            }
            ExprKind::Str(s) => {
                let a = self.c.string_addr(s);
                Ok(self.iconst(a as i64))
            }
            ExprKind::Bool(v) => Ok(self.iconst(*v as i64)),
            ExprKind::Unit => Ok(self.iconst(0)),
            ExprKind::Var(name) => self.var(name, e),
            ExprKind::Tuple(items) => {
                let mut vs = vec![];
                for it in items {
                    vs.push(self.expr(it, false)?);
                }
                let p = self.alloc(vs.len());
                self.store_words(p, 0, &vs);
                Ok(p)
            }
            ExprKind::Update { base, fields } => {
                let t = self.ty_of(base)?;
                let Ty::Con(id, _) = &t else { return unsupported("update of a non-record") };
                let ti = self.c.type_info(id)?;
                let TDef::Record(decl_fields) = &ti.def else { return unsupported("update of a non-record") };
                let n = decl_fields.len();
                let names: Vec<String> = decl_fields.iter().map(|(f, _)| f.clone()).collect();
                let old = self.expr(base, false)?;
                let mut slots: Vec<Option<ir::Value>> = vec![None; n];
                for (fname, fe) in fields {
                    let idx = names.iter().position(|f| *f == fname.name).ok_or("unknown field")?;
                    slots[idx] = Some(self.expr(fe, false)?);
                }
                let p = self.alloc(n);
                for (i, slot) in slots.into_iter().enumerate() {
                    let v = match slot {
                        Some(v) => v,
                        None => self.load_word(old, i),
                    };
                    self.store_words(p, i, &[v]);
                }
                Ok(p)
            }
            ExprKind::Record { name, fields } => {
                let id = self.c.records.get(&name.name).cloned().ok_or_else(|| format!("unknown record `{}`", name.name))?;
                let ti = self.c.type_info(&id)?;
                let TDef::Record(decl_fields) = &ti.def else { unreachable!() };
                let mut vals = vec![0i64; 0];
                let _ = &mut vals;
                let mut slots: Vec<Option<ir::Value>> = vec![None; decl_fields.len()];
                for (fname, fe) in fields {
                    let idx = decl_fields.iter().position(|(n, _)| *n == fname.name).ok_or("unknown field")?;
                    slots[idx] = Some(self.expr(fe, false)?);
                }
                let vs: Vec<ir::Value> = slots.into_iter().collect::<Option<_>>().ok_or("missing record field")?;
                let p = self.alloc(vs.len());
                self.store_words(p, 0, &vs);
                Ok(p)
            }
            ExprKind::Field { expr, name } => {
                let t = self.ty_of(expr)?;
                let base = self.expr(expr, false)?;
                match &t {
                    Ty::Tuple(_) => {
                        let idx: usize = name.name.parse().map_err(|_| "bad tuple field")?;
                        Ok(self.load_word(base, idx))
                    }
                    Ty::Con(id, _) => {
                        let ti = self.c.type_info(id)?;
                        let TDef::Record(fs) = &ti.def else { return Err("field access on non-record".into()) };
                        let idx = fs.iter().position(|(n, _)| *n == name.name).ok_or("unknown field")?;
                        Ok(self.load_word(base, idx))
                    }
                    _ => unsupported("this field access"),
                }
            }
            ExprKind::Unary { op: UnOp::Neg, expr } if matches!(expr.kind, ExprKind::Int(_)) && int_kind(&self.ty_of(expr)?).is_some() => {
                // `-128` is a literal of its own, so it may be the smallest value of its type.
                let ExprKind::Int(s) = &expr.kind else { unreachable!() };
                let ty = self.ty_of(expr)?;
                let v = -parse_int(s).ok_or("bad integer literal")?;
                let (bits, signed) = int_kind(&ty).unwrap();
                let (lo, hi): (i128, i128) = if signed { (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1) } else { (0, (1i128 << bits) - 1) };
                if v < lo || v > hi {
                    let tn = match &ty {
                        Ty::Con(n, _) => n.clone(),
                        _ => String::new(),
                    };
                    return Ok(self.emit_panic(&format!("integer literal `-{s}` does not fit in `{tn}`")));
                }
                Ok(self.iconst(v as i64))
            }
            ExprKind::Unary { op: UnOp::BitNot, expr } => {
                let r = self.bits_builtin("bnot", &[expr])?;
                r.ok_or_else(|| "`~` needs an integer".to_string())
            }
            ExprKind::Unary { op, expr } => {
                let t = self.ty_of(expr)?;
                let v = self.expr(expr, false)?;
                match op {
                    UnOp::BitNot => unreachable!(),
                    UnOp::Not => Ok(self.b.ins().bxor_imm(v, 1)),
                    UnOp::Neg => {
                        if is_con(&t, "f64") || is_con(&t, "f32") {
                            let f = self.b.ins().bitcast(F64, MemFlags::new(), v);
                            let r = self.b.ins().fneg(f);
                            Ok(self.b.ins().bitcast(I64, MemFlags::new(), r))
                        } else {
                            let z = self.iconst(0);
                            self.int_arith(BinOp::Sub, z, v, &t)
                        }
                    }
                }
            }
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs),
            ExprKind::If { cond, then, els } => {
                let c = self.expr(cond, false)?;
                let tb = self.b.create_block();
                let eb = self.b.create_block();
                let mb = self.b.create_block();
                self.b.append_block_param(mb, I64);
                self.b.ins().brif(c, tb, &[], eb, &[]);
                self.b.switch_to_block(tb);
                let tv = self.block(then, tail)?;
                self.b.ins().jump(mb, &[tv.into()]);
                self.b.switch_to_block(eb);
                let ev = match els {
                    Some(b) => self.block(b, tail)?,
                    None => self.iconst(0),
                };
                self.b.ins().jump(mb, &[ev.into()]);
                self.b.switch_to_block(mb);
                Ok(self.b.block_params(mb)[0])
            }
            ExprKind::Region(b) => self.region(e, b),
            ExprKind::Block(b) | ExprKind::Unsafe(b) => self.block(b, tail),
            ExprKind::While { cond, body } => {
                let head = self.b.create_block();
                let top = self.b.create_block();
                let exit = self.b.create_block();
                self.b.ins().jump(head, &[]);
                self.b.switch_to_block(head);
                let c = self.expr(cond, false)?;
                self.b.ins().brif(c, top, &[], exit, &[]);
                self.b.switch_to_block(top);
                self.loops.push((head, exit));
                let r = self.block(body, false);
                self.loops.pop();
                r?;
                self.b.ins().jump(head, &[]);
                self.b.switch_to_block(exit);
                Ok(self.iconst(0))
            }
            ExprKind::For { var, iter, body } => {
                let Ty::List(et) = self.ty_of(iter)? else { return Err("`for` over a non-list".into()) };
                let items = self.expr(iter, false)?;
                let cur = self.new_var();
                self.b.def_var(cur, items);
                let head = self.b.create_block();
                let top = self.b.create_block();
                let exit = self.b.create_block();
                self.b.ins().jump(head, &[]);
                self.b.switch_to_block(head);
                let c = self.b.use_var(cur);
                let done = self.b.ins().icmp_imm(IntCC::Equal, c, 0);
                self.b.ins().brif(done, exit, &[], top, &[]);
                self.b.switch_to_block(top);
                let c = self.b.use_var(cur);
                let h = self.load_word(c, 0);
                let next = self.load_word(c, 1);
                self.b.def_var(cur, next);
                self.scopes.push(HashMap::new());
                self.bind(&var.name, h, *et);
                self.loops.push((head, exit));
                let r = self.block(body, false);
                self.loops.pop();
                self.scopes.pop();
                r?;
                self.b.ins().jump(head, &[]);
                self.b.switch_to_block(exit);
                Ok(self.iconst(0))
            }
            ExprKind::Break | ExprKind::Continue => {
                let Some(&(cont, brk)) = self.loops.last() else { return Err("`break` or `continue` outside a loop".into()) };
                let target = if matches!(e.kind, ExprKind::Break) { brk } else { cont };
                self.b.ins().jump(target, &[]);
                let dead = self.b.create_block();
                self.b.switch_to_block(dead);
                Ok(self.iconst(0))
            }
            ExprKind::Match { scrutinee, arms } => self.match_expr(scrutinee, arms, tail),
            ExprKind::Call { callee, args } => {
                let exprs: Vec<&Expr> = args.iter().collect();
                self.call(callee, &exprs, e, tail)
            }
            ExprKind::Pipe { lhs, rhs } => {
                let (callee, rest): (&Expr, Vec<&Expr>) = match &rhs.kind {
                    ExprKind::Call { callee, args } => (callee, args.iter().collect()),
                    _ => (rhs, vec![]),
                };
                let mut all: Vec<&Expr> = vec![lhs];
                all.extend(rest);
                self.call(callee, &all, e, tail)
            }
            ExprKind::List(items) => {
                let mut vs = vec![];
                for it in items {
                    vs.push(self.expr(it, false)?);
                }
                let nil = self.iconst(0);
                Ok(self.list_from(vs, nil))
            }
            ExprKind::Cons { items, tail } => {
                let mut vs = vec![];
                for it in items {
                    vs.push(self.expr(it, false)?);
                }
                let t = self.expr(tail, false)?;
                Ok(self.list_from(vs, t))
            }
            ExprKind::Lambda { params, body } => self.lambda(e, params, body),
            ExprKind::Index { expr, index } => {
                let ct = self.ty_of(expr)?;
                let name = if matches!(&ct, Ty::Con(n, _) if n.ends_with("Array")) { "array_index" } else { "list_index" };
                let elem = self.ty_of(e)?;
                let conc = Ty::Fn(vec![ct, Ty::prim("usize")], Box::new(elem));
                let (id, _) = self.instance_for(1, name, &conc)?;
                let a = self.expr(expr, false)?;
                let i = self.expr(index, false)?;
                let r = rt::call(&mut self.b, &self.c.be, id, 2, &[a, i]);
                Ok(r[0])
            }
            ExprKind::Try(inner) => {
                let v = self.expr(inner, false)?;
                let tag = self.load_word(v, 0);
                let ok = self.b.ins().icmp_imm(IntCC::Equal, tag, 0);
                let good = self.b.create_block();
                let bad = self.b.create_block();
                self.b.ins().brif(ok, good, &[], bad, &[]);
                self.b.switch_to_block(bad);
                self.run_all_defers()?;
                self.b.ins().return_(&[v]);
                self.b.switch_to_block(good);
                Ok(self.load_word(v, 1))
            }
            ExprKind::With { arg, body, .. } => {
                self.expr(arg, false)?;
                self.block(body, tail)
            }
        }
    }

    pub(crate) fn fconst(&mut self, v: f64) -> ir::Value {
        self.iconst(v.to_bits() as i64)
    }

    fn var(&mut self, name: &str, e: &Expr) -> R<ir::Value> {
        if let Some((v, _)) = self.lookup(name) {
            return Ok(self.b.use_var(v));
        }
        if let Some((id, idx)) = self.c.ctors.get(name).cloned() {
            let ti = self.c.type_info(&id)?;
            let TDef::Variants(vs) = &ti.def else { unreachable!() };
            if vs[idx].fields.is_empty() {
                let a = self.c.nullary_addr(&id, idx);
                return Ok(self.iconst(a as i64));
            }
            return unsupported("constructors as function values");
        }
        if let Some(ce) = self.c.mods[self.m].consts.get(name).copied() {
            return self.expr(ce, false);
        }
        if self.is_trait_method(name) {
            let self_ty = self.trait_self_ty(e)?;
            if let Some((idx, sub)) = self.c.find_impl(name, &self_ty) {
                let id = self.c.impl_instance(idx, &sub)?;
                let n = match self.ty_of(e)? {
                    Ty::Fn(ps, _) => ps.len(),
                    _ => 1,
                };
                return Ok(self.closure_of(id, n));
            }
            return unsupported(&format!("the built-in trait method `{name}` as a value"));
        }
        if let Some((m, fname)) = self.resolve_fn(e) {
            // A function used as a value: wrap it in a closure object.
            let ty = self.ty_of(e)?;
            let (id, n) = self.instance_for(m, &fname, &ty)?;
            let shim = self.c.shim(id, n)?;
            return Ok(self.make_closure(shim, &[]));
        }
        unsupported(&format!("the name `{name}` as a value"))
    }

    /// Which function a name or `Module.name` expression refers to, as (module index, function name).
    fn resolve_fn(&self, callee: &Expr) -> Option<(usize, String)> {
        match &callee.kind {
            ExprKind::Var(name) => {
                if self.lookup(name).is_some() {
                    return None;
                }
                if self.c.mods[self.m].fns.contains_key(name.as_str()) {
                    return Some((self.m, name.clone()));
                }
                if self.m == 0 && cera_sema::PRELUDE_LIST.contains(&name.as_str()) {
                    return Some((1, format!("list_{name}")));
                }
                None
            }
            ExprKind::Field { expr, name } => {
                let ExprKind::Var(ns) = &expr.kind else { return None };
                if self.lookup(ns).is_some() {
                    return None;
                }
                let module = self.c.mods[self.m].ns.get(ns.as_str())?;
                let f = format!("{module}_{}", name.name);
                if self.c.mods[1].fns.contains_key(&f) {
                    Some((1, f))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Resolve a generic instantiation from the use site's type; returns (instance id, arity).
    fn instance_for(&mut self, m: usize, name: &str, conc: &Ty) -> R<(u32, usize)> {
        let scheme = self.c.mods[m].an.fn_schemes.get(name).ok_or_else(|| format!("no type for `{name}`"))?.clone();
        let mut sub = Subst::new();
        match_ty(&scheme.ty, conc, &scheme.generics, &mut sub);
        let targs: Vec<Ty> = scheme.generics.iter().map(|g| sub.get(g).cloned().unwrap_or_else(Ty::unit)).collect();
        let id = self.c.instance(m, name, &targs)?;
        let n = match &scheme.ty {
            Ty::Fn(p, _) => p.len(),
            _ => 0,
        };
        Ok((id, n))
    }

    /// A closure object calling function instance `id` with `n` parameters.
    pub(crate) fn closure_of(&mut self, id: u32, n: usize) -> ir::Value {
        let shim = self.c.shim(id, n).expect("shim");
        self.make_closure(shim, &[])
    }

    fn make_closure(&mut self, func: u32, caps: &[ir::Value]) -> ir::Value {
        let obj = self.alloc(1 + caps.len());
        let sig = self.c.be.sig(0, 1);
        let _ = sig;
        let nm = self.b.func.declare_imported_user_function(Backend::user_name(func));
        let sr = self.b.import_signature(self.c.be.sig(caps.len() + 1, 1));
        let fr = self.b.import_function(ir::ExtFuncData { name: ir::ExternalName::user(nm), signature: sr, colocated: true });
        let addr = self.b.ins().func_addr(I64, fr);
        self.store_words(obj, 0, &[addr]);
        self.store_words(obj, 1, caps);
        obj
    }

    fn lambda(&mut self, e: &Expr, params: &[Param], body: &Expr) -> R<ir::Value> {
        let Ty::Fn(ptys, _) = self.ty_of(e)? else { return Err("lambda without a function type".into()) };
        let mut names = vec![];
        collect_vars(body, &mut names);
        let mut caps: Vec<(String, Ty)> = vec![];
        let mut vals = vec![];
        for n in names {
            if params.iter().any(|p| p.name.name == n) || caps.iter().any(|(c, _)| *c == n) {
                continue;
            }
            if let Some((var, t)) = self.lookup(&n) {
                vals.push(self.b.use_var(var));
                caps.push((n, t));
            }
        }
        let id = self.c.next_id;
        self.c.next_id += 1;
        let spec = FnSpec {
            id,
            name: "<closure>".to_string(),
            m: self.m,
            subst: self.subst.clone(),
            params: params.iter().zip(ptys).map(|(p, t)| (p.name.name.clone(), t)).collect(),
            caps,
            has_env: true,
            self_tail: false,
            body: BodyRef::Expr(unsafe_extend(body)),
        };
        compile_function(self.c, spec)?;
        Ok(self.make_closure(id, &vals))
    }

    fn list_from(&mut self, items: Vec<ir::Value>, tail: ir::Value) -> ir::Value {
        let mut acc = tail;
        for v in items.into_iter().rev() {
            let cell = self.alloc(2);
            self.store_words(cell, 0, &[v, acc]);
            acc = cell;
        }
        acc
    }

    fn binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr) -> R<ir::Value> {
        let bit = match op {
            BinOp::BitAnd => Some("band"),
            BinOp::BitXor => Some("bxor"),
            BinOp::Shl => Some("shl"),
            BinOp::Shr => Some("shr"),
            _ => None,
        };
        if let Some(name) = bit {
            let r = self.bits_builtin(name, &[lhs, rhs])?;
            return r.ok_or_else(|| "bitwise operator needs integers".to_string());
        }
        if matches!(op, BinOp::And | BinOp::Or) {
            let l = self.expr(lhs, false)?;
            let rb = self.b.create_block();
            let mb = self.b.create_block();
            self.b.append_block_param(mb, I64);
            if op == BinOp::And {
                self.b.ins().brif(l, rb, &[], mb, &[l.into()]);
            } else {
                self.b.ins().brif(l, mb, &[l.into()], rb, &[]);
            }
            self.b.switch_to_block(rb);
            let r = self.expr(rhs, false)?;
            self.b.ins().jump(mb, &[r.into()]);
            self.b.switch_to_block(mb);
            return Ok(self.b.block_params(mb)[0]);
        }
        let t = self.ty_of(lhs)?;
        let l = self.expr(lhs, false)?;
        let r = self.expr(rhs, false)?;
        self.binary_values(op, &t, l, r)
    }

    pub(crate) fn binary_values(&mut self, op: BinOp, t: &Ty, l: ir::Value, r: ir::Value) -> R<ir::Value> {
        let t = t.clone();
        if let Some(v) = self.binary_special(op, &t, l, r)? {
            return Ok(v);
        }
        let is_float = is_con(&t, "f64") || is_con(&t, "f32");
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                if is_float {
                    let a = self.b.ins().bitcast(F64, MemFlags::new(), l);
                    let c = self.b.ins().bitcast(F64, MemFlags::new(), r);
                    let res = match op {
                        BinOp::Add => self.b.ins().fadd(a, c),
                        BinOp::Sub => self.b.ins().fsub(a, c),
                        BinOp::Mul => self.b.ins().fmul(a, c),
                        BinOp::Div => self.b.ins().fdiv(a, c),
                        _ => return unsupported("float remainder"),
                    };
                    let res = if is_con(&t, "f32") { self.round_f32(res) } else { res };
                    Ok(self.b.ins().bitcast(I64, MemFlags::new(), res))
                } else if int_kind(&t).is_some() {
                    self.int_arith(op, l, r, &t)
                } else {
                    unsupported("operators on this type")
                }
            }
            _ => {
                let v = if is_float {
                    let a = self.b.ins().bitcast(F64, MemFlags::new(), l);
                    let c = self.b.ins().bitcast(F64, MemFlags::new(), r);
                    let cc = match op {
                        BinOp::Eq => FloatCC::Equal,
                        BinOp::NotEq => FloatCC::NotEqual,
                        BinOp::Lt => FloatCC::LessThan,
                        BinOp::LtEq => FloatCC::LessThanOrEqual,
                        BinOp::Gt => FloatCC::GreaterThan,
                        _ => FloatCC::GreaterThanOrEqual,
                    };
                    self.b.ins().fcmp(cc, a, c)
                } else if int_kind(&t).is_some() || is_con(&t, "bool") || is_con(&t, "char") || matches!(&t, Ty::Tuple(v) if v.is_empty()) {
                    let signed = int_kind(&t).is_none_or(|(_, s)| s);
                    let cc = match (op, signed) {
                        (BinOp::Eq, _) => IntCC::Equal,
                        (BinOp::NotEq, _) => IntCC::NotEqual,
                        (BinOp::Lt, true) => IntCC::SignedLessThan,
                        (BinOp::LtEq, true) => IntCC::SignedLessThanOrEqual,
                        (BinOp::Gt, true) => IntCC::SignedGreaterThan,
                        (BinOp::GtEq, true) => IntCC::SignedGreaterThanOrEqual,
                        (BinOp::Lt, false) => IntCC::UnsignedLessThan,
                        (BinOp::LtEq, false) => IntCC::UnsignedLessThanOrEqual,
                        (BinOp::Gt, false) => IntCC::UnsignedGreaterThan,
                        _ => IntCC::UnsignedGreaterThanOrEqual,
                    };
                    self.b.ins().icmp(cc, l, r)
                } else {
                    return unsupported("comparison on this type");
                };
                Ok(self.b.ins().uextend(I64, v))
            }
        }
    }

    /// Operators on strings and other non-primitive types.
    fn binary_special(&mut self, op: BinOp, t: &Ty, l: ir::Value, r: ir::Value) -> R<Option<ir::Value>> {
        let prim = int_kind(t).is_some() || is_con(t, "f64") || is_con(t, "f32") || is_con(t, "bool") || is_con(t, "char");
        if prim {
            return Ok(None);
        }
        if matches!(op, BinOp::Eq | BinOp::NotEq) {
            let e = self.eq_values(t, l, r)?;
            return Ok(Some(if op == BinOp::Eq { e } else { self.b.ins().bxor_imm(e, 1) }));
        }
        if is_con(t, "String") {
            let lt = |cx: &mut Self, a, b| cx.call_std("string_less", &[], &[a, b]);
            return Ok(Some(match op {
                BinOp::Lt => lt(self, l, r)?,
                BinOp::Gt => lt(self, r, l)?,
                BinOp::LtEq => {
                    let x = lt(self, r, l)?;
                    self.b.ins().bxor_imm(x, 1)
                }
                BinOp::GtEq => {
                    let x = lt(self, l, r)?;
                    self.b.ins().bxor_imm(x, 1)
                }
                _ => return unsupported("this operator on strings"),
            }));
        }
        if matches!(t, Ty::Tuple(_) | Ty::List(_)) && matches!(op, BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq) {
            return Ok(Some(match op {
                BinOp::Lt => self.lt_values(t, l, r)?,
                BinOp::Gt => self.lt_values(t, r, l)?,
                BinOp::LtEq => {
                    let x = self.lt_values(t, r, l)?;
                    self.b.ins().bxor_imm(x, 1)
                }
                _ => {
                    let x = self.lt_values(t, l, r)?;
                    self.b.ins().bxor_imm(x, 1)
                }
            }));
        }
        if matches!(t, Ty::Con(..)) {
            let (method, is_cmp) = match op {
                BinOp::Add => ("add", false),
                BinOp::Sub => ("sub", false),
                BinOp::Mul => ("mul", false),
                BinOp::Div => ("div", false),
                BinOp::Rem => ("rem", false),
                _ => ("compare", true),
            };
            let Some((idx, sub)) = self.c.find_impl(method, t) else {
                return unsupported(&format!("the operator on type {t:?}"));
            };
            let id = self.c.impl_instance(idx, &sub)?;
            let res = rt::call(&mut self.b, &self.c.be, id, 2, &[l, r])[0];
            if !is_cmp {
                return Ok(Some(res));
            }
            // Ordering: Less = 0, Equal = 1, Greater = 2 (nullary variants are static [tag] objects).
            let tag = self.load_word(res, 0);
            let c = match op {
                BinOp::Lt => self.b.ins().icmp_imm(IntCC::Equal, tag, 0),
                BinOp::LtEq => self.b.ins().icmp_imm(IntCC::NotEqual, tag, 2),
                BinOp::Gt => self.b.ins().icmp_imm(IntCC::Equal, tag, 2),
                _ => self.b.ins().icmp_imm(IntCC::NotEqual, tag, 0),
            };
            return Ok(Some(self.b.ins().uextend(I64, c)));
        }
        Ok(None)
    }

    fn round_f32(&mut self, v: ir::Value) -> ir::Value {
        let s = self.b.ins().fdemote(ir::types::F32, v);
        self.b.ins().fpromote(F64, s)
    }

    fn in_range_fail(&mut self, v: ir::Value, bits: u32, signed: bool) -> ir::Value {
        // true when `v` does not fit in the type
        if signed {
            let sh = (64 - bits) as i64;
            let a = self.b.ins().ishl_imm(v, sh);
            let b = self.b.ins().sshr_imm(a, sh);
            self.b.ins().icmp(IntCC::NotEqual, b, v)
        } else {
            let a = self.b.ins().ushr_imm(v, bits as i64);
            self.b.ins().icmp_imm(IntCC::NotEqual, a, 0)
        }
    }

    fn int_arith(&mut self, op: BinOp, l: ir::Value, r: ir::Value, t: &Ty) -> R<ir::Value> {
        let (bits, signed) = int_kind(t).unwrap();
        let name = match t {
            Ty::Con(n, _) => n.clone(),
            _ => unreachable!(),
        };
        let ovf = format!("integer overflow in `{name}` arithmetic");
        match op {
            BinOp::Div | BinOp::Rem => {
                let z = self.b.ins().icmp_imm(IntCC::Equal, r, 0);
                self.panic_if(z, "division by zero");
                if signed {
                    if bits == 64 {
                        let m = self.b.ins().icmp_imm(IntCC::Equal, r, -1);
                        let min = self.b.ins().icmp_imm(IntCC::Equal, l, i64::MIN);
                        let both = self.b.ins().band(m, min);
                        self.panic_if(both, &ovf);
                        // x % -1 is 0, but the hardware traps for MIN; handled above.
                    }
                    let v = if op == BinOp::Div { self.b.ins().sdiv(l, r) } else { self.b.ins().srem(l, r) };
                    if bits < 64 {
                        let f = self.in_range_fail(v, bits, true);
                        self.panic_if(f, &ovf);
                    }
                    Ok(v)
                } else {
                    Ok(if op == BinOp::Div { self.b.ins().udiv(l, r) } else { self.b.ins().urem(l, r) })
                }
            }
            _ => {
                if bits == 64 {
                    let (v, o) = match (op, signed) {
                        (BinOp::Add, true) => self.b.ins().sadd_overflow(l, r),
                        (BinOp::Sub, true) => self.b.ins().ssub_overflow(l, r),
                        (BinOp::Mul, true) => self.b.ins().smul_overflow(l, r),
                        (BinOp::Add, false) => self.b.ins().uadd_overflow(l, r),
                        (BinOp::Sub, false) => self.b.ins().usub_overflow(l, r),
                        _ => self.b.ins().umul_overflow(l, r),
                    };
                    self.panic_if(o, &ovf);
                    Ok(v)
                } else {
                    let v = match op {
                        BinOp::Add => self.b.ins().iadd(l, r),
                        BinOp::Sub => self.b.ins().isub(l, r),
                        _ => self.b.ins().imul(l, r),
                    };
                    let f = self.in_range_fail(v, bits, signed);
                    self.panic_if(f, &ovf);
                    Ok(v)
                }
            }
        }
    }

    // ----- calls ---------------------------------------------------------------------

    fn call(&mut self, callee: &Expr, args: &[&Expr], whole: &Expr, tail: bool) -> R<ir::Value> {
        if let ExprKind::Var(n) = &callee.kind {
            if n == "asm" && self.lookup("asm").is_none() && !self.c.mods[self.m].fns.contains_key("asm") {
                let Some(ExprKind::Str(text)) = args.first().map(|a| &a.kind) else {
                    return Err("`asm` needs a string literal".into());
                };
                let id = self.c.asm_block(text)?;
                let mut vs = vec![];
                for a in &args[1..] {
                    vs.push(self.expr(a, false)?);
                }
                let r = rt::call(&mut self.b, &self.c.be, id, vs.len(), &vs);
                return Ok(r[0]);
            }
        }
        let _ = whole;
        // `io.println(x)` and friends
        if let ExprKind::Field { expr, name } = &callee.kind {
            if let ExprKind::Var(m) = &expr.kind {
                if self.lookup(m).is_none() {
                    match self.c.mods[self.m].ns.get(m.as_str()).map(|x| x.as_str()) {
                        Some("io") => {
                            if let Some(v) = self.io_call(&name.name, args)? {
                                return Ok(v);
                            }
                        }
                        Some("num") if self.m == 0 => {
                            if let Some(v) = self.num_builtin(&name.name, args, whole)? {
                                return Ok(v);
                            }
                        }
                        Some("bits") if self.m == 0 => {
                            if let Some(v) = self.bits_builtin(&name.name, args)? {
                                return Ok(v);
                            }
                        }
                        Some("process") if name.name == "exit" => {}
                        _ => {}
                    }
                }
            }
        }
        if let ExprKind::Var(name) = &callee.kind {
            if self.lookup(name).is_none() {
                if name.starts_with("__") && self.m == 1 {
                    return self.intrinsic(name, args, whole);
                }
                if !self.c.mods[self.m].fns.contains_key(name.as_str()) {
                    if let Some(v) = self.core_builtin(name, args)? {
                        return Ok(v);
                    }
                }
                if name == "__panic" && self.m == 1 {
                    let msg = self.expr(args[0], false)?;
                    rt::call(&mut self.b, &self.c.be, rt::RT_PANIC, 1, &[msg]);
                    self.b.ins().trap(ir::TrapCode::unwrap_user(4));
                    let dead = self.b.create_block();
                    self.b.switch_to_block(dead);
                    return Ok(self.iconst(0));
                }
                if let Some((_, idx)) = self.c.ctors.get(name).cloned() {
                    let mut vs = vec![];
                    for a in args {
                        vs.push(self.expr(a, false)?);
                    }
                    let tag = self.iconst(idx as i64);
                    let p = self.alloc(vs.len() + 1);
                    self.store_words(p, 0, &[tag]);
                    self.store_words(p, 1, &vs);
                    return Ok(p);
                }
            }
        }
        if let Some((m, fname)) = self.resolve_fn(callee) {
            let conc = self.ty_of(callee)?;
            let (id, _) = self.instance_for(m, &fname, &conc)?;
            let mut vs = vec![];
            for a in args {
                vs.push(self.expr(a, false)?);
            }
            if tail && id == self.self_id {
                // Self tail call: rebind the parameters and loop.
                for (var, v) in self.param_vars.clone().into_iter().zip(&vs) {
                    self.b.def_var(var, *v);
                }
                let head = self.loop_head.unwrap();
                self.b.ins().jump(head, &[]);
                let dead = self.b.create_block();
                self.b.switch_to_block(dead);
                return Ok(self.iconst(0));
            }
            let r = rt::call(&mut self.b, &self.c.be, id, vs.len(), &vs);
            return Ok(r[0]);
        }
        if let ExprKind::Var(name) = &callee.kind {
            if self.lookup(name).is_none() && self.is_trait_method(name) {
                return self.trait_call(name, callee, args);
            }
        }
        if let ExprKind::Field { expr, name } = &callee.kind {
            if let ExprKind::Var(ns) = &expr.kind {
                if self.lookup(ns).is_none() && self.c.mods[self.m].ns.contains_key(ns.as_str()) {
                    return unsupported(&format!("`{ns}.{}`", name.name));
                }
            }
        }
        // Anything else is a closure value.
        let f = self.expr(callee, false)?;
        let mut vs = vec![f];
        for a in args {
            vs.push(self.expr(a, false)?);
        }
        let fnptr = self.load_word(f, 0);
        let sr = self.b.import_signature(self.c.be.sig(vs.len(), 1));
        let c = self.b.ins().call_indirect(sr, fnptr, &vs);
        Ok(self.b.inst_results(c)[0])
    }

    fn is_trait_method(&self, name: &str) -> bool {
        self.c.an.trait_defs.values().any(|t| t.methods.iter().any(|(n, _)| n == name))
    }

    fn trait_self_ty(&self, callee: &Expr) -> R<Ty> {
        match self.ty_of(callee)? {
            Ty::Fn(ps, _) if !ps.is_empty() => Ok(ps[0].clone()),
            other => Err(format!("cannot resolve trait method from type {other:?}")),
        }
    }

    fn trait_call(&mut self, name: &str, callee: &Expr, args: &[&Expr]) -> R<ir::Value> {
        let self_ty = self.trait_self_ty(callee)?;
        let mut vs = vec![];
        for a in args {
            vs.push(self.expr(a, false)?);
        }
        if let Some((idx, sub)) = self.c.find_impl(name, &self_ty) {
            let id = self.c.impl_instance(idx, &sub)?;
            return Ok(rt::call(&mut self.b, &self.c.be, id, vs.len(), &vs)[0]);
        }
        // Built-in implementations for primitive types.
        match name {
            "show" => {
                let g = self.c.glue(GlueKind::Show { nested: false }, &self_ty);
                let nil = self.iconst(0);
                let acc = rt::call(&mut self.b, &self.c.be, g, 2, &[vs[0], nil])[0];
                self.call_std("show_done", &[], &[acc])
            }
            "add" | "sub" | "mul" | "div" | "rem" => {
                let op = match name {
                    "add" => BinOp::Add,
                    "sub" => BinOp::Sub,
                    "mul" => BinOp::Mul,
                    "div" => BinOp::Div,
                    _ => BinOp::Rem,
                };
                self.binary_values(op, &self_ty, vs[0], vs[1])
            }
            "neg" => {
                if is_con(&self_ty, "f64") || is_con(&self_ty, "f32") {
                    let f = self.b.ins().bitcast(F64, MemFlags::new(), vs[0]);
                    let r = self.b.ins().fneg(f);
                    Ok(self.b.ins().bitcast(I64, MemFlags::new(), r))
                } else {
                    let z = self.iconst(0);
                    self.int_arith(BinOp::Sub, z, vs[0], &self_ty)
                }
            }
            "compare" => {
                let lt = self.binary_values(BinOp::Lt, &self_ty, vs[0], vs[1])?;
                let gt = self.binary_values(BinOp::Gt, &self_ty, vs[0], vs[1])?;
                let addr = |cx: &mut Self, ctor: &str| -> R<ir::Value> {
                    let (id, idx) = cx.c.ctors.get(ctor).cloned().ok_or("Ordering is missing")?;
                    let a = cx.c.nullary_addr(&id, idx);
                    Ok(cx.iconst(a as i64))
                };
                let (l, e, g) = (addr(self, "Less")?, addr(self, "Equal")?, addr(self, "Greater")?);
                let eg = self.b.ins().select(gt, g, e);
                Ok(self.b.ins().select(lt, l, eg))
            }
            _ => unsupported(&format!("the trait method `{name}` for type {self_ty:?}")),
        }
    }

    fn intrinsic(&mut self, name: &str, args: &[&Expr], whole: &Expr) -> R<ir::Value> {
        let mut vs = vec![];
        for a in args {
            vs.push(self.expr(a, false)?);
        }
        let zero = |cx: &mut Self| cx.iconst(0);
        match name {
            "__panic" => {
                rt::call(&mut self.b, &self.c.be, rt::RT_PANIC, 1, &[vs[0]]);
                self.b.ins().trap(ir::TrapCode::unwrap_user(4));
                let dead = self.b.create_block();
                self.b.switch_to_block(dead);
                Ok(zero(self))
            }
            "__str_alloc" => {
                let sz = self.b.ins().iadd_imm(vs[0], 8 + 7);
                let sz = self.b.ins().band_imm(sz, !7);
                let p = self.alloc_bytes(sz);
                self.store_words(p, 0, &[vs[0]]);
                Ok(p)
            }
            "__str_len" => Ok(self.load_word(vs[0], 0)),
            "__byte" => {
                let a = self.b.ins().iadd(vs[0], vs[1]);
                Ok(self.b.ins().uload8(I64, MemFlags::new(), a, 8))
            }
            "__set_byte" => {
                let a = self.b.ins().iadd(vs[0], vs[1]);
                self.b.ins().istore8(MemFlags::new(), vs[2], a, 8);
                Ok(zero(self))
            }
            "__copy" => {
                let d = self.b.ins().iadd(vs[0], vs[1]);
                let d = self.b.ins().iadd_imm(d, 8);
                let sp = self.b.ins().iadd(vs[2], vs[3]);
                let sp = self.b.ins().iadd_imm(sp, 8);
                rt::call(&mut self.b, &self.c.be, rt::RT_MEMCPY, 3, &[d, sp, vs[4]]);
                Ok(zero(self))
            }
            "__f32_bits" => {
                let f = self.b.ins().bitcast(F64, MemFlags::new(), vs[0]);
                let s = self.b.ins().fdemote(ir::types::F32, f);
                let bits = self.b.ins().bitcast(ir::types::I32, MemFlags::new(), s);
                Ok(self.b.ins().uextend(I64, bits))
            }
            _ => {
                if let Some(v) = self.raw_intrinsic(name, &vs)? {
                    return Ok(v);
                }
                // `__<to>_of_<from>`: convert between integer-like types.
                if name.contains("_of_") {
                    let t = self.ty_of(whole)?;
                    return Ok(self.normalize(vs[0], &t));
                }
                unsupported(&format!("the intrinsic `{name}`"))
            }
        }
    }

    /// Wrap or extend a word to the range of integer type `t` (a no-op for non-integers).
    fn normalize(&mut self, v: ir::Value, t: &Ty) -> ir::Value {
        match int_kind(t) {
            Some((bits, signed)) if bits < 64 => {
                let sh = (64 - bits) as i64;
                let a = self.b.ins().ishl_imm(v, sh);
                if signed {
                    self.b.ins().sshr_imm(a, sh)
                } else {
                    self.b.ins().ushr_imm(a, sh)
                }
            }
            _ => v,
        }
    }

    fn io_call(&mut self, name: &str, args: &[&Expr]) -> R<Option<ir::Value>> {
        let (fd, nl) = match name {
            "println" => (1, true),
            "print" => (1, false),
            "eprintln" => (2, true),
            "eprint" => (2, false),
            "write" | "writeln" if args.len() == 2 => {
                let _console = self.expr(args[0], false)?;
                let v = self.expr(args[1], false)?;
                let fdv = self.iconst(1);
                let nlv = self.iconst((name == "writeln") as i64);
                rt::call(&mut self.b, &self.c.be, rt::RT_PRINT_STR, 3, &[fdv, v, nlv]);
                return Ok(Some(self.iconst(0)));
            }
            _ => return Ok(None),
        };
        if args.len() != 1 {
            return Err("print takes one argument".into());
        }
        let t = self.ty_of(args[0])?;
        let v = self.expr(args[0], false)?;
        let fdv = self.iconst(fd);
        let nlv = self.iconst(nl as i64);
        if is_con(&t, "String") {
            rt::call(&mut self.b, &self.c.be, rt::RT_PRINT_STR, 3, &[fdv, v, nlv]);
        } else if int_kind(&t).is_some() {
            let signed = int_kind(&t).unwrap().1;
            let sv = self.iconst(signed as i64);
            rt::call(&mut self.b, &self.c.be, rt::RT_PRINT_INT, 4, &[fdv, v, sv, nlv]);
        } else {
            let g = self.c.glue(GlueKind::Show { nested: false }, &t);
            let nil = self.iconst(0);
            let acc = rt::call(&mut self.b, &self.c.be, g, 2, &[v, nil])[0];
            let s = self.call_std("show_done", &[], &[acc])?;
            rt::call(&mut self.b, &self.c.be, rt::RT_PRINT_STR, 3, &[fdv, s, nlv]);
        }
        Ok(Some(self.iconst(0)))
    }

    // ----- match ---------------------------------------------------------------------

    fn match_expr(&mut self, scrut: &Expr, arms: &[Arm], tail: bool) -> R<ir::Value> {
        let st = self.ty_of(scrut)?;
        let sv = self.expr(scrut, false)?;
        let merge = self.b.create_block();
        self.b.append_block_param(merge, I64);
        for arm in arms {
            let next = self.b.create_block();
            self.scopes.push(HashMap::new());
            self.pat(&arm.pat, sv, &st, next)?;
            if let Some(g) = &arm.guard {
                let gv = self.expr(g, false)?;
                let ok = self.b.create_block();
                self.b.ins().brif(gv, ok, &[], next, &[]);
                self.b.switch_to_block(ok);
            }
            let v = self.expr(&arm.body, tail)?;
            self.b.ins().jump(merge, &[v.into()]);
            self.scopes.pop();
            self.b.switch_to_block(next);
        }
        // No arm matched (unreachable for exhaustive matches).
        let p = self.panic_block("no match arm matched");
        self.b.ins().jump(p, &[]);
        self.b.switch_to_block(merge);
        Ok(self.b.block_params(merge)[0])
    }

    fn fail_unless(&mut self, ok: ir::Value, fail: ir::Block) {
        let cont = self.b.create_block();
        self.b.ins().brif(ok, cont, &[], fail, &[]);
        self.b.switch_to_block(cont);
    }

    fn pat(&mut self, p: &Pattern, v: ir::Value, t: &Ty, fail: ir::Block) -> R<()> {
        match &p.kind {
            PatKind::Wild => Ok(()),
            PatKind::Bind(name) => {
                // A bare uppercase name may be a nullary constructor.
                if let Some((id, idx)) = self.c.ctors.get(name).cloned() {
                    return self.ctor_pat(&id, idx, &[], v, t, fail);
                }
                self.bind(name, v, t.clone());
                Ok(())
            }
            PatKind::Int(s) => {
                if is_con(t, "f64") || is_con(t, "f32") {
                    let k = parse_int(s).ok_or("bad literal")? as f64;
                    let kv = self.fconst(k);
                    let ok = self.b.ins().icmp(IntCC::Equal, v, kv);
                    self.fail_unless(ok, fail);
                    return Ok(());
                }
                let k = parse_int(s).ok_or("bad literal")?;
                let ok = self.b.ins().icmp_imm(IntCC::Equal, v, k as i64);
                self.fail_unless(ok, fail);
                Ok(())
            }
            PatKind::Bool(b) => {
                let ok = self.b.ins().icmp_imm(IntCC::Equal, v, *b as i64);
                self.fail_unless(ok, fail);
                Ok(())
            }
            PatKind::Float(_) => unsupported("float patterns"),
            PatKind::Str(lit) => {
                let a = self.c.string_addr(lit);
                let lv = self.iconst(a as i64);
                let ok = self.call_std("string_eq", &[], &[v, lv])?;
                self.fail_unless(ok, fail);
                Ok(())
            }
            PatKind::Tuple(ps) => {
                let Ty::Tuple(ts) = t else { return Err("tuple pattern on non-tuple".into()) };
                for (i, (sp, st)) in ps.iter().zip(ts).enumerate() {
                    let f = self.load_word(v, i);
                    self.pat(sp, f, st, fail)?;
                }
                Ok(())
            }
            PatKind::Ctor { name, args } => {
                let (id, idx) = self.c.ctors.get(&name.name).cloned().ok_or_else(|| format!("unknown constructor `{}`", name.name))?;
                self.ctor_pat(&id, idx, args, v, t, fail)
            }
            PatKind::Record { name, fields } => {
                let id = self.c.records.get(&name.name).cloned().ok_or("unknown record")?;
                let ti = self.c.type_info(&id)?;
                let TDef::Record(fs) = &ti.def else { unreachable!() };
                let targs = match t {
                    Ty::Con(_, a) => a.clone(),
                    _ => vec![],
                };
                let m: Subst = ti.generics.iter().cloned().zip(targs).collect();
                for (fname, sub) in fields {
                    let idx = fs.iter().position(|(n, _)| *n == fname.name).ok_or("unknown field")?;
                    let ft = norm(&subst_ty(&fs[idx].1, &m));
                    let f = self.load_word(v, idx);
                    match sub {
                        Some(sp) => self.pat(sp, f, &ft, fail)?,
                        None => self.bind(&fname.name, f, ft),
                    }
                }
                Ok(())
            }
            PatKind::List { items, tail } => {
                let Ty::List(et) = t else { return Err("list pattern on non-list".into()) };
                let mut cur = v;
                for sp in items {
                    let nonnil = self.b.ins().icmp_imm(IntCC::NotEqual, cur, 0);
                    self.fail_unless(nonnil, fail);
                    let h = self.load_word(cur, 0);
                    self.pat(sp, h, et, fail)?;
                    cur = self.load_word(cur, 1);
                }
                match tail {
                    Some(tp) => self.pat(tp, cur, t, fail)?,
                    None => {
                        let isnil = self.b.ins().icmp_imm(IntCC::Equal, cur, 0);
                        self.fail_unless(isnil, fail);
                    }
                }
                Ok(())
            }
        }
    }

    fn ctor_pat(&mut self, id: &str, idx: usize, args: &[Pattern], v: ir::Value, t: &Ty, fail: ir::Block) -> R<()> {
        let ti = self.c.type_info(id)?;
        let TDef::Variants(vs) = &ti.def else { unreachable!() };
        let targs = match t {
            Ty::Con(_, a) => a.clone(),
            _ => vec![],
        };
        let m: Subst = ti.generics.iter().cloned().zip(targs).collect();
        if vs.len() > 1 {
            let tag = self.load_word(v, 0);
            let ok = self.b.ins().icmp_imm(IntCC::Equal, tag, idx as i64);
            self.fail_unless(ok, fail);
        }
        for (i, sp) in args.iter().enumerate() {
            let ft = norm(&subst_ty(&vs[idx].fields[i], &m));
            let f = self.load_word(v, i + 1);
            self.pat(sp, f, &ft, fail)?;
        }
        Ok(())
    }
}

/// The lambda body outlives the compile call, but borrowck cannot see through `&mut Compiler<'a>`.
fn unsafe_extend<'a, T>(x: &T) -> &'a T {
    unsafe { &*(x as *const T) }
}

fn collect_vars(e: &Expr, out: &mut Vec<String>) {
    pub(crate) fn block(b: &Block, out: &mut Vec<String>) {
        for s in &b.stmts {
            match s {
                Stmt::Let { value, .. } | Stmt::Assign { value, .. } => collect_vars(value, out),
                Stmt::Defer(e, _) | Stmt::Expr(e) => collect_vars(e, out),
            }
        }
    }
    match &e.kind {
        ExprKind::Var(n) => {
            if !out.contains(n) {
                out.push(n.clone());
            }
        }
        ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Str(_) | ExprKind::Bool(_) | ExprKind::Unit => {}
        ExprKind::List(v) | ExprKind::Tuple(v) => v.iter().for_each(|x| collect_vars(x, out)),
        ExprKind::Cons { items, tail } => {
            items.iter().for_each(|x| collect_vars(x, out));
            collect_vars(tail, out);
        }
        ExprKind::Record { fields, .. } => fields.iter().for_each(|(_, x)| collect_vars(x, out)),
        ExprKind::Update { base, fields } => {
            collect_vars(base, out);
            fields.iter().for_each(|(_, x)| collect_vars(x, out));
        }
        ExprKind::Unary { expr, .. } | ExprKind::Try(expr) | ExprKind::Field { expr, .. } => collect_vars(expr, out),
        ExprKind::Binary { lhs, rhs, .. } | ExprKind::Pipe { lhs, rhs } => {
            collect_vars(lhs, out);
            collect_vars(rhs, out);
        }
        ExprKind::Call { callee, args } => {
            collect_vars(callee, out);
            args.iter().for_each(|x| collect_vars(x, out));
        }
        ExprKind::Index { expr, index } => {
            collect_vars(expr, out);
            collect_vars(index, out);
        }
        ExprKind::Lambda { body, .. } => collect_vars(body, out),
        ExprKind::If { cond, then, els } => {
            collect_vars(cond, out);
            block(then, out);
            if let Some(b) = els {
                block(b, out);
            }
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_vars(scrutinee, out);
            for a in arms {
                if let Some(g) = &a.guard {
                    collect_vars(g, out);
                }
                collect_vars(&a.body, out);
            }
        }
        ExprKind::Region(b) | ExprKind::Block(b) | ExprKind::Unsafe(b) => block(b, out),
        ExprKind::With { arg, body, .. } => {
            collect_vars(arg, out);
            block(body, out);
        }
        ExprKind::While { cond, body } => {
            collect_vars(cond, out);
            block(body, out);
        }
        ExprKind::For { iter, body, .. } => {
            collect_vars(iter, out);
            block(body, out);
        }
        ExprKind::Break | ExprKind::Continue => {}
    }
}

/// Numbers, booleans, characters, unit and tuples of them: values that hold no pointers.
fn is_flat(t: &Ty) -> bool {
    match t {
        Ty::Con(n, a) if a.is_empty() => {
            matches!(n.as_str(), "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize" | "f32" | "f64" | "bool" | "char")
        }
        Ty::Tuple(v) => v.iter().all(is_flat),
        _ => false,
    }
}

/// Does a value of this type contain an array or a closure (either can be mutated or can mutate)?
fn has_mutable_or_fn(t: &Ty) -> bool {
    match t {
        Ty::Con(n, a) => n.ends_with("Array") || a.iter().any(has_mutable_or_fn),
        Ty::List(x) => has_mutable_or_fn(x),
        Ty::Tuple(v) => v.iter().any(has_mutable_or_fn),
        Ty::Fn(..) => true,
        _ => false,
    }
}

enum Node<'a> {
    E(&'a Expr),
    S(&'a Stmt),
}

/// Visit every statement and expression under `b`, in source order.
fn walk_block<'a>(b: &'a Block, f: &mut dyn FnMut(Node<'a>)) {
    for s in &b.stmts {
        f(Node::S(s));
        match s {
            Stmt::Let { value, .. } | Stmt::Assign { value, .. } => walk_expr(value, f),
            Stmt::Defer(e, _) | Stmt::Expr(e) => walk_expr(e, f),
        }
    }
}

fn walk_expr<'a>(e: &'a Expr, f: &mut dyn FnMut(Node<'a>)) {
    f(Node::E(e));
    match &e.kind {
        ExprKind::Var(_) | ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Str(_) | ExprKind::Bool(_) | ExprKind::Unit | ExprKind::Break | ExprKind::Continue => {}
        ExprKind::List(v) | ExprKind::Tuple(v) => v.iter().for_each(|x| walk_expr(x, f)),
        ExprKind::Cons { items, tail } => {
            items.iter().for_each(|x| walk_expr(x, f));
            walk_expr(tail, f);
        }
        ExprKind::Record { fields, .. } => fields.iter().for_each(|(_, x)| walk_expr(x, f)),
        ExprKind::Update { base, fields } => {
            walk_expr(base, f);
            fields.iter().for_each(|(_, x)| walk_expr(x, f));
        }
        ExprKind::Unary { expr, .. } | ExprKind::Try(expr) | ExprKind::Field { expr, .. } => walk_expr(expr, f),
        ExprKind::Binary { lhs, rhs, .. } | ExprKind::Pipe { lhs, rhs } => {
            walk_expr(lhs, f);
            walk_expr(rhs, f);
        }
        ExprKind::Call { callee, args } => {
            walk_expr(callee, f);
            args.iter().for_each(|x| walk_expr(x, f));
        }
        ExprKind::Index { expr, index } => {
            walk_expr(expr, f);
            walk_expr(index, f);
        }
        ExprKind::Lambda { body, .. } => walk_expr(body, f),
        ExprKind::If { cond, then, els } => {
            walk_expr(cond, f);
            walk_block(then, f);
            if let Some(b) = els {
                walk_block(b, f);
            }
        }
        ExprKind::Match { scrutinee, arms } => {
            walk_expr(scrutinee, f);
            for a in arms {
                if let Some(g) = &a.guard {
                    walk_expr(g, f);
                }
                walk_expr(&a.body, f);
            }
        }
        ExprKind::Region(b) | ExprKind::Block(b) | ExprKind::Unsafe(b) => walk_block(b, f),
        ExprKind::With { arg, body, .. } => {
            walk_expr(arg, f);
            walk_block(body, f);
        }
        ExprKind::While { cond, body } => {
            walk_expr(cond, f);
            walk_block(body, f);
        }
        ExprKind::For { iter, body, .. } => {
            walk_expr(iter, f);
            walk_block(body, f);
        }
    }
}
