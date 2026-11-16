//! Versions of mods and conditions on them (format version 3).
//!
//! A version is semantic: `1.2.3`, with a pre-release suffix such as
//! `1.2.3-beta` or `1.2.3-rc.1`. Shorter numbers are filled with zeros:
//! `1.2` is `1.2.0`, `1` is `1.0.0`. Versions are compared as semver
//! orders them ([`semver::Version`]): `1.2.0-beta` < `1.2.0-rc.1` < `1.2.0`.
//!
//! A condition is a list of comparisons separated by spaces, all of which
//! must hold: `>=1.1 <2`. `*` (or nothing) allows every version.
//!
//! | Comparison | Allows |
//! |---|---|
//! | `1.2`, `=1.2` | exactly 1.2.0 |
//! | `>1.2`, `>=1.2`, `<2`, `<=1.4` | as written; `<2` leaves out the pre-releases of 2.0.0 |
//! | `^1.2` | 1.2.0 up to, not including, 2.0.0 (`^0.3` — up to 0.4.0) |
//! | `~1.2` | 1.2.0 up to, not including, 1.3.0 (`~1` — up to 2.0.0) |
//!
//! Unlike Cargo, a pre-release passes a condition like any other version
//! when it is in range: `>=1.1` allows `1.2.0-beta`.

use std::fmt;

use semver::{Prerelease, Version};

use crate::{Error, Result};

/// Longest text of a condition.
pub const MAX_CONDITION: usize = 200;
/// Most comparisons in one condition.
const MAX_COMPARISONS: usize = 8;

/// Reads a version: `1`, `1.2`, `1.2.3`, each with an optional pre-release
/// (`-beta`, `-rc.1`) and build (`+abc`) suffix. Missing numbers are zeros.
pub fn parse_version(text: &str) -> Result<Version> {
    let bad = || {
        Error::mod_error(format!(
            "`{}` is not a version: use numbers like 1.2 or 1.2.3, optionally with a suffix \
             like -beta",
            text.escape_debug()
        ))
    };
    let text = text.trim();
    let end = text.find(['-', '+']).unwrap_or(text.len());
    let (core, suffix) = text.split_at(end);
    let parts: Vec<&str> = core.split('.').collect();
    if parts.is_empty()
        || parts.len() > 3
        || parts.iter().any(|p| {
            p.is_empty()
                || !p.bytes().all(|b| b.is_ascii_digit())
                || (p.len() > 1 && p.starts_with('0'))
        })
    {
        return Err(bad());
    }
    let mut full = parts.join(".");
    for _ in parts.len()..3 {
        full += ".0";
    }
    full += suffix;
    Version::parse(&full).map_err(|_| bad())
}

/// The version of this program (`doctor_toolkit` and Open Mod Manager share it).
pub fn manager_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("the crate version is semver")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Exact,
    Greater,
    GreaterEq,
    Less,
    LessEq,
    Caret,
    Tilde,
}

/// One comparison of a [`Condition`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct Comparison {
    op: Op,
    version: Version,
    /// How many numbers were written (`~1` and `~1.2` differ).
    parts: usize,
}

impl Comparison {
    fn parse(text: &str) -> Option<Self> {
        let (op, rest) = [
            (">=", Op::GreaterEq),
            ("<=", Op::LessEq),
            (">", Op::Greater),
            ("<", Op::Less),
            ("=", Op::Exact),
            ("^", Op::Caret),
            ("~", Op::Tilde),
        ]
        .into_iter()
        .find_map(|(prefix, op)| text.strip_prefix(prefix).map(|rest| (op, rest)))
        .unwrap_or((Op::Exact, text));
        let rest = rest.trim_start();
        let core = rest.split(['-', '+']).next().unwrap_or_default();
        let parts = core.split('.').count();
        let mut version = parse_version(rest).ok()?;
        // The build suffix plays no part in comparing.
        version.build = semver::BuildMetadata::EMPTY;
        Some(Self { op, version, parts })
    }

    /// The lowest version above the range of `^` and `~`.
    fn upper(&self) -> Version {
        let v = &self.version;
        let next = |major, minor, patch| Version::new(major, minor, patch);
        match self.op {
            Op::Caret if v.major > 0 || self.parts == 1 => next(v.major + 1, 0, 0),
            Op::Caret if v.minor > 0 || self.parts == 2 => next(0, v.minor + 1, 0),
            Op::Caret => next(0, 0, v.patch + 1),
            _ if self.parts == 1 => next(v.major + 1, 0, 0),
            _ => next(v.major, v.minor + 1, 0),
        }
    }

