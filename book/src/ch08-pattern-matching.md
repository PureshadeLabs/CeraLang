# Pattern matching

> **Status:** syntax and exhaustiveness checking implemented. Redundant
> (unreachable) arms are not reported yet.

Patterns appear in `match` arms. A pattern describes the *shape* of a value and
names the pieces you care about. This chapter lists every pattern form.

## The forms

| Pattern               | Matches                                              |
| --------------------- | ---------------------------------------------------- |
| `_`                   | anything, binds nothing                              |
| `name`                | anything, binds it to `name` (lowercase)             |
| `42`, `-1`, `2.5`     | that number                                          |
| `"text"`              | that string                                          |
| `true`, `false`       | that boolean                                         |
| `Nil`                 | a variant with no data                               |
| `Circle(r)`           | a variant, matching its fields with sub-patterns     |
| `Point{x, y: 0.0}`    | a record, with optional sub-patterns per field       |
| `(a, b)`              | a tuple                                              |
| `[]`                  | the empty list                                       |
| `[a, b]`              | a list of exactly two elements                       |
| `[head \| tail]`      | a non-empty list: first element and the rest         |
| `[a, b \| rest]`      | a list with at least two elements                    |

Names that start with an uppercase letter are always variant (constructor)
names; names that start with a lowercase letter always bind a new variable.

## Nesting

Patterns nest as deeply as the data does:

```cera
Option<T> :: type
  | None
  | Some(T)
end

describe :: fn(pair: (Option<i32>, Option<i32>)) -> String do
  match pair do
    (Some(0), _) -> "first is zero"
    (Some(a), Some(b)) -> "both present"
    (None, None) -> "neither"
    (_, _) -> "one is missing"
  end
end
```

## Literals

```cera
fizz :: fn(n: i32) -> String do
  match (n % 3, n % 5) do
    (0, 0) -> "FizzBuzz"
    (0, _) -> "Fizz"
    (_, 0) -> "Buzz"
    _ -> "number"
  end
end

greeting :: fn(lang: String) -> String do
  match lang do
    "en" -> "Hello"
    "es" -> "Hola"
    "fr" -> "Bonjour"
    _ -> "Hi"
  end
end
```

## Lists

```cera
describe_list :: fn(xs: [i32]) -> String do
  match xs do
    [] -> "empty"
    [x] -> "one element"
    [x, y] -> "two elements"
    [x, y | rest] -> "more than two"
  end
end

last :: fn<T>(xs: [T]) -> Option<T> do
  match xs do
    [] -> None
    [x] -> Some(x)
    [_ | rest] -> last(rest)
  end
end
```

## Records

```cera
Point :: type { x: f64, y: f64 }

quadrant :: fn(p: Point) -> String do
  match p do
    Point{x: 0.0, y: 0.0} -> "origin"
    Point{x, y} if x > 0.0 and y > 0.0 -> "first"
    Point{x, y} if x < 0.0 and y > 0.0 -> "second"
    _ -> "elsewhere"
  end
end
```

## Guards

A guard is an extra `bool` condition after the pattern. The arm only applies if
the pattern matches **and** the guard is true. Guards can use the names the
pattern bound:

```cera
bucket :: fn(n: i32) -> String do
  match n do
    x if x < 10 -> "small"
    x if x < 100 -> "medium"
    _ -> "large"
  end
end
```

The compiler cannot prove that a set of guards covers every case, so an arm
with a guard does not count toward exhaustiveness. Finish with an unguarded
arm, as `_` does above.

## Order matters

Arms are tried top to bottom. A more specific arm placed after a general one
can never run; the compiler warns about such unreachable arms:

```cera,compile_fail
f :: fn(n: i32) -> String do
  match n do
    _ -> "anything"
    0 -> "zero"
  end
end
```

## Exhaustiveness

The set of patterns must cover every value the type can hold. For variant types
that means every variant; for integers it means a final catch-all. For nested
patterns the compiler considers the whole product of possibilities. The error
names a value you missed, which is usually enough to find the hole.

## Recap

- Patterns describe shape and bind names; forms are in the table above.
- Uppercase names are variants, lowercase names are variables.
- Guards add conditions but do not count toward exhaustiveness.
- Arms run top to bottom; the first match wins.

## Exercises

`patterns1`, `patterns2`.
