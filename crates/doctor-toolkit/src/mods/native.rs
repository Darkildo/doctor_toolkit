//! Executable code of mods (format version 5, stage 81): `.asi` modules the
//! ASI loader of the game loads from its `scripts` folder — native mods of
//! the platform `doctor_platform.asi` (`docs/analysis/asi-platform.md`) and
//! plain ASI plug-ins.
//!
//! ```json
//! "native": {
//!   "edition": "1.0.789.0",
//!   "platform_abi": 1,
//!   "required": true,
//!   "modules": [
//!     { "file": "doctor_example.asi",
//!       "payload": { "sha256": "…", "size": 123456 } }
//!   ]
//! }
//! ```
//!
//! Data of a mod cannot harm the player's computer; code can. So the rules
//! here are stricter than anywhere else in the mod manager:
//!
//! - **Nothing executable is written without the player's answer.** The
//!   answer is asked once for each mod ([`consent`], [`set_consent`]) and
//!   holds for its later versions: an update with another build of a module
//!   is installed without a new question (the hashes of the files shown are
//!   kept in the record). The player may take the data of the mod without its code
//!   ([`Answer::Data`]) unless the mod says its code is `required`.
//! - **Only into the edition the code is built for** ([`EDITION`]): the
//!   executable of the game is checked as the platform module checks it.
//! - **Only plain `.asi` names in `scripts/`** ([`is_module_name`]), never
//!   the names of the toolkit's own modules, never over a file the mod
//!   manager did not write ([`NativeBlock::FileTaken`]).
//! - **Every module is checked against its hash** when the mod is installed
//!   and again before it is written, and must be a 32-bit Windows DLL
//!   ([`check_module`]).
//! - **A mod cannot ask for a newer platform than there is**
//!   ([`PLATFORM_ABI`]).
//!
//! What the mod manager wrote is recorded in `_doctor_toolkit/mods/native.json`
//! with the hash of each file. Switching a mod off, uninstalling it and
//! "Restore originals" take its modules away ([`prune`], [`remove_all`]),
//! and with the last of them the platform module and the loader — when the
//! mod manager put them there and nothing else needs them. A file changed
//! since, or one somebody else put into `scripts/`, is left alone.
//!
//! The platform module and the ASI loader are files of the toolkit
//! ([`NativeHost::detect`] finds them next to the program): the loader is
//! the toolkit's own `doctor_loader.dll` (crate `doctor-toolkit-loader`),
//! installed as `dinput8.dll` only when the game folder has no loader yet.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::manager::{InstalledMod, ModManager};
use super::{check_text, is_sha256};
use crate::live::SCRIPTS_DIR;
use crate::workspace::{Blob, store::sha256_hex};
use crate::{Error, Result};

/// The edition of the game native code is installed into: the only one the
/// platform module works in.
pub const EDITION: &str = "1.0.789.0";
/// The newest ABI of the platform module this program can install.
pub const PLATFORM_ABI: u32 = doctor_toolkit_live::platform::ABI_VERSION;
/// File name of the platform module in `scripts/`.
pub const PLATFORM_FILE: &str = doctor_toolkit_live::platform::MODULE_FILE;
/// The name the toolkit's ASI loader is installed under, in the game folder.
pub const LOADER_FILE: &str = "dinput8.dll";
/// The loader as a file of the toolkit, next to the programs.
pub const LOADER_BINARY: &str = "doctor_loader.dll";
/// Most modules one mod may carry.
pub const MAX_MODULES: usize = 16;
/// Largest module accepted.
pub const MAX_MODULE_SIZE: u64 = 64 << 20;
/// Longest module file name, with `.asi`.
const MAX_MODULE_NAME: usize = 52;

/// Names in `scripts/` that belong to the toolkit: a mod cannot carry them.
pub const RESERVED: &[&str] = &[
    "doctor_live.asi",
    "doctor_render.asi",
    "doctor_crash.asi",
    "doctor_platform.asi",
    "doctor_loader.asi",
];

const RECORD_FILE: &str = "native.json";
const RECORD_FORMAT: &str = "doctor-toolkit-native";
const RECORD_VERSION: u32 = 1;
/// Owner of the platform module in [`written`].
pub const OWNER_PLATFORM: &str = "platform";
/// Owner of the toolkit's ASI loader in [`written`].
pub const OWNER_LOADER: &str = "loader";

/// `native` of `mod.json`: the executable code of a mod.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModNative {
    /// The edition of the game the code is built for: [`EDITION`].
    pub edition: String,
    /// The ABI of the platform module the modules need
    /// (`doctor_sdk::ABI_VERSION` they were built with); absent for plain
    /// ASI plug-ins, which do not use the platform.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_abi: Option<u32>,
    /// The mod makes no sense without its code: it is not written into the
    /// game at all while the code cannot be installed.
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
    /// The modules, sorted by file name.
    pub modules: Vec<NativeModule>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}

/// One module: a 32-bit Windows DLL for the `scripts` folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeModule {
    /// Plain file name in `scripts/`, e.g. `doctor_example.asi`.
    pub file: String,
    /// The DLL: `payloads/<sha256>` in the package.
    pub payload: Blob,
    /// What the module is, for the player.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
}

impl NativeModule {
    /// Where the module goes, relative to the game folder.
    pub fn path(&self) -> String {
        format!("{SCRIPTS_DIR}/{}", self.file)
    }
}

