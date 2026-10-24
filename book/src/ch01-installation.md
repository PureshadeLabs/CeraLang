# Installation and tooling

> **Status:** `cerac check`, `types`, `parse`, `lex`, `fmt`, `cera-lsp`,
> `ceralings` and `kote` work today. `cerac run` runs programs with a bytecode
> VM (and a tree-walking reference interpreter with `--tree`), and `cerac build`
> compiles to a native x86-64 Linux executable.

Cera's first compiler (called *stage0*) is written in Rust. You build it from
source.

## Prerequisites

- A recent stable Rust toolchain (`rustup` is the easiest way to get one)
- `git`

## Building from source

```text
$ git clone <your Cera repository>
$ cd cera
$ cargo build --release
```

This produces four programs in `target/release/`:

| Program     | What it does                                              |
| ----------- | --------------------------------------------------------- |
| `cerac`     | The compiler driver                                       |
| `kote`      | The package manager and build tool ([chapter 20](ch20-kote.md)) |
| `cera-lsp`  | The language server, for editor integration               |
| `ceralings` | The exercise runner                                       |

Put them on your `PATH`, for example with
`cargo install --path crates/cerac` (and likewise for the others).

Check that it worked:

```text
$ cerac version
cerac 0.0.1
```

## What `cerac` can do today

```text
$ cerac check examples/hello.cera
examples/hello.cera: ok

$ cerac run examples/hello.cera       # run on the bytecode VM
Hello, world!
$ cerac run examples/wordcount.cera -- extra args   # arguments reach `env.args`
$ cerac dis examples/hello.cera       # print the bytecode
$ cerac lex examples/hello.cera       # print tokens
$ cerac parse examples/hello.cera     # print the syntax tree
$ cerac fmt examples/hello.cera       # format the file in place
$ cerac build examples/hello.cera -o hello   # native executable
```

When your code has a syntax error you get a message with a pointer to the
problem:

```text
error: expected `)`, found identifier `io`
 --> examples/broken.cera:3:3
  |
3 |   io.println(x)
  |   ^^

1 error in examples/broken.cera
```

`cerac` exits with status 0 on success and 1 when it reports errors, so it works
in scripts and CI.

## Editor support

`cera-lsp` speaks the Language Server Protocol over standard input and output.
Today it provides:

- **Diagnostics** as you type: syntax errors, and type and name errors once the file parses
- **Document symbols** (outline of types, variants and functions)
- **Hover** on keywords, on names declared in the file, and the inferred type of any expression or local binding
- **Go to definition** within a file, and into other modules (`geometry.area`, or the module name itself)
- **Completion** of keywords, snippets and names declared in the file; after `alias.` the public members of that module; after `import ` the modules you can import

Inside a `kote` package the server checks the file together with the modules it
imports, uses unsaved buffers of the other open files, and reports errors (such
as using a private item) on the file they belong to. Rename and find-references
are not implemented yet.

### Neovim

```lua
vim.filetype.add({ extension = { cera = "cera" } })

vim.api.nvim_create_autocmd("FileType", {
  pattern = "cera",
  callback = function()
    vim.lsp.start({
      name = "cera-lsp",
      cmd = { "cera-lsp" },
      root_dir = vim.fs.root(0, { ".git" }),
    })
  end,
})
```

### Helix

Add this to `languages.toml`:

```toml
[[language]]
name = "cera"
scope = "source.cera"
file-types = ["cera"]
comment-token = "#"
indent = { tab-width = 2, unit = "  " }
language-servers = ["cera-lsp"]

[language-server.cera-lsp]
command = "cera-lsp"
```

### VS Code and others

Any editor that can launch a language server over stdio works: point it at the
`cera-lsp` executable for files ending in `.cera`. A packaged VS Code extension
with syntax highlighting is on the roadmap.

## Practice with ceralings

```text
$ ceralings list      # show all exercises and your progress
$ ceralings run       # check the next unfinished exercise
$ ceralings hint intro1
```

You will meet `ceralings` at the end of each chapter. At this stage of the
project it checks that your fixed program parses and type-checks; when an
exercise has `# expect:` lines it also runs the program and compares its output.

## File conventions

- Source files end in `.cera`.
- Indentation is two spaces by convention. The compiler does not care.
- Comments start with `#` and run to the end of the line.
