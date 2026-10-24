//! Versions (`MAJOR.MINOR.PATCH`) and version requirements (`^1.2`, `~1.4.1`,
//! `>=1.0, <2`). Pre-release and build suffixes are not supported.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    /// The compatibility series: all of `1.x` are one series, and each `0.y` is its own.
    pub fn series(&self) -> String {
        if self.major > 0 { self.major.to_string() } else { format!("0_{}", self.minor) }
    }

    pub fn new(major: u64, minor: u64, patch: u64) -> Version {
        Version { major, minor, patch }
    }

    pub fn parse(s: &str) -> Result<Version, String> {
        let (v, n) = parse_partial(s.trim())?;
        if n != 3 {
            return Err(format!("`{s}` is not a full version; write MAJOR.MINOR.PATCH, for example `1.2.0`"));
        }
        Ok(v)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Parse `1`, `1.2` or `1.2.3`; missing parts are 0. Returns the part count.
fn parse_partial(s: &str) -> Result<(Version, usize), String> {
    if s.is_empty() {
        return Err("empty version".into());
    }
    let mut parts = [0u64; 3];
    let mut n = 0;
    for p in s.split('.') {
        if n == 3 {
            return Err(format!("`{s}` has too many parts; versions look like `1.2.3`"));
        }
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!("`{s}` is not a valid version (pre-release and build suffixes are not supported)"));
        }
        parts[n] = p.parse().map_err(|_| format!("`{p}` is too large in `{s}`"))?;
        n += 1;
    }
    Ok((Version::new(parts[0], parts[1], parts[2]), n))
}

/// One bound pair: `lo <= v < hi` (each side optional; `lo_incl`/`hi_incl` pick the edges).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Range {
    lo: Option<(Version, bool)>,
    hi: Option<(Version, bool)>,
}

impl Range {
    fn contains(&self, v: &Version) -> bool {
        if let Some((lo, incl)) = &self.lo {
            if v < lo || (!incl && v == lo) {
                return false;
            }
        }
        if let Some((hi, incl)) = &self.hi {
            if v > hi || (!incl && v == hi) {
                return false;
            }
        }
        true
    }
}

/// A set of comparators that must all hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Req {
    ranges: Vec<Range>,
    text: String,
}

impl Req {
    pub fn parse(s: &str) -> Result<Req, String> {
        let mut ranges = vec![];
        for part in s.split(',') {
            ranges.push(parse_comparator(part.trim()).map_err(|e| format!("version requirement `{s}`: {e}"))?);
        }
        let text = s.split(',').map(str::trim).collect::<Vec<_>>().join(", ");
        Ok(Req { ranges, text })
    }

    pub fn matches(&self, v: &Version) -> bool {
        self.ranges.iter().all(|r| r.contains(v))
    }
}

impl fmt::Display for Req {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

fn parse_comparator(s: &str) -> Result<Range, String> {
    let (op, rest) = ["^", "~", ">=", "<=", ">", "<", "="]
        .iter()
        .find_map(|op| s.strip_prefix(op).map(|r| (*op, r)))
        .unwrap_or(("^", s));
    let (v, n) = parse_partial(rest.trim())?;
    let bump = |major: u64, minor: u64, patch: u64| Version::new(major, minor, patch);
    // The first version that is NOT covered by a prefix match of `n` parts.
    let next_prefix = match n {
        1 => bump(v.major + 1, 0, 0),
        2 => bump(v.major, v.minor + 1, 0),
        _ => bump(v.major, v.minor, v.patch + 1),
    };
    Ok(match op {
        "=" => Range { lo: Some((v, true)), hi: Some((next_prefix, false)) },
        ">" => Range { lo: Some((next_prefix, true)), hi: None },
        ">=" => Range { lo: Some((v, true)), hi: None },
        "<" => Range { lo: None, hi: Some((v, false)) },
        "<=" => Range { lo: None, hi: Some((next_prefix, false)) },
        "~" => {
            let hi = if n == 1 { bump(v.major + 1, 0, 0) } else { bump(v.major, v.minor + 1, 0) };
            Range { lo: Some((v, true)), hi: Some((hi, false)) }
        }
        _ => {
            // Caret: the leftmost non-zero part may not change.
            let hi = if v.major > 0 || n == 1 {
                bump(v.major + 1, 0, 0)
            } else if v.minor > 0 || n == 2 {
                bump(0, v.minor + 1, 0)
            } else {
                bump(0, 0, v.patch + 1)
            };
            Range { lo: Some((v, true)), hi: Some((hi, false)) }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(req: &str, v: &str) -> bool {
        Req::parse(req).unwrap().matches(&Version::parse(v).unwrap())
    }

    #[test]
    fn versions_order_numerically() {
        assert!(Version::parse("1.10.0").unwrap() > Version::parse("1.9.9").unwrap());
        assert!(Version::parse("1.2").is_err());
        assert!(Version::parse("1.2.3-beta").is_err());
        assert_eq!(Version::parse("0.3.7").unwrap().to_string(), "0.3.7");
    }

    #[test]
    fn caret() {
        assert!(m("^1.2.3", "1.2.3") && m("^1.2.3", "1.9.0") && !m("^1.2.3", "2.0.0") && !m("^1.2.3", "1.2.2"));
        assert!(m("^0.2.3", "0.2.9") && !m("^0.2.3", "0.3.0"));
        assert!(m("^0.0.3", "0.0.3") && !m("^0.0.3", "0.0.4"));
        assert!(m("^1.2", "1.9.9") && !m("^1.2", "1.1.9") && !m("^1.2", "2.0.0"));
        assert!(m("^0", "0.9.0") && !m("^0", "1.0.0"));
        assert!(m("1.2.3", "1.5.0"), "a bare version is a caret requirement");
    }

    #[test]
    fn tilde_exact_and_comparisons() {
        assert!(m("~1.2.3", "1.2.9") && !m("~1.2.3", "1.3.0"));
        assert!(m("~1", "1.9.0") && !m("~1", "2.0.0"));
        assert!(m("=1.2.3", "1.2.3") && !m("=1.2.3", "1.2.4"));
        assert!(m("=1.2", "1.2.7") && !m("=1.2", "1.3.0"));
        assert!(m(">=1.0, <2", "1.5.0") && !m(">=1.0, <2", "2.0.0") && !m(">=1.0, <2", "0.9.0"));
        assert!(m(">1.2.3", "1.2.4") && !m(">1.2.3", "1.2.3"));
        assert!(m("<=1.2", "1.2.9") && !m("<=1.2", "1.3.0"));
    }

    #[test]
    fn bad_requirements() {
        assert!(Req::parse("").is_err());
        assert!(Req::parse("^").is_err());
        assert!(Req::parse("1.2.x").is_err());
        assert!(Req::parse("^1.2.3.4").is_err());
        assert_eq!(Req::parse("^1.2,<1.5").unwrap().to_string(), "^1.2, <1.5");
    }
}
