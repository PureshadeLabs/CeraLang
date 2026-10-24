//! The kote commands.

use crate::link::{link, Linked, Root};
use crate::manifest::{self, DepSource, GitSpec, Manifest};
use crate::registry;
use crate::resolve::{self, Graph, Options, PkgSource};
use crate::semver;
use crate::srcmap::SourceMap;
use cera_syntax::ast::{DeclKind, Item, TypeKind};
use cera_syntax::ast::Program;
use cera_syntax::Diagnostic;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::time::Instant;

pub type CmdResult = Result<i32, String>;

fn status(verb: &str, msg: impl AsRef<str>) {
    eprintln!("{verb:>12} {}", msg.as_ref());
}

pub struct Project {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

impl Project {
    /// The project containing the current directory (searching upwards).
    pub fn find() -> Result<Project, String> {
        let start = std::env::current_dir().map_err(|e| e.to_string())?;
        let mut dir = start.as_path();
        loop {
            if dir.join("kote.toml").is_file() {
                let manifest = Manifest::load(dir)?;
                return Ok(Project { dir: dir.to_path_buf(), manifest });
            }
            match dir.parent() {
                Some(p) => dir = p,
                None => {
                    return Err(format!(
                        "could not find `kote.toml` in {} or any parent directory\nhelp: create a project with `kote new <name>` or `kote init`",
                        start.display()
                    ))
                }
            }
        }
    }

    fn main_file(&self) -> PathBuf {
        self.dir.join("src").join("main.cera")
    }

    fn lib_file(&self) -> PathBuf {
        self.dir.join("src").join("lib.cera")
    }

    fn exe(&self) -> PathBuf {
        self.dir.join("target").join("bin").join(&self.manifest.name)
    }

    fn describe(&self) -> String {
        format!("{} v{} ({})", self.manifest.name, self.manifest.version, self.dir.display())
    }
}

// --------------------------------------------------------------------- flags

#[derive(Default, Clone)]
pub struct Flags {
    pub locked: bool,
    pub offline: bool,
    pub quiet: bool,
    /// Build for Lythos (`--target x86_64-lythos`) instead of Linux.
    pub lythos: bool,
}

impl Flags {
    fn options(&self) -> Options {
        Options { locked: self.locked, offline: self.offline, update: false, dry: false }
    }
}

// ------------------------------------------------------------------- new/init

fn hello() -> &'static str {
    "import std.io\n\nmain :: fn() do\n  io.println(\"Hello, world!\")\nend\n"
}

fn lib_template() -> &'static str {
    "# A library: other packages `import` this package by name.\n\npub add :: fn(a: i32, b: i32) -> i32 do\n  a + b\nend\n"
}

fn scaffold(dir: &Path, name: &str, lib: bool) -> Result<(), String> {
    manifest::valid_name(name)?;
    let w = |p: PathBuf, text: &str| std::fs::write(&p, text).map_err(|e| format!("cannot write {}: {e}", p.display()));
    std::fs::create_dir_all(dir.join("src")).map_err(|e| e.to_string())?;
    w(dir.join("kote.toml"), &manifest::template(name))?;
    if lib {
        w(dir.join("src").join("lib.cera"), lib_template())?;
    } else {
        w(dir.join("src").join("main.cera"), hello())?;
    }
    if !dir.join(".gitignore").exists() {
        w(dir.join(".gitignore"), "target/\n")?;
    }
    status("Created", format!("{} `{name}` package", if lib { "library" } else { "binary (application)" }));
    Ok(())
}

pub fn new(name: &str, lib: bool) -> CmdResult {
    let dir = PathBuf::from(name);
    let pkg = dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| name.to_string());
    if dir.exists() {
        return Err(format!("destination `{}` already exists\nhelp: use `kote init` to create a package in an existing directory", dir.display()));
    }
    manifest::valid_name(&pkg)?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    scaffold(&dir, &pkg, lib)?;
    Ok(0)
}

pub fn init(lib: bool) -> CmdResult {
    let dir = std::env::current_dir().map_err(|e| e.to_string())?;
    if dir.join("kote.toml").exists() {
        return Err("`kote.toml` already exists in this directory".into());
    }
    let raw = dir.file_name().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    let name: String = raw.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let name = if name.chars().next().is_some_and(|c| c.is_ascii_lowercase()) { name } else { format!("p{name}") };
    if dir.join("src").join(if lib { "lib.cera" } else { "main.cera" }).exists() {
        std::fs::write(dir.join("kote.toml"), manifest::template(&name)).map_err(|e| e.to_string())?;
        status("Created", format!("`{name}` package from existing sources"));
        return Ok(0);
    }
    scaffold(&dir, &name, lib)?;
    Ok(0)
}

