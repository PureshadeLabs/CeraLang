# Appendix C: Grammar

The grammar the parser implements today, in EBNF. `{ x }` means zero or more,
`[ x ]` means optional, quoted text is a token. A `NEWLINE` ends a statement or
a declaration; see [Appendix B](appendix-b-operators.md) for the continuation
rules.

```text
program     = { item } ;
item        = import | decl | impl ;

import      = "import" ident { "." ident } [ "as" ident ] ;

decl        = ident [ generics ] "::" ( fn_def | type_def | trait_def | expr ) ;
generics    = "<" ident { "," ident } ">" ;
bounded     = "<" bparam { "," bparam } ">" ;
bparam      = ident [ ":" ident { "+" ident } ] ;
trait_def   = "trait" { ident "::" "fn" "(" [ param { "," param } ] ")" [ "->" type ] } "end" ;
impl        = "impl" [ bounded ] ident "for" type "do" { decl } "end" ;

fn_def      = "fn" [ bounded ] "(" [ param { "," param } ] ")"
              [ "->" type ] "do" block "end" ;
param       = ident ":" type ;

type_def    = "type" ( variants "end" | record ) ;
variants    = variant { variant } ;
variant     = "|" ident [ "(" vfield { "," vfield } ")" ] ;
vfield      = [ ident ":" ] type ;
record      = "{" [ field { "," field } ] "}" ;
field       = ident ":" type ;

type        = ident { "." ident } [ "<" type { "," type } ">" ]
            | "fn" "(" [ type { "," type } ] ")" "->" type
            | "[" type "]"
            | "(" [ type { "," type } ] ")"
            | "ref" type ;

block       = { stmt } ;
stmt        = [ "var" ] ident "=" expr
            | ident ":=" expr
            | "defer" expr
            | expr ;

expr        = binary { "|>" binary } ;
binary      = unary { binop unary } ;          (* precedence climbing *)
binop       = "or" | "and" | "==" | "!=" | "<" | "<=" | ">" | ">="
            | "+" | "-" | "*" | "/" | "%" ;
unary       = ( "-" | "not" ) unary | postfix ;
postfix     = primary { call | "." ident | "[" expr "]" | "?" } ;
call        = "(" [ expr { "," expr } ] ")" ;

primary     = INT | FLOAT | STRING | "true" | "false"
            | ident
            | UPPER_IDENT "{" [ ident ":" expr { "," ident ":" expr } ] "}"
            | "(" ")"
            | "(" expr ")"
            | "(" expr "," [ expr { "," expr } ] ")"
            | "[" [ expr { "," expr } ] [ "|" expr ] "]"
            | lambda | if | match | region | with | loop
            | "break" | "continue" | "do" block "end" ;

loop        = "while" expr "do" block "end"
            | "for" ident "in" expr "do" block "end" ;

lambda      = "fn" "(" [ lparam { "," lparam } ] ")"
              ( "->" expr "end" | "do" block "end" ) ;
lparam      = ident [ ":" type ] ;

if          = "if" expr "do" block
              [ "else" ( if_tail | block ) ] "end" ;
(* `else if` continues the chain and shares the final "end" *)

match       = "match" expr "do" { arm } "end" ;
arm         = pattern [ "if" expr ] "->" expr NEWLINE ;

region      = "region" "do" block "end" ;
with        = "with" ident "(" expr ")" "do" block "end" ;

pattern     = "_"
            | lower_ident
            | [ "-" ] ( INT | FLOAT )
            | STRING | "true" | "false"
            | UPPER_IDENT [ "(" pattern { "," pattern } ")" ]
            | UPPER_IDENT "{" [ pfield { "," pfield } ] "}"
            | "(" pattern { "," pattern } ")"
            | "[" [ pattern { "," pattern } ] [ "|" pattern ] "]" ;
pfield      = ident [ ":" pattern ] ;
```

## Lexical grammar

```text
ident       = ( letter | "_" ) { letter | digit | "_" } ;
INT         = digit { digit | "_" } | "0x" hexdigit { hexdigit | "_" } ;
FLOAT       = digit { digit | "_" } "." digit { digit | "_" } [ exponent ]
            | digit { digit | "_" } exponent ;
exponent    = ( "e" | "E" ) [ "+" | "-" ] digit { digit } ;
STRING      = '"' { char | escape } '"' ;
escape      = "\n" | "\t" | "\r" | "\0" | "\\" | '\"' ;
comment     = "#" { any character except newline } ;
```

Letters are ASCII. A name beginning with an uppercase letter is a type or
variant name (`UPPER_IDENT`); one beginning with a lowercase letter or `_` is a
value name (`lower_ident`).

## Where this differs from the design draft

The first draft grammar in `docs/spec.md` was tightened while writing the
parser:

- Declarations and functions may carry type parameters (`List<T> :: type`,
  `fn<A, B>(...)`).
- Records are `type { ... }` with no trailing `end`. Variant types keep `end`.
- Record literals (`Point{x: 1.0, y: 2.0}`) and record patterns exist.
- Lists have a cons form: `[x | rest]`, for expressions as well as patterns.
- `do ... end` is usable as an expression; lambdas may have a `do` body.
- Address-of is spelled `ref_of(v)`, a function, rather than a unary `&`.
- Effects (`!alloc`) are left out of v0.
