# Appendix B: Operators

From tightest binding to loosest. Binary operators are left-associative.

| Level | Operators                      | Notes                              |
| ----- | ------------------------------ | ---------------------------------- |
| 1     | `.field` `f(args)` `x[i]` `?`  | postfix                            |
| 2     | `-` (negate), `not`, `~`       | prefix (`~` is bitwise not)        |
| 3     | `*` `/` `%`                    | multiplicative                     |
| 4     | `+` `-`                        | additive                           |
| 5     | `<<` `>>`                      | shifts; the count is a `u32`       |
| 6     | `&`                            | bitwise and                        |
| 7     | `^`                            | bitwise xor                        |
| 8     | `<` `<=` `>` `>=`              | comparison                         |
| 9     | `==` `!=`                      | equality                           |
| 10    | `and`                          | short-circuit                      |
| 11    | `or`                           | short-circuit                      |
| 12    | `\|>`                          | pipe; `x \|> f(y)` means `f(x, y)` |

Bitwise or has no operator because `|` separates variants and list heads; use `bits.bor`.
The bit operators work on integers only. `x[i]` indexes lists and arrays, and stops the
program with an error when `i` is out of range.

## Other punctuation

| Token | Meaning                                              |
| ----- | ---------------------------------------------------- |
| `::`  | declaration: `name :: ...`                           |
| `->`  | result type; match arm; lambda body                  |
| `=`   | binding (`var x = ...` for a mutable one)            |
| `:=`  | assignment to a `var`                                |
| `:`   | parameter and field types; record literal fields     |
| `\|`  | variant separator; list head/tail                    |
| `#`   | comment to end of line                               |
| `_`   | wildcard pattern                                     |

## Line continuation

A newline does not end a statement when:

- the line ends with a binary operator, `,`, `(`, `[`, `{`, `->`, `=`, `:=`, `::`,
  `:`, `.`, `and`, `or`, `not` or `|>`;
- the next line starts with `|>`;
- the newline is inside `(...)`, `[...]`, `{...}` or `<...>`.