// ------------------------------------------------------------------ front end

fn print_diags(map: &SourceMap, diags: &[Diagnostic]) {
    for d in diags {
        eprintln!("{}", map.render(d));
    }
}

struct Front {
    program: Program,
    map: SourceMap,
    has_main: bool,
}

fn fail_count(n: usize, name: &str) -> String {
    format!("could not compile `{name}` due to {n} previous error{}", if n == 1 { "" } else { "s" })
}

/// Resolve dependencies, link the modules and type-check the result.
fn front(p: &Project, graph: &Graph, root: Option<Root>) -> Result<Front, String> {
    let (root, has_main) = match root {
        Some(r) => (r, true),
        None if p.main_file().is_file() => (Root::File(p.main_file()), true),
        None if p.lib_file().is_file() => (Root::File(p.lib_file()), false),
        None => return Err("this package has neither `src/main.cera` nor `src/lib.cera`".into()),
    };
    let Linked { program, map, diagnostics, .. } = link(graph, root);
    if !diagnostics.is_empty() {
        print_diags(&map, &diagnostics);
        return Err(fail_count(diagnostics.len(), &p.manifest.name));
    }
    let an = cera_sema::analyze(&program);
    if !an.diagnostics.is_empty() {
        print_diags(&map, &an.diagnostics);
        return Err(fail_count(an.diagnostics.len(), &p.manifest.name));
    }
    Ok(Front { program, map, has_main })
}

fn announce(graph: &Graph) {
    // Dependencies first, the root package last.
    for pkg in graph.pkgs.iter().skip(1).rev() {
        let src = match &pkg.source {
            PkgSource::Git { url, commit, .. } => format!("{url}#{}", &commit[..7.min(commit.len())]),
            _ => pkg.dir.display().to_string(),
        };
        status("Compiling", format!("{} v{} ({src})", pkg.name, pkg.version));
    }
}

pub fn check(flags: &Flags) -> CmdResult {
    let p = Project::find()?;
    let t = Instant::now();
    let graph = resolve::resolve(&p.dir, flags.options())?;
    announce(&graph);
    status("Checking", p.describe());
    front(&p, &graph, None)?;
    status("Finished", format!("check in {:.2}s", t.elapsed().as_secs_f64()));
    Ok(0)
}

struct Compiled {
    exe: Option<PathBuf>,
    front: Front,
    /// Why the native backend could not compile the program.
    native_error: Option<String>,
}

/// Identifies what a native executable was built from: the linked program, the text of
/// every file (diagnostics embed source positions) and the compiler itself.
fn build_key(f: &Front) -> String {
    let mut text = format!("{:?}", f.program);
    for file in &f.map.files {
        text.push_str(&file.name);
        text.push('\0');
        text.push_str(&file.text);
        text.push('\0');
    }
    text.push_str(env!("CARGO_PKG_VERSION"));
    if let Ok(meta) = std::env::current_exe().and_then(std::fs::metadata) {
        let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        text.push_str(&format!("{}:{mtime}", meta.len()));
    }
    let hash = |seed: u64| {
        let mut h: u64 = seed;
        for b in text.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    };
    format!("{:016x}{:016x}", hash(0xcbf29ce484222325), hash(0x84222325cbf29ce4))
}

