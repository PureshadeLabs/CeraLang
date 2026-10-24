use cera_eval::{native_names, run_captured_with, Captured, RunError};
use std::path::PathBuf;

fn exec(src: &str, input: &str, args: &[&str], root: PathBuf) -> Captured {
    match run_captured_with(src, input, args.iter().map(|s| s.to_string()).collect(), root) {
        Ok(c) => c,
        Err(RunError::Syntax(d)) | Err(RunError::Type(d)) => panic!("compile errors: {d:?}\n{src}"),
        Err(RunError::Runtime(e)) => panic!("runtime error: {}\n{src}", e.message),
    }
}

/// Run a body inside `main` (with the std modules imported) and return stdout.
fn body(b: &str) -> String {
    let src = format!(
        "import std.io\nimport std.string\nimport std.map\nimport std.set\nimport std.option\nimport std.result\nimport std.num\nmain :: fn() do\n{b}\nend\n"
    );
    exec(&src, "", &[], std::env::current_dir().unwrap()).out
}

fn body_err(b: &str) -> String {
    let src = format!("import std.io\nimport std.list\nimport std.num\nmain :: fn() do\n{b}\nend\n");
    match run_captured_with(&src, "", vec![], std::env::current_dir().unwrap()) {
        Err(RunError::Runtime(e)) => e.message,
        other => panic!("expected a runtime error, got {other:?}"),
    }
}

fn temp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cera-std-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn every_stub_has_a_native() {
    let names = native_names();
    for (module, name) in cera_sema::stub_functions() {
        let short = module.rsplit('.').next().unwrap();
        let id = format!("{short}.{name}");
        assert!(names.contains(&id.as_str()), "no native for stub `{module}::{name}` (expected `{id}`)");
    }
    // and no native without a stub
    let stubs: Vec<String> = cera_sema::stub_functions()
        .into_iter()
        .map(|(m, n)| format!("{}.{}", m.rsplit('.').next().unwrap(), n))
        .collect();
    for n in names {
        assert!(stubs.contains(&n.to_string()), "native `{n}` has no stub");
    }
}

#[test]
fn list_functions() {
    assert_eq!(body("  io.println(List.get([10, 20, 30], 1))\n  io.println(List.get([10], 5))\n  io.println(List.last([1, 2, 3]))\n  io.println(List.is_empty([1]))"), "Some(20)\nNone\nSome(3)\nfalse\n");
    assert_eq!(body("  io.println(List.take([1, 2, 3, 4], 2))\n  io.println(List.drop([1, 2, 3, 4], 3))\n  io.println(List.drop([1], 9))\n  io.println(List.take([1], 9))"), "[1, 2]\n[4]\n[]\n[1]\n");
    assert_eq!(body("  io.println(List.concat([[1], [2, 3], []]))\n  io.println(List.flat_map([1, 2], fn(x) -> [x, x * 10] end))"), "[1, 2, 3]\n[1, 10, 2, 20]\n");
    assert_eq!(body("  io.println(List.any([1, 2, 3], fn(x) -> x > 2 end))\n  io.println(List.all([1, 2, 3], fn(x) -> x > 2 end))\n  io.println(List.any([], fn(x) -> x > 2 end))\n  io.println(List.all([], fn(x) -> x > 2 end))"), "true\nfalse\nfalse\ntrue\n");
    assert_eq!(body("  io.println(List.find([1, 2, 3], fn(x) -> x > 1 end))\n  io.println(List.zip([1, 2, 3], [\"a\", \"b\"]))\n  io.println(List.range(2, 6))\n  io.println(List.contains([1, 2], 2))"), "Some(2)\n[(1, \"a\"), (2, \"b\")]\n[2, 3, 4, 5]\ntrue\n");
}

#[test]
fn sorting() {
    assert_eq!(body("  io.println(List.sort([3, 1, 2]))\n  io.println(List.sort([\"b\", \"a\", \"c\"]))\n  io.println(List.sort([(2, \"x\"), (1, \"y\"), (2, \"a\")]))"), "[1, 2, 3]\n[\"a\", \"b\", \"c\"]\n[(1, \"y\"), (2, \"a\"), (2, \"x\")]\n");
    assert_eq!(
        body("  desc = fn(a, b) -> if a > b do Less else if a < b do Greater else Equal end end\n  io.println(List.sort_by([1, 3, 2], desc))"),
        "[3, 2, 1]\n"
    );
}

