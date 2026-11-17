//! What `godfather2.exe` (PC 1.0.789.0) gives to Lua scripts: the functions
//! and tables it registers in the Lua state of the game
//! (`data/gf2/lua_api.toml`; found by `research/lua_api.py`, described in
//! `docs/analysis/lua-api.md`).
//!
//! The same file makes the note of the knowledge base (`kb::generate`), the
//! Lua stub ([`stub`], written by `doctor-toolkit types-gen`) and the checks
//! of the mock API of visual scripts (`script::sim`).

use std::collections::HashSet;
use std::fmt::Write as _;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

const SOURCE: &str = include_str!("../../../../data/gf2/lua_api.toml");

/// Path of the data file below the repository root.
pub const DATA_PATH: &str = "data/gf2/lua_api.toml";
/// Path of the generated Lua stub below the repository root.
pub const STUB_PATH: &str = "data/gf2/schema/gf2-lua-api.lua";

/// The whole file.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LuaApi {
    #[serde(rename = "group")]
    pub groups: Vec<LuaGroup>,
    #[serde(rename = "table")]
    pub tables: Vec<LuaTable>,
    #[serde(rename = "function")]
    pub functions: Vec<LuaFunction>,
    #[serde(rename = "missing")]
    pub missing: Vec<LuaMissing>,
}

/// A subsystem the functions are listed under.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LuaGroup {
    pub key: String,
    pub title: String,
    pub ru: String,
}

/// What a table is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LuaTableKind {
    /// A global table of the game (`GroupManager`).
    Global,
    /// The methods of a userdata type.
    Metatable,
    /// A standard library of Lua 5.0, or tolua.
    Library,
    /// Made by Lua text embedded in the exe.
    Lua,
}

/// A table or a metatable.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LuaTable {
    pub name: String,
    pub kind: LuaTableKind,
    pub ru: String,
}

/// Where a function comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LuaFunctionKind {
    /// A C function of the game.
    C,
    /// A standard function of Lua 5.0 or tolua.
    Lib,
    /// Defined by Lua text embedded in the exe.
    Lua,
}

/// How sure a signature is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LuaConfidence {
    /// Read in the code of the exe.
    Exe,
    /// Also seen in the running game.
    Bench,
    Guess,
}

/// A function scripts can call.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LuaFunction {
    pub name: String,
    /// The table it lives in; empty — a global function.
    #[serde(default)]
    pub owner: String,
    pub kind: LuaFunctionKind,
    pub group: String,
    /// Addresses of the registration (`0x…`): a call of `lua_pushstring` or
    /// an entry of a `{name, function}` array.
    #[serde(default)]
    pub register: Vec<String>,
    /// Address of the C function.
    #[serde(default, rename = "impl")]
    pub implementation: String,
    /// Address of the embedded Lua text (`kind = "lua"`).
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub args: String,
    #[serde(default)]
    pub returns: String,
    /// What it does (Russian, for the notes).
    #[serde(default)]
    pub ru: String,
    pub conf: LuaConfidence,
    /// Section of `docs/analysis/game-bench.md` with the check in the game.
    #[serde(default)]
    pub bench: String,
}

/// A name the game's scripts call that the exe does not define.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LuaMissing {
    pub name: String,
    pub ru: String,
}

impl LuaFunction {
    /// The name as a script writes the call: `PlayEmx`, `string.format`,
    /// `GroupManager:RegisterGroup`, `Group:AddMember`.
    pub fn full_name(&self) -> String {
        match self.owner.as_str() {
            "" | "_G" => self.name.clone(),
            owner if self.kind == LuaFunctionKind::Lib => format!("{owner}.{}", self.name),
            owner => format!("{owner}:{}", self.name),
        }
    }

    /// A global function of the game itself (C or embedded Lua), not of the
    /// standard libraries.
    pub fn is_game_global(&self) -> bool {
        self.owner.is_empty()
    }

