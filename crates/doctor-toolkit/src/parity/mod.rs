//! The table of what the toolkit can do and where: every capability with
//! the MCP tools, the editor actions, the commands of the command line and
//! the Python functions that give it (`data/toolkit/capabilities.toml`).
//!
//! The table is the one source: the tests of the MCP server, the editor and
//! the command line compare it with their registries of tools and commands
//! ([`Table::check_names`]), and `doctor-toolkit parity-gen` writes
//! `docs/parity.md` from it ([`document`]).

pub mod document;

use std::collections::{BTreeSet, HashSet};
use std::sync::OnceLock;

use serde::Deserialize;

static TABLE: &str = include_str!("../../../../data/toolkit/capabilities.toml");

/// Path of the table from the repository root.
pub const TABLE_FILE: &str = "data/toolkit/capabilities.toml";

/// A way to reach the toolkit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Mcp,
    Editor,
    Cli,
    Python,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Mcp, Kind::Editor, Kind::Cli, Kind::Python];

    /// The name in messages of the checks.
    pub fn name(self) -> &'static str {
        match self {
            Self::Mcp => "mcp",
            Self::Editor => "editor",
            Self::Cli => "cli",
            Self::Python => "python",
        }
    }
}

/// How much of a capability a surface gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Full,
    /// There, but not all of it: [`Surface::partial`] says what is missing.
    Partial,
    /// Not there.
    Absent,
    /// Not there on purpose: [`Surface::never`] says why.
    Never,
}

/// A rough amount of work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub enum Size {
    #[serde(rename = "S")]
    Small,
    #[serde(rename = "M")]
    Medium,
    #[serde(rename = "L")]
    Large,
}

/// What closing a hole takes.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gap {
    /// What to add.
    pub add: String,
    pub size: Size,
    /// One line of why the size is so.
    pub why: String,
    /// What it depends on.
    #[serde(default)]
    pub needs: String,
    /// Whether checking it takes the game bench.
    #[serde(default)]
    pub bench: bool,
    /// Key of the work package ([`Package::key`]).
    pub package: String,
}

/// One capability on one surface.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Surface {
    /// Names the checks compare with the code: MCP tools, command keys of
    /// the editor (`cmd.*`, `wizard.<slug>`), command paths of the command
    /// line (`level add`), Python names (`Workspace.undo`, `logic_graph`).
    #[serde(default, rename = "use")]
    pub names: Vec<String>,
    /// Where else it is, in words: a panel and its button, an MCP resource.
    #[serde(default)]
    pub at: String,
    /// The editor: how the action is given — `menu`, `form` (fields and
    /// buttons), `key`, `mouse` (only by the pointer: gizmo, drag, colour
    /// button),
    /// or two of them joined with `+`.
    #[serde(default)]
    pub input: String,
    /// The editor: the preset that does it without hands.
    #[serde(default)]
    pub preset: String,
    /// What is missing when the surface gives only a part.
    #[serde(default)]
    pub partial: String,
    /// Why the surface does not have it on purpose.
    #[serde(default)]
    pub never: String,
    pub gap: Option<Gap>,
}

impl Surface {
    /// Whether anything of the capability is there.
    pub fn present(&self) -> bool {
        !self.names.is_empty() || !self.at.is_empty()
    }

    pub fn status(&self) -> Status {
        if !self.never.is_empty() {
            Status::Never
        } else if !self.present() {
            Status::Absent
        } else if !self.partial.is_empty() {
            Status::Partial
        } else {
            Status::Full
        }
    }

    /// Whether the editor action can only be given with the pointer.
    pub fn mouse_only(&self) -> bool {
        self.input == "mouse"
    }
}

/// One thing a user can do with the toolkit.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    pub id: String,
    /// Key of its [`Area`].
    pub area: String,
    pub title: String,
    #[serde(default)]
    pub mcp: Surface,
    #[serde(default)]
    pub editor: Surface,
    #[serde(default)]
    pub cli: Surface,
    #[serde(default)]
    pub python: Surface,
}

impl Capability {
    pub fn surface(&self, kind: Kind) -> &Surface {
        match kind {
            Kind::Mcp => &self.mcp,
            Kind::Editor => &self.editor,
            Kind::Cli => &self.cli,
            Kind::Python => &self.python,
        }
    }
}

/// A group of capabilities.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Area {
    pub key: String,
    pub title: String,
}

/// A package of work of the stages that close the holes.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub key: String,
    /// The stage of the plan: 103 (the editor catches up) or 104 (MCP does).
    pub stage: u32,
    pub title: String,
    pub size: Size,
    pub why: String,
    #[serde(default)]
    pub needs: String,
    #[serde(default)]
    pub bench: bool,
}

