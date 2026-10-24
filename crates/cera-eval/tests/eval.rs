use cera_eval::{run_captured, RunError};

fn run(src: &str) -> String {
    match run_captured(src) {
        Ok(s) => s,
        Err(RunError::Syntax(d)) | Err(RunError::Type(d)) => panic!("compile errors: {d:?}\n{src}"),
        Err(RunError::Runtime(e)) => panic!("runtime error: {}\n{src}", e.message),
    }
}

fn runtime_error(src: &str) -> String {
    match run_captured(src) {
        Err(RunError::Runtime(e)) => e.message,
        Ok(o) => panic!("expected a runtime error, got output {o:?}"),
        Err(other) => panic!("expected a runtime error, got {other:?}"),
    }
}

fn main_body(body: &str) -> String {
    format!("import std.io\nmain :: fn() do\n{body}\nend\n")
}

#[test]
fn hello_world() {
    assert_eq!(
        run("import std.io\nmain :: fn() do\n  \"Hello, world!\" |> io.println\nend\n"),
        "Hello, world!\n"
    );
}

#[test]
fn arithmetic_and_types() {
    assert_eq!(run(&main_body("  io.println(1 + 2 * 3)\n  io.println(7 / 2)\n  io.println(-7 % 3)\n  io.println(1.5 * 2.0)\n  io.println(10.0 / 4.0)")), "7\n3\n-1\n3.0\n2.5\n");
}

#[test]
fn integer_overflow_is_an_error() {
    let m = runtime_error(&format!(
        "import std.io\nf :: fn(x: u8) -> u8 do\n  x + 200\nend\nmain :: fn() do\n  io.println(f(100))\nend\n"
    ));
    assert!(m.contains("overflow"), "{m}");
}

#[test]
fn division_by_zero() {
    let m = runtime_error(&main_body("  io.println(1 / 0)"));
    assert!(m.contains("division by zero"), "{m}");
}

#[test]
fn floats_divide_by_zero_without_error() {
    assert_eq!(run(&main_body("  io.println(1.0 / 0.0)")), "inf\n");
}

#[test]
fn booleans_and_comparison() {
    assert_eq!(
        run(&main_body("  io.println(1 < 2 and 2 < 3)\n  io.println(not (1 == 1))\n  io.println(\"a\" < \"b\")\n  io.println(2 >= 3 or true)")),
        "true\nfalse\ntrue\ntrue\n"
    );
}

#[test]
fn if_expression() {
    assert_eq!(
        run(&main_body("  x = if 3 > 2 do \"yes\" else \"no\" end\n  io.println(x)")),
        "yes\n"
    );
}

#[test]
fn locals_shadow() {
    assert_eq!(run(&main_body("  x = 1\n  x = x + 1\n  io.println(x)")), "2\n");
}

#[test]
fn recursion() {
    let src = "import std.io\nfact :: fn(n: i64) -> i64 do\n  if n <= 1 do 1 else n * fact(n - 1) end\nend\nmain :: fn() do\n  io.println(fact(20))\nend\n";
    assert_eq!(run(src), "2432902008176640000\n");
}

#[test]
fn deep_tail_recursion_does_not_overflow() {
    let src = "\
import std.io
count :: fn(n: i64, acc: i64) -> i64 do
  if n == 0 do acc else count(n - 1, acc + 1) end
end
main :: fn() do
  io.println(count(200000, 0))
end
";
    assert_eq!(run(src), "200000\n");
}

#[test]
fn deep_non_tail_recursion_is_reported() {
    let src = "\
f :: fn(n: i64) -> i64 do
  if n == 0 do 0 else 1 + f(n - 1) end
end
main :: fn() do
  f(10000000)
end
";
    let m = runtime_error(src);
    assert!(m.contains("stack overflow"), "{m}");
}

#[test]
fn moderate_non_tail_recursion_works() {
    let src = "\
import std.io
f :: fn(n: i64) -> i64 do
  if n == 0 do 0 else 1 + f(n - 1) end
end
main :: fn() do
  io.println(f(3000))
end
";
    assert_eq!(run(src), "3000\n");
}

