//! `ceralings` command-line interface.

use std::path::PathBuf;
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

use ceralings::{check, discover, hint, report, Exercise, NOT_DONE_MARKER};

const USAGE: &str = "\
ceralings: small exercises to get you used to reading and writing Cera

USAGE:
    ceralings [--dir <PATH>] <COMMAND>

COMMANDS:
    list            Show every exercise and your progress
    run [NAME]      Check an exercise (default: the first unfinished one)
    hint <NAME>     Show the hint for an exercise
    verify          Check every exercise in order, stopping at the first unfinished one
    help            Print this message

An exercise is finished when the `# I AM NOT DONE` comment is removed and the
file has no errors. Open the file, fix it, remove the marker, run again.

The exercises are looked up in ./ceralings/exercises or ./exercises. Use
--dir to point somewhere else.
";

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    let mut dir: Option<PathBuf> = None;
    if let Some(i) = args.iter().position(|a| a == "--dir") {
        if i + 1 >= args.len() {
            eprintln!("error: --dir needs a path");
            return ExitCode::from(2);
        }
        dir = Some(PathBuf::from(args.remove(i + 1)));
        args.remove(i);
    }

    let command = args.first().map(String::as_str).unwrap_or("help");
    if matches!(command, "help" | "-h" | "--help") {
        outn!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if matches!(command, "version" | "-V" | "--version") {
        out!("ceralings {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    let dir = match dir.or_else(find_default_dir) {
        Some(d) => d,
        None => {
            eprintln!(
                "error: cannot find the exercises. Run ceralings from the repository root, or pass --dir <PATH>."
            );
            return ExitCode::from(2);
        }
    };
    let exercises = match discover(&dir) {
        Ok(e) if !e.is_empty() => e,
        Ok(_) => {
            eprintln!("error: no exercises found in {}", dir.display());
            return ExitCode::from(2);
        }
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", dir.display());
            return ExitCode::from(2);
        }
    };

    match command {
        "list" => list(&exercises),
        "run" => run(&exercises, args.get(1).map(String::as_str)),
        "hint" => match args.get(1) {
            Some(name) => show_hint(&exercises, name),
            None => {
                eprintln!("error: `ceralings hint` needs an exercise name");
                ExitCode::from(2)
            }
        },
        "verify" => verify(&exercises),
        other => {
            eprintln!("error: unknown command `{other}`\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn find_default_dir() -> Option<PathBuf> {
    ["ceralings/exercises", "exercises"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_dir())
}

fn is_done(ex: &Exercise) -> bool {
    check(ex).map(|(_, c)| c.is_done()).unwrap_or(false)
}

fn list(exercises: &[Exercise]) -> ExitCode {
    let width = exercises.iter().map(|e| e.name.len()).max().unwrap_or(0);
    let mut done = 0;
    for ex in exercises {
        let d = is_done(ex);
        if d {
            done += 1;
        }
        out!(
            "[{}] {:<width$}  {}",
            if d { "x" } else { " " },
            ex.name,
            ex.topic,
            width = width
        );
    }
    out!("\nProgress: {done}/{} done", exercises.len());
    ExitCode::SUCCESS
}

fn find<'a>(exercises: &'a [Exercise], name: &str) -> Option<&'a Exercise> {
    exercises.iter().find(|e| e.name == name)
}

fn run(exercises: &[Exercise], name: Option<&str>) -> ExitCode {
    let ex = match name {
        Some(n) => match find(exercises, n) {
            Some(e) => e,
            None => {
                eprintln!("error: no exercise named `{n}`. Try `ceralings list`.");
                return ExitCode::from(2);
            }
        },
        None => match exercises.iter().find(|e| !is_done(e)) {
            Some(e) => e,
            None => {
                out!("All {} exercises are done. Well done!", exercises.len());
                return ExitCode::SUCCESS;
            }
        },
    };
    check_one(exercises, ex)
}

fn check_one(exercises: &[Exercise], ex: &Exercise) -> ExitCode {
    let (source, c) = match check(ex) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", ex.path.display());
            return ExitCode::from(2);
        }
    };
    let path = ex.path.display();

    if !c.diagnostics.is_empty() || c.run_problem.is_some() {
        out!("Exercise `{}` is not right yet.\n", ex.name);
        outn!("{}", report(ex, &source, &c));
        out!("Fix the errors in {path}");
        out!("Stuck? Run `ceralings hint {}`.", ex.name);
        return ExitCode::FAILURE;
    }
    if c.marker_present {
        out!("Exercise `{}` compiles.", ex.name);
        out!(
            "\nWhen you are happy with it, remove the `# {NOT_DONE_MARKER}` line from\n{path}\nand run `ceralings run` again."
        );
        return ExitCode::FAILURE;
    }

    out!("Done: {}", ex.name);
    match exercises.iter().find(|e| !is_done(e)) {
        Some(next) => out!("Next up: {} ({})", next.name, next.rel_path.display()),
        None => out!("That was the last one. All exercises are done!"),
    }
    ExitCode::SUCCESS
}

fn show_hint(exercises: &[Exercise], name: &str) -> ExitCode {
    let Some(ex) = find(exercises, name) else {
        eprintln!("error: no exercise named `{name}`. Try `ceralings list`.");
        return ExitCode::from(2);
    };
    match std::fs::read_to_string(&ex.path) {
        Ok(src) => match hint(&src) {
            Some(h) => {
                out!("{h}");
                ExitCode::SUCCESS
            }
            None => {
                out!("There is no hint for `{name}`.");
                ExitCode::SUCCESS
            }
        },
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", ex.path.display());
            ExitCode::from(2)
        }
    }
}

fn verify(exercises: &[Exercise]) -> ExitCode {
    for ex in exercises {
        if !is_done(ex) {
            out!("Stopped at `{}`.\n", ex.name);
            return check_one(exercises, ex);
        }
    }
    out!("All {} exercises are done.", exercises.len());
    ExitCode::SUCCESS
}
