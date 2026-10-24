# Modules and imports

> **Status:** `import` syntax, module resolution across files and packages, and
> the package manifest are implemented by `kote` ([chapter 20](ch20-kote.md)).
> A top-level declaration is private to its module unless it is marked `pub`;
> circular imports are rejected.

## Imports

```cera
import std.io
import std.collections.map
import std.collections.map as m
```

An `import` names a module by a dotted path. After it, the module's contents
are reached through the module's last name, or through the alias after `as`:

```cera
import std.io
import std.collections.map as m

main :: fn() do
  io.println("hello")
  scores = m.new()
end
```

Imports go at the top of a file, one per line.

## Files are modules

Each `.cera` file is a module. Its name comes from its path inside the package:
`src/geometry/shapes.cera` is the module `geometry.shapes`. Inside a module,
every top-level declaration (`::`) can be referred to by other modules that
import it.

```cera
# geometry/shapes.cera
pub Shape :: type
  | Circle(r: f64)
  | Rect(w: f64, h: f64)
end

pub area :: fn(s: Shape) -> f64 do
  match s do
    Circle(r) -> 3.14159 * r * r
    Rect(w, h) -> w * h
  end
end
```

```cera
# main.cera
import geometry.shapes as shapes

main :: fn() do
  a = shapes.area(shapes.Circle(1.0))
end
```

Visibility: write `pub` before a declaration to let other modules use it;
everything else is private to its file.

## Naming

The standard library follows the style used throughout this book:

- Modules, functions and bindings: `lower_snake_case`
- Types and variants: `UpperCamelCase`
- Constants: `UPPER_SNAKE_CASE`
- Functions that take an explicit allocator end in `_in`
  ([chapter 13](ch13-allocators.md))

Elixir-flavored qualified calls such as `List.map(xs, f)` and `Map.get(m, k)`
are the ordinary way to call library functions. Because the subject comes first,
they pipe well:

```cera
main :: fn() do
  names = people
    |> List.filter(fn(p) -> p.age >= 18 end)
    |> List.map(fn(p) -> p.name end)
end
```

## Recap

- `import a.b.c` or `import a.b.c as x` at the top of a file.
- One file is one module; its path names it.
- Library functions are called qualified and pipe-friendly.
