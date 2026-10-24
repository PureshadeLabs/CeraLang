# Appendix A: Keywords

These words are reserved and cannot be used as names.

| Keyword   | Used for                                                       | Chapter |
| --------- | -------------------------------------------------------------- | ------- |
| `import`  | bring a module into scope                                      | 10      |
| `as`      | rename an import                                               | 10      |
| `fn`      | function declarations, function types, lambdas                 | 4       |
| `type`    | variant and record type declarations                           | 6, 7    |
| `do`      | open a block                                                   | 2       |
| `end`     | close a block, a `type` declaration or a lambda                | 2       |
| `if`      | conditional expression; guard in a `match` arm                 | 5, 8    |
| `else`    | alternative branch of `if` (and `else if`)                     | 5       |
| `match`   | pattern matching                                               | 5, 8    |
| `region`  | scope that owns memory                                         | 12      |
| `with`    | swap the context allocator for a scope                         | 13      |
| `defer`   | run an expression when the scope ends                          | 14      |
| `not`     | boolean negation                                               | 3       |
| `and`     | boolean conjunction                                            | 3       |
| `or`      | boolean disjunction                                            | 3       |
| `true`    | boolean literal                                                | 3       |
| `false`   | boolean literal                                                | 3       |
| `ref`     | reference type `ref T`                                         | 15      |
| `while`   | loop while a condition holds                                   | 5       |
| `for`     | loop over a list (`for x in xs do`); also `impl T for U`       | 5, 16   |
| `in`      | separates the variable and the list in `for`                   | 5       |
| `break`   | leave the nearest loop                                         | 5       |
| `continue`| skip to the next loop iteration                                | 5       |

`var` and `pub` are special only at the start of a statement or declaration
(`var x = 0`, `pub name :: ...`); they are not reserved, so `env.var` still works.

## Reserved for the future

These are not keywords today but are expected to become keywords, so avoid them
as names: `trait`, `impl`, `unsafe`, `repr`, `where`.

## Not keywords

`promote`, `new_in`, `allocator`, `context_allocator`, `Some`, `None`, `Ok`,
`Err` are ordinary names provided by the compiler or the standard library, not
reserved words.
