//! Source spans, diagnostics and line/column bookkeeping.

/// A half-open byte range `[start, end)` into the source text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Span {
        Span { start, end }
    }

    /// The smallest span covering both `self` and `other`.
    pub fn to(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    pub fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    pub fn contains(self, offset: usize) -> bool {
        self.start <= offset && offset < self.end
    }
}

/// A problem found while lexing, parsing or checking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub span: Span,
    pub message: String,
    /// Text shown next to the primary underline.
    pub label: Option<String>,
    /// Further spans, underlined with `-` and a short text.
    pub secondary: Vec<(Span, String)>,
    pub notes: Vec<String>,
    pub help: Vec<String>,
}

impl Diagnostic {
    pub fn new(span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            span,
            message: message.into(),
            label: None,
            secondary: vec![],
            notes: vec![],
            help: vec![],
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Diagnostic {
        self.label = Some(label.into());
        self
    }

    pub fn with_secondary(mut self, span: Span, label: impl Into<String>) -> Diagnostic {
        self.secondary.push((span, label.into()));
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Diagnostic {
        self.notes.push(note.into());
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Diagnostic {
        self.help.push(help.into());
        self
    }
}

/// The separator the module linker puts between a module path and a name.
pub const MODULE_SEP: char = '\u{b7}';

/// A name as the user wrote it: strips the module prefix the linker added.
pub fn display_name(name: &str) -> &str {
    name.rsplit(MODULE_SEP).next().unwrap_or(name)
}

/// Edit distance between two names (for "did you mean" suggestions).
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i];
        for j in 1..=b.len() {
            let sub = prev[j - 1] + usize::from(a[i - 1] != b[j - 1]);
            cur.push(sub.min(prev[j] + 1).min(cur[j - 1] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// The candidate closest to `name`, if it is close enough to be a plausible typo.
pub fn closest<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let n = name.chars().count();
    if n < 3 {
        return None;
    }
    let limit = n.div_ceil(3);
    candidates
        .into_iter()
        .filter(|c| *c != name && !c.starts_with('_'))
        .map(|c| (edit_distance(name, c), c))
        .filter(|(d, _)| *d <= limit)
        .min_by_key(|(d, c)| (*d, c.len()))
        .map(|(_, c)| c.to_string())
}

/// Advice for a runtime error message (shared by the VM and native backends).
pub fn runtime_help(msg: &str) -> Option<&'static str> {
    if msg.starts_with("integer overflow in `") {
        Some("use a wider integer type such as `i64`, or check the operands before computing")
    } else if msg == "division by zero" {
        Some("check that the divisor is not zero before dividing")
    } else if msg == "no match arm matched" {
        Some("add a wildcard arm `_ -> ...` so every value is handled")
    } else {
        None
    }
}

/// Maps byte offsets to (line, column) pairs and back.
///
/// Lines and columns are zero-based. Columns count UTF-16 code units, which is
/// what the Language Server Protocol uses.
#[derive(Clone, Debug)]
pub struct LineIndex {
    line_starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(text: &str) -> LineIndex {
        let mut line_starts = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        LineIndex { line_starts }
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Byte offset of the start of `line` (clamped to the last line).
    pub fn line_start(&self, line: usize) -> usize {
        self.line_starts[line.min(self.line_starts.len() - 1)]
    }

    /// Zero-based line containing `offset`.
    pub fn line_of(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        }
    }

    /// Zero-based `(line, utf16_column)` for a byte offset.
    pub fn line_col(&self, text: &str, offset: usize) -> (usize, usize) {
        let offset = offset.min(text.len());
        let line = self.line_of(offset);
        let start = self.line_starts[line];
        let col = utf16_len(&text[start..floor_char_boundary(text, offset)]);
        (line, col)
    }

    /// Byte offset for a zero-based `(line, utf16_column)`.
    pub fn offset(&self, text: &str, line: usize, col: usize) -> usize {
        if line >= self.line_starts.len() {
            return text.len();
        }
        let start = self.line_starts[line];
        let end = if line + 1 < self.line_starts.len() {
            self.line_starts[line + 1]
        } else {
            text.len()
        };
        let mut units = 0;
        for (i, ch) in text[start..end].char_indices() {
            if units >= col || ch == '\n' {
                return start + i;
            }
            units += ch.len_utf16();
        }
        end
    }
}

fn utf16_len(s: &str) -> usize {
    s.chars().map(|c| c.len_utf16()).sum()
}

fn floor_char_boundary(text: &str, mut i: usize) -> usize {
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Render a diagnostic in a compiler-style format with a source excerpt.
pub fn render_diagnostic(file: &str, text: &str, diag: &Diagnostic) -> String {
    let index = LineIndex::new(text);
    let (line, col) = index.line_col(text, diag.span.start);

    // (line, col, width, mark, label) for the primary span and the secondary ones.
    let mut marks: Vec<(usize, usize, usize, char, Option<String>)> = vec![];
    let line_text_of = |l: usize| -> String {
        let start = index.line_start(l);
        let end = text[start..].find('\n').map(|n| start + n).unwrap_or(text.len());
        text[start..end].trim_end_matches('\r').to_string()
    };
    let width_of = |l: usize, c: usize, len: usize| len.min(line_text_of(l).len().saturating_sub(c)).max(1);
    marks.push((line, col, width_of(line, col, diag.span.len()), '^', diag.label.clone()));
    for (sp, label) in &diag.secondary {
        let (l, c) = index.line_col(text, sp.start);
        if (l, c) != (line, col) {
            marks.push((l, c, width_of(l, c, sp.len()), '-', Some(label.clone())));
        }
    }
    marks.sort_by_key(|m| (m.0, m.1));

    let last_line = marks.iter().map(|m| m.0).max().unwrap_or(line);
    let pad = " ".repeat((last_line + 1).to_string().len());
    let mut out = format!(
        "error: {msg}\n{pad}--> {file}:{line}:{col}\n{pad} |\n",
        msg = diag.message,
        line = line + 1,
        col = col + 1,
    );
    let mut prev_line: Option<usize> = None;
    for (l, c, w, ch, label) in &marks {
        if prev_line != Some(*l) {
            if let Some(p) = prev_line {
                if *l > p + 1 {
                    out.push_str("...\n");
                }
            }
            out.push_str(&format!("{:>width$} | {}\n", l + 1, line_text_of(*l), width = pad.len()));
        }
        let mut under = format!("{pad} | {}{}", " ".repeat(*c), ch.to_string().repeat(*w));
        if let Some(label) = label {
            under.push(' ');
            under.push_str(label);
        }
        out.push_str(&under);
        out.push('\n');
        prev_line = Some(*l);
    }
    if !diag.notes.is_empty() || !diag.help.is_empty() {
        out.push_str(&format!("{pad} |\n"));
        for n in &diag.notes {
            out.push_str(&format!("{pad} = note: {n}\n"));
        }
        for h in &diag.help {
            out.push_str(&format!("{pad} = help: {h}\n"));
        }
    }
    out
}

#[cfg(test)]
mod render_tests {
    use super::*;

    #[test]
    fn renders_label_and_help() {
        let src = "a + b\n";
        let d = Diagnostic::new(Span::new(0, 5), "oops").with_label("here").with_help("try again");
        assert_eq!(
            render_diagnostic("f.cera", src, &d),
            "error: oops\n --> f.cera:1:1\n  |\n1 | a + b\n  | ^^^^^ here\n  |\n  = help: try again\n"
        );
    }

    #[test]
    fn suggests_close_names_only() {
        assert_eq!(closest("pritn", ["print", "other"]), Some("print".to_string()));
        assert_eq!(closest("z", ["x", "y"]), None);
        assert_eq!(closest("abc", ["xyz"]), None);
    }
}
