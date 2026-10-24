//! The formatter must keep programs meaning the same and be idempotent. These
//! tests run it over every Cera file and book snippet in the repository.

use cera_syntax::{format_source, parse, tree_fingerprint};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cera_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "target" || n == ".git") {
                continue;
            }
            cera_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "cera") {
            out.push(p);
        }
    }
}

fn snippets(md: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur: Option<String> = None;
    for line in md.lines() {
        match (&mut cur, line.trim_start().starts_with("```")) {
            (None, true) => {
                if line.trim() == "```cera" {
                    cur = Some(String::new());
                }
            }
            (Some(s), true) => {
                out.push(std::mem::take(s));
                cur = None;
            }
            (Some(s), false) => {
                s.push_str(line);
                s.push('\n');
            }
            _ => {}
        }
    }
    out
}

static CHECKED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static REFUSED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static INVALID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn check(name: &str, src: &str) {
    use std::sync::atomic::Ordering::SeqCst;
    if !parse(src).is_ok() {
        INVALID.fetch_add(1, SeqCst);
        return; // not a valid program (an intentional error example, say)
    }
    let once = match format_source(src) {
        Ok(s) => s,
        Err(e) => {
            // Comments in awkward places are refused, never mangled.
            assert!(e.message.contains("comment"), "{name}: {e}");
            REFUSED.fetch_add(1, SeqCst);
            eprintln!("refused {name}: {e}");
            return;
        }
    };
    CHECKED.fetch_add(1, SeqCst);
    assert!(parse(&once).is_ok(), "{name}: formatted output does not parse:\n{once}");
    assert_eq!(tree_fingerprint(src), tree_fingerprint(&once), "{name}: formatting changed the program:\n{once}");
    let twice = format_source(&once).unwrap_or_else(|e| panic!("{name}: second pass failed: {e}\n{once}"));
    assert_eq!(once, twice, "{name}: not idempotent");
    for (i, l) in once.lines().enumerate() {
        assert!(l.chars().count() <= 100 || l.contains('"') || l.trim_start().starts_with('#'), "{name}:{}: line too long: {l}", i + 1);
        assert!(!l.ends_with(' '), "{name}:{}: trailing space", i + 1);
    }
}

#[test]
fn repository_files_format_cleanly() {
    let mut files = vec![];
    for d in ["examples", "ceralings", "crates"] {
        cera_files(&root().join(d), &mut files);
    }
    assert!(!files.is_empty());
    let total = files.len();
    for f in files {
        if f.to_string_lossy().contains("/std/") || f.to_string_lossy().ends_with("native_std.cera") {
            continue; // intrinsic stubs
        }
        let src = std::fs::read_to_string(&f).unwrap();
        check(&f.display().to_string(), &src);
    }
    use std::sync::atomic::Ordering::SeqCst;
    eprintln!("files: {total}, formatted: {}, refused: {}, not valid programs: {}", CHECKED.load(SeqCst), REFUSED.load(SeqCst), INVALID.load(SeqCst));
    assert!(CHECKED.load(SeqCst) >= 40, "too few files were actually formatted");
}

#[test]
fn book_snippets_format_cleanly() {
    let dir = root().join("book/src");
    let mut n = 0;
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "md") {
            let md = std::fs::read_to_string(&p).unwrap();
            for (i, s) in snippets(&md).iter().enumerate() {
                check(&format!("{}#{i}", p.display()), s);
                n += 1;
            }
        }
    }
    assert!(n > 50, "found only {n} snippets");
}

#[test]
fn indentation_does_not_matter() {
    let mut files = vec![];
    for d in ["examples", "ceralings"] {
        cera_files(&root().join(d), &mut files);
    }
    let mut n = 0;
    for f in files {
        let src = std::fs::read_to_string(&f).unwrap();
        if !parse(&src).is_ok() {
            continue;
        }
        let Ok(a) = format_source(&src) else { continue };
        let stripped: String = src.lines().map(|l| l.trim_start()).collect::<Vec<_>>().join("\n") + "\n";
        let b = format_source(&stripped).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        assert_eq!(a, b, "{}", f.display());
        n += 1;
    }
    assert!(n > 20);
}

