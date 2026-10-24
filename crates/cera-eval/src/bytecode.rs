//! The bytecode the compiler produces and the VM runs.

use crate::value::Value;
use cera_syntax::ast::BinOp;
use cera_syntax::Span;
use std::collections::HashMap;
use std::rc::Rc;

/// One instruction. Jump targets are absolute indices into the chunk.
#[derive(Clone, Copy, Debug)]
pub enum Op {
    /// Push a constant from the constant pool.
    Const(usize),
    Unit,
    Pop,
    /// Swap the two values on top of the stack.
    Swap,
    Local(usize),
    SetLocal(usize),
    Capture(usize),
    /// Push a global constant (evaluated on first use).
    Global(usize),
    MakeList(usize),
    MakeTuple(usize),
    /// `n` items and then the tail list.
    MakeCons(usize),
    /// Fields are on the stack in source order; the descriptor says how to order them.
    MakeRecord(usize),
    /// Pops the new values and a record; pushes a copy with the fields in `records[d].names` replaced.
    UpdateRecord(usize),
    Neg,
    Not,
    BitNot,
    Arith(BinOp),
    Cmp(BinOp),
    /// Compare and jump when the comparison is false.
    CmpJump(BinOp, usize),
    /// `x + k`, `x - k`, `x * k` with an integer constant on the right.
    ArithK(BinOp, usize),
    /// Compare with an integer constant and jump when false.
    CmpJumpK(BinOp, u32, u32),
    /// `local op k`, with the local read in place.
    LocalArithK(BinOp, u32, u32),
    /// Compare a local with an integer constant and jump when false: `(op, slot, const, target)`.
    LocalCmpJumpK(BinOp, u32, u32, u32),
    ReturnLocal(u32),
    /// Call a known top-level function. A placeholder sits where the callee would be.
    CallFn(u32, u32),
    TailCallFn(u32, u32),
    Eq,
    NotEq,
    ExpectBool,
    /// `and`: if the popped left side is false, push false and jump.
    AndJump(usize),
    /// `or`: if the popped left side is true, push true and jump.
    OrJump(usize),
    /// Field access; the operand indexes the name pool.
    Field(usize),
    Index,
    /// `?`: unwrap `Some`/`Ok` or return the `None`/`Err`.
    Try,
    Call(usize),
    TailCall(usize),
    Return,
    /// Make a closure from a chunk and `ncap` captured values on the stack.
    Closure { func: u32, ncap: u32 },
    Jump(usize),
    /// Pop a bool; jump when false.
    JumpIfFalse(usize),
    /// Test the value in a local against a pattern, binding into local slots.
    MatchPat { pat: u32, slot: u32, fail: u32 },
    MatchFail,
    /// Pop a closure and register it to run when the enclosing block ends.
    PushDefer,
    /// Run the last `n` registered defers.
    RunDefers(usize),
    /// Remember the current stack height in a local (for `break` / `continue`).
    SaveSp(usize),
    /// Cut the stack back to the height saved in a local, then jump.
    JumpSp(usize, usize),
    /// `for`: when the list in `list` (starting at the counter in `idx`) is used up,
    /// jump to `exit`; otherwise store the next element in `var`.
    ForNext { list: u32, idx: u32, var: u32, exit: u32 },
    /// Raise a runtime error (the operand indexes the message pool).
    Fail(usize),
}

/// A compiled function body.
#[derive(Debug)]
pub struct Chunk {
    pub name: String,
    pub arity: usize,
    pub nlocals: usize,
    pub code: Vec<Op>,
    pub spans: Vec<Span>,
}

/// A pattern with its bindings resolved to local slots.
#[derive(Debug)]
pub enum Pat {
    Wild,
    Bind(usize),
    Int(i128),
    Float(f64),
    Str(Rc<str>),
    Bool(bool),
    Ctor { name: Rc<str>, args: Vec<Pat> },
    Record { ty: Rc<str>, fields: Vec<(Rc<str>, Pat)> },
    Tuple(Vec<Pat>),
    List { items: Vec<Pat>, tail: Option<Box<Pat>> },
}

/// How to build a record from fields evaluated in source order.
#[derive(Debug)]
pub struct RecordDesc {
    pub ty: Rc<str>,
    /// Field names in declaration order.
    pub names: Vec<Rc<str>>,
    /// For each declared field, the index among the evaluated fields.
    pub perm: Vec<usize>,
}

pub struct Bytecode {
    pub chunks: Vec<Chunk>,
    pub consts: Vec<Value<'static>>,
    pub pats: Vec<Pat>,
    pub records: Vec<RecordDesc>,
    pub messages: Vec<String>,
    pub names: Vec<Rc<str>>,
    /// Thunk chunk for each global constant.
    pub globals: Vec<usize>,
    pub fns: HashMap<String, usize>,
    /// `(trait, type head)` to method name to chunk.
    pub impls: HashMap<(String, String), HashMap<String, usize>>,
}

impl Bytecode {
    /// A readable listing, for `cerac` debugging and tests.
    pub fn disassemble(&self) -> String {
        let mut out = String::new();
        for (i, c) in self.chunks.iter().enumerate() {
            out.push_str(&format!("chunk {i} `{}` arity={} locals={}\n", c.name, c.arity, c.nlocals));
            for (j, op) in c.code.iter().enumerate() {
                out.push_str(&format!("  {j:>4}  {op:?}\n"));
            }
        }
        out
    }
}
