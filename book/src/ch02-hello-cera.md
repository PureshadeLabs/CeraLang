# Hello, Cera

> **Status:** implemented. `cerac run` type-checks a file and runs its `main`
> function with the interpreter.

Here is the smallest useful Cera program:

```cera
import std.io

main :: fn() do
  io.println("Hello, world!")
end
```

Save it as `hello.cera`. Let's take it apart line by line.

## `import std.io`

`import` brings a module into scope. `std.io` is the input and output module
of the standard library. After the import you refer to its contents with the
module name: `io.println`. You can pick another name:

```cera
import std.io as out

main :: fn() do
  out.println("Hello, world!")
end
```

## `main :: fn() do ... end`

This is a **declaration**. Every top-level thing in Cera has the same shape:

```text
name :: <what it is>
```

Read `::` as "is defined as". Here `main` is defined as a function (`fn`) that
takes no parameters (`()`). The body sits between `do` and `end`.

Functions, types and constants all use the same form:

```cera
PI :: 3.14159

Point :: type { x: f64, y: f64 }

double :: fn(n: i32) -> i32 do
  n * 2
end
```

`main` is special: it is where a program starts.

## `io.println("Hello, world!")`

A function call. The text in quotes is a string literal. `println` writes the
string followed by a newline.

Notice what is *not* there: no semicolons, no `return`, no `main(argc, argv)`.
A newline ends a statement, and the last expression in a block is its value.

## The same program, with a pipe

Cera pipelines pass a value into a function from the left:

```cera
import std.io

main :: fn() do
  "Hello, world!" |> io.println
end
```

`x |> f` means `f(x)`. It becomes more useful when you chain steps, which
[chapter 9](ch09-pipes.md) covers.

## No hidden allocation

`"Hello, world!"` is a string literal baked into the program, so printing it
allocates nothing. Cera cares about *when* memory is allocated and *where* it
comes from. When something does allocate, it uses the context allocator, which
[chapter 13](ch13-allocators.md) explains.

## Comments

```cera
# A comment runs to the end of the line.
main :: fn() do # it can trail code, too
  # and sit inside blocks
end
```

## Checking your program

```text
$ cerac check hello.cera
hello.cera: ok
```

## Running your program

```text
$ cerac run hello.cera
Hello, world!
```

`cerac run` parses and type-checks the file, then calls `main`. A runtime
failure (division by zero, integer overflow, an index out of range) stops the
program with a message that points at the offending expression, and exits with
a non-zero status.

## Recap

- A program is a list of declarations of the form `name :: ...`.
- Blocks open with `do` and close with `end`.
- A newline ends a statement; there are no semicolons.
- `x |> f` is `f(x)`.

## Exercises

`ceralings run intro1` and `ceralings run intro2`.
