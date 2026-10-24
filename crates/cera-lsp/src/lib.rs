//! A small language server for Cera.
//!
//! The protocol layer is deliberately thin: [`Server::handle`] takes one decoded
//! JSON-RPC message and returns the messages to send back, so the whole server
//! can be tested without any I/O. `main.rs` adds the stdio framing.
//!
//! Supported today: diagnostics (from the parser), document symbols, hover,
//! go-to-definition within a file, and completion of keywords, snippets and
//! names declared in the file. Type-aware features arrive with the type checker.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use kote::ide::ImportTarget;

use cera_syntax::ast::*;
use cera_sema::TypeAt;
use cera_syntax::token::KEYWORDS;
use cera_syntax::{parse, LineIndex, Parse, Span};
use serde_json::{json, Value};

struct Doc {
    text: String,
    index: LineIndex,
    parse: Parse,
    /// Type errors; only computed when the file has no syntax errors.
    sema: Vec<cera_syntax::Diagnostic>,
    types: Vec<TypeAt>,
    /// Set when the file belongs to a kote package: what its imports name.
    imports: Vec<(String, ImportTarget)>,
    package: Option<PathBuf>,
}

impl Doc {
    /// Parse only; the caller supplies diagnostics and types.
    fn parsed(text: String) -> Doc {
        let index = LineIndex::new(&text);
        let parse = parse(&text);
        Doc { text, index, parse, sema: Vec::new(), types: Vec::new(), imports: Vec::new(), package: None }
    }

    /// Single-file analysis (no package around the file).
    fn new(text: String) -> Doc {
        let mut d = Doc::parsed(text);
        if d.parse.is_ok() {
            let a = cera_sema::analyze(&d.parse.program);
            d.sema = a.diagnostics;
            d.types = a.types;
        }
        d
    }

    /// The recorded type of the smallest expression or binding covering `offset`.
    fn type_at(&self, offset: usize) -> Option<&TypeAt> {
        self.types
            .iter()
            .filter(|t| t.span.start <= offset && offset < t.span.end)
            .min_by_key(|t| t.span.len())
    }

    fn import(&self, alias: &str) -> Option<&ImportTarget> {
        self.imports.iter().find(|(a, _)| a == alias).map(|(_, t)| t)
    }

    fn position(&self, offset: usize) -> Value {
        let (line, character) = self.index.line_col(&self.text, offset);
        json!({ "line": line, "character": character })
    }

    fn range(&self, span: Span) -> Value {
        json!({ "start": self.position(span.start), "end": self.position(span.end) })
    }

    /// The identifier under a zero-based line/character position, with its span.
    fn word_at(&self, line: usize, character: usize) -> Option<(String, Span)> {
        let offset = self.index.offset(&self.text, line, character);
        let bytes = self.text.as_bytes();
        let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        let mut start = offset.min(bytes.len());
        let mut end = start;
        while start > 0 && is_word(bytes[start - 1]) {
            start -= 1;
        }
        while end < bytes.len() && is_word(bytes[end]) {
            end += 1;
        }
        if start == end {
            return None;
        }
        Some((self.text[start..end].to_string(), Span::new(start, end)))
    }
}

pub struct Server {
    docs: HashMap<String, Doc>,
    /// Text of every open file, by path (analysis prefers it over the disk).
    buffers: HashMap<PathBuf, String>,
    /// Files other than the edited one that each root last published diagnostics for.
    published: HashMap<String, HashSet<String>>,
    /// Text of files seen by the last analyses, for jumping into other modules.
    known: HashMap<PathBuf, String>,
    pub shutdown_requested: bool,
    pub exit_requested: bool,
}

impl Default for Server {
    fn default() -> Self {
        Server::new()
    }
}

fn response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

impl Server {
    pub fn new() -> Server {
        Server {
            docs: HashMap::new(),
            buffers: HashMap::new(),
            published: HashMap::new(),
            known: HashMap::new(),
            shutdown_requested: false,
            exit_requested: false,
        }
    }

    /// Handle one incoming message and return everything to send back.
    pub fn handle(&mut self, msg: Value) -> Vec<Value> {
        let method = match msg.get("method").and_then(Value::as_str) {
            Some(m) => m.to_string(),
            None => return Vec::new(), // a response to us; we send no requests
        };
        let id = msg.get("id").cloned();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);

