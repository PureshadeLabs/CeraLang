# Values and bindings

> **Status:** implemented: syntax, type checking and evaluation (`cerac check`,
> `cerac types`, `cerac run`).

## Bindings

A **binding** gives a name to a value:

```cera
main :: fn() do
  answer = 42
  pi = 3.14159
  ready = true
  greeting = "hello"
  nothing = ()
end
```

There is no `let`. `name = expression` on its own line is a binding (`var name =
expression` makes one you can assign to later). Bindings are **immutable**: once `answer` is 42, it stays 42. (When you do need a
value that changes, such as a loop counter, declare it with `var`; see
[chapter 5](ch05-control-flow.md).)

You may reuse a name. The new binding **shadows** the old one from that point
on; it does not change it:

```cera
main :: fn() do
  x = 1
  x = x + 1   # a new x, whose value is 2
end
```

Types are inferred. You rarely write them on bindings. You write them on
function parameters and results, where they document your intent.

> **Provisional:** a literal such as `42` becomes whatever integer type its
> use requires, defaulting to `i32`; `3.14` defaults to `f64`. A way to
> annotate a binding directly (something like `x: u8 = 5`) is an open question,
> listed in [Appendix D](appendix-d-design.md).

## Constants

At the top level, `::` declares a constant:

```cera
MAX_USERS :: 1_000
GREETING :: "hello"
PI :: 3.14159
```

Underscores in numbers are ignored; use them to group digits.

## Primitive types

| Type                        | Meaning                                 |
| --------------------------- | --------------------------------------- |
| `i8` `i16` `i32` `i64`      | signed integers                         |
| `u8` `u16` `u32` `u64`      | unsigned integers                       |
| `isize` `usize`             | pointer-sized integers                  |
| `f32` `f64`                 | floating-point numbers                  |
| `bool`                      | `true` or `false`                       |
| `char`                      | a Unicode scalar value                  |
| `String`                    | UTF-8 text                              |
| `()`                        | unit, the "no interesting value" value  |

Integer arithmetic overflow is never silent. The `core.num` module offers
`checked_`, `wrapping_` and `saturating_` variants so you choose the behavior
explicitly.

## Number literals

```cera
main :: fn() do
  decimal = 255
  hex = 0xFF
  grouped = 1_000_000
  fraction = 2.5
  scientific = 1e6
  small = 3.0e-4
end
```

## Strings

String literals are written in double quotes. They may contain newlines and
these escapes:

| Escape | Meaning         |
| ------ | --------------- |
| `\n`   | newline         |
| `\t`   | tab             |
| `\r`   | carriage return |
| `\0`   | null byte       |
| `\e`   | escape (27)     |
| `\x41` | a byte `\x00` to `\x7f` |
| `\u{e9}` | a Unicode character, 1 to 6 hex digits |
| `\\`   | backslash       |
| `\"`   | double quote    |

```cera
main :: fn() do
  a = "line one\nline two"
  b = "she said \"hi\""
  c = "multi
line"
end
```

## Operators

Arithmetic: `+ - * / %`. Comparison: `== != < <= > >=`. Logic: `and`, `or`,
`not`. Precedence, from tightest to loosest:

| Level | Operators                     |
| ----- | ----------------------------- |
| 1     | postfix: `.` `()` `[]` `?`    |
| 2     | unary `-`, `not`              |
| 3     | `*` `/` `%`                   |
| 4     | `+` `-`                       |
| 5     | `<` `<=` `>` `>=`             |
| 6     | `==` `!=`                     |
| 7     | `and`                         |
| 8     | `or`                          |
| 9     | `\|>` (loosest)               |

The full table is in [Appendix B](appendix-b-operators.md).

```cera
main :: fn() do
  a = 1 + 2 * 3          # 7
  b = (1 + 2) * 3        # 9
  c = 4 > 3 and 2 > 1    # true
  d = not (1 == 2)       # true
end
```

Logical `and` and `or` stop early: the right side is only evaluated when needed.

## Lines and continuation

A newline normally ends a statement. A statement carries on to the next line
when the line ends with an operator, a comma, an opening bracket, `->`, `=`, or
when the next line starts with `|>`. Inside `(...)` and `[...]` newlines are
ignored altogether:

```cera
main :: fn() do
  total = 1 +
    2 +
    3
  items = [
    1,
    2,
    3,
  ]
end
```

## Recap

- `name = value` binds; bindings are immutable and can be shadowed.
- `NAME :: value` declares a top-level constant.
- Types are inferred; primitives are `i32`, `f64`, `bool`, `String` and friends.
- Newlines end statements, with a few continuation rules.

## Exercises

`values1`, `values2`.
