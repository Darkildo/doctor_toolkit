//! `scripts/doctor_cheats.cfg`, next to the module: the key of the menu,
//! the language, the gamepad. Lines `name = value`, `#` starts a comment;
//! an unknown line is left alone. Without the file — the defaults.

use crate::text::Lang;

/// The file name, next to `doctor_cheats.asi`.
pub const FILE: &str = "doctor_cheats.cfg";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// DirectInput scan code of the key that opens the menu (`DIK_GRAVE`,
    /// `~`, the same key in any layout).
    pub key: u8,
    /// `None`: the language of the game.
    pub language: Option<Lang>,
    /// The gamepad opens the menu with Back + RB and drives it.
    pub pad: bool,
    /// The values of the menu are kept between starts of the game.
    pub remember: bool,
    /// The kept values: `name = on|off|number` (names of
    /// [`crate::menu::remembered`]).
    pub values: Vec<(String, String)>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            key: doctor_sdk::abi::KEY_GRAVE,
            language: None,
            pad: true,
            remember: false,
            values: Vec::new(),
        }
    }
}

/// Names of keys a player may give instead of a scan code.
const KEYS: &[(&str, u8)] = &[
    ("grave", 0x29),
    ("tilde", 0x29),
    ("f1", 0x3B),
    ("f2", 0x3C),
    ("f3", 0x3D),
    ("f4", 0x3E),
    ("f5", 0x3F),
    ("f6", 0x40),
    ("f7", 0x41),
    ("f8", 0x42),
    ("f9", 0x43),
    ("f10", 0x44),
    ("f11", 0x57),
    ("f12", 0x58),
    ("insert", 0xD2),
    ("home", 0xC7),
    ("end", 0xCF),
    ("pageup", 0xC9),
    ("pagedown", 0xD1),
];

impl Config {
    pub fn parse(text: &str) -> Self {
        let mut c = Self::default();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((name, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().to_ascii_lowercase();
            match name.trim().to_ascii_lowercase().as_str() {
                "key" => {
                    let code = KEYS
                        .iter()
                        .find(|(n, _)| *n == value)
                        .map(|(_, k)| *k)
                        .or_else(|| match value.strip_prefix("0x") {
                            Some(hex) => u8::from_str_radix(hex, 16).ok(),
                            None => value.parse().ok(),
                        });
                    if let Some(code) = code.filter(|&k| k != 0) {
                        c.key = code;
                    }
                }
                "language" => {
                    c.language = match value.as_str() {
                        "en" => Some(Lang::En),
                        "ru" => Some(Lang::Ru),
                        _ => None,
                    }
                }
                "pad" => c.pad = !matches!(value.as_str(), "off" | "no" | "0" | "false"),
                "remember" => c.remember = matches!(value.as_str(), "on" | "yes" | "1" | "true"),
                other => {
                    if crate::menu::remembered().iter().any(|(n, _)| *n == other) {
                        c.values.push((other.to_owned(), value));
                    }
                }
            }
        }
        c
    }
}

impl Config {
    /// The file with the settings and, when `remember` is on, the values;
    /// the lines of the player not about values are kept.
    pub fn write(&self, old: &str) -> String {
        let names: Vec<&str> = crate::menu::remembered().iter().map(|(n, _)| *n).collect();
        let mut out = String::new();
        for line in old.lines() {
            let name = line
                .split('#')
                .next()
                .and_then(|l| l.split_once('='))
                .map(|(n, _)| n.trim().to_ascii_lowercase());
            if let Some(name) = name
                && (name == "remember" || names.contains(&name.as_str()))
            {
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
        if out.trim().is_empty() {
            out = "# doctor_cheats: key = grave|f8|…, language = auto|en|ru, pad = on|off\n".into();
        }
        out.push_str(if self.remember {
            "remember = on\n"
        } else {
            "remember = off\n"
        });
        if self.remember {
            for (name, value) in &self.values {
                out.push_str(&format!("{name} = {value}\n"));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_are_read_and_the_rest_left_alone() {
        assert_eq!(Config::parse(""), Config::default());
        let c = Config::parse("# mine\nkey = F8\nlanguage = ru  # always\npad = off\nfoo = bar\n");
        assert_eq!(c.key, 0x42);
        assert_eq!(c.language, Some(Lang::Ru));
        assert!(!c.pad);
        assert_eq!(Config::parse("key = 0x3b").key, 0x3B);
        assert_eq!(Config::parse("key = 41").key, 41);
        assert_eq!(Config::parse("key = nonsense").key, 0x29);
        assert_eq!(Config::parse("language = auto").language, None);
    }

    #[test]
    fn the_values_are_kept_only_when_asked() {
        let c = Config::parse("key = f8\nremember = on\ngod = on\ndamage = 2.5\nhour = 3\n");
        assert!(c.remember);
        assert_eq!(
            c.values,
            vec![("god".into(), "on".into()), ("damage".into(), "2.5".into())]
        );
        let mut c2 = c.clone();
        c2.values = vec![("ammo".into(), "on".into())];
        let text = c2.write("# mine\nkey = f8\nremember = on\ngod = on\n");
        assert_eq!(text, "# mine\nkey = f8\nremember = on\nammo = on\n");
        c2.remember = false;
        assert_eq!(c2.write(&text), "# mine\nkey = f8\nremember = off\n");
        assert!(Config::default().write("").starts_with("# doctor_cheats"));
    }
}