#[test]
fn sort_is_stable() {
    let src = "import std.io\nItem :: type { key: i32, tag: String }\nby_key :: fn(a: Item, b: Item) -> Ordering do\n  if a.key < b.key do Less else if a.key > b.key do Greater else Equal end\nend\nmain :: fn() do\n  items = [Item{key: 2, tag: \"a\"}, Item{key: 1, tag: \"b\"}, Item{key: 2, tag: \"c\"}, Item{key: 1, tag: \"d\"}]\n  List.sort_by(items, by_key) |> List.map(fn(i) -> i.tag end) |> io.println\nend\n";
    assert_eq!(exec(src, "", &[], PathBuf::from(".")).out, "[\"b\", \"d\", \"a\", \"c\"]\n");
}

#[test]
fn string_functions() {
    assert_eq!(body("  io.println(String.is_empty(\"\"))\n  io.println(String.contains(\"hello\", \"ell\"))\n  io.println(String.starts_with(\"hello\", \"he\"))\n  io.println(String.ends_with(\"hello\", \"lo\"))"), "true\ntrue\ntrue\ntrue\n");
    assert_eq!(body("  io.println(String.split(\"a,b,c\", \",\"))\n  io.println(String.join([\"a\", \"b\"], \"-\"))\n  io.println(String.trim(\"  hi \"))"), "[\"a\", \"b\", \"c\"]\na-b\nhi\n");
    assert_eq!(body("  io.println(String.to_upper(\"abc\"))\n  io.println(String.to_lower(\"ABC\"))\n  io.println(String.replace(\"a-b-c\", \"-\", \"+\"))\n  io.println(String.repeat(\"ab\", 3))"), "ABC\nabc\na+b+c\nababab\n");
    assert_eq!(body("  io.println(String.lines(\"a\\nb\\r\\nc\"))\n  io.println(String.chars(\"hi\"))\n  io.println(String.from_chars(String.chars(\"hey\")))"), "[\"a\", \"b\", \"c\"]\n['h', 'i']\nhey\n");
    assert_eq!(body("  io.println(String.parse_i64(\"42\"))\n  io.println(String.parse_i64(\"x\"))\n  io.println(String.parse_f64(\"2.5\"))\n  io.println(String.slice(\"hello\", 1, 3))\n  io.println(String.slice(\"hello\", 4, 3))"), "Some(42)\nNone\nSome(2.5)\nSome(\"ell\")\nNone\n");
}

#[test]
fn split_with_empty_separator_is_an_error() {
    let src = "import std.string\nmain :: fn() do\n  String.split(\"abc\", \"\")\nend\n";
    match run_captured_with(src, "", vec![], PathBuf::from(".")) {
        Err(RunError::Runtime(e)) => assert!(e.message.contains("separator"), "{}", e.message),
        other => panic!("{other:?}"),
    }
}

#[test]
fn option_and_result_functions() {
    assert_eq!(body("  io.println(Option.is_some(Some(1)))\n  io.println(Option.is_none(Some(1)))\n  io.println(Option.unwrap_or(None, 7))\n  io.println(Option.map(Some(2), fn(x) -> x * 3 end))\n  io.println(Option.and_then(Some(2), fn(x) -> if x > 5 do Some(x) else None end end))"), "true\nfalse\n7\nSome(6)\nNone\n");
    assert_eq!(body("  io.println(Option.ok_or(Some(1), \"no\"))\n  io.println(Option.ok_or(None, \"no\"))"), "Ok(1)\nErr(\"no\")\n");
    assert_eq!(body("  r = Ok(3)\n  io.println(Result.is_ok(r))\n  io.println(Result.map(r, fn(x) -> x + 1 end))\n  io.println(Result.unwrap_or(Err(\"e\"), 0))\n  io.println(Result.map_err(Err(\"e\"), fn(s) -> String.concat([s, \"!\"]) end))\n  io.println(Result.ok(r))"), "true\nOk(4)\n0\nErr(\"e!\")\nSome(3)\n");
    assert_eq!(body("  io.println(Result.and_then(Ok(2), fn(x) -> if x > 1 do Ok(x * 2) else Err(\"small\") end end))\n  io.println(Result.is_err(Err(1)))"), "Ok(4)\ntrue\n");
}