    /// The signature has arguments and results (the standard libraries do not).
    pub fn is_described(&self) -> bool {
        self.kind != LuaFunctionKind::Lib
    }
}

impl LuaApi {
    pub fn function(&self, owner: &str, name: &str) -> Option<&LuaFunction> {
        self.functions
            .iter()
            .find(|f| f.owner == owner && f.name == name)
    }

    /// A global function of the game by name.
    pub fn global(&self, name: &str) -> Option<&LuaFunction> {
        self.function("", name)
    }

    pub fn group(&self, key: &str) -> Option<&LuaGroup> {
        self.groups.iter().find(|g| g.key == key)
    }

    /// Problems of the data: unknown groups and owners, addresses that are
    /// not addresses, entries listed twice.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let address = |s: &str| {
            s.len() == 10
                && s.starts_with("0x")
                && s[2..]
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'A'..=b'F').contains(&c))
        };
        let mut seen = HashSet::new();
        for f in &self.functions {
            let name = f.full_name();
            if !seen.insert((f.owner.as_str(), f.name.as_str())) {
                out.push(format!("{name}: listed twice"));
            }
            if self.group(&f.group).is_none() {
                out.push(format!("{name}: unknown group `{}`", f.group));
            }
            if !f.owner.is_empty() && !self.tables.iter().any(|t| t.name == f.owner) {
                out.push(format!("{name}: unknown owner `{}`", f.owner));
            }
            match f.kind {
                LuaFunctionKind::Lua => {
                    if !address(&f.source) || !f.register.is_empty() || !f.implementation.is_empty()
                    {
                        out.push(format!("{name}: an embedded function has only `source`"));
                    }
                }
                _ => {
                    if f.register.is_empty()
                        || !f.register.iter().all(|r| address(r))
                        || !address(&f.implementation)
                    {
                        out.push(format!("{name}: bad `register` or `impl` address"));
                    }
                }
            }
            if f.is_described() && (f.returns.is_empty() || f.ru.is_empty()) {
                out.push(format!("{name}: no signature or description"));
            }
            if !f.bench.is_empty() && !f.bench.starts_with('§') {
                out.push(format!("{name}: `bench` is a section, e.g. `§35`"));
            }
            if f.conf == LuaConfidence::Bench && f.bench.is_empty() {
                out.push(format!(
                    "{name}: conf bench without the section of the check"
                ));
            }
        }
        out
    }
}

/// `data/gf2/lua_api.toml`, parsed once.
///
/// # Panics
/// When the file built into the library is broken; the tests of this module
/// read it, so a build that passes them cannot panic here.
pub fn lua_api() -> &'static LuaApi {
    static API: OnceLock<LuaApi> = OnceLock::new();
    API.get_or_init(|| toml::from_str(SOURCE).expect("data/gf2/lua_api.toml"))
}