fn compile(p: &Project, graph: &Graph, root: Option<Root>, out: &Path, lythos: bool) -> Result<Compiled, String> {
    announce(graph);
    status("Compiling", p.describe());
    let f = front(p, graph, root)?;
    if !f.has_main {
        return Ok(Compiled { exe: None, front: f, native_error: None });
    }
    // Reuse the executable when neither the program nor the compiler changed.
    let key = format!("{}{}", build_key(&f), if lythos { ":lythos" } else { "" });
    let key_file = out.with_extension("key");
    if out.is_file() && std::fs::read_to_string(&key_file).ok().as_deref() == Some(key.as_str()) {
        status("Fresh", format!("{} is up to date", out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
        return Ok(Compiled { exe: Some(out.to_path_buf()), front: f, native_error: None });
    }
    let map = f.map.clone();
    let renderer: Rc<dyn Fn(&Diagnostic) -> String> = Rc::new(move |d| map.render(d));
    let files: Vec<(String, usize, String)> = f.map.files.iter().map(|s| (s.name.clone(), s.base, s.text.clone())).collect();
    match cera_native::compile_for(lythos, &f.program, &cera_sema::analyze(&f.program), Some(renderer), Some(files)) {
        Ok(bytes) => {
            std::fs::create_dir_all(out.parent().unwrap()).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_file(&key_file);
            std::fs::write(out, bytes).map_err(|e| format!("cannot write {}: {e}", out.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(out, std::fs::Permissions::from_mode(0o755));
            }
            let _ = std::fs::write(&key_file, &key);
            Ok(Compiled { exe: Some(out.to_path_buf()), front: f, native_error: None })
        }
        Err(e) => Ok(Compiled { exe: None, front: f, native_error: Some(e) }),
    }
}

pub fn build(flags: &Flags) -> CmdResult {
    let p = Project::find()?;
    let t = Instant::now();
    let graph = resolve::resolve(&p.dir, flags.options())?;
    let exe = if flags.lythos { p.dir.join("target").join("lythos").join("bin").join(&p.manifest.name) } else { p.exe() };
    let c = compile(&p, &graph, None, &exe, flags.lythos)?;
    if let Some(e) = &c.native_error {
        return Err(format!("the native backend cannot build this program: {e}\nhelp: `kote run` falls back to the bytecode VM"));
    }
    status("Finished", format!("build in {:.2}s", t.elapsed().as_secs_f64()));
    if let Some(exe) = &c.exe {
        status("Output", exe.strip_prefix(&p.dir).unwrap_or(exe).display().to_string());
    }
    Ok(0)
}

fn run_on_vm(f: &Front, args: Vec<String>) -> CmdResult {
    let an = cera_sema::analyze(&f.program);
    let r = cera_eval::run_program(&f.program, &an, cera_eval::RunConfig::stdio(args), cera_eval::Engine::from_env());
    match r {
        Ok(code) => Ok(code),
        Err(cera_eval::RunError::Runtime(e)) => {
            match e.span.filter(|s| !s.is_empty()) {
                Some(span) => {
                    let mut d = Diagnostic::new(span, format!("runtime error: {}", e.message));
                    if let Some(h) = cera_syntax::runtime_help(&e.message) {
                        d = d.with_help(h);
                    }
                    eprintln!("{}", f.map.render(&d));
                }
                None => eprintln!("runtime error: {}", e.message),
            }
            Ok(1)
        }
        Err(cera_eval::RunError::Syntax(d)) | Err(cera_eval::RunError::Type(d)) => {
            print_diags(&f.map, &d);
            Ok(1)
        }
    }
}

pub fn run(flags: &Flags, args: Vec<String>) -> CmdResult {
    let p = Project::find()?;
    let t = Instant::now();
    let graph = resolve::resolve(&p.dir, flags.options())?;
    let c = compile(&p, &graph, None, &p.exe(), false)?;
    if !c.front.has_main {
        return Err("this package is a library (`src/lib.cera`); there is nothing to run\nhelp: add a `src/main.cera`, or use `kote check`".into());
    }
    status("Finished", format!("build in {:.2}s", t.elapsed().as_secs_f64()));
    match (&c.exe, &c.native_error) {
        (Some(exe), _) => {
            status("Running", format!("`{}`", exe.strip_prefix(&p.dir).unwrap_or(exe).display()));
            let st = Command::new(exe).args(&args).status().map_err(|e| format!("cannot run {}: {e}", exe.display()))?;
            Ok(st.code().unwrap_or(1))
        }
        (None, Some(why)) => {
            status("Note", format!("native backend unavailable ({why}); running on the VM"));
            status("Running", "on the bytecode VM");
            run_on_vm(&c.front, args)
        }
        _ => unreachable!(),
    }
}

// ----------------------------------------------------------------------- test

struct TestCase {
    /// `file.test_name`
    id: String,
    module_alias: String,
    func: String,
    returns_result: bool,
}

fn discover_tests(dir: &Path) -> Result<(Vec<String>, Vec<TestCase>), String> {
    discover(dir, "tests", "test_")
}

/// Functions named `<prefix>*` with no parameters in `<dir>/<folder>/*.cera`.
fn discover(dir: &Path, folder: &str, prefix: &str) -> Result<(Vec<String>, Vec<TestCase>), String> {
    let tests_dir = dir.join(folder);
    let mut files: Vec<PathBuf> = match std::fs::read_dir(&tests_dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "cera")).collect(),
        Err(_) => vec![],
    };
    files.sort();
    let mut modules = vec![];
    let mut cases = vec![];
    for (i, f) in files.iter().enumerate() {
        let stem = f.file_stem().unwrap().to_string_lossy().into_owned();
        manifest::valid_name(&stem).map_err(|e| format!("{folder}/{stem}.cera: {e}"))?;
        let text = std::fs::read_to_string(f).map_err(|e| format!("cannot read {}: {e}", f.display()))?;
        let parsed = cera_syntax::parse(&text);
        modules.push(stem.clone());
        for item in &parsed.program.items {
            let Item::Decl(d) = item else { continue };
            let DeclKind::Fn(func) = &d.kind else { continue };
            if !d.name.name.starts_with(prefix) || !func.params.is_empty() {
                continue;
            }
            let returns_result = matches!(&func.ret, Some(t) if matches!(&t.kind, TypeKind::Path { path, .. } if path.last().is_some_and(|p| p.name == "Result")));
            cases.push(TestCase { id: format!("{stem}.{}", d.name.name), module_alias: format!("t{i}"), func: d.name.name.clone(), returns_result });
        }
    }
    Ok((modules, cases))
}

fn harness(modules: &[String], cases: &[TestCase]) -> String {
    let mut s = String::from("import std.io\nimport std.env\nimport std.process\n");
    for (i, m) in modules.iter().enumerate() {
        s.push_str(&format!("import tests.{m} as t{i}\n"));
    }
    s.push_str("\nrun :: fn(name: String) -> Result<(), String> do\n");
    let mut first = true;
    for c in cases {
        s.push_str(&format!("  {} name == \"{}\" do\n", if first { "if" } else { "else if" }, c.id));
        first = false;
        if c.returns_result {
            s.push_str(&format!("    {}.{}()\n", c.module_alias, c.func));
        } else {
            s.push_str(&format!("    {}.{}()\n    Ok(())\n", c.module_alias, c.func));
        }
    }
    if first {
        s.push_str("  Err(\"no tests\")\n");
    } else {
        s.push_str("  else\n    Err(String.concat([\"unknown test \", name]))\n  end\n");
    }
    s.push_str(
        "end\n\nreport :: fn(r: Result<(), String>) do\n  match r do\n    Ok(_) -> ()\n    Err(msg) -> do\n      io.println(msg)\n      process.exit(1)\n    end\n  end\nend\n\nmain :: fn(caps: Caps) do\n  match env.args(caps.env) do\n    [name | _] -> report(run(name))\n    _ -> process.exit(2)\n  end\nend\n",
    );
    s
}

pub fn test(flags: &Flags, filter: Option<String>) -> CmdResult {
    let p = Project::find()?;
    let t0 = Instant::now();
    let graph = resolve::resolve(&p.dir, flags.options())?;
    let (modules, mut cases) = discover_tests(&p.dir)?;
    if let Some(f) = &filter {
        cases.retain(|c| c.id.contains(f.as_str()));
    }
    if cases.is_empty() {
        status("Finished", "no tests found (tests live in `tests/*.cera` as functions named `test_*`)");
        eprintln!("\nrunning 0 tests\n\ntest result: ok. 0 passed; 0 failed");
        return Ok(0);
    }
    let text = harness(&modules, &cases);
    let exe = p.dir.join("target").join("bin").join(format!("{}-tests", p.manifest.name));
    let c = compile(&p, &graph, Some(Root::Text { name: "<kote test harness>".into(), text }), &exe, false)?;
    status("Finished", format!("test build in {:.2}s", t0.elapsed().as_secs_f64()));
    eprintln!("\nrunning {} test{}", cases.len(), if cases.len() == 1 { "" } else { "s" });
    let mut failed: Vec<(String, String)> = vec![];
    for case in &cases {
        let (code, out) = match (&c.exe, &c.native_error) {
            (Some(exe), _) => {
                let o = Command::new(exe).arg(&case.id).output().map_err(|e| format!("cannot run {}: {e}", exe.display()))?;
                let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
                text.push_str(&String::from_utf8_lossy(&o.stderr));
                (o.status.code().unwrap_or(1), text)
            }
            _ => {
                let an = cera_sema::analyze(&c.front.program);
                let (out, err) = (cera_eval::SharedBuf::default(), cera_eval::SharedBuf::default());
                let cfg = cera_eval::RunConfig {
                    out: Box::new(out.clone()),
                    err: Box::new(err.clone()),
                    input: Box::new(std::io::Cursor::new(Vec::new())),
                    args: vec![case.id.clone()],
                    fs_root: std::env::current_dir().unwrap_or_default(),
                };
                match cera_eval::run_program(&c.front.program, &an, cfg, cera_eval::Engine::from_env()) {
                    Ok(code) => (code, format!("{}{}", out.contents(), err.contents())),
                    Err(cera_eval::RunError::Runtime(e)) => (1, format!("{}{}runtime error: {}\n", out.contents(), err.contents(), e.message)),
                    Err(_) => (1, "internal error".into()),
                }
            }
        };
        if code == 0 {
            eprintln!("test {} ... ok", case.id);
        } else {
            eprintln!("test {} ... FAILED", case.id);
            failed.push((case.id.clone(), c.front.map.clean(out.trim_end())));
        }
    }
    if !failed.is_empty() {
        eprintln!("\nfailures:");
        for (id, out) in &failed {
            eprintln!("\n---- {id} ----\n{out}");
        }
        eprintln!("\nfailures:");
        for (id, _) in &failed {
            eprintln!("    {id}");
        }
    }
    let passed = cases.len() - failed.len();
    eprintln!("\ntest result: {}. {passed} passed; {} failed", if failed.is_empty() { "ok" } else { "FAILED" }, failed.len());
    Ok(if failed.is_empty() { 0 } else { 1 })
}

// --------------------------------------------------------------- add / remove

pub struct AddArgs {
    pub name: Option<String>,
    pub path: Option<String>,
    pub git: Option<String>,
    pub rev: Option<String>,
    pub tag: Option<String>,
    pub branch: Option<String>,
    pub version: Option<String>,
    pub registry: Option<String>,
}

pub fn add(flags: &Flags, a: AddArgs) -> CmdResult {
    let p = Project::find()?;
    let path = p.dir.join("kote.toml");
    let original = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut doc: toml_edit::DocumentMut = original.parse().map_err(|e| format!("{}: {e}", path.display()))?;
    let (given, at_req) = match a.name.as_deref().and_then(|n| n.split_once('@')) {
        Some((n, r)) => (Some(n.to_string()), Some(r.to_string())),
        None => (a.name.clone(), None),
    };
    let name = match (&given, &a.path) {
        (Some(n), _) => n.clone(),
        (None, Some(dir)) => Manifest::load(&p.dir.join(dir)).map(|m| m.name)?,
        _ => return Err("usage: kote add <NAME>[@REQ] [--path <DIR> | --git <URL> [--rev R | --tag T | --branch B]]".into()),
    };
    manifest::valid_name(&name).map_err(|e| format!("dependency {e}"))?;
    let mut entry = toml_edit::InlineTable::new();
    match (&a.path, &a.git) {
        (Some(pth), None) => {
            entry.insert("path", pth.as_str().into());
        }
        (None, Some(url)) => {
            entry.insert("git", url.as_str().into());
            for (k, v) in [("rev", &a.rev), ("tag", &a.tag), ("branch", &a.branch)] {
                if let Some(v) = v {
                    entry.insert(k, v.as_str().into());
                }
            }
        }
        (None, None) => {
            // A registry dependency: `name@REQ`, or the newest published version.
            let reg = registry::open(a.registry.as_deref(), &p.dir);
            reg.sync(&name, false)?;
            let req = match at_req.or(a.version.clone()) {
                Some(r) => {
                    semver::Req::parse(&r)?;
                    r
                }
                None => {
                    let newest = reg.versions(&name, &[]).pop().ok_or_else(|| {
                        format!("no package named `{name}` in the registry at {}\nhelp: give a source with `--path` or `--git`", reg.describe())
                    })?;
                    format!("^{newest}")
                }
            };
            entry.insert("version", req.as_str().into());
            if let Some(r) = &a.registry {
                entry.insert("registry", r.as_str().into());
            }
        }
        _ => return Err("give at most one of `--path` and `--git`".into()),
    }
    if doc.get("dependencies").is_none() {
        doc["dependencies"] = toml_edit::table();
    }
    let existed = doc["dependencies"].get(&name).is_some();
    // `version` alone is written in the short form: `name = "^1.2"`.
    let only_version = entry.len() == 1 && entry.contains_key("version");
    doc["dependencies"][&name] = if only_version { toml_edit::value(entry.get("version").and_then(|v| v.as_str()).unwrap_or("")) } else { toml_edit::value(entry) };
    std::fs::write(&path, doc.to_string()).map_err(|e| e.to_string())?;
    status(if existed { "Updating" } else { "Adding" }, format!("`{name}` to dependencies"));
    match resolve::resolve(&p.dir, Options { update: true, ..flags.options() }) {
        Ok(_) => Ok(0),
        Err(e) => {
            let _ = std::fs::write(&path, original);
            Err(e)
        }
    }
}

pub fn remove(flags: &Flags, name: &str) -> CmdResult {
    let p = Project::find()?;
    let path = p.dir.join("kote.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{}: {e}", path.display()))?;
    let removed = doc.get_mut("dependencies").and_then(|d| d.as_table_like_mut()).and_then(|t| t.remove(name));
    if removed.is_none() {
        return Err(format!("the dependency `{name}` could not be found in `kote.toml`"));
    }
    std::fs::write(&path, doc.to_string()).map_err(|e| e.to_string())?;
    status("Removing", format!("`{name}` from dependencies"));
    resolve::resolve(&p.dir, flags.options())?;
    Ok(0)
}

