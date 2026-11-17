//! Crash reports of the game (stage 99): the toolkit's side of the crash
//! module `doctor_crash.asi` (crate `doctor-toolkit-crash-asi`,
//! [`doctor_toolkit_live::crash`], `docs/analysis/crash-reports.md`).
//!
//! - **The module in the game.** Unlike the live module it is not put in
//!   for one launch: a player who installed mods starts the game from his
//!   launcher too, and a crash there must leave a report as well. The
//!   toolkit puts `scripts/doctor_crash.asi` into the game ([`ensure`]:
//!   when it applies mods and when it starts the game) and leaves it there;
//!   [`uninstall`] takes it away and records the player's "no"
//!   ([`set_disabled`]). A file of that name the toolkit did not write is
//!   never replaced.
//! - **The context of a launch** ([`write_context`]): the enabled mods and
//!   their versions go to `_doctor_toolkit/crashes/context.txt`; the module
//!   copies them into a report.
//! - **The reports** ([`reports`], [`last`]): the newest first, each with
//!   its short story ([`Summary`]); [`annotate`] adds the names of the
//!   game's messages the module cannot know.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub use doctor_toolkit_live::crash::{
    CONTEXT_FILE, CRASH_DIR, Context, ContextMod, LOG_FILE, MODULE_FILE, Report, SYMBOLS, Summary,
    Symbol, TEST_FILE, TEST_KINDS,
};

use crate::game::GameInstance;
use crate::live::SCRIPTS_DIR;
use crate::{Error, Result};

/// The hash of the module the toolkit installed, in [`CRASH_DIR`].
const WRITTEN: &str = "module.sha256";
/// Marks that the player does not want the module, in [`CRASH_DIR`].
const DISABLED: &str = "disabled";
/// The mark of the player's agreement to keep the module in the game.
const AGREED: &str = "agreed";

/// The folder of the reports of the game in `root`.
pub fn dir(root: &Path) -> PathBuf {
    root.join(CRASH_DIR)
}

/// The module file to install: `DOCTOR_CRASH_MODULE`, `doctor_crash.asi`
/// next to the running program, or the build of this repository
/// (`tools/build-crash-module.sh` → `target/live/doctor_crash.asi`).
pub fn module_binary() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("DOCTOR_CRASH_MODULE") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join(MODULE_FILE));
        candidates.push(dir.join("../share/doctor_toolkit").join(MODULE_FILE));
        candidates.push(dir.join("../live").join(MODULE_FILE));
    }
    candidates.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/live")
            .join(MODULE_FILE),
    );
    candidates.into_iter().find(|p| p.is_file())
}

// ---------------------------------------------------------- the module

/// The module in a game folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installed {
    /// No `scripts/doctor_crash.asi`.
    No,
    /// The file the toolkit wrote.
    Ours,
    /// A file of that name the toolkit did not write (or one changed
    /// since): it is left alone.
    Foreign,
}

fn module_path(root: &Path) -> PathBuf {
    root.join(SCRIPTS_DIR).join(MODULE_FILE)
}

/// What is in `scripts/` of the game in `root`.
pub fn installed(root: &Path) -> Installed {
    let Ok(bytes) = std::fs::read(module_path(root)) else {
        return Installed::No;
    };
    let written = std::fs::read_to_string(dir(root).join(WRITTEN)).unwrap_or_default();
    if written.trim() == crate::workspace::store::sha256_hex(&bytes) {
        Installed::Ours
    } else {
        Installed::Foreign
    }
}

/// Whether the player switched the crash reports off for this game.
pub fn disabled(root: &Path) -> bool {
    dir(root).join(DISABLED).is_file()
}

/// What the player said about keeping the module in the game folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consent {
    /// Nobody asked yet: the module is not installed until the player
    /// answers (the mod manager and the editor ask once).
    Unasked,
    /// The player agreed, or the toolkit's module is already in the game.
    Yes,
    /// The player refused or switched the reports off.
    No,
}

