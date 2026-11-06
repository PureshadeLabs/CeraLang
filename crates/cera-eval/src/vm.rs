//! The bytecode virtual machine.
//!
//! Cera-to-Cera calls use an explicit frame stack, so non-tail recursion is
//! bounded by `MAX_FRAMES` rather than the Rust stack, and tail calls reuse the
//! current frame. Built-ins that call back into Cera (`List.map` ...) start a
//! nested run with its own stacks.

use crate::bytecode::*;
use crate::eval::STACK_BYTES;
use crate::rt::{arith, call_trait, compare, make_caps, values_eq, Host, Rt};
use crate::value::*;
use cera_syntax::ast::BinOp;
use cera_syntax::Span;
use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::io::Write;
use std::rc::Rc;

const MAX_FRAMES: usize = 400_000;
#[cfg(not(target_arch = "wasm32"))]
const STACK_MARGIN: usize = 32 << 20;
#[cfg(target_arch = "wasm32")]
const STACK_MARGIN: usize = 2 << 20;
const OVERFLOW: &str = "stack overflow: recursion is too deep (only tail calls are free)";

type V = Value<'static>;
type R<T> = Res<'static, T>;

struct Frame {
    func: usize,
    ip: usize,
    base: usize,
    /// The closure being run (`None` for top-level functions, which capture nothing).
    fun: Option<Rc<FunVal<'static>>>,
    defers: Vec<V>,
}

pub struct Vm {
    bc: Bytecode,
    host: Host,
    global_cache: RefCell<Vec<Option<V>>>,
    stack_base: Cell<usize>,
    /// Spare stacks for nested runs (built-ins calling back into Cera).
    pool: RefCell<Vec<(Vec<V>, Vec<Frame>)>>,
}

enum Called {
    /// A frame was pushed (or reused) and must be run.
    Frame,
    /// The result is on top of the stack.
    Value,
}

impl Vm {
    pub fn new(bc: Bytecode, host: Host) -> Vm {
        let n = bc.globals.len();
        Vm {
            bc,
            host,
            global_cache: RefCell::new(vec![None; n]),
            stack_base: Cell::new(0),
            pool: RefCell::new(Vec::new()),
        }
    }

    /// Run `main`. Returns the process exit code.
    pub fn run_main(&self) -> Result<i32, RuntimeError> {
        let Some(&main) = self.bc.fns.get("main") else {
            return Err(RuntimeError::new(None, "this program has no `main` function"));
        };
        let args = match self.bc.chunks[main].arity {
            0 => vec![],
            1 => vec![make_caps(&self.host)],
            _ => return Err(RuntimeError::new(None, "`main` takes no arguments or just `caps: Caps`")),
        };
        let marker = 0u8;
        self.stack_base.set(&marker as *const u8 as usize);
        let f = Value::Fun(Rc::new(FunVal {
            func: main,
            captures: vec![],
        }));
        let result = Rt::apply(self, f, args, Span::new(0, 0));
        let _ = self.host.out.borrow_mut().flush();
        let _ = self.host.err.borrow_mut().flush();
        match result {
            Ok(Value::Ctor { tag, fields }) if &*tag.name == "Err" => {
                let shown = crate::rt::show_value(self, &fields[0], false).unwrap_or_else(|_| "<error>".to_string());
                Err(RuntimeError::new(None, format!("main returned an error: {shown}")))
            }
            Ok(_) => Ok(0),
            Err(Ctl::Error(e)) => Err(e),
            Err(Ctl::Return(_)) => Ok(0),
            Err(Ctl::Exit(c)) => Ok(c),
            Err(Ctl::Break) | Err(Ctl::Continue) => Ok(0),
        }
    }

    // ----- calls ----------------------------------------------------------------