// ------------------------------------------------------------- tree/update/clean

pub fn tree(flags: &Flags) -> CmdResult {
    let p = Project::find()?;
    let g = resolve::resolve(&p.dir, flags.options())?;
    fn label(g: &Graph, i: usize) -> String {
        let k = &g.pkgs[i];
        match &k.source {
            PkgSource::Root => format!("{} v{} ({})", k.name, k.version, k.dir.display()),
            PkgSource::Path => format!("{} v{} ({})", k.name, k.version, k.dir.display()),
            PkgSource::Git { url, commit, .. } => format!("{} v{} (git+{url}#{})", k.name, k.version, &commit[..7.min(commit.len())]),
            PkgSource::Registry { .. } => format!("{} v{} (registry)", k.name, k.version),
        }
    }
    fn walk(g: &Graph, i: usize, prefix: &str, out: &mut String) {
        let deps = &g.pkgs[i].deps;
        for (n, (_, d)) in deps.iter().enumerate() {
            let last = n + 1 == deps.len();
            out.push_str(&format!("{prefix}{}── {}\n", if last { "└" } else { "├" }, label(g, *d)));
            walk(g, *d, &format!("{prefix}{}   ", if last { " " } else { "│" }), out);
        }
    }
    let mut out = format!("{}\n", label(&g, 0));
    walk(&g, 0, "", &mut out);
    print!("{out}");
    Ok(0)
}

