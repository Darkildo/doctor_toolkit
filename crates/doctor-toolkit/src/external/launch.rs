//! Starting the program that edits a working file.
//!
//! A command comes from the settings (`[external_editors]`) or from
//! `edit --with`: a program and its arguments, split like a shell does
//! (spaces, single and double quotes, backslash), without a shell. `{file}`
//! stands for the working file; a command without it gets the file as the
//! last argument. An empty command opens the file with the desktop's
//! program for its type (`xdg-open`, `open`, `explorer`).

use std::path::Path;
use std::process::{Child, Command, Stdio};

use crate::{Error, Result};

/// Splits a command line into words: whitespace separates, quotes group,
/// a backslash outside single quotes takes the next character as it is.
pub fn split_command(line: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => word.push(c),
                        None => return Err(unclosed(line)),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\')) => word.push(c),
                            Some(c) => {
                                word.push('\\');
                                word.push(c);
                            }
                            None => return Err(unclosed(line)),
                        },
                        Some(c) => word.push(c),
                        None => return Err(unclosed(line)),
                    }
                }
            }
            // Windows paths keep their backslashes.
            '\\' if !cfg!(windows) => {
                in_word = true;
                if let Some(c) = chars.next() {
                    word.push(c);
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

fn unclosed(line: &str) -> Error {
    Error::InvalidArgument(format!("unclosed quote in the command `{line}`"))
}

/// The program and arguments that open `file` with `command`.
pub fn command_words(command: &str, file: &Path) -> Result<Vec<String>> {
    let mut words = split_command(command)?;
    if words.is_empty() {
        return Err(Error::InvalidArgument("the editor command is empty".into()));
    }
    let file = file.to_string_lossy();
    let mut used = false;
    for word in &mut words[1..] {
        if word.contains("{file}") {
            *word = word.replace("{file}", &file);
            used = true;
        }
    }
    if !used {
        words.push(file.into_owned());
    }
    Ok(words)
}

/// Starts `command` (empty: the desktop's program) on `file`. The program
/// runs on its own; its input is empty and its output is discarded.
pub fn launch(command: &str, file: &Path) -> Result<Child> {
    if command.trim().is_empty() {
        return open_with_desktop(file.as_os_str());
    }
    let words = command_words(command, file)?;
    spawn(Command::new(&words[0]).args(&words[1..]))
        .map_err(|e| Error::InvalidArgument(format!("cannot start `{}`: {e}", words[0])))
}

/// Opens a file, folder or address with the desktop's default program.
pub fn open_with_desktop(target: &std::ffi::OsStr) -> Result<Child> {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    spawn(Command::new(program).arg(target))
        .map_err(|e| Error::InvalidArgument(format!("cannot start {program}: {e}")))
}

fn spawn(command: &mut Command) -> std::io::Result<Child> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_split_like_a_shell() {
        assert_eq!(split_command("  gimp   -n  ").unwrap(), ["gimp", "-n"]);
        assert_eq!(
            split_command(r#"flatpak run "org.kde.krita" 'a b' "c\"d" e\ f"#).unwrap(),
            ["flatpak", "run", "org.kde.krita", "a b", "c\"d", "e f"]
        );
        assert_eq!(split_command(r#"x "" y"#).unwrap(), ["x", "", "y"]);
        assert!(split_command("x 'open").is_err());
    }

    #[test]
    fn the_file_goes_where_the_command_says() {
        let file = Path::new("/tmp/a b/tex.png");
        assert_eq!(
            command_words("gimp", file).unwrap(),
            ["gimp", "/tmp/a b/tex.png"]
        );
        assert_eq!(
            command_words("blender --python-expr 'f=r\"{file}\"'", file).unwrap(),
            ["blender", "--python-expr", "f=r\"/tmp/a b/tex.png\""]
        );
        assert!(command_words("   ", file).is_err());
    }
}
