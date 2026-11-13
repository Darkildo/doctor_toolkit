//! Small readers for the configuration files of launchers: Valve's text
//! and binary KeyValues (`libraryfolders.vdf`, `appmanifest_*.acf`,
//! `shortcuts.vdf`), Wine registry files (`system.reg`, `user.reg`), the
//! output of Windows `reg query`, and the few YAML lines of Lutris and
//! Bottles configurations. They read untrusted files: bounded, never panic.

use std::collections::BTreeMap;

/// A node of a KeyValues document: a string or a table.
#[derive(Debug, Clone, PartialEq)]
pub enum Kv {
    Str(String),
    Int(i64),
    Table(Vec<(String, Kv)>),
}

impl Kv {
    /// The first child named `key`, ignoring case.
    pub fn get(&self, key: &str) -> Option<&Kv> {
        match self {
            Self::Table(items) => items
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn int(&self) -> Option<i64> {
        match self {
            Self::Int(i) => Some(*i),
            Self::Str(s) => s.trim().parse().ok(),
            Self::Table(_) => None,
        }
    }

    pub fn children(&self) -> &[(String, Kv)] {
        match self {
            Self::Table(items) => items,
            _ => &[],
        }
    }

    /// Follows a path of keys.
    pub fn at(&self, path: &[&str]) -> Option<&Kv> {
        path.iter().try_fold(self, |node, key| node.get(key))
    }
}

/// Most nesting accepted in KeyValues documents.
const MAX_DEPTH: usize = 32;

/// Parses text KeyValues (`"key" "value"` and `"key" { … }`). Returns the
/// top level as a table; a malformed tail is dropped.
pub fn text_vdf(text: &str) -> Kv {
    let tokens = vdf_tokens(text);
    let mut pos = 0;
    Kv::Table(vdf_table(&tokens, &mut pos, 0))
}

#[derive(Debug, PartialEq)]
enum Token {
    Str(String),
    Open,
    Close,
}

fn vdf_tokens(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => tokens.push(Token::Open),
            '}' => tokens.push(Token::Close),
            '"' => {
                let mut s = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some(other) => s.push(other),
                            None => break,
                        },
                        _ => s.push(c),
                    }
                }
                tokens.push(Token::Str(s));
            }
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            c if c.is_whitespace() => {}
            c => {
                // An unquoted token.
                let mut s = String::from(c);
                while let Some(&n) = chars.peek() {
                    if n.is_whitespace() || n == '{' || n == '}' || n == '"' {
                        break;
                    }
                    s.push(n);
                    chars.next();
                }
                tokens.push(Token::Str(s));
            }
        }
    }
    tokens
}

fn vdf_table(tokens: &[Token], pos: &mut usize, depth: usize) -> Vec<(String, Kv)> {
    let mut items = Vec::new();
    while *pos < tokens.len() {
        let key = match &tokens[*pos] {
            Token::Str(s) => s.clone(),
            Token::Close => {
                *pos += 1;
                return items;
            }
            Token::Open => {
                *pos += 1;
                continue;
            }
        };
        *pos += 1;
        match tokens.get(*pos) {
            Some(Token::Str(value)) => {
                items.push((key, Kv::Str(value.clone())));
                *pos += 1;
            }
            Some(Token::Open) => {
                *pos += 1;
                if depth >= MAX_DEPTH {
                    return items;
                }
                let table = vdf_table(tokens, pos, depth + 1);
                items.push((key, Kv::Table(table)));
            }
            _ => return items,
        }
    }
    items
}

/// Parses binary KeyValues (`shortcuts.vdf`): type byte, name, value; `0`
/// opens a table, `1` a string, `2` a 32-bit integer, `8` closes a table.
pub fn binary_vdf(data: &[u8]) -> Kv {
    let mut pos = 0;
    Kv::Table(binary_table(data, &mut pos, 0))
}

fn binary_table(data: &[u8], pos: &mut usize, depth: usize) -> Vec<(String, Kv)> {
    let mut items = Vec::new();
    while let Some(&kind) = data.get(*pos) {
        *pos += 1;
        if kind == 8 {
            return items;
        }
        let Some(name) = c_string(data, pos) else {
            return items;
        };
        match kind {
            0 => {
                if depth >= MAX_DEPTH {
                    return items;
                }
                let table = binary_table(data, pos, depth + 1);
                items.push((name, Kv::Table(table)));
            }
            1 => {
                let Some(value) = c_string(data, pos) else {
                    return items;
                };
                items.push((name, Kv::Str(value)));
            }
            2 => {
                let Some(bytes) = data.get(*pos..*pos + 4) else {
                    return items;
                };
                *pos += 4;
                let value = u32::from_le_bytes(bytes.try_into().expect("four bytes"));
                items.push((name, Kv::Int(i64::from(value))));
            }
            _ => return items,
        }
    }
    items
}

