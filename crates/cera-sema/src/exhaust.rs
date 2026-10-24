//! Match exhaustiveness (Maranget's usefulness algorithm, witness form).

use crate::defs::{TDef, TypeInfo};
use crate::ty::short_name;
use std::collections::HashMap;

/// A constructor of a pattern, after type checking.
#[derive(Clone, Debug, PartialEq)]
pub enum Con {
    Variant { ty: String, index: usize },
    Tuple(usize),
    /// A record pattern, with all fields in declaration order.
    Record(String, usize),
    Nil,
    Cons,
    Bool(bool),
    /// An integer, float or string literal; never a complete signature.
    Lit(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum DPat {
    Wild,
    Ctor(Con, Vec<DPat>),
}

pub struct Ctx<'t> {
    pub types: &'t HashMap<String, TypeInfo>,
}

impl Ctx<'_> {
    fn arity(&self, c: &Con) -> usize {
        match c {
            Con::Variant { ty, index } => match self.types.get(ty).map(|t| &t.def) {
                Some(TDef::Variants(vs)) => vs.get(*index).map_or(0, |v| v.fields.len()),
                _ => 0,
            },
            Con::Tuple(n) | Con::Record(_, n) => *n,
            Con::Nil | Con::Bool(_) | Con::Lit(_) => 0,
            Con::Cons => 2,
        }
    }

    /// Every constructor of the type `c` belongs to, or `None` if the set is open.
    fn siblings(&self, c: &Con) -> Option<Vec<Con>> {
        match c {
            Con::Variant { ty, .. } => match self.types.get(ty).map(|t| &t.def) {
                Some(TDef::Variants(vs)) => Some(
                    (0..vs.len())
                        .map(|index| Con::Variant {
                            ty: ty.clone(),
                            index,
                        })
                        .collect(),
                ),
                _ => None,
            },
            Con::Tuple(_) | Con::Record(..) => Some(vec![c.clone()]),
            Con::Nil | Con::Cons => Some(vec![Con::Nil, Con::Cons]),
            Con::Bool(_) => Some(vec![Con::Bool(true), Con::Bool(false)]),
            Con::Lit(_) => None,
        }
    }

    fn specialize(&self, rows: &[Vec<DPat>], c: &Con) -> Vec<Vec<DPat>> {
        let arity = self.arity(c);
        let mut out = Vec::new();
        for row in rows {
            match &row[0] {
                DPat::Ctor(c2, args) if c2 == c => {
                    let mut r = args.clone();
                    r.extend_from_slice(&row[1..]);
                    out.push(r);
                }
                DPat::Ctor(..) => {}
                DPat::Wild => {
                    let mut r = vec![DPat::Wild; arity];
                    r.extend_from_slice(&row[1..]);
                    out.push(r);
                }
            }
        }
        out
    }

    /// A vector of `n` patterns that no row matches, or `None` if the rows are exhaustive.
    pub fn witness(&self, rows: &[Vec<DPat>], n: usize) -> Option<Vec<DPat>> {
        if n == 0 {
            return if rows.is_empty() { Some(vec![]) } else { None };
        }
        let mut heads: Vec<Con> = Vec::new();
        for row in rows {
            if let DPat::Ctor(c, _) = &row[0] {
                if !heads.contains(c) {
                    heads.push(c.clone());
                }
            }
        }
        let sibs = heads.first().and_then(|h| self.siblings(h));
        let complete = sibs
            .as_ref()
            .is_some_and(|s| s.iter().all(|c| heads.contains(c)));

        if complete {
            for c in sibs.unwrap() {
                let arity = self.arity(&c);
                let spec = self.specialize(rows, &c);
                if let Some(mut w) = self.witness(&spec, arity + n - 1) {
                    let rest = w.split_off(arity);
                    let mut out = vec![DPat::Ctor(c, w)];
                    out.extend(rest);
                    return Some(out);
                }
            }
            return None;
        }

        let default: Vec<Vec<DPat>> = rows
            .iter()
            .filter(|r| r[0] == DPat::Wild)
            .map(|r| r[1..].to_vec())
            .collect();
        let rest = self.witness(&default, n - 1)?;
        let first = match sibs {
            Some(s) => match s.into_iter().find(|c| !heads.contains(c)) {
                Some(c) => {
                    let arity = self.arity(&c);
                    DPat::Ctor(c, vec![DPat::Wild; arity])
                }
                None => DPat::Wild,
            },
            None => DPat::Wild,
        };
        let mut out = vec![first];
        out.extend(rest);
        Some(out)
    }

