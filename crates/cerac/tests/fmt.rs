use std::io::Write;
use std::process::{Command, Stdio};

fn cerac() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cerac"))
}

#[test]
fn fmt_stdin_to_stdout() {
    let mut c = cerac().args(["fmt", "-"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    c.stdin.take().unwrap().write_all(b"f::fn()->i32 do\n1+2\nend\n").unwrap();
    let o = c.wait_with_output().unwrap();
    assert!(o.status.success());
    assert_eq!(String::from_utf8_lossy(&o.stdout), "f :: fn() -> i32 do\n  1 + 2\nend\n");
}

#[test]
fn fmt_in_place_and_check() {
    let dir = std::env::temp_dir().join(format!("cerac-fmt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("a.cera");
    std::fs::write(&f, "f::fn()->i32 do\n1+2\nend\n").unwrap();
    let o = cerac().args(["fmt", "--check"]).arg(&f).output().unwrap();
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("would reformat"));
    assert!(cerac().arg("fmt").arg(&f).output().unwrap().status.success());
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "f :: fn() -> i32 do\n  1 + 2\nend\n");
    assert!(cerac().args(["fmt", "--check"]).arg(&f).output().unwrap().status.success());
    std::fs::write(&f, "f :: fn( do\nend\n").unwrap();
    let o = cerac().arg("fmt").arg(&f).output().unwrap();
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("syntax error"));
    let _ = std::fs::remove_dir_all(&dir);
}