/// The player's answer for the game in `root`. It is kept in the folder of
/// the reports, so the mod manager, the editor and the command line share it.
pub fn consent(root: &Path) -> Consent {
    if disabled(root) {
        Consent::No
    } else if dir(root).join(AGREED).is_file() || installed(root) == Installed::Ours {
        Consent::Yes
    } else {
        Consent::Unasked
    }
}

fn remove_mark(mark: &Path) -> Result<()> {
    match std::fs::remove_file(mark) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io(mark, e)),
    }
}

/// Records the player's choice ([`consent`] becomes `No` or `Yes`);
/// switching off does not remove the module ([`uninstall`] does both).
pub fn set_disabled(root: &Path, off: bool) -> Result<()> {
    let (set, clear) = if off {
        (DISABLED, AGREED)
    } else {
        (AGREED, DISABLED)
    };
    std::fs::create_dir_all(dir(root)).map_err(|e| Error::io(dir(root), e))?;
    crate::atomic::write(&dir(root).join(set), b"")?;
    remove_mark(&dir(root).join(clear))
}

/// Puts `module` (the DLL bytes) into `scripts/` of the game. Refuses to
/// replace a file the toolkit did not write.
pub fn install(root: &Path, module: &[u8]) -> Result<PathBuf> {
    let target = module_path(root);
    if installed(root) == Installed::Foreign {
        return Err(Error::InvalidArgument(format!(
            "{} was not installed by the toolkit; it is left alone",
            target.display()
        )));
    }
    let scripts = root.join(SCRIPTS_DIR);
    std::fs::create_dir_all(&scripts).map_err(|e| Error::io(&scripts, e))?;
    std::fs::create_dir_all(dir(root)).map_err(|e| Error::io(dir(root), e))?;
    crate::atomic::write(
        &dir(root).join(WRITTEN),
        crate::workspace::store::sha256_hex(module).as_bytes(),
    )?;
    crate::atomic::write(&target, module)?;
    Ok(target)
}

/// Takes the module away (only the file the toolkit wrote) and records
/// that the player does not want it. Returns whether a file was removed.
pub fn uninstall(root: &Path) -> Result<bool> {
    let removed = match installed(root) {
        Installed::Ours => {
            let target = module_path(root);
            std::fs::remove_file(&target).map_err(|e| Error::io(&target, e))?;
            let _ = std::fs::remove_file(dir(root).join(WRITTEN));
            // `scripts/` goes only when nothing else is in it.
            let _ = std::fs::remove_dir(root.join(SCRIPTS_DIR));
            true
        }
        Installed::Foreign => {
            return Err(Error::InvalidArgument(format!(
                "{} was not installed by the toolkit; remove it by hand",
                module_path(root).display()
            )));
        }
        Installed::No => false,
    };
    set_disabled(root, true)?;
    Ok(removed)
}

/// What [`ensure`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ensured {
    /// The module was installed or brought up to date.
    Installed,
    /// The right module was there already.
    Present,
    /// Nothing was done, and why.
    Skipped(String),
}

impl Ensured {
    /// A line for the person; `None` when there is nothing to say.
    pub fn note(&self) -> Option<String> {
        match self {
            Self::Installed => Some(format!(
                "crash reports: {MODULE_FILE} installed in scripts/ (reports go to {CRASH_DIR}; `crash-report uninstall` takes it away)"
            )),
            Self::Present => None,
            Self::Skipped(why) => Some(format!("crash reports: off — {why}")),
        }
    }
}