/// Whether `name` may be the file of a module of a mod: lower-case letters,
/// digits, `_` and `-`, then `.asi`; no path, no name of a module of the
/// toolkit, no device name of Windows.
pub fn is_module_name(name: &str) -> bool {
    const DEVICES: [&str; 22] = [
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    let Some(stem) = name.strip_suffix(".asi") else {
        return false;
    };
    !stem.is_empty()
        && name.len() <= MAX_MODULE_NAME
        && stem.as_bytes()[0].is_ascii_alphanumeric()
        && stem
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        && !DEVICES.contains(&stem)
        && !RESERVED.contains(&name)
}

/// Checks that `data` is a 32-bit Windows DLL (the only thing the loader of
/// the game can load).
pub fn check_module(data: &[u8]) -> std::result::Result<(), String> {
    let u16_at = |at: usize| {
        data.get(at..at + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
    };
    if data.get(..2) != Some(b"MZ") {
        return Err("not a Windows DLL (no MZ header)".into());
    }
    let pe = data
        .get(0x3C..0x40)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
        .filter(|&pe| pe < data.len());
    let Some(pe) = pe else {
        return Err("not a Windows DLL (no PE header)".into());
    };
    if data.get(pe..pe + 4) != Some(b"PE\0\0") {
        return Err("not a Windows DLL (no PE header)".into());
    }
    match (u16_at(pe + 4), u16_at(pe + 22)) {
        (Some(0x014C), Some(flags)) if flags & 0x2000 != 0 => Ok(()),
        (Some(0x014C), Some(_)) => Err("a program, not a DLL".into()),
        (Some(machine), _) => Err(format!(
            "built for another processor (machine {machine:#06x}); the game is 32-bit x86"
        )),
        _ => Err("not a Windows DLL (short PE header)".into()),
    }
}

/// Whether the DLL names the entry of a native mod (`doctor_mod_init`): a
/// module that says it needs the platform but does not export it is most
/// likely built wrong. A search for the name, not a walk of the exports.
pub fn names_mod_entry(data: &[u8]) -> bool {
    let needle = doctor_toolkit_live::platform::MOD_ENTRY.as_bytes();
    data.windows(needle.len()).any(|w| w == needle)
}

impl ModNative {
    /// The code of a mod being built: `modules` are (file name, DLL).
    pub fn of_modules(
        modules: &[(String, Vec<u8>)],
        platform_abi: Option<u32>,
        required: bool,
    ) -> Result<Self> {
        let mut out = Self {
            edition: EDITION.to_owned(),
            platform_abi,
            required,
            modules: Vec::new(),
        };
        for (file, data) in modules {
            check_module(data)
                .map_err(|why| Error::mod_error(format!("native module `{file}`: {why}")))?;
            out.modules.push(NativeModule {
                file: file.clone(),
                payload: Blob::of(data),
                label: String::new(),
            });
        }
        out.modules.sort_by(|a, b| a.file.cmp(&b.file));
        out.validate()?;
        Ok(out)
    }

    /// Checks the section against the format and the limits.
    pub fn validate(&self) -> Result<()> {
        let bad = |why: String| Err(Error::mod_error(format!("mod.json: `native`: {why}")));
        if self.edition != EDITION {
            return bad(format!(
                "edition `{}`: executable code is installed only into the edition {EDITION} \
                 of the game",
                self.edition.escape_debug()
            ));
        }
        if let Some(abi) = self.platform_abi
            && abi == 0
        {
            return bad("`platform_abi` 0 is not a version".into());
        }
        if self.modules.is_empty() || self.modules.len() > MAX_MODULES {
            return bad(format!("1 to {MAX_MODULES} modules are expected"));
        }
        let mut names = BTreeSet::new();
        for module in &self.modules {
            if !is_module_name(&module.file) {
                return bad(format!(
                    "bad module name `{}`: a plain file name of lower-case letters, digits, \
                     `_` and `-` ending in `.asi`, not a module of the toolkit ({})",
                    module.file.escape_debug(),
                    RESERVED.join(", ")
                ));
            }
            if !names.insert(module.file.as_str()) {
                return bad(format!("the module {} is listed twice", module.file));
            }
            if !is_sha256(&module.payload.sha256) {
                return bad(format!("{}: the hash is not a SHA-256", module.file));
            }
            if module.payload.size == 0 || module.payload.size > MAX_MODULE_SIZE {
                return bad(format!(
                    "{}: {} bytes; a module has 1 to {MAX_MODULE_SIZE}",
                    module.file, module.payload.size
                ));
            }
            check_text("native.label", &module.label, 200, true)?;
        }
        Ok(())
    }
}

// ------------------------------------------------------------- the record

/// The player's answer about the code of a mod.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Answer {
    /// The code may be installed.
    Code,
    /// Only the data of the mod; its code stays out of the game.
    Data,
}

/// What the player said about the code of a mod as it is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consent {
    /// The player was not asked about this mod yet: nothing executable is
    /// installed.
    Unasked,
    /// The player agreed to the code of the mod (this and later versions).
    Code,
    /// The player wants the mod without its code.
    DataOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Written {
    sha256: String,
    /// The id of the mod, [`OWNER_PLATFORM`] or [`OWNER_LOADER`].
    owner: String,
}

/// `native.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    format: String,
    version: u32,
    /// Mod id → SHA-256 of a module → the answer.
    #[serde(default)]
    consent: BTreeMap<String, BTreeMap<String, Answer>>,
    /// Path relative to the game folder → what the mod manager wrote there.
    #[serde(default)]
    written: BTreeMap<String, Written>,
}

