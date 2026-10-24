# Allocators and `with`

> **Status:** `with` syntax implemented. The `Allocator` interface and the
> allocator library are designed; names are provisional.

A region is one kind of **allocator**. Cera lets you choose where memory comes
from at every level, which matters when you write kernels, drivers, games and
anything with tight memory limits.

## The context allocator

Every function call carries an invisible **context allocator**, the place where
that code's allocations go by default. This idea comes from Odin. It means
library code does not have to take an allocator parameter at every call, yet
callers still control where memory comes from.

- At program start the context allocator is the **root allocator**, backed by the
  operating system (on Lythos, by a memory capability).
- Inside `region do ... end` it is that region.

So when you write:

```cera
main :: fn() do
  region do
    xs = [1, 2, 3]
  end
end
```

the list `[1, 2, 3]` is allocated in the context allocator, which is the
region.

## Swapping the allocator: `with`

`with allocator(a) do ... end` replaces the context allocator for the block:

```cera
main :: fn() do
  scratch = arena_new(4096)
  with allocator(scratch) do
    # everything here allocates from scratch
    parts = ["a", "b", "c"] |> join("-")
  end
end
```

Code inside the block, including every function it calls, uses `scratch`.

## The `Allocator` interface

An allocator is anything that can do three things:

| Operation | Meaning                                             |
| --------- | --------------------------------------------------- |
| `alloc`   | get a block of memory of a given size and alignment |
| `resize`  | grow or shrink a block, possibly moving it          |
| `free`    | give a block back                                   |

A region is an allocator whose `free` does nothing and whose *end of scope*
releases everything. The standard library provides several more:

| Allocator | Behaves like                                         | Typical use                      |
| --------- | ---------------------------------------------------- | -------------------------------- |
| `region`  | a scope-bound arena                                  | request, frame, phase            |
| `arena`   | an arena you create and destroy yourself             | scratch work with a custom size  |
| `bump`    | a fixed buffer you fill front to back                | stack-like temporary memory      |
| `pool`    | fixed-size slots with fast alloc and free            | many objects of one size         |
| `page`    | whole pages from the OS or kernel                    | building other allocators        |

## Explicit variants: `_in`

Every standard-library function that allocates uses the context allocator by
default and comes with a variant ending in `_in` that takes the allocator as its
first argument:

```cera
main :: fn() do
  a = List.map(xs, f)          # context allocator
  b = List.map_in(arena, xs, f)  # a specific allocator
end
```

Use `_in` when the result should live somewhere other than the current context,
for example when building something that must outlive the current region.

## Allocating nothing

Because allocation is explicit in how it is spelled, you can see where it
cannot happen. The `core` layer of the standard library has **no context
allocator at all**, so calling anything that allocates from code built against
`core` alone is a compile error. That is how kernel code and interrupt handlers
stay allocation-free.

## Who owns an allocator?

An allocator you create (`arena_new(...)`) is a value like any other, and it
lives somewhere: in a region, or in a long-lived owner. It is freed with
`arena_free`, or `defer`:

```cera
main :: fn() do
  scratch = arena_new(1_048_576)
  defer arena_free(scratch)
  with allocator(scratch) do
    work()
  end
end
```

## Recap

- Every call has a context allocator; `region` and `with allocator(a)` change it.
- An allocator provides `alloc`, `resize` and `free`.
- Library functions allocate from the context and have `_in(alloc)` versions.
- `core` has no allocator, so it cannot allocate.
