use kote::driver::{self, AddArgs, Flags};
use std::process::ExitCode;

const USAGE: &str = "\
kote: the Cera package manager

USAGE:
    kote <COMMAND> [OPTIONS]

COMMANDS:
    new <NAME> [--lib]      Create a package in a new directory
    init [--lib]            Create a package in the current directory
    check                   Type-check the package and its dependencies
    build                   Compile the package to target/bin/<name>
                            (--target x86_64-lythos: target/lythos/bin/<name>)
    run [-- ARGS...]        Build and run the package
    test [FILTER]           Run the tests in tests/*.cera
    bench [FILTER]          Run the benchmarks in benches/*.cera (native only)
    doc                     Generate HTML docs for public items into target/doc
    fmt [--check]           Format src/ and tests/ (with --check: list files that would change)
    add <NAME>[@REQ] [--path DIR | --git URL [--rev R | --tag T | --branch B]] [--registry DIR|URL]
                            Add a dependency (without --path/--git: from the registry)
    publish [--registry DIR|URL]
                            Copy the package into the registry
    yank <name>@<version> [--undo] [--registry DIR|URL]
                            Stop new projects from choosing a published version
    remove <NAME>           Remove a dependency
    update [NAME]           Re-resolve git and registry dependencies and rewrite kote.lock
    tree                    Show the dependency tree
    clean                   Remove the target directory
    version                 Print the version
    help                    Print this message

OPTIONS (check, build, run, test, tree, update):
    --locked                Fail if kote.lock would change
    --offline               Do not use the network
";

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // Everything after `--` belongs to the program being run.
    let tail: Vec<String> = match args.iter().position(|a| a == "--") {
        Some(i) => {
            let t = args.split_off(i + 1);
            args.pop();
            t
        }
        None => vec![],
    };
    let mut flags = Flags::default();
    if let Some(i) = args.iter().position(|a| a == "--target") {
        let t = args.get(i + 1).cloned().unwrap_or_default();
        match t.as_str() {
            "x86_64-lythos" => flags.lythos = true,
            "x86_64-linux" => {}
            other => {
                eprintln!("error: unknown target `{other}` (expected x86_64-linux or x86_64-lythos)");
                return ExitCode::FAILURE;
            }
        }
        args.drain(i..(i + 2).min(args.len()));
        if flags.lythos && args.first().map(String::as_str) != Some("build") {
            eprintln!("error: --target x86_64-lythos is only supported by `kote build`");
            return ExitCode::FAILURE;
        }
    }
    args.retain(|a| match a.as_str() {
        "--locked" => {
            flags.locked = true;
            false
        }
        "--offline" => {
            flags.offline = true;
            false
        }
        _ => true,
    });
    let Some(cmd) = args.first().cloned() else {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    };
    let rest: Vec<String> = args[1..].to_vec();
    let opt = |name: &str| -> Option<String> { rest.iter().position(|a| a == name).and_then(|i| rest.get(i + 1)).cloned() };
    let has = |name: &str| rest.iter().any(|a| a == name);
    let positional = || -> Option<String> {
        let mut skip = false;
        for a in &rest {
            if skip {
                skip = false;
                continue;
            }
            if ["--path", "--git", "--rev", "--tag", "--branch", "--version", "--registry"].contains(&a.as_str()) {
                skip = true;
            } else if !a.starts_with("--") {
                return Some(a.clone());
            }
        }
        None
    };
    let result = match cmd.as_str() {
        "new" => match positional() {
            Some(n) => driver::new(&n, has("--lib")),
            None => Err("usage: kote new <NAME> [--lib]".into()),
        },
        "init" => driver::init(has("--lib")),
        "check" => driver::check(&flags),
        "build" => driver::build(&flags),
        "run" => driver::run(&flags, tail),
        "test" => driver::test(&flags, positional()),
        "add" => driver::add(
            &flags,
            AddArgs { name: positional(), path: opt("--path"), git: opt("--git"), rev: opt("--rev"), tag: opt("--tag"), branch: opt("--branch"), version: opt("--version"), registry: opt("--registry") },
        ),
        "remove" | "rm" => match positional() {
            Some(n) => driver::remove(&flags, &n),
            None => Err("usage: kote remove <NAME>".into()),
        },
        "publish" => driver::publish(opt("--registry")),
        "yank" => driver::yank(positional(), opt("--registry"), has("--undo")),
        "update" => driver::update(&flags, positional()),
        "bench" => driver::bench(&flags, positional()),
        "doc" => driver::doc(),
        "fmt" => driver::fmt(has("--check")),
        "tree" => driver::tree(&flags),
        "clean" => driver::clean(),
        "version" | "-V" | "--version" => {
            println!("kote {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        "help" | "-h" | "--help" => {
            print!("{USAGE}");
            Ok(0)
        }
        other => Err(format!("unknown command `{other}`\nhelp: run `kote help` for the list of commands")),
    };
    match result {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}
