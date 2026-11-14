//! Files the game opens from elsewhere (stage 46): playing with a mod or
//! a workspace without writing into the game folder.
//!
//! The toolkit builds the files a mod or a workspace would write (streams,
//! tables of contents, the overlay archive of `.viv` entries, `cmd.txt`)
//! into a folder of its own and lists them in the module's configuration
//! as [`Redirect`]s. The module hooks every call of the game through the
//! import slots of `CreateFileA`, `CreateFileW` and `GetFileAttributesA`
//! (`docs/analysis/live-link.md` §14): when the game opens a listed path of
//! its folder, the module opens the other file instead. The game folder
//! stays as it is, so there is nothing to put back after the game exits.
//!
//! A path the game names is turned into a [`key`]: relative to the game
//! folder, lower case, `/` between the parts — as Windows, the game finds
//! `GF2\CUBA.STR` and `gf2/cuba.str` alike. Paths outside the game folder
//! are never redirected.

use crate::DecodeError;
use crate::wire::{Reader, Writer};

/// At most this many redirects in one configuration.
pub const MAX_REDIRECTS: usize = 65_536;
/// At most this many redirects in a [`Files`] reply.
pub const MAX_LISTED: usize = 4_096;

/// One file the game opens from elsewhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    /// The path the game opens, relative to its folder (`gf2/cuba.str`).
    pub from: String,
    /// The file it gets: relative to the game folder
    /// (`_doctor_toolkit/overlay/gf2/cuba.str`, `GODFATH0.viv`) or an
    /// absolute Windows path.
    pub to: String,
}

impl Redirect {
    /// The line of the configuration: `FROM|TO`.
    pub fn to_line(&self) -> String {
        format!("{}|{}", self.from, self.to)
    }

    /// Reads `FROM|TO`; `None` for a line that is not one.
    pub fn parse(line: &str) -> Option<Self> {
        let (from, to) = line.split_once('|')?;
        let from = key(from.trim())?;
        let to = to.trim();
        if to.is_empty() || !line_safe(to) {
            return None;
        }
        Some(Self {
            from,
            to: to.to_owned(),
        })
    }

    /// Whether the target is relative to the game folder.
    pub fn relative(&self) -> bool {
        !is_absolute(&self.to)
    }
}

/// Whether text can stand in a line of the configuration: no line breaks,
/// no `#` (a comment there), no `|` (the separator), no control
/// characters.
pub fn line_safe(text: &str) -> bool {
    !text.chars().any(|c| c.is_control() || c == '#' || c == '|')
}

/// Whether a Windows path is absolute: `C:\…`, `C:/…`, `\\server\…`,
/// `\\?\…`. A path starting with one separator is relative to the drive of
/// the current folder ([`key_in`] takes that drive).
pub fn is_absolute(path: &str) -> bool {
    let b = path.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/'))
        || path.starts_with("\\\\")
        || path.starts_with("//")
}

/// The parts of a path, lower case, with `.` dropped and `..` taken back;
/// `None` when `..` climbs out of it.
fn parts(path: &str) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for part in path.split(['\\', '/']) {
        match part {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            p => out.push(p.to_lowercase()),
        }
    }
    Some(out)
}

/// The key of a path relative to the game folder: lower case, `/` between
/// the parts, `.` and `..` resolved. `None` when it is empty or climbs out
/// of the folder.
pub fn key(rel: &str) -> Option<String> {
    let parts = parts(rel)?;
    (!parts.is_empty() && !is_absolute(rel)).then(|| parts.join("/"))
}

/// Strips `\\?\` (and `\\?\UNC\`) of a long path.
fn plain(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{rest}")
    } else if let Some(rest) = path.strip_prefix("\\\\?\\") {
        rest.to_owned()
    } else {
        path.to_owned()
    }
}

/// The key of a path the game opens (relative to `cwd`, its current
/// folder, unless absolute), when it lies in `game_dir` (absolute).
pub fn key_in(path: &str, cwd: &str, game_dir: &str) -> Option<String> {
    let path = plain(path);
    let full = if is_absolute(&path) {
        path
    } else if path.starts_with(['\\', '/']) {
        // The root of the current drive.
        let cwd = plain(cwd);
        let drive = cwd.get(..2).filter(|d| d.ends_with(':'))?;
        format!("{drive}{path}")
    } else {
        format!("{}\\{path}", plain(cwd))
    };
    let full = parts(&full)?;
    let root = parts(&plain(game_dir))?;
    if root.is_empty() || full.len() <= root.len() || full[..root.len()] != root[..] {
        return None;
    }
    Some(full[root.len()..].join("/"))
}

/// The redirects of a configuration by key; the last line for a key wins.
pub fn table(redirects: &[Redirect]) -> std::collections::HashMap<String, String> {
    redirects
        .iter()
        .map(|r| (r.from.clone(), r.to.clone()))
        .collect()
}