pub fn update(flags: &Flags, only: Option<String>) -> CmdResult {
    let p = Project::find()?;
    if let Some(n) = &only {
        if !p.manifest.deps.iter().any(|d| &d.name == n) {
            return Err(format!("`{n}` is not a dependency of this package"));
        }
    }
    let before = std::fs::read_to_string(p.dir.join("kote.lock")).unwrap_or_default();
    let g = resolve::resolve(&p.dir, Options { update: true, ..flags.options() })?;
    let after = resolve::render_lock(&g);
    if before == after {
        status("Updating", "nothing to update");
    } else {
        status("Updated", "kote.lock");
    }
    Ok(0)
}

pub fn clean() -> CmdResult {
    let p = Project::find()?;
    let target = p.dir.join("target");
    if target.exists() {
        std::fs::remove_dir_all(&target).map_err(|e| format!("cannot remove {}: {e}", target.display()))?;
    }
    status("Removed", "target");
    Ok(0)
}

pub fn dep_summary(p: &Project) -> Vec<String> {
    p.manifest
        .deps
        .iter()
        .map(|d| match &d.source {
            DepSource::Path(x) => format!("{} (path {x})", d.name),
            DepSource::Registry { req, .. } => format!("{} (registry {req})", d.name),
            DepSource::Git { url, spec } => format!("{} (git {url}{})", d.name, if *spec == GitSpec::Default { String::new() } else { format!(" {}", spec.query().unwrap()) }),
        })
        .collect()
}