const TRICKY: &[&str] = &[
    // precedence and parentheses
    "f :: fn() do\n  a - (b - c)\n  a - b - c\n  (a or b) and c\n  a or b and c\n  -(a + b)\n  - -x\n  not not x\n  (a |> b) + 1\n  (fn(x) -> x end)(1)\n  (-1).abs()\n  (a + b).c\n  a == (b == c)\n  x * (y + z) * w\nend\n",
    // literals
    "f :: fn() do\n  a = 1_000_000\n  b = 0xFF\n  c = 1.5e3\n  d = \"tab\\there \\\"quoted\\\" \\\\ back\"\n  e = \"raw\nnewline\"\n  match a do\n    -1 -> 1\n    0 -> 2\n    \"s\" -> 3\n    true -> 4\n    _ -> 5\n  end\nend\n",
    // comments in many places
    "# header\n\nimport std.io # io\n\n# about f\nf :: fn(a: i32, # first\n        b: i32) -> i32 do # after do\n  # leading\n  x = a + b # trailing\n\n  # dangling before end\nend # after end\n\n# trailing file comment\n",
    "T :: type\n  # first\n  | A(i32) # a\n  | B\n  # last\nend\nR :: type {\n  x: i32, # x\n  y: i32,\n}\nTr :: trait\n  # m\n  m :: fn(self) -> i32 # m\nend\nimpl Tr for R do\n  # impl comment\n  m :: fn(self: R) -> i32 do\n    1\n  end\nend\n",
    "f :: fn(x: i32) -> i32 do\n  if x > 1 do\n    # then\n    1\n    # after then\n  else if x > 0 do\n    2\n  else\n    # else\n    3\n  end\nend\n",
    "f :: fn() do\n  match x do\n    # first\n    A -> 1 # one\n    # between\n    B -> 2\n    # last\n  end\n  g([\n    1, # one\n    2,\n  ])\nend\n",
    // layouts
    "f :: fn() do\n  if a do b end\n  if a do b else c end\n  if a do\n    b\n  end\n  g(fn(x) -> x + 1 end)\n  g(fn(x) ->\n    x + 1\n  end)\n  h(1, fn(x) do\n    x\n  end)\n  xs |> List.map(fn(x) -> x end) |> List.filter(fn(x) -> x > 1 end)\n  y = xs\n    |> List.map(f)\n    |> List.sum()\nend\n",
    "f :: fn() do\n  very_long_function_name(argument_number_one, argument_number_two, argument_number_three, argument_four)\n  total = first_value_with_long_name + second_value_with_long_name + third_value_with_long_name + fourth_value\n  Point { x: very_long_value_one_for_x, y: very_long_value_two_for_y, z: very_long_value_three_for_z }\nend\n",
    "f :: fn() do\nend\ng :: fn() do end\nh :: fn() do\n\n\n  a\n\n\n  b\nend\n",
    // qualified and generic
    "impl<T: Show + Ord> a.Tr for b.T<T> do\n  m :: fn<U: Show>(self: b.T<T>, u: U) -> [U] do\n    [u]\n  end\nend\nf :: fn() do\n  p = m.Point { x: 1 }\n  match p do\n    m.Point { x, y: py } -> x\n    m.A(a, [h | t], (q, r)) -> h\n  end\nend\nPair<A, B> :: type { first: A, second: B }\n",
    "f :: fn() do\n  region do\n    with allocator(arena_new(10)) do\n      defer cleanup()\n      do\n        1\n      end\n    end\n  end\n  r = f(x)?.y[0]\n  s = [1, 2 | rest]\n  t = (1, \"a\", ())\nend\n",
];

#[test]
fn tricky_inputs_round_trip() {
    for (i, src) in TRICKY.iter().enumerate() {
        assert!(parse(src).is_ok(), "case {i} does not parse: {:?}", parse(src).diagnostics);
        let once = format_source(src).unwrap_or_else(|e| panic!("case {i}: {e}"));
        assert!(parse(&once).is_ok(), "case {i}: output does not parse:\n{once}");
        assert_eq!(tree_fingerprint(src), tree_fingerprint(&once), "case {i}: tree changed:\n{once}");
        let twice = format_source(&once).unwrap();
        assert_eq!(once, twice, "case {i}: not idempotent:\n{once}");
        // No comment is lost.
        let count = |s: &str| s.matches('#').count();
        assert_eq!(count(src), count(&once), "case {i}: comments lost:\n{once}");
    }
}

