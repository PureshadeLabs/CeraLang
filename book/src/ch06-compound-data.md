# Tuples, lists and records

> **Status:** syntax implemented. Representation and library functions are
> designed; nothing runs yet.

Three built-in ways to group values: tuples, lists and records.

## Tuples

A tuple holds a fixed number of values of possibly different types:

```cera
main :: fn() do
  point = (3, 4)
  person = ("Ada", 36, true)
  unit = ()
end
```

Its type lists the element types: `(i32, i32)`, `(String, i32, bool)`. The
empty tuple `()` is the **unit** type, used when there is nothing meaningful to
return.

Take tuples apart with patterns, in `match`:

```cera
swap :: fn<A, B>(pair: (A, B)) -> (B, A) do
  match pair do
    (a, b) -> (b, a)
  end
end
```

## Lists

A list holds any number of values **of the same type**:

```cera
main :: fn() do
  numbers = [1, 2, 3]
  words = ["one", "two"]
  empty = []
  first = numbers[0]
end
```

The type of a list of `i32` is written `[i32]`.

Lists are the basic functional sequence: immutable and built from a head and a
tail. A list is either empty or an element followed by the rest. Patterns
reflect that with `[head | tail]`:

```cera
length :: fn<T>(xs: [T]) -> i32 do
  match xs do
    [] -> 0
    [_ | rest] -> 1 + length(rest)
  end
end

sum_list :: fn(xs: [i32]) -> i32 do
  match xs do
    [] -> 0
    [x | rest] -> x + sum_list(rest)
  end
end

first_two :: fn<T>(xs: [T]) -> String do
  match xs do
    [a, b | _] -> "at least two"
    [a] -> "exactly one"
    [] -> "none"
  end
end
```

The same `[head | tail]` form builds lists. It puts elements in front of an
existing list, so it is the mirror image of the pattern:

```cera
prepend :: fn<T>(x: T, xs: [T]) -> [T] do
  [x | xs]
end

with_zeros :: fn(xs: [i32]) -> [i32] do
  [0, 0 | xs]
end

double_all :: fn(xs: [i32]) -> [i32] do
  match xs do
    [] -> []
    [x | rest] -> [x * 2 | double_all(rest)]
  end
end
```

> **Provisional:** the exact representation of `[T]` (cons cells versus
> contiguous arrays) is a standard-library decision. The std plan has both: a
> persistent `list` and a growable `array`. The syntax above is for the
> sequence literal and its patterns either way.

Indexing with `xs[i]` is available; out-of-range access is a runtime error. For
total access use the library's `get`, which returns an `Option`
([chapter 11](ch11-errors.md)).

## Records

A record groups named fields. Declare one with a `type` declaration using braces:

```cera
Point :: type { x: f64, y: f64 }

Person :: type {
  name: String,
  age: i32,
}
```

Create a record by naming the type and giving every field:

```cera
Point :: type { x: f64, y: f64 }

origin :: fn() -> Point do
  Point{x: 0.0, y: 0.0}
end
```

Read a field with a dot:

```cera
Point :: type { x: f64, y: f64 }

length :: fn(p: Point) -> f64 do
  sqrt(p.x * p.x + p.y * p.y)
end
```

Take a record apart in a pattern. Naming a field alone binds it; `field: pattern`
matches deeper:

```cera
Point :: type { x: f64, y: f64 }

on_x_axis :: fn(p: Point) -> bool do
  match p do
    Point{y: 0.0} -> true
    Point{x, y} -> false
  end
end
```

Records are immutable. To change a field, write an update, which builds a copy:

```cera
Point :: type { x: f64, y: f64 }

right :: fn(p: Point) -> Point do
  {p | x: p.x + 1.0}
end
```

`{p | x: 3.0, y: 0.0}` names the fields to replace; the rest are copied. The
fields are checked against the type of `p`.

## Recap

- Tuples: `(a, b)`, fixed size, mixed types.
- Lists: `[a, b, c]`, same type, patterns `[]` and `[head | tail]`.
- Records: `Name :: type { field: T }`, constructed `Name{field: value}`,
  read with `.field`.

## Exercises

`data1`, `data2`.
