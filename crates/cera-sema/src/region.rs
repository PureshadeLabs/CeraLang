//! Region escape analysis (conservative, syntactic).
//!
//! A `region do ... end` frees everything allocated inside it at `end`. Its
//! value therefore may not be a heap value that lives in the region. We flag the
//! tail expression when its type allocates and it either builds a fresh value
//! (list, tuple, record, lambda) or mentions a binding made inside the region,
//! unless it goes through `promote(..)` or `new_in(..)`.

use crate::ty::{is_primitive, Ty};
use cera_syntax::ast::*;
use std::collections::HashSet;

/// Whether a value of this type lives in a region (needs allocation).
pub fn is_heap(t: &Ty) -> bool {
    match t {
        Ty::List(_) | Ty::Ref(_) | Ty::Fn(..) => true,
        Ty::Tuple(ts) => ts.iter().any(is_heap),
        Ty::Con(name, _) => !is_primitive(name),
        Ty::Param(_) | Ty::Var(_) | Ty::Error => false,
    }
}

/// Names bound directly in a block with `name = ...` or `var name = ...`.
pub fn locals(b: &Block) -> HashSet<String> {
    b.stmts
        .iter()
        .filter_map(|s| match s {
            Stmt::Let { name, .. } => Some(name.name.clone()),
            _ => None,
        })
        .collect()
}

fn is_escape_call(callee: &Expr) -> bool {
    matches!(&callee.kind, ExprKind::Var(n) if n == "promote" || n == "new_in")
}

/// Whether `e` may evaluate to a value that lives in the region owning `locals`.
pub fn lives_in_region(e: &Expr, locals: &HashSet<String>) -> bool {
    use ExprKind::*;
    match &e.kind {
        Var(n) => locals.contains(n),
        List(_) | Cons { .. } | Tuple(_) | Record { .. } | Lambda { .. } => true,
        Update { .. } => true,
        Call { callee, args } => {
            if is_escape_call(callee) {
                return false;
            }
            args.iter().any(|a| lives_in_region(a, locals))
        }
        Pipe { lhs, rhs } => {
            if let Call { callee, .. } = &rhs.kind {
                if is_escape_call(callee) {
                    return false;
                }
            }
            if matches!(&rhs.kind, Var(n) if n == "promote") {
                return false;
            }
            lives_in_region(lhs, locals) || lives_in_region(rhs, locals)
        }
        Field { expr, .. } | Index { expr, .. } | Try(expr) => lives_in_region(expr, locals),
        If { then, els, .. } => {
            block_tail(then).is_some_and(|t| lives_in_region(t, locals))
                || els.as_ref().and_then(block_tail).is_some_and(|t| lives_in_region(t, locals))
        }
        Match { arms, .. } => arms.iter().any(|a| lives_in_region(&a.body, locals)),
        _ => false,
    }
}

pub fn block_tail(b: &Block) -> Option<&Expr> {
    match b.stmts.last() {
        Some(Stmt::Expr(e)) => Some(e),
        _ => None,
    }
}
