# Generics and traits

> **Status:** implemented in the parser and type checker: generic types and
> functions, trait declarations, `impl` blocks and bounds. Trait method calls
> are checked statically, and trait methods are dispatched at run time under
> `cerac run`.

## Generic types and functions

You have already seen generics. A type parameter is a placeholder that is
filled in by whoever uses the definition:

```cera
Pair<A, B> :: type { first: A, second: B }

swap :: fn<A, B>(p: Pair<A, B>) -> Pair<B, A> do
  Pair{first: p.second, second: p.first}
end

map :: fn<A, B>(xs: [A], f: fn(A) -> B) -> [B] do
  match xs do
    [] -> []
    [x | rest] -> [f(x) | map(rest, f)]
  end
end
```

The last arm builds a list with `[head | tail]`, which as an expression puts one
or more elements in front of an existing list, mirroring the pattern.

Type inference works out the parameters at each call site, so you rarely
write them out by hand.

## Why traits

Generic code often needs to *do* something with a value of unknown type:
compare it, hash it, print it, add it. In `fn<T>(a: T, b: T) -> T do a + b end`
the compiler must know that `T` supports `+`.

A **trait** names a set of operations a type can support. Cera has no
inheritance; traits are how types share behavior.

## Declaring and implementing traits

A trait is a list of method signatures. Every method takes `self` as its first
parameter (written without a type: it means "the implementing type"):

```cera
Show :: trait
  show :: fn(self) -> String
end

Shape :: type
  | Circle(r: f64)
  | Rect(w: f64, h: f64)
end

impl Show for Shape do
  show :: fn(s: Shape) -> String do
    match s do
      Circle(r) -> "circle"
      Rect(w, h) -> "rectangle"
    end
  end
end
```

An `impl` gives each method a body with ordinary, fully annotated parameters.
The compiler checks that the impl has exactly the trait's methods, each with
the type the trait asks for, once `Self` is replaced by the implementing type.

There is no method-call syntax. A trait method is an ordinary function, so you
call it as `show(shape)` or `shape |> show`.

## Bounds

A **bound** says which traits a type parameter must implement:

```cera
Ord :: trait
  compare :: fn(self, other: Self) -> Ordering
end

largest :: fn<T: Ord>(xs: [T]) -> Option<T> do
  match xs do
    [] -> None
    [x | rest] ->
      match largest(rest) do
        None -> Some(x)
        Some(m) -> if x > m do Some(x) else Some(m) end
      end
  end
end
```

(The example redeclares `Ord` only to be self-contained; the real one is in the
prelude.) Several bounds are joined with `+`: `fn<T: Show + Ord>`. Inside the
body, `T` supports exactly the operations its bounds give it. Calling a bounded
function with a type that has no impl is a compile error:
`the trait `Ord` is not implemented for `Shape``.

Impls can be generic and carry their own bounds:

```cera
Box<T> :: type
  | Full(T)
  | Empty
end

impl<T: Show> Show for Box<T> do
  show :: fn(b: Box<T>) -> String do
    match b do
      Full(x) -> show(x)
      Empty -> "empty"
    end
  end
end
```

## Current rules

- Methods must take `self` first, so the implementation can be chosen from the
  first argument. Methods like `default() -> Self` need associated functions,
  which are not designed yet.
- Methods cannot have their own type parameters yet.
- At most one impl of a trait per type constructor (`Box<i32>` and `Box<String>`
  conflict). There are no blanket impls (`impl<T> Show for T`).
- There are no default method bodies, associated types, or supertraits.
- `==` is structural for every type; there is no `Eq` trait yet.

## The core traits

The prelude defines the traits the language itself relies on, with
implementations for the built-in types:

| Trait      | Method                          | Used by                          |
| ---------- | ------------------------------- | -------------------------------- |
| `Show`     | `show(self) -> String`          | `io.println`, `io.print`         |
| `Ord`      | `compare(self, other) -> Ordering` | `<` `<=` `>` `>=`             |
| `Add`      | `add(self, other) -> Self`      | `+`                              |
| `Sub`      | `sub(self, other) -> Self`      | binary `-`                       |
| `Mul`, `Div`, `Rem` | `mul`, `div`, `rem`    | `*` `/` `%`                      |
| `Neg`      | `neg(self) -> Self`             | unary `-`                        |

All numeric types implement the arithmetic traits and `Ord`; `char`, `String`
and `bool` implement `Ord`; everything built in implements `Show`, as do lists,
tuples, `Option` and `Result` of showable types. Your own types opt in with an
`impl`, which is also how you overload an operator:

```cera
Vec2 :: type { x: f64, y: f64 }

impl Add for Vec2 do
  add :: fn(a: Vec2, b: Vec2) -> Vec2 do
    Vec2{x: a.x + b.x, y: a.y + b.y}
  end
end
```

Planned but not yet present: `Hash`, `Copy`, and an `Eq` trait.

## How traits are compiled

The plan is **monomorphization**: each use of a generic function with a
particular type gets its own specialized machine code, as in Rust and C++. There
is no runtime dispatch unless you ask for it explicitly, which keeps code fast
and suitable for kernels. (The interpreter does not monomorphize: it picks the
impl from the run-time type of the first argument, which gives the same results.)

## Recap

- Generic types and functions work today: `Pair<A, B>`, `fn<A, B>(...)`.
- Traits are how generic code asks "what can this type do?"
- `Name :: trait`, `impl Trait for Type`, and `fn<T: Bound>` are implemented.

## Exercises

`traits1`, `traits2`.
