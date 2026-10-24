# Control flow

> **Status:** syntax implemented; semantics designed.

Cera has two ways to choose between alternatives: `if` for yes/no questions and
`match` for taking data apart. Both are expressions that produce a value.

## `if`

```cera
sign :: fn(n: i32) -> i32 do
  if n < 0 do
    -1
  else if n == 0 do
    0
  else
    1
  end
end
```

- The condition must be a `bool`. There is no "truthiness".
- `else if` chains share a single `end`.
- Both branches must have the same type, because the whole `if` is one value.
- Without an `else`, the `if` has type `()`, so its body can only be run for its
  effects:

```cera
import std.io

warn :: fn(n: i32) do
  if n > 100 do
    io.println("big number")
  end
end
```

Short conditionals fit on one line:

```cera
max :: fn(a: i32, b: i32) -> i32 do
  if a > b do a else b end
end
```

## `match`

`match` compares a value against a list of patterns, top to bottom, and runs
the first arm that fits:

```cera
describe :: fn(n: i32) -> String do
  match n do
    0 -> "zero"
    1 -> "one"
    _ -> "many"
  end
end
```

Each arm is `pattern -> expression`. `_` matches anything.

### Guards

Add `if condition` after a pattern to refine it:

```cera
classify :: fn(n: i32) -> String do
  match n do
    0 -> "zero"
    x if x < 0 -> "negative"
    x if x % 2 == 0 -> "positive even"
    _ -> "positive odd"
  end
end
```

Here `x` is a pattern that matches anything and names it, so the guard can use
it.

### Exhaustiveness

A `match` has to cover every possible value. If you forget a case, the compiler
tells you which one:

```cera,compile_fail
Light :: type
  | Red
  | Yellow
  | Green
end

next :: fn(l: Light) -> Light do
  match l do
    Red -> Green
    Green -> Yellow
  end
end
```

```text
error: non-exhaustive match: `Yellow` is not covered
```

(That message is the planned wording; the checker is not written yet.) This is
one of the most useful properties of algebraic data types: when you add a
variant later, the compiler points at every `match` that needs updating.

More on patterns in [chapter 8](ch08-pattern-matching.md).

## Repetition

There are three ways to repeat work: recursion
([chapter 4](ch04-functions.md)), higher-order library functions, and loops.

```cera
main :: fn() do
  squares = [1, 2, 3, 4] |> map(fn(n) -> n * n end)
  total = squares |> sum
  evens = squares |> filter(fn(n) -> n % 2 == 0 end)
end
```

### Loops and `var`

Bindings are immutable. To change a value from one iteration to the next,
declare it with `var` and assign with `:=`:

```cera
sum_to :: fn(n: i64) -> i64 do
  var total = 0
  var i = 1
  while i <= n do
    total := total + i
    i := i + 1
  end
  total
end

first_big :: fn(xs: [i64]) -> i64 do
  var found = 0
  for x in xs do
    if x > 100 do
      found := x
      break
    end
  end
  found
end
```

- `while cond do ... end` repeats while `cond` is true. `for x in list do ... end`
  runs the body once per element (`x` is immutable). Both have type `()`.
- `break` leaves the nearest loop; `continue` skips to its next iteration.
- `x := value` needs `x` to be a `var` in scope and `value` to have its type.
  `x = value` is still a new binding that shadows `x`.
- Loop over a range with `for i in list.range(0, 10) do`.

Two rules keep mutation simple and predictable:

- A `var` cannot be used inside a lambda, a nested function or a `defer`. Copy it
  into an immutable binding first (`now = total`) and use that.
- `break` and `continue` cannot leave a `region`, a `with` or a block that has a
  `defer`; end that block first.

## Early exit with `?`

There is no `return`. The one early exit is the postfix `?` operator, which
unwraps a success value or leaves the function with the failure.
[Chapter 11](ch11-errors.md) covers it.

## Recap

- `if cond do a else b end` is an expression; `else if` shares one `end`.
- `match value do pattern -> expr ... end` must be exhaustive.
- Guards refine patterns: `x if x < 0 -> ...`.
- Repetition is recursion, library functions such as `map`, `filter`, `sum`, or `while` / `for` loops with `var`, `:=`, `break` and `continue`.

## Exercises

`control1`, `control2`.
