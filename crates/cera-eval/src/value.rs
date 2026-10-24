//! Runtime values.

use cera_syntax::ast::{Expr, Param};
use cera_syntax::Span;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntTy {
    I8,
    I16,
    I32,
    I64,
    Isize,
    U8,
    U16,
    U32,
    U64,
    Usize,
}

impl IntTy {
    pub fn from_name(n: &str) -> Option<IntTy> {
        Some(match n {
            "i8" => IntTy::I8,
            "i16" => IntTy::I16,
            "i32" => IntTy::I32,
            "i64" => IntTy::I64,
            "isize" => IntTy::Isize,
            "u8" => IntTy::U8,
            "u16" => IntTy::U16,
            "u32" => IntTy::U32,
            "u64" => IntTy::U64,
            "usize" => IntTy::Usize,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            IntTy::I8 => "i8",
            IntTy::I16 => "i16",
            IntTy::I32 => "i32",
            IntTy::I64 => "i64",
            IntTy::Isize => "isize",
            IntTy::U8 => "u8",
            IntTy::U16 => "u16",
            IntTy::U32 => "u32",
            IntTy::U64 => "u64",
            IntTy::Usize => "usize",
        }
    }

    pub fn range(self) -> (i128, i128) {
        match self {
            IntTy::I8 => (i8::MIN as i128, i8::MAX as i128),
            IntTy::I16 => (i16::MIN as i128, i16::MAX as i128),
            IntTy::I32 => (i32::MIN as i128, i32::MAX as i128),
            IntTy::I64 | IntTy::Isize => (i64::MIN as i128, i64::MAX as i128),
            IntTy::U8 => (0, u8::MAX as i128),
            IntTy::U16 => (0, u16::MAX as i128),
            IntTy::U32 => (0, u32::MAX as i128),
            IntTy::U64 | IntTy::Usize => (0, u64::MAX as i128),
        }
    }

    pub fn fits(self, v: i128) -> bool {
        let (lo, hi) = self.range();
        v >= lo && v <= hi
    }
}

pub struct Env<'a> {
    vars: RefCell<Vec<(String, Value<'a>)>>,
    parent: Option<Rc<Env<'a>>>,
}

impl<'a> Env<'a> {
    pub fn root() -> Rc<Env<'a>> {
        Rc::new(Env {
            vars: RefCell::new(Vec::new()),
            parent: None,
        })
    }

    pub fn child(parent: &Rc<Env<'a>>) -> Rc<Env<'a>> {
        Rc::new(Env {
            vars: RefCell::new(Vec::new()),
            parent: Some(parent.clone()),
        })
    }

    pub fn bind(&self, name: &str, v: Value<'a>) {
        self.vars.borrow_mut().push((name.to_string(), v));
    }

    /// Replace the value of the nearest binding of `name` (for `x := ...`).
    pub fn set(&self, name: &str, v: Value<'a>) -> bool {
        let mut env = self;
        loop {
            if let Some(slot) = env.vars.borrow_mut().iter_mut().rev().find(|(n, _)| n == name) {
                slot.1 = v;
                return true;
            }
            match &env.parent {
                Some(p) => env = p,
                None => return false,
            }
        }
    }

    pub fn get(&self, name: &str) -> Option<Value<'a>> {
        let mut env = self;
        loop {
            if let Some((_, v)) = env.vars.borrow().iter().rev().find(|(n, _)| n == name) {
                return Some(v.clone());
            }
            match &env.parent {
                Some(p) => env = p,
                None => return None,
            }
        }
    }
}

/// The type and constructor name of a variant value. Shared, so building a
/// value does not allocate names.
#[derive(Debug)]
pub struct CtorTag {
    pub ty: Rc<str>,
    pub name: Rc<str>,
}

/// A trait method used as a function value.
#[derive(Debug)]
pub struct TraitRef {
    pub trait_name: Rc<str>,
    pub method: Rc<str>,
}

/// A tree-walker closure.
pub struct ClosureVal<'a> {
    pub params: &'a [Param],
    pub body: &'a Expr,
    pub env: Rc<Env<'a>>,
}

thread_local! {
    static TAGS: RefCell<Vec<Rc<CtorTag>>> = const { RefCell::new(Vec::new()) };
}

/// The shared tag for a constructor.
pub fn tag_of(ty: &str, name: &str) -> Rc<CtorTag> {
    TAGS.with(|t| {
        let mut t = t.borrow_mut();
        if let Some(x) = t.iter().find(|x| &*x.name == name && &*x.ty == ty) {
            return x.clone();
        }
        let x = Rc::new(CtorTag {
            ty: Rc::from(ty),
            name: Rc::from(name),
        });
        t.push(x.clone());
        x
    })
}

/// A function chunk plus the values it captured when the closure was made.
pub struct FunVal<'a> {
    pub func: usize,
    pub captures: Vec<Value<'a>>,
}

/// What a capability grants.
#[derive(Debug)]
pub enum CapVal {
    Console,
    Dir { root: std::path::PathBuf, read_only: bool },
    Clock,
    Env,
}

