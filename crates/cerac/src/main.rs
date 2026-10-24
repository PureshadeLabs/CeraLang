//! `cerac`: the Cera compiler driver.
//!
//! Right now it can tokenize, parse and check files. Type checking, region
//! checking and code generation come next (see docs/spec.md, section 7).

use std::process::ExitCode;

/// Like `println!`, but exits quietly if stdout is closed (e.g. `| head`).
macro_rules! out {
    ($($arg:tt)*) => {{
        use std::io::Write;
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    }};
}

/// Like `print!`, but exits quietly if stdout is closed.
macro_rules! outn {
    ($($arg:tt)*) => {{
        use std::io::Write;
        if write!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    }};
}

use cera_syntax::lexer::lex;
use cera_syntax::{parse, render_diagnostic, Diagnostic};

const USAGE: &str = "\
cerac: the Cera compiler

USAGE:
    cerac <COMMAND> <FILE>

COMMANDS:
    check <FILE>    Parse and type-check FILE and report errors
    run [--tree|--vm|--native] <FILE> [-- ARGS...]
                    Type-check and run FILE (its `main` function) on the
                    bytecode VM (default), the tree-walking interpreter, or a
                    native build (falls back to the VM if unsupported)
    build <FILE> [-o OUT] [--target x86_64-linux|x86_64-lythos]
                    Compile FILE to a native x86-64 executable for Linux
                    (default) or Lythos userspace
    fmt [--check] <FILE>...
                    Format FILEs in place (`-` reads stdin and writes stdout);
                    with --check, list files that would change and exit 1
    dis <FILE>      Print the bytecode FILE compiles to
    types <FILE>    Print the inferred type of every binding in FILE
    parse <FILE>    Parse FILE and print the syntax tree
    lex <FILE>      Print the tokens of FILE
    version         Print the version
    help            Print this message

