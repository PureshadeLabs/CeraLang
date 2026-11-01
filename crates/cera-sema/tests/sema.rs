use cera_sema::analyze_source;

fn errors(src: &str) -> Vec<String> {
    match analyze_source(src) {
        Ok(a) => a.diagnostics.into_iter().map(|d| d.message).collect(),
        Err(d) => panic!("syntax errors: {d:?}"),
    }
}

fn ok(src: &str) {
    let e = errors(src);
    assert!(e.is_empty(), "unexpected errors: {e:#?}\n{src}");
}

fn fails(src: &str, needle: &str) {
    let e = errors(src);
    assert!(
        e.iter().any(|m| m.contains(needle)),
        "expected an error containing {needle:?}, got {e:#?}\n{src}"
    );
}

#[test]
fn examples_check() {
    for name in ["hello", "shapes"] {
        let path = format!("{}/../../examples/{name}.cera", env!("CARGO_MANIFEST_DIR"));
        let src = std::fs::read_to_string(&path).unwrap();
        ok(&src);
    }
}

fn cera_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            cera_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "cera") {
            out.push(path);
        }
    }
}

#[test]
fn ceraling_solutions_check() {
    let dir = format!("{}/../../ceralings/solutions", env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![];
    cera_files(std::path::Path::new(&dir), &mut files);
    assert!(files.len() >= 20, "found only {} solutions", files.len());
    let mut bad = vec![];
    for path in files {
        let src = std::fs::read_to_string(&path).unwrap();
        let e = errors(&src);
        if !e.is_empty() {
            bad.push((path.file_name().unwrap().to_owned(), e));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn simple_function() {
    ok("add :: fn(a: i32, b: i32) -> i32 do\n  a + b\nend\n");
}

#[test]
fn return_type_mismatch() {
    fails(
        "f :: fn() -> i32 do\n  \"no\"\nend\n",
        "should return `i32`, but its body has type `String`",
    );
}

#[test]
fn integer_literal_is_not_float() {
    fails("f :: fn() -> f64 do\n  1\nend\n", "should return `f64`");
}

#[test]
fn literals_default() {
    ok("f :: fn() -> i32 do\n  1 + 2\nend\ng :: fn() -> f64 do\n  1.5 * 2.0\nend\n");
}

#[test]
fn literal_adapts_to_context() {
    ok("f :: fn(x: u8) -> u8 do\n  x + 1\nend\n");
    fails("f :: fn(x: u8, y: i32) -> u8 do\n  x + y\nend\n", "type mismatch");
}

#[test]
fn unknown_name() {
    fails("f :: fn() -> i32 do\n  nope\nend\n", "unknown name `nope`");
}

#[test]
fn unknown_type() {
    fails("f :: fn(x: Nope) do\n  x\nend\n", "unknown type `Nope`");
}

#[test]
fn duplicate_definitions() {
    fails("f :: fn() do\nend\nf :: fn() do\nend\n", "`f` is already defined");
    fails(
        "A :: type\n  | X\nend\nB :: type\n  | X\nend\n",
        "`X` is already defined",
    );
    fails("T :: type\n  | X\nend\nT :: type\n  | Y\nend\n", "type `T` is already defined");
}

#[test]
fn arity_errors() {
    fails(
        "f :: fn(a: i32) -> i32 do\n  a\nend\ng :: fn() -> i32 do\n  f(1, 2)\nend\n",
        "takes 1 argument(s) but 2 were supplied",
    );
}

#[test]
fn argument_mismatch() {
    fails(
        "f :: fn(a: i32) -> i32 do\n  a\nend\ng :: fn() -> i32 do\n  f(\"x\")\nend\n",
        "expected `i32`, found `String`",
    );
}

#[test]
fn if_rules() {
    ok("f :: fn(b: bool) -> i32 do\n  if b do 1 else 2 end\nend\n");
    fails(
        "f :: fn(b: i32) -> i32 do\n  if b do 1 else 2 end\nend\n",
        "expected `bool`",
    );
    fails(
        "f :: fn(b: bool) -> i32 do\n  if b do 1 else \"x\" end\nend\n",
        "different types",
    );
    fails(
        "f :: fn(b: bool) do\n  if b do 1 end\nend\n",
        "`if` without `else` must have type `()`",
    );
}

#[test]
fn locals_and_shadowing() {
    ok("f :: fn() -> i32 do\n  x = 1\n  x = x + 1\n  x\nend\n");
    fails("f :: fn() -> i32 do\n  y = 1\n  z\nend\n", "unknown name `z`");
}

#[test]
fn adt_and_match() {
    ok("S :: type\n  | A(i32)\n  | B\nend\nf :: fn(s: S) -> i32 do\n  match s do\n    A(n) -> n\n    B -> 0\n  end\nend\n");
}

#[test]
fn non_exhaustive_match() {
    fails(
        "S :: type\n  | A(i32)\n  | B\nend\nf :: fn(s: S) -> i32 do\n  match s do\n    A(n) -> n\n  end\nend\n",
        "non-exhaustive match: `B` is not covered",
    );
}

#[test]
fn guards_do_not_count() {
    fails(
        "f :: fn(n: i32) -> i32 do\n  match n do\n    x if x > 0 -> 1\n  end\nend\n",
        "non-exhaustive",
    );
    ok("f :: fn(n: i32) -> i32 do\n  match n do\n    x if x > 0 -> 1\n    _ -> 0\n  end\nend\n");
}

#[test]
fn literal_patterns_need_catch_all() {
    fails(
        "f :: fn(n: i32) -> i32 do\n  match n do\n    0 -> 1\n    1 -> 2\n  end\nend\n",
        "non-exhaustive match",
    );
}

#[test]
fn bool_and_tuple_patterns() {
    ok("f :: fn(a: bool, b: bool) -> i32 do\n  match (a, b) do\n    (true, true) -> 1\n    (true, false) -> 2\n    (false, _) -> 3\n  end\nend\n");
    fails(
        "f :: fn(a: bool, b: bool) -> i32 do\n  match (a, b) do\n    (true, true) -> 1\n    (false, _) -> 3\n  end\nend\n",
        "`(true, false)` is not covered",
    );
}

#[test]
fn list_patterns() {
    ok("len :: fn(xs: [i32]) -> i32 do\n  match xs do\n    [] -> 0\n    [_ | rest] -> 1 + len(rest)\n  end\nend\n");
    fails(
        "f :: fn(xs: [i32]) -> i32 do\n  match xs do\n    [] -> 0\n  end\nend\n",
        "non-exhaustive",
    );
}

#[test]
fn match_arm_type_mismatch() {
    fails(
        "f :: fn(b: bool) -> i32 do\n  match b do\n    true -> 1\n    false -> \"no\"\n  end\nend\n",
        "type mismatch",
    );
}

#[test]
fn wrong_pattern_type() {
    fails(
        "S :: type\n  | A\nend\nf :: fn(n: i32) -> i32 do\n  match n do\n    A -> 1\n    _ -> 0\n  end\nend\n",
        "type mismatch",
    );
}

#[test]
fn constructor_arity_in_pattern() {
    fails(
        "S :: type\n  | A(i32, i32)\nend\nf :: fn(s: S) -> i32 do\n  match s do\n    A(x) -> x\n  end\nend\n",
        "has 2 field(s), but the pattern has 1",
    );
}

#[test]
fn generics() {
    ok("id :: fn<T>(x: T) -> T do\n  x\nend\nf :: fn() -> i32 do\n  id(1)\nend\ng :: fn() -> String do\n  id(\"a\")\nend\n");
    fails(
        "f :: fn<T>(x: T) -> i32 do\n  x\nend\n",
        "should return `i32`, but its body has type `T`",
    );
}

#[test]
fn generic_types() {
    ok("Box<T> :: type\n  | Full(T)\n  | Empty\nend\nunwrap_or :: fn<T>(b: Box<T>, d: T) -> T do\n  match b do\n    Full(x) -> x\n    Empty -> d\n  end\nend\nmain :: fn() -> i32 do\n  unwrap_or(Full(1), 2)\nend\n");
    fails("f :: fn(x: Option) do\n  x\nend\n", "expects 1 type argument(s)");
}

#[test]
fn operators_need_numbers() {
    fails("f :: fn(a: bool) -> bool do\n  a + a\nend\n", "cannot be applied to type `bool`");
    fails(
        "f :: fn<T>(a: T, b: T) -> bool do\n  a < b\nend\n",
        "cannot be applied to type parameter `T`",
    );
    ok("f :: fn(a: String, b: String) -> bool do\n  a < b\nend\n");
}

#[test]
fn records() {
    let decl = "Point :: type { x: f64, y: f64 }\n";
    ok(&format!("{decl}f :: fn() -> f64 do\n  p = Point{{x: 1.0, y: 2.0}}\n  p.x + p.y\nend\n"));
    fails(
        &format!("{decl}f :: fn() -> Point do\n  Point{{x: 1.0}}\nend\n"),
        "missing field(s) `y`",
    );
    fails(
        &format!("{decl}f :: fn(p: Point) -> f64 do\n  p.z\nend\n"),
        "has no field `z`",
    );
    ok(&format!("{decl}f :: fn(p: Point) -> f64 do\n  match p do\n    Point{{x, y}} -> x + y\n  end\nend\n"));
}

#[test]
fn lambdas_and_higher_order() {
    ok("f :: fn(xs: [i32]) -> [i32] do\n  map(xs, fn(x) -> x * 2 end)\nend\n");
    ok("f :: fn(xs: [i32]) -> i32 do\n  xs |> map(fn(x) -> x + 1 end) |> sum\nend\n");
    fails(
        "f :: fn(xs: [i32]) -> [String] do\n  map(xs, fn(x) -> x + 1 end)\nend\n",
        "should return `[String]`",
    );
}

#[test]
fn pipes() {
    ok("double :: fn(x: i32) -> i32 do\n  x * 2\nend\nadd :: fn(a: i32, b: i32) -> i32 do\n  a + b\nend\nf :: fn() -> i32 do\n  1 |> double |> add(10)\nend\n");
    fails(
        "double :: fn(x: i32) -> i32 do\n  x * 2\nend\nf :: fn() -> i32 do\n  \"s\" |> double\nend\n",
        "expected `i32`, found `String`",
    );
}

#[test]
fn try_operator() {
    ok("f :: fn(x: Option<i32>) -> Option<i32> do\n  v = x?\n  Some(v + 1)\nend\n");
    fails(
        "f :: fn(x: Option<i32>) -> i32 do\n  x?\nend\n",
        "needs the enclosing function to return an Option",
    );
    fails(
        "f :: fn(x: Result<i32, String>) -> Result<i32, i32> do\n  v = x?\n  Ok(v)\nend\n",
        "error type `String`",
    );
    ok("f :: fn(x: Result<i32, String>) -> Result<i32, String> do\n  v = x?\n  Ok(v)\nend\n");
    fails("f :: fn(x: i32) -> i32 do\n  x?\nend\n", "only be used on Option or Result");
}

#[test]
fn modules_and_imports() {
    ok("import std.io\nmain :: fn() do\n  io.println(\"hi\")\nend\n");
    ok("import std.io as out\nmain :: fn() do\n  out.println(1)\nend\n");
    ok("import std.io\nmain :: fn() do\n  std.io.println(1)\nend\n");
    fails("main :: fn() do\n  io.println(1)\nend\n", "unknown name `io`");
    fails("import std.nothing\nmain :: fn() do\nend\n", "unknown module `std.nothing`");
    fails(
        "import std.io\nmain :: fn() do\n  io.nope(1)\nend\n",
        "module `io` has no member `nope`",
    );
    fails(
        "import std.io\nimport std.io\nmain :: fn() do\nend\n",
        "already imported",
    );
}

#[test]
fn string_namespace() {
    ok("f :: fn(s: String) -> usize do\n  String.length(s)\nend\n");
    ok("f :: fn() -> String do\n  String.concat([\"a\", \"b\"])\nend\n");
}

#[test]
fn constants() {
    ok("LIMIT :: 10\nf :: fn() -> i32 do\n  LIMIT + 1\nend\n");
    ok("A :: B + 1\nB :: 2\nf :: fn() -> i32 do\n  A\nend\n");
    fails("A :: B\nB :: A\nf :: fn() do\nend\n", "depends on itself");
}

#[test]
fn regions_and_allocators() {
    ok("f :: fn() -> i32 do\n  region do\n    x = 1\n    promote(x)\n  end\nend\n");
    fails(
        "f :: fn() do\n  with other(1) do\n  end\nend\n",
        "unknown `with` form `other`",
    );
}

#[test]
fn region_escape_is_rejected() {
    fails(
        "f :: fn() -> [i32] do\n  region do\n    temp = [3, 1, 2]\n    temp\n  end\nend\n",
        "value escapes region",
    );
    fails("f :: fn() -> [i32] do\n  region do\n    [1, 2]\n  end\nend\n", "value escapes region");
    ok("f :: fn() -> [i32] do\n  region do\n    temp = [3, 1, 2]\n    promote(temp)\n  end\nend\n");
    ok("f :: fn() -> i32 do\n  region do\n    temp = [3, 1, 2]\n    sum(temp)\n  end\nend\n");
}

#[test]
fn defer_is_checked() {
    ok("import std.io\nf :: fn() do\n  defer io.println(\"bye\")\n  io.println(\"hi\")\nend\n");
}

#[test]
fn inferred_return_type() {
    ok("f :: fn() do\n  1\nend\ng :: fn() -> i32 do\n  f()\nend\n");
    fails(
        "f :: fn() do\n  1\nend\ng :: fn() -> String do\n  f()\nend\n",
        "should return `String`",
    );
}

#[test]
fn field_on_unknown_type_is_reported() {
    fails(
        "f :: fn() do\n  g = fn(p) -> p.x end\nend\n",
        "type is not known here",
    );
}

#[test]
fn infinite_type() {
    fails(
        "f :: fn() do\n  g = fn(x) -> x(x) end\nend\n",
        "infinite type",
    );
}

#[test]
fn hover_types_are_recorded() {
    let a = analyze_source("f :: fn(x: i32) -> i32 do\n  y = x + 1\n  y\nend\n").unwrap();
    let texts: Vec<&str> = a.types.iter().map(|t| t.text.as_str()).collect();
    assert!(texts.contains(&"y: i32"), "{texts:?}");
    assert!(texts.contains(&"x: i32"), "{texts:?}");
    assert!(texts.contains(&"f: fn(i32) -> i32"), "{texts:?}");
}

#[test]
fn syntax_errors_stop_analysis() {
    assert!(analyze_source("f :: fn(\n").is_err());
}

#[test]
fn user_type_shadows_prelude() {
    ok("Option<T> :: type\n  | Some(T)\n  | None\nend\nf :: fn() -> Option<i32> do\n  Some(1)\nend\n");
}

const SHAPE_SHOW: &str = "\
Shape :: type
  | Dot
  | Sq(f64)
end

impl Show for Shape do
  show :: fn(s: Shape) -> String do
    match s do
      Dot -> \"dot\"
      Sq(_) -> \"square\"
    end
  end
end
";

#[test]
fn user_impl_of_core_trait() {
    ok(&format!(
        "import std.io\n{SHAPE_SHOW}main :: fn() do\n  io.println(Dot)\n  io.println([Dot, Sq(1.0)])\n  s = show(Sq(2.0))\n  io.println(s)\nend\n"
    ));
}

#[test]
fn missing_impl_is_reported() {
    fails(
        "import std.io\nS :: type\n  | A\nend\nmain :: fn() do\n  io.println(A)\nend\n",
        "the trait `Show` is not implemented for `S`",
    );
}

#[test]
fn nested_missing_impl() {
    fails(
        "import std.io\nS :: type\n  | A\nend\nmain :: fn() do\n  io.println([A])\nend\n",
        "the trait `Show` is not implemented for `S`",
    );
}

#[test]
fn user_trait_with_bounds() {
    ok("\
Area :: trait
  area :: fn(self) -> f64
end
Sq :: type { side: f64 }
impl Area for Sq do
  area :: fn(s: Sq) -> f64 do
    s.side * s.side
  end
end
total :: fn<T: Area>(xs: [T]) -> f64 do
  match xs do
    [] -> 0.0
    [x | rest] -> area(x) + total(rest)
  end
end
main :: fn() -> f64 do
  total([Sq{side: 2.0}, Sq{side: 3.0}])
end
");
}

#[test]
fn bound_is_required_in_generic_body() {
    fails(
        "Area :: trait\n  area :: fn(self) -> f64\nend\nf :: fn<T>(x: T) -> f64 do\n  area(x)\nend\n",
        "type parameter `T` does not implement `Area`",
    );
}

#[test]
fn bound_is_checked_at_call_site() {
    fails(
        "Area :: trait\n  area :: fn(self) -> f64\nend\nf :: fn<T: Area>(x: T) -> f64 do\n  area(x)\nend\ng :: fn() -> f64 do\n  f(1)\nend\n",
        "the trait `Area` is not implemented for `i32`",
    );
}

#[test]
fn operator_overloading_via_traits() {
    ok("\
V :: type { x: f64, y: f64 }
impl Add for V do
  add :: fn(a: V, b: V) -> V do
    V{x: a.x + b.x, y: a.y + b.y}
  end
end
f :: fn(a: V, b: V) -> V do
  a + b
end
");
    fails(
        "V :: type { x: f64 }\nf :: fn(a: V, b: V) -> V do\n  a - b\nend\n",
        "operator `-` cannot be applied to type `V` (requires `Sub`)",
    );
}

#[test]
fn bounded_generic_operators() {
    ok("f :: fn<T: Add>(a: T, b: T) -> T do\n  a + b\nend\n");
    ok("f :: fn<T: Ord>(a: T, b: T) -> bool do\n  a < b\nend\n");
}

#[test]
fn impl_must_match_trait() {
    fails(
        "T :: trait\n  f :: fn(self) -> i32\nend\nS :: type\n  | A\nend\nimpl T for S do\n  f :: fn(s: S) -> String do\n    \"x\"\n  end\nend\n",
        "method `f` has type",
    );
    fails(
        "T :: trait\n  f :: fn(self) -> i32\n  g :: fn(self) -> i32\nend\nS :: type\n  | A\nend\nimpl T for S do\n  f :: fn(s: S) -> i32 do\n    1\n  end\nend\n",
        "missing method(s) `g`",
    );
    fails(
        "T :: trait\n  f :: fn(self) -> i32\nend\nS :: type\n  | A\nend\nimpl T for S do\n  h :: fn(s: S) -> i32 do\n    1\n  end\n  f :: fn(s: S) -> i32 do\n    1\n  end\nend\n",
        "trait `T` has no method `h`",
    );
}

#[test]
fn impl_body_is_checked() {
    fails(
        "T :: trait\n  f :: fn(self) -> i32\nend\nS :: type\n  | A\nend\nimpl T for S do\n  f :: fn(s: S) -> i32 do\n    \"no\"\n  end\nend\n",
        "should return `i32`",
    );
}

#[test]
fn conflicting_and_builtin_impls() {
    fails(
        "T :: trait\n  f :: fn(self) -> i32\nend\nS :: type\n  | A\nend\nimpl T for S do\n  f :: fn(s: S) -> i32 do\n    1\n  end\nend\nimpl T for S do\n  f :: fn(s: S) -> i32 do\n    2\n  end\nend\n",
        "conflicting implementations",
    );
    fails(
        "impl Show for i32 do\n  show :: fn(a: i32) -> String do\n    \"x\"\n  end\nend\n",
        "conflicting implementations",
    );
}

#[test]
fn trait_errors() {
    fails("impl Nope for i32 do\nend\n", "unknown trait `Nope`");
    fails("f :: fn<T: Nope>(x: T) do\nend\n", "unknown trait `Nope`");
    fails(
        "T :: trait\n  f :: fn(x: i32) -> i32\nend\n",
        "must take `self` as its first parameter",
    );
}

#[test]
fn generic_impl_with_bounds() {
    ok("\
Box<T> :: type
  | Full(T)
  | Empty
end
impl<T: Show> Show for Box<T> do
  show :: fn(b: Box<T>) -> String do
    match b do
      Full(x) -> show(x)
      Empty -> \"empty\"
    end
  end
end
main :: fn() -> String do
  show(Full(1))
end
");
}

// ---- var, `:=`, loops --------------------------------------------------------

#[test]
fn vars_loops_and_assignment_check() {
    ok("main :: fn() do\n  var n = 0\n  while n < 3 do\n    n := n + 1\n  end\n  for x in [1, 2] do\n    n := n + x\n    if n > 100 do\n      break\n    end\n    continue\n  end\nend\n");
}

#[test]
fn assignment_rules() {
    fails("main :: fn() do\n  x = 1\n  x := 2\nend\n", "`x` it is not a `var`".replace("`x` it", "`x`: it").as_str());
    fails("main :: fn() do\n  y := 2\nend\n", "cannot assign to `y`: it is not defined");
    fails("main :: fn() do\n  var x = 1\n  x := \"s\"\nend\n", "mismatch");
}

#[test]
fn loop_typing_and_control_flow_rules() {
    fails("main :: fn() do\n  while 1 do\n  end\nend\n", "mismatch");
    fails("main :: fn() do\n  for x in 5 do\n  end\nend\n", "mismatch");
    fails("main :: fn() do\n  break\nend\n", "`break` outside of a loop");
    fails("main :: fn() do\n  continue\nend\n", "`continue` outside of a loop");
    // A lambda is a new function: the loop around it does not count.
    fails("main :: fn() do\n  while true do\n    f = fn() do\n      break\n    end\n  end\nend\n", "`break` outside of a loop");
    // Cleanup boundaries cannot be jumped out of.
    fails("main :: fn() do\n  while true do\n    region do\n      break\n    end\n  end\nend\n", "cannot leave a `region`");
    fails("main :: fn() do\n  while true do\n    defer 1\n    break\n  end\nend\n", "cannot leave a `region`");
    // A loop inside a region may break itself.
    ok("main :: fn() do\n  region do\n    while true do\n      break\n    end\n  end\nend\n");
}

#[test]
fn closures_cannot_capture_vars() {
    fails("main :: fn() do\n  var n = 0\n  f = fn() do\n    n\n  end\nend\n", "`n` is a `var` and cannot be used inside a function, closure or `defer`");
    fails("main :: fn() do\n  var n = 0\n  f = fn() do\n    n := 1\n  end\nend\n", "cannot be used inside a function");
    // Copying into an immutable binding is the way out.
    ok("main :: fn() do\n  var n = 0\n  m = n\n  f = fn() do\n    m\n  end\nend\n");
}

#[test]
fn raw_memory_and_syscalls_need_unsafe() {
    let head = "import std.mem\nimport std.sys\n";
    fails(&format!("{head}main :: fn() do\n  mem.read_u8(0)\nend\n"), "`mem.read_u8` is unsafe: call it inside an `unsafe do ... end` block");
    fails(&format!("{head}main :: fn() do\n  sys.syscall0(39)\nend\n"), "`sys.syscall0` is unsafe");
    ok(&format!("{head}main :: fn() do\n  unsafe do\n    mem.read_u8(0)\n    sys.syscall0(39)\n  end\nend\n"));
    // The block is a value, and it can be nested in other expressions.
    ok(&format!("{head}f :: fn() -> i64 do\n  x = unsafe do sys.syscall0(39) end\n  x + 1\nend\n"));
    // A closure made inside `unsafe` may run anywhere, so it is not inside it.
    fails(&format!("{head}main :: fn() do\n  unsafe do\n    f = fn() do\n      mem.read_u8(0)\n    end\n  end\nend\n"), "is unsafe");
    // An alias does not hide the module.
    fails("import std.mem as m\nmain :: fn() do\n  m.read_u8(0)\nend\n", "is unsafe");
    // `unsafe` is an ordinary name elsewhere.
    ok("main :: fn() do\n  unsafe = 3\n  unsafe + 1\nend\n");
}

#[test]
fn unsafe_fn_and_repr_accessors_need_unsafe() {
    let decl = "peek :: unsafe fn(a: i64) -> i64 do a end\n";
    fails(&format!("{decl}main :: fn() do\n  peek(1)\nend\n"), "is `unsafe`");
    ok(&format!("{decl}main :: fn() do\n  unsafe do peek(1) end\nend\n"));
    ok(&format!("{decl}other :: unsafe fn() -> i64 do\n  peek(2)\nend\n"));
    let repr = "H :: repr type { a: u8, b: u32 }\n";
    fails(&format!("{repr}main :: fn() do\n  h_read(0)\nend\n"), "is `unsafe`");
    ok(&format!("{repr}main :: fn() do\n  unsafe do\n    h_write(0, H{{a: 1, b: 2}})\n    h_read(0)\n    h_size()\n  end\nend\n"));
}

#[test]
fn bit_operators_need_integers() {
    ok("main :: fn() -> i64 do\n  (1 & 3) ^ (1 << 2) + ~1 + (8 >> 1)\nend\n");
    fails("main :: fn() -> f64 do\n  1.5 & 2.5\nend\n", "Bitwise");
    fails("main :: fn() -> i64 do\n  1 << 2.0\nend\n", "mismatch");
    ok("Wrap<T> :: type { v: Option<Option<T>> }\nmain :: fn() do\n  ()\nend\n");
}