/// Answer to [`crate::Request::Files`] (protocol 5): what the module
/// redirects and how often the game opened each file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Files {
    /// Whether the hooks are in place (the configuration lists redirects
    /// or asks for the log of opened files, and the edition fits).
    pub hooked: bool,
    /// Whether every file the game opens goes to the log (`files_log=1`).
    pub logging: bool,
    /// How many redirects the configuration lists.
    pub total: u32,
    /// The redirects, at most [`MAX_LISTED`] (those opened first).
    pub redirects: Vec<FileUse>,
    /// Opens of paths in the game folder that were not redirected.
    pub passed: u32,
}

/// One redirect and its use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileUse {
    pub from: String,
    pub to: String,
    /// How often the game opened it (or asked for its attributes).
    pub opens: u32,
    /// Opens of the target that failed (the file is missing).
    pub failed: u32,
}

impl Files {
    /// Redirects the game used at least once.
    pub fn used(&self) -> usize {
        self.redirects.iter().filter(|r| r.opens > 0).count()
    }
}

pub(crate) fn put_files(w: &mut Writer, f: &Files) {
    w.bool(f.hooked);
    w.bool(f.logging);
    w.u32(f.total);
    w.u32(f.passed);
    w.u32(f.redirects.len() as u32);
    for r in &f.redirects {
        w.str(&r.from);
        w.str(&r.to);
        w.u32(r.opens);
        w.u32(r.failed);
    }
}

pub(crate) fn get_files(r: &mut Reader<'_>) -> Result<Files, DecodeError> {
    let hooked = r.bool()?;
    let logging = r.bool()?;
    let total = r.u32()?;
    let passed = r.u32()?;
    let n = r.count(12)?;
    let mut redirects = Vec::with_capacity(n);
    for _ in 0..n {
        redirects.push(FileUse {
            from: r.str()?,
            to: r.str()?,
            opens: r.u32()?,
            failed: r.u32()?,
        });
    }
    Ok(Files {
        hooked,
        logging,
        total,
        redirects,
        passed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAME: &str = "Z:\\games\\The Godfather II";

    #[test]
    fn keys_of_relative_paths() {
        assert_eq!(key("gf2\\CUBA.STR").as_deref(), Some("gf2/cuba.str"));
        assert_eq!(key("./gf2//cuba.str").as_deref(), Some("gf2/cuba.str"));
        assert_eq!(key("gf2/x/../cuba.str").as_deref(), Some("gf2/cuba.str"));
        assert_eq!(key("../cuba.str"), None);
        assert_eq!(key(""), None);
        assert_eq!(key("C:\\gf2\\cuba.str"), None);
    }

    #[test]
    fn keys_of_paths_the_game_opens() {
        // Relative to the current folder, which is the game folder.
        assert_eq!(
            key_in("gf2\\cuba.str", GAME, GAME).as_deref(),
            Some("gf2/cuba.str")
        );
        assert_eq!(
            key_in(".\\GODFATH0.VIV", GAME, GAME).as_deref(),
            Some("godfath0.viv")
        );
        // Absolute, with other case and separators, or a long path.
        assert_eq!(
            key_in("z:/GAMES/the godfather ii/cmd.txt", "C:\\", GAME).as_deref(),
            Some("cmd.txt")
        );
        assert_eq!(
            key_in(
                "\\\\?\\Z:\\games\\The Godfather II\\gf2\\a.toc",
                "C:\\",
                GAME
            )
            .as_deref(),
            Some("gf2/a.toc")
        );
        // From the root of the current drive.
        assert_eq!(
            key_in("\\games\\The Godfather II\\gf2\\a.toc", GAME, GAME).as_deref(),
            Some("gf2/a.toc")
        );
        // Relative to another current folder.
        assert_eq!(
            key_in("..\\gf2\\a.toc", &format!("{GAME}\\scripts"), GAME).as_deref(),
            Some("gf2/a.toc")
        );
        // Outside the game folder, or the folder itself.
        assert_eq!(key_in("C:\\windows\\win.ini", GAME, GAME), None);
        assert_eq!(
            key_in("Z:\\games\\The Godfather II2\\cmd.txt", GAME, GAME),
            None
        );
        assert_eq!(key_in(GAME, GAME, GAME), None);
        assert_eq!(key_in("..\\..\\x", GAME, GAME), None);
    }

    #[test]
    fn redirect_lines() {
        let r = Redirect::parse("GF2\\Cuba.str | _doctor_toolkit/overlay/gf2/cuba.str").unwrap();
        assert_eq!(r.from, "gf2/cuba.str");
        assert_eq!(r.to, "_doctor_toolkit/overlay/gf2/cuba.str");
        assert!(r.relative());
        assert_eq!(Redirect::parse(&r.to_line()), Some(r));
        let abs = Redirect::parse("cmd.txt|Z:\\tmp\\cmd.txt").unwrap();
        assert!(!abs.relative());
        assert_eq!(Redirect::parse("cmd.txt"), None);
        assert_eq!(Redirect::parse("cmd.txt|"), None);
        assert_eq!(Redirect::parse("..\\x|y"), None);
        assert!(!line_safe("a#b"));
        assert!(!line_safe("a\nb"));
        assert!(line_safe("_doctor_toolkit/overlay/gf2/x.str"));
    }
}