impl Default for Record {
    fn default() -> Self {
        Self {
            format: RECORD_FORMAT.to_owned(),
            version: RECORD_VERSION,
            consent: BTreeMap::new(),
            written: BTreeMap::new(),
        }
    }
}

impl Record {
    fn path(manager: &ModManager) -> PathBuf {
        manager.dir().join(RECORD_FILE)
    }

    fn load(manager: &ModManager) -> Result<Self> {
        let path = Self::path(manager);
        let text = match fs::read(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(Error::io(&path, e)),
        };
        let record: Self = serde_json::from_slice(&text)
            .map_err(|e| Error::mod_error(format!("{}: {e}", path.display())))?;
        if record.format != RECORD_FORMAT || record.version > RECORD_VERSION {
            return Err(Error::mod_error(format!(
                "{} was written by a newer program or is not a record of native code",
                path.display()
            )));
        }
        Ok(record)
    }

    /// Writes the record; an empty one leaves no file.
    fn save(&self, manager: &ModManager) -> Result<()> {
        let path = Self::path(manager);
        if self.consent.is_empty() && self.written.is_empty() {
            return match fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(Error::io(&path, e)),
            };
        }
        fs::create_dir_all(manager.dir()).map_err(|e| Error::io(manager.dir(), e))?;
        let mut text = serde_json::to_vec_pretty(self)
            .map_err(|e| Error::mod_error(format!("{RECORD_FILE}: {e}")))?;
        text.push(b'\n');
        crate::atomic::write(&path, &text)
    }

    fn consent(&self, m: &InstalledMod) -> Consent {
        let Some(native) = &m.manifest.native else {
            return Consent::Code;
        };
        // The answer is for the mod, whatever build of its modules it
        // carries now: an update of the mod is not asked about again (the
        // owner's decision; the hashes in the record say what was shown).
        let _ = native;
        match self.consent.get(&m.id) {
            Some(answers) if !answers.is_empty() => {
                if answers.values().all(|a| *a == Answer::Code) {
                    Consent::Code
                } else {
                    Consent::DataOnly
                }
            }
            _ => Consent::Unasked,
        }
    }
}

/// What the player said about the code of the installed mod `id`
/// ([`Consent::Code`] for a mod without code).
pub fn consent(manager: &ModManager, id: &str) -> Result<Consent> {
    let record = Record::load(manager)?;
    Ok(manager
        .get(id)
        .map_or(Consent::Unasked, |m| record.consent(m)))
}

/// Whether installing a mod `id` with the modules `sha256` would put code
/// into the game that the player was never shown: the player agreed to the
/// code of the mod (the answer holds for its later versions), and one of
/// these modules is not among the files of that answer. A person updating
/// a mod knows what they install; a program acting for them (the MCP
/// server) must not change agreed code behind the answer.
pub fn replaces_agreed_code<'a>(
    manager: &ModManager,
    id: &str,
    sha256: impl IntoIterator<Item = &'a str>,
) -> Result<bool> {
    let record = Record::load(manager)?;
    let Some(answers) = record.consent.get(id) else {
        return Ok(false);
    };
    if answers.is_empty() || !answers.values().all(|a| *a == Answer::Code) {
        return Ok(false);
    }
    Ok(sha256.into_iter().any(|hash| !answers.contains_key(hash)))
}

/// A module of a mod being built as the surfaces take it: `FILE`, or
/// `NAME.asi=FILE` to name it in `scripts/`. Returns the file name in
/// `scripts/` and the path of the DLL.
pub fn module_spec(spec: &str) -> (String, PathBuf) {
    match spec.split_once('=') {
        Some((name, path)) => (name.trim().to_owned(), PathBuf::from(path.trim())),
        None => {
            let path = PathBuf::from(spec.trim());
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            (format!("{stem}.asi"), path)
        }
    }
}

/// The executable code of a mod being built, read from the disk.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeSpec {
    /// The modules ([`module_spec`]).
    #[serde(default)]
    pub modules: Vec<String>,
    /// The mod makes no sense without its code.
    #[serde(default)]
    pub required: bool,
    /// Plain ASI plug-ins that do not use the platform module.
    #[serde(default)]
    pub plain: bool,
    /// The ABI of the platform module the code was built with [default:
    /// [`PLATFORM_ABI`]].
    #[serde(default)]
    pub platform_abi: Option<u32>,
}

impl NativeSpec {
    /// Reads the modules: `None` when there are none, and what the author
    /// should hear about them (a module for the platform that does not name
    /// its entry).
    pub fn read(&self) -> Result<(Option<super::NativeBuild>, Vec<String>)> {
        if self.modules.is_empty() {
            return Ok((None, Vec::new()));
        }
        let mut modules = Vec::new();
        let mut warnings = Vec::new();
        for spec in &self.modules {
            let (name, path) = module_spec(spec);
            let data = fs::read(&path).map_err(|e| Error::io(&path, e))?;
            if !self.plain && !names_mod_entry(&data) {
                warnings.push(format!(
                    "{} does not export doctor_mod_init: the platform module will not start \
                     it (a plain ASI plug-in? then build it as plain)",
                    path.display()
                ));
            }
            modules.push((name, data));
        }
        let build = super::NativeBuild {
            modules,
            platform_abi: if self.plain {
                None
            } else {
                Some(self.platform_abi.unwrap_or(PLATFORM_ABI))
            },
            required: self.required,
        };
        Ok((Some(build), warnings))
    }
}

