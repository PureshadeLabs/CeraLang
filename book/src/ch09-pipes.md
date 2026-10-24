# Pipes

> **Status:** syntax implemented. The desugaring below is the language
> definition; the parser keeps `|>` as written for tooling.

The pipe operator `|>` feeds the value on its left into the function on its
right. It turns nested calls inside-out so that code reads in the order things
happen.

## The rule

```text
x |> f          means   f(x)
x |> f(a, b)    means   f(x, a, b)
```

The piped value becomes the **first argument**. That is why Cera standard
library functions take their "subject" first.

Compare nested calls with a pipeline:

```cera
main :: fn() do
  # inside-out: read from the middle
  a = sum(map(filter(numbers, is_even), square))

  # left to right: read in order
  b = numbers
    |> filter(is_even)
    |> map(square)
    |> sum
end
```

## Layout

A line that starts with `|>` continues the line before it. The usual style
puts the first value on its own line and indents each step:

```cera
import std.io

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

You can also write short pipelines on one line:

```cera
main :: fn() do
  "hello" |> io.println
end
```

## Modules and pipes

`io.println` is an ordinary function in the `io` module, so it can sit on the
right of a pipe: `"hi" |> io.println` is `io.println("hi")`. Elixir-style
module functions pipe naturally:

```cera
main :: fn() do
  words = "the quick brown fox"
    |> String.split(" ")
    |> map(String.length)
    |> sum
end
```

## Passing extra arguments

The extra arguments go after the piped one:

```cera
main :: fn() do
  # the same as clamp(value, 0, 255)
  c = value |> clamp(0, 255)

  # the same as List.take(items, 3)
  first_three = items |> List.take(3)
end
```

If you need the piped value somewhere other than first, wrap the step in a
lambda and call it. The call `(lambda)()` receives the piped value as its first
(and only) argument:

```cera
main :: fn() do
  y = x |> (fn(v) -> pow(2, v) end)()
end
```

That says `pow(2, x)`. For readability, prefer to reorder a function's
parameters or to give the step a name.

## Pipes and precedence

`|>` has the lowest precedence of any operator, so each side of it is a complete
operator expression:

```cera
main :: fn() do
  # arithmetic happens first, then the pipe: double(1 + 2)
  a = 1 + 2 |> double

  # steps chain left to right: length(xs) is piped into is_even
  b = xs |> length |> is_even
end
```

When in doubt, add parentheses.

## Pipes with `?`

Pipelines combine with the early-exit operator from
[chapter 11](ch11-errors.md). A step can end in `?`, as long as the step is a
call with parentheses so that the `?` has something to attach to:

```cera
load_config :: fn(path: String) -> Result<Config, Error> do
  text = path |> fs.read_to_string()?
  text |> parse_config
end
```

`path |> fs.read_to_string()?` means `fs.read_to_string(path)?`.

## Recap

- `x |> f(a)` is `f(x, a)`.
- A line beginning with `|>` continues the previous line.
- `|>` binds loosest of all operators.
- Library functions put the subject first so that pipes work.

## Exercises

`pipes1`.