    fn push_frame(&self, fv: &Rc<FunVal<'static>>, argc: usize, stack: &mut Vec<V>, frames: &mut Vec<Frame>, span: Span) -> R<()> {
        let chunk = &self.bc.chunks[fv.func];
        if chunk.arity != argc {
            return rt_err(span, format!("expected {} argument(s), got {argc}", chunk.arity));
        }
        if frames.len() >= MAX_FRAMES {
            return rt_err(span, OVERFLOW);
        }
        let base = stack.len() - argc;
        stack.resize(base + chunk.nlocals, Value::Unit);
        frames.push(Frame {
            func: fv.func,
            ip: 0,
            base,
            fun: Some(fv.clone()),
            defers: Vec::new(),
        });
        Ok(())
    }

    /// Call the value below the `argc` arguments on top of the stack.
    fn call(&self, stack: &mut Vec<V>, frames: &mut Vec<Frame>, argc: usize, tail: bool, span: Span) -> R<Called> {
        let callee_pos = stack.len() - argc - 1;
        let mut callee = stack[callee_pos].clone();
        loop {
            match callee {
                Value::Fun(fv) => {
                    if tail {
                        let chunk = &self.bc.chunks[fv.func];
                        if chunk.arity != argc {
                            return rt_err(span, format!("expected {} argument(s), got {argc}", chunk.arity));
                        }
                        let fr = frames.last_mut().expect("tail call outside a frame");
                        debug_assert!(fr.defers.is_empty());
                        let args: Vec<V> = stack.drain(callee_pos + 1..).collect();
                        stack.truncate(fr.base);
                        stack.extend(args);
                        stack.resize(fr.base + chunk.nlocals, Value::Unit);
                        fr.func = fv.func;
                        fr.ip = 0;
                        fr.fun = Some(fv);
                    } else {
                        stack[callee_pos] = Value::Fun(fv.clone());
                        self.push_frame(&fv, argc, stack, frames, span)?;
                    }
                    return Ok(Called::Frame);
                }
                Value::CtorFn { tag, arity } => {
                    if arity != argc {
                        return rt_err(span, format!("`{}` takes {arity} argument(s), got {argc}", tag.name));
                    }
                    let fields: Vec<V> = stack.drain(callee_pos + 1..).collect();
                    stack.truncate(callee_pos);
                    stack.push(Value::Ctor {
                        tag,
                        fields: Rc::new(fields),
                    });
                    return Ok(Called::Value);
                }
                Value::TraitMethod(tr) => {
                    let (trait_name, method) = (&tr.trait_name, &tr.method);
                    if argc == 0 {
                        return rt_err(span, format!("`{method}` needs a receiver argument"));
                    }
                    let head = stack[callee_pos + 1].head();
                    let user = self
                        .bc
                        .impls
                        .get(&(trait_name.to_string(), head))
                        .and_then(|m| m.get(&**method))
                        .copied();
                    match user {
                        Some(func) => {
                            callee = Value::Fun(Rc::new(FunVal {
                                func,
                                captures: vec![],
                            }));
                            stack[callee_pos] = callee.clone();
                            continue;
                        }
                        None => {
                            let args: Vec<V> = stack.drain(callee_pos + 1..).collect();
                            stack.truncate(callee_pos);
                            let v = crate::native::native_method(self, &method, args, span)?;
                            stack.push(v);
                            return Ok(Called::Value);
                        }
                    }
                }
                Value::Native(name) => {
                    let args: Vec<V> = stack.drain(callee_pos + 1..).collect();
                    stack.truncate(callee_pos);
                    let v = crate::native::call_native(self, name, args, span)?;
                    stack.push(v);
                    return Ok(Called::Value);
                }
                Value::Ref(inner) => {
                    callee = (*inner).clone();
                    stack[callee_pos] = callee.clone();
                }
                _ => return rt_err(span, "this value is not a function"),
            }
        }
    }

