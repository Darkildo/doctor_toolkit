//! Diagnostics for users and bug reports: the version line, a log file,
//! a panic hook that writes to it, hints for common errors and their exit codes.
//!
//! The log is `$XDG_STATE_HOME/doctor-toolkit/log.txt`
//! (`~/.local/state/doctor-toolkit/log.txt`; `%LOCALAPPDATA%\doctor-toolkit\log.txt`
//! on Windows), or `$DOCTOR_TOOLKIT_LOG_DIR/log.txt`. It is appended to and
//! renamed to `log.old.txt` once it exceeds [`MAX_LOG_SIZE`]. Logging never
//! fails loudly: a log that cannot be written is skipped.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::Error;
use crate::game::GameInstance;

/// Version of the library and of every program built from this workspace.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The log is rotated once it grows beyond this many bytes.
pub const MAX_LOG_SIZE: u64 = 1 << 20;

/// Name of the program writing to the log, set by [`init`].
static PROGRAM: OnceLock<String> = OnceLock::new();
/// Serialises writes of the threads of one process.
static LOCK: Mutex<()> = Mutex::new(());

/// `doctor-toolkit 0.1.0 (linux x86_64)`.
pub fn version_line(program: &str) -> String {
    format!(
        "{program} {VERSION} ({} {})",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// Directory of the log file; `None` when no home directory is known.
pub fn log_dir() -> Option<PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = var("DOCTOR_TOOLKIT_LOG_DIR") {
        return Some(dir);
    }
    if cfg!(windows) {
        return var("LOCALAPPDATA").map(|d| d.join("doctor-toolkit"));
    }
    var("XDG_STATE_HOME")
        .or_else(|| var("HOME").map(|home| home.join(".local").join("state")))
        .map(|d| d.join("doctor-toolkit"))
}

/// Path of the log file.
pub fn log_path() -> Option<PathBuf> {
    log_dir().map(|dir| dir.join("log.txt"))
}

/// Starts logging for `program`: records the version, the platform and the
/// command line, and installs a panic hook that writes panics to the log.
pub fn init(program: &str) {
    if PROGRAM.set(program.to_owned()).is_err() {
        return;
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    log(&format!("start: {} args: {args:?}", version_line(program)));
    install_panic_hook();
}

/// Appends one line to the log.
pub fn log(message: &str) {
    let Some(path) = log_path() else {
        return;
    };
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _ = append(&path, message);
}

fn append(path: &Path, message: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    if fs::metadata(path).is_ok_and(|m| m.len() > MAX_LOG_SIZE) {
        fs::rename(path, path.with_file_name("log.old.txt"))?;
    }
    let program = PROGRAM.get().map_or("doctor-toolkit", String::as_str);
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let mut text = String::new();
    for (i, line) in message.lines().enumerate() {
        if i == 0 {
            text.push_str(&format!(
                "{} [{program} {}] {line}\n",
                timestamp(SystemTime::now()),
                std::process::id()
            ));
        } else {
            text.push_str(&format!("    {line}\n"));
        }
    }
    file.write_all(text.as_bytes())
}

/// Records the game a program works on: directory, profile and executable,
/// whose size and SHA-256 tell game versions apart.
pub fn log_game(game: &GameInstance) {
    log(&format!("game: {}", game_summary(game)));
}

/// One line describing the game for bug reports.
pub fn game_summary(game: &GameInstance) -> String {
    let profile = game.profile();
    let exe = fs::read_dir(game.root()).ok().and_then(|entries| {
        entries
            .filter_map(|e| e.ok())
            .find(|e| {
                e.file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(profile.executable)
            })
            .map(|e| e.path())
    });
    let exe = match exe.map(|path| fs::read(&path)) {
        Some(Ok(bytes)) => format!(
            "{} {} bytes sha256 {}",
            profile.executable,
            bytes.len(),
            &crate::workspace::store::sha256_hex(&bytes)[..16]
        ),
        _ => format!("{} not found", profile.executable),
    };
    let skipped = match game.skipped().len() {
        0 => String::new(),
        n => format!("; {n} other .str/.toc file(s) skipped"),
    };
    format!(
        "{} at {}: {exe}; {} streams, {} tables of contents{skipped}",
        profile.name,
        game.root().display(),
        game.streams().len(),
        game.tocs().len()
    )
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        log(&format!("panic: {info}\n{backtrace}"));
        previous(info);
        if let Some(path) = log_path() {
            eprintln!(
                "This is a bug in doctor_toolkit. Details are in {}; please attach it to a bug report.",
                path.display()
            );
        }
    }));
}