Use `-` as FILE to read from standard input.
";

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // `cerac run --tree FILE`: pick the engine for this run.
    if args.first().is_some_and(|c| c == "run") {
        match args.get(1).map(String::as_str) {
            Some("--tree") => {
                std::env::set_var("CERA_ENGINE", "tree");
                args.remove(1);
            }
            Some("--native") => {
                std::env::set_var("CERA_ENGINE", "native");
                args.remove(1);
            }
            Some("--vm") => {
                std::env::set_var("CERA_ENGINE", "vm");
                args.remove(1);
            }
            _ => {}
        }
    }
    // `cerac build --target x86_64-lythos FILE -o OUT`: options may come before the file.
    if args.first().is_some_and(|c| c == "build") {
        let at = (1..args.len()).find(|&i| !args[i].starts_with('-') && !matches!(args[i - 1].as_str(), "-o" | "--target"));
        if let Some(i) = at {
            let file = args.remove(i);
            args.insert(1, file);
        }
    }
    match args.first().map(String::as_str) {
        None | Some("help") | Some("-h") | Some("--help") => {
            outn!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some("version") | Some("-V") | Some("--version") => {
            out!("cerac {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("fmt") => fmt_command(&args[1..]),
        Some(cmd @ ("check" | "parse" | "lex" | "types" | "run" | "dis" | "build")) => match args.get(1) {
            Some(file) => run(cmd, file, &args[2..]),
            None => {
                eprintln!("error: `cerac {cmd}` needs a file\n\n{USAGE}");
                ExitCode::from(2)
            }
        },
        Some(other) => {
            eprintln!("error: unknown command `{other}`\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn read_source(file: &str) -> std::io::Result<String> {
    if file == "-" {
        use std::io::Read;
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        Ok(s)
    } else {
        std::fs::read_to_string(file)
    }
}

fn report(file: &str, src: &str, diags: &[Diagnostic]) {
    for d in diags {
        eprintln!("{}", render_diagnostic(file, src, d));
    }
    let n = diags.len();
    eprintln!("{n} error{} in {file}", if n == 1 { "" } else { "s" },);
}

fn run(cmd: &str, file: &str, rest: &[String]) -> ExitCode {
    let src = match read_source(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {file}: {e}");
            return ExitCode::from(2);
        }
    };

    // Programs that use raw memory or system calls (`std.mem`, `std.sys`) only run natively.
    let raw = src.contains("import std.mem") || src.contains("import std.sys");
    if cmd == "run" && (raw || std::env::var("CERA_ENGINE").is_ok_and(|e| e == "native")) {
        if let Some(code) = run_native(file, &src, &rest) {
            return code;
        }
    }
    match cmd {
        "build" => {
            let parsed = parse(&src);
            if !parsed.is_ok() {
                report(file, &src, &parsed.diagnostics);
                return ExitCode::FAILURE;
            }
            let an = cera_sema::analyze(&parsed.program);
            if !an.diagnostics.is_empty() {
                report(file, &src, &an.diagnostics);
                return ExitCode::FAILURE;
            }
            let out = match rest.iter().position(|a| a == "-o") {
                Some(i) => rest.get(i + 1).cloned().unwrap_or_else(|| "a.out".into()),
                None => std::path::Path::new(file).file_stem().map_or("a.out".to_string(), |s| s.to_string_lossy().into_owned()),
            };
            let lythos = match rest.iter().position(|a| a == "--target").and_then(|i| rest.get(i + 1)).map(String::as_str) {
                None | Some("x86_64-linux") => false,
                Some("x86_64-lythos") => true,
                Some(other) => {
                    eprintln!("error: unknown target `{other}` (expected x86_64-linux or x86_64-lythos)");
                    return ExitCode::FAILURE;
                }
            };
            let renderer: std::rc::Rc<dyn Fn(&cera_syntax::Diagnostic) -> String> = {
                let (f, t) = (file.to_string(), src.to_string());
                std::rc::Rc::new(move |d| cera_syntax::render_diagnostic(&f, &t, d))
            };
            match cera_native::compile_for(lythos, &parsed.program, &an, Some(renderer), Some(vec![(file.to_string(), 0, src.to_string())])) {
                Ok(bytes) => {
                    if let Err(e) = std::fs::write(&out, bytes) {
                        eprintln!("error: cannot write {out}: {e}");
                        return ExitCode::FAILURE;
                    }
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o755));
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        "run" => match {
            let prog_args: Vec<String> = rest.iter().skip_while(|a| a.as_str() == "--").cloned().collect();
            cera_eval::run_with(&src, cera_eval::RunConfig::stdio(prog_args))
        } {
            Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
            Err(cera_eval::RunError::Syntax(d)) | Err(cera_eval::RunError::Type(d)) => {
                report(file, &src, &d);
                ExitCode::FAILURE
            }
            Err(cera_eval::RunError::Runtime(e)) => {
                match e.span.filter(|s| !s.is_empty()) {
                    Some(span) => eprintln!(
                        "{}",
                        render_diagnostic(file, &src, &runtime_diag(span, &e.message))
                    ),
                    None => eprintln!("runtime error: {}", e.message),
                }
                ExitCode::FAILURE
            }
        },
        "dis" => match cera_eval::disassemble(&src) {
            Ok(listing) => {
                outn!("{listing}");
                ExitCode::SUCCESS
            }
            Err(cera_eval::RunError::Syntax(d)) | Err(cera_eval::RunError::Type(d)) => {
                report(file, &src, &d);
                ExitCode::FAILURE
            }
            Err(cera_eval::RunError::Runtime(e)) => {
                eprintln!("error: {}", e.message);
                ExitCode::FAILURE
            }
        },
        "lex" => {
            let (toks, diags) = lex(&src);
            for t in &toks {
                out!("{:>5}..{:<5} {:?}", t.span.start, t.span.end, t.kind);
            }
            if diags.is_empty() {
                ExitCode::SUCCESS
            } else {
                report(file, &src, &diags);
                ExitCode::FAILURE
            }
        }
        "parse" | "check" | "types" => {
            let parsed = parse(&src);
            if cmd == "parse" {
                out!("{:#?}", parsed.program);
            }
            if !parsed.is_ok() {
                report(file, &src, &parsed.diagnostics);
                return ExitCode::FAILURE;
            }
            if cmd == "parse" {
                return ExitCode::SUCCESS;
            }
            let analysis = cera_sema::analyze(&parsed.program);
            if cmd == "types" {
                print_types(&src, &analysis);
            }
            if !analysis.diagnostics.is_empty() {
                report(file, &src, &analysis.diagnostics);
                return ExitCode::FAILURE;
            }
            if cmd == "check" {
                out!("{file}: ok");
            }
            ExitCode::SUCCESS
        }
        _ => unreachable!(),
    }
}

/// Print `line:col  text` for every binding and declaration (not every expression).
fn print_types(src: &str, analysis: &cera_sema::Analysis) {
    let index = cera_syntax::LineIndex::new(src);
    let mut rows: Vec<_> = analysis
        .types
        .iter()
        .filter(|t| t.text.contains(": ") && !src[t.span.start..t.span.end].contains(char::is_whitespace))
        .collect();
    rows.sort_by_key(|t| t.span.start);
    rows.dedup_by_key(|t| (t.span.start, t.text.clone()));
    for t in rows {
        let (line, col) = index.line_col(src, t.span.start);
        out!("{:>4}:{:<3} {}", line + 1, col + 1, t.text);
    }
}

/// Compile to a temporary native executable and run it. Returns `None` when the
/// native backend cannot handle the program (the caller falls back to the VM).
fn run_native(file: &str, src: &str, rest: &[String]) -> Option<ExitCode> {
    use std::sync::OnceLock;
    static RESULT: OnceLock<Option<u8>> = OnceLock::new();
    let code = RESULT.get_or_init(|| {
        let parsed = parse(src);
        if !parsed.is_ok() {
            report(file, src, &parsed.diagnostics);
            return Some(1);
        }
        let an = cera_sema::analyze(&parsed.program);
        if !an.diagnostics.is_empty() {
            report(file, src, &an.diagnostics);
            return Some(1);
        }
        let bytes = cera_native::compile_with(&parsed.program, &an, Some((file, src))).ok()?;
        let path = std::env::temp_dir().join(format!("cera-run-{}", std::process::id()));
        std::fs::write(&path, bytes).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
        }
        let prog_args: Vec<String> = rest.iter().skip_while(|a| a.as_str() == "--").cloned().collect();
        let status = std::process::Command::new(&path).args(prog_args).status();
        let _ = std::fs::remove_file(&path);
        Some(status.ok().and_then(|s| s.code()).unwrap_or(1).clamp(0, 255) as u8)
    });
    code.map(ExitCode::from)
}

fn runtime_diag(span: cera_syntax::Span, msg: &str) -> Diagnostic {
    let mut d = Diagnostic::new(span, format!("runtime error: {msg}"));
    if let Some(h) = cera_syntax::runtime_help(msg) {
        d = d.with_help(h);
    }
    d
}

fn fmt_command(args: &[String]) -> ExitCode {
    let check = args.iter().any(|a| a == "--check");
    let files: Vec<&String> = args.iter().filter(|a| *a != "--check").collect();
    if files.is_empty() {
        eprintln!("error: `cerac fmt` needs at least one file\n\n{USAGE}");
        return ExitCode::from(2);
    }
    let mut code = ExitCode::SUCCESS;
    for file in files {
        let src = match read_source(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: cannot read {file}: {e}");
                code = ExitCode::FAILURE;
                continue;
            }
        };
        match cera_syntax::format_source(&src) {
            Ok(formatted) => {
                if file == "-" {
                    print!("{formatted}");
                } else if formatted != src {
                    if check {
                        eprintln!("would reformat {file}");
                        code = ExitCode::FAILURE;
                    } else if let Err(e) = std::fs::write(file, formatted) {
                        eprintln!("error: cannot write {file}: {e}");
                        code = ExitCode::FAILURE;
                    } else {
                        eprintln!("formatted {file}");
                    }
                }
            }
            Err(e) => {
                eprintln!("error: {file}:{e}");
                code = ExitCode::FAILURE;
            }
        }
    }
    code
}
