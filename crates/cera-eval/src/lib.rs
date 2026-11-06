//! A tree-walking interpreter for Cera.
//!
//! It runs programs that `cera-sema` accepts. Regions and allocators have no
//! runtime effect here (memory is reference counted); they matter for the
//! compiled backends. Tail calls do not grow the stack.
//!
//! ```
//! let out = cera_eval::run_captured("import std.io\nmain :: fn() do\n  io.println(1 + 2)\nend\n").unwrap();
//! assert_eq!(out, "3\n");
//! ```

mod bytecode;
mod compile;
mod eval;
mod native;
mod rt;
mod value;
mod vm;

pub use native::native_names;
pub use rt::RunConfig;
pub use value::RuntimeError;

use cera_syntax::{parse, Diagnostic};
use std::io::Write;
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub enum RunError {
    Syntax(Vec<Diagnostic>),
    Type(Vec<Diagnostic>),
    Runtime(RuntimeError),
}

/// A clonable, thread-safe output buffer.
#[derive(Clone, Default)]
pub struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl SharedBuf {
    pub fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}


/// Which execution engine runs the program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// The tree-walking interpreter.
    Tree,
    /// The bytecode compiler and virtual machine.
    Vm,
}

impl Engine {
    /// The engine named by the `CERA_ENGINE` environment variable (`tree` or
    /// `vm`), or the default.
    pub fn from_env() -> Engine {
        match std::env::var("CERA_ENGINE").as_deref() {
            Ok("tree") => Engine::Tree,
            Ok("vm") => Engine::Vm,
            _ => Engine::Vm,
        }
    }
}

/// Parse, type-check and run `src` with the given environment, on the engine
/// chosen by `CERA_ENGINE` (the VM by default). Returns the exit code.
pub fn run_with(src: &str, config: RunConfig) -> Result<i32, RunError> {
    run_with_engine(src, config, Engine::from_env())
}

/// The bytecode listing of a program (for debugging).
pub fn disassemble(src: &str) -> Result<String, RunError> {
    let parsed = parse(src);
    if !parsed.is_ok() {
        return Err(RunError::Syntax(parsed.diagnostics));
    }
    let analysis = cera_sema::analyze(&parsed.program);
    if !analysis.diagnostics.is_empty() {
        return Err(RunError::Type(analysis.diagnostics));
    }
    Ok(compile::compile(&parsed.program, &analysis).disassemble())
}

pub fn run_with_engine(src: &str, config: RunConfig, engine: Engine) -> Result<i32, RunError> {
    let parsed = parse(src);
    if !parsed.is_ok() {
        return Err(RunError::Syntax(parsed.diagnostics));
    }
    let analysis = cera_sema::analyze(&parsed.program);
    if !analysis.diagnostics.is_empty() {
        return Err(RunError::Type(analysis.diagnostics));
    }
    run_program(&parsed.program, &analysis, config, engine)
}

/// Run an already parsed and checked program (for example one the package
/// manager assembled from several files).
pub fn run_program(
    program: &cera_syntax::ast::Program,
    analysis: &cera_sema::Analysis,
    config: RunConfig,
    engine: Engine,
) -> Result<i32, RunError> {
    #[cfg(target_arch = "wasm32")]
    {
        // No threads on wasm: run on the caller's stack.
        return match engine {
            Engine::Tree => eval::Interp::new(program, analysis, config.into_host()).run_main(),
            Engine::Vm => {
                let bc = compile::compile(program, analysis);
                vm::Vm::new(bc, config.into_host()).run_main()
            }
        }
        .map_err(RunError::Runtime);
    }
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(eval::STACK_BYTES)
            .spawn_scoped(s, move || match engine {
                Engine::Tree => eval::Interp::new(program, analysis, config.into_host()).run_main(),
                Engine::Vm => {
                    let bc = compile::compile(program, analysis);
                    vm::Vm::new(bc, config.into_host()).run_main()
                }
            })
            .expect("cannot start the interpreter thread")
            .join()
            .unwrap_or_else(|_| Err(RuntimeError::new(None, "the interpreter crashed")))
    })
    .map_err(RunError::Runtime)
}

/// Run `src`, writing program output to `out`, with no input and no arguments.
pub fn run_source(src: &str, out: Box<dyn Write + Send>) -> Result<i32, RunError> {
    run_with(
        src,
        RunConfig {
            out,
            err: Box::new(std::io::sink()),
            input: Box::new(std::io::Cursor::new(Vec::new())),
            args: vec![],
            fs_root: std::env::current_dir().unwrap_or_default(),
        },
    )
}

/// What a captured run produced.
#[derive(Debug, Clone)]
pub struct Captured {
    pub code: i32,
    pub out: String,
    pub err: String,
}

/// Run `src` with scripted input, arguments and filesystem root, capturing output.
pub fn run_captured_with(
    src: &str,
    input: &str,
    args: Vec<String>,
    fs_root: std::path::PathBuf,
) -> Result<Captured, RunError> {
    run_captured_on(Engine::from_env(), src, input, args, fs_root)
}

/// Like `run_captured_with`, on a chosen engine.
pub fn run_captured_on(
    engine: Engine,
    src: &str,
    input: &str,
    args: Vec<String>,
    fs_root: std::path::PathBuf,
) -> Result<Captured, RunError> {
    let (out, err) = (SharedBuf::default(), SharedBuf::default());
    let code = run_with_engine(
        src,
        RunConfig {
            out: Box::new(out.clone()),
            err: Box::new(err.clone()),
            input: Box::new(std::io::Cursor::new(input.as_bytes().to_vec())),
            args,
            fs_root,
        },
        engine,
    )?;
    Ok(Captured {
        code,
        out: out.contents(),
        err: err.contents(),
    })
}

/// Run `src` and return everything it printed.
pub fn run_captured(src: &str) -> Result<String, RunError> {
    let buf = SharedBuf::default();
    run_source(src, Box::new(buf.clone()))?;
    Ok(buf.contents())
}
