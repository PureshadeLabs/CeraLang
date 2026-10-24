# Cleanup with `defer`

> **Status:** syntax implemented; semantics designed.

Regions free **memory**. Memory is not the only thing a program has to give
back: files, sockets, locks, device handles. For those, Cera has `defer`.

Cera has **no destructors**. Nothing runs automatically when a value stops
being used. Cleanup is something you say, in the place where you acquire the
resource.

## Using `defer`

`defer expression` schedules the expression to run when the current scope
ends, no matter how it ends:

```cera
import std.fs
import std.io

copy_first_line :: fn(path: String) -> Result<(), Error> do
  file = fs.open(path)?
  defer fs.close(file)

  line = io.read_line(file)?
  io.println(line)
  Ok(())
end
```

`fs.close(file)` runs when `copy_first_line` finishes, whether it reaches the
last line, or leaves early through one of the `?` operators.

The acquire and the release are next to each other, so you can see at a glance
that they match.

## Order

Several `defer`s in one scope run in **reverse** order, like unstacking
plates: the last acquired is the first released.

```cera
main :: fn() do
  a = acquire("a")
  defer release(a)
  b = acquire("b")
  defer release(b)
  work(a, b)
end
```

`work` runs, then `release(b)`, then `release(a)`.

## Defer inside regions

`defer` runs when its enclosing **block** ends. Inside a region, that means just
before the region's memory is released, so deferred code can still use the
region's data:

```cera
main :: fn() do
  region do
    log = open_log("run.txt")
    defer flush_and_close(log)
    entries = collect_entries()
    write_all(log, entries)
  end
end
```

`flush_and_close` runs first (and can still read `entries`), then the region's
memory goes away.

## Defer is for things, not memory

If all you allocated was memory, put it in a region and you are done. Use
`defer` when you hold something the operating system or hardware knows about.
This keeps the model simple: regions for memory, `defer` for everything else.

## Recap

- No destructors. `defer expr` runs `expr` when the scope ends.
- Runs on normal exit and on early exit through `?`.
- Multiple defers run last-in, first-out.
- Memory belongs to regions; other resources belong to `defer`.

## Exercises

`defer1`.