// ------------------------------------------------------------------------ fmt

fn cera_files_in(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            cera_files_in(&p, out);
        } else if p.extension().is_some_and(|x| x == "cera") {
            out.push(p);
        }
    }
}

/// Format every `.cera` file under `src/` and `tests/`. With `check`, change nothing
/// and fail if any file would change.
pub fn fmt(check: bool) -> CmdResult {
    let p = Project::find()?;
    let mut files = vec![];
    cera_files_in(&p.dir.join("src"), &mut files);
    cera_files_in(&p.dir.join("tests"), &mut files);
    let (mut changed, mut failed) = (0, 0);
    for f in &files {
        let shown = f.strip_prefix(&p.dir).unwrap_or(f).display().to_string();
        let src = std::fs::read_to_string(f).map_err(|e| format!("cannot read {shown}: {e}"))?;
        match cera_syntax::format_source(&src) {
            Ok(out) if out == src => {}
            Ok(out) => {
                changed += 1;
                if check {
                    eprintln!("would reformat {shown}");
                } else {
                    std::fs::write(f, out).map_err(|e| format!("cannot write {shown}: {e}"))?;
                    status("Formatted", &shown);
                }
            }
            Err(e) => {
                failed += 1;
                eprintln!("error: {shown}:{e}");
            }
        }
    }
    if failed > 0 {
        return Err(format!("{failed} file{} could not be formatted", if failed == 1 { "" } else { "s" }));
    }
    if check && changed > 0 {
        return Err(format!("{changed} file{} would be reformatted; run `kote fmt`", if changed == 1 { "" } else { "s" }));
    }
    Ok(0)
}

