//! Project-aware analysis for editors: type-check one file together with the
//! modules it imports, using unsaved buffers, and report per-file results.

use crate::link::{link_with, AliasTarget, Root};
use crate::manifest::Manifest;
use crate::resolve::{self, Options};
use cera_syntax::{Diagnostic, Span};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub enum ImportTarget {
    File(PathBuf),
    Std(String),
}

#[derive(Debug, Default)]
pub struct Analysis {
    /// Every file that was loaded, with the text that was analyzed.
    pub files: Vec<(PathBuf, String)>,
    /// Problems, with spans local to their file.
    pub diagnostics: Vec<(PathBuf, Span, String)>,
    /// `(file, span, type text)` for expressions and bindings.
    pub types: Vec<(PathBuf, Span, String)>,
    /// For each file: alias -> what it imports.
    pub imports: HashMap<PathBuf, Vec<(String, ImportTarget)>>,
}

/// The directory of the package that contains `file`.
pub fn find_project(file: &Path) -> Option<PathBuf> {
    let mut dir = file.parent()?;
    loop {
        if dir.join("kote.toml").is_file() {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

/// Names that can follow `import` in a file of the package at `dir`.
pub fn importable_modules(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(base, &p, out);
            } else if p.extension().is_some_and(|e| e == "cera") {
                let rel = p.strip_prefix(base).unwrap_or(&p).with_extension("");
                let mut parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                if parts.last().is_some_and(|l| l == "mod") {
                    parts.pop();
                }
                let name = parts.join(".");
                if !name.is_empty() && name != "main" && name != "lib" {
                    out.push(name);
                }
            }
        }
    }
    let mut out = vec![];
    walk(&dir.join("src"), &dir.join("src"), &mut out);
    if let Ok(m) = Manifest::load(dir) {
        out.push(m.name);
        out.extend(m.deps.into_iter().map(|d| d.name));
    }
    for s in ["io", "list", "string", "option", "result", "num", "bits", "array", "map", "set", "env", "time", "fs", "mem", "sys", "process"] {
        out.push(format!("std.{s}"));
    }
    out.push("kote.test".into());
    out.sort();
    out.dedup();
    out
}

fn canon(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| match (p.parent().and_then(|d| d.canonicalize().ok()), p.file_name()) {
        (Some(d), Some(n)) => d.join(n),
        _ => p.to_path_buf(),
    })
}

/// Analyze `file` (and what it imports). `overlay` maps paths to unsaved text.
pub fn analyze_file(file: &Path, overlay: &HashMap<PathBuf, String>) -> Result<Analysis, String> {
    let file = canon(file);
    let dir = find_project(&file).ok_or("not inside a kote package")?;
    let dir = canon(&dir);
    let graph = resolve::resolve(&dir, Options { offline: true, dry: true, ..Options::default() })?;
    let overlay: HashMap<PathBuf, String> = overlay.iter().map(|(k, v)| (canon(k), v.clone())).collect();
    let linked = link_with(&graph, Root::File(file.clone()), overlay);
    let mut out = Analysis::default();
    // `map.files[i]` is `modules[i]`.
    let path_of = |i: usize| linked.modules.get(i).and_then(|m| m.file.clone());
    for (i, f) in linked.map.files.iter().enumerate() {
        if let Some(p) = path_of(i) {
            out.files.push((p, f.text.clone()));
        }
    }
    for (i, m) in linked.modules.iter().enumerate() {
        let Some(p) = path_of(i) else { continue };
        let list = m
            .aliases
            .iter()
            .filter_map(|(a, t)| match t {
                AliasTarget::Module(j) => path_of(*j).map(|f| (a.clone(), ImportTarget::File(f))),
                AliasTarget::Std(n) => Some((a.clone(), ImportTarget::Std(n.clone()))),
            })
            .collect();
        out.imports.insert(p, list);
    }
    let locate = |span: Span| -> Option<(PathBuf, Span, usize)> {
        let (i, f) = linked.map.files.iter().enumerate().rev().find(|(_, f)| f.base <= span.start && span.start <= f.base + f.text.len())?;
        let p = path_of(i)?;
        Some((p, Span::new(span.start - f.base, span.end.saturating_sub(f.base)), i))
    };
    let describe = |d: &Diagnostic| {
        let mut m = linked.map.clean(&d.message);
        if let Some(l) = &d.label {
            m.push_str(&format!(" ({})", linked.map.clean(l)));
        }
        for n in &d.notes {
            m.push_str(&format!("\nnote: {}", linked.map.clean(n)));
        }
        for h in &d.help {
            m.push_str(&format!("\nhelp: {}", linked.map.clean(h)));
        }
        m
    };
    let mut push = |ds: &[Diagnostic]| {
        for d in ds {
            match locate(d.span) {
                Some((p, sp, _)) => out.diagnostics.push((p, sp, describe(d))),
                None => out.diagnostics.push((file.clone(), Span::new(0, 0), describe(d))),
            }
        }
    };
    if !linked.diagnostics.is_empty() {
        push(&linked.diagnostics);
        return Ok(out);
    }
    let an = cera_sema::analyze(&linked.program);
    push(&an.diagnostics);
    for t in &an.types {
        if let Some((p, sp, _)) = locate(t.span) {
            out.types.push((p, sp, linked.map.clean(&t.text)));
        }
    }
    Ok(out)
}
