# Project: a tiny expression language

> **Status:** the syntax is real and parses today. Running it needs the
> interpreter, so for now this is a program to read and to check with
> `cerac check`.

Let's put the first half of the book to work: algebraic data types, pattern
matching and recursion. We build a tiny arithmetic language with variables, an
evaluator and a simplifier, the classic first program for any language with
ADTs.

## The data

An expression is a number, a variable, a sum, a product or a negation. That is
an algebraic data type:

```cera
Expr :: type
  | Num(i64)
  | Var(String)
  | Add(Expr, Expr)
  | Mul(Expr, Expr)
  | Neg(Expr)
end
```

The expression `2 * (x + 3)` is a tree:

```cera
Expr :: type
  | Num(i64)
  | Var(String)
  | Add(Expr, Expr)
  | Mul(Expr, Expr)
  | Neg(Expr)
end

example :: fn() -> Expr do
  Mul(Num(2), Add(Var("x"), Num(3)))
end
```

## An environment

To evaluate variables we need to look their values up. For this project a
simple association list will do:

```cera
Binding :: type { name: String, value: i64 }

lookup :: fn(env: [Binding], name: String) -> Option<i64> do
  match env do
    [] -> None
    [b | rest] -> if b.name == name do Some(b.value) else lookup(rest, name) end
  end
end
```

## The evaluator

Evaluating is a `match` over the shape of the expression. Variables can be
unbound, so the result is an `Option`:

```cera
eval :: fn(e: Expr, env: [Binding]) -> Option<i64> do
  match e do
    Num(n) -> Some(n)
    Var(name) -> lookup(env, name)
    Neg(inner) ->
      match eval(inner, env) do
        None -> None
        Some(v) -> Some(0 - v)
      end
    Add(a, b) ->
      match (eval(a, env), eval(b, env)) do
        (Some(x), Some(y)) -> Some(x + y)
        _ -> None
      end
    Mul(a, b) ->
      match (eval(a, env), eval(b, env)) do
        (Some(x), Some(y)) -> Some(x * y)
        _ -> None
      end
  end
end
```

Notice what the compiler does for you: if you add a new variant, say
`Sub(Expr, Expr)`, `eval` stops compiling until you handle it.

## The same thing with `?`

Using the early-exit operator from [chapter 11](ch11-errors.md), the evaluator
gets much shorter:

```cera
eval_short :: fn(e: Expr, env: [Binding]) -> Option<i64> do
  match e do
    Num(n) -> Some(n)
    Var(name) -> lookup(env, name)
    Neg(inner) -> Some(0 - eval_short(inner, env)?)
    Add(a, b) -> Some(eval_short(a, env)? + eval_short(b, env)?)
    Mul(a, b) -> Some(eval_short(a, env)? * eval_short(b, env)?)
  end
end
```

Each `?` either yields the number or makes `eval_short` return `None`
immediately.

## A simplifier

Rewriting trees is where pattern matching shines. Nested patterns spot special
cases directly:

```cera
simplify :: fn(e: Expr) -> Expr do
  match e do
    Add(Num(0), x) -> simplify(x)
    Add(x, Num(0)) -> simplify(x)
    Mul(Num(1), x) -> simplify(x)
    Mul(x, Num(1)) -> simplify(x)
    Mul(Num(0), _) -> Num(0)
    Mul(_, Num(0)) -> Num(0)
    Neg(Neg(x)) -> simplify(x)
    Add(a, b) -> Add(simplify(a), simplify(b))
    Mul(a, b) -> Mul(simplify(a), simplify(b))
    Neg(x) -> Neg(simplify(x))
    other -> other
  end
end
```

The order of the arms matters: the specific ones come before the general
`Add(a, b)` and `Mul(a, b)`. The final arm `other -> other` returns numbers and
variables unchanged.

## Printing

A function that turns an expression into text. Strings are built with the
library's `String.concat`, which joins a list of strings:

```cera
show_expr :: fn(e: Expr) -> String do
  match e do
    Num(n) -> show(n)
    Var(name) -> name
    Neg(x) -> String.concat(["-", show_expr(x)])
    Add(a, b) -> String.concat(["(", show_expr(a), " + ", show_expr(b), ")"])
    Mul(a, b) -> String.concat([show_expr(a), " * ", show_expr(b)])
  end
end
```

Every call builds a new string, and each one is allocated in the context
allocator: inside a `region`, that is the region.

## Putting it together

```cera
import std.io

main :: fn() do
  region do
    e = Mul(Num(2), Add(Var("x"), Num(0)))
    env = [Binding{name: "x", value: 5}]

    e |> simplify |> show_expr |> io.println
    eval(e, env) |> show |> io.println
  end
end
```

The whole program runs inside one `region`, so every tree node, string and
intermediate list is freed together at the `end`. There is nothing else to
manage.

## Ideas to extend it

- Add `Sub`, `Div` (with `Result` for division by zero) and watch the compiler
  list every `match` you must update.
- Add `Let(String, Expr, Expr)` for local bindings.
- Write a parser from text to `Expr`.
- Make `simplify` repeat until nothing changes.