        match (method.as_str(), id) {
            ("initialize", Some(id)) => vec![response(id, self.capabilities())],
            ("shutdown", Some(id)) => {
                self.shutdown_requested = true;
                vec![response(id, Value::Null)]
            }
            ("exit", _) => {
                self.exit_requested = true;
                Vec::new()
            }
            ("textDocument/didOpen", _) => {
                let td = &params["textDocument"];
                match (td["uri"].as_str(), td["text"].as_str()) {
                    (Some(uri), Some(text)) => self.update(uri, text.to_string()),
                    _ => Vec::new(),
                }
            }
            ("textDocument/didChange", _) => {
                let uri = params["textDocument"]["uri"].as_str();
                let text = params["contentChanges"]
                    .as_array()
                    .and_then(|c| c.last())
                    .and_then(|c| c["text"].as_str());
                match (uri, text) {
                    (Some(uri), Some(text)) => self.update(uri, text.to_string()),
                    _ => Vec::new(),
                }
            }
            ("textDocument/didClose", _) => {
                let Some(uri) = params["textDocument"]["uri"].as_str() else {
                    return Vec::new();
                };
                self.docs.remove(uri);
                if let Some(p) = uri_to_path(uri) {
                    self.buffers.remove(&p);
                }
                vec![publish(uri, json!([]))]
            }
            ("textDocument/documentSymbol", Some(id)) => {
                vec![response(id, self.document_symbols(&params))]
            }
            ("textDocument/hover", Some(id)) => vec![response(id, self.hover(&params))],
            ("textDocument/definition", Some(id)) => {
                vec![response(id, self.definition(&params))]
            }
            ("textDocument/completion", Some(id)) => {
                vec![response(id, self.completion(&params))]
            }
            (_, Some(id)) if !method.starts_with("$/") => {
                vec![error_response(
                    id,
                    -32601,
                    &format!("method not found: {method}"),
                )]
            }
            _ => Vec::new(),
        }
    }

    fn capabilities(&self) -> Value {
        json!({
            "capabilities": {
                "textDocumentSync": 1,
                "documentSymbolProvider": true,
                "hoverProvider": true,
                "definitionProvider": true,
                "completionProvider": { "triggerCharacters": ["."] }
            },
            "serverInfo": { "name": "cera-lsp", "version": env!("CARGO_PKG_VERSION") }
        })
    }

    fn update(&mut self, uri: &str, text: String) -> Vec<Value> {
        let path = uri_to_path(uri);
        if let Some(p) = &path {
            self.buffers.insert(p.clone(), text.clone());
        }
        let in_package = path.as_ref().filter(|p| kote::ide::find_project(p).is_some());
        let mut out = Vec::new();
        let mut now: HashSet<String> = HashSet::new();
        let doc = match in_package {
            Some(p) => {
                let mut doc = Doc::parsed(text);
                doc.package = kote::ide::find_project(p);
                match kote::ide::analyze_file(p, &self.buffers) {
                    Ok(an) => {
                        let me = canon(p);
                        // This file's problems come from the package analysis.
                        let mut by_file: HashMap<PathBuf, Vec<(Span, String)>> = HashMap::new();
                        for (f, sp, m) in &an.diagnostics {
                            by_file.entry(f.clone()).or_default().push((*sp, m.clone()));
                        }
                        doc.sema = by_file.remove(&me).unwrap_or_default().into_iter().map(|(sp, m)| cera_syntax::Diagnostic::new(sp, m)).collect();
                        doc.parse.diagnostics.clear();
                        doc.types = an.types.iter().filter(|(f, _, _)| *f == me).map(|(_, sp, t)| TypeAt { span: *sp, text: t.clone() }).collect();
                        doc.imports = an.imports.get(&me).cloned().unwrap_or_default();
                        for (f, t) in &an.files {
                            self.known.insert(f.clone(), t.clone());
                        }
                        for (f, ds) in by_file {
                            let Some(text) = an.files.iter().find(|(p, _)| *p == f).map(|(_, t)| t.clone()) else { continue };
                            let u = path_to_uri(&f);
                            out.push(publish(&u, Value::Array(ds.iter().map(|(sp, m)| diagnostic_json(&text, *sp, m)).collect())));
                            now.insert(u);
                        }
                    }
                    Err(e) => {
                        doc.sema = vec![cera_syntax::Diagnostic::new(Span::new(0, 0), format!("kote: {e}"))];
                    }
                }
                doc
            }
            None => Doc::new(text),
        };
        let diagnostics: Vec<Value> = doc
            .parse
            .diagnostics
            .iter()
            .chain(doc.sema.iter())
            .map(|d| diagnostic_json(&doc.text, d.span, &d.message))
            .collect();
        // Files this root published for before, but no longer has problems in.
        let old = self.published.remove(uri).unwrap_or_default();
        for u in old.difference(&now) {
            if u != uri && !self.docs.contains_key(u) {
                out.push(publish(u, json!([])));
            }
        }
        now.remove(uri);
        self.published.insert(uri.to_string(), now);
        // The edited file itself (replace any entry for it made above).
        out.retain(|m| m["params"]["uri"] != uri);
        out.push(publish(uri, Value::Array(diagnostics)));
        self.docs.insert(uri.to_string(), doc);
        out
    }

    fn doc_for<'a>(&'a self, params: &'a Value) -> Option<(&'a str, &'a Doc)> {
        let uri = params["textDocument"]["uri"].as_str()?;
        self.docs.get(uri).map(|d| (uri, d))
    }

    // -------------------------------------------------------------- symbols

    fn document_symbols(&self, params: &Value) -> Value {
        let Some((_, doc)) = self.doc_for(params) else {
            return json!([]);
        };
        let mut out = Vec::new();
        for item in &doc.parse.program.items {
            match item {
                Item::Import(i) => {
                    let name: Vec<&str> = i.path.iter().map(|p| p.name.as_str()).collect();
                    out.push(symbol(doc, &name.join("."), None, 2, i.span, i.span, None));
                }
                Item::Decl(d) => out.push(decl_symbol(doc, d)),
                Item::Impl(im) => {
                    let name = format!("impl {} for {}", im.trait_name.name, fmt_type(&im.ty));
                    out.push(symbol(doc, &name, None, 5, im.span, im.trait_name.span, None));
                }
            }
        }
        Value::Array(out)
    }

    // ---------------------------------------------------------------- hover

    fn hover(&self, params: &Value) -> Value {
        let Some((_, doc)) = self.doc_for(params) else {
            return Value::Null;
        };
        let pos = &params["position"];
        let (Some(line), Some(ch)) = (pos["line"].as_u64(), pos["character"].as_u64()) else {
            return Value::Null;
        };
        let Some((word, span)) = doc.word_at(line as usize, ch as usize) else {
            return Value::Null;
        };

        let module_hover = qualifier(&doc.text, span).and_then(|alias| match doc.import(&alias) {
            Some(ImportTarget::File(p)) => {
                let (_, prog) = self.module_program(p)?;
                find_symbol(&prog, &word).map(|f| f.hover)
            }
            _ => None,
        });
        let text = if let Some(h) = module_hover {
            Some(h)
        } else if let Some(doc_text) = keyword_doc(&word) {
            Some(format!("**`{word}`**\n\n{doc_text}"))
        } else {
            let offset = doc.index.offset(&doc.text, line as usize, ch as usize);
            find_symbol(&doc.parse.program, &word)
                .map(|s| s.hover)
                .or_else(|| {
                    doc.type_at(offset)
                        .map(|t| format!("```cera\n{}\n```", t.text))
                })
        };
        match text {
            Some(value) => json!({
                "contents": { "kind": "markdown", "value": value },
                "range": doc.range(span),
            }),
            None => Value::Null,
        }
    }

    // ----------------------------------------------------------- definition

    fn definition(&self, params: &Value) -> Value {
        let Some((uri, doc)) = self.doc_for(params) else {
            return Value::Null;
        };
        let pos = &params["position"];
        let (Some(line), Some(ch)) = (pos["line"].as_u64(), pos["character"].as_u64()) else {
            return Value::Null;
        };
        let Some((word, span)) = doc.word_at(line as usize, ch as usize) else {
            return Value::Null;
        };
        if let Some(target) = self.qualified_target(doc, &word, span) {
            return target;
        }
        match find_symbol(&doc.parse.program, &word) {
            Some(s) => json!({ "uri": uri, "range": doc.range(s.name_span) }),
            None => Value::Null,
        }
    }

    /// The text and syntax tree of a module file (open buffer first, then disk).
    fn module_program(&self, path: &Path) -> Option<(String, Program)> {
        let text = self.buffers.get(path).cloned().or_else(|| self.known.get(path).cloned()).or_else(|| std::fs::read_to_string(path).ok())?;
        let program = parse(&text).program;
        Some((text, program))
    }

    /// `alias.word` or the bare `alias`: the location it refers to in another module.
    fn qualified_target(&self, doc: &Doc, word: &str, span: Span) -> Option<Value> {
        if let Some(alias) = qualifier(&doc.text, span) {
            if let Some(ImportTarget::File(p)) = doc.import(&alias) {
                let (text, prog) = self.module_program(p)?;
                let found = find_symbol(&prog, word)?;
                return Some(json!({ "uri": path_to_uri(p), "range": span_range(&text, found.name_span) }));
            }
        }
        if let Some(ImportTarget::File(p)) = doc.import(word) {
            let text = self.module_program(p)?.0;
            return Some(json!({ "uri": path_to_uri(p), "range": span_range(&text, Span::new(0, 0)) }));
        }
        None
    }

    // ----------------------------------------------------------- completion

    fn completion(&self, params: &Value) -> Value {
        let mut items: Vec<Value> = Vec::new();
        let doc = self.doc_for(params).map(|(_, d)| d);
        let cursor = doc.zip(params["position"]["line"].as_u64()).zip(params["position"]["character"].as_u64());
        if let Some(((doc, line), ch)) = cursor {
            let off = doc.index.offset(&doc.text, line as usize, ch as usize).min(doc.text.len());
            let prefix = &doc.text[doc.text[..off].rfind('\n').map_or(0, |i| i + 1)..off];
            // `import <partial>`: module names.
            if prefix.trim_start().starts_with("import ") {
                if let Some(dir) = &doc.package {
                    for m in kote::ide::importable_modules(dir) {
                        items.push(json!({ "label": m, "kind": 9, "detail": "module" }));
                    }
                }
                return json!({ "isIncomplete": false, "items": items });
            }
            // `alias.<partial>`: the public members of that module.
            let head = prefix.trim_end_matches(|c: char| c.is_ascii_alphanumeric() || c == '_');
            if head.ends_with('.') {
                if let Some(alias) = qualifier(head, Span::new(head.len(), head.len())) {
                    match doc.import(&alias) {
                        Some(ImportTarget::File(p)) => {
                            if let Some((_, prog)) = self.module_program(p) {
                                items = decl_items(&prog, true);
                            }
                            return json!({ "isIncomplete": false, "items": items });
                        }
                        Some(ImportTarget::Std(_)) => return json!({ "isIncomplete": false, "items": items }),
                        None => {}
                    }
                }
            }
        }

        for kw in KEYWORDS.iter().chain(std::iter::once(&"pub")) {
            items.push(json!({ "label": kw, "kind": 14, "detail": "keyword" }));
        }
        for (label, body, detail) in SNIPPETS {
            items.push(json!({
                "label": label,
                "kind": 15,
                "detail": detail,
                "insertText": body,
                "insertTextFormat": 2,
            }));
        }
        if let Some(doc) = doc {
            items.extend(decl_items(&doc.parse.program, false));
            for (alias, _) in &doc.imports {
                items.push(json!({ "label": alias, "kind": 9, "detail": "module" }));
            }
        }
        json!({ "isIncomplete": false, "items": items })
    }
}

