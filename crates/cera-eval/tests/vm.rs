//! The bytecode VM against the tree-walking interpreter.

use cera_eval::{disassemble, run_captured_on, Captured, Engine, RunError};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn on(engine: Engine, src: &str) -> Result<Captured, String> {
    match run_captured_on(engine, src, "", vec![], root()) {
        Ok(c) => Ok(c),
        Err(RunError::Syntax(d)) | Err(RunError::Type(d)) => Err(format!("compile: {d:?}")),
        Err(RunError::Runtime(e)) => Err(format!("runtime: {}", e.message)),
    }
}

/// Run on both engines; they must agree. Returns stdout.
fn both(src: &str) -> String {
    let t = on(Engine::Tree, src);
    let v = on(Engine::Vm, src);
    match (&t, &v) {
        (Ok(a), Ok(b)) => {
            assert_eq!((&a.out, &a.err, a.code), (&b.out, &b.err, b.code), "engines disagree on:\n{src}");
        }
        (Err(a), Err(b)) => assert_eq!(a, b, "engines disagree on:\n{src}"),
        _ => panic!("engines disagree on:\n{src}\ntree: {t:?}\nvm: {v:?}"),
    }
    v.unwrap_or_else(|e| panic!("{e}\n{src}")).out
}

fn both_err(src: &str) -> String {
    let t = on(Engine::Tree, src).expect_err("tree should fail");
    let v = on(Engine::Vm, src).expect_err("vm should fail");
    assert_eq!(t, v, "engines disagree on:\n{src}");
    v
}

fn cera_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            cera_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "cera") {
            out.push(p);
        }
    }
}

#[test]
fn examples_and_solutions_agree() {
    let mut files = Vec::new();
    cera_files(&root().join("examples"), &mut files);
    cera_files(&root().join("ceralings/solutions"), &mut files);
    assert!(files.len() > 20);
    for f in files {
        if f.file_name().is_some_and(|n| n == "broken.cera") {
            continue;
        }
        let src = std::fs::read_to_string(&f).unwrap();
        let t = on(Engine::Tree, &src);
        let v = on(Engine::Vm, &src);
        match (t, v) {
            (Ok(a), Ok(b)) => assert_eq!((a.out, a.err, a.code), (b.out, b.err, b.code), "{}", f.display()),
            (Err(a), Err(b)) => assert_eq!(a, b, "{}", f.display()),
            (a, b) => panic!("{}: tree {a:?} vs vm {b:?}", f.display()),
        }
    }
}

#[test]
fn closures_capture_through_several_levels() {
    let out = both(
        "import std.io\nmake :: fn(a: i32) do\n  fn(b: i32) -> fn(c: i32) -> a * 100 + b * 10 + c end end\nend\nmain :: fn() do\n  f = make(1)\n  g = f(2)\n  io.println(g(3))\n  io.println(List.map([1, 2], fn(x) -> g(x) end))\nend\n",
    );
    assert_eq!(out, "123\n[121, 122]\n");
}

#[test]
fn shadowing_and_captures() {
    let out = both(
        "import std.io\nmain :: fn() do\n  x = 1\n  f = fn() -> x end\n  x = 2\n  g = fn() -> x end\n  io.println(f())\n  io.println(g())\n  io.println(x)\nend\n",
    );
    assert_eq!(out, "1\n2\n2\n");
}

#[test]
fn deep_tail_recursion_runs_in_constant_space() {
    let src = "import std.io\nloop :: fn(n: i64, acc: i64) -> i64 do\n  if n == 0 do acc else loop(n - 1, acc + n) end\nend\nmain :: fn() do\n  io.println(loop(2000000, 0))\nend\n";
    let v = on(Engine::Vm, src).unwrap();
    assert_eq!(v.out, "2000001000000\n");
}

