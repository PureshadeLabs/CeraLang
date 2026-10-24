# Moving values out of regions

> **Status:** syntax implemented (`promote` and `new_in` are ordinary calls to
> compiler-known functions). Checking is designed, not written.

A region is a scratchpad, but sometimes the *answer* has to outlive it. This
chapter is about getting results out on purpose.

## What goes wrong

```cera,compile_fail
build :: fn() -> [i32] do
  region do
    temp = [3, 1, 2]
    temp |> sort
  end
end
```

The region's last expression, the sorted list, lives in that region. The region
ends, the list is freed, and `build` would return a dangling reference. The
compiler refuses:

```text
error: value escapes region
  |     temp |> sort
  |     ^^^^^^^^^^^^ this value lives in the region that ends here
  = help: use `promote(v)` or `new_in(parent, v)`
```

## `promote`

`promote(v)` **copies** `v` into the parent region and returns the copy. After
that, the region can end safely:

```cera
build :: fn() -> [i32] do
  region do
    temp = [3, 1, 2]
    sorted = temp |> sort
    promote(sorted)
  end
end
```

`promote` copies the whole structure it is given, so it costs time and memory
proportional to the size of the value. That is on purpose. The cost is **visible
in the source**: you can search a program for `promote` and see every place data
crosses a region boundary. The language never copies behind your back.

If the value is small, such as a number, `promote` does nothing at all.

## `new_in`: allocate in the right place from the start

When you already know a value will outlive the region, copying at the end is
wasted work. `new_in(allocator, v)` builds the value in a chosen allocator in
the first place:

```cera
build :: fn() -> [i32] do
  outer = context_allocator()
  region do
    temp = [3, 1, 2]
    sorted = temp |> sort
    new_in(outer, sorted)
  end
end
```

Here `outer` is the allocator that was current *before* the region began.
`context_allocator()` returns it, and the `_in` functions of the standard library
take any allocator, so you can often do even better by building the final result
there from the start:

```cera
build :: fn() -> [i32] do
  outer = context_allocator()
  region do
    temp = [3, 1, 2]
    temp |> List.sort_in(outer)
  end
end
```

The scratch work happens in the region, and only the final list lands in `outer`.

> **Provisional:** the names `context_allocator`, `promote` and `new_in` are
> the current plan. The behavior (explicit, visible copying) is the decision.

## Why explicit?

The alternative, copying automatically when a value leaves a region, looks
friendlier but has real problems for a systems language:

- It hides an O(n) cost in an innocent-looking `end`.
- It breaks for data that cannot be copied meaningfully: values with cycles,
  shared substructure, raw pointers, or handles to the operating system.
- It makes performance hard to predict, which is exactly what systems code needs
  to be able to do.

So the compiler tells you, and you decide: copy now with `promote`, or put it in
the right place from the start with `new_in`.

## Mutation across regions

Storing an inner-region value into an outer-region record or list would create
the forbidden outer-to-inner reference, so it is an error too. The fix is the
same: `promote` the value you are storing.

```cera
remember :: fn(cache: ref Cache, key: String, value: [i32]) do
  # `value` may live in a short-lived region; store a copy in the cache's region
  cache_put(cache, key, promote(value))
end
```

## A server loop

A classic use is one region per iteration of a long-running loop:

```cera
serve :: fn(listener: Listener, stats: Stats) do
  region do
    conn = accept(listener)
    request = read_request(conn)
    response = handle(request, stats)
    send(conn, response)
  end
  serve(listener, stats)
end
```

All of an iteration's temporary data, including the request and the response,
is freed at that iteration's `end`. Only `stats`, which was allocated before the
loop began, lives on.

## Recap

- A region's value cannot silently leave it; the compiler stops you.
- `promote(v)` copies `v` into the parent region (visible cost).
- `new_in(alloc, v)` builds directly in a chosen allocator (no copy).
- Use one region per iteration of a long-running loop to keep memory flat.