#[test]
fn adts_and_match() {
    let src = "\
import std.io
Shape :: type
  | Circle(r: f64)
  | Rect(w: f64, h: f64)
end
area :: fn(s: Shape) -> f64 do
  match s do
    Circle(r)  -> 3.0 * r * r
    Rect(w, h) -> w * h
  end
end
main :: fn() do
  io.println(area(Circle(2.0)))
  io.println(area(Rect(3.0, 4.0)))
end
";
    assert_eq!(run(src), "12.0\n12.0\n");
}

#[test]
fn examples_run() {
    let dir = format!("{}/../../examples", env!("CARGO_MANIFEST_DIR"));
    let hello = std::fs::read_to_string(format!("{dir}/hello.cera")).unwrap();
    assert_eq!(run(&hello), "Hello, world!\n");
    let shapes = std::fs::read_to_string(format!("{dir}/shapes.cera")).unwrap();
    assert_eq!(run(&shapes), "24.56636\n");
    let traits = std::fs::read_to_string(format!("{dir}/traits.cera")).unwrap();
    assert_eq!(run(&traits), "[circle, rectangle]\n9.14159\n");
}

#[test]
fn lists_patterns_and_cons() {
    let src = "\
import std.io
len :: fn(xs: [i32]) -> i32 do
  match xs do
    [] -> 0
    [_ | rest] -> 1 + len(rest)
  end
end
main :: fn() do
  xs = [1, 2, 3]
  io.println(len(xs))
  io.println([0 | xs])
  io.println(xs |> map(fn(x) -> x * 10 end))
  io.println(xs |> filter(fn(x) -> x > 1 end) |> sum)
  io.println(fold(xs, 100, fn(acc, x) -> acc - x end))
  io.println(reverse(xs))
  io.println(append(xs, [4]))
  io.println(head(xs))
  io.println(tail([1]))
  io.println(head([]: [i32]))
end
";
    // `[]: [i32]` is not Cera syntax; use a typed empty list through a helper instead.
    let src = src.replace("  io.println(head([]: [i32]))\n", "");
    assert_eq!(
        run(&src),
        "3\n[0, 1, 2, 3]\n[10, 20, 30]\n5\n94\n[3, 2, 1]\n[1, 2, 3, 4]\nSome(1)\nSome([])\n"
    );
}

#[test]
fn list_patterns_with_fixed_prefix() {
    let src = "\
import std.io
f :: fn(xs: [i32]) -> String do
  match xs do
    [] -> \"empty\"
    [a] -> \"one\"
    [a, b] -> \"two\"
    [a, b | rest] -> \"many\"
  end
end
main :: fn() do
  io.println(f([]))
  io.println(f([1]))
  io.println(f([1, 2]))
  io.println(f([1, 2, 3, 4]))
end
";
    assert_eq!(run(src), "empty\none\ntwo\nmany\n");
}

#[test]
fn index_and_out_of_range() {
    assert_eq!(run(&main_body("  xs = [5, 6, 7]\n  io.println(xs[1])")), "6\n");
    let m = runtime_error(&main_body("  xs = [5, 6, 7]\n  io.println(xs[3])"));
    assert!(m.contains("out of range"), "{m}");
}