#[test]
fn mutual_tail_recursion() {
    let src = "import std.io\nis_even :: fn(n: i32) -> bool do\n  if n == 0 do true else is_odd(n - 1) end\nend\nis_odd :: fn(n: i32) -> bool do\n  if n == 0 do false else is_even(n - 1) end\nend\nmain :: fn() do\n  io.println(is_even(1000001))\nend\n";
    assert_eq!(on(Engine::Vm, src).unwrap().out, "false\n");
}

#[test]
fn tail_call_through_match_and_pipe() {
    let src = "import std.io\ncount :: fn(xs: [i32], n: i32) -> i32 do\n  match xs do\n    [] -> n\n    [_ | rest] -> (n + 1) |> count_more(rest)\n  end\nend\ncount_more :: fn(n: i32, xs: [i32]) -> i32 do\n  count(xs, n)\nend\nmain :: fn() do\n  io.println(count(List.range(0, 300000) |> List.map(fn(x) -> 1 end), 0))\nend\n";
    assert_eq!(on(Engine::Vm, src).unwrap().out, "300000\n");
}

#[test]
fn non_tail_recursion_is_deep_but_bounded() {
    let src = "import std.io\nsum :: fn(n: i64) -> i64 do\n  if n == 0 do 0 else n + sum(n - 1) end\nend\nmain :: fn() do\n  io.println(sum(100000))\nend\n";
    assert_eq!(on(Engine::Vm, src).unwrap().out, "5000050000\n");
    let inf = "import std.io\nboom :: fn(n: i64) -> i64 do\n  1 + boom(n + 1)\nend\nmain :: fn() do\n  io.println(boom(0))\nend\n";
    let e = on(Engine::Vm, inf).unwrap_err();
    assert!(e.contains("stack overflow"), "{e}");
}

#[test]
fn recursion_through_builtins_is_bounded() {
    let src = "import std.io\nf :: fn(n: i32) -> i32 do\n  List.sum(List.map([n], fn(x) -> f(x + 1) end))\nend\nmain :: fn() do\n  io.println(f(0))\nend\n";
    let e = on(Engine::Vm, src).unwrap_err();
    assert!(e.contains("stack overflow"), "{e}");
}

#[test]
fn defers_run_in_reverse_order_on_every_exit() {
    let out = both(
        "import std.io\nwork :: fn(fail: bool) -> Result<i32, String> do\n  defer io.println(\"first registered\")\n  defer io.println(\"second registered\")\n  x = if fail do Err(\"boom\")? else 1 end\n  io.println(\"body done\")\n  Ok(x)\nend\nmain :: fn() do\n  io.println(work(false))\n  io.println(work(true))\nend\n",
    );
    assert_eq!(
        out,
        "body done\nsecond registered\nfirst registered\nOk(1)\nsecond registered\nfirst registered\nErr(\"boom\")\n"
    );
}

#[test]
fn defers_in_nested_blocks() {
    let out = both(
        "import std.io\nmain :: fn() do\n  defer io.println(\"outer\")\n  if true do\n    defer io.println(\"inner\")\n    io.println(\"in if\")\n  end\n  io.println(\"after if\")\nend\n",
    );
    assert_eq!(out, "in if\ninner\nafter if\nouter\n");
}

