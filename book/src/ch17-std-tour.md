# A tour of `core`, `alloc` and `std`

> **Status:** a first slice runs today (see the next section). The rest of the
> chapter is the design. Names outside the implemented slice are
> **provisional**; the *layering* and the *conventions* are the decisions.

## What runs today

Under `cerac run`, these modules exist, each with the functions listed:

| Module       | Functions |
| ------------ | --------- |
| prelude      | `map filter fold sum length reverse append head tail`, `promote ref_of new_in arena_new`, `Option`, `Result`, `Ordering`, `Error`, `Caps` |
| `List`       | the prelude list functions plus `get last is_empty take drop concat flat_map any all find zip range contains sort sort_by` |
| `String`     | `length concat is_empty contains starts_with ends_with split join trim to_upper to_lower replace repeat lines chars from_chars parse_i64 parse_f64 slice` |
| `Option`     | `is_some is_none unwrap_or map and_then ok_or` |
| `Result`     | `is_ok is_err unwrap_or map map_err and_then ok` |
| `Num`        | `min max clamp abs pow sqrt floor ceil round to_f64 to_i8 to_i16 to_i32 to_i64 to_isize to_u8 to_u16 to_u32 to_u64 to_usize` |
| `Bits`       | `band bor bxor bnot shl shr rotl rotr popcount leading_zeros trailing_zeros wrapping_add wrapping_sub wrapping_mul` on any integer width (`and`, `or`, `not` are keywords) |
| `Array`      | growable arrays changed in place, O(1) `get` and `set`: `new filled from_list to_list length get set push pop insert remove swap clear copy` |
| `Map`, `Set` | immutable ordered collections: `new insert get get_or remove contains_key size is_empty keys values to_list from_list merge` and `new insert contains remove size is_empty to_list from_list union` |
| `std.io`     | `println print eprintln eprint read_line read_all`, and with a console capability `write writeln read_line_from` |
| `std.fs`     | `open_dir read_only read_to_string write_string append_string exists list remove make_dir` |
| `std.time`   | `now_ms sleep_ms` (take a `Clock`) |
| `std.env`    | `args var` (take an `EnvCap`) |
| `std.process`| `exit` |
| `std.mem`, `std.sys` | unsafe raw memory, allocation and system calls ([chapter 20](ch20-unsafe.md)) |

The capitalized names (`List`, `String`, `Option`, `Result`, `Num`, `Bits`, `Array`, `Map`, `Set`)
work without an import; the lowercase modules need `import std.io` and so on.
Arrays are the one mutable collection: every name that refers to an array sees
changes, and `Array.copy` makes an independent one. `Map` and `Set` keep their entries sorted and copy on update, so `insert` is
linear; hash-based versions come with `alloc`. Strings count bytes.

Cera's standard library has three layers. Each layer builds on the one before
and needs less from the environment than the one after.

```text
  std     needs an operating system (or a HAL)   files, network, threads, time
   |
  alloc   needs an allocator                     lists, maps, strings, streams
   |
  core    needs nothing                          Option, Result, numbers, traits
```

A program picks the layers it links through its manifest. A kernel or driver
can link `core` and `alloc` only; an ordinary program links all three.

## `core`: no allocator, no OS

Everything here works in an interrupt handler or at the very start of boot.

| Module        | Contains                                                         |
| ------------- | ---------------------------------------------------------------- |
| `option`      | `Option<T>`: `Some`, `None`                                      |
| `result`      | `Result<T, E>`: `Ok`, `Err`                                      |
| `ordering`, `cmp` | `Ordering`, comparison helpers                               |
| `ops`         | the traits `Eq`, `Ord`, `Hash`, `Show`, `Copy`, `Add`, ...       |
| `num`         | integer and float helpers, checked/wrapping/saturating arithmetic|
| `bool`, `char`| helpers for the primitives                                       |
| `mem`         | `size_of`, `align_of`, `copy`, `swap`                            |
| `ptr`, `slice`| raw pointers and slices (unsafe)                                 |
| `fmt`         | the formatting machinery behind `Show`                           |
| `iter`        | the iteration traits                                             |
| `intrinsics`  | operations the compiler implements itself                        |

Because `core` has no context allocator, nothing in it allocates.

## `alloc`: with an allocator

| Module                  | Contains                                             |
| ----------------------- | ---------------------------------------------------- |
| `allocator`             | the `Allocator` interface ([chapter 13](ch13-allocators.md)) |
| `region`, `arena`, `bump`, `pool`, `page` | the standard allocators            |
| `list`                  | the persistent singly linked list                    |
| `array`                 | a growable contiguous array                          |
| `map`, `set`            | hash-based and ordered maps and sets                 |
| `deque`, `heap`         | double-ended queue, priority queue                   |
| `string`                | UTF-8 `String`                                       |
| `bytes`, `buffer`       | byte sequences and builders                          |
| `format`                | building strings from values                         |
| `iter`                  | iterator adapters: `map`, `filter`, `fold`, ...      |
| `stream`                | lazy, Elixir-style pipelines                         |

## `std`: with an operating system

| Module          | Contains                                               |
| --------------- | ------------------------------------------------------ |
| `io`            | `Reader` and `Writer` traits, buffered I/O, `println`  |
| `fs`, `path`    | files and paths                                        |
| `env`, `process`| environment and subprocesses                           |
| `time`          | clocks and durations                                   |
| `thread`, `sync`| threads, mutexes, channels, atomics                    |
| `net`           | sockets                                                |
| `ipc`           | messages and capability transfer ([chapter 18](ch18-capabilities.md)) |
| `os`            | the per-platform backends: `linux`, `lythos`           |

## Conventions that run through all of it

**Subject first.** The first parameter is the thing being operated on, so calls
pipe: `names |> List.map(f) |> List.sort`.

**Allocation is visible.** Allocating functions use the context allocator and
have an `_in(alloc)` form.

```cera
main :: fn() do
  a = List.map(xs, f)
  b = List.map_in(scratch, xs, f)
end
```

**Errors are values.** Fallible functions return `Result`. There are no
exceptions.

**Small, flat modules.** Names follow Elixir: `List.map`, `Map.get`,
`String.split`. A module has a handful of functions, not a hundred.

**No ambient authority.** Anything that touches the outside world takes a
capability ([chapter 18](ch18-capabilities.md)); `std` may offer a convenience
`println` that uses the default console capability, but the explicit form
always exists.

## Using the library

Typical code, using the implemented modules:

```cera
import std.io

main :: fn() do
  region do
    words = ["pear", "apple", "fig"]
    words
      |> List.sort
      |> List.map(String.length)
      |> List.sum
      |> io.println
  end
end
```

## Build order

The library is built in the order a self-hosted compiler needs it:

1. Compiler intrinsics and `core`
2. `alloc`: the region allocator, then `string`, `array` and `map`
3. `fmt`, `io` and `fs` on Linux
4. Concurrency: `sync` and `thread`
5. The Lythos backend

The first versions will be written as Rust built-ins inside the stage0 compiler
and ported to Cera once it can compile them.
