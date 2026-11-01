# Regions

> **Status:** native builds rewind the heap at the `end` of a `region` when it is
> safe to (below). The region checker enforces only the escape rule for a
> region's value (chapter 15); cross-region mutation is designed but not checked.

This chapter is the center of Cera. If you understand regions, you understand
how Cera manages memory.

### What the native compiler does today

A `region do ... end` frees everything its body allocated when all of these hold:

- the value of the region is a number, `bool`, `char`, `()`, a tuple of those, or a `String`
  (a string is copied down to the start of the region);
- the body assigns only to `var`s declared inside it;
- the body uses no `unsafe` code and calls no `std.array` mutator;
- the body does not use an array or a function declared outside it.

Anything else leaves the region as a plain block and the memory is not freed. Raw memory
from `mem.alloc` inside a freeing region is released with it. Buffered standard input
survives. `arena_new` and `new_in` are still accepted but allocate normally.

## The problem

Programs allocate memory: lists, strings, trees, closures. Something has to
free it. The usual answers each have a cost:

| Approach               | Cost                                                    |
| ---------------------- | ------------------------------------------------------- |
| Manual `free`          | Easy to forget (leaks) or to do twice (crashes)         |
| Garbage collector      | Runtime pauses and a runtime you can't always have      |
| Ownership and borrowing| Safe and fast, but you must learn to satisfy the checker|

Cera takes a fourth route, **regions**, which is as simple as a scratchpad.

## The idea

A **region** is a scope that owns memory. You open one with `region do ... end`.
Everything allocated inside is placed in that region. When execution reaches the
`end`, the whole region is released **at once**: no per-object bookkeeping, no
free list, nothing to forget.

```cera
main :: fn() do
  region do
    numbers = [1, 2, 3, 4, 5]
    doubled = numbers |> map(fn(n) -> n * 2 end)
    total = doubled |> sum
  end
  # numbers and doubled are gone, all memory is released
end
```

Think of it as a whiteboard you erase in one swipe.

## Why that works so well

Most programs have a natural *shape* in time:

- A server handles a request, then forgets it.
- A game builds a frame, draws it, then forgets it.
- A compiler runs a phase, hands over its result, then forgets the phase's
  scratch data.
- A kernel handles an event, then forgets it.

A region per request, frame, phase or event frees everything that scope
allocated in a single step. Freeing is constant time and cannot be forgotten.

## Regions nest

```cera
main :: fn() do
  region do
    config = load_config()
    region do
      request = read_request()
      respond(request, config)
    end
    # request is gone, config is still here
  end
end
```

The inner region is shorter-lived than the outer one. Values in the **outer**
region outlive values in the **inner** one.

## The one rule

> **A reference from an outer region to a value in an inner region is a compile
> error.**

Why? When the inner region ends, its values vanish. An outer value that still
pointed at one would be a dangling pointer. The compiler rules that out:

```cera,compile_fail
main :: fn() do
  region do
    holder = make_holder()
    region do
      temp = [1, 2, 3]
      set_field(holder, temp)
    end
    use(holder)
  end
end
```

```text
error: value escapes region
  |   set_field(holder, temp)
  |                     ^^^^ `temp` lives in an inner region
  = help: use `promote(temp)` to move it to the outer region
```

(Planned wording; the checker is not written yet.) This is the only memory
safety rule you must keep in your head. There are no lifetime annotations to
write, no moves to track, no borrow states.

## What is *in* a region?

Anything that allocates: list and record construction, strings built at run
time, closures that capture variables. Values that need no allocation (integers,
floats, booleans, string literals) cost nothing and belong to no region.

## How the compiler knows

Each reference type carries an invisible **region variable**. Inside a function
body the compiler infers regions for you. For function signatures, it makes
functions **region-polymorphic by default**, so a function like `length` works
on a list from any region without you saying so.

For version 0 the design is intentionally modest: regions are lexical (they are
exactly the `region do ... end` blocks), and inference stays inside one
function. Cleverer whole-program inference is possible later but is not a goal
for v0, because predictability matters more than cleverness here.

## Returning data from a region

A region's last expression is its value, and that value normally needs to
survive the `end`. Getting values out is the subject of
[chapter 15](ch15-escaping.md). In short: you place the result in the parent
region on purpose, with `promote` or `new_in`, so the cost is visible.

## Long-lived data

Regions suit data with a clear end. Some data has no natural end: a cache, an
in-memory database, the state of a long-running service. For those you use a
**long-lived allocator** with explicit freeing, or a **pool** of fixed-size
objects. [Chapter 13](ch13-allocators.md) shows how.

A good rule of thumb: put short-lived, bursty allocations in regions, and give
long-lived things an owner and an allocator.

## Recap

- `region do ... end` owns the memory allocated inside it and frees it all at
  `end`.
- Regions nest; inner regions are shorter-lived.
- One rule: outer data must not point into inner regions.
- Escaping is explicit (chapter 15). Long-lived data uses allocators
  (chapter 13).

## Exercises

`regions1`, `regions2`.