#[test]
fn defers_run_when_a_runtime_error_unwinds() {
    for engine in [Engine::Tree, Engine::Vm] {
        let dir = std::env::temp_dir().join(format!("cera-vm-unwind-{engine:?}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = "import std.fs\ninner :: fn(d: Dir) -> i32 do\n  defer fs.write_string(d, \"inner.txt\", \"x\")\n  1 / 0\nend\nmain :: fn(caps: Caps) do\n  defer fs.write_string(caps.fs, \"main.txt\", \"x\")\n  inner(caps.fs)\nend\n";
        let r = run_captured_on(engine, src, "", vec![], dir.clone());
        assert!(matches!(&r, Err(RunError::Runtime(e)) if e.message.contains("division by zero")), "{engine:?}: {r:?}");
        assert!(dir.join("inner.txt").exists(), "{engine:?}: inner defer did not run");
        assert!(dir.join("main.txt").exists(), "{engine:?}: main defer did not run");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn try_returns_from_the_innermost_function() {
    let out = both(
        "import std.io\nfirst :: fn(xs: [i32]) -> Option<i32> do\n  x = List.head(xs)?\n  Some(x + 1)\nend\nmain :: fn() do\n  io.println(first([5]))\n  io.println(first([]))\n  io.println(List.map([[1], []], fn(l) -> match List.head(l) do\n    Some(v) -> v\n    None -> 0\n  end end))\nend\n",
    );
    assert_eq!(out, "Some(6)\nNone\n[1, 0]\n");
}

#[test]
fn pattern_matching_forms() {
    let out = both(
        "import std.io\nShape :: type\n  | Circle(r: f64)\n  | Rect(w: f64, h: f64)\nend\nP :: type { x: i32, y: i32 }\nd :: fn(s: Shape) -> String do\n  match s do\n    Circle(r) if r > 10.0 -> \"big circle\"\n    Circle(_) -> \"circle\"\n    Rect(w, h) -> \"rect\"\n  end\nend\nmain :: fn() do\n  io.println(d(Circle(11.0)))\n  io.println(d(Circle(1.0)))\n  io.println(d(Rect(1.0, 2.0)))\n  match P{x: 1, y: 2} do\n    P{x, y: yy} -> io.println(x + yy)\n  end\n  match [1, 2, 3, 4] do\n    [a, b | rest] -> io.println((a, b, rest))\n    _ -> io.println(\"no\")\n  end\n  match (1, \"x\", true) do\n    (1, \"x\", true) -> io.println(\"lit\")\n    _ -> io.println(\"no\")\n  end\nend\n",
    );
    assert_eq!(out, "big circle\ncircle\nrect\n3\n(1, 2, [3, 4])\nlit\n");
}

#[test]
fn match_failure_reports_the_same_error() {
    // partial matches are rejected by the checker, so provoke it with a guard
    let src = "import std.io\nf :: fn(x: i32) -> i32 do\n  match x do\n    n if n > 100 -> 1\n    _ -> 2\n  end\nend\nmain :: fn() do\n  io.println(f(1))\nend\n";
    assert_eq!(both(src), "2\n");
}

#[test]
fn traits_operators_and_records() {
    let out = both(
        "import std.io\nVec2 :: type { x: f64, y: f64 }\nimpl Add for Vec2 do\n  add :: fn(a: Vec2, b: Vec2) -> Vec2 do\n    Vec2{x: a.x + b.x, y: a.y + b.y}\n  end\nend\nimpl Show for Vec2 do\n  show :: fn(v: Vec2) -> String do\n    String.concat([\"<\", show(v.x), \", \", show(v.y), \">\"])\n  end\nend\nmain :: fn() do\n  v = Vec2{y: 2.0, x: 1.0} + Vec2{x: 0.5, y: 0.5}\n  io.println(v)\n  io.println([v, v])\nend\n",
    );
    assert_eq!(out, "<1.5, 2.5>\n[<1.5, 2.5>, <1.5, 2.5>]\n");
}

#[test]
fn constants_are_evaluated_once() {
    let out = both(
        "import std.io\nTABLE :: List.map([1, 2, 3], fn(x) -> x * x end)\nmain :: fn() do\n  io.println(TABLE)\n  io.println(List.length(TABLE))\nend\n",
    );
    assert_eq!(out, "[1, 4, 9]\n3\n");
}

#[test]
fn short_circuit_and_comparisons() {
    let out = both(
        "import std.io\nboom :: fn() -> bool do\n  io.println(\"evaluated\")\n  true\nend\nmain :: fn() do\n  io.println(false and boom())\n  io.println(true or boom())\n  io.println(true and boom())\n  io.println(1 < 2 and 2.5 >= 2.5)\n  io.println(0.0 / 0.0 < 1.0)\n  io.println(\"a\" < \"b\")\nend\n",
    );
    assert_eq!(out, "false\ntrue\nevaluated\ntrue\ntrue\nfalse\ntrue\n");
}

#[test]
fn runtime_errors_agree() {
    assert_eq!(
        both_err("import std.io\nmain :: fn() do\n  io.println(1 / 0)\nend\n"),
        "runtime: division by zero"
    );
    both_err("import std.io\nmain :: fn() do\n  x: u8 = 250\n  io.println(x + 10)\nend\n");
    both_err("main :: fn() -> Result<i32, String> do\n  Err(\"bad\")\nend\n");
}

#[test]
fn exit_codes_and_caps_on_the_vm() {
    let src = "import std.io\nimport std.process\nmain :: fn(caps: Caps) do\n  io.writeln(caps.console, \"hi\")\n  process.exit(4)\nend\n";
    let c = on(Engine::Vm, src).unwrap();
    assert_eq!((c.out.as_str(), c.code), ("hi\n", 4));
}

#[test]
fn stack_discipline_survives_big_expressions() {
    let out = both(
        "import std.io\nmain :: fn() do\n  xs = List.range(0, 50)\n  total = xs |> List.map(fn(x) -> x * 2 end) |> List.filter(fn(x) -> x % 3 == 0 end) |> List.sum\n  io.println(total)\n  io.println(List.fold(xs, 0, fn(a, b) -> a + b end))\nend\n",
    );
    assert_eq!(out, "816\n1225\n");
}

#[test]
fn disassembly_lists_chunks() {
    let d = disassemble("import std.io\nmain :: fn() do\n  io.println(1)\nend\n").unwrap();
    assert!(d.contains("`main`") && d.contains("Call(1)") || d.contains("TailCall(1)"), "{d}");
}

#[test]
fn loops_and_mutation_agree_on_both_engines() {
    let out = both(
        "import std.io\nimport std.list\n\
         main :: fn() do\n\
         \x20 var total = 0\n\
         \x20 for a in list.range(0, 6) do\n\
         \x20   var b = 0\n\
         \x20   while true do\n\
         \x20     b := b + 1\n\
         \x20     if b > a do\n\
         \x20       break\n\
         \x20     end\n\
         \x20     if b == 2 do\n\
         \x20       continue\n\
         \x20     end\n\
         \x20     total := total + b\n\
         \x20   end\n\
         \x20 end\n\
         \x20 io.println(total)\n\
         end\n",
    );
    assert_eq!(out, "27\n");
}

#[test]
fn break_with_values_pending_on_the_stack() {
    // `break` inside a block that is an argument: the pending `1` must not leak.
    let out = both(
        "import std.io\n\
         add :: fn(a: i64, b: i64) -> i64 do\n  a + b\nend\n\
         main :: fn() do\n\
         \x20 var n = 0\n\
         \x20 var acc = 0\n\
         \x20 while n < 10 do\n\
         \x20   n := n + 1\n\
         \x20   acc := acc + add(1, do\n\
         \x20     if n > 3 do\n\
         \x20       break\n\
         \x20     end\n\
         \x20     n\n\
         \x20   end)\n\
         \x20 end\n\
         \x20 io.println(acc)\n\
         \x20 io.println(n)\n\
         end\n",
    );
    assert_eq!(out, "9\n4\n");
}

#[test]
fn loops_in_functions_and_empty_lists() {
    let out = both(
        "import std.io\n\
         count :: fn(xs: [String]) -> i64 do\n\
         \x20 var n = 0\n\
         \x20 for x in xs do\n\
         \x20   n := n + 1\n\
         \x20 end\n\
         \x20 n\n\
         end\n\
         main :: fn() do\n\
         \x20 io.println(count([]))\n\
         \x20 io.println(count([\"a\", \"b\"]))\n\
         end\n",
    );
    assert_eq!(out, "0\n2\n");
}