/// The flag of the MCP server a tool needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// Served always.
    Read,
    /// `--allow-edits`.
    Edits,
    /// `--allow-game`.
    Game,
}

/// The tools of the MCP server by the flag that turns them on.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpGates {
    pub edits: Vec<String>,
    pub game: Vec<String>,
}

/// The whole table.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Table {
    #[serde(rename = "area")]
    pub areas: Vec<Area>,
    #[serde(rename = "package")]
    pub packages: Vec<Package>,
    pub mcp_gates: McpGates,
    #[serde(rename = "cap")]
    pub caps: Vec<Capability>,
}

/// The table built into the library.
pub fn table() -> &'static Table {
    static PARSED: OnceLock<Table> = OnceLock::new();
    PARSED.get_or_init(|| Table::parse(TABLE).expect("data/toolkit/capabilities.toml"))
}

impl Table {
    pub fn parse(text: &str) -> crate::Result<Self> {
        toml::from_str(text)
            .map_err(|e| crate::Error::InvalidArgument(format!("{TABLE_FILE}: {e}")))
    }

    /// The flag a tool of the MCP server needs, as the table has it.
    pub fn gate(&self, tool: &str) -> Gate {
        if self.mcp_gates.edits.iter().any(|t| t == tool) {
            Gate::Edits
        } else if self.mcp_gates.game.iter().any(|t| t == tool) {
            Gate::Game
        } else {
            Gate::Read
        }
    }

    pub fn package(&self, key: &str) -> Option<&Package> {
        self.packages.iter().find(|p| p.key == key)
    }

    /// Every name the rows give for a surface.
    pub fn names(&self, kind: Kind) -> BTreeSet<&str> {
        self.caps
            .iter()
            .flat_map(|c| c.surface(kind).names.iter().map(String::as_str))
            .collect()
    }