fn c_string(data: &[u8], pos: &mut usize) -> Option<String> {
    let rest = data.get(*pos..)?;
    let end = rest.iter().position(|&b| b == 0)?;
    *pos += end + 1;
    Some(String::from_utf8_lossy(&rest[..end]).into_owned())
}

/// Keys and their values of a Wine registry file, key paths in lower case
/// with single backslashes (`software\\wow6432node\\ea games\\the godfather ii`),
/// value names as written (`@` is the default value). Only string values.
pub fn wine_reg(text: &str) -> BTreeMap<String, Vec<(String, String)>> {
    let mut keys: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix('[') {
            let Some(end) = rest.rfind(']') else {
                current = None;
                continue;
            };
            current = Some(unescape_reg(&rest[..end]).to_lowercase());
            continue;
        }
        let Some(key) = &current else { continue };
        let (name, value) = if let Some(rest) = line.strip_prefix('"') {
            let Some((name, rest)) = split_quoted(rest) else {
                continue;
            };
            (name, rest)
        } else if let Some(rest) = line.strip_prefix('@') {
            ("@".to_owned(), rest)
        } else {
            continue;
        };
        let Some(value) = value.strip_prefix("=\"") else {
            continue;
        };
        let Some((value, _)) = split_quoted(value) else {
            continue;
        };
        keys.entry(key.clone()).or_default().push((name, value));
    }
    keys
}

/// Splits `text"rest` at the closing quote, unescaping `\\` and `\"`.
fn split_quoted(text: &str) -> Option<(String, &str)> {
    let mut out = String::new();
    let mut chars = text.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, &text[i + 1..])),
            '\\' => {
                if let Some((_, next)) = chars.next() {
                    out.push(next);
                }
            }
            _ => out.push(c),
        }
    }
    None
}

fn unescape_reg(text: &str) -> String {
    text.replace("\\\\", "\\")
}

/// Values in the output of `reg query KEY /s`: `(key, name, data)` for
/// `REG_SZ` and `REG_EXPAND_SZ` values, key paths in lower case.
pub fn reg_query(output: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut key = String::new();
    for line in output.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with("HKEY_") {
            key = line.trim().to_lowercase();
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.len() == line.len() {
            continue;
        }
        for kind in ["REG_EXPAND_SZ", "REG_SZ"] {
            let marker = format!("    {kind}    ");
            if let Some(at) = trimmed.find(&marker) {
                let name = trimmed[..at].trim().to_owned();
                let data = trimmed[at + marker.len()..].trim().to_owned();
                out.push((key.clone(), name, data));
                break;
            }
        }
    }
    out
}

/// The values of `key:` in a simple YAML file (Lutris games, Bottles), in
/// order, with `(indent, value)`: unquoted, a plain value folded over more
/// lines joined with spaces (Lutris wraps long paths).
pub fn yaml_entries(text: &str, key: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim_start().trim_start_matches("- ");
        let Some(rest) = trimmed.strip_prefix(key) else {
            continue;
        };
        let Some(value) = rest.strip_prefix(':') else {
            continue;
        };
        let mut value = value.trim().to_owned();
        let quoted = value.starts_with(['"', '\'']);
        if !quoted && !value.is_empty() {
            // Continuation lines: deeper than the key and not a key themselves.
            for next in &lines[i + 1..] {
                let next_indent = next.len() - next.trim_start().len();
                let next = next.trim();
                if next.is_empty()
                    || next_indent <= indent
                    || next.starts_with("- ")
                    || next.ends_with(':')
                    || next.contains(": ")
                {
                    break;
                }
                value.push(' ');
                value.push_str(next);
            }
        }
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(&value)
            .to_owned();
        if !value.is_empty() {
            out.push((indent, value));
        }
    }
    out
}