    pub fn show(&self, p: &DPat) -> String {
        match p {
            DPat::Wild => "_".to_string(),
            DPat::Ctor(c, args) => match c {
                Con::Variant { ty, index } => {
                    let name = match self.types.get(ty).map(|t| &t.def) {
                        Some(TDef::Variants(vs)) => vs[*index].name.clone(),
                        _ => short_name(ty).to_string(),
                    };
                    if args.is_empty() {
                        name
                    } else {
                        format!("{name}({})", self.list(args))
                    }
                }
                Con::Tuple(_) => format!("({})", self.list(args)),
                Con::Record(name, _) => format!("{}{{..}}", short_name(name)),
                Con::Nil => "[]".to_string(),
                Con::Cons => format!("[{} | {}]", self.show(&args[0]), self.show(&args[1])),
                Con::Bool(b) => b.to_string(),
                Con::Lit(s) => s.clone(),
            },
        }
    }

    fn list(&self, ps: &[DPat]) -> String {
        ps.iter().map(|p| self.show(p)).collect::<Vec<_>>().join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs::VariantInfo;
    use crate::ty::Ty;

    fn shape_types() -> HashMap<String, TypeInfo> {
        let mut m = HashMap::new();
        m.insert(
            "Shape".to_string(),
            TypeInfo {
                name: "Shape".into(),
                generics: vec![],
                def: TDef::Variants(vec![
                    VariantInfo {
                        name: "Circle".into(),
                        fields: vec![Ty::prim("f64")],
                    },
                    VariantInfo {
                        name: "Rect".into(),
                        fields: vec![Ty::prim("f64"), Ty::prim("f64")],
                    },
                ]),
            },
        );
        m
    }

    fn v(index: usize, args: Vec<DPat>) -> DPat {
        DPat::Ctor(
            Con::Variant {
                ty: "Shape".into(),
                index,
            },
            args,
        )
    }

    #[test]
    fn missing_variant_is_reported() {
        let types = shape_types();
        let cx = Ctx { types: &types };
        let rows = vec![vec![v(0, vec![DPat::Wild])]];
        let w = cx.witness(&rows, 1).unwrap();
        assert_eq!(cx.show(&w[0]), "Rect(_, _)");
    }

    #[test]
    fn all_variants_is_exhaustive() {
        let types = shape_types();
        let cx = Ctx { types: &types };
        let rows = vec![
            vec![v(0, vec![DPat::Wild])],
            vec![v(1, vec![DPat::Wild, DPat::Wild])],
        ];
        assert!(cx.witness(&rows, 1).is_none());
    }

    #[test]
    fn literals_need_a_wildcard() {
        let types = HashMap::new();
        let cx = Ctx { types: &types };
        let rows = vec![vec![DPat::Ctor(Con::Lit("1".into()), vec![])]];
        assert!(cx.witness(&rows, 1).is_some());
        let rows = vec![
            vec![DPat::Ctor(Con::Lit("1".into()), vec![])],
            vec![DPat::Wild],
        ];
        assert!(cx.witness(&rows, 1).is_none());
    }

    #[test]
    fn nested_tuple_of_bools() {
        let types = HashMap::new();
        let cx = Ctx { types: &types };
        let t = |a: bool, b: bool| {
            vec![DPat::Ctor(
                Con::Tuple(2),
                vec![
                    DPat::Ctor(Con::Bool(a), vec![]),
                    DPat::Ctor(Con::Bool(b), vec![]),
                ],
            )]
        };
        let rows = vec![t(true, true), t(false, false), t(true, false)];
        let w = cx.witness(&rows, 1).unwrap();
        assert_eq!(cx.show(&w[0]), "(false, true)");
    }

    #[test]
    fn lists() {
        let types = HashMap::new();
        let cx = Ctx { types: &types };
        let rows = vec![vec![DPat::Ctor(Con::Nil, vec![])]];
        let w = cx.witness(&rows, 1).unwrap();
        assert_eq!(cx.show(&w[0]), "[_ | _]");
    }
}
