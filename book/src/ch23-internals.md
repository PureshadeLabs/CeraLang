# How cerac works and how Cera self-hosts

> **Status:** the front end (lexer, parser, name resolution, type checker), a
> bytecode compiler and VM, and a tree-walking interpreter exist. The region
> checker, typed core IR and native backends are planned.

You do not need this chapter to *use* Cera. It is here for people who want to
contribute to the compiler, and because the compiler is Cera's first big
program.

## The pipeline

```text
 source text
     |
  lexer           tokens (newlines filtered by continuation rules)
     |
  parser          syntax tree (AST), with error recovery per declaration
     |
  name resolution which name means what; imports
     |
  type checker    Hindley-Milner inference, algebraic types, exhaustiveness
     |
  region checker  the one rule: outer must not point into inner
     |
  typed core IR   a small language with all the sugar removed
     |
  +--------------+---------------------+
  |              |                     |
 bytecode VM    native backend (Cranelift first, own x86-64 later)
 (interpreter)
```

### What exists

- **`cera-syntax`** holds the lexer, AST and parser, and the diagnostics
  rendering shared by every tool.
- **`cera-sema`** does name resolution, type inference (with traits) and match
  exhaustiveness, against stub signatures for the standard library.
- **`cera-eval`** runs checked programs two ways. The **bytecode VM** is the
  default: a compiler (`compile.rs`) turns the syntax tree into one chunk of
  instructions per function, and `vm.rs` runs them. The **tree-walking
  interpreter** (`eval.rs`) is kept as a reference: `cerac run --tree`. Both
  share the value type, the built-in library (`native.rs`) and the operator
  semantics (`rt.rs`), so they print the same thing; a test runs every example
  and ceralings solution on both and compares.
- **`cerac`** is the command-line driver: `check`, `types`, `run`, `dis`
  (print the bytecode), `parse`, `lex`.
- **`cera-lsp`** is the language server; it uses `cera-syntax` and `cera-sema`.
- **`ceralings`** is the exercise runner; it type-checks and runs exercises.

### How the parser works

A hand-written recursive-descent parser. Two details are worth knowing:

1. **Newlines are tokens, and some are filtered.** The lexer drops a newline
   after an operator or comma and before a line that begins with `|>`. The parser
   ignores newlines entirely inside `( )`, `[ ]`, `{ }` and `< >`, and turns them
   back on inside `do ... end`.
2. **Errors are recovered per declaration.** On a syntax error the parser records
   it, skips to the next declaration that starts in the first column, and keeps
   going. An editor sees *all* the broken declarations in a file, not just the
   first.

### How the VM works

Names are resolved at compile time: a local variable is a slot in the function's
frame, a captured variable is a slot in the closure, and functions, constants,
constructors and built-ins become constants. Closures capture by value, which is
safe because bindings are immutable. Patterns are compiled to small trees that
bind straight into frame slots. A tail call reuses the current frame, so tail
recursion runs in constant space. `defer` registers a closure on the frame and
the block's exit (or an early `?` return, or an error) runs it. Cera-to-Cera
calls do not use the Rust stack; recursion depth is limited to 400,000 frames and
reports a Cera stack overflow past that. Run `cerac dis file.cera` to see the
instructions.

### Why bytecode first

A small interpreter is the fastest way to test the language's meaning: write the
type checker, then a VM that runs typed core IR, and you can run real programs
long before native code generation exists. The interpreter also becomes the
oracle for testing the native backend later.

## The standard library during bootstrap

At first the library is written as built-ins in Rust inside the stage0
compiler. As soon as stage0 can compile real Cera, the library is ported to Cera
module by module. That order matters: a self-hosting compiler needs `core` and
enough of `alloc` (strings, arrays, maps) to be written in Cera.

## Self-hosting

A **self-hosted** compiler is one written in the language it compiles. Cera will
get there in three stages:

1. **Stage 0**: the compiler written in Rust (this repository today).
2. **Stage 1**: a Cera compiler written in Cera, compiled by stage 0.
3. **Stage 2**: stage 1 compiles its own source.

The check that ties it together: the **stage 1 and stage 2 binaries must be
identical**. If the compiler compiled by Rust and the compiler compiled by itself
both produce exactly the same output, the compiler is faithfully implementing the
language it is written in.

After that, Rust is no longer needed to build Cera, only to start the chain, and
the compiler can run on Lythos itself.

## Why regions help a compiler

A compiler is a pipeline of phases, each producing a data structure for the next
and then forgetting its own scratch space: the textbook shape for regions. Each
phase can run in its own region; the result is `promote`d once; the scratch
space disappears at the `end`. Writing the Cera compiler in Cera will be the
best test of whether regions are as easy as they are meant to be.

## Contributing

Start with `cargo test` at the repository root. The tests are organized by
crate; every `cera` code block in this book is also parsed by a test, so a
change to the grammar that breaks the book will fail CI.
