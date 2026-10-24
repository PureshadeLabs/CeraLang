//! ceralings: small broken programs you fix one at a time.
//!
//! An exercise is a `.cera` file. It counts as **done** when
//!
//! 1. it no longer contains the marker comment `# I AM NOT DONE`, and
//! 2. it parses without errors.
//!
//! Once the Cera interpreter exists, step 2 grows into "it runs and prints what
//! the `# expect:` lines say". Until then the check is syntax-level.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use cera_syntax::{parse, render_diagnostic, Diagnostic};

pub const NOT_DONE_MARKER: &str = "I AM NOT DONE";

#[derive(Clone, Debug)]
pub struct Exercise {
    /// File stem, e.g. `intro1`. Unique across all exercises.
    pub name: String,
    /// Parent directory name, e.g. `00_intro`.
    pub topic: String,
    /// Path relative to the exercises directory, e.g. `00_intro/intro1.cera`.
    pub rel_path: PathBuf,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct Check {
    pub marker_present: bool,
    /// Syntax and type errors.
    pub diagnostics: Vec<Diagnostic>,
    /// Set when the program compiles but its output differs from the `# expect:` lines,
    /// or it fails at run time.
    pub run_problem: Option<String>,
}

impl Check {
    pub fn is_done(&self) -> bool {
        !self.marker_present && self.diagnostics.is_empty() && self.run_problem.is_none()
    }
}

/// Find every `.cera` file under `dir`, sorted by relative path.
pub fn discover(dir: &Path) -> io::Result<Vec<Exercise>> {
    let mut files = Vec::new();
    collect(dir, &mut files)?;
    files.sort();
    let mut out: Vec<Exercise> = Vec::new();
    for path in files {
        let rel = path.strip_prefix(dir).unwrap_or(&path).to_path_buf();
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if out.iter().any(|e| e.name == name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("two exercises are named `{name}`"),
            ));
        }
        let topic = rel
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push(Exercise {
            name,
            topic,
            rel_path: rel,
            path,
        });
    }
    Ok(out)
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "cera") {
            out.push(path);
        }
    }
    Ok(())
}

/// Whether the source still has the `# I AM NOT DONE` marker.
pub fn has_marker(source: &str) -> bool {
    source.lines().any(|l| {
        let l = l.trim();
        l.starts_with('#') && l.to_ascii_uppercase().contains(NOT_DONE_MARKER)
    })
}

pub fn check_source(source: &str) -> Check {
    let parsed = parse(source);
    let mut diagnostics = parsed.diagnostics.clone();
    if parsed.is_ok() {
        diagnostics = cera_sema::analyze(&parsed.program).diagnostics;
    }
    let mut run_problem = None;
    if diagnostics.is_empty() {
        if let Some(expected) = expected_output(source) {
            run_problem = match cera_eval::run_captured(source) {
                Ok(actual) if actual.trim_end() == expected.trim_end() => None,
                Ok(actual) => Some(format!(
                    "the program ran, but printed the wrong thing.\nexpected:\n{}\nactual:\n{}",
                    expected.trim_end(),
                    actual.trim_end()
                )),
                Err(cera_eval::RunError::Runtime(e)) => Some(format!("runtime error: {}", e.message)),
                Err(_) => None,
            };
        }
    }
    Check {
        marker_present: has_marker(source),
        diagnostics,
        run_problem,
    }
}

pub fn check(exercise: &Exercise) -> io::Result<(String, Check)> {
    let source = fs::read_to_string(&exercise.path)?;
    let result = check_source(&source);
    Ok((source, result))
}

/// The lines after a `# HINT` line, with the comment markers removed.
pub fn hint(source: &str) -> Option<String> {
    let mut lines = source.lines();
    for l in lines.by_ref() {
        if l.trim().eq_ignore_ascii_case("# hint") {
            let text: Vec<String> = lines
                .take_while(|l| l.trim_start().starts_with('#'))
                .map(|l| {
                    let l = l.trim_start().trim_start_matches('#');
                    l.strip_prefix(' ').unwrap_or(l).to_string()
                })
                .collect();
            return Some(text.join("\n"));
        }
    }
    None
}