/// `2026-11-12T10:15:00Z`.
pub fn timestamp(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Days since 1970-01-01 to a civil date (H. Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

/// Kinds of failure a user can do something about, with their exit codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// The directory is not a game directory, or a file does not exist. Exit code 3.
    NotFound,
    /// No permission to read or write a file. Exit code 4.
    PermissionDenied,
    /// The disk is full. Exit code 5.
    DiskFull,
    /// A file is in use by another program, usually the running game. Exit code 6.
    Busy,
    /// The file system is read-only. Exit code 4.
    ReadOnly,
    /// A file is damaged or not of the expected format. Exit code 7.
    BadData,
}

impl Problem {
    /// Exit code of the command line programs for this problem; 1 is any other error.
    pub fn exit_code(self) -> u8 {
        match self {
            Self::NotFound => 3,
            Self::PermissionDenied | Self::ReadOnly => 4,
            Self::DiskFull => 5,
            Self::Busy => 6,
            Self::BadData => 7,
        }
    }

    /// What the user can do about it.
    pub fn hint(self) -> &'static str {
        match self {
            Self::NotFound => {
                "check the path. The game directory is the folder with the game executable \
                 (godfather2.exe) and the gf2 folder"
            }
            Self::PermissionDenied => {
                "you have no permission for this file. A game installed by another user or \
                 through Steam/Proton may need its folder made writable \
                 (chmod -R u+w on the game directory) or a copy of the game"
            }
            Self::ReadOnly => {
                "the file system is read-only. Copy the game to a writable disk or remount it"
            }
            Self::DiskFull => {
                "the disk is full. Free some space: writing a stream needs room for a full copy \
                 of it next to the original"
            }
            Self::Busy => {
                "the file is in use. Close the game (and other tools that opened its files) \
                 and try again"
            }
            Self::BadData => {
                "the file is damaged or not of the expected kind. If it is a game file, \
                 `doctor-toolkit integrity` tells who changed it and `restore` puts the \
                 original back"
            }
        }
    }
}

/// Classifies an error chain: the first library or I/O error in it decides.
pub fn classify(error: &(dyn std::error::Error + 'static)) -> Option<Problem> {
    let mut current = Some(error);
    while let Some(e) = current {
        if let Some(problem) = e.downcast_ref::<Error>().and_then(classify_library) {
            return Some(problem);
        }
        if let Some(io) = e.downcast_ref::<std::io::Error>() {
            return classify_io(io);
        }
        current = e.source();
    }
    None
}

fn classify_library(error: &Error) -> Option<Problem> {
    match error {
        Error::Io { source, .. } => classify_io(source),
        Error::NotAGame { .. } => Some(Problem::NotFound),
        Error::UnexpectedEof { .. }
        | Error::BadMagic { .. }
        | Error::Malformed { .. }
        | Error::Refpack(_) => Some(Problem::BadData),
        _ => None,
    }
}

fn classify_io(error: &std::io::Error) -> Option<Problem> {
    use std::io::ErrorKind;
    Some(match error.kind() {
        ErrorKind::NotFound => Problem::NotFound,
        ErrorKind::PermissionDenied => Problem::PermissionDenied,
        ErrorKind::StorageFull | ErrorKind::QuotaExceeded => Problem::DiskFull,
        ErrorKind::ReadOnlyFilesystem => Problem::ReadOnly,
        ErrorKind::ResourceBusy | ErrorKind::ExecutableFileBusy => Problem::Busy,
        // ERROR_SHARING_VIOLATION and ERROR_LOCK_VIOLATION: a file open in the game.
        _ if cfg!(windows) && matches!(error.raw_os_error(), Some(32 | 33)) => Problem::Busy,
        _ => return None,
    })
}

/// Process ids of running copies of the game, natively or under Wine and
/// Proton; see [`crate::play::game_processes`].
pub fn running_game_processes() -> Vec<u32> {
    crate::play::game_processes()
}

/// A warning for programs about to write into the game while it runs.
pub fn game_running_warning() -> Option<String> {
    let pids = running_game_processes();
    (!pids.is_empty()).then(|| {
        format!(
            "the game is running (process {}); close it before changing its files. \
             A running game keeps the old data and may crash when it loads more",
            pids.iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timestamps_are_utc_dates() {
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        let t = UNIX_EPOCH + Duration::from_secs(1_794_441_600 + 3723);
        assert_eq!(timestamp(t), "2026-11-12T01:02:03Z");
        let leap = UNIX_EPOCH + Duration::from_secs(951_782_400);
        assert_eq!(timestamp(leap), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn errors_are_classified() {
        let io = |kind| Error::io("/x", std::io::Error::from(kind));
        assert_eq!(
            classify(&io(std::io::ErrorKind::PermissionDenied)),
            Some(Problem::PermissionDenied)
        );
        assert_eq!(
            classify(&io(std::io::ErrorKind::StorageFull)),
            Some(Problem::DiskFull)
        );
        assert_eq!(
            classify(&Error::NotAGame { path: "/x".into() }),
            Some(Problem::NotFound)
        );
        assert_eq!(
            classify(&Error::malformed("x", 0, "y")),
            Some(Problem::BadData)
        );
        assert_eq!(classify(&Error::InvalidArgument("x".into())), None);
        assert_eq!(Problem::DiskFull.exit_code(), 5);
    }

    #[test]
    fn log_lines_are_appended_and_rotated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.txt");
        append(&path, "first\nsecond line").unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("] first\n    second line\n"), "{text}");
        fs::write(&path, vec![b'x'; MAX_LOG_SIZE as usize + 1]).unwrap();
        append(&path, "after rotation").unwrap();
        assert!(dir.path().join("log.old.txt").is_file());
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("after rotation")
        );
    }

    #[test]
    fn this_process_is_not_the_game() {
        assert!(!running_game_processes().contains(&std::process::id()));
    }
}
