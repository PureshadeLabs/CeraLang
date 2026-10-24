# Functions

> **Status:** syntax, type checking and evaluation implemented.

## Declaring functions

```cera
add :: fn(a: i32, b: i32) -> i32 do
  a + b
end
```

- Parameters are written `name: Type`. Types on parameters are required.
- `-> Type` is the result type. It may be left out when the function returns
  unit.
- The body is a block. **The value of the last expression is the result.**
  There is no `return` statement.

A function that does something and returns nothing useful:

```cera
import std.io

greet :: fn(name: String) do
  io.println("Hello!")
end
```

## Calling functions

```cera
add :: fn(a: i32, b: i32) -> i32 do
  a + b
end

main :: fn() do
  three = add(1, 2)
  six = add(three, add(1, 2))
end
```

A call needs its `(` on the same line as the function name.

## Everything is an expression

`if`, `match`, `region` and plain `do ... end` blocks all produce values, so
they can sit on the right of a binding or be a function's last expression:

```cera
abs :: fn(n: i32) -> i32 do
  if n < 0 do
    -n
  else
    n
  end
end

describe :: fn(n: i32) -> String do
  kind = if n < 0 do "negative" else "non-negative" end
  kind
end
```

## Recursion

With immutable values, repetition is expressed by a function calling itself:

```cera
factorial :: fn(n: i64) -> i64 do
  if n <= 1 do
    1
  else
    n * factorial(n - 1)
  end
end

sum_to :: fn(n: i64, acc: i64) -> i64 do
  if n == 0 do
    acc
  else
    sum_to(n - 1, acc + n)
  end
end
```

`sum_to` passes its running total along. That *accumulator* style is the
functional replacement for a loop with a mutable counter.

> **Open question:** Cera v0 has no `for` or `while`. Whether the compiler
> guarantees that calls in tail position reuse the stack frame (as Elixir and
> Scheme do) is undecided; see [Appendix D](appendix-d-design.md).

## Functions are values

A function can be passed around like any other value. Its type is written
`fn(A, B) -> C`:

```cera
apply_twice :: fn(f: fn(i32) -> i32, x: i32) -> i32 do
  f(f(x))
end

inc :: fn(n: i32) -> i32 do
  n + 1
end

main :: fn() do
  five = apply_twice(inc, 3)
end
```

## Lambdas

For short, unnamed functions, use `fn(params) -> expression end`:

```cera
main :: fn() do
  double = fn(x) -> x * 2 end
  ten = double(5)
  evens = [1, 2, 3, 4] |> filter(fn(n) -> n % 2 == 0 end)
end
```

Lambda parameters may leave their types out, since they are inferred from
where the lambda is used. When the body needs several statements, use `do`
instead of `->`:

```cera
main :: fn() do
  report = fn(n: i32) do
    doubled = n * 2
    doubled + 1
  end
end
```

Lambdas capture the bindings around them. In Cera a capture is just a
reference to a value in some region, which means a lambda can't outlive the
region its captured values live in. The compiler checks this for you
([chapter 12](ch12-regions.md)).

## Generic functions

Put type parameters after `fn`:

```cera
identity :: fn<T>(x: T) -> T do
  x
end

pair :: fn<A, B>(a: A, b: B) -> (A, B) do
  (a, b)
end
```

One definition works for every type. [Chapter 16](ch16-generics.md) covers
generics together with traits.

## Recap

- `name :: fn(p: T) -> R do ... end` declares a function.
- The last expression is the result; there is no `return`.
- Functions are values; lambdas are `fn(x) -> expr end`.
- `fn<T>(...)` makes a function generic.

## Exercises

`functions1`, `functions2`.