/// The program output promised by `# expect:` lines, if any.
pub fn expected_output(source: &str) -> Option<String> {
    let lines: Vec<&str> = source
        .lines()
        .filter_map(|l| l.trim().strip_prefix("# expect:"))
        .map(|l| l.strip_prefix(' ').unwrap_or(l))
        .collect();
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

/// Compiler-style report for the problems in an exercise.
pub fn report(exercise: &Exercise, source: &str, check: &Check) -> String {
    let mut out = String::new();
    let file = exercise.rel_path.to_string_lossy();
    for d in check.diagnostics.iter().take(3) {
        out.push_str(&render_diagnostic(&file, source, d));
        out.push('\n');
    }
    if check.diagnostics.len() > 3 {
        out.push_str(&format!("... and {} more\n", check.diagnostics.len() - 3));
    }
    if let Some(p) = &check.run_problem {
        out.push_str(p);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_detection() {
        assert!(has_marker("# I AM NOT DONE\nmain :: fn() do\nend\n"));
        assert!(has_marker("  #   i am not done  \n"));
        assert!(!has_marker("main :: fn() do\nend\n"));
        // only comments count
        assert!(!has_marker("x = \"I AM NOT DONE\"\n"));
    }

    #[test]
    fn done_needs_no_marker_and_valid_syntax() {
        assert!(check_source("main :: fn() do\nend\n").is_done());
        assert!(!check_source("# I AM NOT DONE\nmain :: fn() do\nend\n").is_done());
        assert!(!check_source("main :: fn() do\n").is_done());
    }

    #[test]
    fn hints_and_expectations() {
        let src = "# expect: Hello\n# expect: World\nmain :: fn() do\nend\n\n# HINT\n# first line\n# second line\n\nunrelated\n";
        assert_eq!(hint(src).unwrap(), "first line\nsecond line");
        assert_eq!(expected_output(src).unwrap(), "Hello\nWorld");
        assert!(hint("main :: fn() do\nend\n").is_none());
        assert!(expected_output("main :: fn() do\nend\n").is_none());
    }

    fn exercises_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ceralings/exercises")
    }

    fn solutions_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ceralings/solutions")
    }

    #[test]
    fn exercises_are_unsolved_and_have_hints() {
        let exercises = discover(&exercises_dir()).unwrap();
        assert!(exercises.len() >= 20, "found {}", exercises.len());
        for ex in &exercises {
            let (src, c) = check(ex).unwrap();
            assert!(c.marker_present, "{} has no marker", ex.name);
            assert!(
                !c.diagnostics.is_empty(),
                "{} already parses; an exercise should be broken",
                ex.name
            );
            assert!(hint(&src).is_some(), "{} has no hint", ex.name);
        }
    }

    #[test]
    fn every_exercise_has_a_working_solution() {
        for ex in discover(&exercises_dir()).unwrap() {
            let solution = solutions_dir().join(&ex.rel_path);
            let src = fs::read_to_string(&solution)
                .unwrap_or_else(|_| panic!("missing solution for {}", ex.name));
            let c = check_source(&src);
            assert!(
                c.is_done(),
                "solution for {} is not clean: marker={} diagnostics={:?}",
                ex.name,
                c.marker_present,
                c.diagnostics
            );
            assert!(c.run_problem.is_none(), "solution for {}: {:?}", ex.name, c.run_problem);
        }
    }

    #[test]
    fn exercises_referenced_by_the_book_exist() {
        let names: Vec<String> = discover(&exercises_dir())
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        let book = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../book/src");
        for entry in fs::read_dir(book).unwrap() {
            let path = entry.unwrap().path();
            if !path.extension().is_some_and(|e| e == "md") {
                continue;
            }
            let text = fs::read_to_string(&path).unwrap();
            let mut in_exercises = false;
            for line in text.lines() {
                if line.starts_with("## Exercises") {
                    in_exercises = true;
                    continue;
                }
                if line.starts_with('#') {
                    in_exercises = false;
                }
                if !in_exercises {
                    continue;
                }
                // names appear in backticks: `values1`, `ceralings run intro1`
                for part in line.split('`').skip(1).step_by(2) {
                    let word = part.split_whitespace().last().unwrap_or("");
                    let word = word.trim_end_matches('.');
                    if word.chars().last().is_some_and(|c| c.is_ascii_digit()) {
                        assert!(
                            names.iter().any(|n| n == word),
                            "{} references missing exercise `{word}`",
                            path.display()
                        );
                    }
                }
            }
        }
    }
}
