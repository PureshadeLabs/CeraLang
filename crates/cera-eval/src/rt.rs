//! The interface between the engines (tree-walker, bytecode VM) and the
//! built-in library, plus the engine-independent value operations.

use crate::value::*;
use cera_syntax::ast::BinOp;
use cera_syntax::Span;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::rc::Rc;

/// Everything a program can reach outside itself.
pub struct Host {
    pub out: RefCell<Box<dyn Write + Send>>,
    pub err: RefCell<Box<dyn Write + Send>>,
    pub input: RefCell<Box<dyn BufRead + Send>>,
    pub args: Vec<String>,
    pub fs_root: PathBuf,
}

/// How to run a program.
pub struct RunConfig {
    pub out: Box<dyn Write + Send>,
    pub err: Box<dyn Write + Send>,
    pub input: Box<dyn BufRead + Send>,
    pub args: Vec<String>,
    /// The directory the program's filesystem capability is rooted at.
    pub fs_root: PathBuf,
}

impl RunConfig {
    /// The real console, with the filesystem rooted at the current directory.
    pub fn stdio(args: Vec<String>) -> RunConfig {
        RunConfig {
            out: Box::new(std::io::stdout()),
            err: Box::new(std::io::stderr()),
            input: Box::new(std::io::BufReader::new(std::io::stdin())),
            args,
            fs_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }

    pub fn into_host(self) -> Host {
        Host {
            out: RefCell::new(self.out),
            err: RefCell::new(self.err),
            input: RefCell::new(self.input),
            args: self.args,
            fs_root: self.fs_root,
        }
    }
}

/// An engine, as the library sees it.
pub trait Rt<'a> {
    /// Call a function value.
    fn apply(&self, f: Value<'a>, args: Vec<Value<'a>>, span: Span) -> Res<'a, Value<'a>>;
    /// `apply` for one argument (engines override this to avoid the allocation).
    fn apply1(&self, f: Value<'a>, a: Value<'a>, span: Span) -> Res<'a, Value<'a>> {
        self.apply(f, vec![a], span)
    }
    /// `apply` for two arguments.
    fn apply2(&self, f: Value<'a>, a: Value<'a>, b: Value<'a>, span: Span) -> Res<'a, Value<'a>> {
        self.apply(f, vec![a, b], span)
    }
    /// Does the program implement `trait_name` for values whose head is `head`?
    fn has_impl(&self, trait_name: &str, head: &str) -> bool;
    fn host(&self) -> &Host;
}

/// The `caps` value passed to `main :: fn(caps: Caps)`.
pub fn make_caps<'a>(host: &Host) -> Value<'a> {
    let cap = |c: CapVal| Value::Cap(Rc::new(c));
    let field = |n: &str, v: Value<'a>| (Rc::from(n), v);
    Value::Record {
        ty: Rc::from("Caps"),
        fields: Rc::new(vec![
            field("console", cap(CapVal::Console)),
            field(
                "fs",
                cap(CapVal::Dir {
                    root: host.fs_root.clone(),
                    read_only: false,
                }),
            ),
            field("clock", cap(CapVal::Clock)),
            field("env", cap(CapVal::Env)),
        ]),
    }
}

pub fn call_trait<'a, R: Rt<'a> + ?Sized>(
    rt: &R,
    trait_name: &str,
    method: &str,
    args: Vec<Value<'a>>,
    span: Span,
) -> Res<'a, Value<'a>> {
    rt.apply(
        Value::trait_method(trait_name, method),
        args,
        span,
    )
}

pub fn arith<'a, R: Rt<'a> + ?Sized>(rt: &R, op: BinOp, l: Value<'a>, r: Value<'a>, span: Span) -> Res<'a, Value<'a>> {
    let bit = match op {
        BinOp::BitAnd => Some("bits.band"),
        BinOp::BitXor => Some("bits.bxor"),
        BinOp::Shl => Some("bits.shl"),
        BinOp::Shr => Some("bits.shr"),
        _ => None,
    };
    if let Some(name) = bit {
        return crate::native::call_native(rt, name, vec![l, r], span);
    }
    match (&l, &r) {
        (Value::Int(a, t), Value::Int(b, _)) => {
            let res = match op {
                BinOp::Add => a + b,
                BinOp::Sub => a - b,
                BinOp::Mul => a * b,
                BinOp::Div | BinOp::Rem => {
                    if *b == 0 {
                        return rt_err(span, "division by zero");
                    }
                    if op == BinOp::Div {
                        a / b
                    } else {
                        a % b
                    }
                }
                _ => return rt_err(span, "bad integer operator"),
            };
            if t.fits(res) {
                Ok(Value::Int(res, *t))
            } else {
                rt_err(span, format!("integer overflow in `{}` arithmetic", t.name()))
            }
        }
        (Value::Float(a, f), Value::Float(b, _)) => {
            let res = match op {
                BinOp::Add => a + b,
                BinOp::Sub => a - b,
                BinOp::Mul => a * b,
                BinOp::Div => a / b,
                BinOp::Rem => a % b,
                _ => return rt_err(span, "bad float operator"),
            };
            Ok(Value::Float(if *f { res as f32 as f64 } else { res }, *f))
        }
        _ => {
            let (tr, m) = match op {
                BinOp::Add => ("Add", "add"),
                BinOp::Sub => ("Sub", "sub"),
                BinOp::Mul => ("Mul", "mul"),
                BinOp::Div => ("Div", "div"),
                _ => ("Rem", "rem"),
            };
            call_trait(rt, tr, m, vec![l, r], span)
        }
    }
}

