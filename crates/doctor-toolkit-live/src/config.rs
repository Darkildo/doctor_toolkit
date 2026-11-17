//! `doctor_live.cfg`: the configuration the installer writes next to the
//! module. Lines `key=value`; `#` starts a comment; unknown keys are kept
//! out of the way (a newer installer may write more).
//!
//! ```text
//! port=47801
//! token=4f1c…
//! writes=1
//! fix_input=1
//! journal=1
//! files_log=0
//! redirect=gf2/cuba.str|_doctor_toolkit/overlay/gf2/cuba.str
//! ```
//!
//! `redirect` lines (stage 46, [`crate::files`]) name files the game opens
//! from elsewhere: the path in the game folder, `|`, the file it gets.
//! `resources` lines (stage 100, [`crate::resources`]) name streams the
//! module composes in memory: the stream in the game folder, `|`, its
//! resource patch and, after another `|`, the file to compose from when
//! that is not the game's own stream.

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
    /// Whether the module records the game's journal (stage 49): it hooks
    /// what the game prints, the errors of Lua and the message bus
    /// (`docs/analysis/live-link.md` §11). On unless the file says
    /// `journal=0`.
    pub journal: bool,
    /// Whether the module logs every file of the game folder the game
    /// opens (stage 46, for finding what to redirect). Off unless the file
    /// says `files_log=1`.
    pub files_log: bool,
    /// Files the game opens from elsewhere (stage 46).
    pub redirects: Vec<crate::files::Redirect>,
    /// Streams the module composes from the game's file and a resource
    /// patch (stage 100): the stream, and its patch file.
    pub resources: Vec<crate::resources::PatchLine>,
    /// Whether the module writes every composed stream next to its patch
    /// (`<patch>.composed`, for comparing it with what "apply" writes).
    /// Off unless the file says `resources_dump=1`.
    pub resources_dump: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            token: String::new(),
            writes: false,
            fix_input: true,
            journal: true,
            files_log: false,
            redirects: Vec::new(),
            resources: Vec::new(),
            resources_dump: false,
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
                "journal" => out.journal = !matches!(value, "0" | "false" | "no"),
                "files_log" => out.files_log = matches!(value, "1" | "true" | "yes"),
                "redirect" => {
                    if out.redirects.len() < crate::files::MAX_REDIRECTS
                        && let Some(r) = crate::files::Redirect::parse(value)
                    {
                        out.redirects.push(r);
                    }
                }
                "resources" => {
                    if out.resources.len() < crate::resources::MAX_PATCHES
                        && let Some(r) = crate::resources::PatchLine::parse(value)
                    {
                        out.resources.push(r);
                    }
                }
                "resources_dump" => out.resources_dump = matches!(value, "1" | "true" | "yes"),
                _ => {}
            }
        }
        out
    }

    pub fn to_text(&self) -> String {
        let mut out = format!(
            "# doctor_toolkit live link (stage 34); written by the toolkit, removed after the game exits\r\nport={}\r\ntoken={}\r\nwrites={}\r\nfix_input={}\r\njournal={}\r\n",
            self.port,
            self.token,
            u8::from(self.writes),
            u8::from(self.fix_input),
            u8::from(self.journal)
        );
        if self.files_log {
            out.push_str("files_log=1\r\n");
        }
        if !self.redirects.is_empty() {
            out.push_str("# files the game opens from elsewhere (stage 46)\r\n");
        }
        for r in &self.redirects {
            out.push_str("redirect=");
            out.push_str(&r.to_line());
            out.push_str("\r\n");
        }
        if self.resources_dump {
            out.push_str("resources_dump=1\r\n");
        }
        if !self.resources.is_empty() {
            out.push_str("# streams composed in memory from a resource patch (stage 100)\r\n");
        }
        for r in &self.resources {
            out.push_str("resources=");
            out.push_str(&r.to_line());
            out.push_str("\r\n");
        }
        out
    }

    /// Whether the module hooks the files the game opens.
    pub fn hooks_files(&self) -> bool {
        self.files_log || !self.redirects.is_empty() || !self.resources.is_empty()
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
            journal: false,
            files_log: true,
            redirects: vec![
                crate::files::Redirect {
                    from: "gf2/cuba.str".into(),
                    to: "_doctor_toolkit/overlay/gf2/cuba.str".into(),
                },
                crate::files::Redirect {
                    from: "godfath1.viv".into(),
                    to: "GODFATH0.viv".into(),
                },
            ],
            resources: vec![
                crate::resources::PatchLine {
                    from: "gf2/cuba.str".into(),
                    to: "Z:\\cache\\overlay\\game\\gf2\\cuba.str".into(),
                    base: None,
                },
                crate::resources::PatchLine {
                    from: "gf2/miami.str".into(),
                    to: "Z:\\cache\\overlay\\game\\gf2\\miami.str".into(),
                    base: Some("_doctor_toolkit/backups/gf2/miami.str".into()),
                },
            ],
            resources_dump: true,
        };
        assert_eq!(Config::parse(&c.to_text()), c);
        assert!(c.hooks_files());
        assert!(!Config::default().hooks_files());
        // A file of an older installer has no fix_input: the fix is on.
        assert!(Config::parse("port=47801\nwrites=0").fix_input);
        assert!(Config::parse("port=47801\nwrites=0").journal);
        assert_eq!(Config::parse(""), Config::default());
        let odd = Config::parse("port=0\nport = 99999\ntoken = x # comment\nwrites=no\nmore=1");
        assert_eq!(
            odd,
            Config {
                port: DEFAULT_PORT,
                token: "x".into(),
                writes: false,
                fix_input: true,
                journal: true,
                files_log: false,
                redirects: Vec::new(),
                resources: Vec::new(),
                resources_dump: false,
            }
        );
        // A bad redirect line is left out.
        assert!(
            Config::parse("redirect=../x|y\nredirect=cmd.txt")
                .redirects
                .is_empty()
        );
    }
}
