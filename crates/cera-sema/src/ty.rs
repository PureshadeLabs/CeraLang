//! Semantic types.

/// A semantic type. Nominal types carry a unique id (`Shape`, `core::Option`).
#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Con(String, Vec<Ty>),
    Fn(Vec<Ty>, Box<Ty>),
    List(Box<Ty>),
    Tuple(Vec<Ty>),
    Ref(Box<Ty>),
    /// A rigid, user-declared type parameter.
    Param(String),
    /// An inference variable.
    Var(u32),
    /// A type that already produced an error; unifies with anything.
    Error,
}

pub const INTS: &[&str] = &[
    "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "isize", "usize",
];
pub const FLOATS: &[&str] = &["f32", "f64"];

impl Ty {
    pub fn prim(name: &str) -> Ty {
        Ty::Con(name.to_string(), vec![])
    }
    pub fn unit() -> Ty {
        Ty::Tuple(vec![])
    }
    pub fn bool() -> Ty {
        Ty::prim("bool")
    }
    pub fn string() -> Ty {
        Ty::prim("String")
    }
}

pub fn is_primitive(name: &str) -> bool {
    INTS.contains(&name) || FLOATS.contains(&name) || matches!(name, "bool" | "char" | "String")
}

/// The display name of a type id: `core::Option` becomes `Option`.
pub fn short_name(id: &str) -> &str {
    id.rsplit("::").next().unwrap_or(id)
}
