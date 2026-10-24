//! Package registries.
//!
//! A registry is a directory `<registry>/<name>/<version>/` holding a copy of each
//! published package (`kote.toml` and `src/`), or an HTTP server that offers the same
//! packages (see `docs/registry-protocol` in the book, chapter "kote"). An HTTP
//! registry is mirrored into a local cache directory on demand, so resolution always
//! reads files.
//!
//! A version can be *yanked*: it stays downloadable, and projects whose `kote.lock`
//! already names it keep using it, but resolution never picks it for a new lock.

use crate::git::kote_home;
use crate::manifest::{DepSource, Manifest};
use crate::semver::Version;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

static REQUESTS: AtomicUsize = AtomicUsize::new(0);

pub struct Registry {
    /// The directory holding the packages (the cache directory for an HTTP registry).
    pub dir: PathBuf,
    /// The base URL of an HTTP registry.
    pub url: Option<String>,
}

fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

/// The registry named by `spec` (relative to `base`), else `$KOTE_REGISTRY`,
/// else `$KOTE_HOME/registry`.
pub fn open(spec: Option<&str>, base: &Path) -> Registry {
    let spec = match spec {
        Some(s) => Some(s.to_string()),
        None => std::env::var("KOTE_REGISTRY").ok().filter(|p| !p.is_empty()),
    };
    match spec {
        Some(s) if is_url(&s) => {
            let url = s.trim_end_matches('/').to_string();
            let mut h: u64 = 0xcbf29ce484222325;
            for b in url.bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
            Registry { dir: kote_home().join("registry-cache").join(format!("{h:016x}")), url: Some(url) }
        }
        Some(s) => Registry { dir: if Path::new(&s).is_absolute() { PathBuf::from(&s) } else { base.join(&s) }, url: None },
        None => Registry { dir: kote_home().join("registry"), url: None },
    }
}

static SYNCED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn token_args() -> Vec<String> {
    match std::env::var("KOTE_TOKEN") {
        Ok(t) if !t.is_empty() => vec!["-H".into(), format!("Authorization: Bearer {t}")],
        _ => vec![],
    }
}