/// The values of `key:` at any depth; see [`yaml_entries`].
pub fn yaml_values(text: &str, key: &str) -> Vec<String> {
    yaml_entries(text, key)
        .into_iter()
        .map(|(_, v)| v)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_text_keyvalues() {
        let text = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"/home/u/.local/share/Steam"
		"apps" { "228980" "1" "24830" "8000" }
	}
	"1" { "path" "/mnt/games/Steam\\Lib" }  // comment
}"#;
        let kv = text_vdf(text);
        let folders = kv.get("libraryfolders").unwrap();
        assert_eq!(folders.children().len(), 2);
        assert_eq!(
            folders.at(&["0", "path"]).and_then(Kv::str),
            Some("/home/u/.local/share/Steam")
        );
        assert_eq!(
            folders.at(&["0", "apps", "24830"]).and_then(Kv::int),
            Some(8000)
        );
        assert_eq!(
            folders.at(&["1", "path"]).and_then(Kv::str),
            Some("/mnt/games/Steam\\Lib")
        );
        // Broken input does not panic.
        let _ = text_vdf("\"a\" { \"b\" { { } } } } \"");
        let deep = "\"a\" {".repeat(1000);
        let _ = text_vdf(&deep);
    }

    #[test]
    fn reads_binary_keyvalues() {
        let mut data = vec![0u8];
        data.extend_from_slice(b"shortcuts\0");
        data.push(0);
        data.extend_from_slice(b"0\0");
        data.push(2);
        data.extend_from_slice(b"appid\0");
        data.extend_from_slice(&0x8123_4567u32.to_le_bytes());
        data.push(1);
        data.extend_from_slice(b"Exe\0\"/g/godfather2.exe\"\0");
        data.extend_from_slice(&[8, 8, 8]);
        let kv = binary_vdf(&data);
        let entry = kv.at(&["shortcuts", "0"]).unwrap();
        assert_eq!(entry.get("appid").and_then(Kv::int), Some(0x8123_4567));
        assert_eq!(
            entry.get("exe").and_then(Kv::str),
            Some("\"/g/godfather2.exe\"")
        );
        for cut in 0..data.len() {
            let _ = binary_vdf(&data[..cut]);
        }
    }

    #[test]
    fn reads_wine_registry_files() {
        let text = "WINE REGISTRY Version 2\n\
            [Software\\\\Wow6432Node\\\\EA Games\\\\The Godfather II] 1700000000\n\
            #time=1d\n\
            \"DisplayName\"=\"The Godfather\\u2122 II\"\n\
            \"Install Dir\"=\"C:\\\\Program Files (x86)\\\\EA Games\\\\The Godfather II\\\\\"\n\
            @=\"default\"\n\
            \"Number\"=dword:00000001\n";
        let keys = wine_reg(text);
        let values = &keys["software\\wow6432node\\ea games\\the godfather ii"];
        assert!(values.contains(&(
            "Install Dir".to_owned(),
            "C:\\Program Files (x86)\\EA Games\\The Godfather II\\".to_owned()
        )));
        assert!(values.contains(&("@".to_owned(), "default".to_owned())));
        assert_eq!(values.len(), 3);
    }

    #[test]
    fn reads_reg_query_output() {
        let output = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\WOW6432Node\\EA Games\\The Godfather II\r\n    \
            DisplayName    REG_SZ    The Godfather II\r\n    \
            Install Dir    REG_SZ    D:\\Games\\The Godfather II\\\r\n    \
            Locale    REG_DWORD    0x1\r\n\r\n";
        let values = reg_query(output);
        assert_eq!(values.len(), 2);
        assert_eq!(values[1].1, "Install Dir");
        assert_eq!(values[1].2, "D:\\Games\\The Godfather II\\");
        assert!(values[1].0.ends_with("the godfather ii"));
    }

    #[test]
    fn reads_yaml_values() {
        let text = "game:\n  exe: \"/home/u/Games/gf2/drive_c/GF2/godfather2.exe\"\n  prefix: /home/u/Games/gf2\nwine:\n  version: lutris\n";
        assert_eq!(
            yaml_values(text, "exe"),
            ["/home/u/Games/gf2/drive_c/GF2/godfather2.exe"]
        );
        assert_eq!(yaml_values(text, "prefix"), ["/home/u/Games/gf2"]);
        assert!(yaml_values(text, "missing").is_empty());
        // Lutris folds long paths.
        let text = "game:\n  exe: /home/u/Games/gf2/drive_c/EA Games/The Godfather\n    II/godfather2.exe\n  prefix: /p\nslug: gf2-setup\ngame_slug: gf2\n";
        assert_eq!(
            yaml_values(text, "exe"),
            ["/home/u/Games/gf2/drive_c/EA Games/The Godfather II/godfather2.exe"]
        );
        assert_eq!(yaml_entries(text, "slug"), [(0, "gf2-setup".to_owned())]);
    }
}
