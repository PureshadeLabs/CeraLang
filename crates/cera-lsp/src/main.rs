//! `cera-lsp`: speaks the Language Server Protocol over stdio.

use std::io::{self, BufRead, Write};
use std::process::ExitCode;

use cera_lsp::Server;
use serde_json::Value;

fn read_message(input: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            if content_length.is_some() {
                break;
            }
            continue;
        }
        if let Some(rest) = line
            .strip_prefix("Content-Length:")
            .or_else(|| line.strip_prefix("content-length:"))
        {
            content_length = rest.trim().parse().ok();
        }
    }
    let len = content_length.unwrap_or(0);
    let mut buf = vec![0u8; len];
    input.read_exact(&mut buf)?;
    match serde_json::from_slice(&buf) {
        Ok(v) => Ok(Some(v)),
        Err(e) => Err(io::Error::new(io::ErrorKind::InvalidData, e)),
    }
}

fn write_message(out: &mut impl Write, msg: &Value) -> io::Result<()> {
    let body = msg.to_string();
    write!(out, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
    out.flush()
}

fn main() -> ExitCode {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("cera-lsp {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    let stdin = io::stdin();
    let mut input = stdin.lock();
    let stdout = io::stdout();
    let mut output = stdout.lock();
    let mut server = Server::new();

    loop {
        let msg = match read_message(&mut input) {
            Ok(Some(m)) => m,
            Ok(None) => break,
            Err(e) => {
                eprintln!("cera-lsp: protocol error: {e}");
                break;
            }
        };
        for reply in server.handle(msg) {
            if write_message(&mut output, &reply).is_err() {
                return ExitCode::FAILURE;
            }
        }
        if server.exit_requested {
            break;
        }
    }

    // The protocol says: exit code 0 only if shutdown was requested first.
    if server.shutdown_requested {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