    /// Run the defers of one block or frame, last registered first. Returns the
    /// first runtime error, if any; all of them run regardless.
    fn run_defers(&self, defers: Vec<V>, span: Span) -> Option<Ctl<'static>> {
        let mut first = None;
        for d in defers.into_iter().rev() {
            if let Err(Ctl::Error(e)) = Rt::apply(self, d, vec![], span) {
                if first.is_none() {
                    first = Some(Ctl::Error(e));
                }
            }
        }
        first
    }

    /// Leave the current frame with the value on top of the stack. Returns the
    /// final result when the last frame of this run has finished.
    fn do_return(&self, stack: &mut Vec<V>, frames: &mut Vec<Frame>, span: Span) -> R<Option<V>> {
        let result = stack.pop().expect("return without a value");
        let mut fr = frames.pop().expect("return without a frame");
        let defers = std::mem::take(&mut fr.defers);
        let failed = if defers.is_empty() { None } else { self.run_defers(defers, span) };
        stack.truncate(fr.base - 1);
        if let Some(e) = failed {
            return Err(e);
        }
        if frames.is_empty() {
            Ok(Some(result))
        } else {
            stack.push(result);
            Ok(None)
        }
    }

    fn global(&self, i: usize, span: Span) -> R<V> {
        if let Some(v) = &self.global_cache.borrow()[i] {
            return Ok(v.clone());
        }
        let thunk = Value::Fun(Rc::new(FunVal {
            func: self.bc.globals[i],
            captures: vec![],
        }));
        let v = Rt::apply(self, thunk, vec![], span)?;
        self.global_cache.borrow_mut()[i] = Some(v.clone());
        Ok(v)
    }

    // ----- the loop ---------------------------------------------------------------

    fn exec(&self, stack: &mut Vec<V>, frames: &mut Vec<Frame>) -> R<V> {
        let result = self.exec_inner(stack, frames);
        if result.is_err() {
            // Unwind: every frame still active runs its defers.
            while let Some(mut fr) = frames.pop() {
                let defers = std::mem::take(&mut fr.defers);
                if !defers.is_empty() {
                    let _ = self.run_defers(defers, Span::new(0, 0));
                }
            }
        }
        result
    }

    fn exec_inner(&self, stack: &mut Vec<V>, frames: &mut Vec<Frame>) -> R<V> {
        // The hot state lives in locals; `save!` writes the instruction pointer
        // back to the frame before anything that can change the frame stack.
        let mut fi;
        let mut ip;
        let mut base;
        let mut chunk: &Chunk;
        macro_rules! load {
            () => {{
                fi = frames.len() - 1;
                let fr = &frames[fi];
                ip = fr.ip;
                base = fr.base;
                chunk = &self.bc.chunks[fr.func];
            }};
        }
        macro_rules! save {
            () => {
                frames[fi].ip = ip
            };
        }
        macro_rules! sp {
            () => {
                chunk.spans[ip - 1]
            };
        }
        macro_rules! pop {
            () => {
                // The compiler keeps the stack balanced (tests run with debug assertions).
                stack.pop().unwrap()
            };
        }
        load!();
        loop {
            // Every chunk ends in a transfer and jump targets are in range.
            debug_assert!(ip < chunk.code.len());
            let op = unsafe { *chunk.code.get_unchecked(ip) };
            ip += 1;
            match op {
                Op::Const(i) => stack.push(self.bc.consts[i].clone()),
                Op::Unit => stack.push(Value::Unit),
                Op::Pop => {
                    stack.pop();
                }
                Op::Swap => {
                    let n = stack.len();
                    stack.swap(n - 1, n - 2);
                }
                Op::Local(s) => {
                    debug_assert!(base + s < stack.len());
                    let v = unsafe { stack.get_unchecked(base + s) }.clone();
                    stack.push(v);
                }
                Op::SetLocal(s) => {
                    let v = pop!();
                    debug_assert!(base + s < stack.len());
                    *unsafe { stack.get_unchecked_mut(base + s) } = v;
                }
                Op::Capture(i) => stack.push(frames[fi].fun.as_ref().expect("capture outside a closure").captures[i].clone()),
                Op::Global(i) => {
                    let v = self.global(i, sp!())?;
                    stack.push(v);
                }
                Op::MakeList(n) => {
                    let items = stack.split_off(stack.len() - n);
                    stack.push(Value::list(items));
                }
                Op::MakeTuple(n) => {
                    let items = stack.split_off(stack.len() - n);
                    stack.push(Value::Tuple(Rc::new(items)));
                }
                Op::MakeCons(n) => {
                    let tail = pop!();
                    let mut items = stack.split_off(stack.len() - n);
                    match tail.list_items() {
                        Some(rest) => {
                            items.extend(rest.iter().cloned());
                            stack.push(Value::list(items));
                        }
                        None => return rt_err(sp!(), "the tail of a list must be a list"),
                    }
                }
                Op::MakeRecord(d) => {
                    let desc = &self.bc.records[d];
                    let mut given: Vec<Option<V>> =
                        stack.split_off(stack.len() - desc.perm.len()).into_iter().map(Some).collect();
                    let fields = desc
                        .names
                        .iter()
                        .zip(&desc.perm)
                        .map(|(n, i)| (n.clone(), given[*i].take().unwrap()))
                        .collect();
                    stack.push(Value::Record {
                        ty: desc.ty.clone(),
                        fields: Rc::new(fields),
                    });
                }
                Op::UpdateRecord(d) => {
                    let desc = &self.bc.records[d];
                    let new = stack.split_off(stack.len() - desc.names.len());
                    let base = pop!();
                    let Value::Record { ty, fields } = base.clone() else {
                        return rt_err(sp!(), "`{value | field: ..}` needs a record");
                    };
                    let mut out = (*fields).clone();
                    for (n, v) in desc.names.iter().zip(new) {
                        match out.iter_mut().find(|(f, _)| f == n) {
                            Some(slot) => slot.1 = v,
                            None => return rt_err(sp!(), format!("no field `{n}`")),
                        }
                    }
                    stack.push(Value::Record { ty, fields: Rc::new(out) });
                }
                Op::Neg => {
                    let v = pop!();
                    let r = match v {
                        Value::Int(x, t) => {
                            if t.fits(-x) {
                                Value::Int(-x, t)
                            } else {
                                return rt_err(sp!(), "integer overflow");
                            }
                        }
                        Value::Float(x, f) => Value::Float(-x, f),
                        other => call_trait(self, "Neg", "neg", vec![other], sp!())?,
                    };
                    stack.push(r);
                }
                Op::Not => match pop!() {
                    Value::Bool(b) => stack.push(Value::Bool(!b)),
                    _ => return rt_err(sp!(), "bad operand for unary operator"),
                },
                Op::BitNot => {
                    let v = pop!();
                    let r = crate::native::call_native(self, "bits.bnot", vec![v], sp!())?;
                    stack.push(r);
                }
                Op::Arith(o) => {
                    let r = pop!();
                    let l = pop!();
                    // Fast path: same-typed integers that do not overflow.
                    if let (Value::Int(a, t), Value::Int(b, _)) = (&l, &r) {
                        let fast = match o {
                            BinOp::Add => Some(a + b),
                            BinOp::Sub => Some(a - b),
                            BinOp::Mul => a.checked_mul(*b),
                            BinOp::Div if *b != 0 => Some(a / b),
                            BinOp::Rem if *b != 0 => Some(a % b),
                            _ => None,
                        };
                        if let Some(s) = fast {
                            if t.fits(s) {
                                stack.push(Value::Int(s, *t));
                                continue;
                            }
                        }
                    }
                    let v = arith(self, o, l, r, sp!())?;
                    stack.push(v);
                }
                Op::ArithK(o, k) => {
                    let l = pop!();
                    if let (Value::Int(a, t), Value::Int(b, _)) = (&l, &self.bc.consts[k]) {
                        let s = match o {
                            BinOp::Add => a + b,
                            BinOp::Sub => a - b,
                            _ => a.wrapping_mul(*b),
                        };
                        if t.fits(s) && (o != BinOp::Mul || a.checked_mul(*b).is_some()) {
                            stack.push(Value::Int(s, *t));
                            continue;
                        }
                    }
                    let v = arith(self, o, l, self.bc.consts[k].clone(), sp!())?;
                    stack.push(v);
                }
                Op::LocalArithK(o, s, k) => {
                    let l = unsafe { stack.get_unchecked(base + s as usize) };
                    if let (Value::Int(a, t), Value::Int(b, _)) = (l, &self.bc.consts[k as usize]) {
                        let r = match o {
                            BinOp::Add => a.checked_add(*b),
                            BinOp::Sub => a.checked_sub(*b),
                            _ => a.checked_mul(*b),
                        };
                        if let Some(s) = r {
                            if t.fits(s) {
                                let v = Value::Int(s, *t);
                                stack.push(v);
                                continue;
                            }
                        }
                    }
                    let l = l.clone();
                    let v = arith(self, o, l, self.bc.consts[k as usize].clone(), sp!())?;
                    stack.push(v);
                }
                Op::LocalCmpJumpK(o, s, k, target) => {
                    let l = unsafe { stack.get_unchecked(base + s as usize) };
                    let ok = match (l, &self.bc.consts[k as usize]) {
                        (Value::Int(a, _), Value::Int(b, _)) => {
                            let ord = a.cmp(b);
                            match o {
                                BinOp::Lt => ord == Ordering::Less,
                                BinOp::LtEq => ord != Ordering::Greater,
                                BinOp::Gt => ord == Ordering::Greater,
                                _ => ord != Ordering::Less,
                            }
                        }
                        (l, c) => {
                            let l = l.clone();
                            self.cmp_bool(o, &l, c, sp!())?
                        }
                    };
                    if !ok {
                        ip = target as usize;
                    }
                }
                Op::ReturnLocal(s) => {
                    let v = unsafe { stack.get_unchecked(base + s as usize) }.clone();
                    stack.push(v);
                    if frames.len() > 1 && frames[fi].defers.is_empty() {
                        let result = pop!();
                        stack.truncate(base - 1);
                        stack.push(result);
                        frames.pop();
                    } else {
                        save!();
                        if let Some(r) = self.do_return(stack, frames, sp!())? {
                            return Ok(r);
                        }
                    }
                    load!();
                }
                Op::CmpJumpK(o, k, target) => {
                    let l = pop!();
                    if !self.cmp_bool(o, &l, &self.bc.consts[k as usize], sp!())? {
                        ip = target as usize;
                    }
                }
                Op::CallFn(func, argc) => {
                    let (func, argc) = (func as usize, argc as usize);
                    let callee = &self.bc.chunks[func];
                    let cp = stack.len() - argc - 1;
                    if callee.arity != argc {
                        return rt_err(sp!(), format!("expected {} argument(s), got {argc}", callee.arity));
                    }
                    if frames.len() >= MAX_FRAMES {
                        return rt_err(sp!(), OVERFLOW);
                    }
                    stack.extend((0..callee.nlocals - argc).map(|_| Value::Unit));
                    save!();
                    frames.push(Frame {
                        func,
                        ip: 0,
                        base: cp + 1,
                        fun: None,
                        defers: Vec::new(),
                    });
                    load!();
                }
                Op::TailCallFn(func, argc) => {
                    let (func, argc) = (func as usize, argc as usize);
                    let callee = &self.bc.chunks[func];
                    let cp = stack.len() - argc - 1;
                    if callee.arity != argc {
                        return rt_err(sp!(), format!("expected {} argument(s), got {argc}", callee.arity));
                    }
                    let fr = &mut frames[fi];
                    debug_assert!(fr.defers.is_empty());
                    // Slide the arguments down over this frame's locals.
                    stack.drain(fr.base..=cp);
                    stack.extend((0..callee.nlocals - argc).map(|_| Value::Unit));
                    fr.func = func;
                    fr.ip = 0;
                    fr.fun = None;
                    load!();
                }
                Op::Cmp(o) => {
                    let r = pop!();
                    let l = pop!();
                    let b = self.cmp_bool(o, &l, &r, sp!())?;
                    stack.push(Value::Bool(b));
                }
                Op::CmpJump(o, target) => {
                    let r = pop!();
                    let l = pop!();
                    if !self.cmp_bool(o, &l, &r, sp!())? {
                        ip = target;
                    }
                }
                Op::Eq | Op::NotEq => {
                    let r = pop!();
                    let l = pop!();
                    let eq = match (&l, &r) {
                        (Value::Int(x, _), Value::Int(y, _)) => x == y,
                        _ => values_eq(self, &l, &r, sp!())?,
                    };
                    stack.push(Value::Bool(if matches!(op, Op::Eq) { eq } else { !eq }));
                }
                Op::ExpectBool => {
                    if !matches!(stack.last(), Some(Value::Bool(_))) {
                        return rt_err(sp!(), "operand is not a bool");
                    }
                }
                Op::AndJump(t) | Op::OrJump(t) => match pop!() {
                    Value::Bool(b) => {
                        let is_and = matches!(op, Op::AndJump(_));
                        if (is_and && !b) || (!is_and && b) {
                            stack.push(Value::Bool(b));
                            ip = t;
                        }
                    }
                    _ => return rt_err(sp!(), "operand is not a bool"),
                },
                Op::Field(n) => {
                    let name = &self.bc.names[n];
                    match pop!() {
                        Value::Record { fields, .. } => match fields.iter().find(|(f, _)| f == name) {
                            Some((_, v)) => stack.push(v.clone()),
                            None => return rt_err(sp!(), format!("no field `{name}`")),
                        },
                        _ => return rt_err(sp!(), format!("no field `{name}` on this value")),
                    }
                }
                Op::Index => {
                    let i = pop!();
                    let xs = pop!();
                    if let (Value::Array(a), Value::Int(i, _)) = (&xs, &i) {
                        let a = a.borrow();
                        if *i >= 0 && (*i as usize) < a.len() {
                            stack.push(a[*i as usize].clone());
                            continue;
                        }
                        return rt_err(sp!(), format!("index {i} is out of range for an array of length {}", a.len()));
                    }
                    match (xs.list_items(), i) {
                        (Some(items), Value::Int(i, _)) => {
                            if i >= 0 && (i as usize) < items.len() {
                                stack.push(items[i as usize].clone());
                            } else {
                                return rt_err(
                                    sp!(),
                                    format!("index {i} is out of range for a list of length {}", items.len()),
                                );
                            }
                        }
                        _ => return rt_err(sp!(), "cannot index this value"),
                    }
                }
                Op::Try => {
                    let v = pop!();
                    match &v {
                        Value::Ctor { tag, fields } => match &*tag.name {
                            "Some" | "Ok" => stack.push(fields[0].clone()),
                            "None" | "Err" => {
                                stack.push(v.clone());
                                save!();
                                if let Some(r) = self.do_return(stack, frames, sp!())? {
                                    return Ok(r);
                                }
                                load!();
                            }
                            _ => return rt_err(sp!(), "`?` needs an Option or a Result"),
                        },
                        _ => return rt_err(sp!(), "`?` needs an Option or a Result"),
                    }
                }
                Op::Call(argc) => {
                    let cp = stack.len() - argc - 1;
                    // Fast path: a compiled function.
                    if let Value::Fun(fv) = &stack[cp] {
                        let callee = &self.bc.chunks[fv.func];
                        if callee.arity == argc && frames.len() < MAX_FRAMES {
                            let fv = fv.clone();
                            let nbase = cp + 1;
                            stack.extend((0..callee.nlocals - argc).map(|_| Value::Unit));
                            save!();
                            frames.push(Frame {
                                func: fv.func,
                                ip: 0,
                                base: nbase,
                                fun: Some(fv),
                                defers: Vec::new(),
                            });
                            load!();
                            continue;
                        }
                    }
                    save!();
                    self.call(stack, frames, argc, false, sp!())?;
                    load!();
                }
                Op::TailCall(argc) => {
                    save!();
                    match self.call(stack, frames, argc, true, sp!())? {
                        Called::Frame => {}
                        Called::Value => {
                            if let Some(r) = self.do_return(stack, frames, sp!())? {
                                return Ok(r);
                            }
                        }
                    }
                    load!();
                }
                Op::Return => {
                    if frames.len() > 1 && frames[fi].defers.is_empty() {
                        let result = pop!();
                        stack.truncate(base - 1);
                        stack.push(result);
                        frames.pop();
                    } else {
                        save!();
                        if let Some(r) = self.do_return(stack, frames, sp!())? {
                            return Ok(r);
                        }
                    }
                    load!();
                }
                Op::Closure { func, ncap } => {
                    let (func, ncap) = (func as usize, ncap as usize);
                    let captures = stack.split_off(stack.len() - ncap);
                    stack.push(Value::Fun(Rc::new(FunVal { func, captures })));
                }
                Op::Jump(t) => ip = t,
                Op::SaveSp(s) => {
                    let h = stack.len() as i128;
                    stack[base + s] = Value::Int(h, IntTy::I64);
                }
                Op::JumpSp(s, t) => {
                    if let Value::Int(h, _) = stack[base + s] {
                        stack.truncate(h as usize);
                    }
                    ip = t;
                }
                Op::ForNext { list, idx, var, exit } => {
                    let i = match &stack[base + idx as usize] {
                        Value::Int(i, _) => *i as usize,
                        _ => 0,
                    };
                    let item = stack[base + list as usize].list_items().and_then(|items| items.get(i).cloned());
                    match item {
                        Some(v) => {
                            stack[base + var as usize] = v;
                            stack[base + idx as usize] = Value::Int(i as i128 + 1, IntTy::I64);
                        }
                        None => ip = exit as usize,
                    }
                }
                Op::JumpIfFalse(t) => match pop!() {
                    Value::Bool(true) => {}
                    Value::Bool(false) => ip = t,
                    _ => return rt_err(sp!(), "condition is not a bool"),
                },
                Op::MatchPat { pat, slot, fail } => {
                    let v = stack[base + slot as usize].clone();
                    if !match_pat(&self.bc.pats[pat as usize], &v, stack, base) {
                        ip = fail as usize;
                    }
                }
                Op::MatchFail => return rt_err(sp!(), "no match arm matched the value"),
                Op::PushDefer => {
                    let d = pop!();
                    frames[fi].defers.push(d);
                }
                Op::RunDefers(n) => {
                    let fr = &mut frames[fi];
                    let at = fr.defers.len() - n;
                    let mine = fr.defers.split_off(at);
                    if let Some(e) = self.run_defers(mine, sp!()) {
                        return Err(e);
                    }
                }
                Op::Fail(m) => return rt_err(sp!(), self.bc.messages[m].clone()),
            }
        }
    }

    #[inline]
    fn cmp_bool(&self, op: BinOp, l: &V, r: &V, span: Span) -> R<bool> {
        let ord = match (l, r) {
            (Value::Int(a, _), Value::Int(b, _)) => Some(a.cmp(b)),
            _ => compare(self, l, r, span)?,
        };
        Ok(match ord {
            None => false,
            Some(o) => match op {
                BinOp::Lt => o == Ordering::Less,
                BinOp::LtEq => o != Ordering::Greater,
                BinOp::Gt => o == Ordering::Greater,
                _ => o != Ordering::Less,
            },
        })
    }

    fn check_native_stack(&self, span: Span) -> R<()> {
        let marker = 0u8;
        let here = &marker as *const u8 as usize;
        if self.stack_base.get().saturating_sub(here) > STACK_BYTES - STACK_MARGIN {
            return rt_err(span, OVERFLOW);
        }
        Ok(())
    }
}