    /// Problems of the table itself: rows without a surface, unknown areas
    /// and packages, a hole between MCP and the editor without an estimate.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for area in &self.areas {
            if !seen.insert(area.key.as_str()) {
                out.push(format!("area {}: listed twice", area.key));
            }
        }
        let mut seen = HashSet::new();
        for package in &self.packages {
            if !seen.insert(package.key.as_str()) {
                out.push(format!("package {}: listed twice", package.key));
            }
            if package.stage != 103 && package.stage != 104 {
                out.push(format!("package {}: stage {}", package.key, package.stage));
            }
        }
        let mut seen = HashSet::new();
        for cap in &self.caps {
            let id = &cap.id;
            if !seen.insert(id.as_str()) {
                out.push(format!("{id}: listed twice"));
            }
            if cap.title.trim().is_empty() {
                out.push(format!("{id}: no title"));
            }
            if !self.areas.iter().any(|a| a.key == cap.area) {
                out.push(format!("{id}: unknown area {}", cap.area));
            }
            if !Kind::ALL.iter().any(|&k| cap.surface(k).present()) {
                out.push(format!("{id}: no surface has it"));
            }
            for kind in Kind::ALL {
                let s = cap.surface(kind);
                let name = kind.name();
                if !s.never.is_empty() && s.present() {
                    out.push(format!("{id}: {name} is there and `never` at once"));
                }
                if !s.partial.is_empty() && !s.present() {
                    out.push(format!("{id}: {name} is `partial` but names nothing"));
                }
                if kind != Kind::Editor && (!s.input.is_empty() || !s.preset.is_empty()) {
                    out.push(format!("{id}: {name} has `input` or `preset`"));
                }
                if kind == Kind::Editor && s.present() {
                    let known = ["menu", "form", "key", "mouse"];
                    if s.input.is_empty() || !s.input.split('+').all(|i| known.contains(&i)) {
                        out.push(format!("{id}: editor `input` is '{}'", s.input));
                    }
                }
                let mut names = HashSet::new();
                for n in &s.names {
                    if !names.insert(n) {
                        out.push(format!("{id}: {name} names {n} twice"));
                    }
                }
                let Some(gap) = &s.gap else { continue };
                let stage = match kind {
                    Kind::Editor => 103,
                    Kind::Mcp => 104,
                    Kind::Cli | Kind::Python => {
                        out.push(format!("{id}: {name} has a gap (only mcp and editor do)"));
                        continue;
                    }
                };
                if matches!(s.status(), Status::Full | Status::Never) {
                    out.push(format!("{id}: {name} has a gap but is not a hole"));
                }
                match self.package(&gap.package) {
                    None => out.push(format!("{id}: unknown package {}", gap.package)),
                    Some(p) if p.stage != stage => out.push(format!(
                        "{id}: {name} gap in package {} of stage {}",
                        p.key, p.stage
                    )),
                    Some(_) => {}
                }
            }
            // A hole between MCP and the editor — one of them gives more than
            // the other — needs an estimate.
            for (kind, other) in [(Kind::Mcp, Kind::Editor), (Kind::Editor, Kind::Mcp)] {
                let s = cap.surface(kind);
                let hole = match s.status() {
                    Status::Absent => cap.surface(other).present(),
                    Status::Partial => cap.surface(other).status() == Status::Full,
                    Status::Full | Status::Never => false,
                };
                if hole && s.gap.is_none() {
                    out.push(format!(
                        "{id}: {} lacks it (or a part) and has neither `gap` nor `never`",
                        kind.name()
                    ));
                }
            }
        }
        for package in &self.packages {
            let used = self.caps.iter().any(|c| {
                [&c.mcp, &c.editor]
                    .iter()
                    .any(|s| s.gap.as_ref().is_some_and(|g| g.package == package.key))
            });
            if !used && !package.key.ends_with("-fix") {
                out.push(format!("package {}: no gap is in it", package.key));
            }
        }
        for tool in self.mcp_gates.edits.iter().chain(&self.mcp_gates.game) {
            if !self.names(Kind::Mcp).contains(tool.as_str()) {
                out.push(format!("mcp_gates: {tool} is in no row"));
            }
        }
        out
    }

    /// Compares the names of one surface with the code: `existing` is every
    /// name the code has, `required` those a row must mention. Returns what
    /// is wrong: a row naming something that does not exist, a required
    /// name no row mentions.
    pub fn check_names<'a>(
        &self,
        kind: Kind,
        existing: impl IntoIterator<Item = &'a str>,
        required: impl IntoIterator<Item = &'a str>,
    ) -> Vec<String> {
        let existing: HashSet<&str> = existing.into_iter().collect();
        let mentioned = self.names(kind);
        let mut out = Vec::new();
        for cap in &self.caps {
            for n in &cap.surface(kind).names {
                if !existing.contains(n.as_str()) {
                    out.push(format!("{}: {} `{n}` does not exist", cap.id, kind.name()));
                }
            }
        }
        let mut missing: Vec<&str> = required
            .into_iter()
            .filter(|n| !mentioned.contains(n))
            .collect();
        missing.sort_unstable();
        missing.dedup();
        for n in missing {
            out.push(format!(
                "{} `{n}` is in no row of {TABLE_FILE}",
                kind.name()
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: &str = r#"
[[area]]
key = "level"
title = "Level"

[[package]]
key = "103-A"
stage = 103
title = "New areas"
size = "M"
why = "forms over library calls"

[mcp_gates]
edits = ["add_stream"]
game = []

[[cap]]
id = "level.stream.new"
area = "level"
title = "New stream"
[cap.mcp]
use = ["add_stream"]
[cap.cli]
use = ["add-stream"]
[cap.editor.gap]
add = "a form"
size = "S"
why = "one call"
package = "103-A"
"#;

    #[test]
    fn a_small_table_is_checked() {
        let table = Table::parse(SMALL).unwrap();
        assert_eq!(table.problems(), Vec::<String>::new());
        assert_eq!(table.gate("add_stream"), Gate::Edits);
        assert_eq!(table.gate("search"), Gate::Read);
        assert_eq!(table.caps[0].editor.status(), Status::Absent);
        assert_eq!(table.caps[0].mcp.status(), Status::Full);

        // A tool of the code no row mentions, a row naming a missing one.
        let problems = table.check_names(Kind::Mcp, ["search"], ["search"]);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].contains("`add_stream` does not exist"));
        assert!(problems[1].contains("`search` is in no row"));
        assert!(
            table
                .check_names(Kind::Cli, ["add-stream"], ["add-stream"])
                .is_empty()
        );
    }

    #[test]
    fn a_row_without_a_surface_or_an_estimate_is_a_problem() {
        let no_surface = SMALL
            .replace("[cap.mcp]\nuse = [\"add_stream\"]\n", "")
            .replace("[cap.cli]\nuse = [\"add-stream\"]\n", "");
        let problems = Table::parse(&no_surface).unwrap().problems();
        assert!(
            problems.iter().any(|p| p.contains("no surface has it")),
            "{problems:?}"
        );

        let no_gap = SMALL.split("[cap.editor.gap]").next().unwrap().to_owned();
        let problems = Table::parse(&no_gap).unwrap().problems();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("neither `gap` nor `never`")),
            "{problems:?}"
        );
    }

    #[test]
    fn the_table_of_the_toolkit_has_no_problems() {
        let problems = table().problems();
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }
}
