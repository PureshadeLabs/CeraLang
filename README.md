# Cera

A functional systems programming language: Odin-style declarations, Elixir-style
blocks and pipes, ML-style types, and **regions** instead of a garbage collector
or a borrow checker. First-class targets: Linux and Lythos.

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

## Status

Early. The front end (syntax, names, types, exhaustiveness) and the tooling
around it exist; code generation and the region checker do not yet.

| Piece                                  | State           |
| -------------------------------------- | --------------- |
| Lexer, parser, syntax errors           | done            |
| `cerac check / types / run / parse / lex` | done         |
| Language server (`cera-lsp`)           | done (syntax-level features) |
| `ceralings` exercises (22)             | done (type-checks and runs) |
| The book (23 chapters, 5 appendices)   | drafted         |
| Name resolution, type checker, exhaustiveness | done (against std stubs) |
| Region checker                         | designed        |
| Tree-walking interpreter (`cerac run --tree`) | done (reference) |
| Bytecode VM (`cerac run`, `cerac dis`) | done            |
| Native backend (x86-64 Linux ELF, `cerac build`) | done  |
| Package manager (`kote`)               | done (path and git dependencies) |
| Standard library                       | first slice (lists, strings, maps, sets, io, fs, time, env, process) |

## Layout

```text
crates/cera-syntax   lexer, AST, parser, diagnostics
crates/cera-sema       name resolution, type inference, exhaustiveness, std stubs
crates/cera-eval       bytecode compiler + VM, tree-walking interpreter, std natives
crates/cera-native     native x86-64 backend (Cranelift, own ELF writer, no libc)
crates/cerac         compiler driver (+ tests that parse every book snippet)
crates/kote          package manager and build tool
crates/cera-lsp      language server (stdio, JSON-RPC)
crates/ceralings     exercise runner
ceralings/           exercises/ and solutions/
book/                the book (mdBook)
docs/spec.md         language specification (draft)
examples/            sample programs
```

## Build and test

```text
cargo build --release
cargo test
```

Binaries land in `target/release/`: `cerac`, `kote`, `cera-lsp`, `ceralings`.

```text
cerac check examples/shapes.cera
cerac types examples/shapes.cera
cerac run examples/shapes.cera
cerac build examples/shapes.cera -o shapes
kote new hello && cd hello && kote run
cerac parse examples/shapes.cera
ceralings list
ceralings run
```

## The book

The sources are in `book/src`. To render it:

```text
cargo install mdbook
mdbook serve book
```

`cargo test` also checks that every `cera` code block in the book parses, that
every chapter is in `SUMMARY.md`, that internal links resolve, and that every
exercise the book mentions exists.

## Editor support

`cera-lsp` speaks LSP over stdio. Configuration snippets for Neovim and Helix
are in the book, chapter 1.

## License

MIT. See `LICENSE`.