fn match_pat(p: &Pat, v: &V, stack: &mut [V], base: usize) -> bool {
    match p {
        Pat::Wild => true,
        Pat::Bind(s) => {
            stack[base + s] = v.clone();
            true
        }
        Pat::Int(y) => matches!(v, Value::Int(x, _) if x == y),
        Pat::Float(y) => matches!(v, Value::Float(x, _) if x == y),
        Pat::Str(s) => matches!(v, Value::Str(x) if **x == **s),
        Pat::Bool(b) => matches!(v, Value::Bool(x) if x == b),
        Pat::Ctor { name, args } => match v {
            Value::Ctor { tag, fields } => {
                &tag.name == name
                    && args.len() == fields.len()
                    && args.iter().zip(fields.iter()).all(|(a, f)| match_pat(a, f, stack, base))
            }
            Value::Ref(inner) => match_pat(p, inner, stack, base),
            _ => false,
        },
        Pat::Record { ty, fields: fps } => match v {
            Value::Record { ty: t, fields } if t == ty => {
                for (fname, sub) in fps {
                    let Some((_, fv)) = fields.iter().find(|(n, _)| n == fname) else {
                        return false;
                    };
                    if !match_pat(sub, fv, stack, base) {
                        return false;
                    }
                }
                true
            }
            Value::Ref(inner) => match_pat(p, inner, stack, base),
            _ => false,
        },
        Pat::Tuple(ps) => match v {
            Value::Tuple(vs) => {
                ps.len() == vs.len() && ps.iter().zip(vs.iter()).all(|(a, f)| match_pat(a, f, stack, base))
            }
            Value::Unit => ps.is_empty(),
            _ => false,
        },
        Pat::List { items, tail } => {
            let Some(xs) = v.list_items() else { return false };
            if xs.len() < items.len() || (tail.is_none() && xs.len() != items.len()) {
                return false;
            }
            if !items.iter().zip(xs.iter()).all(|(a, f)| match_pat(a, f, stack, base)) {
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
                    match_pat(t, &rest, stack, base)
                }
            }
        }
    }
}

