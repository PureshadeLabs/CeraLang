//! `kote doc`: HTML API documentation from `pub` declarations and `##` comments.
//!
//! A run of `##` lines directly above a declaration documents it. A run of `##`
//! lines at the top of a file, followed by a blank line, documents the module.

use cera_syntax::ast::{DeclKind, Item, TypeDef};
use cera_syntax::parse;

pub struct DocItem {
    pub kind: &'static str,
    pub name: String,
    pub signature: String,
    pub doc: String,
}

pub struct ModDoc {
    /// The name other code imports: the package name for `lib.cera`, else the module path.
    pub name: String,
    pub doc: String,
    pub items: Vec<DocItem>,
}

/// The `##` block ending just above byte offset `at`, without the markers.
fn doc_above(src: &str, at: usize) -> String {
    let before = &src[..at];
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let mut lines: Vec<&str> = vec![];
    let mut end = line_start;
    while end > 0 {
        let prev_start = src[..end - 1].rfind('\n').map_or(0, |i| i + 1);
        let line = src[prev_start..end - 1].trim();
        match line.strip_prefix("##") {
            Some(rest) => lines.push(rest.strip_prefix(' ').unwrap_or(rest)),
            None => break,
        }
        end = prev_start;
    }
    lines.reverse();
    lines.join("\n")
}

/// The `##` block at the very top of the file when a blank line follows it.
fn module_doc(src: &str) -> String {
    let mut lines = vec![];
    let mut rest = src;
    loop {
        let (line, tail) = rest.split_once('\n').unwrap_or((rest, ""));
        match line.trim().strip_prefix("##") {
            Some(r) => {
                lines.push(r.strip_prefix(' ').unwrap_or(r).to_string());
                rest = tail;
            }
            None => break,
        }
        if tail.is_empty() {
            break;
        }
    }
    let followed_by_blank = rest.lines().next().is_some_and(|l| l.trim().is_empty());
    if followed_by_blank { lines.join("\n") } else { String::new() }
}

