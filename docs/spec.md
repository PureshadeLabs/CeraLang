# Cera Language Spec (v0 draft)

Functional systems language. Odin-style declarations, Elixir-style blocks and pipes. Region-based memory. Rust stage0, then self-hosted.

## 1. Goals
- Easy memory management: no GC, no borrow checker, no lifetime annotations
- Usable for kernel code (OROS/Lythos) via explicit allocators and a `core` layer with no allocation
- ML-style inference, ADTs, exhaustive pattern matching
- Pipe-friendly, small flat modules

Non-goals for v0: effect tracking, traits beyond a basic set, concurrency, full region inference.

## 2. Lexical
- Statements end at newline. A line starting with `|>` continues the previous expression. A trailing binary operator or open delimiter also continues.
- Comments: `#` to end of line
- Blocks: `do ... end`
- Identifiers: `[a-z_][a-zA-Z0-9_]*` values and functions, `[A-Z][a-zA-Z0-9_]*` types and variants

## 3. Grammar (EBNF)
```
program   = { item } ;
item      = import | decl ;
import    = "import" path [ "as" ident ] ;
decl      = ident "::" ( fn_def | type_def | const_def ) ;

fn_def    = "fn" "(" [ params ] ")" [ "->" type ] "do" block "end" ;
params    = param { "," param } ;
param     = ident ":" type ;

type_def  = "type" ( variants | record ) "end" ;
variants  = { "|" ident [ "(" fields ")" ] } ;
record    = "{" fields "}" ;
fields    = field { "," field } ;
field     = ident ":" type ;
const_def = expr ;

type      = path [ "<" type { "," type } ">" ]
          | "fn" "(" [ type { "," type } ] ")" "->" type
          | "[" type "]"
          | "(" type { "," type } ")"
          | "ref" type ;

block     = { stmt } [ expr ] ;
stmt      = ident "=" expr
          | "defer" expr
          | expr ;

expr      = pipe ;
pipe      = binary { "|>" binary } ;
binary    = unary { binop unary } ;
unary     = [ "-" | "not" ] postfix ;
postfix   = primary { call | field_access | index | "?" } ;
call      = "(" [ expr { "," expr } ] ")" ;
primary   = literal | path | list | tuple | lambda
          | if | match | region | with | "(" expr ")" ;

if        = "if" expr "do" block [ "else" block ] "end" ;
match     = "match" expr "do" { pattern [ "if" expr ] "->" expr } "end" ;
region    = "region" "do" block "end" ;
with      = "with" ident "(" expr ")" "do" block "end" ;
lambda    = "fn" "(" [ params ] ")" "->" expr "end" ;

pattern   = "_" | ident | literal
          | ident "(" pattern { "," pattern } ")"
          | "(" pattern { "," pattern } ")"
          | "[" [ pattern { "," pattern } ] [ "|" pattern ] "]" ;
```
Notes:
- `x |> f(y)` desugars to `f(x, y)`.
- Reference types use the `ref` keyword. Address-of is a function, `ref_of(v)`, avoiding `&` ambiguity.
- Binary operator precedence: see the table below.

**Changes made while implementing the parser (v0.1).** The parser in
`crates/cera-syntax` is now the source of truth; the grammar actually accepted
is in the book, Appendix C. Differences from the draft above:

- Declarations and functions take type parameters: `List<T> :: type ...`,
  `fn<A, B>(...)`.
- Record types are `type { ... }` with **no** trailing `end`; variant types keep `end`.
- Record literals `Point{x: 1.0, y: 2.0}` and record patterns `Point{x, y: p}` exist.
- List cons works in expressions as well as patterns: `[x | rest]`.
- `do ... end` is an expression; lambdas may have a `do` body: `fn(x) do ... end`.
- `else if` chains share a single `end`.
- Address-of is the function `ref_of(v)`, not a unary `&`.
- Effects (`!alloc`) are not in v0.
- Lines that begin with `|>` continue the previous line; a trailing operator,
  comma, `->`, `=`, `::`, `:`, `.` or opening bracket also continues the line.

Operator precedence, loosest to tightest: `|>`, `or`, `and`, `== !=`,
`< <= > >=`, `+ -`, `* / %`, unary `- not`, postfix `. () [] ?`.

## 4. Memory model: regions

1. **Context allocator.** Implicit parameter threaded through every call (Odin-style). Default is the enclosing region.
2. **`region do ... end`** creates an arena backed by the parent allocator. Allocations inside go to it by default. Everything is freed at `end`.
3. **Lexical regions.** Every reference type carries a region. Inference happens only inside a function body. Function signatures are region-polymorphic by default with simple outlives constraints. Full Tofte-Talpin-style inference is deferred.
4. **Single safety rule.** A reference from an outer region to an inner-region value is a compile error.
5. **Escape is explicit.** A value escaping a region block must be placed in the parent via `promote(v)` or `new_in(parent, v)`. No implicit deep copy. Error text: "value escapes region; use `promote(v)` or `new_in(parent, v)`".
6. **Cross-region mutation is rejected.** Assigning an inner-region value into an outer-region field is an error. Fix with `promote`.
7. **`with allocator(a) do ... end`** swaps the context allocator for the scope. `a` implements the `Allocator` trait (`alloc`, `resize`, `free`). A region is an allocator whose `free` is a no-op and whose `deinit` frees everything.
8. **Cleanup.** No destructors. Non-memory resources use `defer`, run in reverse order at scope exit, including early return.
9. **Long-lived state.** Use a root/global allocator with explicit `free`, or pool allocators. Regions are for request, frame, and phase scopes. One region per loop iteration is the idiom for long-running work.
10. **`core` layer** has no context allocator. Calling an allocating function there is a compile error.

