# Packages and `kote`

`kote` is Cera's package manager and build tool, in the role `cargo` plays for
Rust. It reads a manifest, fetches dependencies, links every module a program
imports into one program, and builds it with the native backend (or runs it on
the VM when the native backend cannot handle something yet).

## A package

A package is a directory with a `kote.toml` and a `src/` folder.

```text
hello/
  kote.toml
  kote.lock        (written by kote)
  src/
    main.cera      the entry point of a program
    lib.cera       the public face of a library
    util.cera      another module: `import util`
    net/
      mod.cera     module `net`   (or src/net.cera)
      http.cera    module `net.http`
  tests/
    basic.cera     tests: `test_*` functions
```

```text
[package]
name = "hello"
version = "0.1.0"

[dependencies]
geometry = { path = "../geometry" }
fmt = { git = "https://example.com/fmt", tag = "v1.2.0" }
```

A package name uses lower-case letters, digits and `_`, and starts with a
letter. The names `std`, `kote`, `main`, `lib` and `tests` are reserved.

## Commands

```text
kote new <name> [--lib]     create a package in a new directory
kote init [--lib]           create a package in the current directory
kote check                  type-check the package and its dependencies
kote build                  compile to target/bin/<name>
kote run [-- args...]       build and run
kote test [filter]          run tests/*.cera
kote fmt [--check]          format src/ and tests/
kote doc                    write HTML docs for `pub` items to target/doc/
kote bench [filter]         run benches/*.cera natively and report time per call
kote add <name>[@req]       add a registry dependency (newest version if no @req)
kote add <name> --path <dir>
kote add <name> --git <url> [--rev <r> | --tag <t> | --branch <b>]
kote remove <name>
kote update [name]          re-resolve git and registry dependencies
kote publish [--registry <dir|url>]   copy the package into the registry
kote yank <name>@<version> [--undo]   hide a version from new resolutions
kote tree                   show the dependency tree
kote clean                  delete target/
```

`--locked` fails if `kote.lock` would change; `--offline` never uses the
network. Commands work from any subdirectory of a package.

## Modules

Every `.cera` file is a module. An import names it by its path inside the
package, and the module is reached through the last name or an alias.

```cera
import std.io
import geometry
import geometry.util as u

main :: fn() do
  s = geometry.Rect(3, 4)
  io.println(geometry.area(s))
  io.println(u.double(21))
end
```

An import resolves in this order:

1. `std.*` is the standard library.
2. A name equal to a dependency (or to the package itself) is that package's
   `src/lib.cera`; `dep.sub` is `src/sub.cera` inside it.
3. Otherwise it is a module of the current package.

A top-level declaration is private to its module unless it is written with
`pub`. Other modules can only name public items; trying anything else is an
error that points at the definition:

```cera
# geometry/lib.cera
pub Shape :: type
  | Circle(r: i32)
  | Rect(w: i32, h: i32)
end

pub area :: fn(s: Shape) -> i32 do
  helper(s)
end

helper :: fn(s: Shape) -> i32 do
  1
end
```

`pub` goes before the name of a function, type, trait or constant. A public
type makes its constructors public, and a public trait makes its methods
public. `impl` blocks and their methods take no `pub`; record fields are always
visible. Qualify types, constructors and traits the same way as functions, in
every position:

```cera
Wrapper :: type { inner: geometry.Point }

describe_it :: fn(s: geometry.Shape) -> i32 do
  match s do
    geometry.Circle(r) -> r
    geometry.Rect(w, h) -> w + h
  end
end

main :: fn() do
  p = geometry.Point { x: 1, y: 2 }
  w = Wrapper { inner: p }
end
```

`impl geometry.Describe for geometry.Point do ... end` implements a trait from
another module. A local variable that shares a name with an imported module
shadows it, as in any other scope.

Imports may not form a cycle. `kote` reports the chain and suggests moving the
shared definitions into a third module. The entry module `main` cannot be
imported.

## Dependencies and the lock file

A path dependency is a directory with its own `kote.toml`; the dependency's
`name` must match the key under `[dependencies]`. A git dependency is cloned
into `~/.kote` (override with `KOTE_HOME`) and checked out at one commit.

`kote.lock` records every package and, for git dependencies, the exact commit:

```text
[[package]]
name = "fmt"
version = "1.2.0"
source = "git+https://example.com/fmt?tag=v1.2.0#3f9c...e1"
```

As long as the manifest still asks for the same tag, revision or branch, `kote`
builds the locked commit. `kote update` follows branches forward again. Commit
`kote.lock` for programs; libraries may leave it out.

## The registry and versions

A registry is a directory with one folder per published package version:
`<registry>/<name>/<version>/` holds `kote.toml` and `src/`. The default is
`~/.kote/registry`; set `KOTE_REGISTRY` or write `registry = "../dir"` in a
dependency to use another one (share the directory to share the packages).

```text
$ kote publish                  # from the library's directory
   Published mathx v1.1.0 to /home/me/.kote/registry/mathx/1.1.0
```

Versions are `MAJOR.MINOR.PATCH` (no pre-release suffixes) and a published
version is immutable. Published packages may depend only on other registry
packages. Depend on one with a version requirement:

```toml
[dependencies]
mathx = "^1.0"
text = { version = ">=2.1, <3", registry = "../shared" }
```

| Requirement | Means |
| ----------- | ----- |
| `^1.2.3` or `1.2.3` | `>=1.2.3, <2.0.0` (`^0.2.3` is `<0.3.0`, `^0.0.3` is exactly `0.0.3`) |
| `~1.2.3` | `>=1.2.3, <1.3.0` |
| `=1.2.3` | exactly that version |
| `>=1.0, <2` | all comparators must hold (`>`, `>=`, `<`, `<=`) |