/// `None` means "unordered" (a NaN was involved).
pub fn compare<'a, R: Rt<'a> + ?Sized>(rt: &R, a: &Value<'a>, b: &Value<'a>, span: Span) -> Res<'a, Option<Ordering>> {
    Ok(match (a, b) {
        (Value::Int(x, _), Value::Int(y, _)) => Some(x.cmp(y)),
        (Value::Float(x, _), Value::Float(y, _)) => x.partial_cmp(y),
        (Value::Char(x), Value::Char(y)) => Some(x.cmp(y)),
        (Value::Str(x), Value::Str(y)) => Some(x.cmp(y)),
        (Value::Bool(x), Value::Bool(y)) => Some(x.cmp(y)),
        (Value::Unit, Value::Unit) => Some(Ordering::Equal),
        (Value::Tuple(x), Value::Tuple(y)) => return compare_seq(rt, x, y, span),
        (Value::List(..), Value::List(..)) => {
            return compare_seq(rt, a.list_items().unwrap(), b.list_items().unwrap(), span)
        }
        (Value::Ref(x), y) => return compare(rt, x, y, span),
        (x, Value::Ref(y)) => return compare(rt, x, y, span),
        _ => {
            let v = call_trait(rt, "Ord", "compare", vec![a.clone(), b.clone()], span)?;
            match v {
                Value::Ctor { tag, .. } => match &*tag.name {
                    "Less" => Some(Ordering::Less),
                    "Equal" => Some(Ordering::Equal),
                    "Greater" => Some(Ordering::Greater),
                    _ => return rt_err(span, "`compare` must return an Ordering"),
                },
                _ => return rt_err(span, "`compare` must return an Ordering"),
            }
        }
    })
}

fn compare_seq<'a, R: Rt<'a> + ?Sized>(rt: &R, x: &[Value<'a>], y: &[Value<'a>], span: Span) -> Res<'a, Option<Ordering>> {
    for (p, q) in x.iter().zip(y) {
        match compare(rt, p, q, span)? {
            Some(Ordering::Equal) => {}
            other => return Ok(other),
        }
    }
    Ok(Some(x.len().cmp(&y.len())))
}

/// Like `compare` but unordered values are an error (for sorting and keys).
pub fn total_cmp<'a, R: Rt<'a> + ?Sized>(rt: &R, a: &Value<'a>, b: &Value<'a>, span: Span) -> Res<'a, Ordering> {
    match compare(rt, a, b, span)? {
        Some(o) => Ok(o),
        None => rt_err(span, "cannot order these values (a NaN is involved)"),
    }
}

