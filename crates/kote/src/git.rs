//! Fetching git dependencies into a shared cache (`$KOTE_HOME`, default `~/.kote`).

use crate::manifest::GitSpec;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn kote_home() -> PathBuf {
    if let Ok(h) = std::env::var("KOTE_HOME") {
        return PathBuf::from(h);
    }
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".kote")
}

fn fnv(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn git(dir: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let mut c = Command::new("git");
    if let Some(d) = dir {
        c.arg("-C").arg(d);
    }
    let out = c
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("cannot run `git` (is it installed?): {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn has_commit(db: &Path, commit: &str) -> bool {
    git(Some(db), &["cat-file", "-e", &format!("{commit}^{{commit}}")]).is_ok()
}

/// Make `url` available at `spec` (or at the already locked `commit`) and return
/// the checkout directory and the full commit hash.
pub fn fetch(name: &str, url: &str, spec: &GitSpec, locked: Option<&str>, offline: bool) -> Result<(PathBuf, String), String> {
    let key = format!("{name}-{}", fnv(url));
    let home = kote_home().join("git");
    let db = home.join("db").join(&key);
    std::fs::create_dir_all(home.join("db")).map_err(|e| e.to_string())?;
    let refspecs = ["+refs/heads/*:refs/heads/*", "+refs/tags/*:refs/tags/*"];
    let sync = |db: &Path| -> Result<(), String> {
        if offline {
            return Err(format!("`{name}` ({url}) is not in the local cache and --offline was given"));
        }
        git(Some(db), &["fetch", "--quiet", "--force", url, refspecs[0], refspecs[1]])
            .map(|_| ())
            .map_err(|e| format!("cannot fetch {url}: {e}"))
    };
    if !db.exists() {
        if offline {
            return Err(format!("`{name}` ({url}) is not in the local cache and --offline was given"));
        }
        git(None, &["clone", "--bare", "--quiet", url, &db.to_string_lossy()]).map_err(|e| format!("cannot clone {url}: {e}"))?;
    } else if locked.is_none_or(|c| !has_commit(&db, c)) && !matches!(spec, GitSpec::Rev(r) if has_commit(&db, r)) {
        sync(&db)?;
    }
    let commit = match locked {
        Some(c) => c.to_string(),
        None => {
            let rev = match spec {
                GitSpec::Default => "HEAD".to_string(),
                GitSpec::Rev(r) => format!("{r}^{{commit}}"),
                GitSpec::Tag(t) => format!("refs/tags/{t}^{{commit}}"),
                GitSpec::Branch(b) => format!("refs/heads/{b}^{{commit}}"),
            };
            git(Some(&db), &["rev-parse", "--verify", &rev]).map_err(|_| match spec {
                GitSpec::Default => format!("{url} has no default branch to use"),
                GitSpec::Rev(r) => format!("{url} has no commit `{r}`"),
                GitSpec::Tag(t) => format!("{url} has no tag `{t}`"),
                GitSpec::Branch(b) => format!("{url} has no branch `{b}`"),
            })?
        }
    };
    let dir = home.join("checkouts").join(&key).join(&commit[..12.min(commit.len())]);
    if !dir.join("kote.toml").exists() {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.parent().unwrap()).map_err(|e| e.to_string())?;
        git(None, &["clone", "--quiet", "--no-checkout", &db.to_string_lossy(), &dir.to_string_lossy()])?;
        git(Some(&dir), &["checkout", "--quiet", "--detach", &commit]).map_err(|e| format!("cannot check out {commit}: {e}"))?;
    }
    Ok((dir, commit))
}
