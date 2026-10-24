# Targets: Linux and Lythos

> **Status:** partly built. Both targets compile native executables today
> ([Building for Lythos today](#building-for-lythos-today)); the `std.os` split
> and the target-specific capability startup described below are still the plan.

Cera has two **tier 1** targets from the start:

| Target            | What it is                                              |
| ----------------- | ------------------------------------------------------- |
| `x86_64-linux`    | ordinary Linux userspace                                |
| `x86_64-lythos`   | userspace on Lythos, the capability-based OS            |

Tier 1 means both are built, tested and kept working together. Neither is an
afterthought of the other. The same compiler, the same `core` and `alloc`, and
the same language semantics serve both; only the `os` backend of `std` differs.

Both targets produce **ELF** executables. Cera targets *userspace* programs on
both; writing a kernel in Cera is not a goal of version 0.

## Choosing a target

```text
$ cerac build hello.cera                          # host target
$ cerac build --target x86_64-lythos hello.cera   # cross-compile for Lythos
```

## Building for Lythos today

The native backend can already produce Lythos userspace executables:

```text
$ cerac build --target x86_64-lythos hello.cera -o hello
$ kote build --target x86_64-lythos        # target/lythos/bin/<name>
```

The executable is a static ELF64 loaded at `0x1_0000_0000` (Lythos rejects
segments below 1 GiB) that speaks the Lythos syscall ABI directly: standard
output and error go to the kernel console (`SYS_LOG`), memory comes from the
program break (`SYS_BRK`), and the exit status goes through `SYS_TASK_EXIT`.
`std.time` uses `SYS_TIME_EPOCH` and `SYS_NANOSLEEP`; standard input reads the
serial port. Arguments reach `main` through the initial stack frame.

Limits for now:

- `std.fs` works on Lythos through `SYS_OPEN`, `SYS_READ`, `SYS_WRITE` and friends.
  The kernel has no truncate, append or rmdir, so a write replaces the file
  (unlink then create), an append rewrites it, and removing a directory fails with
  "Invalid argument". The network is not wired up.
- `Caps` is built by the startup code, not from the capabilities the kernel hands
  out. Lythos authorizes by syscall, so file access is limited by the directory
  value in `Caps`, and the rest by the capability handles you pass to the
  `lythos` package.
- The stack is the 256 KiB the kernel provides, so deep non-tail recursion is bounded by it.
  Running out is a clean runtime error ("stack overflow"), on Linux too (1 GiB there).
- Ordinary values (strings, lists, records) are never freed: the heap only grows.
  Long-running services reclaim memory with `mem.alloc`/`mem.free` for buffers, or
  `mem.mark`/`mem.reset` around each unit of work (see [chapter 20](ch20-unsafe.md)).
- The tests run the executables under an emulator that models the syscall
  ABI. Booting them under QEMU on Lythos itself is the next check.

### Kernel interface: the `lythos` package

Everything else the kernel offers is in the `lythos` package (`packages/lythos`
in the source tree), written in Cera on top of `std.sys` and `std.mem`:

| Module          | Wraps                                                        |
| --------------- | ------------------------------------------------------------ |
| `lythos`        | error values, `check`, capability rights                     |
| `lythos.ipc`    | create, bind, lookup, send, recv, poll, timeouts, send_cap, recv_cap |
| `lythos.task`   | spawn (ELF, caps, argv), wait, kill, state, grant, revoke, exit, yield |
| `lythos.pages`  | `SYS_MMAP` and `SYS_MUNMAP` at chosen addresses              |
| `lythos.file`   | `read_bytes`: whole files as raw bytes (used by `task.spawn_file`) |
| `lythos.caps`   | `Cap` handles tagged with a kind, rights, `boot()` handles 0 to 3, `boot_info`, typed `spawn` and `grant` |
| `lythos.device` | claim, config space, registers, DMA, interrupts              |

```cera,ignore
import lythos.ipc

main :: fn() do
  match ipc.lookup("lythmsg", 3) do
    Ok(bus) -> ipc.send(bus, "ping")
    Err(e) -> Err(e)
  end
end
```

The kernel has no call to list the capabilities a task holds, so a program learns what
it has from whoever started it: `lythd` gets handles 0 (memory), 1 (rollback), 2 (the
boot-info endpoint) and 3 (filesystem), and a spawned task finds the capabilities its
parent listed, in order. `caps.assume(handle, kind)` states what a handle is, and the
`caps` functions refuse to use it as the wrong kind. The `Caps` value passed to `main`
already reflects what the kernel enforces: console, a root `Dir`, clock.

Programs reach raw memory and system calls with `unsafe do ... end` and the
`std.mem` and `std.sys` modules. Calling those outside an `unsafe` block is a
compile error, and the interpreters stop with a message because only native
builds can run them.

`--target` is accepted by `kote build` only; `kote run` and `kote test` run on
the host.

## One API, two backends

The standard library's platform layer, `std.os`, is a small set of traits:

| Trait    | Responsibility                                      |
| -------- | --------------------------------------------------- |
| `Mem`    | getting pages of memory                             |
| `Fs`     | files and directories                               |
| `Net`    | sockets and connections                             |
| `Clock`  | time                                                |
| `Ipc`    | messages and capability transfer                    |
| `Thread` | threads and synchronization primitives              |

There is a `linux` implementation and a `lythos` implementation. Everything
above that layer (`io`, `fs`, `net`, `fmt`, collections...) is written once.

## How a Cera program gets its capabilities

On Lythos, the kernel hands the new process real capabilities, and the startup
code turns them into the `Caps` value that reaches `main`
([chapter 18](ch18-capabilities.md)).

On Linux there are no kernel capabilities, so a small startup shim builds the
same `Caps` out of file descriptors: standard streams become the console
capabilities, and a directory file descriptor opened with `openat` stands in for
a directory capability. The API your program sees is identical.

## Code that differs per target

Most programs never need to know which target they run on. When one does (for
example a tool that only makes sense on Lythos), the target is available at
compile time:

```cera,ignore
import std.target

describe :: fn() -> String do
  match target.os do
    Linux -> "running on Linux"
    Lythos -> "running on Lythos"
  end
end
```

> **Provisional:** how conditional compilation is spelled is not decided.

## Testing

- Linux builds are tested natively.
- Lythos builds are tested under QEMU, booting a Lythos image.

## Debugging

Native executables from `cerac build` and `kote build` carry DWARF line information. Breakpoints and backtraces use Cera source files and lines:

```text
$ gdb -batch -ex 'break sum_to' -ex run -ex bt ./loops
Breakpoint 1, sum_to () at loops.cera:9
```

`addr2line -e ./loops <address>` also works. Only functions and lines are described; variables and types are not.

## The long-term goal

The Cera compiler itself will be self-hosted ([chapter 23](ch23-internals.md)),
and the goal after that is for it to run **on Lythos**, so the OS and the
language that builds its userspace stand on each other.

## Recap

- Linux and Lythos are both tier 1; both emit ELF; userspace only.
- `std.os` is the thin platform layer; everything above it is shared.
- Capabilities map onto kernel capabilities on Lythos and file descriptors on
  Linux, behind the same API.