#[test]
fn num_functions() {
    assert_eq!(body("  io.println(Num.min(3, 9))\n  io.println(Num.max(3, 9))\n  io.println(Num.clamp(15, 0, 10))\n  io.println(Num.clamp(-5, 0, 10))\n  io.println(Num.min(\"b\", \"a\"))"), "3\n9\n10\n0\na\n");
    assert_eq!(body("  io.println(Num.abs(-4))\n  io.println(Num.abs(2.5))\n  io.println(Num.pow(2, 10))\n  io.println(Num.pow(2.0, 3))\n  io.println(Num.sqrt(9.0))"), "4\n2.5\n1024\n8.0\n3.0\n");
    assert_eq!(body("  io.println(Num.floor(2.7))\n  io.println(Num.ceil(2.1))\n  io.println(Num.round(2.5))\n  io.println(Num.to_f64(3))"), "2.0\n3.0\n3.0\n3.0\n");
    assert_eq!(body("  io.println(Num.to_u8(300))\n  io.println(Num.to_u8(200))\n  io.println(Num.to_i32(-5))\n  io.println(Num.to_usize(-1))\n  io.println(Num.to_i64(3.9))"), "None\nSome(200)\nSome(-5)\nNone\nSome(3)\n");
}

#[test]
fn pow_overflow_is_reported() {
    let m = body_err("  io.println(Num.pow(10, 40))");
    assert!(m.contains("overflow"), "{m}");
}

#[test]
fn map_operations() {
    let out = body(
        "  m = Map.new() |> Map.insert(\"b\", 2) |> Map.insert(\"a\", 1) |> Map.insert(\"b\", 20)\n  io.println(m)\n  io.println(Map.get(m, \"a\"))\n  io.println(Map.get(m, \"z\"))\n  io.println(Map.get_or(m, \"z\", 0))\n  io.println(Map.contains_key(m, \"b\"))\n  io.println(Map.size(m))\n  io.println(Map.keys(m))\n  io.println(Map.values(m))\n  io.println(Map.to_list(m))\n  io.println(Map.remove(m, \"a\"))\n  io.println(Map.is_empty(Map.new()))",
    );
    assert_eq!(
        out,
        "{\"a\": 1, \"b\": 20}\nSome(1)\nNone\n0\ntrue\n2\n[\"a\", \"b\"]\n[1, 20]\n[(\"a\", 1), (\"b\", 20)]\n{\"b\": 20}\ntrue\n"
    );
}

#[test]
fn map_from_list_and_merge() {
    let out = body("  m = Map.from_list([(1, \"x\"), (2, \"y\"), (1, \"z\")])\n  io.println(m)\n  io.println(Map.merge(m, Map.from_list([(2, \"Y\"), (3, \"w\")])))\n  io.println(m == Map.from_list([(2, \"y\"), (1, \"z\")]))");
    assert_eq!(out, "{1: \"z\", 2: \"y\"}\n{1: \"z\", 2: \"Y\", 3: \"w\"}\ntrue\n");
}

#[test]
fn maps_are_persistent() {
    let out = body("  a = Map.insert(Map.new(), 1, 1)\n  b = Map.insert(a, 2, 2)\n  io.println(Map.size(a))\n  io.println(Map.size(b))");
    assert_eq!(out, "1\n2\n");
}

#[test]
fn set_operations() {
    let out = body(
        "  s = Set.from_list([3, 1, 2, 3])\n  io.println(s)\n  io.println(Set.contains(s, 2))\n  io.println(Set.contains(s, 9))\n  io.println(Set.size(s))\n  io.println(Set.to_list(Set.remove(s, 1)))\n  io.println(Set.union(s, Set.from_list([9, 1])))\n  io.println(Set.is_empty(Set.new()))\n  io.println(Set.insert(s, 2) == s)",
    );
    assert_eq!(out, "#{1, 2, 3}\ntrue\nfalse\n3\n[2, 3]\n#{1, 2, 3, 9}\ntrue\ntrue\n");
}

#[test]
fn map_with_float_nan_key_is_an_error() {
    let m = body_err("  io.println(1)\n  io.println(Num.min(0.0 / 0.0, 1.0))");
    assert!(m.contains("NaN"), "{m}");
}

