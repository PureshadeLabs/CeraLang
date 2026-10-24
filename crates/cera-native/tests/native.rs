//! Differential tests: native binaries must behave exactly like the VM.

use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static N: AtomicUsize = AtomicUsize::new(0);

fn native(src: &str) -> (i32, String, String) {
    native_with(src, "", &[])
}

fn native_with(src: &str, input: &str, args: &[&str]) -> (i32, String, String) {
    let elf = cera_native::compile_source(src).unwrap_or_else(|e| panic!("compile failed: {e}\n{src}"));
    let path = std::env::temp_dir().join(format!("cera_native_t{}_{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    std::fs::write(&path, elf).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    use std::io::Write;
    let mut child = Command::new(&path)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let o = child.wait_with_output().unwrap();
    let _ = std::fs::remove_file(&path);
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into(), String::from_utf8_lossy(&o.stderr).into())
}

fn vm(src: &str) -> (i32, String, String) {
    let c = cera_eval::run_captured_on(cera_eval::Engine::Vm, src, "", vec![], std::env::temp_dir()).unwrap();
    (c.code, c.out, c.err)
}

fn same_io(src: &str, input: &str, args: &[&str]) {
    let n = native_with(src, input, args);
    let c = cera_eval::run_captured_on(cera_eval::Engine::Vm, src, input, args.iter().map(|s| s.to_string()).collect(), std::env::temp_dir()).unwrap();
    assert_eq!(n.0, c.code, "exit code differs for:\n{src}\nstderr: {}", n.2);
    assert_eq!(n.1, c.out, "stdout differs for:\n{src}");
}

fn same(src: &str) {
    let n = native(src);
    let v = vm(src);
    assert_eq!(n.0, v.0, "exit code differs for:\n{src}");
    assert_eq!(n.1, v.1, "stdout differs for:\n{src}");
}

#[test]
fn hello_and_ints() {
    same("import std.io\nmain :: fn() do\n  io.println(\"hi\")\n  io.println(-42)\n  io.println(0)\n  io.println(true)\nend\n");
}

#[test]
fn fib_and_loops() {
    same("import std.io\nfib :: fn(n: i32) -> i32 do\n  if n < 2 do n else fib(n - 1) + fib(n - 2) end\nend\nsum_to :: fn(n: i64, acc: i64) -> i64 do\n  if n == 0 do acc else sum_to(n - 1, acc + n) end\nend\nmain :: fn() do\n  io.println(fib(20))\n  io.println(sum_to(10000000, 0))\nend\n");
}

#[test]
fn adts_and_match() {
    same(r#"import std.io
Shape :: type
  | Circle(r: i32)
  | Rect(w: i32, h: i32)
  | Empty
end
area :: fn(s: Shape) -> i32 do
  match s do
    Circle(r) -> 3 * r * r
    Rect(w, h) -> w * h
    Empty -> 0
  end
end
main :: fn() do
  io.println(area(Circle(2)))
  io.println(area(Rect(3, 4)))
  io.println(area(Empty))
end
"#);
}

#[test]
fn records_tuples_generics() {
    same(r#"import std.io
Point :: type { x: i32, y: i32 }
swap :: fn<A, B>(p: (A, B)) -> (B, A) do
  match p do
    (a, b) -> (b, a)
  end
end
id :: fn<T>(x: T) -> T do
  x
end
main :: fn() do
  p = Point{x: 3, y: 4}
  io.println(p.x * p.y)
  q = swap((1, true))
  match q do
    (a, _) -> io.println(a)
  end
  io.println(id(5) + id(6))
  io.println(id("s"))
end
"#);
}

#[test]
fn overflow_and_div_zero() {
    let src = "import std.io\nf :: fn(a: i32, b: i32) -> i32 do\n  a * b\nend\nmain :: fn() do\n  io.println(f(100000, 100000))\nend\n";
    let n = native(src);
    assert_eq!(n.0, 1);
    assert!(n.2.contains("runtime error: integer overflow in `i32` arithmetic"), "{}", n.2);
    let src = "import std.io\nf :: fn(a: i32, b: i32) -> i32 do\n  a / b\nend\nmain :: fn() do\n  io.println(f(1, 0))\nend\n";
    let n = native(src);
    assert_eq!(n.0, 1);
    assert!(n.2.contains("division by zero"), "{}", n.2);
    assert!(n.2.contains(" --> main.cera:3:3"), "{}", n.2);
    assert!(n.2.contains("3 |   a / b\n  |   ^^^^^"), "{}", n.2);
}

#[test]
fn native_std_typechecks() {
    let src = include_str!("../src/native_std.cera");
    let parsed = cera_syntax::parse_with(src, cera_syntax::ParseOptions { intrinsics: true });
    assert!(parsed.is_ok(), "{:?}", parsed.diagnostics);
    let an = cera_sema::analyze(&parsed.program);
    for d in &an.diagnostics {
        let (line, _) = src[..d.span.start].lines().enumerate().last().map_or((0, ""), |(i, l)| (i + 1, l));
        eprintln!("line {line}: {}", d.message);
    }
    assert!(an.diagnostics.is_empty());
}

#[test]
fn lists_and_closures() {
    same(r#"import std.io
double :: fn(x: i32) -> i32 do
  x * 2
end
main :: fn() do
  xs = [1, 2, 3, 4, 5]
  ys = xs |> map(double) |> filter(fn(x) -> x > 4 end)
  io.println(sum(ys))
  io.println(length(xs))
  k = 10
  zs = map(xs, fn(x) -> x + k end)
  io.println(fold(zs, 0, fn(a, b) -> a + b end))
  match reverse(xs) do
    [a, b | _] -> io.println(a - b)
    _ -> io.println(0)
  end
  io.println(sum(List.range(0, 100000) |> map(fn(x) -> x * 2 end)))
end
"#);
}

#[test]
fn std_list_option_sort() {
    same(r#"import std.io
main :: fn() do
  xs = [5, 3, 9, 1, 7, 3]
  ys = List.sort(xs)
  io.println(head(ys) |> Option.unwrap_or(0))
  io.println(List.last(ys) |> Option.unwrap_or(0))
  io.println(length(List.take(ys, 3)))
  io.println(sum(List.drop(ys, 4)))
  io.println(List.contains(xs, 9))
  io.println(List.any(xs, fn(x) -> x > 8 end))
  io.println(List.all(xs, fn(x) -> x > 8 end))
  io.println(sum(List.concat([[1, 2], [3], []])))
  io.println(length(List.zip([1, 2, 3], [4, 5])))
  io.println(Num.max(3, 8))
  io.println(Num.clamp(15, 0, 10))
  io.println(List.sort_by([3, 1, 2], fn(a, b) -> if a > b do Less else Greater end end) |> head |> Option.unwrap_or(0))
end
"#);
}

fn cera_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            cera_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "cera") {
            out.push(p);
        }
    }
}

/// Every example and ceralings solution that the native backend accepts must match the VM.
#[test]
fn examples_and_solutions_match_vm() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../");
    let mut files = Vec::new();
    cera_files(&root.join("examples"), &mut files);
    cera_files(&root.join("ceralings/solutions"), &mut files);
    let (mut ok, mut skipped) = (0, vec![]);
    for f in files {
        if f.file_name().is_some_and(|n| n == "broken.cera") {
            continue;
        }
        let src = std::fs::read_to_string(&f).unwrap();
        match cera_native::compile_source(&src) {
            Err(e) if e.contains("not supported by the native backend") || e.contains("no `main`") => skipped.push(format!("{}: {e}", f.file_name().unwrap().to_string_lossy())),
            Err(e) => panic!("{}: {e}", f.display()),
            Ok(_) => {
                // The editor needs a terminal; compiling it is the check here.
                if !f.file_name().is_some_and(|n| n == "kilo.cera") {
                    same(&src);
                }
                ok += 1;
            }
        }
    }
    eprintln!("native: {ok} matched the VM, {} skipped", skipped.len());
    for s in &skipped {
        eprintln!("  skipped {s}");
    }
}

#[test]
fn float_printing() {
    same(r#"import std.io
ident32 :: fn(x: f32) -> f32 do
  x
end
main :: fn() do
  xs = [0.1 + 0.2, 1.0 / 3.0, 2.0 / 3.0, 100.0, 1.5, 0.0, 1234567.891, 1.0e16, 1.0e15, 1.0e-5, 0.0001, 123456789012345680.0, 5.0e-324, 1.7976931348623157e308, 3.14159, 78.53975, 0.5, 2.5e-10, 9.999999999999999e22, 1.0 / 7.0, 100000.0 * 100000.0]
  List.map(xs, fn(x) -> io.println(x) end)
  io.println(xs)
  io.println(-0.0)
  io.println(ident32(0.1))
  io.println(ident32(16777216.0))
  io.println(1.0 / 3.0)
end
"#);
}

#[test]
fn strings_and_show() {
    same(r#"import std.io
Shape :: type
  | Circle(r: i32)
  | Named(name: String, tags: [String])
  | Empty
end
P :: type { x: i32, label: String }
impl Show for Shape do
  show :: fn(s: Shape) -> String do
    "shape"
  end
end
main :: fn() do
  s = "Hello, World"
  io.println(String.length(s))
  io.println(String.to_upper(s))
  io.println(String.to_lower(s))
  io.println(String.contains(s, "World"))
  io.println(String.starts_with(s, "Hell"))
  io.println(String.ends_with(s, "d"))
  io.println(String.split("a,b,,c", ","))
  io.println(String.join(["x", "y", "z"], "-"))
  io.println(String.trim("  pad \n"))
  io.println(String.replace("a.b.c", ".", "::"))
  io.println(String.repeat("ab", 3))
  io.println(String.lines("one\ntwo\r\nthree\n"))
  io.println(String.chars("héllo"))
  io.println(String.from_chars(String.chars("aé😀")))
  io.println(String.chars("q\"'\n"))
  io.println(String.slice("hello", 1, 3))
  io.println(String.slice("héllo", 2, 1))
  io.println(show(Circle(3)) == "")
  io.println((1, "two"))
  io.println(Some(3))
  io.println(String.concat(["a", "b", "c"]))
  io.println("a" < "b")
  io.println("abc" == "abc")
  io.println(Circle(3) == Circle(3))
  io.println([1, 2] == [1, 2])
  io.println(compare("a", "b"))
  io.println(show(42) == "42")
end
"#);
}

#[test]
fn traits_defer_try() {
    same(r#"import std.io
Speaker :: trait
  speak :: fn(self) -> String
end
Dog :: type { name: String }
Cat :: type { lives: i32 }
impl Speaker for Dog do
  speak :: fn(d: Dog) -> String do
    String.concat([d.name, " says woof"])
  end
end
impl Speaker for Cat do
  speak :: fn(c: Cat) -> String do
    "meow"
  end
end
V :: type { x: i32, y: i32 }
impl Add for V do
  add :: fn(a: V, b: V) -> V do
    V{x: a.x + b.x, y: a.y + b.y}
  end
end
impl Show for V do
  show :: fn(v: V) -> String do
    String.concat(["<", show(v.x), ",", show(v.y), ">"])
  end
end
half :: fn(n: i32) -> Result<i32, String> do
  if n % 2 == 0 do Ok(n / 2) else Err("odd") end
end
quarter :: fn(n: i32) -> Result<i32, String> do
  a = half(n)?
  defer io.println("quarter done")
  half(a)
end
main :: fn() do
  io.println(speak(Dog{name: "Rex"}))
  io.println(speak(Cat{lives: 9}))
  io.println(V{x: 1, y: 2} + V{x: 10, y: 20})
  io.println([V{x: 1, y: 1}])
  io.println(quarter(8))
  io.println(quarter(6))
  io.println(quarter(3))
  defer io.println("bye")
  io.println(map([Dog{name: "a"}], speak))
end
"#);
}

#[test]
fn maps_sets_num() {
    same(r#"import std.io
import std.map
import std.set
big :: fn() -> i64 do
  3000000000
end
main :: fn() do
  m = Map.from_list([("b", 2), ("a", 1), ("c", 3)])
  io.println(m)
  io.println(Map.get(m, "a"))
  io.println(Map.get_or(Map.insert(m, "z", 26), "z", 0))
  io.println(Map.keys(Map.remove(m, "b")))
  io.println(Map.size(m))
  s = Set.from_list([3, 1, 2, 3, 1])
  io.println(s)
  io.println(Set.contains(s, 2))
  io.println(Set.to_list(Set.union(s, Set.from_list([9]))))
  io.println(m == Map.from_list([("a", 1), ("b", 2), ("c", 3)]))
  io.println(List.sort([(2, "b"), (1, "z"), (2, "a")]))
  io.println([1, 2] < [1, 3])
  io.println(Num.abs(-5))
  io.println(Num.abs(-2.5))
  io.println(Num.pow(2, 10))
  io.println(Num.pow(1.5, 3))
  io.println(Num.sqrt(2.0))
  io.println(Num.floor(-1.5))
  io.println(Num.ceil(1.2))
  io.println(Num.round(2.5))
  io.println(Num.round(-2.5))
  io.println(Num.round(0.49999999999999994))
  io.println(Num.to_f64(7))
  io.println(Num.to_i32(big()))
  io.println(Num.to_u8(255))
  io.println(Num.to_u8(256))
  io.println(Num.to_i64(2.9))
  io.println(Num.to_usize(-1))
  io.println(Num.min(3, 2))
end
"#);
}

#[test]
fn regions_and_allocators() {
    same(r#"import std.io
main :: fn() do
  region do
    xs = [1, 2, 3]
    ys = promote(xs)
    io.println(ys)
  end
  scratch = arena_new(4096)
  with allocator(scratch) do
    io.println("in arena")
  end
end
"#);
}

#[test]
fn caps_stdin_env_time() {
    let src = r#"import std.io
import std.env
import std.time
import std.process
main :: fn(caps: Caps) do
  args = env.args(caps.env)
  io.writeln(caps.console, show(args))
  match io.read_line() do
    Some(l) -> io.println(String.concat(["first: ", l]))
    None -> io.println("no input")
  end
  match io.read_line_from(caps.console) do
    Some(l) -> io.println(String.concat(["second: ", l]))
    None -> io.println("no input")
  end
  rest = io.read_all()
  io.println(String.length(rest))
  t0 = time.now_ms(caps.clock)
  time.sleep_ms(caps.clock, 5)
  io.println(time.now_ms(caps.clock) - t0 >= 5)
  io.println(env.var(caps.env, "CERA_NATIVE_TEST_UNSET"))
  process.exit(3)
end
"#;
    same_io(src, "one\r\ntwo\nthree\nfour", &["a", "b c"]);
}

#[test]
fn parse_and_deep_recursion() {
    same(r#"import std.io
count :: fn(n: i64) -> i64 do
  if n == 0 do 0 else 1 + count(n - 1) end
end
main :: fn() do
  io.println(String.parse_i64("123"))
  io.println(String.parse_i64("-9223372036854775808"))
  io.println(String.parse_i64("9223372036854775808"))
  io.println(String.parse_i64("+7"))
  io.println(String.parse_i64(""))
  io.println(String.parse_i64("-"))
  io.println(String.parse_i64("12a"))
  io.println(String.parse_i64(" 1"))
  io.println(count(10000))
end
"#);
}

#[test]
fn native_recursion_is_deep() {
    let src = "import std.io\ncount :: fn(n: i64) -> i64 do\n  if n == 0 do 0 else 1 + count(n - 1) end\nend\nmain :: fn() do\n  io.println(count(5000000))\nend\n";
    let n = native(src);
    assert_eq!((n.0, n.1.as_str()), (0, "5000000\n"), "{}", n.2);
}

#[test]
fn loops_and_mutation() {
    same(
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
}

#[test]
fn break_inside_an_argument_block() {
    same(
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
}

#[test]
fn for_over_strings_floats_and_records() {
    same(
        "import std.io\n\
         P :: type { x: i64, y: f64 }\n\
         main :: fn() do\n\
         \x20 var s = 0.0\n\
         \x20 for p in [P{x: 1, y: 1.5}, P{x: 2, y: 2.5}] do\n\
         \x20   s := s + p.y\n\
         \x20 end\n\
         \x20 io.println(s)\n\
         \x20 var out = \"\"\n\
         \x20 for w in [\"a\", \"b\"] do\n\
         \x20   io.println(w)\n\
         \x20 end\n\
         end\n",
    );
}

#[test]
fn executables_carry_debug_info() {
    let src = "import std.io\n\nhelper :: fn(n: i32) -> i32 do\n  n + 1\nend\n\nmain :: fn() do\n  io.println(helper(41))\nend\n";
    let elf = cera_native::compile_source(src).unwrap();
    let has = |needle: &[u8]| elf.windows(needle.len()).any(|w| w == needle);
    assert!(has(b".debug_line") && has(b".debug_info") && has(b".debug_abbrev"));
    assert!(has(b"main.cera") && has(b"helper"), "file and function names are recorded");
    // The image still runs.
    same(src);
    // With `addr2line` around, an address inside `helper` maps back to its source line.
    if let Ok(o) = Command::new("addr2line").arg("--version").output() {
        if o.status.success() {
            let path = std::env::temp_dir().join(format!("cera_dbg_{}", std::process::id()));
            std::fs::write(&path, &elf).unwrap();
            let line = Command::new("readelf").args(["--debug-dump=decodedline"]).arg(&path).output();
            let _ = std::fs::remove_file(&path);
            if let Ok(l) = line {
                let text = String::from_utf8_lossy(&l.stdout).into_owned();
                assert!(text.contains("main.cera"), "{text}");
            }
        }
    }
}

// ---- Lythos target -------------------------------------------------------------------

fn lythos_elf(src: &str) -> Vec<u8> {
    let parsed = cera_syntax::parse(src);
    assert!(parsed.is_ok(), "parse failed");
    let an = cera_sema::analyze(&parsed.program);
    assert!(an.diagnostics.is_empty(), "type errors");
    cera_native::compile_for(true, &parsed.program, &an, None, None).unwrap_or_else(|e| panic!("compile failed: {e}"))
}

/// Run a Lythos executable under the Unicorn model of the Lythos syscall ABI.
/// Returns `None` when python3 or its `unicorn` package is missing.
fn run_lythos(elf: &[u8], args: &[&str]) -> Option<(i32, String)> {
    let ok = Command::new("python3").args(["-c", "import unicorn"]).output().ok()?.status.success();
    if !ok {
        eprintln!("skipping: python3 `unicorn` is not installed");
        return None;
    }
    let path = std::env::temp_dir().join(format!("cera_lythos_t{}_{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    std::fs::write(&path, elf).unwrap();
    let emu = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/lythos_emu.py");
    let o = Command::new("python3").args(["-I", emu]).arg(&path).args(args).output().unwrap();
    let _ = std::fs::remove_file(&path);
    assert!(o.status.code() != Some(139), "emulated program faulted: {}", String::from_utf8_lossy(&o.stderr));
    Some((o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into()))
}

#[test]
fn lythos_executables_use_the_lythos_layout() {
    let elf = lythos_elf("import std.io\nmain :: fn() do\n  io.println(\"hi\")\nend\n");
    let phoff = u64::from_le_bytes(elf[32..40].try_into().unwrap()) as usize;
    let entry = u64::from_le_bytes(elf[24..32].try_into().unwrap());
    assert!(entry >= 0x4000_0000, "entry {entry:#x} must be above 1 GiB");
    let phnum = u16::from_le_bytes(elf[56..58].try_into().unwrap()) as usize;
    for i in 0..phnum {
        let ph = phoff + i * 56;
        let va = u64::from_le_bytes(elf[ph + 16..ph + 24].try_into().unwrap());
        assert!(va >= 0x4000_0000, "segment at {va:#x} must be above 1 GiB");
    }
    // The Linux start code maps its stack with syscall 9, which is `SYS_ROLLBACK` on Lythos.
    let linux = cera_native::compile_source("import std.io\nmain :: fn() do\n  io.println(\"hi\")\nend\n").unwrap();
    assert_ne!(elf[..512], linux[..512]);
}

#[test]
fn lythos_programs_match_the_vm() {
    let progs = [
        "import std.io\nmain :: fn() do\n  io.println(\"Hello, Lythos\")\nend\n",
        "import std.io\nimport std.list\nmain :: fn() do\n  xs = list.map(list.range(0, 200000), fn(x) do x * 2 end)\n  io.println(show(list.length(xs)))\n  io.println(show(list.sum(xs)))\nend\n",
        "import std.io\nmain :: fn() do\n  var t = 0.0\n  var i = 0\n  while i < 10 do\n    t := t + 0.5\n    i := i + 1\n  end\n  io.println(show(t))\n  io.println(show(2.5 * 4.0))\nend\n",
        "import std.io\nShape :: type\n  | Circle(r: f64)\n  | Rect(w: f64, h: f64)\nend\narea :: fn(s: Shape) -> f64 do\n  match s do\n    Circle(r) -> 3.0 * r * r\n    Rect(w, h) -> w * h\n  end\nend\nmain :: fn() do\n  io.println(show(area(Rect(2.0, 3.0)) + area(Circle(1.0))))\nend\n",
    ];
    for src in progs {
        let Some((code, out)) = run_lythos(&lythos_elf(src), &[]) else { return };
        let v = vm(src);
        assert_eq!(code, v.0, "exit code differs for:\n{src}");
        assert_eq!(out, v.1, "stdout differs for:\n{src}");
    }
}

#[test]
fn lythos_exit_status_and_runtime_errors() {
    let Some((code, _)) = run_lythos(&lythos_elf("import std.process\nmain :: fn() do\n  process.exit(7)\nend\n"), &[]) else { return };
    assert_eq!(code, 7);
    let src = "import std.io\nf :: fn(a: i32, b: i32) -> i32 do\n  a / b\nend\nmain :: fn() do\n  io.println(f(1, 0))\nend\n";
    let (code, out) = run_lythos(&lythos_elf(src), &[]).unwrap();
    assert_eq!(code, 1);
    assert!(out.contains("division by zero"), "output was: {out}");
}

#[test]
fn lythos_arguments_and_time() {
    let src = "import std.io\nimport std.env\nimport std.time\nmain :: fn(caps: Caps) do\n  io.println(show(env.args(caps.env)))\n  t0 = time.now_ms(caps.clock)\n  time.sleep_ms(caps.clock, 5)\n  io.println(time.now_ms(caps.clock) - t0 >= 5)\nend\n";
    let Some((code, out)) = run_lythos(&lythos_elf(src), &["a", "b c"]) else { return };
    let v = cera_eval::run_captured_on(cera_eval::Engine::Vm, src, "", vec!["a".into(), "b c".into()], std::env::temp_dir()).unwrap();
    assert_eq!(code, v.code);
    assert_eq!(out, v.out);
}

// ---- files ---------------------------------------------------------------------------

const FS_PROGRAM: &str = r#"import std.io
import std.fs

main :: fn(caps: Caps) do
  d = caps.fs
  match fs.write_string(d, "t1.txt", "hello\n") do
    Ok(_) -> io.println("wrote")
    Err(e) -> io.println(e.message)
  end
  match fs.append_string(d, "t1.txt", "more\n") do
    Ok(_) -> io.println("appended")
    Err(e) -> io.println(e.message)
  end
  match fs.read_to_string(d, "t1.txt") do
    Ok(s) -> io.println(s)
    Err(e) -> io.println(e.message)
  end
  io.println(fs.exists(d, "t1.txt"))
  io.println(fs.exists(d, "nope"))
  match fs.read_to_string(d, "nope") do
    Ok(s) -> io.println(s)
    Err(e) -> io.println(e.message)
  end
  match fs.read_to_string(d, "../x") do
    Ok(s) -> io.println(s)
    Err(e) -> io.println(e.message)
  end
  match fs.make_dir(d, "sub") do
    Ok(_) -> io.println("made")
    Err(e) -> io.println(e.message)
  end
  match fs.open_dir(d, "sub") do
    Ok(sd) -> do
      match fs.write_string(sd, "a.txt", "A") do
        Ok(_) -> io.println("in sub")
        Err(e) -> io.println(e.message)
      end
      match fs.list(sd) do
        Ok(names) -> io.println(show(names))
        Err(e) -> io.println(e.message)
      end
      match fs.write_string(fs.read_only(sd), "b.txt", "B") do
        Ok(_) -> io.println("bad")
        Err(e) -> io.println(e.message)
      end
      match fs.remove(sd, "a.txt") do
        Ok(_) -> io.println("removed")
        Err(e) -> io.println(e.message)
      end
    end
    Err(e) -> io.println(e.message)
  end
  match fs.remove(d, "t1.txt") do
    Ok(_) -> io.println("removed file")
    Err(e) -> io.println(e.message)
  end
  match fs.open_dir(d, "t1.txt") do
    Ok(_) -> io.println("bad")
    Err(e) -> io.println(e.message)
  end
end
"#;

fn fresh_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cera_fs_t{}_{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Run a native executable with `dir` as its working directory (the root of `caps.fs`).
fn native_in(src: &str, dir: &std::path::Path) -> (i32, String) {
    let elf = cera_native::compile_source(src).unwrap_or_else(|e| panic!("compile failed: {e}\n{src}"));
    let path = dir.join("..").join(format!("cera_fs_bin{}_{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    std::fs::write(&path, elf).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let o = Command::new(&path).current_dir(dir).output().unwrap();
    let _ = std::fs::remove_file(&path);
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into())
}

fn vm_in(src: &str, dir: &std::path::Path) -> (i32, String) {
    let c = cera_eval::run_captured_on(cera_eval::Engine::Vm, src, "", vec![], dir.to_path_buf()).unwrap();
    (c.code, c.out)
}

#[test]
fn native_files_match_the_vm() {
    let (a, b) = (fresh_dir(), fresh_dir());
    let n = native_in(FS_PROGRAM, &a);
    let v = vm_in(FS_PROGRAM, &b);
    assert_eq!(n, v);
    assert!(n.1.contains("hello\nmore\n") && n.1.contains("outside the directory"), "{}", n.1);
    // The same files are left behind.
    let list = |d: &std::path::Path| {
        let mut v: Vec<_> = std::fs::read_dir(d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    };
    assert_eq!(list(&a), list(&b));
}

#[test]
fn native_files_edge_cases() {
    let src = r#"import std.io
import std.fs
main :: fn(caps: Caps) do
  d = caps.fs
  match fs.write_string(d, "bad.bin", "ok") do
    Ok(_) -> io.println("w")
    Err(e) -> io.println(e.message)
  end
  match fs.write_string(d, "", "x") do
    Ok(_) -> io.println("w")
    Err(e) -> io.println(e.message)
  end
  match fs.write_string(d, "/etc/x", "x") do
    Ok(_) -> io.println("w")
    Err(e) -> io.println(e.message)
  end
  match fs.write_string(d, "a/../b", "x") do
    Ok(_) -> io.println("w")
    Err(e) -> io.println(e.message)
  end
  match fs.make_dir(d, "dir") do
    Ok(_) -> io.println("m")
    Err(e) -> io.println(e.message)
  end
  match fs.make_dir(d, "dir") do
    Ok(_) -> io.println("m")
    Err(e) -> io.println(e.message)
  end
  match fs.read_to_string(d, "dir") do
    Ok(_) -> io.println("r")
    Err(e) -> io.println(e.message)
  end
  match fs.remove(d, "dir") do
    Ok(_) -> io.println("removed dir")
    Err(e) -> io.println(e.message)
  end
  match fs.read_to_string(d, "link/secret") do
    Ok(s) -> io.println(s)
    Err(e) -> io.println(e.message)
  end
  io.println(fs.exists(d, "link"))
  match fs.list(d) do
    Ok(names) -> io.println(show(names))
    Err(e) -> io.println(e.message)
  end
end
"#;
    let (a, b) = (fresh_dir(), fresh_dir());
    for d in [&a, &b] {
        std::fs::write(d.join("bad.bin"), [0xffu8, 0xfe]).unwrap();
        let outside = fresh_dir();
        std::fs::write(outside.join("secret"), "top secret").unwrap();
        std::os::unix::fs::symlink(&outside, d.join("link")).unwrap();
    }
    let n = native_in(src, &a);
    let v = vm_in(src, &b);
    assert_eq!(n, v);
    // A symlink that leaves the directory is not followed.
    assert!(n.1.contains("outside the directory") && !n.1.contains("top secret"), "{}", n.1);
}

#[test]
fn native_reads_large_and_multibyte_files() {
    let dir = fresh_dir();
    let big = "héllo wörld ✓\n".repeat(20000);
    std::fs::write(dir.join("big.txt"), &big).unwrap();
    let src = "import std.io\nimport std.fs\nimport std.string\nmain :: fn(caps: Caps) do\n  match fs.read_to_string(caps.fs, \"big.txt\") do\n    Ok(s) -> io.println(string.length(s))\n    Err(e) -> io.println(e.message)\n  end\nend\n";
    let n = native_in(src, &dir);
    assert_eq!(n, vm_in(src, &dir));
    assert_eq!(n.1.trim(), big.len().to_string());
}

#[test]
fn lythos_files() {
    let n = run_lythos(&lythos_elf(FS_PROGRAM), &[]);
    let Some((code, out)) = n else { return };
    let (a,) = (fresh_dir(),);
    let v = vm_in(FS_PROGRAM, &a);
    assert_eq!((code, out), v);
}

// ---- unsafe: raw memory and system calls ---------------------------------------------

const RAW_PROGRAM: &str = r#"import std.io
import std.mem
import std.sys

fill :: fn() -> u64 do
  unsafe do
    buf = mem.buffer(32)
    a = mem.addr_of(buf)
    mem.write_u32(a, 3735928559)
    mem.write_u8(a + 4, 7)
    mem.write_u16(a + 6, 513)
    mem.write_u64(a + 8, 1234567890123)
    mem.write_u8(a + 16, 200)
    mem.read_u64(a + 8)
  end
end

main :: fn() do
  io.println(fill())
  unsafe do
    buf = mem.buffer(8)
    a = mem.addr_of(buf)
    mem.write_u32(a, 3735928559)
    io.println(mem.read_u32(a))
    io.println(mem.read_u8(a))
    io.println(mem.read_u16(a + 2))
    mem.copy_to(a, "ABCDEFGH")
    io.println(mem.string_from(a + 2, 4))
  end
end
"#;

#[test]
fn raw_memory_native() {
    let n = native(RAW_PROGRAM);
    assert_eq!(n.0, 0, "{}", n.2);
    assert_eq!(n.1, "1234567890123\n3735928559\n239\n57005\nCDEF\n");
}

#[test]
fn raw_memory_on_lythos() {
    let Some((code, out)) = run_lythos(&lythos_elf(RAW_PROGRAM), &[]) else { return };
    assert_eq!(code, 0);
    assert_eq!(out, "1234567890123\n3735928559\n239\n57005\nCDEF\n");
}

#[test]
fn raw_syscalls_reach_the_kernel() {
    // Linux: getpid (39) is positive. Lythos: SYS_TIME_EPOCH (44) is a plausible time.
    let src = |nr: i32| format!("import std.io\nimport std.sys\nmain :: fn() do\n  t = unsafe do sys.syscall0({nr}) end\n  io.println(t > 1000)\nend\n");
    assert_eq!(native(&src(39)).1, "true\n");
    if let Some((code, out)) = run_lythos(&lythos_elf(&src(44)), &[]) {
        assert_eq!((code, out.as_str()), (0, "true\n"));
    }
}

const ALLOC_SRC: &str = r#"import std.io
import std.mem

main :: fn() do
  unsafe do
    a = mem.alloc(100)
    mem.write_u64(a, 11)
    mem.write_u8(a + 99, 7)
    io.println(mem.live())
    mem.free(a, 100)
    io.println(mem.live())
    b = mem.alloc(120)
    io.println(b == a)
    io.println(mem.read_u64(b))
    io.println(mem.read_u8(b + 99))
    mem.write_u32(b, 305419896)
    c = mem.realloc(b, 120, 1000)
    io.println(mem.read_u32(c))
    io.println(mem.live())
    mem.fill(c + 8, 170, 4)
    mem.copy(c + 100, c, 16)
    io.println(mem.read_u32(c + 100))
    io.println(mem.read_u8(c + 108))
    mem.copy(c + 2, c, 8)
    io.println(mem.read_u16(c + 2))
    mem.free(c, 1000)
    io.println(mem.live())
    m = mem.mark()
    s = String.repeat("ab", 1000)
    t = [1, 2, 3]
    io.println(String.length(s))
    mem.reset(m)
    io.println(m == mem.mark())
    u = String.repeat("z", 5)
    io.println(u)
    d = mem.alloc(16)
    io.println(mem.read_u64(d))
  end
end
"#;

const ALLOC_OUT: &str = "128\n0\ntrue\n0\n0\n305419896\n1024\n305419896\n170\n22136\n0\n2000\ntrue\nzzzzz\n0\n";

#[test]
fn manual_allocation_native() {
    let (code, out, err) = native(ALLOC_SRC);
    assert_eq!((code, out.as_str()), (0, ALLOC_OUT), "{err}");
}

#[test]
fn manual_allocation_on_lythos() {
    if let Some((code, out)) = run_lythos(&lythos_elf(ALLOC_SRC), &[]) {
        assert_eq!((code, out.as_str()), (0, ALLOC_OUT));
    }
}

#[test]
fn reset_keeps_free_lists_and_stdin_consistent() {
    let src = r#"import std.io
import std.mem

main :: fn() do
  unsafe do
    a = mem.alloc(64)
    m = mem.mark()
    b = mem.alloc(64)
    mem.free(b, 64)
    c = mem.alloc(5000)
    mem.free(c, 5000)
    mem.reset(m)
    io.println(mem.alloc(64) != a)
    x = mem.alloc(5000)
    mem.write_u64(x, 5)
    io.println(mem.read_u64(x))
    io.println(mem.read_u64(mem.alloc(5000)))
  end
end
"#;
    assert_eq!(native(src).1, "true\n5\n0\n");
}

#[test]
fn bits_and_conversions_match_the_vm() {
    let src = r#"import std.io
import std.bits

row :: fn(parts: [String]) do
  io.println(String.join(parts, " "))
end

u8s :: fn(x: u8, y: u8) do
  row([show(bits.band(x, y)), show(bits.bor(x, y)), show(bits.bxor(x, y)), show(bits.bnot(x)), show(bits.shl(x, 3)), show(bits.shr(x, 2)), show(bits.rotl(x, 3)), show(bits.rotr(x, 3)), show(bits.popcount(x)), show(bits.leading_zeros(x)), show(bits.trailing_zeros(x)), show(bits.wrapping_add(x, y)), show(bits.wrapping_sub(x, y)), show(bits.wrapping_mul(x, y))])
end

i8s :: fn(x: i8, y: i8) do
  row([show(bits.band(x, y)), show(bits.bor(x, y)), show(bits.bxor(x, y)), show(bits.bnot(x)), show(bits.shl(x, 3)), show(bits.shr(x, 2)), show(bits.rotl(x, 3)), show(bits.rotr(x, 3)), show(bits.popcount(x)), show(bits.leading_zeros(x)), show(bits.trailing_zeros(x)), show(bits.wrapping_add(x, y)), show(bits.wrapping_sub(x, y)), show(bits.wrapping_mul(x, y))])
end

i16s :: fn(x: i16, y: i16) do
  row([show(bits.band(x, y)), show(bits.bor(x, y)), show(bits.bxor(x, y)), show(bits.bnot(x)), show(bits.shl(x, 5)), show(bits.shr(x, 4)), show(bits.rotl(x, 5)), show(bits.rotr(x, 5)), show(bits.popcount(x)), show(bits.leading_zeros(x)), show(bits.trailing_zeros(x)), show(bits.wrapping_add(x, y)), show(bits.wrapping_sub(x, y)), show(bits.wrapping_mul(x, y))])
end

u32s :: fn(x: u32, y: u32) do
  row([show(bits.band(x, y)), show(bits.bor(x, y)), show(bits.bxor(x, y)), show(bits.bnot(x)), show(bits.shl(x, 7)), show(bits.shr(x, 9)), show(bits.rotl(x, 13)), show(bits.rotr(x, 13)), show(bits.popcount(x)), show(bits.leading_zeros(x)), show(bits.trailing_zeros(x)), show(bits.wrapping_add(x, y)), show(bits.wrapping_sub(x, y)), show(bits.wrapping_mul(x, y))])
end

i32s :: fn(x: i32, y: i32) do
  row([show(bits.band(x, y)), show(bits.bor(x, y)), show(bits.bxor(x, y)), show(bits.bnot(x)), show(bits.shl(x, 7)), show(bits.shr(x, 9)), show(bits.rotl(x, 13)), show(bits.rotr(x, 13)), show(bits.popcount(x)), show(bits.leading_zeros(x)), show(bits.trailing_zeros(x)), show(bits.wrapping_add(x, y)), show(bits.wrapping_sub(x, y)), show(bits.wrapping_mul(x, y))])
end

u64s :: fn(x: u64, y: u64) do
  row([show(bits.band(x, y)), show(bits.bor(x, y)), show(bits.bxor(x, y)), show(bits.bnot(x)), show(bits.shl(x, 33)), show(bits.shr(x, 17)), show(bits.rotl(x, 29)), show(bits.rotr(x, 29)), show(bits.popcount(x)), show(bits.leading_zeros(x)), show(bits.trailing_zeros(x)), show(bits.wrapping_add(x, y)), show(bits.wrapping_sub(x, y)), show(bits.wrapping_mul(x, y))])
end

i64s :: fn(x: i64, y: i64) do
  row([show(bits.band(x, y)), show(bits.bor(x, y)), show(bits.bxor(x, y)), show(bits.bnot(x)), show(bits.shl(x, 33)), show(bits.shr(x, 17)), show(bits.rotl(x, 29)), show(bits.rotr(x, 29)), show(bits.popcount(x)), show(bits.leading_zeros(x)), show(bits.trailing_zeros(x)), show(bits.wrapping_add(x, y)), show(bits.wrapping_sub(x, y)), show(bits.wrapping_mul(x, y))])
end

main :: fn() do
  u8s(200, 77)
  u8s(0, 255)
  i8s(-100, 77)
  i8s(-128, -1)
  i8s(0, 127)
  i16s(-12345, 23456)
  i16s(-32768, 32767)
  u32s(3000000000, 123456789)
  u32s(0, 4294967295)
  i32s(-2000000000, 1999999999)
  i32s(-2147483648, -1)
  u64s(18000000000000000000, 12345678901234567)
  u64s(0, 18446744073709551615)
  i64s(-9000000000000000000, 8999999999999999999)
  i64s(-9223372036854775808, -1)
  row([show(Num.to_i8(200)), show(Num.to_i8(-128)), show(Num.to_i16(40000)), show(Num.to_i16(-3)), show(Num.to_u16(65536)), show(Num.to_u16(65535)), show(Num.to_isize(-5)), show(Num.to_i8(127.9)), show(Num.to_u16(-1.5)), show(Num.to_u8(255)), show(Num.to_i16(3.7e5))])
end
"#;
    same(src);
    let out = native(src).1;
    assert!(out.starts_with("72 205 133 55 64 50 70 25 3 0 3 21 123 40\n"), "{out}");
    assert!(out.ends_with("None Some(-128) None Some(-3) None Some(65535) Some(-5) Some(127) None Some(255) None\n"), "{out}");
}

#[test]
fn shift_amount_must_fit() {
    let src = "import std.io\nimport std.bits\nf :: fn(x: u8, n: u32) -> u8 do\n  bits.shl(x, n)\nend\nmain :: fn() do\n  io.println(f(1, 8))\nend\n";
    let (code, _, err) = native(src);
    assert_ne!(code, 0);
    assert!(err.contains("shift amount"), "{err}");
}

#[test]
fn record_update_and_string_escapes_match_the_vm() {
    let src = r#"import std.io

Point :: type { x: i32, y: i32 }
Pair<T> :: type { a: T, b: String }

move :: fn(p: Point, dx: i32) -> Point do
  {p | x: p.x + dx}
end

main :: fn() do
  p = Point{x: 1, y: 2}
  q = move(p, 10)
  r = {q | x: 0, y: 99}
  io.println([p.x, p.y, q.x, q.y, r.x, r.y])
  s = Pair{a: 1.5, b: "hi"}
  io.println({s | a: 2.5}.a)
  io.println({s | b: "yo"}.b)
  io.println(s.b)
  io.println("a\x41\x7e|\u{e9}\u{1F600}|\u{41}|\e")
  io.println(String.length("\u{e9}\x00"))
  io.println(Option.unwrap_or(Num.to_i8(-128), 0) == -128)
end
"#;
    same(src);
    assert_eq!(native(src).1, "[1, 2, 11, 2, 0, 99]\n2.5\nyo\nhi\naA~|\u{e9}\u{1F600}|A|\u{1b}\n3\ntrue\n");
}

const DEEP_SRC: &str = "import std.io\ntotal :: fn(n: i64) -> i64 do\n  if n == 0 do 0 else n + total(n - 1) end\nend\nmain :: fn() do\n  io.println(total(1000))\n  io.println(total(1000000000))\nend\n";

#[test]
fn deep_recursion_is_a_runtime_error_not_a_crash() {
    let (code, out, err) = native(DEEP_SRC);
    assert_eq!((code, out.as_str()), (1, "500500\n"));
    assert!(err.contains("stack overflow"), "{err}");
    if let Some((code, out)) = run_lythos(&lythos_elf(DEEP_SRC), &[]) {
        // The Lythos console carries both streams.
        assert_eq!(code, 1);
        assert!(out.starts_with("500500\n") && out.contains("stack overflow"), "{out}");
    }
}

const ARRAY_SRC: &str = r#"import std.io
import std.array

Item :: type { name: String, qty: i32 }

main :: fn() do
  a = Array.new()
  var i = 0
  while i < 100000 do
    Array.push(a, i * 2)
    i := i + 1
  end
  io.println(Array.length(a))
  io.println(Array.get(a, 99999))
  total = Array.to_list(a) |> List.fold(0, fn(acc, x) -> acc + Option.unwrap_or(Num.to_i64(x), 0) end)
  io.println(total)
  b = a
  Array.set(b, 0, 7)
  io.println(Array.get(a, 0))
  c = Array.copy(a)
  Array.set(c, 0, 8)
  io.println([Array.get(a, 0), Array.get(c, 0)])
  items = Array.from_list([Item{name: "bolt", qty: 3}, Item{name: "nut", qty: 5}])
  Array.insert(items, 1, Item{name: "washer", qty: 9})
  Array.push(items, Item{name: "screw", qty: 1})
  Array.swap(items, 0, 3)
  io.println(Array.to_list(items) |> List.map(fn(it) -> it.name end))
  io.println(Array.remove(items, 2).qty)
  words = Array.filled(2, "ab")
  Array.push(words, "cd")
  io.println(words)
  floats = Array.from_list([1.5, 2.5])
  Array.set(floats, 1, 9.25)
  io.println(floats)
  flags = Array.filled(3, true)
  Array.set(flags, 1, false)
  io.println(flags)
  small = Array.from_list([Option.unwrap_or(Num.to_i8(0), 1)])
  Array.push(small, Option.unwrap_or(Num.to_i8(-128), 1))
  io.println(small)
  while Option.is_some(Array.pop(a)) do
    ()
  end
  io.println(Array.length(a))
end
"#;

const ARRAY_FAIL: &str = "import std.io\nimport std.array\nmain :: fn() do\n  a = Array.from_list([1, 2, 3])\n  io.println(Array.length(a))\n  io.println(Array.remove(a, 3))\nend\n";

#[test]
fn arrays_match_the_vm() {
    same(ARRAY_SRC);
    let (code, out, _) = native(ARRAY_SRC);
    assert_eq!(code, 0);
    assert_eq!(out, "100000\nSome(199998)\n9999900000\nSome(7)\n[Some(7), Some(8)]\n[\"screw\", \"washer\", \"nut\", \"bolt\"]\n5\n[\"ab\", \"ab\", \"cd\"]\n[1.5, 9.25]\n[true, false, true]\n[0, -128]\n0\n");
    let (code, out, err) = native(ARRAY_FAIL);
    assert_eq!((code, out.as_str()), (1, "3\n"));
    assert!(err.contains("index 3 is out of range for an array of length 3"), "{err}");
}

#[test]
fn arrays_on_lythos() {
    if let Some((code, out)) = run_lythos(&lythos_elf(ARRAY_SRC), &[]) {
        assert_eq!(code, 0);
        assert!(out.contains("9999900000\nSome(7)\n") && out.ends_with("[0, -128]\n0\n"), "{out}");
    }
    if let Some((code, out)) = run_lythos(&lythos_elf(ARRAY_FAIL), &[]) {
        assert_eq!(code, 1);
        assert!(out.starts_with("3\n") && out.contains("index 3 is out of range for an array of length 3"), "{out}");
    }
}

#[test]
fn interpreters_refuse_raw_memory_with_a_clear_message() {
    let src = "import std.mem\nmain :: fn() do\n  unsafe do\n    mem.read_u8(0)\n  end\nend\n";
    let r = cera_eval::run_captured_on(cera_eval::Engine::Vm, src, "", vec![], std::env::temp_dir());
    let msg = format!("{:?}", r.map(|c| c.err));
    assert!(msg.contains("needs the native backend"), "{msg}");
}

const REPR_SRC: &str = r#"import std.io
import std.mem

Inner :: repr type { a: u8, b: i16 }

Header :: repr type { magic: u32, len: i16, flags: u8, ok: bool, delta: i64, small: i8, count: usize, inner: Inner, tag: [u8; 4], neg: i32 }

main :: fn() do
  io.println([header_size(), header_align(), header_delta_offset(), header_inner_offset(), header_tag_offset(), header_neg_offset(), inner_size()])
  unsafe do
    buf = mem.alloc(header_size())
    mem.fill(buf, 0, header_size())
    h = Header{magic: 3735928559, len: -2, flags: 7, ok: true, delta: -123456789012, small: -5, count: 42, inner: Inner{a: 9, b: -300}, tag: [1, 2, 3], neg: -70000}
    header_write(buf, h)
    io.println(mem.read_u16(buf + 4))
    r = header_read(buf)
    io.println(r.magic)
    io.println(r.len)
    io.println(r.flags)
    io.println(r.ok)
    io.println(r.delta)
    io.println(r.small)
    io.println(r.count)
    io.println(r.inner.a)
    io.println(r.inner.b)
    io.println(r.tag)
    io.println(r.neg)
    mem.free(buf, header_size())
  end
end
"#;
const REPR_OUT: &str = "[48, 8, 8, 32, 36, 40, 4]\n65534\n3735928559\n-2\n7\ntrue\n-123456789012\n-5\n42\n9\n-300\n[1, 2, 3, 0]\n-70000\n";

#[test]
fn repr_records_lay_out_like_c_native() {
    let (code, out, err) = native(REPR_SRC);
    assert_eq!((code, out.as_str()), (0, REPR_OUT), "{err}");
}

#[test]
fn repr_records_on_lythos() {
    if let Some((code, out)) = run_lythos(&lythos_elf(REPR_SRC), &[]) {
        assert_eq!((code, out.as_str()), (0, REPR_OUT));
    }
}

#[test]
fn repr_signed_extremes_round_trip() {
    let src = r#"import std.io
import std.mem

E :: repr type { a: i64, b: i32, c: i16, d: i8 }

main :: fn() do
  unsafe do
    p = mem.alloc(e_size())
    e_write(p, E{a: -9223372036854775807 - 1, b: -2147483648, c: -32768, d: -128})
    r = e_read(p)
    io.println(r.a)
    io.println(r.b)
    io.println(r.c)
    io.println(r.d)
    e_write(p, E{a: 9223372036854775807, b: 2147483647, c: 32767, d: 127})
    r2 = e_read(p)
    io.println(r2.a)
    io.println(r2.b)
    io.println(r2.c)
    io.println(r2.d)
    io.println(mem.read_u8(p + 8))
  end
end
"#;
    let (code, out, err) = native(src);
    assert_eq!(
        (code, out.as_str()),
        (0, "-9223372036854775808\n-2147483648\n-32768\n-128\n9223372036854775807\n2147483647\n32767\n127\n255\n"),
        "{err}"
    );
}

#[test]
fn bit_operators_and_indexing_match_the_vm() {
    same(r#"import std.io
import std.array

main :: fn() do
  a = 12
  b = 10
  io.println(a & b)
  io.println(a ^ b)
  io.println(1 << 4 + 1)
  io.println(256 >> 4)
  io.println(~0)
  io.println(-16 >> 2)
  io.println(~5)
  io.println((a & 8) != 0)
  xs = [10, 20, 30]
  io.println(xs[2])
  arr = array.from_list(["x", "y"])
  io.println(arr[1])
end
"#);
}

#[test]
fn index_out_of_range_is_a_runtime_error_native() {
    let (code, out, _err) = native("main :: fn() do\n  xs = [1, 2]\n  xs[2]\nend\n");
    assert_ne!(code, 0);
    assert!(out.is_empty() || !out.contains("3"));
}

#[test]
fn regions_free_memory_native() {
    let src = r#"import std.io
import std.list
import std.string

label :: fn(n: i64) -> String do
  region do
    parts = List.map(List.range(0, 50), fn(x) -> show(x + n) end)
    String.concat(parts)
  end
end

line :: fn() -> String do
  region do
    l = io.read_line()
    String.concat(["<", Option.unwrap_or(l, "eof"), ">"])
  end
end

main :: fn() do
  var i = 0
  var keep = ""
  while i < 3000 do
    keep := label(i)
    i := i + 1
  end
  io.println(keep)
  io.println(line())
  io.println(line())
  io.println(line())
  io.println(label(7))
end
"#;
    same_io(src, "first\nsecond\n", &[]);
}

#[test]
fn regions_keep_state_that_escapes() {
    // Writing a region value into an outer var or array must keep it alive.
    same(r#"import std.io
import std.array

main :: fn() do
  var outer = [0]
  var i = 0
  while i < 3 do
    region do
      outer := [i, i * 2]
    end
    i := i + 1
  end
  io.println(outer)
  a = array.new()
  region do
    array.push(a, "kept")
  end
  junk = List.map(List.range(0, 100), fn(x) -> show(x) end)
  io.println(array.to_list(a))
  io.println(List.length(junk))
end
"#);
}

#[test]
fn regions_on_lythos() {
    let src = r#"import std.io
import std.list

work :: fn(n: i64) -> i64 do
  region do
    xs = List.range(0, n)
    List.fold(List.map(xs, fn(x) -> x * 2 end), 0, fn(a, b) -> a + b end)
  end
end

main :: fn() do
  var i = 0
  var total = 0
  while i < 3000 do
    total := total + work(1000)
    i := i + 1
  end
  io.println(total)
end
"#;
    if let Some((code, out)) = run_lythos(&lythos_elf(src), &[]) {
        assert_eq!((code, out.as_str()), (0, "2997000000\n"));
    }
}

const ASM_SRC: &str = r#"import std.io

add3 :: fn(a: i64, b: i64, c: i64) -> i64 do
  unsafe do
    asm("mov rax, rdi
         add rax, rsi
         add rax, rdx", a, b, c)
  end
end

sum_to :: fn(n: i64) -> i64 do
  unsafe do
    asm("xor eax, eax
         test rdi, rdi
         jz done
       again:
         add rax, rdi
         dec rdi
         jnz again
       done:", n)
  end
end

main :: fn() do
  io.println(add3(1, 2, 3))
  io.println(sum_to(100))
  io.println(unsafe do asm("mov rax, 7; ret; mov rax, 8") end)
  io.println(unsafe do asm("push rbx; mov rbx, 5; mov rax, rbx; pop rbx") end)
  io.println(unsafe do asm("mov rax, rdi; shl rax, cl; bswap eax", 1, 0, 0, 4) end)
end
"#;
const ASM_OUT: &str = "6\n5050\n7\n5\n268435456\n";

#[test]
fn inline_assembly_native() {
    let (code, out, err) = native(ASM_SRC);
    assert_eq!((code, out.as_str()), (0, ASM_OUT), "{err}");
}

#[test]
fn inline_assembly_on_lythos() {
    if let Some((code, out)) = run_lythos(&lythos_elf(ASM_SRC), &[]) {
        assert_eq!((code, out.as_str()), (0, ASM_OUT));
    }
}

#[test]
fn inline_assembly_needs_the_native_backend_in_the_vm() {
    let r = cera_eval::run_captured_on(cera_eval::Engine::Vm, "main :: fn() do\n  unsafe do\n    asm(\"nop\")\n  end\nend\n", "", vec![], std::env::temp_dir());
    let text = match r {
        Ok(c) => c.err,
        Err(e) => format!("{e:?}"),
    };
    assert!(text.contains("native backend"), "{text}");
}