pub fn collect(name: &str, src: &str) -> Result<ModDoc, String> {
    let parsed = parse(src);
    if !parsed.is_ok() {
        return Err(format!("{name}: the file has syntax errors; run `kote check`"));
    }
    let mut items = vec![];
    for item in &parsed.program.items {
        let Item::Decl(d) = item else { continue };
        if !d.public {
            continue;
        }
        let text = &src[d.span.start..d.span.end.min(src.len())];
        let (kind, signature) = match &d.kind {
            DeclKind::Fn(f) => {
                let sig = match &f.body {
                    Some(b) => src[d.span.start..b.span.start.min(src.len())].trim_end().trim_end_matches("do").trim_end().to_string(),
                    None => text.to_string(),
                };
                ("function", sig)
            }
            DeclKind::Type(TypeDef::Variants(_)) => ("type", text.to_string()),
            DeclKind::Type(TypeDef::Record(_)) => ("type", text.to_string()),
            DeclKind::Type(TypeDef::Opaque) => ("type", text.to_string()),
            DeclKind::Trait(_) => ("trait", text.to_string()),
            DeclKind::Const(_) => ("constant", text.lines().next().unwrap_or("").to_string()),
        };
        items.push(DocItem { kind, name: d.name.name.clone(), signature, doc: doc_above(src, d.span.start) });
    }
    Ok(ModDoc { name: name.to_string(), doc: module_doc(src), items })
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn inline(s: &str) -> String {
    let mut out = String::new();
    for (i, part) in esc(s).split('`').enumerate() {
        if i % 2 == 1 {
            out.push_str(&format!("<code>{part}</code>"));
        } else {
            out.push_str(part);
        }
    }
    out
}

/// A small subset of Markdown: paragraphs, `-` lists, fenced code and `code` spans.
pub fn markdown(src: &str) -> String {
    let mut out = String::new();
    let (mut para, mut list): (Vec<&str>, Vec<&str>) = (vec![], vec![]);
    let mut code: Option<Vec<&str>> = None;
    fn flush(out: &mut String, para: &mut Vec<&str>, list: &mut Vec<&str>) {
        if !para.is_empty() {
            out.push_str(&format!("<p>{}</p>\n", inline(&para.join(" "))));
            para.clear();
        }
        if !list.is_empty() {
            out.push_str("<ul>\n");
            for l in list.drain(..) {
                out.push_str(&format!("<li>{}</li>\n", inline(l)));
            }
            out.push_str("</ul>\n");
        }
    }
    for line in src.lines() {
        if let Some(buf) = &mut code {
            if line.trim_start().starts_with("```") {
                out.push_str(&format!("<pre><code>{}</code></pre>\n", esc(&buf.join("\n"))));
                code = None;
            } else {
                buf.push(line);
            }
            continue;
        }
        let t = line.trim();
        if t.starts_with("```") {
            flush(&mut out, &mut para, &mut list);
            code = Some(vec![]);
        } else if t.is_empty() {
            flush(&mut out, &mut para, &mut list);
        } else if let Some(item) = t.strip_prefix("- ") {
            if !para.is_empty() {
                flush(&mut out, &mut para, &mut list);
            }
            list.push(item);
        } else {
            if !list.is_empty() {
                flush(&mut out, &mut para, &mut list);
            }
            para.push(t);
        }
    }
    if let Some(buf) = code {
        out.push_str(&format!("<pre><code>{}</code></pre>\n", esc(&buf.join("\n"))));
    }
    flush(&mut out, &mut para, &mut list);
    out
}

fn first_sentence(doc: &str) -> String {
    let flat = doc.lines().take_while(|l| !l.trim().is_empty()).collect::<Vec<_>>().join(" ");
    match flat.find(". ") {
        Some(i) => flat[..=i].to_string(),
        None => flat,
    }
}

const STYLE: &str = "body{font:16px/1.5 system-ui,sans-serif;max-width:52rem;margin:2rem auto;padding:0 1rem;color:#1d2024;background:#fff}\
@media(prefers-color-scheme:dark){body{color:#e6e8eb;background:#14161a}pre,code{background:#1f2328}a{color:#7ab7ff}}\
code,pre{font-family:ui-monospace,monospace;background:#f2f4f6;border-radius:4px}code{padding:0 .25em}pre{padding:.75rem;overflow-x:auto}\
h1,h2,h3{line-height:1.2}.item{border-top:1px solid #8884;padding-top:.5rem;margin-top:1.5rem}.kind{font-size:.8rem;opacity:.7}nav a{margin-right:1rem}";

fn page(title: &str, body: &str) -> String {
    format!("<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{}</title><style>{STYLE}</style></head><body>\n{body}</body></html>\n", esc(title))
}

fn file_name(module: &str) -> String {
    format!("{}.html", module.replace('.', "_"))
}

/// `(file name, html)` pairs: an index and one page per module.
pub fn render(package: &str, version: &str, mods: &[ModDoc]) -> Vec<(String, String)> {
    let mut files = vec![];
    let mut index = format!("<h1>{} <small>v{}</small></h1>\n", esc(package), esc(version));
    index.push_str("<h2>Modules</h2>\n<ul>\n");
    for m in mods {
        index.push_str(&format!(
            "<li><a href=\"{}\"><code>{}</code></a> {}</li>\n",
            file_name(&m.name),
            esc(&m.name),
            inline(&first_sentence(&m.doc))
        ));
    }
    index.push_str("</ul>\n");
    files.push(("index.html".to_string(), page(package, &index)));
    for m in mods {
        let mut b = format!("<nav><a href=\"index.html\">{}</a></nav>\n<h1>Module <code>{}</code></h1>\n", esc(package), esc(&m.name));
        b.push_str(&markdown(&m.doc));
        if m.items.is_empty() {
            b.push_str("<p>No public items.</p>\n");
        }
        for kind in ["type", "trait", "function", "constant"] {
            let of_kind: Vec<&DocItem> = m.items.iter().filter(|i| i.kind == kind).collect();
            if of_kind.is_empty() {
                continue;
            }
            let heading = match kind {
                "type" => "Types",
                "trait" => "Traits",
                "function" => "Functions",
                _ => "Constants",
            };
            b.push_str(&format!("<h2>{heading}</h2>\n"));
            for i in of_kind {
                b.push_str(&format!(
                    "<div class=\"item\" id=\"{n}\"><span class=\"kind\">{k}</span><pre><code>{s}</code></pre>\n{d}</div>\n",
                    n = esc(&i.name),
                    k = i.kind,
                    s = esc(&i.signature),
                    d = markdown(&i.doc)
                ));
            }
        }
        files.push((file_name(&m.name), page(&format!("{} - {}", m.name, package), &b)));
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "## Shapes and their areas.\n##\n## Used by the geometry examples.\n\n## A shape.\npub Shape :: type\n  | Circle(r: i32)\nend\n\n## Area of a shape, in `i32` units.\n##\n## - circles use r*r\npub area :: fn(s: Shape) -> i32 do\n  1\nend\n\nhelper :: fn() -> i32 do\n  2\nend\n";

    #[test]
    fn collects_public_items_with_docs() {
        let m = collect("geometry", SRC).unwrap();
        assert_eq!(m.doc, "Shapes and their areas.\n\nUsed by the geometry examples.");
        let names: Vec<&str> = m.items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Shape", "area"]);
        assert_eq!(m.items[1].signature, "pub area :: fn(s: Shape) -> i32");
        assert!(m.items[1].doc.starts_with("Area of a shape, in `i32` units."));
        assert_eq!(m.items[0].doc, "A shape.");
    }

    #[test]
    fn markdown_subset() {
        let h = markdown("Hello `x` <b>\n\n- one\n- two\n\n```\nlet a < b\n```\n");
        assert!(h.contains("<p>Hello <code>x</code> &lt;b&gt;</p>"), "{h}");
        assert!(h.contains("<li>one</li>") && h.contains("<pre><code>let a &lt; b</code></pre>"), "{h}");
    }

    #[test]
    fn module_doc_needs_a_blank_line() {
        assert_eq!(module_doc("## only a decl comment\npub f :: fn() do\nend\n"), "");
    }
}
