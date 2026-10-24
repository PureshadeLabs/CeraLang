//! Keeps the book honest: every ```cera block must parse with the real parser.
//!
//! Fence tags:
//!   cera                 must parse (as a whole program, or else as the body of a function)
//!   cera,compile_fail    must parse; it is meant to fail *later* checks (not written yet)
//!   cera,syntax_error    must NOT parse
//!   cera,ignore          not checked (design previews)

use std::fs;
use std::path::{Path, PathBuf};

use cera_syntax::parse;

fn book_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../book/src")
}

struct Snippet {
    file: String,
    line: usize,
    tags: Vec<String>,
    code: String,
}

fn snippets(file: &Path) -> Vec<Snippet> {
    let text = fs::read_to_string(file).unwrap();
    let name = file.file_name().unwrap().to_string_lossy().into_owned();
    let mut out = Vec::new();
    let mut lines = text.lines().enumerate();
    while let Some((i, line)) = lines.next() {
        let Some(info) = line.trim_start().strip_prefix("```") else {
            continue;
        };
        let info = info.trim();
        let mut tags = info.split(',').map(|s| s.trim().to_string());
        let lang = tags.next().unwrap_or_default();
        let tags: Vec<String> = tags.collect();
        let mut code = String::new();
        for (_, l) in lines.by_ref() {
            if l.trim_start().starts_with("```") {
                break;
            }
            code.push_str(l);
            code.push('\n');
        }
        if lang == "cera" {
            out.push(Snippet {
                file: name.clone(),
                line: i + 1,
                tags,
                code,
            });
        }
    }
    out
}

fn parses(code: &str) -> Result<(), String> {
    let direct = parse(code);
    if direct.is_ok() {
        return Ok(());
    }
    // statements and expressions are fine too: wrap them in a function body
    let wrapped: String = format!(
        "__snippet :: fn() do\n{}\nend\n",
        code.lines()
            .map(|l| format!("  {l}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    if parse(&wrapped).is_ok() {
        return Ok(());
    }
    let first = &direct.diagnostics[0];
    let line = code[..first.span.start.min(code.len())]
        .matches('\n')
        .count()
        + 1;
    Err(format!("snippet line {line}: {}", first.message))
}

fn markdown_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(book_src())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    files.sort();
    files
}

#[test]
fn every_cera_block_in_the_book_parses() {
    let mut failures = Vec::new();
    let mut checked = 0;
    for file in markdown_files() {
        for s in snippets(&file) {
            let ignore = s.tags.iter().any(|t| t == "ignore");
            let expect_syntax_error = s.tags.iter().any(|t| t == "syntax_error");
            if ignore {
                continue;
            }
            checked += 1;
            match (parses(&s.code), expect_syntax_error) {
                (Ok(()), false) | (Err(_), true) => {}
                (Err(e), false) => failures.push(format!("{}:{}: {e}", s.file, s.line)),
                (Ok(()), true) => failures.push(format!(
                    "{}:{}: expected a syntax error but it parsed",
                    s.file, s.line
                )),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {checked} book snippets failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(checked > 40, "suspiciously few snippets checked: {checked}");
}

#[test]
fn summary_lists_every_chapter_and_all_links_resolve() {
    let src = book_src();
    let summary = fs::read_to_string(src.join("SUMMARY.md")).unwrap();

    let mut listed = Vec::new();
    for part in summary.split("](").skip(1) {
        if let Some(end) = part.find(')') {
            listed.push(part[..end].to_string());
        }
    }
    for l in &listed {
        assert!(src.join(l).exists(), "SUMMARY.md links to missing file {l}");
    }
    for file in markdown_files() {
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        if name == "SUMMARY.md" {
            continue;
        }
        assert!(listed.contains(&name), "{name} is not listed in SUMMARY.md");
    }

    // every relative .md link inside a chapter must exist
    for file in markdown_files() {
        let text = fs::read_to_string(&file).unwrap();
        for part in text.split("](").skip(1) {
            let Some(end) = part.find(')') else { continue };
            let target = &part[..end];
            if target.starts_with("http") || !target.contains(".md") {
                continue;
            }
            let path = target.split('#').next().unwrap();
            assert!(
                src.join(path).exists(),
                "{}: broken link to {target}",
                file.display()
            );
        }
    }
}

#[test]
fn examples_directory_parses() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "cera") {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let ok = parse(&fs::read_to_string(&path).unwrap()).is_ok();
            // broken.cera is the deliberate error example used in the docs
            assert_eq!(ok, name != "broken.cera", "{name}");
        }
    }
}
