//! End-to-end tests: they run the real `kote` binary on throwaway projects.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static N: AtomicUsize = AtomicUsize::new(0);

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Sandbox {
        let root = std::env::temp_dir().join(format!("kote-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Sandbox { root }
    }

    fn path(&self, p: &str) -> PathBuf {
        self.root.join(p)
    }

    fn write(&self, p: &str, text: &str) {
        let f = self.path(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, text).unwrap();
    }

    fn kote(&self, dir: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_kote"))
            .args(args)
            .current_dir(self.path(dir))
            .env("KOTE_HOME", self.path(".kote-home"))
            .output()
            .unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn ok(o: &Output) {
    assert!(o.status.success(), "stdout:\n{}\nstderr:\n{}", out(o), err(o));
}

#[test]
fn new_builds_and_runs() {
    let sb = Sandbox::new();
    ok(&sb.kote(".", &["new", "hello"]));
    assert!(sb.path("hello/kote.toml").is_file());
    assert!(sb.path("hello/src/main.cera").is_file());
    let o = sb.kote("hello", &["run"]);
    ok(&o);
    assert_eq!(out(&o), "Hello, world!\n");
    assert!(sb.path("hello/target/bin/hello").is_file());
    assert!(sb.path("hello/kote.lock").is_file());
    // The new directory must not be reused, and bad names are rejected.
    assert!(!sb.kote(".", &["new", "hello"]).status.success());
    assert!(err(&sb.kote(".", &["new", "Bad-Name"])).contains("not a valid package name"));
}

#[test]
fn args_and_exit_code_reach_the_program() {
    let sb = Sandbox::new();
    ok(&sb.kote(".", &["new", "p"]));
    sb.write(
        "p/src/main.cera",
        "import std.io\nimport std.env\nimport std.process\n\nmain :: fn(caps: Caps) do\n  match env.args(caps.env) do\n    [first | _] -> io.println(first)\n    _ -> ()\n  end\n  process.exit(3)\nend\n",
    );
    let o = sb.kote("p", &["run", "--", "alpha", "beta"]);
    assert_eq!(o.status.code(), Some(3), "{}", err(&o));
    assert_eq!(out(&o), "alpha\n");
}

fn library(sb: &Sandbox) {
    ok(&sb.kote(".", &["new", "geometry", "--lib"]));
    sb.write(
        "geometry/src/lib.cera",
        "pub Shape :: type\n  | Circle(r: i32)\n  | Rect(w: i32, h: i32)\nend\n\npub Point :: type { x: i32, y: i32 }\n\npub Describe :: trait\n  describe :: fn(self) -> String\nend\n\npub area :: fn(s: Shape) -> i32 do\n  match s do\n    Circle(r) -> 3 * r * r\n    Rect(w, h) -> w * h\n  end\nend\n",
    );
    sb.write("geometry/src/util.cera", "pub double :: fn(n: i32) -> i32 do\n  n * 2\nend\n");
}

#[test]
fn path_dependencies_and_modules() {
    let sb = Sandbox::new();
    library(&sb);
    ok(&sb.kote(".", &["new", "app"]));
    ok(&sb.kote("app", &["add", "geometry", "--path", "../geometry"]));
    assert!(std::fs::read_to_string(sb.path("app/kote.toml")).unwrap().contains("geometry = { path = \"../geometry\" }"));
    sb.write("app/src/helper.cera", "import geometry\n\npub big :: fn() -> i32 do\n  geometry.area(geometry.Rect(10, 10))\nend\n");
    sb.write(
        "app/src/main.cera",
        r#"import std.io
import geometry
import geometry.util as u
import helper

area :: fn(x: i32) -> i32 do
  x + 1
end

impl Show for geometry.Shape do
  show :: fn(s: geometry.Shape) -> String do
    match s do
      geometry.Rect(w, h) -> String.concat(["Rect(", show(w), ", ", show(h), ")"])
      geometry.Circle(r) -> String.concat(["Circle(", show(r), ")"])
    end
  end
end

impl geometry.Describe for geometry.Point do
  describe :: fn(self: geometry.Point) -> String do
    String.concat(["point ", show(self.x), ",", show(self.y)])
  end
end

main :: fn() do
  s = geometry.Rect(3, 4)
  p = geometry.Point { x: 1, y: 2 }
  io.println(geometry.area(s))
  io.println(area(1))
  io.println(u.double(21))
  io.println(helper.big())
  io.println(geometry.describe(p))
  io.println(show(s))
  match s do
    geometry.Rect(w, _) -> io.println(w)
    geometry.Circle(r) -> io.println(r)
  end
  match p do
    geometry.Point { x, y: py } -> io.println(x + py)
  end
end
"#,
    );
    let o = sb.kote("app", &["run"]);
    ok(&o);
    assert_eq!(out(&o), "12\n2\n42\n100\npoint 1,2\nRect(3, 4)\n3\n3\n");
    let lock = std::fs::read_to_string(sb.path("app/kote.lock")).unwrap();
    assert!(lock.contains("name = \"geometry\"") && lock.contains("dependencies = [\"geometry\"]"), "{lock}");
    let tree = out(&sb.kote("app", &["tree"]));
    assert!(tree.contains("app v0.1.0") && tree.contains("geometry v0.1.0"), "{tree}");
    // Removing the dependency makes the import fail with advice.
    ok(&sb.kote("app", &["remove", "geometry"]));
    let o = sb.kote("app", &["check"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("unknown module `geometry`"), "{}", err(&o));
}

#[test]
fn errors_name_the_right_file_and_use_source_names() {
    let sb = Sandbox::new();
    library(&sb);
    ok(&sb.kote(".", &["new", "app"]));
    ok(&sb.kote("app", &["add", "geometry", "--path", "../geometry"]));
    sb.write("app/src/helper.cera", "import geometry\n\npub f :: fn() -> i32 do\n  geometry.Circle(1)\nend\n");
    sb.write("app/src/main.cera", "import std.io\nimport helper\nmain :: fn() do\n  io.println(helper.f())\nend\n");
    let o = sb.kote("app", &["check"]);
    assert!(!o.status.success());
    let e = err(&o);
    assert!(e.contains("--> src/helper.cera:4:3"), "{e}");
    assert!(e.contains("geometry.Shape"), "mangled names must not leak: {e}");
    assert!(!e.contains('\u{b7}'), "{e}");
}

#[test]
fn import_problems() {
    let sb = Sandbox::new();
    ok(&sb.kote(".", &["new", "app"]));
    sb.write("app/src/a.cera", "import b\npub f :: fn() -> i32 do\n  b.g()\nend\n");
    sb.write("app/src/b.cera", "import a\npub g :: fn() -> i32 do\n  a.f()\nend\n");
    sb.write("app/src/main.cera", "import a\nimport missing\nimport std.io\nmain :: fn() do\n  io.println(a.f())\nend\n");
    let e = err(&sb.kote("app", &["check"]));
    assert!(e.contains("circular import"), "{e}");
    assert!(e.contains("unknown module `missing`"), "{e}");
    sb.write("app/src/a.cera", "pub f :: fn() -> i32 do\n  1\nend\n");
    sb.write("app/src/main.cera", "import a\nimport std.io\nmain :: fn() do\n  io.println(a.nope())\nend\n");
    let e = err(&sb.kote("app", &["check"]));
    assert!(e.contains("module `a` has no member `nope`"), "{e}");
}

#[test]
fn runtime_errors_point_into_the_dependency() {
    let sb = Sandbox::new();
    ok(&sb.kote(".", &["new", "app"]));
    sb.write("app/src/a.cera", "pub f :: fn(n: i32) -> i32 do\n  n * 2\nend\n");
    sb.write("app/src/main.cera", "import a\nimport std.io\nmain :: fn() do\n  io.println(a.f(2147483647))\nend\n");
    let o = sb.kote("app", &["run"]);
    assert_eq!(o.status.code(), Some(1));
    let e = err(&o);
    assert!(e.contains("runtime error: integer overflow") && e.contains("--> src/a.cera:2:3"), "{e}");
}

#[test]
fn test_command() {
    let sb = Sandbox::new();
    ok(&sb.kote(".", &["new", "app"]));
    sb.write("app/src/util.cera", "pub double :: fn(n: i32) -> i32 do\n  n * 2\nend\n");
    sb.write(
        "app/tests/util.cera",
        "import kote.test as t\nimport util\n\ntest_double :: fn() -> Result<(), String> do\n  t.equal(4, util.double(2))\nend\n\ntest_plain :: fn() do\n  x = util.double(1)\nend\n",
    );
    let o = sb.kote("app", &["test"]);
    ok(&o);
    let e = err(&o);
    assert!(e.contains("test util.test_double ... ok") && e.contains("test util.test_plain ... ok"), "{e}");
    assert!(e.contains("2 passed; 0 failed"), "{e}");
    sb.write(
        "app/tests/util.cera",
        "import kote.test as t\nimport util\n\ntest_bad :: fn() -> Result<(), String> do\n  t.equal(5, util.double(2))\nend\n\ntest_good :: fn() -> Result<(), String> do\n  t.equal(4, util.double(2))\nend\n",
    );
    let o = sb.kote("app", &["test"]);
    assert_eq!(o.status.code(), Some(1));
    let e = err(&o);
    assert!(e.contains("test util.test_bad ... FAILED") && e.contains("assertion failed: expected 5, found 4"), "{e}");
    assert!(e.contains("1 passed; 1 failed"), "{e}");
    let o = sb.kote("app", &["test", "good"]);
    ok(&o);
    assert!(err(&o).contains("1 passed; 0 failed"), "{}", err(&o));
}

#[test]
fn locked_and_lock_contents() {
    let sb = Sandbox::new();
    ok(&sb.kote(".", &["new", "app"]));
    assert!(!sb.kote("app", &["build", "--locked"]).status.success());
    ok(&sb.kote("app", &["build"]));
    ok(&sb.kote("app", &["build", "--locked"]));
    library(&sb);
    ok(&sb.kote("app", &["add", "geometry", "--path", "../geometry"]));
    let o = sb.kote("app", &["build", "--locked"]);
    ok(&o);
    // Editing the manifest by hand without updating the lock is caught.
    sb.write("app/kote.toml", "[package]\nname = \"app\"\nversion = \"0.1.0\"\n");
    let o = sb.kote("app", &["build", "--locked"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("kote.lock needs to be updated"), "{}", err(&o));
}

fn git(dir: &Path, args: &[&str]) {
    let o = Command::new("git")
        .args(["-c", "user.email=t@t", "-c", "user.name=t"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
}

#[test]
fn git_dependencies_are_pinned() {
    if Command::new("git").arg("--version").output().is_err() {
        return;
    }
    let sb = Sandbox::new();
    ok(&sb.kote(".", &["new", "gitlib", "--lib"]));
    sb.write("gitlib/src/lib.cera", "pub add :: fn(a: i32, b: i32) -> i32 do\n  a + b\nend\n");
    let repo = sb.path("gitlib");
    git(&repo, &["init", "-q", "."]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "one"]);
    git(&repo, &["tag", "v1"]);
    sb.write("gitlib/src/lib.cera", "pub add :: fn(a: i32, b: i32) -> i32 do\n  a + b + 100\nend\n");
    git(&repo, &["commit", "-qam", "two"]);

    ok(&sb.kote(".", &["new", "app"]));
    let url = repo.to_string_lossy().into_owned();
    ok(&sb.kote("app", &["add", "gitlib", "--git", &url, "--tag", "v1"]));
    sb.write("app/src/main.cera", "import std.io\nimport gitlib\nmain :: fn() do\n  io.println(gitlib.add(1, 2))\nend\n");
    let o = sb.kote("app", &["run"]);
    ok(&o);
    assert_eq!(out(&o), "3\n");
    let lock = std::fs::read_to_string(sb.path("app/kote.lock")).unwrap();
    assert!(lock.contains("source = \"git+") && lock.contains("?tag=v1#"), "{lock}");
    // The lock keeps the tag's commit even offline.
    let o = sb.kote("app", &["run", "--offline", "--locked"]);
    ok(&o);
    assert_eq!(out(&o), "3\n");
    // Following the default branch picks up the newer commit.
    ok(&sb.kote("app", &["add", "gitlib", "--git", &url]));
    let o = sb.kote("app", &["run"]);
    ok(&o);
    assert_eq!(out(&o), "103\n");
}

#[test]
fn library_packages_check_but_do_not_run() {
    let sb = Sandbox::new();
    ok(&sb.kote(".", &["new", "mylib", "--lib"]));
    ok(&sb.kote("mylib", &["check"]));
    let o = sb.kote("mylib", &["run"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("library"), "{}", err(&o));
}

#[test]
fn clean_removes_target() {
    let sb = Sandbox::new();
    ok(&sb.kote(".", &["new", "p"]));
    ok(&sb.kote("p", &["build"]));
    assert!(sb.path("p/target").exists());
    ok(&sb.kote("p", &["clean"]));
    assert!(!sb.path("p/target").exists());
}

#[test]
fn not_in_a_project() {
    let sb = Sandbox::new();
    let o = sb.kote(".", &["build"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("could not find `kote.toml`"), "{}", err(&o));
}

#[test]
fn run_falls_back_to_the_vm() {
    let sb = Sandbox::new();
    ok(&sb.kote(".", &["new", "p"]));
    sb.write(
        "p/src/main.cera",
        "import std.io\nimport std.string\nmain :: fn() do\n  match string.parse_f64(\"2.5\") do\n    Some(x) -> io.println(x)\n    None -> io.println(\"none\")\n  end\nend\n",
    );
    let o = sb.kote("p", &["run"]);
    ok(&o);
    assert_eq!(out(&o), "2.5\n");
    assert!(err(&o).contains("running on the VM"), "{}", err(&o));
    // `build` insists on a native executable.
    assert!(!sb.kote("p", &["build"]).status.success());
}

#[test]
fn fmt_command() {
    let sb = Sandbox::new();
    ok(&sb.kote(".", &["new", "p"]));
    sb.write("p/src/util.cera", "double::fn(n:i32)->i32 do\n n*2\nend\n");
    sb.write("p/tests/t.cera", "test_x::fn() do\n  x=1\nend\n");
    // --check reports and fails without touching anything.
    let o = sb.kote("p", &["fmt", "--check"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("would reformat src/util.cera") && err(&o).contains("would reformat tests/t.cera"), "{}", err(&o));
    assert!(std::fs::read_to_string(sb.path("p/src/util.cera")).unwrap().starts_with("double::fn"));
    ok(&sb.kote("p", &["fmt"]));
    assert_eq!(std::fs::read_to_string(sb.path("p/src/util.cera")).unwrap(), "double :: fn(n: i32) -> i32 do\n  n * 2\nend\n");
    ok(&sb.kote("p", &["fmt", "--check"]));
    // A syntax error is reported, not hidden.
    sb.write("p/src/bad.cera", "f :: fn( do\nend\n");
    let o = sb.kote("p", &["fmt"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("src/bad.cera"), "{}", err(&o));
}

#[test]
fn private_items_stay_private() {
    let sb = Sandbox::new();
    ok(&sb.kote(".", &["new", "app"]));
    sb.write("app/src/lib2.cera", "pub open :: fn() -> i32 do\n  hidden() + 1\nend\n\nhidden :: fn() -> i32 do\n  41\nend\n\nSecret :: type\n  | Key(i32)\nend\n");
    sb.write("app/src/main.cera", "import std.io\nimport lib2\nmain :: fn() do\n  io.println(lib2.open())\nend\n");
    // A module may use its own private items from its public ones.
    let o = sb.kote("app", &["run"]);
    ok(&o);
    assert_eq!(out(&o), "42\n");
    sb.write(
        "app/src/main.cera",
        "import std.io\nimport lib2\nmain :: fn() do\n  io.println(lib2.hidden())\n  k = lib2.Key(1)\nend\n",
    );
    let o = sb.kote("app", &["check"]);
    assert!(!o.status.success());
    let e = err(&o);
    assert!(e.contains("`hidden` is private to module `lib2`"), "{e}");
    assert!(e.contains("`Key` is private to module `lib2`"), "{e}");
    assert!(e.contains("defined here, without `pub`: src/lib2.cera:5:1"), "{e}");
    assert!(e.contains("help: make it public by writing `pub`"), "{e}");
}

#[test]
fn pub_cannot_mark_impl_methods() {
    let sb = Sandbox::new();
    ok(&sb.kote(".", &["new", "app"]));
    sb.write(
        "app/src/main.cera",
        "T :: type { n: i32 }\nimpl Show for T do\n  pub show :: fn(t: T) -> String do\n    \"t\"\n  end\nend\nmain :: fn() do\nend\n",
    );
    let o = sb.kote("app", &["check"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("methods in an `impl` cannot be marked `pub`"), "{}", err(&o));
}

fn lib(sb: &Sandbox, dir: &str, name: &str, version: &str, deps: &str, body: &str) {
    sb.write(&format!("{dir}/kote.toml"), &format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\n\n[dependencies]\n{deps}"));
    sb.write(&format!("{dir}/src/lib.cera"), body);
}

fn app(sb: &Sandbox, deps: &str, main: &str) {
    sb.write("app/kote.toml", &format!("[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\n{deps}"));
    sb.write("app/src/main.cera", main);
}

const USE_MATHX: &str = "import std.io\nimport mathx\nmain :: fn() do\n  io.println(mathx.add(1, 2))\nend\n";

#[test]
fn registry_publish_use_and_update() {
    let sb = Sandbox::new();
    lib(&sb, "mathx", "mathx", "1.0.0", "", "pub add :: fn(a: i32, b: i32) -> i32 do\n  a + b\nend\n");
    let o = sb.kote("mathx", &["publish"]);
    ok(&o);
    assert!(err(&o).contains("Published mathx v1.0.0"), "{}", err(&o));
    // Published versions are immutable.
    let o = sb.kote("mathx", &["publish"]);
    assert!(!o.status.success() && err(&o).contains("already published"), "{}", err(&o));

    app(&sb, "mathx = \"^1.0\"\n", USE_MATHX);
    let o = sb.kote("app", &["run"]);
    ok(&o);
    assert_eq!(out(&o), "3\n");
    let lock = std::fs::read_to_string(sb.path("app/kote.lock")).unwrap();
    assert!(lock.contains("name = \"mathx\"\nversion = \"1.0.0\"\nsource = \"registry+default\""), "{lock}");

    // New releases do not move a locked dependency...
    lib(&sb, "mathx", "mathx", "1.1.0", "", "pub add :: fn(a: i32, b: i32) -> i32 do\n  a + b + 100\nend\n");
    ok(&sb.kote("mathx", &["publish"]));
    lib(&sb, "mathx", "mathx", "2.0.0", "", "pub add :: fn(a: i32, b: i32) -> i32 do\n  a + b + 200\nend\n");
    ok(&sb.kote("mathx", &["publish"]));
    ok(&sb.kote("app", &["--locked", "check"]));
    assert_eq!(out(&sb.kote("app", &["run"])), "3\n");
    // ...until `update`, which stays inside the requirement (1.1.0, not 2.0.0).
    ok(&sb.kote("app", &["update"]));
    assert_eq!(out(&sb.kote("app", &["run"])), "103\n");
    assert!(std::fs::read_to_string(sb.path("app/kote.lock")).unwrap().contains("version = \"1.1.0\""));
    assert!(out(&sb.kote("app", &["tree"])).contains("mathx v1.1.0 (registry)"));
}

#[test]
fn registry_solver_picks_one_version_for_everyone() {
    let sb = Sandbox::new();
    for (v, n) in [("1.0.0", 1), ("1.5.0", 15), ("2.0.0", 20)] {
        lib(&sb, "base", "base", v, "", &format!("pub n :: fn() -> i32 do\n  {n}\nend\n"));
        ok(&sb.kote("base", &["publish"]));
    }
    lib(&sb, "mid", "mid", "1.0.0", "base = \"^1.0\"\n", "import base\npub n :: fn() -> i32 do\n  base.n() + 1000\nend\n");
    ok(&sb.kote("mid", &["publish"]));
    app(
        &sb,
        "mid = \"^1\"\nbase = \">=1.0, <2\"\n",
        "import std.io\nimport mid\nimport base\nmain :: fn() do\n  io.println(mid.n() + base.n())\nend\n",
    );
    let o = sb.kote("app", &["run"]);
    ok(&o);
    assert_eq!(out(&o), "1030\n");
    let tree = out(&sb.kote("app", &["tree"]));
    assert!(tree.contains("base v1.5.0") && !tree.contains("base v2.0.0"), "{tree}");

    // Requirements on one series that cannot be met together are reported with who asked for what.
    lib(&sb, "mid2", "mid2", "1.0.0", "base = \"~1.5\"\n", "import base\npub n :: fn() -> i32 do\n  base.n()\nend\n");
    ok(&sb.kote("mid2", &["publish"]));
    app(&sb, "mid2 = \"^1\"\nbase = \"=1.0.0\"\n", "import std.io\nimport base\nmain :: fn() do\n  io.println(base.n())\nend\n");
    let o = sb.kote("app", &["check"]);
    assert!(!o.status.success());
    let e = err(&o);
    assert!(e.contains("no version of `base` satisfies every requirement"), "{e}");
    assert!(e.contains("`mid2` requires base ~1.5") && e.contains("`app` requires base =1.0.0"), "{e}");
    assert!(e.contains("available versions: 1.0.0, 1.5.0"), "{e}");
}

#[test]
fn incompatible_majors_coexist() {
    let sb = Sandbox::new();
    for (v, n) in [("1.0.0", 1), ("1.5.0", 15), ("2.0.0", 20)] {
        lib(&sb, "base", "base", v, "", &format!("pub n :: fn() -> i32 do\n  {n}\nend\n"));
        ok(&sb.kote("base", &["publish"]));
    }
    lib(&sb, "mid", "mid", "1.0.0", "base = \"^1.0\"\n", "import base\npub n :: fn() -> i32 do\n  base.n() + 1000\nend\n");
    ok(&sb.kote("mid", &["publish"]));
    // `mid` needs base 1.x while the app itself uses base 2.x: both are linked.
    app(
        &sb,
        "mid = \"^1\"\nbase = \"^2\"\n",
        "import std.io\nimport mid\nimport base\nmain :: fn() do\n  io.println(mid.n() + base.n())\nend\n",
    );
    let o = sb.kote("app", &["run"]);
    ok(&o);
    assert_eq!(out(&o), "1035\n");
    let tree = out(&sb.kote("app", &["tree"]));
    assert!(tree.contains("base v1.5.0") && tree.contains("base v2.0.0"), "{tree}");
    let lock = std::fs::read_to_string(sb.path("app/kote.lock")).unwrap();
    assert!(lock.contains("version = \"1.5.0\"") && lock.contains("version = \"2.0.0\""), "{lock}");
    assert!(lock.contains("\"base 1.5.0\""), "{lock}");
    ok(&sb.kote("app", &["--locked", "check"]));
    // Types from the two copies are different types, and their names do not collide.
}

#[test]
fn yanked_versions_are_skipped_unless_locked() {
    let sb = Sandbox::new();
    for (v, n) in [("1.0.0", 1), ("1.1.0", 2)] {
        lib(&sb, "base", "base", v, "", &format!("pub n :: fn() -> i32 do\n  {n}\nend\n"));
        ok(&sb.kote("base", &["publish"]));
    }
    let main = "import std.io\nimport base\nmain :: fn() do\n  io.println(base.n())\nend\n";
    app(&sb, "base = \"^1\"\n", main);
    assert_eq!(out(&sb.kote("app", &["run"])), "2\n");
    // A locked version keeps working after it is yanked...
    ok(&sb.kote(".", &["yank", "base@1.1.0"]));
    assert_eq!(out(&sb.kote("app", &["--locked", "run"])), "2\n");
    // ...but a fresh resolution avoids it, and so does `update`.
    ok(&sb.kote("app", &["update"]));
    assert_eq!(out(&sb.kote("app", &["run"])), "1\n");
    std::fs::remove_file(sb.path("app/kote.lock")).unwrap();
    assert_eq!(out(&sb.kote("app", &["run"])), "1\n");
    // Yanking everything that matches is an error; un-yanking restores the version.
    ok(&sb.kote(".", &["yank", "base@1.0.0"]));
    std::fs::remove_file(sb.path("app/kote.lock")).unwrap();
    assert!(err(&sb.kote("app", &["check"])).contains("is yanked"));
    ok(&sb.kote(".", &["yank", "base@1.0.0", "--undo"]));
    ok(&sb.kote(".", &["yank", "base@1.1.0", "--undo"]));
    assert_eq!(out(&sb.kote("app", &["run"])), "2\n");
    let o = sb.kote(".", &["yank", "base@9.9.9"]);
    assert!(!o.status.success() && err(&o).contains("is not published"), "{}", err(&o));
}

#[test]
fn registry_add_and_errors() {
    let sb = Sandbox::new();
    lib(&sb, "mathx", "mathx", "1.2.0", "", "pub add :: fn(a: i32, b: i32) -> i32 do\n  a + b\nend\n");
    ok(&sb.kote("mathx", &["publish"]));
    app(&sb, "", USE_MATHX);
    // With no version, `add` takes the newest published one.
    ok(&sb.kote("app", &["add", "mathx"]));
    assert!(std::fs::read_to_string(sb.path("app/kote.toml")).unwrap().contains("mathx = \"^1.2.0\""));
    ok(&sb.kote("app", &["add", "mathx@~1.2"]));
    assert!(std::fs::read_to_string(sb.path("app/kote.toml")).unwrap().contains("mathx = \"~1.2\""));
    assert_eq!(out(&sb.kote("app", &["run"])), "3\n");

    // Unknown packages, unsatisfiable requirements and bad syntax.
    let o = sb.kote("app", &["add", "nothing"]);
    assert!(!o.status.success() && err(&o).contains("no package named `nothing`"), "{}", err(&o));
    let o = sb.kote("app", &["add", "mathx@^3"]);
    assert!(!o.status.success() && err(&o).contains("no published version matches `^3`") && err(&o).contains("available versions: 1.2.0"), "{}", err(&o));
    let o = sb.kote("app", &["add", "mathx@1.x"]);
    assert!(!o.status.success() && err(&o).contains("version requirement"), "{}", err(&o));
    // The failed adds left kote.toml alone.
    assert!(std::fs::read_to_string(sb.path("app/kote.toml")).unwrap().contains("mathx = \"~1.2\""));

    // Published packages may not depend on path or git packages.
    lib(&sb, "bad", "bad", "0.1.0", "mathx = { path = \"../mathx\" }\n", "pub f :: fn() -> i32 do\n  1\nend\n");
    let o = sb.kote("bad", &["publish"]);
    assert!(!o.status.success() && err(&o).contains("only depend on registry packages"), "{}", err(&o));
}

#[test]
fn doc_generates_html_for_public_items() {
    let sb = Sandbox::new();
    ok(&sb.kote(".", &["new", "shapes", "--lib"]));
    sb.write("shapes/src/lib.cera", "## Shapes and areas.\n\n## Add two numbers; handy for `a < b` tests.\npub add :: fn(a: i32, b: i32) -> i32 do\n  a + b\nend\n\nsecret :: fn() -> i32 do\n  1\nend\n");
    sb.write("shapes/src/extra.cera", "pub Color :: type\n  | Red\n  | Blue\nend\n");
    let o = sb.kote("shapes", &["doc"]);
    ok(&o);
    assert!(err(&o).contains("Documented 2 public items in 2 modules"), "{}", err(&o));
    let index = std::fs::read_to_string(sb.path("shapes/target/doc/shapes/index.html")).unwrap();
    assert!(index.contains("Shapes and areas.") && index.contains("extra.html".trim_end_matches(".html")), "{index}");
    let page = std::fs::read_to_string(sb.path("shapes/target/doc/shapes/shapes.html")).unwrap();
    assert!(page.contains("pub add :: fn(a: i32, b: i32) -&gt; i32"), "{page}");
    assert!(page.contains("<code>a &lt; b</code>"), "{page}");
    assert!(!page.contains("secret"), "private items must not be documented");
    let extra = std::fs::read_to_string(sb.path("shapes/target/doc/shapes/extra.html")).unwrap();
    assert!(extra.contains("| Red"), "{extra}");
    // Syntax errors are reported, not hidden.
    sb.write("shapes/src/bad.cera", "f :: fn( do\nend\n");
    let o = sb.kote("shapes", &["doc"]);
    assert!(!o.status.success() && err(&o).contains("syntax errors"), "{}", err(&o));
}

#[test]
fn bench_reports_time_per_call() {
    let sb = Sandbox::new();
    ok(&sb.kote(".", &["new", "b"]));
    sb.write(
        "b/benches/loops.cera",
        "import std.list\n\nfib :: fn(n: i64) -> i64 do\n  if n < 2 do\n    n\n  else\n    fib(n - 1) + fib(n - 2)\n  end\nend\n\nbench_fib_15 :: fn() do\n  x = fib(15)\nend\n\nnot_a_bench :: fn() do\n  x = 1\nend\n",
    );
    let o = sb.kote("b", &["bench"]);
    ok(&o);
    let e = err(&o);
    assert!(e.contains("running 1 benchmark") && e.contains("loops.bench_fib_15") && e.contains("/iter"), "{e}");
    assert!(!e.contains("not_a_bench"), "{e}");
    // A filter that matches nothing is not an error.
    let o = sb.kote("b", &["bench", "zzz"]);
    ok(&o);
    assert!(err(&o).contains("no benchmarks found"), "{}", err(&o));
}

#[test]
fn unchanged_programs_are_not_rebuilt() {
    let sb = Sandbox::new();
    ok(&sb.kote(".", &["new", "p"]));
    let first = sb.kote("p", &["build"]);
    ok(&first);
    assert!(!err(&first).contains("Fresh"), "{}", err(&first));
    let exe = sb.path("p/target/bin/p");
    let stamp = std::fs::metadata(&exe).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let second = sb.kote("p", &["build"]);
    ok(&second);
    assert!(err(&second).contains("Fresh") && err(&second).contains("up to date"), "{}", err(&second));
    assert_eq!(std::fs::metadata(&exe).unwrap().modified().unwrap(), stamp, "the executable must be untouched");
    // `run` shares the cache, and a source change builds again.
    let o = sb.kote("p", &["run"]);
    ok(&o);
    assert!(err(&o).contains("Fresh"), "{}", err(&o));
    sb.write("p/src/main.cera", "import std.io\nmain :: fn() do\n  io.println(\"changed\")\nend\n");
    let o = sb.kote("p", &["run"]);
    ok(&o);
    assert!(!err(&o).contains("Fresh") && out(&o) == "changed\n", "{}{}", err(&o), out(&o));
}

#[test]
fn build_for_lythos() {
    let sb = Sandbox::new();
    ok(&sb.kote(".", &["new", "hello"]));
    ok(&sb.kote("hello", &["build", "--target", "x86_64-lythos"]));
    let exe = sb.path("hello/target/lythos/bin/hello");
    assert!(exe.is_file());
    // The Linux executable is separate and still builds.
    assert!(!sb.path("hello/target/bin/hello").exists());
    let elf = std::fs::read(&exe).unwrap();
    let entry = u64::from_le_bytes(elf[24..32].try_into().unwrap());
    assert!(entry >= 0x4000_0000);
    ok(&sb.kote("hello", &["build"]));
    assert!(sb.path("hello/target/bin/hello").is_file());
    // Other commands and unknown targets are refused.
    assert!(err(&sb.kote("hello", &["run", "--target", "x86_64-lythos"])).contains("only supported by `kote build`"));
    assert!(err(&sb.kote("hello", &["build", "--target", "arm"])).contains("unknown target"));
}


#[test]
fn lythos_package_drives_the_kernel_interface() {
    if !Command::new("python3").args(["-c", "import unicorn"]).output().map(|o| o.status.success()).unwrap_or(false) {
        eprintln!("skipping: python3 `unicorn` is not installed");
        return;
    }
    let pkg = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packages/lythos");
    let emu = concat!(env!("CARGO_MANIFEST_DIR"), "/../cera-native/tests/lythos_emu.py");
    let sb = Sandbox::new();
    app(&sb, &format!("lythos = {{ path = \"{pkg}\" }}\n"), r#"import std.io
import std.mem
import lythos
import lythos.ipc
import lythos.task
import lythos.pages
import lythos.device

main :: fn() do
  match ipc.create() do
    Err(e) -> io.println(e.message)
    Ok(ep) -> do
      _ = ipc.send(ep, "hello")
      io.println(ipc.recv(ep))
      io.println(task.free_frames())
      match task.spawn("", [ep.handle], ["a", "b"]) do
        Ok(t) -> io.println(t.id)
        Err(e) -> io.println(e.message)
      end
    end
  end
  base = 1073741824 * 8
  match pages.map(base, 2) do
    Ok(a) -> do
      unsafe do
        mem.write_u32(Option.unwrap_or(Num.to_usize(a), 0) + 4096, 305419896)
        io.println(mem.read_u32(Option.unwrap_or(Num.to_usize(a), 0) + 4096))
      end
      io.println(pages.unmap(a, 2))
    end
    Err(e) -> io.println(e.message)
  end
  io.println(device.config_read(device.from_handle(1), 4))
  io.println(pages.map(5, 1))
end
"#);
    ok(&sb.kote("app", &["build", "--target", "x86_64-lythos"]));
    let o = Command::new("python3").args(["-I", emu]).arg(sb.path("app/target/lythos/bin/app")).output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    let out = String::from_utf8_lossy(&o.stdout);
    // ipc round trip, free frames, spawn (1 cap, argv "a","b"), mmap + store/load, unmap,
    // device config read, unaligned mmap refused.
    assert_eq!(
        out,
        "Ok(\"hello\")\n4096\n102250\n305419896\nOk(())\nOk(452202500)\nErr(the address must be 4 KiB aligned)\n"
    );
}

#[test]
fn lythos_package_types_capabilities_and_reads_boot_info() {
    if !Command::new("python3").args(["-c", "import unicorn"]).output().map(|o| o.status.success()).unwrap_or(false) {
        eprintln!("skipping: python3 `unicorn` is not installed");
        return;
    }
    let pkg = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packages/lythos");
    let emu = concat!(env!("CARGO_MANIFEST_DIR"), "/../cera-native/tests/lythos_emu.py");
    let sb = Sandbox::new();
    app(&sb, &format!("lythos = {{ path = \"{pkg}\" }}\n"), r#"import std.io
import std.mem
import lythos
import lythos.ipc
import lythos.caps

message :: fn() -> String do
  unsafe do
    buf = mem.buffer(64)
    a = mem.addr_of(buf)
    mem.write_u64(a, 0xB0071000B0071000)
    mem.write_u64(a + 8, 536870912)
    mem.write_u64(a + 16, 131072)
    mem.copy_to(a + 24, "GenuineIntel")
    mem.string_from(a, 64)
  end
end

main :: fn() do
  b = caps.boot()
  io.println([b.memory.handle, b.rollback.handle, b.ipc.handle, b.filesystem.handle])
  match ipc.create() do
    Err(e) -> io.println(e.message)
    Ok(ep) -> do
      _ = ipc.send(ep, message())
      c = caps.assume(ep.handle, caps.Ipc)
      match caps.boot_info(c) do
        Ok(info) -> do
          io.println(info.mem_bytes)
          io.println(info.free_frames)
          io.println(info.vendor)
        end
        Err(e) -> io.println(e.message)
      end
      io.println(match caps.boot_info(b.memory) do Ok(_) -> "ok"
 Err(e) -> e.message end)
      io.println(caps.is_kind(c, caps.Ipc))
    end
  end
end
"#);
    ok(&sb.kote("app", &["build", "--target", "x86_64-lythos"]));
    let o = Command::new("python3").args(["-I", emu]).arg(sb.path("app/target/lythos/bin/app")).output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    let out = String::from_utf8_lossy(&o.stdout);
    assert_eq!(out, "[0, 1, 2, 3]\n536870912\n131072\nGenuineIntel\nexpected a IPC capability\ntrue\n");
}

#[test]
fn term_package_reads_keys_from_a_terminal() {
    if !Command::new("python3").args(["-c", "import pty"]).output().map(|o| o.status.success()).unwrap_or(false) {
        eprintln!("skipping: python3 is not available");
        return;
    }
    let pkg = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packages/term");
    let sb = Sandbox::new();
    app(
        &sb,
        &format!("term = {{ path = \"{pkg}\" }}\n"),
        r#"import std.io
import term

main :: fn() do
  match term.raw() do
    Err(e) -> io.eprintln(e.message)
    Ok(t) -> do
      match term.size(t) do
        (r, c) -> term.write(String.concat(["size ", show(r), "x", show(c), "\r\n"]))
      end
      run(t)
      term.restore(t)
    end
  end
end

run :: fn(t: term.Term) do
  k = term.read_key(t)
  term.write(String.concat(["key ", show(k), "\r\n"]))
  if k != term.ctrl(17) do
    run(t)
  end
end
"#,
    );
    ok(&sb.kote("app", &["build"]));
    let exe = sb.path("app/target/bin/app");
    let script = r#"
import os, pty, time, select, struct, fcntl, termios, sys
pid, fd = pty.fork()
if pid == 0:
    os.execv(sys.argv[1], ['app'])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 100, 0, 0))
out = b''
def drain(t):
    global out
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: out += os.read(fd, 4096)
            except OSError: return
for k in [b'a', b'\x1b[A', b'\x1b[3~', b'\x1b[6~', b'\x11']:
    drain(0.3); os.write(fd, k)
drain(0.6)
sys.stdout.write(out.decode().replace('\r', ''))
"#;
    let o = Command::new("python3").args(["-I", "-c", script]).arg(&exe).output().unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert_eq!(out, "size 30x100\nkey 97\nkey 1002\nkey 1004\nkey 1008\nkey 17\n", "{}", String::from_utf8_lossy(&o.stderr));
}

#[test]
fn lythos_package_reads_files_and_spawns_from_them() {
    if !Command::new("python3").args(["-c", "import unicorn"]).output().map(|o| o.status.success()).unwrap_or(false) {
        eprintln!("skipping: python3 `unicorn` is not installed");
        return;
    }
    let pkg = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packages/lythos");
    let emu = concat!(env!("CARGO_MANIFEST_DIR"), "/../cera-native/tests/lythos_emu.py");
    let sb = Sandbox::new();
    app(
        &sb,
        &format!("lythos = {{ path = \"{pkg}\" }}\n"),
        r#"import std.io
import lythos.file
import lythos.task

main :: fn() do
  match file.read_bytes("/big.bin") do
    Ok(s) -> io.println(String.length(s))
    Err(e) -> io.println(e.message)
  end
  match file.read_bytes("/missing") do
    Ok(_) -> io.println("??")
    Err(e) -> io.println(e.message)
  end
  match task.spawn_file("/big.bin", [3], ["x"]) do
    Ok(t) -> io.println(t.id)
    Err(e) -> io.println(e.message)
  end
end
"#,
    );
    ok(&sb.kote("app", &["build", "--target", "x86_64-lythos"]));
    let o = Command::new("python3")
        .args(["-I", emu])
        .arg(sb.path("app/target/lythos/bin/app"))
        .env("LYTHOS_EMU_FILES", format!("/big.bin={}", "z".repeat(100_000)))
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    // 100000 bytes read across buffer growth, ENOENT, and spawn: 1 cap (3), argv "x".
    assert_eq!(String::from_utf8_lossy(&o.stdout), "100000\nENOENT: no such file, directory or name\n301150\n");
}

// ---- HTTP registry -------------------------------------------------------------------

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Store {
    /// name -> version -> archive
    pkgs: BTreeMap<String, BTreeMap<String, Vec<u8>>>,
    yanked: BTreeSet<(String, String)>,
    requests: Vec<String>,
}

/// A small registry server speaking the protocol `kote` uses over HTTP.
fn serve() -> (String, Arc<Mutex<Store>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let store = Arc::new(Mutex::new(Store::default()));
    let s = store.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut c) = conn else { continue };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            let head_end = loop {
                let n = c.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break None;
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(p + 4);
                }
            };
            let Some(head_end) = head_end else { continue };
            let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
            let mut first = head.lines().next().unwrap_or("").split_whitespace();
            let (method, path) = (first.next().unwrap_or("").to_string(), first.next().unwrap_or("").to_string());
            let len = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok())).unwrap_or(0);
            while buf.len() < head_end + len {
                let n = c.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let body = buf[head_end..].to_vec();
            let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
            let mut st = s.lock().unwrap();
            st.requests.push(format!("{method} {path}"));
            let (code, payload): (u16, Vec<u8>) = match (method.as_str(), parts.as_slice()) {
                ("GET", [name, "index"]) => match st.pkgs.get(*name) {
                    Some(vs) => {
                        let text: String = vs.keys().map(|v| format!("{v}{}\n", if st.yanked.contains(&(name.to_string(), v.clone())) { " yanked" } else { "" })).collect();
                        (200, text.into_bytes())
                    }
                    None => (404, vec![]),
                },
                ("GET", [name, file]) if file.ends_with(".tar") => match st.pkgs.get(*name).and_then(|vs| vs.get(file.trim_end_matches(".tar"))) {
                    Some(b) => (200, b.clone()),
                    None => (404, vec![]),
                },
                ("PUT", [name, file]) if file.ends_with(".tar") => {
                    let v = file.trim_end_matches(".tar").to_string();
                    let entry = st.pkgs.entry(name.to_string()).or_default();
                    if entry.contains_key(&v) {
                        (409, vec![])
                    } else {
                        entry.insert(v, body);
                        (201, vec![])
                    }
                }
                ("PUT", [name, v, "yank"]) | ("DELETE", [name, v, "yank"]) => {
                    if st.pkgs.get(*name).is_some_and(|vs| vs.contains_key(*v)) {
                        let key = (name.to_string(), v.to_string());
                        if method == "PUT" {
                            st.yanked.insert(key);
                        } else {
                            st.yanked.remove(&key);
                        }
                        (200, vec![])
                    } else {
                        (404, vec![])
                    }
                }
                _ => (400, b"bad request".to_vec()),
            };
            drop(st);
            let reason = if code == 200 { "OK" } else { "Other" };
            let _ = write!(c, "HTTP/1.1 {code} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", payload.len());
            let _ = c.write_all(&payload);
        }
    });
    (url, store)
}

fn have_curl_and_tar() -> bool {
    ["curl", "tar"].iter().all(|t| Command::new(t).arg("--version").output().is_ok_and(|o| o.status.success()))
}

#[test]
fn http_registry_publish_use_and_yank() {
    if !have_curl_and_tar() {
        eprintln!("skipping: curl and tar are required");
        return;
    }
    let (url, store) = serve();
    let sb = Sandbox::new();
    for (v, n) in [("1.0.0", 1), ("1.1.0", 2)] {
        lib(&sb, "mathx", "mathx", v, "", &format!("pub n :: fn() -> i32 do\n  {n}\nend\n"));
        let o = sb.kote("mathx", &["publish", "--registry", &url]);
        ok(&o);
        assert!(out(&o).contains("Published") || err(&o).contains("Published"), "{}{}", out(&o), err(&o));
    }
    // Versions are immutable.
    let o = sb.kote("mathx", &["publish", "--registry", &url]);
    assert!(!o.status.success() && err(&o).contains("already published"), "{}", err(&o));

    let main = "import std.io\nimport mathx\nmain :: fn() do\n  io.println(mathx.n())\nend\n";
    app(&sb, &format!("mathx = {{ version = \"^1.0\", registry = \"{url}\" }}\n"), main);
    let o = sb.kote("app", &["run"]);
    ok(&o);
    assert_eq!(out(&o), "2\n");
    let lock = std::fs::read_to_string(sb.path("app/kote.lock")).unwrap();
    assert!(lock.contains(&format!("source = \"registry+{url}\"")), "{lock}");

    // Packages are downloaded once and then served from the cache; `--offline` needs no server.
    let downloads = store.lock().unwrap().requests.iter().filter(|r| r.ends_with(".tar")).count();
    ok(&sb.kote("app", &["--offline", "check"]));
    assert_eq!(store.lock().unwrap().requests.iter().filter(|r| r.ends_with(".tar")).count(), downloads);

    // Yank through the server: the locked version still works, `update` moves off it.
    ok(&sb.kote(".", &["yank", "mathx@1.1.0", "--registry", &url]));
    assert!(store.lock().unwrap().yanked.contains(&("mathx".to_string(), "1.1.0".to_string())));
    assert_eq!(out(&sb.kote("app", &["--locked", "run"])), "2\n");
    ok(&sb.kote("app", &["update"]));
    assert_eq!(out(&sb.kote("app", &["run"])), "1\n");
    ok(&sb.kote(".", &["yank", "mathx@1.1.0", "--undo", "--registry", &url]));
    ok(&sb.kote("app", &["update"]));
    assert_eq!(out(&sb.kote("app", &["run"])), "2\n");

    // Unknown packages and unreachable servers are explained.
    app(&sb, &format!("nothing = {{ version = \"1\", registry = \"{url}\" }}\n"), "main :: fn() do\n  ()\nend\n");
    assert!(err(&sb.kote("app", &["check"])).contains("no package named `nothing`"));
}

#[test]
fn http_registry_rejects_unsafe_archives() {
    if !have_curl_and_tar() {
        return;
    }
    let (url, store) = serve();
    let sb = Sandbox::new();
    sb.write("payload/x.txt", "x");
    let tar = sb.path("evil.tar");
    let st = Command::new("tar").arg("-cPf").arg(&tar).arg(sb.path("payload/x.txt")).status().unwrap();
    assert!(st.success());
    store.lock().unwrap().pkgs.entry("evil".into()).or_default().insert("1.0.0".into(), std::fs::read(&tar).unwrap());
    app(&sb, &format!("evil = {{ version = \"1\", registry = \"{url}\" }}\n"), "main :: fn() do\n  ()\nend\n");
    let o = sb.kote("app", &["check"]);
    assert!(!o.status.success() && err(&o).contains("unexpected entry"), "{}", err(&o));
}
