# Capabilities

> **Status:** implemented for the console, filesystem, clock and environment
> under `cerac run`. `Caps` has the fields `console`, `fs`, `clock` and `env`.
> Network and IPC capabilities, and Lythos kernel capabilities, are designed but
> not built; names there are provisional.

A **capability** is an unforgeable token that grants permission to do one
specific thing: read this directory, write to that console, open connections to
that address. Lythos, the OS Cera is built for, is capability-based: a program
can only touch what it has been handed.

Cera's standard library is shaped the same way. There is **no ambient
authority**: no global "open any file by path" that works because the program
happens to be allowed to. Operations that reach the outside world take the
capability they need as an argument.

## `main` receives capabilities

A program that needs the outside world declares a `Caps` parameter:

```cera
import std.io

main :: fn(caps: Caps) do
  io.writeln(caps.console, "hi")
end
```

`Caps` holds what the program was started with: `console`, `fs` (a directory,
by default the current one), `clock` and `env` (arguments and variables).
`main` may take no parameter at all, in which case it gets no capabilities.

For convenience, plain `io.println("hi")` stays available. It prints to the
default console.

The `fs` capability is a sandbox: names may not be absolute or contain `..`, and
symbolic links that lead outside the directory are refused. Failures come back
as `Err(Error{message: ...})`.

## Passing capabilities down

Because capabilities are ordinary values, you decide who gets what by passing
them (or not):

```cera
import std.fs

count_lines :: fn(dir: Dir, name: String) -> Result<usize, Error> do
  text = fs.read_to_string(dir, name)?
  Ok(String.lines(text) |> List.length)
end

main :: fn(caps: Caps) do
  match fs.open_dir(caps.fs, "data") do
    Ok(data) -> io.println(count_lines(data, "notes.txt"))
    Err(e) -> io.println(e)
  end
end
```

`count_lines` can only read inside the directory it was handed. It cannot wander
elsewhere, even by mistake, because it was never given the authority to.

## Attenuating

You can derive a *weaker* capability from a stronger one: a read-only view of a
directory, a subdirectory, a connection to a single host. You can hand the weak
one to untrusted code and keep the strong one:

```cera
main :: fn(caps: Caps) do
  match fs.open_dir(caps.fs, "logs") do
    Ok(logs) -> analyze(fs.read_only(logs))
    Err(e) -> io.println(e)
  end
end
```

## Why this belongs in a language

- **Least privilege by default.** A library you call can only affect what you
  pass it.
- **Testing is easy.** Hand a function a fake directory or a fake clock.
- **It maps directly onto Lythos**, where a capability is a real kernel
  object, and onto Linux, where the same API is built from file descriptors.

## Memory is a capability too

The root allocator is backed by a memory capability. Regions and arenas are
built on top of it. A component given no memory capability simply cannot
allocate beyond what its parent gave it, which makes resource limits enforceable
rather than conventional.

## Sending capabilities between programs

Programs on Lythos talk through IPC, and a message can carry capabilities along
with data. `std.ipc` exposes that directly:

```cera
import std.ipc

forward :: fn(chan: Channel, file: File) -> Result<(), Error> do
  chan |> ipc.send(Message{data: "here you go", caps: [file]})
end
```

On Linux the same module works by passing file descriptors across Unix domain
sockets, so programs behave the same on both targets.

## Recap

- Operations that reach the outside world take a capability argument.
- `main :: fn(caps: Caps)` receives what the program was started with.
- You can attenuate: make a weaker capability from a stronger one.
- The same API runs on Lythos (kernel capabilities) and Linux (file descriptors).
