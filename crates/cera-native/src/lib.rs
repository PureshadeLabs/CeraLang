//! Native x86-64 Linux backend.
pub mod elf;
pub mod dwarf;
pub mod backend;
pub mod rt;
pub mod lower;

/// Compile a checked program to a static x86-64 Linux ELF executable.
pub fn compile(program: &cera_syntax::ast::Program, analysis: &cera_sema::Analysis) -> Result<Vec<u8>, String> {
    compile_with(program, analysis, None)
}

/// Like [`compile`], but runtime errors in the produced executable show the
/// source line (`src` is the file name and text of `program`).
pub fn compile_with(
    program: &cera_syntax::ast::Program,
    analysis: &cera_sema::Analysis,
    src: Option<(&str, &str)>,
) -> Result<Vec<u8>, String> {
    let files = src.map(|(f, t)| vec![(f.to_string(), 0usize, t.to_string())]);
    let renderer = src.map(|(f, t)| {
        let (f, t) = (f.to_string(), t.to_string());
        std::rc::Rc::new(move |d: &cera_syntax::Diagnostic| cera_syntax::render_diagnostic(&f, &t, d))
            as std::rc::Rc<dyn Fn(&cera_syntax::Diagnostic) -> String>
    });
    compile_full(program, analysis, renderer, files)
}

/// Like [`compile_full`], for a chosen target: `lythos` builds a Lythos userspace
/// executable (Lythos syscall ABI, loaded above 1 GiB) instead of a Linux one.
pub fn compile_for(
    lythos: bool,
    program: &cera_syntax::ast::Program,
    analysis: &cera_sema::Analysis,
    renderer: Option<std::rc::Rc<dyn Fn(&cera_syntax::Diagnostic) -> String>>,
    files: Option<Vec<(String, usize, String)>>,
) -> Result<Vec<u8>, String> {
    elf::with_target(lythos, || compile_full(program, analysis, renderer, files))
}

/// Like [`compile`], with a function that renders runtime-error locations
/// (used when the program was assembled from several files).
pub fn compile_with_renderer(
    program: &cera_syntax::ast::Program,
    analysis: &cera_sema::Analysis,
    renderer: Option<std::rc::Rc<dyn Fn(&cera_syntax::Diagnostic) -> String>>,
) -> Result<Vec<u8>, String> {
    compile_full(program, analysis, renderer, None)
}

/// Like [`compile_with_renderer`], and also embeds DWARF line tables and function
/// names. `files` are `(name, offset of the file in the program's spans, text)`.
pub fn compile_full(
    program: &cera_syntax::ast::Program,
    analysis: &cera_sema::Analysis,
    renderer: Option<std::rc::Rc<dyn Fn(&cera_syntax::Diagnostic) -> String>>,
    files: Option<Vec<(String, usize, String)>>,
) -> Result<Vec<u8>, String> {
    let std_src = include_str!("native_std.cera");
    let parsed = cera_syntax::parse_with(std_src, cera_syntax::ParseOptions { intrinsics: true });
    if !parsed.is_ok() {
        let d = &parsed.diagnostics[0];
        let line = std_src[..d.span.start.min(std_src.len())].matches('\n').count() + 1;
        return Err(format!("internal error: native std does not parse (line {line}): {}", d.message));
    }
    let std_an = cera_sema::analyze(&parsed.program);
    if !std_an.diagnostics.is_empty() {
        let d = &std_an.diagnostics[0];
        let line = std_src[..d.span.start.min(std_src.len())].matches('\n').count() + 1;
        return Err(format!("internal error: native std does not type-check (line {line}): {}", d.message));
    }
    let mut c = lower::Compiler::new(program, analysis, &parsed.program, &std_an);
    c.renderer = renderer;
    if let Some(files) = files {
        c.be.debug = Some(files.into_iter().map(|(name, base, text)| dwarf::SrcFile { name, base, text }).collect());
    }
    c.compile()
}

/// Parse, check and compile source text.
pub fn compile_source(src: &str) -> Result<Vec<u8>, String> {
    let parsed = cera_syntax::parse(src);
    if !parsed.is_ok() {
        return Err(parsed.diagnostics.iter().map(|d| d.message.clone()).collect::<Vec<_>>().join("\n"));
    }
    let an = cera_sema::analyze(&parsed.program);
    if !an.diagnostics.is_empty() {
        return Err(an.diagnostics.iter().map(|d| d.message.clone()).collect::<Vec<_>>().join("\n"));
    }
    compile_with(&parsed.program, &an, Some(("main.cera", src)))
}
pub(crate) mod glue;
pub(crate) mod builtins;