/// Completion items for the declarations of `program` (only `pub` ones if `only_public`).
fn decl_items(program: &Program, only_public: bool) -> Vec<Value> {
    let mut items = Vec::new();
    for item in &program.items {
        let Item::Decl(d) = item else { continue };
        if only_public && !d.public {
            continue;
        }
        match &d.kind {
            DeclKind::Trait(_) => items.push(json!({ "label": d.name.name, "kind": 8, "detail": "trait" })),
            DeclKind::Fn(f) => items.push(json!({ "label": d.name.name, "kind": 3, "detail": fn_signature(f) })),
            DeclKind::Const(_) => items.push(json!({ "label": d.name.name, "kind": 21, "detail": "constant" })),
            DeclKind::Type(TypeDef::Record(_)) => items.push(json!({ "label": d.name.name, "kind": 22, "detail": "record type" })),
            DeclKind::Type(TypeDef::Opaque) => items.push(json!({ "label": d.name.name, "kind": 7, "detail": "opaque type" })),
            DeclKind::Type(TypeDef::Variants(vs)) => {
                items.push(json!({ "label": d.name.name, "kind": 13, "detail": "type" }));
                for v in vs {
                    items.push(json!({ "label": v.name.name, "kind": 20, "detail": format!("variant of {}", d.name.name) }));
                }
            }
        }
    }
    items
}