#[test]
fn comment_inside_an_expression_is_refused() {
    let e = format_source("f :: fn() do\n  a + # why\n    b\nend\n").unwrap_err();
    assert!(e.message.contains("comment"), "{e}");
    // Hash signs inside strings are not comments.
    let ok = format_source("f :: fn() do\n  \"# not a comment\"\nend\n").unwrap();
    assert!(ok.contains("\"# not a comment\""));
}

#[test]
fn canonical_layout() {
    let got = format_source("area::fn(s:Shape)->i32 do\n  match s do\n    Circle(r)->3*r*r\n  end\nend\n").unwrap();
    assert_eq!(got, "area :: fn(s: Shape) -> i32 do\n  match s do\n    Circle(r) -> 3 * r * r\n  end\nend\n");
    assert_eq!(format_source("").unwrap(), "");
    assert_eq!(format_source("# only a comment\n").unwrap(), "# only a comment\n");
}

#[test]
fn pub_is_kept() {
    let out = cera_syntax::format_source("pub   f::fn()->i32 do\n 1\nend\npub T::type{x:i32}\n").unwrap();
    assert_eq!(out, "pub f :: fn() -> i32 do\n  1\nend\n\npub T :: type { x: i32 }\n");
    assert_eq!(cera_syntax::format_source(&out).unwrap(), out);
}

#[test]
fn loops_and_assignment_format_and_round_trip() {
    let src = "main::fn() do\nvar   n=0\nwhile n<3 do\nn:=n+1\nif n==2 do\ncontinue\nend\nend\nfor   x   in [1,2,3] do\nif x>5 do\nbreak\nend\nn:=n+x\nend\nend\n";
    let out = cera_syntax::format_source(src).unwrap();
    assert_eq!(
        out,
        "main :: fn() do\n  var n = 0\n  while n < 3 do\n    n := n + 1\n    if n == 2 do\n      continue\n    end\n  end\n  for x in [1, 2, 3] do\n    if x > 5 do\n      break\n    end\n    n := n + x\n  end\nend\n"
    );
    assert_eq!(cera_syntax::format_source(&out).unwrap(), out);
    // `env.var(...)` still parses: `var` is only special at the start of a statement.
    let ok = cera_syntax::format_source("f :: fn(e: E) do\n  v = env.var(e, \"X\")\nend\n").unwrap();
    assert!(ok.contains("env.var(e, \"X\")"), "{ok}");
}

#[test]
fn unsafe_blocks_format_and_round_trip() {
    let src = "f::fn(a: usize) -> u8 do\nunsafe   do\nmem.read_u8(a)\nend\nend\n";
    let out = cera_syntax::format_source(src).unwrap();
    assert_eq!(out, "f :: fn(a: usize) -> u8 do\n  unsafe do\n    mem.read_u8(a)\n  end\nend\n");
    assert_eq!(cera_syntax::format_source(&out).unwrap(), out);
    let inline = cera_syntax::format_source("f :: fn() -> i64 do\n  x = unsafe do sys.syscall0(39) end\n  x\nend\n").unwrap();
    assert!(inline.contains("unsafe do"), "{inline}");
}

#[test]
fn record_update_and_escapes_round_trip() {
    let src = "Point :: type { x: i32, y: i32 }\n\nmove :: fn(p: Point) -> Point do\n  {p | x: p.x + 1, y: 0}\nend\n\nmain :: fn() do\n  \"\\e[0m \\x41 \\u{e9}\"\nend\n";
    let once = cera_syntax::format_source(src).unwrap();
    assert_eq!(once, src);
    assert_eq!(cera_syntax::format_source(&once).unwrap(), once);
}

#[test]
fn repr_and_unsafe_fn_round_trip() {
    let src = "Hdr :: repr type { magic: u32, tag: [u8; 4] }\n\npeek :: unsafe fn(addr: usize) -> u8 do\n  0\nend\n";
    let once = cera_syntax::format_source(src).unwrap();
    assert_eq!(once, src);
    assert_eq!(cera_syntax::format_source(&once).unwrap(), once);
}

#[test]
fn bit_operators_round_trip() {
    let src = "f :: fn(a: u32, b: u32) -> u32 do\n  (a & b) ^ ~a + (b << 2) + (a >> 1)\nend\n";
    let once = cera_syntax::format_source(src).unwrap();
    assert_eq!(cera_syntax::format_source(&once).unwrap(), once);
}