/// An HTTP request through `curl`: returns the status code and the body.
fn http(method: &str, url: &str, upload: Option<&Path>) -> Result<(u16, Vec<u8>), String> {
    let body = std::env::temp_dir().join(format!("kote-http-{}-{}", std::process::id(), REQUESTS.fetch_add(1, Ordering::SeqCst)));
    let mut cmd = Command::new("curl");
    cmd.args(["-sS", "-L", "--max-time", "120", "-X", method, "-o"]).arg(&body).args(["-w", "%{http_code}"]).args(token_args());
    if let Some(f) = upload {
        cmd.arg("-T").arg(f);
    }
    cmd.arg(url);
    let out = cmd.output().map_err(|e| format!("cannot run `curl` (needed for HTTP registries): {e}"))?;
    let data = std::fs::read(&body).unwrap_or_default();
    let _ = std::fs::remove_file(&body);
    if !out.status.success() {
        return Err(format!("request to {url} failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let code = String::from_utf8_lossy(&out.stdout).trim().parse::<u16>().unwrap_or(0);
    Ok((code, data))
}

fn safe_archive(tar: &Path) -> Result<(), String> {
    let out = Command::new("tar").arg("-tf").arg(tar).output().map_err(|e| format!("cannot run `tar`: {e}"))?;
    if !out.status.success() {
        return Err("the downloaded package is not a valid archive".into());
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let p = Path::new(line);
        let bad = p.is_absolute() || p.components().any(|c| matches!(c, std::path::Component::ParentDir));
        let top = line.trim_start_matches("./");
        if bad || !(top == "kote.toml" || top == "src" || top.starts_with("src/")) {
            return Err(format!("the downloaded package contains an unexpected entry `{line}`"));
        }
    }
    Ok(())
}

impl Registry {
    pub fn describe(&self) -> String {
        self.url.clone().unwrap_or_else(|| self.dir.display().to_string())
    }

    /// Bring the cache of an HTTP registry up to date for `name` (once per run). Other
    /// registries need nothing. With `offline`, or when the server cannot be reached
    /// but versions are cached, the cache is used as it is.
    pub fn sync(&self, name: &str, offline: bool) -> Result<(), String> {
        let Some(url) = &self.url else { return Ok(()) };
        if offline {
            return Ok(());
        }
        let key = format!("{url}#{name}");
        {
            let mut g = SYNCED.lock().unwrap();
            if !g.get_or_insert_with(HashSet::new).insert(key) {
                return Ok(());
            }
        }
        let cached = !self.versions(name, &[]).is_empty();
        let fail = |e: String| -> Result<(), String> {
            if cached {
                eprintln!("warning: {e}; using cached versions of `{name}`");
                Ok(())
            } else {
                Err(e)
            }
        };
        let (code, body) = match http("GET", &format!("{url}/{name}/index"), None) {
            Ok(r) => r,
            Err(e) => return fail(e),
        };
        match code {
            404 => return Ok(()),
            200 => {}
            c => return fail(format!("the registry at {url} answered {c} for `{name}`")),
        }
        for line in String::from_utf8_lossy(&body).lines() {
            let mut words = line.split_whitespace();
            let Some(Ok(v)) = words.next().map(Version::parse) else { continue };
            let yanked = words.next() == Some("yanked");
            let dir = self.dir.join(name).join(v.to_string());
            if !dir.join("kote.toml").is_file() {
                self.download(url, name, &v, &dir)?;
            }
            let marker = dir.join(".yanked");
            if yanked {
                let _ = std::fs::write(&marker, "");
            } else {
                let _ = std::fs::remove_file(&marker);
            }
        }
        Ok(())
    }

    fn download(&self, url: &str, name: &str, v: &Version, dir: &Path) -> Result<(), String> {
        let tar = std::env::temp_dir().join(format!("kote-pkg-{}-{name}-{v}.tar", std::process::id()));
        let (code, body) = http("GET", &format!("{url}/{name}/{v}.tar"), None)?;
        if code != 200 {
            return Err(format!("cannot download `{name} {v}` from {url} (status {code})"));
        }
        std::fs::write(&tar, body).map_err(|e| e.to_string())?;
        let result = (|| {
            safe_archive(&tar)?;
            let stage = dir.with_extension("partial");
            let _ = std::fs::remove_dir_all(&stage);
            std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
            let st = Command::new("tar").arg("-xf").arg(&tar).arg("-C").arg(&stage).status().map_err(|e| e.to_string())?;
            if !st.success() || !stage.join("kote.toml").is_file() {
                let _ = std::fs::remove_dir_all(&stage);
                return Err(format!("the package `{name} {v}` is malformed"));
            }
            std::fs::rename(&stage, dir).map_err(|e| e.to_string())
        })();
        let _ = std::fs::remove_file(&tar);
        result
    }

    /// Published versions of `name`, ascending. Yanked versions are left out unless they
    /// are in `keep`.
    pub fn versions(&self, name: &str, keep: &[Version]) -> Vec<Version> {
        let mut out: Vec<Version> = std::fs::read_dir(self.dir.join(name))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let v = Version::parse(&e.file_name().to_string_lossy()).ok()?;
                (!e.path().join(".yanked").exists() || keep.contains(&v)).then_some(v)
            })
            .collect();
        out.sort();
        out
    }

    /// Whether any version of `name` exists, yanked or not.
    pub fn has_versions(&self, name: &str) -> bool {
        std::fs::read_dir(self.dir.join(name)).into_iter().flatten().flatten().any(|e| Version::parse(&e.file_name().to_string_lossy()).is_ok())
    }

    pub fn package_dir(&self, name: &str, v: &Version) -> PathBuf {
        self.dir.join(name).join(v.to_string())
    }
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("cannot create {}: {e}", to.display()))?;
    for e in std::fs::read_dir(from).map_err(|e| format!("cannot read {}: {e}", from.display()))?.flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst).map_err(|e| format!("cannot copy {}: {e}", src.display()))?;
        }
    }
    Ok(())
}