#[test]
fn tuples_and_records() {
    let src = "\
import std.io
Point :: type { x: f64, y: f64 }
norm1 :: fn(p: Point) -> f64 do
  match p do
    Point{x, y} -> x + y
  end
end
main :: fn() do
  p = Point{y: 2.0, x: 1.0}
  io.println(p.y)
  io.println(p.x)
  io.println(norm1(p))
  io.println((1, \"a\", true))
  match (1, 2) do
    (a, b) -> io.println(a + b)
  end
end
";
    assert_eq!(run(src), "2.0\n1.0\n3.0\n(1, \"a\", true)\n3\n");
}

#[test]
fn guards() {
    let src = "\
import std.io
sign :: fn(n: i32) -> String do
  match n do
    x if x < 0 -> \"neg\"
    0 -> \"zero\"
    _ -> \"pos\"
  end
end
main :: fn() do
  io.println(sign(-5))
  io.println(sign(0))
  io.println(sign(9))
end
";
    assert_eq!(run(src), "neg\nzero\npos\n");
}

#[test]
fn closures_capture() {
    let src = "\
import std.io
adder :: fn(n: i32) -> fn(i32) -> i32 do
  fn(x) -> x + n end
end
main :: fn() do
  add5 = adder(5)
  io.println(add5(10))
  io.println([1, 2] |> map(add5))
end
";
    assert_eq!(run(src), "15\n[6, 7]\n");
}

#[test]
fn pipes_with_extra_arguments() {
    let src = "\
import std.io
add :: fn(a: i32, b: i32) -> i32 do
  a + b
end
double :: fn(x: i32) -> i32 do
  x * 2
end
main :: fn() do
  1 |> double |> add(10) |> io.println
end
";
    assert_eq!(run(src), "12\n");
}

#[test]
fn option_and_try() {
    let src = "\
import std.io
half :: fn(n: i32) -> Option<i32> do
  if n % 2 == 0 do Some(n / 2) else None end
end
quarter :: fn(n: i32) -> Option<i32> do
  h = half(n)?
  half(h)
end
main :: fn() do
  io.println(quarter(8))
  io.println(quarter(6))
  io.println(quarter(3))
end
";
    assert_eq!(run(src), "Some(2)\nNone\nNone\n");
}

#[test]
fn result_and_try() {
    let src = "\
import std.io
parse :: fn(s: String) -> Result<i32, String> do
  match s do
    \"1\" -> Ok(1)
    \"2\" -> Ok(2)
    _ -> Err(\"bad\")
  end
end
add :: fn(a: String, b: String) -> Result<i32, String> do
  x = parse(a)?
  y = parse(b)?
  Ok(x + y)
end
main :: fn() do
  io.println(add(\"1\", \"2\"))
  io.println(add(\"1\", \"x\"))
end
";
    assert_eq!(run(src), "Ok(3)\nErr(\"bad\")\n");
}

#[test]
fn try_inside_a_lambda_returns_from_the_lambda() {
    let src = "\
import std.io
main :: fn() do
  f = fn(x: Option<i32>) do
    v = x?
    Some(v + 1)
  end
  io.println(f(Some(1)))
  io.println(f(None))
end
";
    assert_eq!(run(src), "Some(2)\nNone\n");
}

#[test]
fn main_returning_err_is_an_error() {
    let src = "main :: fn() -> Result<(), String> do\n  Err(\"boom\")\nend\n";
    let m = runtime_error(src);
    assert!(m.contains("boom"), "{m}");
}

#[test]
fn defer_runs_in_reverse_order_at_block_end() {
    let src = "\
import std.io
main :: fn() do
  defer io.println(\"first deferred\")
  defer io.println(\"second deferred\")
  io.println(\"body\")
end
";
    assert_eq!(run(src), "body\nsecond deferred\nfirst deferred\n");
}

#[test]
fn defer_runs_on_early_return() {
    let src = "\
import std.io
f :: fn(x: Option<i32>) -> Option<i32> do
  defer io.println(\"cleanup\")
  v = x?
  Some(v)
end
main :: fn() do
  io.println(f(None))
end
";
    assert_eq!(run(src), "cleanup\nNone\n");
}

#[test]
fn regions_and_promote() {
    let src = "\
import std.io
main :: fn() do
  r = region do
    xs = [1, 2, 3]
    promote(sum(xs))
  end
  io.println(r)
end
";
    assert_eq!(run(src), "6\n");
}

#[test]
fn user_trait_dispatch() {
    let src = "\
import std.io
Dog :: type
  | Dog
end
Cat :: type
  | Cat
end
Speak :: trait
  speak :: fn(self) -> String
end
impl Speak for Dog do
  speak :: fn(d: Dog) -> String do
    \"woof\"
  end
end
impl Speak for Cat do
  speak :: fn(c: Cat) -> String do
    \"meow\"
  end
end
twice :: fn<T: Speak>(x: T) -> String do
  String.concat([speak(x), speak(x)])
end
main :: fn() do
  io.println(speak(Dog))
  io.println(speak(Cat))
  io.println(twice(Cat))
end
";
    assert_eq!(run(src), "woof\nmeow\nmeowmeow\n");
}

#[test]
fn operator_overloading() {
    let src = "\
import std.io
V :: type { x: i32, y: i32 }
impl Add for V do
  add :: fn(a: V, b: V) -> V do
    V{x: a.x + b.x, y: a.y + b.y}
  end
end
impl Show for V do
  show :: fn(v: V) -> String do
    String.concat([\"<\", show(v.x), \",\", show(v.y), \">\"])
  end
end
main :: fn() do
  io.println(V{x: 1, y: 2} + V{x: 10, y: 20})
  io.println([V{x: 1, y: 1}, V{x: 2, y: 2}] |> sum)
end
";
    assert_eq!(run(src), "<11,22>\n<3,3>\n");
}

#[test]
fn ord_overloading() {
    let src = "\
import std.io
Level :: type
  | Low
  | High
end
rank :: fn(l: Level) -> i32 do
  match l do
    Low -> 0
    High -> 1
  end
end
impl Ord for Level do
  compare :: fn(a: Level, b: Level) -> Ordering do
    compare(rank(a), rank(b))
  end
end
main :: fn() do
  io.println(Low < High)
  io.println(High <= Low)
  io.println(compare(High, Low))
end
";
    assert_eq!(run(src), "true\nfalse\nGreater\n");
}

#[test]
fn generic_impl_dispatch_on_nested_values() {
    let src = "\
import std.io
Box<T> :: type
  | Full(T)
  | Empty
end
impl<T: Show> Show for Box<T> do
  show :: fn(b: Box<T>) -> String do
    match b do
      Full(x) -> String.concat([\"Full of \", show(x)])
      Empty -> \"empty\"
    end
  end
end
main :: fn() do
  io.println(Full(3))
  io.println([Full(1), Empty])
end
";
    assert_eq!(run(src), "Full of 3\n[Full of 1, empty]\n");
}

#[test]
fn constants() {
    let src = "import std.io\nLIMIT :: 10\nDOUBLE :: LIMIT * 2\nmain :: fn() do\n  io.println(DOUBLE)\nend\n";
    assert_eq!(run(src), "20\n");
}

#[test]
fn string_functions() {
    assert_eq!(
        run(&main_body("  io.println(String.length(\"hello\"))\n  io.println(String.concat([\"a\", \"b\", \"c\"]))\n  io.println([\"x\", \"yz\"] |> map(String.length))")),
        "5\nabc\n[1, 2]\n"
    );
}

#[test]
fn integer_types_are_respected() {
    let m = runtime_error(&main_body("  x = 255\n  y = 1\n  io.println(x + y)\n  z = [200, 100] |> map(fn(a) -> a * 2 end)\n  io.println(z)").replace("main :: fn() do", "f :: fn(a: u8) -> u8 do\n  a\nend\nmain :: fn() do").replace("  x = 255", "  x = f(255)"));
    assert!(m.contains("overflow"), "{m}");
}

#[test]
fn missing_main() {
    let m = runtime_error("f :: fn() do\nend\n");
    assert!(m.contains("no `main`"), "{m}");
}

#[test]
fn type_errors_stop_the_run() {
    assert!(matches!(
        run_captured("main :: fn() -> i32 do\n  \"x\"\nend\n"),
        Err(RunError::Type(_))
    ));
    assert!(matches!(run_captured("main :: fn(\n"), Err(RunError::Syntax(_))));
}

#[test]
fn sum_of_empty_list_is_a_clear_error() {
    let src = "import std.io\nmain :: fn() do\n  xs = filter([1, 2], fn(x) -> x > 5 end)\n  io.println(sum(xs))\nend\n";
    let m = runtime_error(src);
    assert!(m.contains("empty list"), "{m}");
}

#[test]
fn equality_is_structural() {
    assert_eq!(
        run(&main_body("  io.println([1, 2] == [1, 2])\n  io.println(Some(1) == Some(2))\n  io.println((1, \"a\") != (1, \"a\"))")),
        "true\nfalse\nfalse\n"
    );
}
