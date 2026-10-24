# Algebraic data types

> **Status:** syntax, type checking and exhaustiveness checking implemented.

An **algebraic data type** (ADT) says "a value is exactly one of these
alternatives", and each alternative can carry data. It is the main way to model
a domain in Cera.

## Variant types

```cera
Shape :: type
  | Circle(r: f64)
  | Rect(w: f64, h: f64)
end
```

`Shape` is a type with two **variants**. `Circle` carries a radius; `Rect`
carries a width and a height. Variants are also the functions that build
values:

```cera
Shape :: type
  | Circle(r: f64)
  | Rect(w: f64, h: f64)
end

main :: fn() do
  c = Circle(2.0)
  r = Rect(3.0, 4.0)
  shapes = [c, r]
end
```

Fields may be named (`r: f64`) or positional (`Cons(T, List<T>)`). Named fields
are documentation; you still match and construct them by position.

A variant with no data needs no parentheses:

```cera
Light :: type
  | Red
  | Yellow
  | Green
end
```

This is how Cera spells an enum.

## Taking them apart

You use `match` to find out which variant you have and to get at its data:

```cera
Shape :: type
  | Circle(r: f64)
  | Rect(w: f64, h: f64)
end

area :: fn(s: Shape) -> f64 do
  match s do
    Circle(r)  -> 3.14159 * r * r
    Rect(w, h) -> w * h
  end
end
```

The compiler checks that you handled every variant. Add a `Triangle` later and
`area` stops compiling until you handle it.

## Generic types

Put type parameters after the name:

```cera
Maybe<T> :: type
  | Nothing
  | Just(T)
end

default :: fn<T>(m: Maybe<T>, fallback: T) -> T do
  match m do
    Just(x) -> x
    Nothing -> fallback
  end
end
```

(The standard library already provides `Option<T>` with `Some` and `None`; this
is for illustration.)

## Recursive types

A variant may refer to its own type. A singly linked list:

```cera
List<T> :: type
  | Nil
  | Cons(T, List<T>)
end

map :: fn<A, B>(xs: List<A>, f: fn(A) -> B) -> List<B> do
  match xs do
    Nil -> Nil
    Cons(h, t) -> Cons(f(h), map(t, f))
  end
end
```

A binary search tree:

```cera
Tree<T> :: type
  | Leaf
  | Node(left: Tree<T>, value: T, right: Tree<T>)
end

insert :: fn(t: Tree<i32>, x: i32) -> Tree<i32> do
  match t do
    Leaf -> Node(Leaf, x, Leaf)
    Node(l, v, r) ->
      if x < v do
        Node(insert(l, x), v, r)
      else if x > v do
        Node(l, v, insert(r, x))
      else
        t
      end
  end
end

contains :: fn(t: Tree<i32>, x: i32) -> bool do
  match t do
    Leaf -> false
    Node(l, v, r) ->
      if x == v do
        true
      else if x < v do
        contains(l, x)
      else
        contains(r, x)
      end
  end
end
```

Because values are immutable, `insert` builds a new tree that shares everything
it did not change with the old one. How that sharing interacts with regions is
the subject of [chapter 15](ch15-escaping.md).

## Records versus variants

| Use a **record** when...                | Use a **variant type** when...            |
| --------------------------------------- | ----------------------------------------- |
| a value has *all* of several fields     | a value is *one of* several alternatives  |
| `Point{x, y}`                           | `Circle(r)` or `Rect(w, h)`               |

They combine: a variant can hold a record, and a record can hold a variant.

## Recap

- `Name :: type | A | B(x: T) end` declares a variant type.
- `Name<T> :: type ...` makes it generic.
- Variants can be recursive.
- `match` takes values apart and checks that every variant is handled.

## Exercises

`adt1`, `adt2`.