    fn matches(&self, version: &Version) -> bool {
        let v = &self.version;
        let below = |bound: &Version| {
            // `<2` leaves out 2.0.0-beta: a bound without a pre-release
            // excludes the pre-releases of that version too.
            version < bound
                && !(bound.pre.is_empty()
                    && !version.pre.is_empty()
                    && (version.major, version.minor, version.patch)
                        == (bound.major, bound.minor, bound.patch))
        };
        match self.op {
            Op::Exact => cmp_no_build(version, v) == std::cmp::Ordering::Equal,
            Op::Greater => cmp_no_build(version, v) == std::cmp::Ordering::Greater,
            Op::GreaterEq => cmp_no_build(version, v) != std::cmp::Ordering::Less,
            Op::Less => below(v),
            Op::LessEq => cmp_no_build(version, v) != std::cmp::Ordering::Greater,
            Op::Caret | Op::Tilde => {
                cmp_no_build(version, v) != std::cmp::Ordering::Less && below(&self.upper())
            }
        }
    }
}

/// Compares two versions, ignoring the build suffix.
fn cmp_no_build(a: &Version, b: &Version) -> std::cmp::Ordering {
    (a.major, a.minor, a.patch)
        .cmp(&(b.major, b.minor, b.patch))
        .then_with(|| match (a.pre.is_empty(), b.pre.is_empty()) {
            (true, true) => std::cmp::Ordering::Equal,
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            (false, false) => a.pre.cmp(&b.pre),
        })
}

impl fmt::Display for Comparison {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let op = match self.op {
            Op::Exact => "=",
            Op::Greater => ">",
            Op::GreaterEq => ">=",
            Op::Less => "<",
            Op::LessEq => "<=",
            Op::Caret => "^",
            Op::Tilde => "~",
        };
        let v = &self.version;
        let mut text = match self.parts {
            1 => format!("{}", v.major),
            2 => format!("{}.{}", v.major, v.minor),
            _ => format!("{}.{}.{}", v.major, v.minor, v.patch),
        };
        if v.pre != Prerelease::EMPTY {
            text += &format!("-{}", v.pre);
        }
        write!(f, "{op}{text}")
    }
}

/// A condition on a version: comparisons that must all hold, or any
/// version (`*`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Condition {
    comparisons: Vec<Comparison>,
}

impl Condition {
    /// Any version.
    pub fn any() -> Self {
        Self {
            comparisons: Vec::new(),
        }
    }

    /// At least `version`: `>=1.2`.
    pub fn at_least(version: &Version) -> Self {
        Self {
            comparisons: vec![Comparison {
                op: Op::GreaterEq,
                version: version.clone(),
                parts: 3,
            }],
        }
    }

    /// Reads a condition such as `>=1.1 <2`, `^1.2` or `*`.
    pub fn parse(text: &str) -> Result<Self> {
        let bad = |why: &str| {
            Err(Error::mod_error(format!(
                "`{}` is not a version condition: {why}",
                text.escape_debug()
            )))
        };
        if text.len() > MAX_CONDITION {
            return bad("too long");
        }
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed == "*" {
            return Ok(Self::any());
        }
        // `>= 1.2` is read as `>=1.2`: glue an operator to the version after it.
        let mut words: Vec<String> = Vec::new();
        for word in trimmed.split_whitespace() {
            match words.last_mut() {
                Some(last) if last.chars().all(|c| "<>=^~".contains(c)) => last.push_str(word),
                _ => words.push(word.to_owned()),
            }
        }
        if words.len() > MAX_COMPARISONS {
            return bad("too many comparisons");
        }
        let mut comparisons = Vec::new();
        for word in &words {
            if word == "*" {
                continue;
            }
            match Comparison::parse(word) {
                Some(c) => comparisons.push(c),
                None => {
                    return bad(&format!(
                        "`{}` is not a comparison; use =, >, >=, <, <=, ^ or ~ before a \
                         version like 1.2, and spaces between comparisons",
                        word.escape_debug()
                    ));
                }
            }
        }
        Ok(Self { comparisons })
    }

    /// Whether every version is allowed.
    pub fn is_any(&self) -> bool {
        self.comparisons.is_empty()
    }

    /// Whether `version` is allowed.
    pub fn matches(&self, version: &Version) -> bool {
        self.comparisons.iter().all(|c| c.matches(version))
    }

    /// Whether the version written as `text` is allowed; a text that is no
    /// version only passes `*`.
    pub fn matches_text(&self, text: &str) -> bool {
        self.is_any() || parse_version(text).is_ok_and(|v| self.matches(&v))
    }
}