/// Records the player's answer for mod `id` (with the hashes of its modules
/// as they are now); `None` forgets the answer, so the question is asked again.
/// Nothing is written into the game: [`sync`] installs, [`prune`] removes.
pub fn set_consent(manager: &ModManager, id: &str, answer: Option<Answer>) -> Result<()> {
    let m = manager
        .get(id)
        .ok_or_else(|| Error::mod_error(format!("no mod `{id}` is installed")))?;
    let Some(native) = &m.manifest.native else {
        return Err(Error::mod_error(format!(
            "mod `{id}` has no executable code: there is nothing to agree to"
        )));
    };
    let mut record = Record::load(manager)?;
    match answer {
        Some(answer) => {
            // The hashes say which files the player was shown.
            record.consent.insert(
                id.to_owned(),
                native
                    .modules
                    .iter()
                    .map(|m| (m.payload.sha256.clone(), answer))
                    .collect(),
            );
        }
        None => {
            record.consent.remove(id);
        }
    }
    record.save(manager)
}

// ----------------------------------------------------------------- the host

/// What installing code depends on besides the mods: the game and the files
/// of the toolkit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeHost {
    /// Whether the executable of the game is the edition [`EDITION`] with
    /// the places the platform module patches untouched; else why not.
    pub edition: std::result::Result<(), String>,
    /// File name of the ASI loader in the game folder, when there is one.
    pub loader: Option<String>,
    /// The platform module of the toolkit ([`PLATFORM_FILE`]).
    pub platform_binary: Option<PathBuf>,
    /// The ASI loader of the toolkit ([`LOADER_BINARY`]).
    pub loader_binary: Option<PathBuf>,
}

/// A file of the toolkit: named by the environment variable `env`, next to
/// the running program, or in the build directory of this repository.
fn toolkit_file(env: &str, name: &str) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(env) {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join(name));
        candidates.push(dir.join("../share/doctor_toolkit").join(name));
        candidates.push(dir.join("../live").join(name));
    }
    candidates.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/live")
            .join(name),
    );
    candidates.into_iter().find(|p| p.is_file())
}

impl NativeHost {
    /// Looks at the game in `root` and for the files of the toolkit: the
    /// platform module (`DOCTOR_PLATFORM_MODULE`, `doctor_platform.asi` next
    /// to the program, `tools/build-platform-module.sh`) and the loader
    /// (`DOCTOR_LOADER_MODULE`, `doctor_loader.dll`,
    /// `tools/build-loader.sh`).
    pub fn detect(root: &Path) -> Self {
        let edition = match crate::play::find_executable(root).map(fs::read) {
            Some(Ok(data)) => doctor_toolkit_live::edition::check_file_with(
                &data,
                doctor_toolkit_live::platform::CHECKS,
            )
            .map_err(|e| e.to_string()),
            Some(Err(e)) => Err(format!("the executable cannot be read: {e}")),
            None => Err("the executable of the game is not found".to_owned()),
        };
        Self {
            edition,
            loader: crate::live::loader::find(root)
                .and_then(|l| l.file.file_name().map(|n| n.to_string_lossy().into_owned())),
            platform_binary: toolkit_file("DOCTOR_PLATFORM_MODULE", PLATFORM_FILE),
            loader_binary: toolkit_file("DOCTOR_LOADER_MODULE", LOADER_BINARY),
        }
    }

    /// A host that stands in the way of nothing and has no files to give:
    /// for [`prune`], which only takes away.
    fn permissive() -> Self {
        Self {
            edition: Ok(()),
            loader: Some(String::new()),
            platform_binary: None,
            loader_binary: None,
        }
    }
}

// ------------------------------------------------------------------ states

/// Why the code of a mod is not installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeBlock {
    /// The player has not answered yet.
    Unasked,
    /// The player wants the mod without its code.
    Refused,
    /// The game is not [`EDITION`]: what the check said.
    Edition(String),
    /// The game folder has no ASI loader and the toolkit has none to install.
    NoLoader,
    /// The mod needs the platform module and the toolkit has none to install.
    NoPlatform,
    /// The mod needs a newer platform than this program installs.
    Abi { required: u32 },
    /// `scripts/` has a file of this name the mod manager did not write, or
    /// an earlier mod carries another module of this name.
    FileTaken { file: String },
    /// A module of the installed mod is damaged: what is wrong.
    Damaged(String),
}

impl NativeBlock {
    /// Whether the mod cannot be written at all with this, whatever it says
    /// about `required`.
    fn always_fatal(&self) -> bool {
        matches!(self, Self::Abi { .. } | Self::Damaged(_))
    }

    /// A sentence for the person.
    pub fn describe(&self) -> String {
        match self {
            Self::Unasked => "its executable code waits for your consent; nothing executable \
                              is installed before you answer"
                .to_owned(),
            Self::Refused => "you chose to install it without its executable code".to_owned(),
            Self::Edition(why) => {
                format!("its executable code is only for the edition {EDITION} of the game ({why})")
            }
            Self::NoLoader => format!(
                "the game folder has no ASI loader ({LOADER_FILE}) and the toolkit's \
                 {LOADER_BINARY} is not next to the program; put an ASI loader into the game \
                 folder"
            ),
            Self::NoPlatform => format!(
                "its code needs {PLATFORM_FILE}, which is not next to the program (a file of \
                 the release; from the sources: tools/build-platform-module.sh)"
            ),
            Self::Abi { required } => format!(
                "its code needs version {required} of the platform of native mods, this \
                 program installs version {PLATFORM_ABI}; update Open Mod Manager \
                 (doctor_toolkit)"
            ),
            Self::FileTaken { file } => format!(
                "{SCRIPTS_DIR}/{file} is already in the game and was not installed by the \
                 mod manager (or another enabled mod carries a different {file}); it is left \
                 alone"
            ),
            Self::Damaged(why) => format!("its executable code is damaged: {why}"),
        }
    }
}