impl Vm {
    fn take_buffers(&self) -> (Vec<V>, Vec<Frame>) {
        self.pool
            .borrow_mut()
            .pop()
            .unwrap_or_else(|| (Vec::with_capacity(256), Vec::with_capacity(16)))
    }

    /// Run the call whose callee and `argc` arguments are already on `stack`.
    fn run_prepared(&self, mut stack: Vec<V>, mut frames: Vec<Frame>, argc: usize, span: Span) -> R<V> {
        let started = match &stack[0] {
            Value::Fun(fv) => {
                let fv = fv.clone();
                match self.push_frame(&fv, argc, &mut stack, &mut frames, span) {
                    Ok(()) => Ok(Called::Frame),
                    Err(e) => Err(e),
                }
            }
            _ => self.call(&mut stack, &mut frames, argc, false, span),
        };
        let result = match started {
            Ok(Called::Value) => Ok(stack.pop().expect("call result")),
            Ok(Called::Frame) => self.exec(&mut stack, &mut frames),
            Err(e) => Err(e),
        };
        // Give the buffers back unless a deep run left them huge.
        if stack.capacity() <= 1 << 16 {
            stack.clear();
            frames.clear();
            self.pool.borrow_mut().push((stack, frames));
        }
        result
    }
}

impl Rt<'static> for Vm {
    fn apply(&self, f: V, args: Vec<V>, span: Span) -> R<V> {
        self.check_native_stack(span)?;
        let argc = args.len();
        let (mut stack, frames) = self.take_buffers();
        stack.push(f);
        stack.extend(args);
        self.run_prepared(stack, frames, argc, span)
    }

    fn apply1(&self, f: V, a: V, span: Span) -> R<V> {
        self.check_native_stack(span)?;
        let (mut stack, frames) = self.take_buffers();
        stack.push(f);
        stack.push(a);
        self.run_prepared(stack, frames, 1, span)
    }

    fn apply2(&self, f: V, a: V, b: V, span: Span) -> R<V> {
        self.check_native_stack(span)?;
        let (mut stack, frames) = self.take_buffers();
        stack.push(f);
        stack.push(a);
        stack.push(b);
        self.run_prepared(stack, frames, 2, span)
    }

    fn has_impl(&self, trait_name: &str, head: &str) -> bool {
        self.bc.impls.contains_key(&(trait_name.to_string(), head.to_string()))
    }

    fn host(&self) -> &Host {
        &self.host
    }
}