impl fmt::Display for Condition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.comparisons.is_empty() {
            return write!(f, "*");
        }
        let words: Vec<String> = self.comparisons.iter().map(|c| c.to_string()).collect();
        write!(f, "{}", words.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        parse_version(text).unwrap()
    }

    fn ok(condition: &str, version: &str) -> bool {
        Condition::parse(condition).unwrap().matches(&v(version))
    }

    #[test]
    fn short_versions_are_filled_with_zeros() {
        assert_eq!(v("1.2"), Version::new(1, 2, 0));
        assert_eq!(v("3"), Version::new(3, 0, 0));
        assert_eq!(v("1.2.3"), Version::new(1, 2, 3));
        assert_eq!(v("1.2-beta").to_string(), "1.2.0-beta");
        assert_eq!(v("1.2.3-rc.1").to_string(), "1.2.3-rc.1");
        assert_eq!(v(" 0.4 ").to_string(), "0.4.0");
        for bad in [
            "",
            "1.",
            ".1",
            "1.2.3.4",
            "a",
            "v1.2",
            "1.02",
            "1.2-",
            "1..2",
            "1.2 beta",
            "-1",
            "1.2-be ta",
        ] {
            assert!(parse_version(bad).is_err(), "{bad}");
        }
        assert!(v("1.2.0-beta") < v("1.2.0-rc.1"));
        assert!(v("1.2.0-rc.1") < v("1.2"));
    }

    #[test]
    fn conditions_compare_versions() {
        assert!(ok("*", "0.0.1"));
        assert!(ok("", "7"));
        assert!(ok("=1.2", "1.2.0"));
        assert!(!ok("=1.2", "1.2.1"));
        assert!(ok("1.2", "1.2.0"));
        assert!(!ok("1.2", "1.3"));
        assert!(ok(">=1.1", "1.1"));
        assert!(ok(">=1.1", "1.2.0-beta"));
        assert!(!ok(">=1.1", "1.0.9"));
        assert!(ok(">1.0", "1.0.1"));
        assert!(!ok(">1.0", "1.0"));
        assert!(ok("<2", "1.99"));
        assert!(!ok("<2", "2.0.0-beta"));
        assert!(!ok("<2", "2"));
        assert!(ok("<2.0.0-rc.1", "2.0.0-beta"));
        assert!(ok("<=1.4", "1.4"));
        assert!(ok(">=1.1 <2", "1.5"));
        assert!(!ok(">=1.1 <2", "2.1"));
        assert!(!ok(">=1.1 <2", "1.0"));
        assert!(ok(">= 1.1  < 2", "1.5"));
        assert!(ok("^1.2", "1.9.9"));
        assert!(!ok("^1.2", "2.0"));
        assert!(!ok("^1.2", "1.1"));
        assert!(ok("^0.3", "0.3.5"));
        assert!(!ok("^0.3", "0.4"));
        assert!(!ok("^0.0.3", "0.0.4"));
        assert!(ok("~1.2", "1.2.7"));
        assert!(!ok("~1.2", "1.3"));
        assert!(ok("~1", "1.9"));
        assert!(!ok("~1", "2"));
        for bad in [
            ">=", "1.2.3.4", ">=1.x", "!1.2", ">=1,<2", "1.2 || 2", ">>1",
        ] {
            assert!(Condition::parse(bad).is_err(), "{bad}");
        }
        assert!(Condition::parse(&"1 ".repeat(9)).is_err());
        assert!(Condition::parse(&"1".repeat(MAX_CONDITION + 1)).is_err());
    }

    #[test]
    fn conditions_are_written_back_in_short() {
        let c = Condition::parse(">= 1.1 <2 ^0.3-beta").unwrap();
        assert_eq!(c.to_string(), ">=1.1 <2 ^0.3-beta");
        assert_eq!(Condition::parse(c.to_string().as_str()).unwrap(), c);
        assert_eq!(Condition::any().to_string(), "*");
        let built = Condition::parse(">=1.2+abc").unwrap();
        assert_eq!(Condition::parse(&built.to_string()).unwrap(), built);
        assert_eq!(Condition::at_least(&v("0.0.5")).to_string(), ">=0.0.5");
        assert!(Condition::parse("*").unwrap().matches_text("anything"));
        assert!(!Condition::parse(">=1").unwrap().matches_text("anything"));
        assert!(Condition::parse(">=1").unwrap().matches_text("1.0"));
    }

    #[test]
    fn this_program_has_a_version() {
        assert!(manager_version() >= Version::new(0, 0, 4));
    }
}
