//! Syntax front end for Cera: tokens, lexer, AST and parser.
//!
//! ```
//! let parsed = cera_syntax::parse("main :: fn() do\n  io.println(\"hi\")\nend\n");
//! assert!(parsed.is_ok());
//! assert_eq!(parsed.program.items.len(), 1);
//! ```

pub mod asm;
pub mod ast;
pub mod fmt;
pub mod lexer;
pub mod parser;
pub mod repr;
pub mod span;
pub mod token;

pub use fmt::{format_source, tree_fingerprint, FormatError};
pub use parser::{parse, parse_with, Parse, ParseOptions};
pub use span::{display_name, closest, MODULE_SEP, edit_distance, render_diagnostic, runtime_help, Diagnostic, LineIndex, Span};
