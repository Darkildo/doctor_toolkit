//! The language of the game.
//!
//! The game (1.0.789.0) takes it from the line `Locale = xx` of
//! `%LOCALAPPDATA%\EA Games\<title>\settings.txt`, which it writes itself
//! once it has run; while the line is absent, from the value `Locale` of
//! `HKLM\SOFTWARE\EA Games\The Godfather II` (the 32-bit view). The line of
//! the file wins over the registry, so both are set.

use std::path::PathBuf;

use crate::{Error, Result};

/// Registry key the game reads, under `HKLM` (32-bit view).
pub const REGISTRY_KEY: &str = r"SOFTWARE\EA Games\The Godfather II";

/// A language of the unified game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Language {
    /// What goes into `Locale`: the game cuts it to two letters.
    pub code: &'static str,
    pub name: &'static str,
    /// Whether the speech is in this language too (otherwise English).
    pub speech: bool,
}

/// The languages of the unified game, in the order of its language table.
pub const LANGUAGES: &[Language] = &[
    Language {
        code: "en",
        name: "English",
        speech: true,
    },
    Language {
        code: "es",
        name: "Español",
        speech: true,
    },
    Language {
        code: "ru",
        name: "Русский",
        speech: true,
    },
    Language {
        code: "cs",
        name: "Čeština",
        speech: false,
    },
    Language {
        code: "nl",
        name: "Nederlands",
        speech: false,
    },
    Language {
        code: "pl",
        name: "Polski",
        speech: false,
    },
];

pub fn language(code: &str) -> Option<&'static Language> {
    LANGUAGES.iter().find(|l| l.code.eq_ignore_ascii_case(code))
}

/// The `settings.txt` files of the game of this user.
pub fn settings_files() -> Vec<PathBuf> {
    let Some(root) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) else {
        return Vec::new();
    };
    let Ok(titles) = std::fs::read_dir(root.join("EA Games")) else {
        return Vec::new();
    };
    titles
        .filter_map(|e| e.ok())
        .map(|e| e.path().join("settings.txt"))
        .filter(|p| p.is_file())
        .collect()
}

/// `text` with its `Locale = …` line set to `code`; `None` when it has no
/// such line (the game then asks the registry).
pub fn with_locale(text: &str, code: &str) -> Option<String> {
    if !text.lines().any(|l| l.starts_with("Locale = ")) {
        return None;
    }
    Some(
        text.split_inclusive('\n')
            .map(|line| {
                if line.starts_with("Locale = ") {
                    let end = if line.ends_with("\r\n") {
                        "\r\n"
                    } else if line.ends_with('\n') {
                        "\n"
                    } else {
                        ""
                    };
                    format!("Locale = {code} {end}")
                } else {
                    line.to_string()
                }
            })
            .collect(),
    )
}

/// The command that sets the registry value, for people who run the game
/// under Wine: there the tool cannot know the prefix.
pub fn registry_command(code: &str) -> String {
    format!(r#"reg add "HKLM\{REGISTRY_KEY}" /v Locale /t REG_SZ /d {code} /f /reg:32"#)
}

#[cfg(windows)]
fn set_registry(code: &str) -> Result<()> {
    use std::process::Command;
    let key = format!(r"HKLM\{REGISTRY_KEY}");
    let args = [
        "add",
        key.as_str(),
        "/v",
        "Locale",
        "/t",
        "REG_SZ",
        "/d",
        code,
        "/f",
        "/reg:32",
    ];
    let direct = Command::new("reg").args(args).output();
    if direct.is_ok_and(|o| o.status.success()) {
        return Ok(());
    }
    // Writing under HKLM needs administrator rights: ask for them.
    let list = args
        .iter()
        .map(|a| format!("'\"{a}\"'"))
        .collect::<Vec<_>>()
        .join(",");
    let script = format!(
        "$p = Start-Process reg -ArgumentList {list} -Verb RunAs -Wait -PassThru -WindowStyle Hidden; exit $p.ExitCode"
    );
    let elevated = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .status()
        .map_err(|e| Error::io("powershell", e))?;
    if elevated.success() {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!(
            "the registry value could not be written; run as administrator: {}",
            registry_command(code)
        )))
    }
}

/// Sets the language of the game for this user; returns what was done, for
/// people. On Windows: the registry value and every `settings.txt` that has
/// the line. Elsewhere the game runs under Wine in a prefix the tool does
/// not know: the answer is the command to run there.
pub fn set_language(code: &str) -> Result<String> {
    let language = language(code).ok_or_else(|| {
        Error::InvalidArgument(format!(
            "unknown language `{code}`; the unified game has {}",
            LANGUAGES
                .iter()
                .map(|l| l.code)
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })?;
    let mut done = Vec::new();
    for path in settings_files() {
        let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
        if let Some(new) = with_locale(&text, language.code) {
            crate::support::atomic::write(&path, new.as_bytes())?;
            done.push(path.display().to_string());
        }
    }
    #[cfg(windows)]
    {
        set_registry(language.code)?;
        done.push(format!(r"HKLM\{REGISTRY_KEY}"));
        Ok(format!("{}: {}", language.name, done.join(", ")))
    }
    #[cfg(not(windows))]
    {
        let _ = done;
        Ok(format!(
            "{}: in the Wine prefix of the game run  wine {}  and, when the game has run before, set \
             `Locale = {}` in its settings.txt (AppData/Local/EA Games/…)",
            language.name,
            registry_command(language.code),
            language.code
        ))
    }
}