fn publish(uri: &str, diagnostics: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": { "uri": uri, "diagnostics": diagnostics }
    })
}

/// The identifier before the `.` that precedes `span`, as in `geometry` of `geometry.area`.
fn qualifier(text: &str, span: Span) -> Option<String> {
    let b = text.as_bytes();
    if span.start == 0 || b.get(span.start - 1) != Some(&b'.') {
        return None;
    }
    let end = span.start - 1;
    let mut start = end;
    while start > 0 && (b[start - 1].is_ascii_alphanumeric() || b[start - 1] == b'_') {
        start -= 1;
    }
    (start < end).then(|| text[start..end].to_string())
}

fn span_range(text: &str, span: Span) -> Value {
    let idx = LineIndex::new(text);
    let pos = |o: usize| {
        let (line, character) = idx.line_col(text, o.min(text.len()));
        json!({ "line": line, "character": character })
    };
    json!({ "start": pos(span.start), "end": pos(span.end) })
}

fn diagnostic_json(text: &str, span: Span, message: &str) -> Value {
    let idx = LineIndex::new(text);
    let pos = |o: usize| {
        let (line, character) = idx.line_col(text, o.min(text.len()));
        json!({ "line": line, "character": character })
    };
    json!({ "range": { "start": pos(span.start), "end": pos(span.end) }, "severity": 1, "source": "cera", "message": message })
}

