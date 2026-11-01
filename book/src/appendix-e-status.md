# Appendix E: Status and roadmap

## Done

- [x] Language design draft (`docs/spec.md`)
- [x] Lexer, parser, AST, syntax errors with recovery (`cera-syntax`)
- [x] `cerac check | types | parse | lex`
- [x] Name resolution, type inference (Hindley-Milner with literal defaulting), match exhaustiveness (`cera-sema`), checked against stub signatures for the standard library
- [x] Language server: syntax and type diagnostics, document symbols, hover (declarations and inferred types), definition, completion (`cera-lsp`)
- [x] `ceralings` runner: type-checks exercises and compares output when an exercise has `# expect:` lines
- [x] Standard library slice with Rust natives: lists, strings, options, results, numbers, ordered maps and sets, console, filesystem, clock, environment, exit codes, and `Caps` passed to `main`
- [x] `kote`, the package manager: `kote.toml`, multi-file modules, path and git dependencies, `kote.lock`, `new/init/check/build/run/test/add/remove/update/tree/clean`
- [x] This book, through the designed language
- [x] Native Lythos target, `unsafe`, `std.mem`/`std.sys`, manual allocation, the `lythos` and `term` packages
- [x] `std.bits`, mutable `Array`, record update, `\\x`/`\\u{}` escapes, stack overflow checks
- [x] `&` `^` `<<` `>>` `~`, `x[i]` on lists and arrays, regions that free memory (native), inline `asm`
- [x] `lythos.caps` typed capabilities, `unsafe fn`, `repr type` records (C layout, generated read/write)

## Roadmap

1. [x] Name resolution
2. [x] Type checker (Hindley-Milner, algebraic data types, exhaustiveness); traits (`trait`, `impl`, bounds)
3. [ ] Region checker
4. [x] Tree-walking evaluator (`cerac run --tree`)
   - [x] Bytecode compiler and VM (`cerac run`, `cerac dis`)
5. [~] `core` and `alloc`, as Rust built-ins first (a first slice is done; hash maps, arrays, `format` remain)
6. [~] `std` on Linux (`io`, `fs`, `time`, `env`, `process` done), then on Lythos (`std.os.lythos`, capabilities, `std.ipc`)
7. [~] Native backend: x86-64 Linux ELF done (`cerac build`, `kote build`); Lythos target remains
8. [ ] Self-hosting: Cera compiler written in Cera; stage1 and stage2 must match
9. [ ] Concurrency, effects, and the Lythos-hosted compiler

## Tooling still to come

- [ ] Rename and find references in the language server
- [x] `cerac fmt` and `kote fmt` (canonical layout, comments kept, idempotent)
- [ ] Syntax highlighting grammar and a VS Code extension
- [ ] mdBook syntax highlighting for `cera` code blocks

## Known limits of the current type checker

- The standard library is a set of stub signatures, one file per module under
  `crates/cera-sema/src/std`; chapter 17 lists them. Importing any other module
  is an error.
- Traits are implemented (chapter 16) with the restrictions listed there:
  `self`-first methods only, no default bodies, associated types or supertraits.
  Operators work through the core traits (`Add`, `Ord`, ...), and `io.println`
  needs `Show`.
- Local bindings are monomorphic: a lambda stored in a local variable cannot
  be used at two different types. Top-level functions are fully generic.
- Field access needs the record type to be known at that point; annotate a
  lambda parameter if the checker reports that it cannot tell.
- A function without a `->` return type has its return type inferred from the
  body, but it is not generalized.
- Unused or unreachable match arms are not reported yet.
- The region checker is partial: a `region` whose value is a heap value built or bound inside it is
  rejected unless wrapped in `promote`/`new_in`. Cross-region mutation is not checked yet.

## Known limits of the interpreter

- The default engine is a bytecode VM; the tree-walking interpreter remains as a
  reference with `--tree`. Both run at interpreter speed (fib(32) takes about
  0.45 s on the VM, CPython 3.13 about 0.35 s).
- `cerac build FILE -o OUT` compiles to a static x86-64 Linux ELF (no libc, raw
  syscalls) through Cranelift; `cerac run --native` builds and runs it, falling
  back to the VM when the program uses something unsupported. fib(32) takes about
  0.02 s. Output matches the VM (checked by the differential tests), including runtime errors with a source excerpt. Diagnostics are Rust-style: labels, `= help:` advice, and did-you-mean suggestions.
- Native limits: Map and Set are O(n) association lists; stdout is unbuffered;
  only self tail calls become loops;
  case mapping and trim are ASCII only; `fs` and `parse_f64` are not supported.
- Regions, `with allocator`, `promote`, `ref_of` and `new_in` have no runtime
  effect; memory is reference counted. They matter for the compiled backends.
- Tail calls do not grow the stack. Other recursion is limited (400,000 frames on
  the VM, a 512 MiB stack on the tree-walker); exceeding it is reported as a Cera
  stack overflow.
- Integers carry their type and arithmetic overflow is an error.
- `sum` of an empty list is an error until a `Zero`-like trait exists.
- The standard library is the slice listed in chapter 17. There is no network,
  thread or path module yet; `Map` and `Set` are sorted vectors (linear update).
- Capabilities are checked when used, not by the type checker alone: `Dir` and
  friends are distinct types, but the sandbox rules (no `..`, no escaping
  symlinks, read-only views) are enforced at run time.
- `String.length` counts bytes.

## Book status

Chapters 1 to 11 describe features whose syntax already parses. Chapters 12 to
15 describe the region model, which is fully designed but needs the region
checker. Chapters 16 to 20 are design previews of features that are not yet in
the grammar or not implemented. Chapters 21 and 22 are projects that become
runnable as the compiler grows.
