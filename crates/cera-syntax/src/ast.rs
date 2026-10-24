//! The Cera abstract syntax tree.

use crate::span::Span;

#[derive(Clone, Debug, PartialEq)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Import(Import),
    Decl(Decl),
    /// `impl Show for Shape do ... end`
    Impl(ImplDef),
}

/// `T: Ord + Show` in a generic parameter list.
#[derive(Clone, Debug, PartialEq)]
pub struct Bound {
    pub param: Ident,
    pub traits: Vec<Ident>,
}

/// `Show :: trait ... end`: a set of method signatures.
#[derive(Clone, Debug, PartialEq)]
pub struct TraitDef {
    pub methods: Vec<TraitMethod>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TraitMethod {
    pub name: Ident,
    /// The first parameter is `self` (no type annotation).
    pub params: Vec<Param>,
    pub ret: Option<Type>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImplDef {
    pub generics: Vec<Ident>,
    pub bounds: Vec<Bound>,
    pub trait_name: Ident,
    pub ty: Type,
    /// Function declarations; bodiless only in the standard-library stubs.
    pub methods: Vec<Decl>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Import {
    pub path: Vec<Ident>,
    pub alias: Option<Ident>,
    pub span: Span,
}

/// A top-level `name :: ...` declaration.
#[derive(Clone, Debug, PartialEq)]
pub struct Decl {
    /// Written `pub name :: ...`: other modules may use it.
    pub public: bool,
    pub name: Ident,
    /// Type parameters written on the name: `Option<T> :: type ...`.
    pub generics: Vec<Ident>,
    pub kind: DeclKind,
    /// Written `Name :: repr type { ... }`: a record with a fixed C-style memory layout.
    pub repr: bool,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DeclKind {
    Fn(FnDef),
    Type(TypeDef),
    Trait(TraitDef),
    Const(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub struct FnDef {
    /// Written `unsafe fn`: callers need an `unsafe do ... end` block, and the body is unsafe code.
    pub unsafe_: bool,
    /// Type parameters written after `fn`: `fn<A, B>(...)`.
    pub generics: Vec<Ident>,
    /// Trait bounds written on the type parameters: `fn<T: Ord>(...)`.
    pub bounds: Vec<Bound>,
    pub params: Vec<Param>,
    pub ret: Option<Type>,
    /// `None` only for intrinsic declarations in the standard-library stubs.
    pub body: Option<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: Ident,
    /// Required in function declarations, optional in lambdas.
    pub ty: Option<Type>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeDef {
    Variants(Vec<Variant>),
    Record(Vec<Field>),
    /// A type with no visible structure (`Allocator :: type`); intrinsics only.
    Opaque,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    pub name: Ident,
    pub fields: Vec<VariantField>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VariantField {
    pub name: Option<Ident>,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub name: Ident,
    pub ty: Type,
    /// `[T; N]` in a `repr type`: the field holds `N` elements inline (its type is `[T]`).
    pub array: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Type {
    pub kind: TypeKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeKind {
    /// `Foo`, `std.io.Reader`, `Map<K, V>`
    Path { path: Vec<Ident>, args: Vec<Type> },
    /// `fn(A, B) -> C`
    Fn { params: Vec<Type>, ret: Box<Type> },
    /// `[T]`
    List(Box<Type>),
    /// `(A, B)`; the empty tuple is the unit type
    Tuple(Vec<Type>),
    /// `ref T`
    Ref(Box<Type>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Let {
        name: Ident,
        value: Expr,
        /// `var x = ...`: the binding may be assigned with `:=`.
        mutable: bool,
        span: Span,
    },
    /// `x := value`
    Assign {
        name: Ident,
        value: Expr,
        span: Span,
    },
    Defer(Expr, Span),
    Expr(Expr),
}

impl Stmt {
    pub fn span(&self) -> Span {
        match self {
            Stmt::Let { span, .. } | Stmt::Assign { span, .. } => *span,
            Stmt::Defer(_, span) => *span,
            Stmt::Expr(e) => e.span,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    /// `~x`: bitwise not
    BitNot,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    /// `a & b`
    BitAnd,
    /// `a ^ b`
    BitXor,
    /// `a << n`
    Shl,
    /// `a >> n`
    Shr,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    Int(String),
    Float(String),
    Str(String),
    Bool(bool),
    Unit,
    Var(String),
    List(Vec<Expr>),
    /// `[a, b | rest]`: elements prepended to an existing list
    Cons {
        items: Vec<Expr>,
        tail: Box<Expr>,
    },
    Tuple(Vec<Expr>),
    /// `Point{x: 1.0, y: 2.0}`
    Record {
        name: Ident,
        fields: Vec<(Ident, Expr)>,
    },
    /// `{p | x: 3}`: a copy of a record with some fields replaced
    Update {
        base: Box<Expr>,
        fields: Vec<(Ident, Expr)>,
    },
    Unary {
        op: UnOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `lhs |> rhs`. Kept as written; `x |> f(y)` means `f(x, y)`.
    Pipe {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    Field {
        expr: Box<Expr>,
        name: Ident,
    },
    Index {
        expr: Box<Expr>,
        index: Box<Expr>,
    },
    /// Postfix `?`: unwrap an `Ok`/`Some` or return early.
    Try(Box<Expr>),
    Lambda {
        params: Vec<Param>,
        body: Box<Expr>,
    },
    If {
        cond: Box<Expr>,
        then: Block,
        els: Option<Block>,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<Arm>,
    },
    Region(Block),
    While {
        cond: Box<Expr>,
        body: Block,
    },
    /// `for x in list do ... end`
    For {
        var: Ident,
        iter: Box<Expr>,
        body: Block,
    },
    Break,
    Continue,
    With {
        what: Ident,
        arg: Box<Expr>,
        body: Block,
    },
    /// `do ... end` used as an expression.
    Block(Block),
    /// `unsafe do ... end`: raw memory and system calls (`std.mem`, `std.sys`) are allowed inside.
    Unsafe(Block),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Arm {
    pub pat: Pattern,
    pub guard: Option<Expr>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pattern {
    pub kind: PatKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PatKind {
    Wild,
    Bind(String),
    Int(String),
    Float(String),
    Str(String),
    Bool(bool),
    /// `Circle(r)`, or a bare constructor like `Nil` (empty `args`)
    Ctor {
        name: Ident,
        args: Vec<Pattern>,
    },
    /// `Point{x, y: py}`
    Record {
        name: Ident,
        fields: Vec<(Ident, Option<Pattern>)>,
    },
    Tuple(Vec<Pattern>),
    /// `[a, b | rest]`
    List {
        items: Vec<Pattern>,
        tail: Option<Box<Pattern>>,
    },
}

/// Whether a name denotes a type or constructor (starts with an uppercase letter).
pub fn is_upper_name(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}
