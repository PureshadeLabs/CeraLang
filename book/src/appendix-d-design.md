# Appendix D: Design decisions and open questions

## Decisions

These are settled for version 0.

**Regions, not ownership.** Memory is managed by lexical regions. There is no
garbage collector, no borrow checker and no lifetime annotations. The single
safety rule: a reference from an outer region into an inner region is an error.

**Explicit escape.** A value leaves a region only through `promote(v)` (a
visible copy) or `new_in(alloc, v)`. There is no implicit deep copy.

**No cross-region mutation.** Storing an inner-region value into an outer-region
structure is rejected; `promote` is the fix.

**Lexical region inference.** Regions are exactly the `region do ... end`
blocks. Inference stays within a function body; signatures are
region-polymorphic by default. Whole-program region inference is deferred.

**Context allocator.** Every call carries one. `with allocator(a)` swaps it.
Library functions that allocate have `_in(alloc)` forms. `core` has none.

**No destructors.** Memory belongs to regions, other resources to `defer`.

**Errors are values.** `Option` and `Result`, with postfix `?`. No exceptions,
no `null`.

**Syntax.** Odin-style declarations (`name :: ...`), Elixir-style `do ... end`
blocks and `|>` pipes, newline-terminated statements, `#` comments.

**Types.** Hindley-Milner inference, algebraic data types, exhaustive pattern
matching, traits (no inheritance), monomorphization.

**Standard library.** Three layers (`core`, `alloc`, `std`), subject-first
functions, small flat modules, no ambient authority.

**Targets.** `x86_64-linux` and `x86_64-lythos`, both tier 1, both ELF,
userspace only.

**Implementation.** Stage 0 in Rust; a bytecode VM first; native backend after
(Cranelift, later our own); self-host with a stage1 = stage2 check.

**Effect tracking is out of v0.**

## Open questions

Each of these needs a decision before the feature it touches is implemented.

| Question | Why it matters |
| -------- | -------------- |
| **Type annotations on bindings.** `x: u8 = 5`? A cast function? | Needed for integer width control. Literals currently default to `i32`/`f64`. |
| **Mutation beyond locals.** Mutable record fields, in-place list updates, and letting closures capture a `var` (today a `var` is local to its function). | Systems code (buffers, hardware) and ergonomics of imperative loops. |
| **Loop forms.** Ranges (`for i in 0..n`), iteration over strings and maps, `loop`, labelled `break`, `break` with a value, and guaranteed tail calls. | Everyday iteration; today `for` takes a list and ranges are `list.range`. |
| **Registry hosting and trust.** `kote` has path, git, directory and HTTP registry dependencies with semver, yanking and several majors per graph. A reference server, index checksums, signatures and owners are open. | Sharing packages safely. |
| **Concurrency.** Threads and channels, Elixir-style actors, or both? | Interacts with regions and capabilities. |
| **Trait extensions.** Associated types, default methods, supertraits, associated functions (`default() -> Self`), `Eq`/`Hash`/`Copy`. | The v0 core (`trait`, `impl`, bounds) is implemented; see chapter 16. |
| **String building.** Concatenation operator, interpolation, or only `String.concat`/`format`? | Everyday ergonomics. |
| **Tuple indexing.** `t.0`/`t.1`, or patterns only? | The lexer reads `t.0.1` as a float. |
| **Record update.** Syntax for "this record with `x` changed". | Ergonomics of immutable records. |
| **Conditional compilation.** How do programs branch on target or features? | Cross-target libraries. |
| **Character literals.** Syntax such as `'a'`. | The lexer has none yet. |
| **Early return.** Is `?` enough, or do we want `return`? | Control-flow style. |
| **`unsafe` and `repr` syntax.** Final spelling and semantics. | Chapter 20 is a plan only. |
| **Integer overflow.** Checked by default, wrapping, or build-mode dependent? | Safety versus speed. |
| **Operator overloading.** Only through core traits like `Add`? | Library design. |
| **Where std is written.** Rust built-ins first (leaning) or Cera from day one. | Bootstrap order. |
| **Formatter.** A canonical `cerac fmt`. | Style consistency, editor integration. |