/// Makes sure the game in `root` has the current crash module, unless the
/// player switched it off: called when mods are applied and before the
/// game is started. Never fails a launch: what cannot be done is said in
/// the result.
pub fn ensure(root: &Path) -> Ensured {
    if disabled(root) {
        return Ensured::Skipped("switched off for this game".into());
    }
    let state = installed(root);
    if state == Installed::Foreign {
        return Ensured::Skipped(format!(
            "scripts/{MODULE_FILE} was not installed by the toolkit and is left as it is"
        ));
    }
    let Some(path) = module_binary() else {
        return match state {
            Installed::Ours => Ensured::Present,
            _ => Ensured::Skipped(format!(
                "{MODULE_FILE} is not next to the program (a file of the release; from the sources: tools/build-crash-module.sh)"
            )),
        };
    };
    if crate::live::loader::find(root).is_none() {
        return Ensured::Skipped(
            "no ASI loader (dinput8.dll) in the game folder: the module cannot load".into(),
        );
    }
    // The module works on one build only; on another it would install
    // nothing, so it is not put there at all.
    let exe = crate::play::find_executable(root);
    let known = exe
        .and_then(|exe| std::fs::read(exe).ok())
        .is_some_and(|data| {
            doctor_toolkit_live::edition::check_file_with(&data, doctor_toolkit_live::crash::CHECKS)
                .is_ok()
        });
    if !known {
        return Ensured::Skipped(format!(
            "the executable is not {}",
            doctor_toolkit_live::edition::NAME
        ));
    }
    let Ok(bytes) = std::fs::read(&path) else {
        return Ensured::Skipped(format!("{} cannot be read", path.display()));
    };
    if state == Installed::Ours && std::fs::read(module_path(root)).ok().as_ref() == Some(&bytes) {
        return Ensured::Present;
    }
    match install(root, &bytes) {
        Ok(_) => Ensured::Installed,
        Err(e) => Ensured::Skipped(e.to_string()),
    }
}

// ---------------------------------------------------------- the context

/// `YYYY-MM-DD HH:MM:SS` (UTC) of now.
fn now() -> String {
    crate::report::timestamp(SystemTime::now())
        .replace('T', " ")
        .trim_end_matches('Z')
        .to_owned()
}