/// The code of an enabled mod and the game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeState {
    /// Every module is in `scripts/` as the mod has it.
    Installed,
    /// Agreed to and possible, not written yet: "Apply to game" installs it.
    NotWritten,
    /// Not installed, and it will not be until this changes.
    Blocked(NativeBlock),
}

/// Whether a mod with `block` cannot be written into the game at all:
/// its code is `required`, or the block is one that no choice of the player
/// lifts.
pub fn is_fatal(block: &NativeBlock, required: bool) -> bool {
    required || block.always_fatal()
}

/// Where a file to write comes from.
#[derive(Debug, Clone)]
enum Source {
    /// A payload of the installed mod at this index.
    Payload(usize, Blob),
    /// A file of the toolkit.
    File(PathBuf),
    /// Whatever is there stays.
    Keep,
}

#[derive(Debug, Clone)]
struct Desired {
    owner: String,
    source: Source,
}

#[derive(Debug, Default)]
struct Evaluation {
    /// Enabled mods with code: id → what stands in the way, if anything.
    mods: BTreeMap<String, Option<NativeBlock>>,
    /// Path relative to the game folder → what should be there.
    desired: BTreeMap<String, Desired>,
}

/// The file `name` of `dir` whatever the case of its letters (the game runs
/// on Windows, where `Foo.asi` and `foo.asi` are one file).
fn find_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
        .map(|e| e.path())
}

/// The file at the relative path `rel` (`scripts/x.asi`, `dinput8.dll`) of
/// the game, whatever the case of its name.
fn game_file(root: &Path, rel: &str) -> Option<PathBuf> {
    match rel.rsplit_once('/') {
        Some((dir, name)) => find_ci(&root.join(dir), name),
        None => find_ci(root, rel),
    }
}

fn hash_of(path: &Path) -> Option<String> {
    fs::read(path).ok().map(|data| sha256_hex(&data))
}

/// Whether the file at `rel` is the one the record says the mod manager
/// wrote: there, and unchanged.
fn is_ours(root: &Path, record: &Record, rel: &str) -> bool {
    record.written.get(rel).is_some_and(|w| {
        game_file(root, rel)
            .and_then(|p| hash_of(&p))
            .is_some_and(|h| h == w.sha256)
    })
}

fn evaluate(manager: &ModManager, host: &NativeHost, record: &Record) -> Evaluation {
    let root = manager.game_root();
    let mut eval = Evaluation::default();
    // File name → hash of the module an earlier mod puts there.
    let mut taken: BTreeMap<&str, &str> = BTreeMap::new();
    let mut going: Vec<usize> = Vec::new();
    for (index, m) in manager.mods().iter().enumerate() {
        let Some(native) = m.manifest.native.as_ref().filter(|_| m.enabled) else {
            continue;
        };
        if m.problem.is_some() {
            continue;
        }
        let block = (|| {
            if let Some(required) = native.platform_abi.filter(|&abi| abi > PLATFORM_ABI) {
                return Some(NativeBlock::Abi { required });
            }
            match record.consent(m) {
                Consent::Unasked => return Some(NativeBlock::Unasked),
                Consent::DataOnly => return Some(NativeBlock::Refused),
                Consent::Code => {}
            }
            if let Err(why) = &host.edition {
                return Some(NativeBlock::Edition(why.clone()));
            }
            for module in &native.modules {
                let rel = module.path();
                let other_mod = taken
                    .get(module.file.as_str())
                    .is_some_and(|sha| *sha != module.payload.sha256);
                let foreign = game_file(root, &rel).is_some_and(|path| {
                    !is_ours(root, record, &rel)
                        && hash_of(&path).as_deref() != Some(module.payload.sha256.as_str())
                });
                if other_mod || foreign {
                    return Some(NativeBlock::FileTaken {
                        file: module.file.clone(),
                    });
                }
            }
            None
        })();
        if block.is_none() {
            for module in &native.modules {
                taken.insert(&module.file, &module.payload.sha256);
            }
            going.push(index);
        }
        eval.mods.insert(m.id.clone(), block);
    }

    // The platform module, for the mods that need it.
    let platform_rel = format!("{SCRIPTS_DIR}/{PLATFORM_FILE}");
    let needs_platform = |i: &usize| {
        manager.mods()[*i]
            .manifest
            .native
            .as_ref()
            .is_some_and(|n| n.platform_abi.is_some())
    };
    let mut platform = None;
    if going.iter().any(needs_platform) {
        let there = game_file(root, &platform_rel).is_some();
        let ours = is_ours(root, record, &platform_rel);
        platform = match (&host.platform_binary, there, ours) {
            // Ours, or none yet: the toolkit's file goes there.
            (Some(binary), _, true) | (Some(binary), false, _) => Some(Desired {
                owner: OWNER_PLATFORM.to_owned(),
                source: Source::File(binary.clone()),
            }),
            // Somebody's own platform module (an author's build): it is used
            // as it is and never recorded.
            (_, true, false) => None,
            (None, true, true) => Some(Desired {
                owner: OWNER_PLATFORM.to_owned(),
                source: Source::Keep,
            }),
            (None, false, _) => {
                going.retain(|i| {
                    let blocked = needs_platform(i);
                    if blocked {
                        eval.mods
                            .insert(manager.mods()[*i].id.clone(), Some(NativeBlock::NoPlatform));
                    }
                    !blocked
                });
                None
            }
        };
    }

    // The loader: the player's own when there is one, else the toolkit's.
    let mut loader = None;
    if !going.is_empty() {
        let there = game_file(root, LOADER_FILE).is_some();
        if is_ours(root, record, LOADER_FILE) {
            loader = Some(Desired {
                owner: OWNER_LOADER.to_owned(),
                source: host
                    .loader_binary
                    .clone()
                    .map_or(Source::Keep, Source::File),
            });
        } else if !there
            // The host may be older than the last removal: a loader it
            // names counts only while it is there.
            && !host
                .loader
                .as_ref()
                .is_some_and(|name| name.is_empty() || game_file(root, name).is_some())
        {
            match &host.loader_binary {
                Some(binary) => {
                    loader = Some(Desired {
                        owner: OWNER_LOADER.to_owned(),
                        source: Source::File(binary.clone()),
                    });
                }
                None => {
                    for i in going.drain(..) {
                        eval.mods
                            .insert(manager.mods()[i].id.clone(), Some(NativeBlock::NoLoader));
                    }
                    platform = None;
                }
            }
        }
    }

    for &i in &going {
        let m = &manager.mods()[i];
        for module in m.manifest.native.iter().flat_map(|n| &n.modules) {
            eval.desired.insert(
                module.path(),
                Desired {
                    owner: m.id.clone(),
                    source: Source::Payload(i, module.payload.clone()),
                },
            );
        }
    }
    if !going.is_empty() {
        if let Some(platform) = platform {
            eval.desired.insert(platform_rel, platform);
        }
        if let Some(loader) = loader {
            eval.desired.insert(LOADER_FILE.to_owned(), loader);
        }
    }
    eval
}