pub fn publish(registry_flag: Option<String>) -> CmdResult {
    let p = Project::find()?;
    let reg = registry::open(registry_flag.as_deref(), &std::env::current_dir().map_err(|e| e.to_string())?);
    let (name, version, dest) = registry::publish(&p.dir, &reg)?;
    status("Published", format!("{name} v{version} to {dest}"));
    Ok(0)
}

/// Yank or un-yank `name@version`.
pub fn yank(spec: Option<String>, registry_flag: Option<String>, undo: bool) -> CmdResult {
    let spec = spec.ok_or("usage: kote yank <name>@<version> [--undo] [--registry R]")?;
    let (name, ver) = spec.split_once('@').ok_or("give the version to yank as `name@version`")?;
    let version = semver::Version::parse(ver).map_err(|e| format!("version `{ver}`: {e}"))?;
    let reg = registry::open(registry_flag.as_deref(), &std::env::current_dir().map_err(|e| e.to_string())?);
    registry::yank(&reg, name, &version, undo)?;
    status(if undo { "Unyanked" } else { "Yanked" }, format!("{name} v{version}"));
    Ok(0)
}

/// Generate HTML documentation for the public items of `lib.cera` and the other modules.
pub fn doc() -> CmdResult {
    let p = Project::find()?;
    let src = p.dir.join("src");
    let mut files = vec![];
    cera_files_in(&src, &mut files);
    let mut mods = vec![];
    for f in &files {
        let rel = f.strip_prefix(&src).unwrap_or(f).with_extension("");
        let mut parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        if parts.last().is_some_and(|l| l == "mod") {
            parts.pop();
        }
        let name = if parts == ["main"] {
            continue;
        } else if parts == ["lib"] {
            p.manifest.name.clone()
        } else {
            parts.join(".")
        };
        let text = std::fs::read_to_string(f).map_err(|e| format!("cannot read {}: {e}", f.display()))?;
        mods.push(crate::doc::collect(&name, &text)?);
    }
    // The package's own module first, the rest in path order.
    mods.sort_by(|a, b| (a.name != p.manifest.name, &a.name).cmp(&(b.name != p.manifest.name, &b.name)));
    let out = p.dir.join("target").join("doc").join(&p.manifest.name);
    std::fs::create_dir_all(&out).map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    for (name, html) in crate::doc::render(&p.manifest.name, &p.manifest.version, &mods) {
        std::fs::write(out.join(&name), html).map_err(|e| format!("cannot write {name}: {e}"))?;
    }
    let n: usize = mods.iter().map(|m| m.items.len()).sum();
    status("Documented", format!("{n} public item{} in {} module{}", if n == 1 { "" } else { "s" }, mods.len(), if mods.len() == 1 { "" } else { "s" }));
    status("Generated", out.join("index.html").display().to_string());
    Ok(0)
}

// ---------------------------------------------------------------------- bench

