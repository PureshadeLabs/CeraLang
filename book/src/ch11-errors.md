# Errors: Option, Result and `?`

> **Status:** syntax and type checking of `?` implemented, with `Option` and
> `Result` provided by the compiler's stub prelude; programs using them run under
> `cerac run`.

Cera has no exceptions. A function that can fail says so in its return type,
and the caller has to deal with it. Two ordinary variant types carry almost all
the weight.

## Option: a value that might be missing

```cera
Option<T> :: type
  | None
  | Some(T)
end
```

Use it when "nothing there" is a normal answer, like looking up a key:

```cera
first_even :: fn(xs: [i32]) -> Option<i32> do
  match xs do
    [] -> None
    [x | rest] -> if x % 2 == 0 do Some(x) else first_even(rest) end
  end
end

main :: fn() do
  message = match first_even([1, 3, 4, 5]) do
    Some(n) -> "found an even number"
    None -> "no even numbers"
  end
end
```

There is no `null`. A value that cannot be missing has a plain type; one that
can has `Option<T>`, and the compiler makes you handle both cases.

## Result: an operation that might fail

```cera
Result<T, E> :: type
  | Ok(T)
  | Err(E)
end
```

`Result` is for failures that carry information about *what* went wrong:

```cera
ParseError :: type
  | Empty
  | NotANumber(text: String)
  | OutOfRange(value: i64)
end

parse_age :: fn(text: String) -> Result<i32, ParseError> do
  if text == "" do
    Err(Empty)
  else
    match to_int(text) do
      None -> Err(NotANumber(text))
      Some(n) if n < 0 or n > 150 -> Err(OutOfRange(n))
      Some(n) -> Ok(n)
    end
  end
end
```

## The `?` operator

Handling every `Result` by hand with `match` gets noisy when several steps can
fail. The postfix `?` does the common thing: if the value is `Ok(x)` (or
`Some(x)`), the expression is `x`; if it is `Err(e)` (or `None`), the
**current function returns that failure immediately**.

```cera
add_ages :: fn(a: String, b: String) -> Result<i32, ParseError> do
  x = parse_age(a)?
  y = parse_age(b)?
  Ok(x + y)
end
```

That is equivalent to:

```cera
add_ages :: fn(a: String, b: String) -> Result<i32, ParseError> do
  match parse_age(a) do
    Err(e) -> Err(e)
    Ok(x) ->
      match parse_age(b) do
        Err(e) -> Err(e)
        Ok(y) -> Ok(x + y)
      end
  end
end
```

Rules for `?`:

- It only works inside a function whose result type can hold the failure.
- `?` on a `Result<_, E>` needs the function to return `Result<_, E2>` where
  `E` converts into `E2` (an `Into`-style conversion in the standard library).
- `?` on an `Option` needs the function to return an `Option`.
- `defer` blocks run when `?` leaves early, so cleanup still happens
  ([chapter 14](ch14-defer.md)).

`?` is the only early exit Cera has. It is not general control flow; it exists
precisely for the error path.

## Panics

Some failures are bugs, not conditions to handle: indexing out of range,
dividing by zero, a failed assertion. These abort the program with a message
(`panic`). Panics are not caught by ordinary code. They are for "this should be
impossible", never for "the file wasn't there".

## Recap

- No exceptions, no `null`. Use `Option<T>` and `Result<T, E>`.
- `match` handles each case; `?` forwards the failure.
- `?` is the one early exit; `defer` still runs.
- Panics are for bugs, not for expected errors.

## Exercises

`errors1`, `errors2`.
