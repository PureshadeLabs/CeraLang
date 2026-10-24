# Introduction

Cera is a functional systems programming language. It aims at one specific
combination:

- **Easy memory management.** No garbage collector, no borrow checker, no
  lifetime annotations. Memory lives in *regions* that are freed all at once.
- **A pleasant syntax.** Declarations look like Odin (`name :: ...`), blocks and
  pipelines look like Elixir (`do ... end`, `|>`).
- **Real systems access.** Explicit allocators, explicit data layout, no hidden
  runtime. Cera is meant to write operating-system userspace, including for
  Lythos (the capability-based OS from Pureshade Labs) as well as Linux.
- **A functional core.** Immutable values, algebraic data types, exhaustive
  pattern matching and Hindley-Milner type inference.

A first taste:

```cera
import std.io

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

main :: fn() do
  region do
    shapes = [Circle(2.0), Rect(3.0, 4.0)]
    shapes
      |> map(area)
      |> sum
      |> io.println
  end
end
```

Everything in that program is explained in this book. By the end you will know
what `region` does, why `|>` reads left to right, and how `match` keeps you
honest about every case.

## Who this book is for

You should have written some code in any language. You do not need to know
Rust, Haskell, Elixir or Odin, though readers who do will recognize the
ingredients. The book starts from zero and builds up:

1. **Getting started** installs the tools and runs a first program.
2. **The basics** covers values, functions, data and pattern matching.
3. **Memory** is the heart of Cera: regions, allocators and `defer`.
4. **Abstraction**, **the standard library** and **systems programming**
   cover generics, the three std layers, capabilities and targets.
5. **Projects** puts it all together.
6. **Under the hood** explains the compiler and the plan to self-host it.

## Where Cera is today

Cera is young. This book describes the language as it is *designed*, and it is
honest about how much of that design exists in the compiler yet. Each chapter
starts with a status line.

| Piece                                   | State                             |
| --------------------------------------- | --------------------------------- |
| Lexer, parser, syntax errors            | Implemented (`cerac check`)       |
| Language server (`cera-lsp`)            | Implemented: diagnostics, symbols, hover, go-to-definition, completion |
| Name resolution, type checker, exhaustiveness | Implemented (`cerac check`, `cerac types`), against stub std signatures |
| Region checker                          | Designed, not implemented         |
| Bytecode VM (`cerac run`)               | Implemented                       |
| Bytecode VM, native backend             | Designed, not implemented         |
| Standard library                        | Designed, not implemented         |
| `ceralings` exercises                   | 20 exercises, syntax-level checking |

Every `cera` code block in this book is run through the real parser as part of
the test suite, so the syntax you see is the syntax the compiler accepts. Code
that depends on unfinished features says so, and names in the standard library
are **provisional** until the std stabilizes.

## Conventions

- `code in running text` is Cera unless noted.
- Blocks marked `text` show program output or compiler messages.
- Boxes that start with **Provisional** describe a decision that is likely but
  not final. [Appendix D](appendix-d-design.md) lists every open question.

## Learning by doing

Reading is not enough. The repository ships **ceralings**, a set of small
broken programs you fix one at a time, in the spirit of Rust's `rustlings`.
Each chapter ends with the matching exercises.

Let's begin.