fn validate(dir: &Path) -> Result<(Manifest, Version), String> {
    let m = Manifest::load(dir)?;
    let version = Version::parse(&m.version).map_err(|e| format!("package version: {e}"))?;
    for d in &m.deps {
        if !matches!(d.source, DepSource::Registry { .. }) {
            return Err(format!(
                "cannot publish: dependency `{}` is a path or git dependency\nhelp: published packages may only depend on registry packages, such as `{} = \"^1.0\"`",
                d.name, d.name
            ));
        }
    }
    if !dir.join("src").join("lib.cera").exists() && !dir.join("src").join("main.cera").exists() {
        return Err("cannot publish: the package has no `src/lib.cera` or `src/main.cera`".into());
    }
    Ok((m, version))
}

/// Publish the package in `dir`; returns `(name, version, destination)`.
pub fn publish(dir: &Path, reg: &Registry) -> Result<(String, Version, String), String> {
    let (m, version) = validate(dir)?;
    let exists = format!("`{} {version}` is already published; bump `version` in kote.toml\nnote: published versions are immutable", m.name);
    if let Some(url) = &reg.url {
        let tar = std::env::temp_dir().join(format!("kote-pub-{}-{}-{version}.tar", std::process::id(), m.name));
        let st = Command::new("tar").arg("-cf").arg(&tar).arg("-C").arg(dir).args(["kote.toml", "src"]).status().map_err(|e| format!("cannot run `tar`: {e}"))?;
        if !st.success() {
            return Err("cannot archive the package".into());
        }
        let r = http("PUT", &format!("{url}/{}/{version}.tar", m.name), Some(&tar));
        let _ = std::fs::remove_file(&tar);
        let (code, body) = r?;
        return match code {
            200 | 201 | 204 => Ok((m.name, version, url.clone())),
            409 => Err(exists),
            401 | 403 => Err(format!("the registry refused the upload (status {code})\nhelp: set `KOTE_TOKEN` to your registry token")),
            c => Err(format!("the registry answered {c}: {}", String::from_utf8_lossy(&body).trim())),
        };
    }
    let dest = reg.package_dir(&m.name, &version);
    if dest.exists() {
        return Err(exists);
    }
    copy_dir(&dir.join("src"), &dest.join("src"))?;
    std::fs::copy(dir.join("kote.toml"), dest.join("kote.toml")).map_err(|e| e.to_string())?;
    Ok((m.name, version, dest.display().to_string()))
}

/// Yank (or, with `undo`, un-yank) `name` `version`.
pub fn yank(reg: &Registry, name: &str, version: &Version, undo: bool) -> Result<(), String> {
    if let Some(url) = &reg.url {
        let (code, body) = http(if undo { "DELETE" } else { "PUT" }, &format!("{url}/{name}/{version}/yank"), None)?;
        return match code {
            200 | 201 | 204 => Ok(()),
            404 => Err(format!("`{name} {version}` is not published in {url}")),
            401 | 403 => Err(format!("the registry refused the request (status {code})\nhelp: set `KOTE_TOKEN` to your registry token")),
            c => Err(format!("the registry answered {c}: {}", String::from_utf8_lossy(&body).trim())),
        };
    }
    let dir = reg.package_dir(name, version);
    if !dir.is_dir() {
        return Err(format!("`{name} {version}` is not published in {}", reg.describe()));
    }
    let marker = dir.join(".yanked");
    if undo {
        let _ = std::fs::remove_file(&marker);
        Ok(())
    } else {
        std::fs::write(&marker, "").map_err(|e| e.to_string())
    }
}
