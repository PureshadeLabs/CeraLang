//! Declarations collected from a module: types, constructors, functions.

use crate::ty::Ty;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Scheme {
    pub generics: Vec<String>,
    /// `(generic name, trait id)` pairs that every use must satisfy.
    pub bounds: Vec<(String, String)>,
    pub ty: Ty,
    /// Declared `unsafe fn`: callable only inside `unsafe do ... end` or another `unsafe fn`.
    pub unsafe_: bool,
}

#[derive(Clone, Debug)]
pub struct TraitInfo {
    pub name: String,
    /// Method name and type; the implementing type appears as `Param("Self")`.
    pub methods: Vec<(String, Ty)>,
}

#[derive(Clone, Debug)]
pub struct ImplInfo {
    pub trait_id: String,
    pub generics: Vec<String>,
    pub bounds: Vec<(String, String)>,
    pub self_ty: Ty,
}

#[derive(Clone, Debug)]
pub struct VariantInfo {
    pub name: String,
    pub fields: Vec<Ty>,
}

#[derive(Clone, Debug)]
pub enum TDef {
    Variants(Vec<VariantInfo>),
    Record(Vec<(String, Ty)>),
    Opaque,
}

#[derive(Clone, Debug)]
pub struct TypeInfo {
    pub name: String,
    pub generics: Vec<String>,
    pub def: TDef,
}

#[derive(Clone, Debug)]
pub struct CtorInfo {
    pub type_id: String,
    pub index: usize,
    pub generics: Vec<String>,
    pub fields: Vec<Ty>,
}

#[derive(Clone, Debug)]
pub enum Value {
    Fn(Scheme),
    Const(usize),
    Ctor(CtorInfo),
}

/// The names a module makes visible.
#[derive(Clone, Debug, Default)]
pub struct Exports {
    /// type name -> type id
    pub types: HashMap<String, String>,
    /// trait name -> trait id
    pub traits: HashMap<String, String>,
    pub values: HashMap<String, Value>,
}