/// The context of a launch of `game`: its enabled mods in load order.
/// `source` says how they reach the game (`written`, `staged`, a
/// workspace…), `program` who writes.
pub fn context(game: &GameInstance, program: &str, source: &str) -> Context {
    let mods = crate::mods::ModManager::open(game)
        .map(|manager| {
            manager
                .mods()
                .iter()
                .filter(|m| m.enabled && m.problem.is_none())
                .map(|m| ContextMod {
                    id: m.id.clone(),
                    version: m.manifest.version.clone(),
                    name: m.manifest.name.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    Context {
        writer: crate::report::version_line(program),
        written: now(),
        source: source.to_owned(),
        mods,
    }
}

/// Writes the context for the module of the game in `root`.
pub fn write_context(root: &Path, context: &Context) -> Result<()> {
    let dir = dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
    crate::atomic::write(&dir.join(CONTEXT_FILE), context.to_text().as_bytes())
}

/// Before a launch of `game` and after its mods changed: the module and
/// the context. With `install` the module is put into the game when it is
/// not there ([`ensure`]) and the player agreed to it ([`consent`]: nothing
/// is installed before the answer); without, a module the toolkit installed
/// earlier is only brought up to date. Returns lines for the person; never
/// fails.
pub fn prepare(game: &GameInstance, program: &str, source: &str, install: bool) -> Vec<String> {
    let root = game.root();
    let mut notes = Vec::new();
    if (install && consent(root) == Consent::Yes) || installed(root) == Installed::Ours {
        notes.extend(ensure(root).note());
    }
    // The context only where a module can read it.
    if installed(root) != Installed::No
        && let Err(e) = write_context(root, &context(game, program, source))
    {
        notes.push(format!(
            "crash reports: the list of mods was not written: {e}"
        ));
    }
    notes
}

/// After a game that ran in a mirror of its folder (Large Address Aware)
/// exited: the reports the module wrote into the mirror move to the game
/// folder. Returns how many files moved.
pub fn collect_from(mirror_root: &Path, game_root: &Path) -> usize {
    let from = dir(mirror_root);
    let Ok(entries) = std::fs::read_dir(&from) else {
        return 0;
    };
    let to = dir(game_root);
    let mut moved = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        // Links are the game folder's own files, mirrored.
        let real = std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file());
        let Some(name) = path.file_name() else {
            continue;
        };
        if real
            && std::fs::create_dir_all(&to).is_ok()
            && std::fs::copy(&path, to.join(name)).is_ok()
        {
            moved += 1;
        }
    }
    moved
}

/// Asks the module of a running game to crash it on purpose (`kind` of
/// [`TEST_KINDS`]); the module looks once a second.
pub fn request_test(root: &Path, kind: &str) -> Result<()> {
    if !TEST_KINDS.contains(&kind) {
        return Err(Error::InvalidArgument(format!(
            "unknown kind of crash `{kind}`: one of {}",
            TEST_KINDS.join(", ")
        )));
    }
    let dir = dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
    crate::atomic::write(&dir.join(TEST_FILE), kind.as_bytes())
}

// ---------------------------------------------------------- the reports

/// A report on the disk.
#[derive(Debug, Clone)]
pub struct CrashReport {
    /// The text report.
    pub path: PathBuf,
    /// The minidump next to it, when it was written.
    pub dump: Option<PathBuf>,
    /// The file name without its extension: `crash-20261116-125500`.
    pub name: String,
    pub summary: Summary,
}

impl CrashReport {
    /// One line: when, what and where.
    pub fn line(&self) -> String {
        let s = &self.summary;
        let mut out = format!("{}  {}", s.time, s.exception);
        if let Some(a) = s.address {
            out.push_str(&format!(" at {a:#010x} {}", s.place));
        }
        out
    }

    /// The name the knowledge base gives the place, when it has one: what
    /// it says about the address itself, else the function around it.
    pub fn place_name(&self) -> Option<&str> {
        let s = &self.summary;
        s.known
            .first()
            .map(String::as_str)
            .or_else(|| s.function.split_once(": ").map(|(_, name)| name))
    }
}

impl CrashReport {
    /// The summary of the report as JSON: what the command line prints with
    /// `--json` and the MCP server answers.
    pub fn json(&self) -> serde_json::Value {
        let s = &self.summary;
        serde_json::json!({
            "name": self.name,
            "report": self.path,
            "minidump": self.dump,
            "time": s.time,
            "exception": s.exception,
            "address": s.address.map(|a| format!("{a:#010x}")),
            "place": s.place,
            "known": s.known,
            "function": s.function,
            "module": s.module,
        })
    }
}

/// One of `reports` (newest first): the newest, number `which` from 1, or
/// the one of that name.
pub fn pick(reports: Vec<CrashReport>, which: Option<&str>) -> Option<CrashReport> {
    match which.map(str::trim) {
        None => reports.into_iter().next(),
        Some(w) => match w.parse::<usize>() {
            Ok(n) => reports.into_iter().nth(n.wrapping_sub(1)),
            Err(_) => reports
                .into_iter()
                .find(|r| r.name == w.trim_end_matches(".txt")),
        },
    }
}

/// The reports of the game in `root`, newest first. Files that are not
/// reports are skipped.
pub fn reports(root: &Path) -> Vec<CrashReport> {
    let dir = dir(root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| doctor_toolkit_live::crash::is_report_name(n))
        .collect();
    // The names sort by time.
    names.sort();
    names.reverse();
    names
        .into_iter()
        .filter_map(|file| {
            let path = dir.join(&file);
            let text = std::fs::read_to_string(&path).ok()?;
            let report = Report::parse(&text)?;
            let dump = path.with_extension("dmp");
            Some(CrashReport {
                dump: dump.is_file().then_some(dump),
                name: file.trim_end_matches(".txt").to_owned(),
                summary: report.summary(),
                path,
            })
        })
        .collect()
}

/// The newest report of the game in `root`.
pub fn last(root: &Path) -> Option<CrashReport> {
    reports(root).into_iter().next()
}

/// The text of a report with the names of the messages added: the module
/// knows the engine's names only, the toolkit knows those of the game's
/// files too (`0x1234abcd` in `[messages]` becomes `0x1234abcd name`).
pub fn annotate(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 1024);
    let mut in_messages = false;
    for line in text.lines() {
        if line.starts_with('[') {
            in_messages = line == "[messages]";
        }
        out.push_str(&named(line, in_messages).unwrap_or_else(|| line.to_owned()));
        out.push('\n');
    }
    out
}

/// A line of `[messages]` with the name of its message, when the line has
/// a hash without a name and the toolkit knows one.
fn named(line: &str, in_messages: bool) -> Option<String> {
    if !in_messages || line.starts_with('#') {
        return None;
    }
    // `-0.001 s  #7  0xc2010fb7 [name]  data …`
    let at = line.find("  0x")? + 2;
    let hash = u32::from_str_radix(line.get(at + 2..at + 10)?, 16).ok()?;
    let rest = line.get(at + 10..)?;
    if !rest.starts_with("  ") {
        // The module named it already.
        return None;
    }
    let id = crate::game_types::MessageId::from_hash(hash);
    let name = id.name()?;
    Some(format!("{} {name}{rest}", &line[..at + 10]))
}

/// What the knowledge base says about `address`: the lines of the table
/// for the address itself, else the nearest named address of the code
/// below it.
pub fn lookup(address: u32) -> Vec<String> {
    use doctor_toolkit_live::crash::{in_text, symbol_before, symbols};
    let exact: Vec<String> = symbols(SYMBOLS)
        .filter(|s| s.address == address)
        .map(|s| format!("{address:#010x} = [{}] {}", s.note, s.what))
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    if !in_text(address) {
        return Vec::new();
    }
    symbol_before(SYMBOLS, address, 0)
        .map(|s| {
            format!(
                "{address:#010x} is {:#x} after {:#010x} [{}] {} (the nearest named address below; it may be another function)",
                address - s.address,
                s.address,
                s.note,
                s.what
            )
        })
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPORT: &str = "doctor_crash 0.0.6 on PC 1.0.789.0 (build of 2009-10-05)\n\
time: 2026-11-16 12:55:00 UTC (2026-11-16 15:55:00 local)\n\n[exception]\n\
code: 0xC0000005 ACCESS_VIOLATION (read of 0x0000000c)\n\
address: 0x0042138b godfather2.exe+0x0002138b\n\
known: [known-crashes] падение при `-stream` с двумя и более потоками\n\
function: 0x004212b0 +0xdb; the knowledge base names 0x004212b0 in it: [resource-loader] привязка ресурса\n\n\
[messages]\n# The last 48 in order, newest first:\n\
-0.001 s  #7  0xc2010fb7 iMsgStreamBeginLoad  data 0x0389a848  thread 0x24  from 0x00402f78 x\n\
-0.002 s  #6  0x5c1a9b0e  data 0x00000000  thread 0x24  from 0x00402f78 x\n\n[streams]\nloaded: gf2\\cuba.str\n";

    fn game() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        crate::testing::write_game(dir.path()).unwrap();
        let root = dir.path().to_path_buf();
        (dir, root)
    }

    /// Nothing is installed before the player answers; the answer is kept
    /// in the folder of the reports and can be changed.
    #[test]
    fn the_module_waits_for_the_players_answer() {
        let (_keep, root) = game();
        assert_eq!(consent(&root), Consent::Unasked);
        let game = GameInstance::open(&root).unwrap();
        // A launch that would install the module does not, unasked.
        prepare(&game, "test", "written into the game", true);
        assert_eq!(installed(&root), Installed::No);
        assert!(!dir(&root).join(CONTEXT_FILE).exists());
        set_disabled(&root, true).unwrap();
        assert_eq!(consent(&root), Consent::No);
        set_disabled(&root, false).unwrap();
        assert_eq!(consent(&root), Consent::Yes);
        assert!(!disabled(&root));
        // A module the toolkit installed earlier counts as agreement.
        std::fs::remove_file(dir(&root).join(AGREED)).unwrap();
        assert_eq!(consent(&root), Consent::Unasked);
        install(&root, b"MZ module").unwrap();
        assert_eq!(consent(&root), Consent::Yes);
        uninstall(&root).unwrap();
        assert_eq!(consent(&root), Consent::No);
    }

    #[test]
    fn reports_are_listed_newest_first() {
        let (_keep, root) = game();
        assert!(reports(&root).is_empty());
        assert!(last(&root).is_none());
        let dir = dir(&root);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("crash-20261116-125500.txt"), REPORT).unwrap();
        std::fs::write(dir.join("crash-20261116-125500.dmp"), b"MDMP").unwrap();
        std::fs::write(
            dir.join("crash-20261117-080000.txt"),
            REPORT.replace("12:55:00 UTC", "08:00:00 UTC"),
        )
        .unwrap();
        std::fs::write(dir.join("crash-broken.txt"), "not a report").unwrap();
        std::fs::write(dir.join(LOG_FILE), "log").unwrap();
        let all = reports(&root);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "crash-20261117-080000");
        assert!(all[0].dump.is_none());
        assert!(all[1].dump.is_some());
        assert_eq!(all[1].summary.address, Some(0x0042_138B));
        assert!(all[1].line().contains("at 0x0042138b godfather2.exe+"));
        assert!(all[1].place_name().unwrap().starts_with("[known-crashes]"));
    }

    #[test]
    fn the_names_of_game_messages_are_added() {
        let hash = crate::game_types::MessageCatalog::gf2()
            .iter()
            .next()
            .unwrap();
        let text = REPORT.replace("5c1a9b0e", &format!("{:08x}", hash.hash));
        let out = annotate(&text);
        assert!(
            out.contains(&format!("0x{:08x} {}  data", hash.hash, hash.name)),
            "{out}"
        );
        // Named lines and other sections stay as they were.
        assert!(out.contains("0xc2010fb7 iMsgStreamBeginLoad  data"));
        assert!(out.contains("loaded: gf2\\cuba.str"));
        assert_eq!(out.lines().count(), text.lines().count());
    }

    #[test]
    fn the_module_is_installed_once_and_never_over_a_foreign_file() {
        let (_keep, root) = game();
        assert_eq!(installed(&root), Installed::No);
        install(&root, b"module one").unwrap();
        assert_eq!(installed(&root), Installed::Ours);
        install(&root, b"module two").unwrap();
        assert_eq!(std::fs::read(module_path(&root)).unwrap(), b"module two");
        // Someone changed the file: it is not ours any more.
        std::fs::write(module_path(&root), b"someone's").unwrap();
        assert_eq!(installed(&root), Installed::Foreign);
        assert!(install(&root, b"module three").is_err());
        assert!(uninstall(&root).is_err());
        assert!(matches!(ensure(&root), Ensured::Skipped(_)));
        assert_eq!(std::fs::read(module_path(&root)).unwrap(), b"someone's");
    }

    #[test]
    fn uninstall_takes_the_module_away_and_switches_it_off() {
        let (_keep, root) = game();
        install(&root, b"module").unwrap();
        assert!(uninstall(&root).unwrap());
        assert_eq!(installed(&root), Installed::No);
        assert!(disabled(&root));
        assert_eq!(
            ensure(&root),
            Ensured::Skipped("switched off for this game".into())
        );
        set_disabled(&root, false).unwrap();
        assert!(!disabled(&root));
        // The mini game has no ASI loader (or no built module here).
        assert!(matches!(ensure(&root), Ensured::Skipped(_)));
    }

    #[test]
    fn the_context_lists_the_enabled_mods() {
        let (_keep, root) = game();
        let game = GameInstance::open(&root).unwrap();
        let c = context(&game, "doctor-toolkit", "written");
        assert!(c.mods.is_empty());
        assert!(c.writer.starts_with("doctor-toolkit "));
        assert_eq!(c.written.len(), "2026-11-16 12:00:00".len());
        write_context(&root, &c).unwrap();
        let back = Context::parse(&std::fs::read_to_string(dir(&root).join(CONTEXT_FILE)).unwrap());
        assert_eq!(back, c);
    }

    #[test]
    fn a_test_crash_is_asked_for_by_a_file() {
        let (_keep, root) = game();
        assert!(request_test(&root, "explode").is_err());
        request_test(&root, "jump").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir(&root).join(TEST_FILE)).unwrap(),
            "jump"
        );
    }

    #[test]
    fn an_address_is_looked_up_in_the_knowledge_base() {
        let known = lookup(0x6E75_6F7B);
        assert!(known[0].contains("[known-crashes]"), "{known:?}");
        let near = lookup(0x0040_8A40);
        assert!(near[0].contains("after 0x00408a"), "{near:?}");
        assert!(lookup(0x7000_0000).is_empty());
    }
}