/// The state of the code of every enabled mod that has code, in load order.
pub fn states(manager: &ModManager, host: &NativeHost) -> Result<Vec<(String, NativeState)>> {
    let record = Record::load(manager)?;
    let eval = evaluate(manager, host, &record);
    let root = manager.game_root();
    let mut out = Vec::new();
    for m in manager.mods() {
        let Some(block) = eval.mods.get(&m.id) else {
            continue;
        };
        let state = match block {
            Some(block) => NativeState::Blocked(block.clone()),
            None => {
                let written = m
                    .manifest
                    .native
                    .iter()
                    .flat_map(|n| &n.modules)
                    .all(|module| {
                        game_file(root, &module.path())
                            .and_then(|p| hash_of(&p))
                            .is_some_and(|h| h == module.payload.sha256)
                    });
                // The platform and the loader count too: without them the
                // modules are files nobody loads.
                let rest = eval
                    .desired
                    .iter()
                    .filter(|(_, d)| d.owner == OWNER_PLATFORM || d.owner == OWNER_LOADER)
                    .all(|(rel, _)| game_file(root, rel).is_some());
                if written && rest {
                    NativeState::Installed
                } else {
                    NativeState::NotWritten
                }
            }
        };
        out.push((m.id.clone(), state));
    }
    Ok(out)
}

/// What stands in the way of the code of each enabled mod: for the checks
/// of the mod manager. A mod whose code is fine is not listed.
pub fn blocks(manager: &ModManager, host: &NativeHost) -> Result<BTreeMap<String, NativeBlock>> {
    let record = Record::load(manager)?;
    Ok(evaluate(manager, host, &record)
        .mods
        .into_iter()
        .filter_map(|(id, block)| Some((id, block?)))
        .collect())
}

/// The files the mod manager has in the game by its record, relative to the
/// game folder, with the id of the mod (or `platform`, `loader`).
pub fn written(manager: &ModManager) -> Result<Vec<(String, String)>> {
    Ok(Record::load(manager)?
        .written
        .into_iter()
        .map(|(rel, w)| (rel, w.owner))
        .collect())
}

// ------------------------------------------------------------ the question

/// One file of the question to the player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionFile {
    /// Where it goes, relative to the game folder.
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

/// What the player is asked before the code of a mod is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    /// The mod is not installed at all without its code.
    pub required: bool,
    /// The modules of the mod: code of its author.
    pub modules: Vec<QuestionFile>,
    /// Files of the toolkit that come with them when they are not in the
    /// game yet: the platform module, the ASI loader.
    pub toolkit: Vec<QuestionFile>,
    /// What stands in the way besides the answer (another edition, no
    /// loader…): the code will not be installed even after a "yes".
    pub obstacle: Option<NativeBlock>,
}