## 5. Standard library

### Layers
1. **`core`** (no allocator, no OS)
   - `option`, `result`, `ordering`, `cmp`, `ops` (Eq, Ord, Hash, Show, Copy, Add...)
   - `num` (checked/wrapping/saturating ops), `bool`, `char`
   - `mem`, `ptr`, `slice` (unsafe, raw), `fmt` core, `iter` traits, `intrinsics`
2. **`alloc`** (needs an allocator)
   - `allocator` interface; `region`, `arena`, `bump`, `pool`, `page`
   - `list` (cons), `array` (growable), `map`/`set` (hash and ordered), `deque`, `heap`
   - `string` (UTF-8), `bytes`/`buffer`, `format`, iter combinators, lazy `stream`
3. **`std`** (needs OS or HAL)
   - `io` (Reader/Writer, buffered), `fs`, `path`, `env`, `process`
   - `time`, `thread`, `sync`, `net`
   - `os` backends: Linux first, then OROS/Lythos (capability-based)

### Rules
- Every allocating function uses the context allocator by default and has an explicit `_in(alloc)` variant.
- Errors are `Result` values with `?` propagation. No exceptions.
- Subject-first argument order so pipes work: `x |> f(y)`.
- Elixir-style module naming: `List.map`, `Map.get`.
- Layers are opt-in per target via manifest. Kernel builds link only `core` + `alloc`.

## 6. Example
```
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

## 7. Implementation plan
**Pipeline:** lexer, parser, name resolution, type check (HM + region check), typed core IR, then backends: bytecode VM first, native (Cranelift, later own x86-64) second.

**Roadmap**
1. Lexer, parser, AST (done)
2. Type checker (HM, ADTs, exhaustiveness) (done, against std stubs; includes traits)
3. Region checker
4. Tree-walking evaluator (done) / bytecode VM
5. `core` + `alloc` (prototype as Rust builtins, port to Cera later)
6. `std` on Linux (`io`, `fs`) and Lythos (`std.os.lythos`, caps, `std.ipc`)
7. Native backend (ELF output for both targets)
8. Self-host: stage0 (Rust) builds stage1 (Cera compiler in Cera), stage1 builds stage2, stage1 and stage2 binaries must match
9. Concurrency, OROS backend, effects

## 8. Targets: Linux and Lythos (both tier 1)

- **Targets:** `x86_64-linux` and `x86_64-lythos`, same compiler, same `core`/`alloc`, separate `os` backends. Output format is ELF on both.
- **Scope:** userspace only. No freestanding/kernel mode in v0. The Lythos syscall ABI is stable and is bound directly by the `lythos` backend.
- **Capability-first std:** `io`, `fs`, `net`, `time`, `process` take capabilities as arguments, no ambient authority. `main` may receive a `Caps` value:
  ```
  main :: fn(caps: Caps) do
    caps.stdout |> io.println("hi")
  end
  ```
  `io.println("hi")` remains as sugar over the default stdout cap.
  - Lythos: caps are real kernel capabilities.
  - Linux: startup shim builds a root `Caps` from fds (`openat` and directory fds act as caps), keeping the API identical.
- **Allocator:** the root allocator is backed by a memory/page capability. Regions layer on top; `with allocator(a)` works with cap-backed allocators.
- **Platform layer:** `std.os` is a small trait set (`Mem`, `Fs`, `Net`, `Clock`, `Ipc`, `Thread`) with `linux` and `lythos` implementations. Everything above is portable.
- **`std.ipc`:** first-class module for IPC and capability transfer. Linux emulation via unix sockets and SCM_RIGHTS.
- **Compiler:** `--target` flag, per-target syscall intrinsics.
- **Testing:** Linux native, Lythos under QEMU.
- **Long-term:** the Cera compiler runs on Lythos itself.

## 9. Open questions
The full, maintained list is in the book, Appendix D. Highlights:
- Loops and tail calls (recursion only, or `for`/`while`?)
- Mutable state (bindings, ref cells, mutable fields)
- Type annotations on bindings and integer width control
- Concurrency model: threads + channels vs actors
- Trait system syntax and scope for v0
- Visibility, package manifest, import cycles
- String concatenation / interpolation; tuple indexing; record update
- Operator overloading; integer overflow semantics
- Whether `std` is written in Cera from day one or prototyped as Rust builtins (leaning: builtins first)

## 10. Tooling and learning materials
- `cerac`: compiler driver (`check`, `parse`, `lex` today).
- `cera-lsp`: language server (diagnostics, symbols, hover, definition, completion today).
- `ceralings`: rustlings-style exercises; type-checked and, where an exercise has `# expect:` lines, run and compared.
- `book/`: "The Cera Programming Language", an mdBook. Every ```cera block is parsed by a test.