fn bench_harness(modules: &[String], cases: &[TestCase]) -> String {
    let mut s = String::from("import std.io\nimport std.env\nimport std.process\nimport std.time\n");
    for (i, m) in modules.iter().enumerate() {
        s.push_str(&format!("import benches.{m} as t{i}\n"));
    }
    for (n, c) in cases.iter().enumerate() {
        // One timed run of `iters` calls, in milliseconds.
        s.push_str(&format!(
            "\nrun_{n} :: fn(clock: Clock, iters: i64) -> i64 do\n  start = time.now_ms(clock)\n  var k = 0\n  while k < iters do\n    {}.{}()\n    k := k + 1\n  end\n  time.now_ms(clock) - start\nend\n",
            c.module_alias, c.func
        ));
        // Find an iteration count that takes at least 100 ms, then take five samples.
        s.push_str(&format!(
            "\nmeasure_{n} :: fn(clock: Clock) do\n  var iters = 1\n  var ms = 0\n  while true do\n    ms := run_{n}(clock, iters)\n    if ms >= 100 do\n      break\n    end\n    iters := iters * 4\n  end\n  a = run_{n}(clock, iters)\n  b = run_{n}(clock, iters)\n  c = run_{n}(clock, iters)\n  d = run_{n}(clock, iters)\n  io.println(String.concat([show(iters), \" \", show(ms), \" \", show(a), \" \", show(b), \" \", show(c), \" \", show(d)]))\nend\n"
        ));
    }
    s.push_str("\nmain :: fn(caps: Caps) do\n  match env.args(caps.env) do\n    [name | _] -> do\n");
    let mut first = true;
    for (n, c) in cases.iter().enumerate() {
        s.push_str(&format!("      {} name == \"{}\" do\n        measure_{n}(caps.clock)\n", if first { "if" } else { "else if" }, c.id));
        first = false;
    }
    if !first {
        s.push_str("      else\n        process.exit(2)\n      end\n");
    }
    s.push_str("    end\n    _ -> process.exit(2)\n  end\nend\n");
    s
}

fn human_time(ns: f64) -> String {
    if ns >= 1e9 {
        format!("{:.2} s", ns / 1e9)
    } else if ns >= 1e6 {
        format!("{:.2} ms", ns / 1e6)
    } else if ns >= 1e3 {
        format!("{:.2} us", ns / 1e3)
    } else {
        format!("{ns:.1} ns")
    }
}

/// Run the `bench_*` functions in `benches/*.cera` natively and report time per call.
pub fn bench(flags: &Flags, filter: Option<String>) -> CmdResult {
    let p = Project::find()?;
    let graph = resolve::resolve(&p.dir, flags.options())?;
    let (modules, mut cases) = discover(&p.dir, "benches", "bench_")?;
    if let Some(f) = &filter {
        cases.retain(|c| c.id.contains(f.as_str()));
    }
    if cases.is_empty() {
        status("Finished", "no benchmarks found (they live in `benches/*.cera` as functions named `bench_*`)");
        return Ok(0);
    }
    let text = bench_harness(&modules, &cases);
    let exe = p.dir.join("target").join("bin").join(format!("{}-bench", p.manifest.name));
    let c = compile(&p, &graph, Some(Root::Text { name: "<kote bench harness>".into(), text }), &exe, false)?;
    let Some(exe) = &c.exe else {
        return Err(format!(
            "benchmarks need the native backend, which is unavailable: {}",
            c.native_error.as_deref().unwrap_or("unknown reason")
        ));
    };
    eprintln!("\nrunning {} benchmark{}", cases.len(), if cases.len() == 1 { "" } else { "s" });
    for case in &cases {
        let o = Command::new(exe).arg(&case.id).output().map_err(|e| format!("cannot run {}: {e}", exe.display()))?;
        let out = String::from_utf8_lossy(&o.stdout).into_owned();
        let nums: Vec<f64> = out.split_whitespace().filter_map(|w| w.parse().ok()).collect();
        if !o.status.success() || nums.len() != 6 {
            eprintln!("bench {} ... FAILED", case.id);
            eprintln!("{}{}", out, String::from_utf8_lossy(&o.stderr));
            return Ok(1);
        }
        let iters = nums[0].max(1.0);
        let mut per: Vec<f64> = nums[2..].iter().map(|ms| ms * 1e6 / iters).collect();
        per.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = per[per.len() / 2];
        eprintln!("bench {:<32} {:>12}/iter  (min {}, max {}, {} iterations x 5 samples)", case.id, human_time(median), human_time(per[0]), human_time(per[per.len() - 1]), nums[0] as u64);
    }
    Ok(0)
}