impl Question {
    /// The question as lines of text, for the command line.
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![format!(
            "mod `{}` ({} {}{}) contains EXECUTABLE CODE, not only data: it runs inside the \
             game with your rights and can do anything a program can.",
            self.id,
            self.name,
            self.version,
            if self.author.is_empty() {
                String::new()
            } else {
                format!(", by {}", self.author)
            }
        )];
        out.push("files of the mod that would go into the game folder:".to_owned());
        for f in &self.modules {
            out.push(format!(
                "  {}  {} bytes  sha256 {}",
                f.path, f.size, f.sha256
            ));
        }
        if !self.toolkit.is_empty() {
            out.push("files of the toolkit that come with them:".to_owned());
            for f in &self.toolkit {
                out.push(format!(
                    "  {}  {} bytes  sha256 {}",
                    f.path, f.size, f.sha256
                ));
            }
        }
        if self.required {
            out.push("the mod does not work without its code: refuse and it is not installed into the game at all.".to_owned());
        }
        if let Some(obstacle) = &self.obstacle {
            out.push(format!("note: {}", obstacle.describe()));
        }
        out
    }
}

fn toolkit_file_entry(rel: String, binary: &Path) -> Option<QuestionFile> {
    let data = fs::read(binary).ok()?;
    Some(QuestionFile {
        path: rel,
        sha256: sha256_hex(&data),
        size: data.len() as u64,
    })
}

/// The question about the code of the installed mod `id`; `None` for a mod
/// without code.
pub fn question(manager: &ModManager, host: &NativeHost, id: &str) -> Result<Option<Question>> {
    let Some(m) = manager.get(id) else {
        return Err(Error::mod_error(format!("no mod `{id}` is installed")));
    };
    let Some(native) = &m.manifest.native else {
        return Ok(None);
    };
    let root = manager.game_root();
    let mut toolkit = Vec::new();
    let platform_rel = format!("{SCRIPTS_DIR}/{PLATFORM_FILE}");
    if native.platform_abi.is_some()
        && game_file(root, &platform_rel).is_none()
        && let Some(binary) = &host.platform_binary
    {
        toolkit.extend(toolkit_file_entry(platform_rel, binary));
    }
    if host.loader.is_none()
        && let Some(binary) = &host.loader_binary
    {
        toolkit.extend(toolkit_file_entry(LOADER_FILE.to_owned(), binary));
    }
    // What would stand in the way after a "yes": the same evaluation with
    // the answer given.
    let mut record = Record::load(manager)?;
    record.consent.insert(
        id.to_owned(),
        native
            .modules
            .iter()
            .map(|m| (m.payload.sha256.clone(), Answer::Code))
            .collect(),
    );
    let obstacle = if m.enabled {
        evaluate(manager, host, &record).mods.remove(id).flatten()
    } else if let Some(required) = native.platform_abi.filter(|&abi| abi > PLATFORM_ABI) {
        Some(NativeBlock::Abi { required })
    } else {
        host.edition.clone().err().map(NativeBlock::Edition)
    };
    Ok(Some(Question {
        id: m.id.clone(),
        name: m.manifest.name.clone(),
        version: m.manifest.version.clone(),
        author: m.manifest.author.clone(),
        required: native.required,
        modules: native
            .modules
            .iter()
            .map(|module| QuestionFile {
                path: module.path(),
                sha256: module.payload.sha256.clone(),
                size: module.payload.size,
            })
            .collect(),
        toolkit,
        obstacle,
    }))
}

// ------------------------------------------------------- writing, removing

/// What [`sync`], [`prune`] and [`remove_all`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeReport {
    /// Files written, relative to the game folder.
    pub written: Vec<String>,
    /// Files taken away.
    pub removed: Vec<String>,
    /// Enabled mods whose code was not installed, and why.
    pub blocked: Vec<(String, NativeBlock)>,
    /// What did not go as asked and did not stop the rest.
    pub warnings: Vec<String>,
}

impl NativeReport {
    pub fn is_empty(&self) -> bool {
        self.written.is_empty()
            && self.removed.is_empty()
            && self.blocked.is_empty()
            && self.warnings.is_empty()
    }

    /// Lines for the person.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        for rel in &self.written {
            out.push(format!("native code: installed {rel}"));
        }
        for rel in &self.removed {
            out.push(format!("native code: removed {rel}"));
        }
        for (id, block) in &self.blocked {
            out.push(format!(
                "native code of mod `{id}` is not installed: {}",
                block.describe()
            ));
        }
        out.extend(self.warnings.iter().cloned());
        out
    }
}

/// Whether the game folder or its `scripts/` has an `.asi` file: something
/// a loader is there for.
fn any_modules(root: &Path) -> bool {
    [root.to_path_buf(), root.join(SCRIPTS_DIR)]
        .iter()
        .filter_map(|dir| fs::read_dir(dir).ok())
        .flat_map(|list| list.filter_map(|e| e.ok()))
        .any(|e| {
            e.file_name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with(".asi")
        })
}