pub fn values_eq<'a, R: Rt<'a> + ?Sized>(rt: &R, a: &Value<'a>, b: &Value<'a>, span: Span) -> Res<'a, bool> {
    let all_eq = |x: &[Value<'a>], y: &[Value<'a>]| -> Res<'a, bool> {
        if x.len() != y.len() {
            return Ok(false);
        }
        for (p, q) in x.iter().zip(y) {
            if !values_eq(rt, p, q, span)? {
                return Ok(false);
            }
        }
        Ok(true)
    };
    Ok(match (a, b) {
        (Value::Int(x, _), Value::Int(y, _)) => x == y,
        (Value::Float(x, _), Value::Float(y, _)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Char(x), Value::Char(y)) => x == y,
        (Value::Unit, Value::Unit) => true,
        (Value::List(..), Value::List(..)) => all_eq(a.list_items().unwrap(), b.list_items().unwrap())?,
        (Value::Tuple(x), Value::Tuple(y)) => all_eq(x, y)?,
        (Value::Ctor { tag: n1, fields: f1 }, Value::Ctor { tag: n2, fields: f2 }) => {
            (Rc::ptr_eq(n1, n2) || n1.name == n2.name) && all_eq(f1, f2)?
        }
        (Value::Record { fields: f1, .. }, Value::Record { fields: f2, .. }) => {
            if f1.len() != f2.len() {
                false
            } else {
                for ((n1, v1), (n2, v2)) in f1.iter().zip(f2.iter()) {
                    if n1 != n2 || !values_eq(rt, v1, v2, span)? {
                        return Ok(false);
                    }
                }
                true
            }
        }
        (Value::Map(x), Value::Map(y)) => {
            if x.len() != y.len() {
                false
            } else {
                for ((k1, v1), (k2, v2)) in x.iter().zip(y.iter()) {
                    if !values_eq(rt, k1, k2, span)? || !values_eq(rt, v1, v2, span)? {
                        return Ok(false);
                    }
                }
                true
            }
        }
        (Value::Set(x), Value::Set(y)) => all_eq(x, y)?,
        (Value::Ref(x), y) => values_eq(rt, x, y, span)?,
        (x, Value::Ref(y)) => values_eq(rt, x, y, span)?,
        (Value::Closure(_) | Value::FnItem(_) | Value::Native(_) | Value::Fun(_), _) => {
            return rt_err(span, "functions cannot be compared")
        }
        _ => false,
    })
}

/// Text form of a value. Strings are raw at the top level and quoted inside other values.
pub fn show_value<'a, R: Rt<'a> + ?Sized>(rt: &R, v: &Value<'a>, nested: bool) -> Res<'a, String> {
    let span = Span::new(0, 0);
    if matches!(v, Value::Ctor { .. } | Value::Record { .. }) && rt.has_impl("Show", &v.head()) {
        return match call_trait(rt, "Show", "show", vec![v.clone()], span)? {
            Value::Str(s) => Ok(s.to_string()),
            _ => rt_err(span, "`show` must return a String"),
        };
    }
    let seq = |items: &[Value<'a>], open: &str, close: &str| -> Res<'a, String> {
        let mut parts = Vec::with_capacity(items.len());
        for x in items {
            parts.push(show_value(rt, x, true)?);
        }
        Ok(format!("{open}{}{close}", parts.join(", ")))
    };
    Ok(match v {
        Value::Int(x, _) => x.to_string(),
        Value::Float(x, true) => format!("{:?}", *x as f32),
        Value::Float(x, false) => format!("{x:?}"),
        Value::Str(s) => {
            if nested {
                format!("{:?}", &**s)
            } else {
                s.to_string()
            }
        }
        Value::Bool(b) => b.to_string(),
        Value::Char(c) => {
            if nested {
                format!("{c:?}")
            } else {
                c.to_string()
            }
        }
        Value::Unit => "()".to_string(),
        Value::List(..) => seq(v.list_items().unwrap(), "[", "]")?,
        Value::Tuple(items) => seq(items, "(", ")")?,
        Value::Set(items) => seq(items, "#{", "}")?,
        Value::Array(items) => {
            let items = items.borrow().clone();
            seq(&items, "[", "]")?
        }
        Value::Map(entries) => {
            let mut parts = Vec::new();
            for (k, x) in entries.iter() {
                parts.push(format!("{}: {}", show_value(rt, k, true)?, show_value(rt, x, true)?));
            }
            format!("{{{}}}", parts.join(", "))
        }
        Value::Ctor { tag, fields } => {
            if fields.is_empty() {
                cera_syntax::display_name(&tag.name).to_string()
            } else {
                format!("{}{}", cera_syntax::display_name(&tag.name), seq(fields, "(", ")")?)
            }
        }
        Value::Record { ty, fields } => {
            if &**ty == "Error" {
                if let Some((_, Value::Str(m))) = fields.first() {
                    return Ok(m.to_string());
                }
            }
            let mut parts = Vec::new();
            for (n, x) in fields.iter() {
                parts.push(format!("{n}: {}", show_value(rt, x, true)?));
            }
            format!("{}{{{}}}", cera_syntax::display_name(ty), parts.join(", "))
        }
        Value::Ref(inner) => show_value(rt, inner, nested)?,
        Value::Allocator => "<allocator>".to_string(),
        Value::Cap(_) => "<capability>".to_string(),
        Value::Closure(_)
        | Value::Fun(_)
        | Value::FnItem(_)
        | Value::CtorFn { .. }
        | Value::TraitMethod(_)
        | Value::Native(_) => "<function>".to_string(),
    })
}