#[test]
fn show_for_tuples_error_and_ordering() {
    assert_eq!(body("  io.println((1, \"a\") < (1, \"b\"))\n  io.println([1, 2] < [1, 3])\n  io.println([1, 2] < [1, 2, 0])"), "true\ntrue\ntrue\n");
}

#[test]
fn stdin_and_stderr() {
    let src = "import std.io\nmain :: fn() do\n  match io.read_line() do\n    Some(l) -> io.println(l)\n    None -> io.println(\"eof\")\n  end\n  io.eprintln(\"warn\")\n  io.println(io.read_all())\n  io.println(io.read_line())\nend\n";
    let c = exec(src, "first\r\nsecond\nthird\n", &[], PathBuf::from("."));
    assert_eq!(c.out, "first\nsecond\nthird\n\nNone\n");
    assert_eq!(c.err, "warn\n");
    assert_eq!(c.code, 0);
}

#[test]
fn capabilities_are_passed_to_main() {
    let src = "import std.io\nimport std.env\nmain :: fn(caps: Caps) do\n  io.writeln(caps.console, \"hello\")\n  io.write(caps.console, \"a\")\n  io.writeln(caps.console, \"b\")\n  io.println(env.args(caps.env))\n  io.println(env.var(caps.env, \"CERA_SURELY_UNSET_VARIABLE\"))\nend\n";
    let c = exec(src, "", &["x", "y z"], PathBuf::from("."));
    assert_eq!(c.out, "hello\nab\n[\"x\", \"y z\"]\nNone\n");
}

#[test]
fn read_line_from_console() {
    let src = "import std.io\nmain :: fn(caps: Caps) do\n  io.println(io.read_line_from(caps.console))\nend\n";
    assert_eq!(exec(src, "abc\n", &[], PathBuf::from(".")).out, "Some(\"abc\")\n");
}

#[test]
fn capabilities_can_be_narrowed_and_handed_down() {
    let src = "import std.io\nshout :: fn(c: Console, s: String) do\n  io.writeln(c, String.to_upper(s))\nend\nmain :: fn(caps: Caps) do\n  shout(caps.console, \"hi\")\nend\n";
    assert_eq!(exec(&format!("import std.string\n{src}"), "", &[], PathBuf::from(".")).out, "HI\n");
}

#[test]
fn exit_codes() {
    let src = "import std.io\nimport std.process\nmain :: fn() do\n  io.println(\"before\")\n  process.exit(3)\n  io.println(\"after\")\nend\n";
    let c = exec(src, "", &[], PathBuf::from("."));
    assert_eq!(c.out, "before\n");
    assert_eq!(c.code, 3);
}

