//! WebAssembly bindings for the Cera stage0 front end and interpreter.

use cera_eval::{run_with_engine, Engine, RunConfig, RunError, SharedBuf};
use cera_syntax::{format_source, parse, render_diagnostic, runtime_help, Diagnostic};
use wasm_bindgen::prelude::*;

/// The result of running a program.
#[wasm_bindgen]
pub struct RunResult {
    code: i32,
    out: String,
    err: String,
    ok: bool,
}

#[wasm_bindgen]
impl RunResult {
    /// Process exit code (0 on success; -1 when the program did not run).
    #[wasm_bindgen(getter)]
    pub fn code(&self) -> i32 {
        self.code
    }
    /// Everything the program wrote to stdout.
    #[wasm_bindgen(getter)]
    pub fn out(&self) -> String {
        self.out.clone()
    }
    /// Stderr, or rendered compiler/runtime diagnostics.
    #[wasm_bindgen(getter)]
    pub fn err(&self) -> String {
        self.err.clone()
    }
    /// False when the program failed to parse, type-check or run.
    #[wasm_bindgen(getter)]
    pub fn ok(&self) -> bool {
        self.ok
    }
}

fn render(src: &str, ds: &[Diagnostic]) -> String {
    ds.iter()
        .map(|d| render_diagnostic("main.cera", src, d))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parse, type-check and run `src` with `input` on stdin.
///
/// Output produced before a runtime error is kept in `out`.
#[wasm_bindgen]
pub fn run(src: &str, input: &str) -> RunResult {
    let (out, err) = (SharedBuf::default(), SharedBuf::default());
    let config = RunConfig {
        out: Box::new(out.clone()),
        err: Box::new(err.clone()),
        input: Box::new(std::io::Cursor::new(input.as_bytes().to_vec())),
        args: vec![],
        fs_root: std::path::PathBuf::from("/"),
    };
    let fail = |diag: String| RunResult {
        code: -1,
        out: out.contents(),
        err: format!("{}{}", err.contents(), diag),
        ok: false,
    };
    match run_with_engine(src, config, Engine::Vm) {
        Ok(code) => RunResult { code, out: out.contents(), err: err.contents(), ok: true },
        Err(RunError::Syntax(ds)) | Err(RunError::Type(ds)) => fail(render(src, &ds)),
        Err(RunError::Runtime(e)) => {
            let mut d = Diagnostic::new(e.span.unwrap_or_default(), e.message.clone());
            if let Some(h) = runtime_help(&e.message) {
                d = d.with_help(h);
            }
            fail(render(src, &[d]))
        }
    }
}

/// Type-check without running. Returns rendered diagnostics, empty when clean.
#[wasm_bindgen]
pub fn check(src: &str) -> String {
    let parsed = parse(src);
    if !parsed.is_ok() {
        return render(src, &parsed.diagnostics);
    }
    let analysis = cera_sema::analyze(&parsed.program);
    render(src, &analysis.diagnostics)
}

/// Format `src`. Returns the formatted source, or the error text prefixed by `error: `.
#[wasm_bindgen]
pub fn format(src: &str) -> String {
    match format_source(src) {
        Ok(s) => s,
        Err(e) => format!("error: {e:?}"),
    }
}