Versions fall into *series*: all of `1.x` is one series, and each `0.y` is its
own. `kote` gives each series of a package the highest version that satisfies
every requirement on that series, or the version in `kote.lock` when that still
satisfies them. If no version does, the error lists who asked for what and which
versions exist. `kote update` moves locked packages forward within their
requirements.

Requirements in different series do not conflict: if `mid` needs `base ^1.0` and
your app needs `base ^2`, both copies are linked, as `base@1` and `base@2`, and
each package sees the one it asked for. Their types are distinct types, so a
`base@1` value cannot be passed where `base@2` is expected. `kote.lock` lists
both, and `kote tree` shows both.

### Yanking

```text
$ kote yank mathx@1.1.0            # a bad release
$ kote yank mathx@1.1.0 --undo
```

A yanked version stays downloadable, and projects whose `kote.lock` already
names it keep building. New resolutions (no lock, `kote update`, `kote add`)
skip it. Published versions are never deleted.

### HTTP registries

A registry can be a server. Give its URL wherever a directory is accepted:

```toml
mathx = { version = "^1.0", registry = "https://packages.example.org" }
```

```text
$ KOTE_TOKEN=... kote publish --registry https://packages.example.org
```

`kote` uses `curl` and `tar` for transfers, mirrors the packages it needs into
`~/.kote/registry-cache`, and uses that cache when the server is unreachable or
with `--offline`. Archives are checked before extraction: they may contain only
`kote.toml` and `src/`. `KOTE_TOKEN`, when set, is sent as a bearer token.

A server implements five requests, with `<base>` the registry URL:

| Request | Meaning |
| ------- | ------- |
| `GET <base>/<name>/index` | one line per version, `1.2.0` or `1.2.0 yanked`; `404` if unknown |
| `GET <base>/<name>/<version>.tar` | the archive of `kote.toml` and `src/` |
| `PUT <base>/<name>/<version>.tar` | publish; `201` created, `409` if the version exists |
| `PUT <base>/<name>/<version>/yank` | yank |
| `DELETE <base>/<name>/<version>/yank` | un-yank |

There are no checksums in the index yet; use `https` so the transfer itself is
protected.

## Tests

A file in `tests/` is a module that can import the package's modules and its
dependencies. Functions named `test_*` with no parameters are tests. A test
passes when it returns `Ok(())` (or returns nothing and does not fail), and
fails when it returns `Err(message)` or hits a runtime error. `import kote.test`
provides the helpers:

```cera
import kote.test as t
import geometry

test_area :: fn() -> Result<(), String> do
  t.equal(12, geometry.area(geometry.Rect(3, 4)))
end
```

`t.equal`, `t.not_equal`, `t.check(condition, message)`, `t.pass` and `t.fail`
all return `Result<(), String>`, so `?` stops a test at the first failure.
`kote test area` runs only tests whose name contains `area`; each test runs in
its own process.

## Documentation

Write `##` lines directly above a `pub` declaration to document it. A `##`
block at the top of a file, followed by a blank line, documents the module.
Comments support paragraphs, `-` lists, `code` spans and fenced code blocks.

```cera
## Geometry helpers.

## Area of a shape, in square units. A `Circle` uses `r * r`.
pub area :: fn(s: Shape) -> i32 do
  match s do
    Circle(r) -> r * r
  end
end
```

`kote doc` writes `target/doc/<package>/index.html` and a page per module,
listing only public items. Private items never appear.

## Benchmarks

Put zero-argument functions named `bench_*` in `benches/*.cera`. `kote bench`
builds them natively, repeats each call until a run takes at least 100 ms and
then takes five timed runs:

```text
bench fib.bench_fib_20        49.56 us/iter  (min 43.46 us, max 50.78 us, 4096 iterations x 5 samples)
```

Benchmarks need the native backend (the VM's timings say little about compiled
code) and run each in its own process. Use `kote bench fib` to filter by name.

## Rebuilds

`kote build`, `run`, `test` and `bench` keep a fingerprint of the linked
program, the text of every source file and the compiler next to the executable
(`target/bin/<name>.key`). When nothing changed they print `Fresh` and skip code
generation.

## Formatting

`kote fmt` rewrites every `.cera` file under `src/` and `tests/` in the
canonical style; `kote fmt --check` changes nothing and fails if a file would
change (useful in CI). `cerac fmt FILE...` does the same for single files, and
`cerac fmt -` filters standard input to standard output.

The style is fixed: two-space indentation, lines of at most 100 columns,
one blank line between declarations, and parentheses only where precedence
needs them. Lists, argument lists and records that do not fit are broken one
element per line with a trailing comma. A multi-line `if`, lambda or pipeline
stays multi-line, and one that fits on a line stays on one line. Comments stay
with the line they were on. A comment in the middle of an expression cannot be
placed safely, so the formatter refuses the file and says where; move the
comment onto its own line. The formatter never changes what a program means,
and formatting twice gives the same text.

## Errors

Diagnostics name the file they come from, and use the names you wrote
(`geometry.Shape`, not the internal one). Runtime errors in the native
executable point into the source file too.

## Limits

- HTTP registries have no checksums or signatures in the index, and `kote`
  ships no registry server (the protocol above is small enough to write one).
- Within one series a package appears once, so two requirements on it that no
  version meets together are an error, and two different packages with the same
  name are an error.
- Visibility is per declaration (`pub` or private); there is no `pub(package)` and no re-export of imports.
- There is one build profile, and rebuilds are all-or-nothing: `kote` reuses the
  executable when nothing changed, and otherwise recompiles the whole program.
- Test files may only use `test_*` functions with no parameters.
