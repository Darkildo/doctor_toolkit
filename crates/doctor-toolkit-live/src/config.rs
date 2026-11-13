//! `doctor_live.cfg`: the configuration the installer writes next to the
//! module. Lines `key=value`; `#` starts a comment; unknown keys are kept
//! out of the way (a newer installer may write more).
//!
//! ```text
//! port=47801
//! token=4f1c…
//! writes=1
//! fix_input=1
//! ```

use crate::DEFAULT_PORT;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// TCP port on 127.0.0.1.
    pub port: u16,
    /// Secret the client sends in Hello; empty: any local client.
    pub token: String,
    /// Whether teleport, messages and Lua are allowed.
    pub writes: bool,
    /// Whether the module gives the player the keyboard of a start straight
    /// into a level (`-stream`): the game makes the player before it knows
    /// the controller, and he does not move (`docs/analysis/live-link.md`
    /// §5.1). On unless the file says `fix_input=0`.
    pub fix_input: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            token: String::new(),
            writes: false,
            fix_input: true,
        }
    }
}

impl Config {
    /// Reads the text of a configuration; bad values keep their defaults.
    pub fn parse(text: &str) -> Self {
        let mut out = Self::default();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or_default().trim();
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "port" => {
                    if let Ok(p) = value.parse::<u16>()
                        && p != 0
                    {
                        out.port = p;
                    }
                }
                "token" => out.token = value.to_owned(),
                "writes" => out.writes = matches!(value, "1" | "true" | "yes"),
                "fix_input" => out.fix_input = !matches!(value, "0" | "false" | "no"),
                _ => {}
            }
        }
        out
    }

    pub fn to_text(&self) -> String {
        format!(
            "# doctor_toolkit live link (stage 34); written by the toolkit, removed after the game exits\r\nport={}\r\ntoken={}\r\nwrites={}\r\nfix_input={}\r\n",
            self.port,
            self.token,
            u8::from(self.writes),
            u8::from(self.fix_input)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let c = Config {
            port: 50_000,
            token: "00ff".into(),
            writes: true,
            fix_input: false,
        };
        assert_eq!(Config::parse(&c.to_text()), c);
        // A file of an older installer has no fix_input: the fix is on.
        assert!(Config::parse("port=47801\nwrites=0").fix_input);
        assert_eq!(Config::parse(""), Config::default());
        let odd = Config::parse("port=0\nport = 99999\ntoken = x # comment\nwrites=no\nmore=1");
        assert_eq!(
            odd,
            Config {
                port: DEFAULT_PORT,
                token: "x".into(),
                writes: false,
                fix_input: true,
            }
        );
    }
}
