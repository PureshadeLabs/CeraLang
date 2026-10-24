//! `kote.toml`: the package manifest.

use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitSpec {
    /// The default branch of the repository.
    Default,
    Rev(String),
    Tag(String),
    Branch(String),
}

impl GitSpec {
    /// `rev=..`, `tag=..` or `branch=..`, as stored in the lock file.
    pub fn query(&self) -> Option<String> {
        match self {
            GitSpec::Default => None,
            GitSpec::Rev(r) => Some(format!("rev={r}")),
            GitSpec::Tag(t) => Some(format!("tag={t}")),
            GitSpec::Branch(b) => Some(format!("branch={b}")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DepSource {
    Path(String),
    Git { url: String, spec: GitSpec },
    /// A published package: `req` is a version requirement such as `^1.2`; `registry`
    /// is a directory (relative to the manifest) or the default registry.
    Registry { req: String, registry: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dependency {
    pub name: String,
    pub source: DepSource,
}

#[derive(Clone, Debug)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub deps: Vec<Dependency>,
}

const RESERVED: &[&str] = &["std", "kote", "main", "lib", "tests", "benches", "do", "end", "fn", "if", "else", "match", "type", "trait", "impl", "import", "let", "defer", "with", "region", "for", "as", "true", "false"];

/// A package or module name: lower-case letters, digits and underscores, starting with a letter.
pub fn valid_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let ok = chars.next().is_some_and(|c| c.is_ascii_lowercase()) && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !ok {
        return Err(format!("`{name}` is not a valid package name; use lower-case letters, digits and `_`, starting with a letter"));
    }
    if RESERVED.contains(&name) {
        return Err(format!("`{name}` is reserved and cannot be a package name"));
    }
    Ok(())
}

impl Manifest {
    pub fn load(dir: &Path) -> Result<Manifest, String> {
        let path = dir.join("kote.toml");
        let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Manifest::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Manifest, String> {
        let doc: toml::Table = text.parse().map_err(|e: toml::de::Error| e.message().to_string())?;
        let pkg = doc
            .get("package")
            .and_then(|v| v.as_table())
            .ok_or("missing `[package]` table")?;
        let name = pkg
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or("`[package]` needs a `name`")?
            .to_string();
        valid_name(&name)?;
        let version = pkg.get("version").and_then(|v| v.as_str()).unwrap_or("0.1.0").to_string();
        let mut deps = vec![];
        if let Some(t) = doc.get("dependencies") {
            let t = t.as_table().ok_or("`[dependencies]` must be a table")?;
            for (dname, v) in t {
                valid_name(dname).map_err(|e| format!("dependency {e}"))?;
                if let Some(req) = v.as_str() {
                    crate::semver::Req::parse(req).map_err(|e| format!("dependency `{dname}`: {e}"))?;
                    deps.push(Dependency { name: dname.clone(), source: DepSource::Registry { req: req.to_string(), registry: None } });
                    continue;
                }
                let spec = v.as_table().ok_or_else(|| {
                    format!("dependency `{dname}` must be a version such as `\"^1.0\"` or a table such as `{{ path = \"../{dname}\" }}`")
                })?;
                let get = |k: &str| spec.get(k).and_then(|v| v.as_str()).map(str::to_string);
                for k in spec.keys() {
                    if !["path", "git", "rev", "tag", "branch", "version", "registry"].contains(&k.as_str()) {
                        return Err(format!("dependency `{dname}` has an unknown key `{k}`"));
                    }
                }
                if spec.contains_key("version") || spec.contains_key("registry") {
                    if spec.contains_key("path") || spec.contains_key("git") {
                        return Err(format!("dependency `{dname}`: `version` cannot be combined with `path` or `git`"));
                    }
                    let req = get("version").ok_or_else(|| format!("dependency `{dname}`: a registry dependency needs a `version`"))?;
                    crate::semver::Req::parse(&req).map_err(|e| format!("dependency `{dname}`: {e}"))?;
                    deps.push(Dependency { name: dname.clone(), source: DepSource::Registry { req, registry: get("registry") } });
                    continue;
                }
                let source = match (get("path"), get("git")) {
                    (Some(p), None) => {
                        if spec.len() > 1 {
                            return Err(format!("dependency `{dname}`: `rev`, `tag` and `branch` only apply to `git` dependencies"));
                        }
                        DepSource::Path(p)
                    }
                    (None, Some(url)) => {
                        let picks: Vec<GitSpec> = [
                            get("rev").map(GitSpec::Rev),
                            get("tag").map(GitSpec::Tag),
                            get("branch").map(GitSpec::Branch),
                        ]
                        .into_iter()
                        .flatten()
                        .collect();
                        if picks.len() > 1 {
                            return Err(format!("dependency `{dname}`: give only one of `rev`, `tag` and `branch`"));
                        }
                        DepSource::Git { url, spec: picks.into_iter().next().unwrap_or(GitSpec::Default) }
                    }
                    (Some(_), Some(_)) => return Err(format!("dependency `{dname}`: give either `path` or `git`, not both")),
                    (None, None) => return Err(format!("dependency `{dname}` needs a `version`, a `path` or a `git` source")),
                };
                deps.push(Dependency { name: dname.clone(), source });
            }
        }
        Ok(Manifest { name, version, deps })
    }
}

/// The text of a new manifest.
pub fn template(name: &str) -> String {
    format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n\n[dependencies]\n")
}