fn canon(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 3 <= bytes.len() {
            if let Ok(b) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    Some(PathBuf::from(String::from_utf8_lossy(&out).into_owned()))
}

pub fn path_to_uri(p: &Path) -> String {
    let mut s = String::from("file://");
    for b in p.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

const SNIPPETS: &[(&str, &str, &str)] = &[
    ("main", "main :: fn() do\n\t$0\nend", "program entry point"),
    (
        "fn",
        "${1:name} :: fn(${2:args}) -> ${3:T} do\n\t$0\nend",
        "function declaration",
    ),
    (
        "type",
        "${1:Name} :: type\n\t| ${2:Variant}\nend",
        "variant type",
    ),
    (
        "match",
        "match ${1:value} do\n\t${2:pattern} -> ${3:result}\nend",
        "match expression",
    ),
    ("region", "region do\n\t$0\nend", "memory region"),
    ("if", "if ${1:cond} do\n\t$0\nend", "conditional"),
];

fn keyword_doc(word: &str) -> Option<&'static str> {
    Some(match word {
        "import" => "Bring a module into scope: `import std.io`, or `import std.io as io`.",
        "as" => "Give an imported module another name.",
        "fn" => "Declares a function (`name :: fn(a: T) -> R do ... end`), names a function type (`fn(A) -> B`), or writes a lambda (`fn(x) -> x + 1 end`).",
        "type" => "Declares a variant type (`| A | B(x: T)`) or a record type (`{ x: T }`).",
        "do" => "Opens a block. Every `do` is closed by `end`.",
        "end" => "Closes a block, `type` declaration or lambda.",
        "if" => "`if cond do ... else ... end`. It is an expression. Chain with `else if`.",
        "else" => "The alternative branch of an `if`.",
        "match" => "Pattern matching. Arms are `pattern [if guard] -> expr`. Matches must be exhaustive.",
        "region" => "`region do ... end` creates an arena. Everything allocated inside is freed at `end`. Values leaving the region must be moved out with `promote` or `new_in`.",
        "with" => "`with allocator(a) do ... end` swaps the context allocator for the scope.",
        "defer" => "Runs an expression when the scope exits, in reverse order, including on early return.",
        "not" => "Boolean negation.",
        "and" => "Boolean and (short-circuiting).",
        "or" => "Boolean or (short-circuiting).",
        "true" | "false" => "Boolean literal.",
        "ref" => "`ref T` is a reference to a `T` living in some region.",
        _ => return None,
    })
}

// ------------------------------------------------------------------ symbols

fn symbol(
    doc: &Doc,
    name: &str,
    detail: Option<String>,
    kind: u32,
    range: Span,
    selection: Span,
    children: Option<Vec<Value>>,
) -> Value {
    let mut v = json!({
        "name": name,
        "kind": kind,
        "range": doc.range(range),
        "selectionRange": doc.range(selection),
    });
    if let Some(d) = detail {
        v["detail"] = Value::String(d);
    }
    if let Some(c) = children {
        v["children"] = Value::Array(c);
    }
    v
}

fn decl_symbol(doc: &Doc, d: &Decl) -> Value {
    match &d.kind {
        DeclKind::Trait(_) => symbol(doc, &d.name.name, None, 11, d.span, d.name.span, None),
        DeclKind::Fn(f) => symbol(
            doc,
            &d.name.name,
            Some(fn_signature(f)),
            12,
            d.span,
            d.name.span,
            None,
        ),
        DeclKind::Const(_) => symbol(doc, &d.name.name, None, 14, d.span, d.name.span, None),
        DeclKind::Type(TypeDef::Variants(vs)) => {
            let children = vs
                .iter()
                .map(|v| {
                    symbol(
                        doc,
                        &v.name.name,
                        Some(variant_signature(v)),
                        22,
                        v.span,
                        v.name.span,
                        None,
                    )
                })
                .collect();
            symbol(
                doc,
                &d.name.name,
                None,
                10,
                d.span,
                d.name.span,
                Some(children),
            )
        }
        DeclKind::Type(TypeDef::Opaque) => {
            symbol(doc, &d.name.name, None, 5, d.span, d.name.span, None)
        }
        DeclKind::Type(TypeDef::Record(fields)) => {
            let children = fields
                .iter()
                .map(|f| {
                    let span = f.name.span.to(f.ty.span);
                    symbol(
                        doc,
                        &f.name.name,
                        Some(fmt_type(&f.ty)),
                        8,
                        span,
                        f.name.span,
                        None,
                    )
                })
                .collect();
            symbol(
                doc,
                &d.name.name,
                None,
                23,
                d.span,
                d.name.span,
                Some(children),
            )
        }
    }
}

struct Found {
    hover: String,
    name_span: Span,
}

/// Find a top-level declaration or type variant by name.
fn find_symbol(program: &Program, name: &str) -> Option<Found> {
    for item in &program.items {
        let Item::Decl(d) = item else { continue };
        if d.name.name == name {
            let hover = match &d.kind {
                DeclKind::Trait(t) => {
                    let body: Vec<String> = t
                        .methods
                        .iter()
                        .map(|m| {
                            let ps: Vec<String> = m
                                .params
                                .iter()
                                .map(|p| match &p.ty {
                                    Some(ty) => format!("{}: {}", p.name.name, fmt_type(ty)),
                                    None => p.name.name.clone(),
                                })
                                .collect();
                            let ret = m.ret.as_ref().map_or(String::new(), |r| format!(" -> {}", fmt_type(r)));
                            format!("  {} :: fn({}){}", m.name.name, ps.join(", "), ret)
                        })
                        .collect();
                    format!("```cera\n{} :: trait\n{}\nend\n```", d.name.name, body.join("\n"))
                }
                DeclKind::Fn(f) => format!("```cera\n{} :: {}\n```", d.name.name, fn_signature(f)),
                DeclKind::Const(_) => format!("```cera\n{} :: <constant>\n```", d.name.name),
                DeclKind::Type(TypeDef::Record(fields)) => {
                    let body: Vec<String> = fields
                        .iter()
                        .map(|f| format!("  {}: {}", f.name.name, fmt_type(&f.ty)))
                        .collect();
                    format!(
                        "```cera\n{}{} :: type {{\n{}\n}}\n```",
                        d.name.name,
                        fmt_generics(&d.generics),
                        body.join(",\n")
                    )
                }
                DeclKind::Type(TypeDef::Opaque) => {
                    format!("```cera\n{}{} :: type\n```", d.name.name, fmt_generics(&d.generics))
                }
                DeclKind::Type(TypeDef::Variants(vs)) => {
                    let body: Vec<String> = vs
                        .iter()
                        .map(|v| format!("  | {}", variant_signature(v)))
                        .collect();
                    format!(
                        "```cera\n{}{} :: type\n{}\nend\n```",
                        d.name.name,
                        fmt_generics(&d.generics),
                        body.join("\n")
                    )
                }
            };
            return Some(Found {
                hover,
                name_span: d.name.span,
            });
        }
        if let DeclKind::Type(TypeDef::Variants(vs)) = &d.kind {
            for v in vs {
                if v.name.name == name {
                    return Some(Found {
                        hover: format!(
                            "```cera\n{}\n```\nvariant of `{}`",
                            variant_signature(v),
                            d.name.name
                        ),
                        name_span: v.name.span,
                    });
                }
            }
        }
    }
    None
}

fn fmt_generics(gs: &[Ident]) -> String {
    if gs.is_empty() {
        String::new()
    } else {
        let names: Vec<&str> = gs.iter().map(|g| g.name.as_str()).collect();
        format!("<{}>", names.join(", "))
    }
}

fn fn_signature(f: &FnDef) -> String {
    let params: Vec<String> = f
        .params
        .iter()
        .map(|p| match &p.ty {
            Some(t) => format!("{}: {}", p.name.name, fmt_type(t)),
            None => p.name.name.clone(),
        })
        .collect();
    let ret = match &f.ret {
        Some(t) => format!(" -> {}", fmt_type(t)),
        None => String::new(),
    };
    format!(
        "fn{}({}){}",
        fmt_generics(&f.generics),
        params.join(", "),
        ret
    )
}

fn variant_signature(v: &Variant) -> String {
    if v.fields.is_empty() {
        return v.name.name.clone();
    }
    let fields: Vec<String> = v
        .fields
        .iter()
        .map(|f| match &f.name {
            Some(n) => format!("{}: {}", n.name, fmt_type(&f.ty)),
            None => fmt_type(&f.ty),
        })
        .collect();
    format!("{}({})", v.name.name, fields.join(", "))
}

pub fn fmt_type(t: &Type) -> String {
    match &t.kind {
        TypeKind::Path { path, args } => {
            let names: Vec<&str> = path.iter().map(|p| p.name.as_str()).collect();
            let mut s = names.join(".");
            if !args.is_empty() {
                let a: Vec<String> = args.iter().map(fmt_type).collect();
                s.push_str(&format!("<{}>", a.join(", ")));
            }
            s
        }
        TypeKind::Fn { params, ret } => {
            let p: Vec<String> = params.iter().map(fmt_type).collect();
            format!("fn({}) -> {}", p.join(", "), fmt_type(ret))
        }
        TypeKind::List(inner) => format!("[{}]", fmt_type(inner)),
        TypeKind::Tuple(items) => {
            let p: Vec<String> = items.iter().map(fmt_type).collect();
            format!("({})", p.join(", "))
        }
        TypeKind::Ref(inner) => format!("ref {}", fmt_type(inner)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const URI: &str = "file:///t.cera";

    const GOOD: &str = "\
Shape :: type
  | Circle(r: f64)
  | Rect(w: f64, h: f64)
end

area :: fn(s: Shape) -> f64 do
  match s do
    Circle(r) -> r
    Rect(w, h) -> w * h
  end
end
";

    fn open(server: &mut Server, text: &str) -> Vec<Value> {
        server.handle(json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": { "textDocument": { "uri": URI, "text": text, "version": 1 } }
        }))
    }

    fn request(server: &mut Server, method: &str, params: Value) -> Value {
        let mut out = server.handle(json!({
            "jsonrpc": "2.0", "id": 7, "method": method, "params": params
        }));
        assert_eq!(out.len(), 1);
        let msg = out.remove(0);
        assert_eq!(msg["id"], 7);
        msg["result"].clone()
    }

    fn at(line: u64, character: u64) -> Value {
        json!({ "textDocument": { "uri": URI }, "position": { "line": line, "character": character } })
    }

    #[test]
    fn initialize_advertises_capabilities() {
        let mut s = Server::new();
        let r = request(&mut s, "initialize", json!({}));
        assert_eq!(r["capabilities"]["hoverProvider"], true);
        assert_eq!(r["serverInfo"]["name"], "cera-lsp");
    }

    #[test]
    fn clean_file_has_no_diagnostics() {
        let mut s = Server::new();
        let out = open(&mut s, GOOD);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["method"], "textDocument/publishDiagnostics");
        assert_eq!(out[0]["params"]["diagnostics"], json!([]));
    }

    #[test]
    fn broken_file_reports_position() {
        let mut s = Server::new();
        let out = open(&mut s, "main :: fn() do\n  x = (1 + 2\n  y\nend\n");
        let diags = out[0]["params"]["diagnostics"].as_array().unwrap();
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0]["range"]["start"]["line"], 2);
        assert_eq!(diags[0]["severity"], 1);
    }

    #[test]
    fn did_change_replaces_the_document() {
        let mut s = Server::new();
        open(&mut s, "f :: fn() do\n");
        let out = s.handle(json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": URI, "version": 2 },
                "contentChanges": [{ "text": "f :: fn() do\nend\n" }]
            }
        }));
        assert_eq!(out[0]["params"]["diagnostics"], json!([]));
    }

    #[test]
    fn did_close_clears_diagnostics() {
        let mut s = Server::new();
        open(&mut s, "f :: fn() do\n");
        let out = s.handle(json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didClose",
            "params": { "textDocument": { "uri": URI } }
        }));
        assert_eq!(out[0]["params"]["diagnostics"], json!([]));
    }

    #[test]
    fn symbols_nest_variants() {
        let mut s = Server::new();
        open(&mut s, GOOD);
        let r = request(
            &mut s,
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": URI } }),
        );
        let syms = r.as_array().unwrap();
        assert_eq!(syms.len(), 2);
        assert_eq!(syms[0]["name"], "Shape");
        assert_eq!(syms[0]["children"].as_array().unwrap().len(), 2);
        assert_eq!(syms[1]["name"], "area");
        assert_eq!(syms[1]["detail"], "fn(s: Shape) -> f64");
    }

    #[test]
    fn hover_on_keyword_and_function() {
        let mut s = Server::new();
        open(&mut s, GOOD);
        // `match` on line 6
        let r = request(&mut s, "textDocument/hover", at(6, 3));
        assert!(r["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("exhaustive"));
        // `area` on line 5
        let r = request(&mut s, "textDocument/hover", at(5, 1));
        assert!(r["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("area :: fn(s: Shape) -> f64"));
        // whitespace: nothing
        let r = request(&mut s, "textDocument/hover", at(0, 8));
        assert!(r.is_null());
    }

    #[test]
    fn definition_finds_types_and_variants() {
        let mut s = Server::new();
        open(&mut s, GOOD);
        // `Shape` in the parameter list of `area`
        let r = request(&mut s, "textDocument/definition", at(5, 15));
        assert_eq!(r["range"]["start"]["line"], 0);
        // `Circle` in the match arm
        let r = request(&mut s, "textDocument/definition", at(7, 6));
        assert_eq!(r["range"]["start"]["line"], 1);
    }

    #[test]
    fn completion_offers_keywords_snippets_and_names() {
        let mut s = Server::new();
        open(&mut s, GOOD);
        let r = request(&mut s, "textDocument/completion", at(0, 0));
        let labels: Vec<&str> = r["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["label"].as_str().unwrap())
            .collect();
        for want in ["region", "match", "main", "area", "Shape", "Circle"] {
            assert!(labels.contains(&want), "missing {want}");
        }
    }

    #[test]
    fn unknown_requests_get_an_error() {
        let mut s = Server::new();
        let out = s.handle(json!({ "jsonrpc": "2.0", "id": 1, "method": "nope/nothing" }));
        assert_eq!(out[0]["error"]["code"], -32601);
    }

    #[test]
    fn shutdown_then_exit() {
        let mut s = Server::new();
        request(&mut s, "shutdown", Value::Null);
        assert!(s.shutdown_requested);
        s.handle(json!({ "jsonrpc": "2.0", "method": "exit" }));
        assert!(s.exit_requested);
    }
    #[test]
    fn type_errors_are_published() {
        let mut s = Server::new();
        let out = open(&mut s, "f :: fn() -> i32 do\n  \"no\"\nend\n");
        let diags = out[0]["params"]["diagnostics"].as_array().unwrap();
        assert_eq!(diags.len(), 1);
        assert!(diags[0]["message"].as_str().unwrap().contains("should return `i32`"));
        assert_eq!(diags[0]["range"]["start"]["line"], 1);
    }

    #[test]
    fn syntax_errors_suppress_type_errors() {
        let mut s = Server::new();
        let out = open(&mut s, "f :: fn() -> i32 do\n  nope +\nend\ng :: fn( do\nend\n");
        let diags = out[0]["params"]["diagnostics"].as_array().unwrap();
        assert!(!diags.is_empty());
        assert!(diags
            .iter()
            .all(|d| !d["message"].as_str().unwrap().contains("unknown name")));
    }

    #[test]
    fn hover_shows_local_types() {
        let mut s = Server::new();
        open(&mut s, "f :: fn(x: i32) -> i32 do\n  y = x + 1\n  y\nend\n");
        // `y` on line 2
        let r = request(&mut s, "textDocument/hover", at(2, 2));
        let v = r["contents"]["value"].as_str().unwrap();
        assert!(v.contains("i32"), "{v}");
    }
}

#[cfg(test)]
mod package_tests {
    use super::*;

    struct Pkg(PathBuf);

    impl Pkg {
        fn new(tag: &str) -> Pkg {
            let d = std::env::temp_dir().join(format!("cera-lsp-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(d.join("src")).unwrap();
            std::fs::write(d.join("kote.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\n").unwrap();
            std::fs::write(
                d.join("src/geometry.cera"),
                "pub Shape :: type\n  | Circle(r: i32)\nend\n\npub area :: fn(s: Shape) -> i32 do\n  helper(s)\nend\n\nhelper :: fn(s: Shape) -> i32 do\n  1\nend\n",
            )
            .unwrap();
            Pkg(d.canonicalize().unwrap())
        }
        fn uri(&self, f: &str) -> String {
            path_to_uri(&self.0.join(f))
        }
    }

    impl Drop for Pkg {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn open(s: &mut Server, uri: &str, text: &str) -> Vec<Value> {
        s.handle(json!({ "jsonrpc": "2.0", "method": "textDocument/didOpen", "params": { "textDocument": { "uri": uri, "text": text, "version": 1 } } }))
    }

    fn req(s: &mut Server, method: &str, uri: &str, line: u64, ch: u64) -> Value {
        let mut out = s.handle(json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": { "textDocument": { "uri": uri }, "position": { "line": line, "character": ch } } }));
        out.remove(0)["result"].clone()
    }

    fn diags_for(out: &[Value], uri: &str) -> Vec<String> {
        out.iter()
            .filter(|m| m["params"]["uri"] == uri)
            .flat_map(|m| m["params"]["diagnostics"].as_array().cloned().unwrap_or_default())
            .map(|d| d["message"].as_str().unwrap().to_string())
            .collect()
    }

    const MAIN: &str = "import std.io\nimport geometry\n\nmain :: fn() do\n  io.println(geometry.area(geometry.Circle(2)))\nend\n";

    #[test]
    fn imports_resolve_and_clean_files_have_no_diagnostics() {
        let p = Pkg::new("clean");
        let mut s = Server::new();
        let out = open(&mut s, &p.uri("src/main.cera"), MAIN);
        assert!(diags_for(&out, &p.uri("src/main.cera")).is_empty(), "{out:?}");
    }

    #[test]
    fn private_access_is_reported_with_help() {
        let p = Pkg::new("private");
        let mut s = Server::new();
        let src = MAIN.replace("geometry.area(geometry.Circle(2))", "geometry.helper(geometry.Circle(2))");
        let out = open(&mut s, &p.uri("src/main.cera"), &src);
        let d = diags_for(&out, &p.uri("src/main.cera"));
        assert_eq!(d.len(), 1, "{d:?}");
        assert!(d[0].contains("`helper` is private to module `geometry`") && d[0].contains("help: make it public"), "{}", d[0]);
    }

    #[test]
    fn unsaved_changes_in_another_module_are_used() {
        let p = Pkg::new("overlay");
        let mut s = Server::new();
        open(&mut s, &p.uri("src/geometry.cera"), "pub Shape :: type\n  | Circle(r: i32)\nend\n\npub area :: fn(s: Shape) -> i32 do\n  1\nend\n");
        // Remove `area` in the open buffer of geometry; main must notice.
        s.handle(json!({ "jsonrpc": "2.0", "method": "textDocument/didChange", "params": { "textDocument": { "uri": p.uri("src/geometry.cera") }, "contentChanges": [ { "text": "pub Shape :: type\n  | Circle(r: i32)\nend\n" } ] } }));
        let out = open(&mut s, &p.uri("src/main.cera"), MAIN);
        let d = diags_for(&out, &p.uri("src/main.cera"));
        assert!(d.iter().any(|m| m.contains("no member `area`")), "{d:?}");
    }

    #[test]
    fn errors_in_an_imported_file_land_on_that_file() {
        let p = Pkg::new("other");
        std::fs::write(p.0.join("src/geometry.cera"), "pub area :: fn() -> i32 do\n  \"no\"\nend\n").unwrap();
        let mut s = Server::new();
        let out = open(&mut s, &p.uri("src/main.cera"), "import geometry\nmain :: fn() do\n  x = geometry.area()\nend\n");
        assert!(!diags_for(&out, &p.uri("src/geometry.cera")).is_empty(), "{out:?}");
        assert!(diags_for(&out, &p.uri("src/main.cera")).is_empty());
    }

    #[test]
    fn definition_hover_and_completion_cross_modules() {
        let p = Pkg::new("nav");
        let mut s = Server::new();
        let uri = p.uri("src/main.cera");
        open(&mut s, &uri, MAIN);
        // `area` in `geometry.area(` on line 4, column 22.
        let def = req(&mut s, "textDocument/definition", &uri, 4, 22);
        assert_eq!(def["uri"], p.uri("src/geometry.cera"), "{def}");
        assert_eq!(def["range"]["start"]["line"], 4);
        // The module name itself jumps to the file.
        let def = req(&mut s, "textDocument/definition", &uri, 4, 14);
        assert_eq!(def["uri"], p.uri("src/geometry.cera"));
        let hover = req(&mut s, "textDocument/hover", &uri, 4, 22);
        assert!(hover["contents"]["value"].as_str().unwrap().contains("area :: fn(s: Shape) -> i32"), "{hover}");
        // `geometry.` offers the public members only.
        open(&mut s, &uri, "import geometry\nmain :: fn() do\n  geometry.\nend\n");
        let c = req(&mut s, "textDocument/completion", &uri, 2, 11);
        let labels: Vec<&str> = c["items"].as_array().unwrap().iter().map(|i| i["label"].as_str().unwrap()).collect();
        assert!(labels.contains(&"area") && labels.contains(&"Shape") && labels.contains(&"Circle"), "{labels:?}");
        assert!(!labels.contains(&"helper") && !labels.contains(&"fn"), "{labels:?}");
        // `import ` offers modules.
        open(&mut s, &uri, "import \n");
        let c = req(&mut s, "textDocument/completion", &uri, 0, 7);
        let labels: Vec<&str> = c["items"].as_array().unwrap().iter().map(|i| i["label"].as_str().unwrap()).collect();
        assert!(labels.contains(&"geometry") && labels.contains(&"std.io") && !labels.contains(&"main"), "{labels:?}");
    }

    #[test]
    fn a_missing_dependency_is_reported_not_fatal() {
        let p = Pkg::new("baddep");
        std::fs::write(p.0.join("kote.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\nghost = { path = \"../nowhere\" }\n").unwrap();
        let mut s = Server::new();
        let out = open(&mut s, &p.uri("src/main.cera"), "main :: fn() do\nend\n");
        let d = diags_for(&out, &p.uri("src/main.cera"));
        assert!(d.iter().any(|m| m.starts_with("kote:") && m.contains("ghost")), "{d:?}");
    }
}