#[derive(Clone)]
pub enum Value<'a> {
    Int(i128, IntTy),
    /// The flag is true for `f32`.
    Float(f64, bool),
    Str(Rc<str>),
    Bool(bool),
    #[allow(dead_code)]
    Char(char),
    Unit,
    /// A list is a shared vector and a start offset, so taking the tail is O(1).
    List(Rc<Vec<Value<'a>>>, usize),
    Tuple(Rc<Vec<Value<'a>>>),
    Ctor {
        tag: Rc<CtorTag>,
        fields: Rc<Vec<Value<'a>>>,
    },
    /// Fields in declaration order.
    Record {
        ty: Rc<str>,
        fields: Rc<Vec<(Rc<str>, Value<'a>)>>,
    },
    Closure(Rc<ClosureVal<'a>>),
    /// A top-level function by name.
    FnItem(Rc<str>),
    /// A constructor used as a function.
    CtorFn {
        tag: Rc<CtorTag>,
        arity: usize,
    },
    /// A trait method, dispatched on its first argument.
    TraitMethod(Rc<TraitRef>),
    Native(&'static str),
    /// A compiled function or closure (bytecode VM).
    Fun(Rc<FunVal<'a>>),
    /// An immutable map, sorted by key.
    Map(Rc<Vec<(Value<'a>, Value<'a>)>>),
    /// An immutable set, sorted.
    Set(Rc<Vec<Value<'a>>>),
    /// A growable array, changed in place (shared by reference).
    Array(Rc<RefCell<Vec<Value<'a>>>>),
    /// A capability token.
    Cap(Rc<CapVal>),
    Ref(Rc<Value<'a>>),
    Allocator,
}

impl<'a> Value<'a> {
    pub fn str(s: &str) -> Value<'a> {
        Value::Str(Rc::from(s))
    }

    pub fn list(items: Vec<Value<'a>>) -> Value<'a> {
        Value::List(Rc::new(items), 0)
    }

    pub fn ctor(ty: &str, name: &str, fields: Vec<Value<'a>>) -> Value<'a> {
        Value::Ctor {
            tag: tag_of(ty, name),
            fields: Rc::new(fields),
        }
    }

    pub fn trait_method(trait_name: &str, method: &str) -> Value<'a> {
        Value::TraitMethod(Rc::new(TraitRef {
            trait_name: Rc::from(trait_name),
            method: Rc::from(method),
        }))
    }

    pub fn some(v: Value<'a>) -> Value<'a> {
        Value::ctor("Option", "Some", vec![v])
    }

    pub fn none() -> Value<'a> {
        Value::ctor("Option", "None", vec![])
    }

    pub fn ordering(o: std::cmp::Ordering) -> Value<'a> {
        let name = match o {
            std::cmp::Ordering::Less => "Less",
            std::cmp::Ordering::Equal => "Equal",
            std::cmp::Ordering::Greater => "Greater",
        };
        Value::ctor("Ordering", name, vec![])
    }

    pub fn list_items(&self) -> Option<&[Value<'a>]> {
        match self {
            Value::List(v, off) => Some(&v[(*off).min(v.len())..]),
            Value::Ref(v) => v.list_items(),
            _ => None,
        }
    }

    /// The type name used to pick a trait implementation.
    pub fn head(&self) -> String {
        match self {
            Value::Int(_, t) => t.name().to_string(),
            Value::Float(_, f32) => if *f32 { "f32" } else { "f64" }.to_string(),
            Value::Str(_) => "String".to_string(),
            Value::Bool(_) => "bool".to_string(),
            Value::Char(_) => "char".to_string(),
            Value::Unit => "tuple0".to_string(),
            Value::List(..) => "[]".to_string(),
            Value::Tuple(v) => format!("tuple{}", v.len()),
            Value::Ctor { tag, .. } => tag.ty.to_string(),
            Value::Record { ty, .. } => ty.to_string(),
            Value::Map(_) => "Map".to_string(),
            Value::Set(_) => "Set".to_string(),
            Value::Array(_) => "Array".to_string(),
            Value::Cap(_) => "cap".to_string(),
            Value::Ref(v) => v.head(),
            _ => "fn".to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeError {
    pub message: String,
    pub span: Option<Span>,
}

impl RuntimeError {
    pub fn new(span: Option<Span>, message: impl Into<String>) -> RuntimeError {
        RuntimeError {
            message: message.into(),
            span,
        }
    }
}

/// Non-local control flow.
pub enum Ctl<'a> {
    /// `?` hit an `Err` or `None`: return this value from the enclosing function.
    Return(Value<'a>),
    Error(RuntimeError),
    /// `process.exit`.
    Exit(i32),
    /// `break` / `continue` unwinding to the nearest loop.
    Break,
    Continue,
}

pub type Res<'a, T> = Result<T, Ctl<'a>>;

pub fn rt_err<'a, T>(span: Span, msg: impl Into<String>) -> Res<'a, T> {
    Err(Ctl::Error(RuntimeError::new(Some(span), msg)))
}
