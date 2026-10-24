# Unsafe code and data layout

> **Status:** built. `unsafe do ... end` blocks, `unsafe fn`, `std.mem` (raw loads
> and stores), `std.sys` (raw system calls), `repr type` records and `asm` blocks work in native
> builds. Raw pointers are plain `usize` addresses; typed pointer types and
> slices are not part of the language.

Cera's safe subset gives you regions, algebraic data types and pattern
matching. A systems language also has to reach below that: to talk to hardware,
to lay out memory exactly as a device or a wire format demands, to implement
the allocators themselves. This chapter is about that escape hatch.

## Raw memory and system calls today

```cera,ignore
import std.mem
import std.sys

main :: fn() do
  unsafe do
    mem.write_u32(0x4_0000_0000, 7)
    sys.syscall1(1, mem.read_u32(0x4_0000_0000) |> Num.to_i64 |> Option.unwrap_or(0))
  end
end
```

- `std.mem`: `addr_of`, `buffer`, `string_from`, `copy_to`, and `read_u8/16/32/64`,
  `write_u8/16/32/64`. Loads and stores have exactly the width asked for and are
  never merged or dropped, so they are safe to use on device registers.
- Allocation: `alloc(size)`, `free(addr, size)`, `realloc(addr, old, size)`, `live()`,
  `copy(dst, src, len)` and `fill(addr, byte, len)`. Blocks are zeroed and 8-byte
  aligned; sizes round up to a power of two (16 bytes to 1 GiB), and freed blocks are
  reused by later `alloc` calls. Pass `free` the size you asked for.
- Scoped reclaim: `mark()` notes the heap position and `reset(mark)` rewinds to it,
  zeroing the memory. Everything created after the mark (strings and lists too) is
  invalid afterwards, so copy results out first. A `reset` after the heap grew a new
  chunk keeps everything, and discards buffered standard input if that buffer was
  made after the mark.
- `std.sys`: `syscall0` to `syscall6`; the result is the raw return value.
- Calling either module outside `unsafe do ... end` is a compile error. Lambdas
  start outside the block even when written inside one.
- The interpreters refuse these calls with a clear message; use `--native` or
  build an executable.
- The `lythos` package wraps the Lythos kernel calls in safe functions
  ([chapter 19](ch19-targets.md)).

## Terminal programs

`packages/term` (a kote library on Linux) wraps raw terminal mode, window size, key
reading and output: `term.raw()`, `term.size(t)`, `term.read_key(t)`,
`term.write(text)` and `term.restore(t)`. `examples/kilo.cera` is a complete text
editor in the style of kilo; it does the same calls by hand in one file.

## `unsafe`

Calls to `std.mem` and `std.sys` functions are only allowed inside an
`unsafe do ... end` block. Inside it, the compiler trusts you instead of checking.
Keep the blocks small, put a safe function around them, and document what they assume.

A function that is itself unsafe to call is declared `unsafe fn`. Calling it needs
an `unsafe` block, and its body counts as unsafe code:

```cera,ignore
read_register :: unsafe fn(addr: usize) -> u32 do
  mem.read_u32(addr)
end

main :: fn() do
  v = unsafe do read_register(4096) end
end
```

## Explicit layout with `repr type`

Ordinary records have a layout chosen by the compiler. For hardware registers,
packets and file formats, `repr type` pins it down with the C rules: fields in order,
each aligned to its own size, total size rounded up to the largest alignment.

```cera,ignore
Header :: repr type { magic: u32, len: i16, flags: u8, ok: bool, tag: [u8; 4] }
```

`Header` is still an ordinary record. The compiler also generates these functions,
named after the type in snake case:

| Function                    | Meaning                                         |
| --------------------------- | ----------------------------------------------- |
| `header_size()`             | size in bytes                                   |
| `header_align()`            | alignment in bytes                              |
| `header_len_offset()`       | byte offset of the field `len` (one per field)  |
| `unsafe header_read(addr)`  | load a `Header` from memory                     |
| `unsafe header_write(addr, h)` | store a `Header` to memory                   |

Field types: `i8` to `i64`, `u8` to `u64`, `isize`, `usize`, `bool`, a `repr` record
declared earlier in the same file, and `[T; N]` (1 to 256 inline elements of those
types except records). An array field is a list in Cera: reading gives `N`
elements, writing stores the first `N` and zero-fills the rest. Multi-byte values
use the machine's byte order (little-endian on x86-64). There is no `packed` or
explicit alignment, and no floats.

## Inline assembly

`asm("...", inputs...)` assembles a block of x86-64 at compile time and calls it. It is
`unsafe`, native-only (the interpreters stop with an error), and returns an `i64`.

```cera,ignore
sum_to :: fn(n: i64) -> i64 do
  unsafe do
    asm("xor eax, eax
         test rdi, rdi
         jz done
       again:
         add rax, rdi
         dec rdi
         jnz again
       done:", n)
  end
end

ticks :: fn() -> i64 do
  unsafe do
    asm("rdtsc; shl rdx, 32; or rax, rdx")
  end
end
```

- The first argument is a string literal. Up to six more arguments are integers; they arrive
  in `rdi, rsi, rdx, rcx, r8, r9` in that order. The result is whatever is in `rax` at the end.
- The block is its own function: it may use every register and the flags. `rbx`, `rbp`,
  `r12` to `r15` and the stack alignment are restored for you. `ret` anywhere leaves the block.
- Intel syntax. One instruction per line or separated by `;`, `#` for comments, `name:` labels,
  `.byte/.word/.long/.quad` data, `lock`/`rep`/`repne` prefixes.
- Operands: registers of every width (`rax`, `eax`, `ax`, `al`, `r8` to `r15` with `d`, `w`,
  `b`), control registers (`cr0` to `cr8`), numbers, and memory such as
  `qword ptr [rbx + rcx*8 + 16]` or `[rip + label]`.
- Instructions: `mov`, `movzx`, `movsx`, `movsxd`, `lea`, `add or adc sbb and sub xor cmp`,
  `test`, `inc dec neg not mul imul div idiv`, shifts and rotates, `push pop`, `jmp call jcc`,
  `setcc cmovcc`, `xchg xadd cmpxchg bswap`, `bt bts btr btc bsf bsr popcnt lzcnt tzcnt`,
  string instructions, `in out`, `syscall cpuid rdtsc rdmsr wrmsr cli sti hlt`, `lgdt lidt ltr`,
  `invlpg`, `mfence lfence sfence`, `iretq swapgs`, and more. Encodings are checked against GNU as.
- Not supported: SSE and AVX, segment registers, `ret imm`, and named operands. Use `.byte`
  for anything missing.

## Raw pointers

A pointer is a `usize` address. `std.mem` loads and stores at addresses,
`mem.alloc` returns one, and the `repr` functions read and write whole records at one.
Nothing checks that an address is valid.

## Recap

- `unsafe` blocks allow `std.mem` and `std.sys` calls; `unsafe fn` marks functions that need them.
- `repr type` fixes layout and generates size, offset, read and write functions.
- `asm("...")` runs inline x86-64 assembly (native builds only).
- Pointers are `usize`; typed pointers and slices are not built.