#[test]
fn fs_roundtrip() {
    let root = temp_root("roundtrip");
    let src = "import std.io\nimport std.fs\nmain :: fn(caps: Caps) do\n  d = caps.fs\n  io.println(fs.exists(d, \"a.txt\"))\n  io.println(fs.write_string(d, \"a.txt\", \"one\\n\"))\n  io.println(fs.append_string(d, \"a.txt\", \"two\\n\"))\n  io.println(fs.read_to_string(d, \"a.txt\"))\n  io.println(fs.make_dir(d, \"sub\"))\n  match fs.open_dir(d, \"sub\") do\n    Ok(sub) -> io.println(fs.write_string(sub, \"b.txt\", \"x\"))\n    Err(e) -> io.println(e)\n  end\n  io.println(fs.list(d))\n  io.println(fs.remove(d, \"a.txt\"))\n  io.println(fs.exists(d, \"a.txt\"))\nend\n";
    let c = exec(src, "", &[], root.clone());
    assert_eq!(
        c.out,
        "false\nOk(())\nOk(())\nOk(\"one\\ntwo\\n\")\nOk(())\nOk(())\nOk([\"a.txt\", \"sub\"])\nOk(())\nfalse\n"
    );
    assert_eq!(std::fs::read_to_string(root.join("sub/b.txt")).unwrap(), "x");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fs_errors_are_values() {
    let root = temp_root("errors");
    let src = "import std.io\nimport std.fs\nmain :: fn(caps: Caps) do\n  match fs.read_to_string(caps.fs, \"missing.txt\") do\n    Ok(s) -> io.println(s)\n    Err(e) -> io.println(String.contains(e.message, \"missing.txt\"))\n  end\nend\n";
    let c = exec(&format!("import std.string\n{src}"), "", &[], root.clone());
    assert_eq!(c.out, "true\n");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fs_rejects_paths_that_escape() {
    let root = temp_root("escape");
    let outer = root.parent().unwrap().join(format!("cera-secret-{}.txt", std::process::id()));
    std::fs::write(&outer, "secret").unwrap();
    let src = format!(
        "import std.io\nimport std.fs\ntry :: fn(d: Dir, name: String) do\n  match fs.read_to_string(d, name) do\n    Ok(s) -> io.println(s)\n    Err(e) -> io.println(\"denied\")\n  end\nend\nmain :: fn(caps: Caps) do\n  try(caps.fs, \"../{}\")\n  try(caps.fs, \"/etc/passwd\")\n  try(caps.fs, \"a/../../x\")\n  io.println(fs.exists(caps.fs, \"../{}\"))\nend\n",
        outer.file_name().unwrap().to_string_lossy(),
        outer.file_name().unwrap().to_string_lossy()
    );
    let c = exec(&src, "", &[], root.clone());
    assert_eq!(c.out, "denied\ndenied\ndenied\nfalse\n");
    let _ = std::fs::remove_file(outer);
    let _ = std::fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn fs_rejects_symlinks_that_escape() {
    let root = temp_root("symlink");
    let outside = temp_root("symlink-outside");
    std::fs::write(outside.join("s.txt"), "secret").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
    let src = "import std.io\nimport std.fs\nmain :: fn(caps: Caps) do\n  match fs.read_to_string(caps.fs, \"link/s.txt\") do\n    Ok(s) -> io.println(s)\n    Err(e) -> io.println(\"denied\")\n  end\nend\n";
    assert_eq!(exec(src, "", &[], root.clone()).out, "denied\n");
    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(outside);
}

#[test]
fn read_only_directories_refuse_writes() {
    let root = temp_root("readonly");
    std::fs::write(root.join("f.txt"), "data").unwrap();
    let src = "import std.io\nimport std.fs\nmain :: fn(caps: Caps) do\n  ro = fs.read_only(caps.fs)\n  io.println(fs.read_to_string(ro, \"f.txt\"))\n  match fs.write_string(ro, \"f.txt\", \"x\") do\n    Ok(_) -> io.println(\"wrote\")\n    Err(e) -> io.println(e)\n  end\n  match fs.open_dir(ro, \".\") do\n    Ok(d) -> io.println(fs.remove(d, \"f.txt\"))\n    Err(e) -> io.println(e)\n  end\nend\n";
    let c = exec(src, "", &[], root.clone());
    assert_eq!(c.out, "Ok(\"data\")\nthis directory is read-only\nErr(this directory is read-only)\n");
    assert_eq!(std::fs::read_to_string(root.join("f.txt")).unwrap(), "data");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn clock_capability() {
    let src = "import std.io\nimport std.time\nmain :: fn(caps: Caps) do\n  t0 = time.now_ms(caps.clock)\n  time.sleep_ms(caps.clock, 5)\n  t1 = time.now_ms(caps.clock)\n  io.println(t1 >= t0 + 5)\n  io.println(t0 > 1000000000000)\nend\n";
    assert_eq!(exec(src, "", &[], PathBuf::from(".")).out, "true\ntrue\n");
}

#[test]
fn word_count_program() {
    let src = r#"import std.io
import std.string
import std.map

count_words :: fn(words: [String]) -> Map<String, i32> do
  List.fold(words, Map.new(), fn(counts, w) -> Map.insert(counts, w, Map.get_or(counts, w, 0) + 1) end)
end

main :: fn() do
  text = "the cat and the hat and the bat"
  counts = text |> String.split(" ") |> count_words
  Map.to_list(counts) |> io.println
end
"#;
    let c = exec(src, "", &[], PathBuf::from("."));
    assert_eq!(
        c.out,
        "[(\"and\", 2), (\"bat\", 1), (\"cat\", 1), (\"hat\", 1), (\"the\", 3)]\n"
    );
}

#[test]
fn wordcount_example_runs() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let src = std::fs::read_to_string(root.join("examples/wordcount.cera")).unwrap();
    let c = exec(&src, "", &[], root);
    let lines: Vec<&str> = c.out.lines().collect();
    assert_eq!(lines.len(), 10, "{}", c.out);
    assert!(lines.iter().all(|l| l.contains(": ")), "{}", c.out);
}