fn run(
    manager: &ModManager,
    eval: Evaluation,
    mut record: Record,
    write: bool,
) -> Result<NativeReport> {
    let root = manager.game_root();
    let mut report = NativeReport {
        blocked: eval
            .mods
            .iter()
            .filter_map(|(id, block)| Some((id.clone(), block.clone()?)))
            .collect(),
        ..NativeReport::default()
    };

    // 1. What is recorded and not wanted any more goes — the loader last,
    //    and only when nothing is left for it to load.
    let mut unwanted: Vec<String> = record
        .written
        .keys()
        .filter(|rel| !eval.desired.contains_key(*rel))
        .cloned()
        .collect();
    unwanted.sort_by_key(|rel| rel == LOADER_FILE);
    for rel in unwanted {
        let path = game_file(root, &rel);
        let ours = is_ours(root, &record, &rel);
        match path {
            None => {
                record.written.remove(&rel);
            }
            Some(_) if !ours => {
                // Changed since the mod manager wrote it: not ours any more.
                report.warnings.push(format!(
                    "native code: {rel} was changed by another program since the mod manager \
                     installed it; it is left in the game"
                ));
                record.written.remove(&rel);
            }
            Some(_) if rel == LOADER_FILE && any_modules(root) => {
                // Other modules (the crash reports, another mod) still need
                // a loader: it stays, and stays recorded.
            }
            Some(path) => match fs::remove_file(&path) {
                Ok(()) => {
                    record.written.remove(&rel);
                    if rel.ends_with(PLATFORM_FILE) {
                        // The log of the platform module is the toolkit's too.
                        let log = root
                            .join(SCRIPTS_DIR)
                            .join(doctor_toolkit_live::platform::LOG_FILE);
                        let _ = fs::remove_file(log);
                    }
                    report.removed.push(rel);
                }
                Err(e) => report.warnings.push(format!(
                    "native code: {rel} could not be removed ({e}); is the game running? It \
                     is removed the next time"
                )),
            },
        }
        record.save(manager)?;
    }
    // `scripts/` goes only when nothing else is in it.
    if !report.removed.is_empty() {
        let _ = fs::remove_dir(root.join(SCRIPTS_DIR));
    }

    // 2. What is wanted and not there as wanted is written: the record
    //    first, so that a file is never in the game without its record.
    if write {
        let stores: Vec<_> = manager.mods().iter().map(InstalledMod::store).collect();
        for (rel, desired) in &eval.desired {
            let data = match &desired.source {
                Source::Keep => continue,
                Source::Payload(index, blob) => {
                    // The store checks the hash; the DLL is checked again.
                    let data = stores[*index].get(blob)?;
                    check_module(&data).map_err(|why| {
                        Error::mod_error(format!("mod `{}`: {rel}: {why}", desired.owner))
                    })?;
                    data
                }
                Source::File(path) => fs::read(path).map_err(|e| Error::io(path, e))?,
            };
            let sha256 = sha256_hex(&data);
            let existing = game_file(root, rel);
            if existing.as_deref().and_then(hash_of).as_deref() == Some(sha256.as_str()) {
                // There already. A file the mod manager did not write stays
                // unrecorded: it is never removed either.
                continue;
            }
            if existing.is_some() && !is_ours(root, &record, rel) {
                report.warnings.push(format!(
                    "native code: {rel} is in the game and was not installed by the mod \
                     manager; it is left alone"
                ));
                continue;
            }
            let target = existing.unwrap_or_else(|| root.join(rel));
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
            }
            record.written.insert(
                rel.clone(),
                Written {
                    sha256,
                    owner: desired.owner.clone(),
                },
            );
            record.save(manager)?;
            crate::atomic::write(&target, &data)?;
            report.written.push(rel.clone());
        }
    }

    // 3. Answers about mods that are not installed any more are forgotten.
    let before = record.consent.len();
    record.consent.retain(|id, _| manager.get(id).is_some());
    if record.consent.len() != before {
        record.save(manager)?;
    }
    Ok(report)
}

/// Brings the code in the game folder in line with the enabled mods and the
/// player's answers: installs the modules that were agreed to (with the
/// platform module and, when the game has no ASI loader, the toolkit's
/// one), takes away those of mods switched off, removed or refused.
/// "Apply to game" calls it after the data is written.
pub fn sync(manager: &ModManager, host: &NativeHost) -> Result<NativeReport> {
    let record = Record::load(manager)?;
    if record == Record::default() && !manager.mods().iter().any(|m| m.manifest.native.is_some()) {
        return Ok(NativeReport::default());
    }
    let eval = evaluate(manager, host, &record);
    run(manager, eval, record, true)
}

/// Takes away the code that is not wanted any more — of mods switched off,
/// uninstalled or no longer agreed to — and installs nothing. Called
/// whenever the list of mods changes.
pub fn prune(manager: &ModManager) -> Result<NativeReport> {
    let record = Record::load(manager)?;
    if record == Record::default() {
        return Ok(NativeReport::default());
    }
    let eval = evaluate(manager, &NativeHost::permissive(), &record);
    let mut report = run(manager, eval, record, false)?;
    // What a mod waits for is said by the checks, not by a removal.
    report.blocked.clear();
    Ok(report)
}

/// Before the game is started with the mods of `game`: the code of the
/// enabled mods is brought in line with the player's answers, as "Apply to
/// game" does ([`sync`]) — a launch that stages the data of the mods outside
/// the game folder still needs their code inside it. Returns lines for the
/// person; never fails.
pub fn prepare(game: &crate::game::GameInstance) -> Vec<String> {
    match ModManager::open(game) {
        Ok(mut manager) => manager.sync_native().lines(),
        Err(e) => vec![format!("native code: {e}")],
    }
}

/// Takes away everything the mod manager installed: "Restore originals".
/// The answers of the player stay.
pub fn remove_all(manager: &ModManager) -> Result<NativeReport> {
    let record = Record::load(manager)?;
    if record.written.is_empty() {
        return Ok(NativeReport::default());
    }
    run(manager, Evaluation::default(), record, false)
}

impl fmt::Display for NativeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Installed => write!(f, "installed"),
            Self::NotWritten => write!(f, "agreed to, not written into the game yet"),
            Self::Blocked(block) => write!(f, "not installed: {}", block.describe()),
        }
    }
}

#[cfg(test)]
mod tests;
