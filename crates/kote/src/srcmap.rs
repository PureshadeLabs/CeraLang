//! Several source files laid out in one virtual text, so that spans from different
//! modules never collide once they are merged into a single program.

use cera_syntax::{render_diagnostic, Diagnostic, Span};

#[derive(Clone, Debug)]
pub struct SourceFile {
    /// The name shown in diagnostics.
    pub name: String,
    /// Offset of the file's first byte in the virtual text.
    pub base: usize,
    pub text: String,
}

#[derive(Clone, Debug, Default)]
pub struct SourceMap {
    pub files: Vec<SourceFile>,
    /// `(mangled name, name as the user writes it)`, longest first.
    pub names: Vec<(String, String)>,
}

impl SourceMap {
    /// Register a file and return its base offset.
    pub fn add(&mut self, name: String, text: String) -> usize {
        let base = self.files.last().map_or(0, |f| f.base + f.text.len() + 1);
        self.files.push(SourceFile { name, base, text });
        base
    }

    pub fn file_at(&self, offset: usize) -> Option<&SourceFile> {
        self.files.iter().rev().find(|f| f.base <= offset && offset <= f.base + f.text.len())
    }

    /// Replace linker-mangled names by the names the user wrote.
    pub fn clean(&self, s: &str) -> String {
        let mut out = s.to_string();
        if out.contains(cera_syntax::MODULE_SEP) {
            for (m, d) in &self.names {
                out = out.replace(m.as_str(), d);
            }
        }
        out
    }

    /// Render a diagnostic against whichever file its span points into.
    pub fn render(&self, d: &Diagnostic) -> String {
        let Some(f) = self.file_at(d.span.start) else {
            return format!("error: {}\n", self.clean(&d.message));
        };
        let shift = |s: Span| Span::new(s.start.saturating_sub(f.base), s.end.saturating_sub(f.base));
        let mut local = Diagnostic::new(shift(d.span), self.clean(&d.message));
        local.label = d.label.as_ref().map(|l| self.clean(l));
        local.notes = d.notes.iter().map(|n| self.clean(n)).collect();
        for (sp, l) in &d.secondary {
            match self.file_at(sp.start) {
                Some(g) if g.base == f.base => local.secondary.push((shift(*sp), self.clean(l))),
                Some(g) => {
                    let off = sp.start - g.base;
                    let idx = cera_syntax::LineIndex::new(&g.text);
                    let (line, col) = idx.line_col(&g.text, off);
                    local.notes.push(format!("{l}: {}:{}:{}", g.name, line + 1, col + 1));
                }
                None => {}
            }
        }
        local.help = d.help.iter().map(|h| self.clean(h)).collect();
        render_diagnostic(&f.name, &f.text, &local)
    }
}