/// A Lua file that declares every function of the game with its signature:
/// for editors (the annotations are those of the Lua language server) and
/// for reading. The bodies are empty; the game provides the functions.
pub fn stub() -> String {
    let api = lua_api();
    let mut out = String::from(
        "-- Generated by `doctor-toolkit types-gen` from data/gf2/lua_api.toml; do not edit.\n\
         -- The functions godfather2.exe (PC 1.0.789.0) registers for Lua 5.0 scripts.\n\
         -- A declaration only: the bodies are empty, the game provides the functions.\n\
         -- `register` and `impl` are addresses in the exe; `conf` says whether the\n\
         -- signature was only read in the code (exe) or also seen in the game (bench).\n\
         ---@meta\n",
    );
    for group in &api.groups {
        let functions: Vec<&LuaFunction> = api
            .functions
            .iter()
            .filter(|f| f.group == group.key && f.is_described())
            .collect();
        if functions.is_empty() {
            continue;
        }
        writeln!(out, "\n-- {}", group.title).unwrap();
        let mut declared = HashSet::new();
        for f in functions {
            if !f.owner.is_empty() && declared.insert(f.owner.as_str()) && f.owner != "GroupManager"
            {
                writeln!(out, "\n---@class {}\nlocal {} = {{}}", f.owner, f.owner).unwrap();
            } else if f.owner == "GroupManager" && declared.insert("GroupManager") {
                out.push_str("\nGroupManager = {}\n");
            }
            out.push('\n');
            let place = match f.kind {
                LuaFunctionKind::Lua => format!("embedded Lua text {}", f.source),
                _ => format!(
                    "register {}, impl {}",
                    f.register.join(" "),
                    f.implementation
                ),
            };
            let conf = match f.conf {
                LuaConfidence::Exe => "exe",
                LuaConfidence::Bench => "bench",
                LuaConfidence::Guess => "guess",
            };
            writeln!(out, "-- {place}; conf {conf}").unwrap();
            writeln!(
                out,
                "-- args: {}",
                if f.args.is_empty() { "-" } else { &f.args }
            )
            .unwrap();
            writeln!(out, "-- returns: {}", f.returns).unwrap();
            let params = parameter_names(&f.args);
            // Methods are declared with `:`; `self` is then implied.
            let (name, params) = if f.owner.is_empty() {
                (f.name.clone(), params)
            } else {
                let rest: Vec<String> = params.into_iter().filter(|p| p != "self").collect();
                (format!("{}:{}", f.owner, f.name), rest)
            };
            writeln!(out, "function {name}({}) end", params.join(", ")).unwrap();
        }
    }
    out
}

/// Names of the parameters of a signature like `a: number, b?: string, ...`.
fn parameter_names(args: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut depth = 0usize;
    let mut piece = String::new();
    let mut pieces = Vec::new();
    for c in args.chars() {
        match c {
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                pieces.push(std::mem::take(&mut piece));
                continue;
            }
            _ => {}
        }
        piece.push(c);
    }
    pieces.push(piece);
    for p in pieces {
        let name = p
            .split(':')
            .next()
            .unwrap_or("")
            .trim()
            .trim_end_matches('?');
        if name == "..." {
            names.push("...".to_owned());
        } else if !name.is_empty() && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
            names.push(name.to_owned());
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_data_file_is_sound() {
        let api = lua_api();
        assert_eq!(api.problems(), Vec::<String>::new());
        // The counts of docs/analysis/lua-api.md §1.
        let game: Vec<_> = api.functions.iter().filter(|f| f.is_described()).collect();
        let globals = api
            .functions
            .iter()
            .filter(|f| f.is_game_global() && f.kind == LuaFunctionKind::C)
            .count();
        assert_eq!(globals, 74);
        assert_eq!(game.len(), 117);
        assert_eq!(api.functions.len(), 195);
        assert_eq!(api.tables.len(), 16);
        assert!(api.global("PlayEarsAudioStream").is_some());
        assert_eq!(
            api.function("Group", "AddMember").unwrap().full_name(),
            "Group:AddMember"
        );
        assert_eq!(
            api.function("string", "format").unwrap().full_name(),
            "string.format"
        );
    }

    #[test]
    fn the_stub_is_lua_the_toolkit_compiles() {
        let text = stub();
        assert!(text.contains("function PlayEarsAudioStream("));
        assert!(text.contains("function Group:AddMember(object) end"));
        crate::resource::lua::compile_for_game(text.as_bytes(), b"gf2-lua-api.lua")
            .expect("the stub compiles");
    }

    #[test]
    fn parameter_names_of_signatures() {
        assert_eq!(
            parameter_names("name: string|number, volume?: number, ...: any"),
            ["name", "volume", "..."]
        );
        assert_eq!(
            parameter_names("receiveFunction: function(self, messageName, dataTypeName, data)"),
            ["receiveFunction"]
        );
        assert!(parameter_names("").is_empty());
    }
}
