//! Name resolution, type inference and exhaustiveness checking for Cera.
//!
//! ```
//! let parsed = cera_syntax::parse("f :: fn(x: i32) -> i32 do\n  x + 1\nend\n");
//! let analysis = cera_sema::analyze(&parsed.program);
//! assert!(analysis.diagnostics.is_empty());
//! ```

mod check;
pub mod defs;
pub mod exhaust;
mod region;
pub mod ty;

pub use check::{analyze, stub_functions, Analysis, TypeAt, PRELUDE_LIST};

use cera_syntax::Diagnostic;

/// Parse and analyze a source file. Returns the syntax errors if there are any
/// (semantic analysis only runs on syntactically clean files).
pub fn analyze_source(src: &str) -> Result<Analysis, Vec<Diagnostic>> {
    let parsed = cera_syntax::parse(src);
    if !parsed.is_ok() {
        return Err(parsed.diagnostics);
    }
    Ok(analyze(&parsed.program))
}
