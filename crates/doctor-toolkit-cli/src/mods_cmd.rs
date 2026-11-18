//! `mods` subcommands (building, installing and applying mods) and `integrity`.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::mods::integrity::{self, Condition, IntegrityOptions};
use doctor_toolkit::mods::manager::{InstallReport, ModApplyOptions};
use doctor_toolkit::mods::native::{self, Answer, NativeState};
use doctor_toolkit::mods::vpat::{self, VpatPatch};
use doctor_toolkit::mods::{
    self, Incompatible, ModArchive, ModInfo, ModManager, NativeBuild, Preview, Relations,
};
use doctor_toolkit::workspace::{ApplyReport, ResourceKey, StreamAction, Workspace};

macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

/// Metadata of a mod being built.
#[derive(Args)]
pub struct MetaArgs {
    /// Identifier: lower-case letters, digits, `-` and `_` [default: made from the name]
    #[arg(long)]
    id: Option<String>,
    /// Name shown to players
    #[arg(long)]
    name: String,
    /// Version of the mod
    #[arg(long = "mod-version", default_value = "1.0")]
    version: String,
    #[arg(long, default_value = "")]
    author: String,
    #[arg(long, default_value = "")]
    description: String,
    /// Home page, http:// or https://
    #[arg(long)]
    url: Option<String>,
    /// A mod this one needs: `ID` or `ID=CONDITION` such as `nny=">=1.1 <2"`
    /// (repeat for more). Mods the game has written that the changes were
    /// made on are added by themselves
    #[arg(long = "depends", value_name = "ID[=CONDITION]")]
    depends: Vec<String>,
    /// A mod that goes well with this one: `ID` or `ID=CONDITION` (a hint)
    #[arg(long = "recommends", value_name = "ID[=CONDITION]")]
    recommends: Vec<String>,
    /// A mod this one does not work with: `ID`, `ID=CONDITION` or
    /// `ID=CONDITION:REASON`
    #[arg(long = "incompatible", value_name = "ID[=CONDITION][:REASON]")]
    incompatible: Vec<String>,
    /// Load this mod after mod ID when it is installed (a soft rule)
    #[arg(long = "load-after", value_name = "ID")]
    load_after: Vec<String>,
    /// Load this mod before mod ID when it is installed (a soft rule)
    #[arg(long = "load-before", value_name = "ID")]
    load_before: Vec<String>,
    /// An edition of the game the mod is tested on, e.g. 1.0.789.0, or
    /// `this` for the edition of this game (repeat for more). On another
    /// edition the mod does not fit unless compatibility is ignored
    /// [default: none, any edition: the resources are checked one by one]
    #[arg(long = "edition", value_name = "EDITION")]
    editions: Vec<String>,
    /// Versions of the mod manager the mod needs, e.g. ">=0.0.5"; `none`
    /// for no condition [default: this program's version or newer]
    #[arg(long = "requires-manager", value_name = "CONDITION")]
    requires_manager: Option<String>,
    /// Executable code of the mod: a 32-bit DLL for the `scripts` folder of
    /// the game, `FILE` or `NAME.asi=FILE` (repeat for more). The mod is
    /// written in format version 5; the mod manager installs the code only
    /// after the player agrees to it, and only into the edition 1.0.789.0
    #[arg(long = "native", value_name = "[NAME.asi=]FILE")]
    native: Vec<String>,
    /// The mod makes no sense without its code: it is not written into the
    /// game at all while the code is not installed
    #[arg(long = "native-required", requires = "native")]
    native_required: bool,
    /// The modules are plain ASI plug-ins that do not use doctor_platform.asi
    #[arg(
        long = "native-plain",
        requires = "native",
        conflicts_with = "platform_abi"
    )]
    native_plain: bool,
    /// The ABI of the platform module the code was built with
    /// (DOCTOR_ABI_VERSION of doctor_sdk.h) [default: the one of this program]
    #[arg(long = "platform-abi", value_name = "N", requires = "native")]
    platform_abi: Option<u32>,
    /// Keep every resource whole. Without it a changed SimGroup, string
    /// table, parts database or world grid goes into the mod as a delta —
    /// the operations that make the change — which merges with other mods
    /// and needs format version 6; with it any mod manager installs the
    /// mod, and it replaces what other mods do to the same resources
    #[arg(long = "no-deltas")]
    no_deltas: bool,
}

/// `ID` or `ID=CONDITION`.
fn id_condition(text: &str) -> (String, String) {
    match text.split_once('=') {
        Some((id, condition)) => (id.trim().to_owned(), condition.trim().to_owned()),
        None => (text.trim().to_owned(), "*".to_owned()),
    }
}

impl MetaArgs {
    /// The modules of `--native`, read from the disk.
    fn native(&self) -> Result<Option<NativeBuild>> {
        let spec = native::NativeSpec {
            modules: self.native.clone(),
            required: self.native_required,
            plain: self.native_plain,
            platform_abi: self.platform_abi,
        };
        let (build, warnings) = spec.read()?;
        for warning in warnings {
            out!("warning: {warning}");
        }
        Ok(build)
    }

    fn info(&self) -> ModInfo {
        let editions = self
            .editions
            .iter()
            .filter(|e| e.as_str() != "any")
            .cloned()
            .collect();
        let requires_manager = self.requires_manager.as_ref().map(|r| {
            if r.trim() == "none" {
                String::new()
            } else {
                r.clone()
            }
        });
        let relations = Relations {
            depends: self.depends.iter().map(|d| id_condition(d)).collect(),
            recommends: self.recommends.iter().map(|d| id_condition(d)).collect(),
            incompatible: self
                .incompatible
                .iter()
                .map(|text| {
                    let (id, rest) = id_condition(text);
                    let (versions, reason) = match rest.split_once(':') {
                        Some((v, r)) => (v.trim().to_owned(), r.trim().to_owned()),
                        None => (rest, String::new()),
                    };
                    let versions = if versions.is_empty() {
                        "*".to_owned()
                    } else {
                        versions
                    };
                    (id, Incompatible { versions, reason })
                })
                .collect(),
            load_after: self.load_after.clone(),
            load_before: self.load_before.clone(),
            editions,
            requires_manager,
            made_on: Default::default(),
            made_on_data: Default::default(),
            native: None,
        };
        ModInfo {
            id: self
                .id
                .clone()
                .unwrap_or_else(|| mods::id_from_name(&self.name)),
            name: self.name.clone(),
            version: self.version.clone(),
            author: self.author.clone(),
            description: self.description.replace("\\n", "\n"),
            url: self.url.clone(),
            relations,
            whole_resources: self.no_deltas,
        }
    }

    /// [`Self::info`] with the dependencies on the mods in the game that the
    /// changes were made on; tells which were added.
    fn info_for(
        &self,
        game: &GameInstance,
        ws: &Workspace,
        keys: Option<&[ResourceKey]>,
    ) -> Result<ModInfo> {
        let mut info = self.info();
        info.relations.native = self.native()?;
        for edition in &mut info.relations.editions {
            if edition == "this" {
                *edition =
                    doctor_toolkit::edition::game_edition(game.root()).with_context(|| {
                        "`--edition this`: the edition of this game is not known".to_owned()
                    })?;
            }
        }
        for id in mods::add_suggested_dependencies(&mut info, game, ws, keys)? {
            out!(
                "depends on {id} {}: the changes are made on what it wrote into the game",
                info.relations.depends[&id]
            );
        }
        Ok(info)
    }
}

#[derive(Subcommand)]
pub enum ModsCommand {
    /// Build a mod (.zip) from the changes of a workspace
    Build {
        game_dir: PathBuf,
        /// The mod file to write
        #[arg(short, long)]
        output: PathBuf,
        #[command(flatten)]
        meta: MetaArgs,
        /// A PNG or JPEG picture shown in the mod manager
        #[arg(long)]
        preview: Option<PathBuf>,
        /// Workspace directory [default: <GAME_DIR>/_doctor_toolkit/workspace]
        #[arg(long = "dir", value_name = "DIR")]
        dir: Option<PathBuf>,
        /// Only these changes: resource keys or text that matches part of them [default: all]
        keys: Vec<String>,
    },
    /// Show what a mod (.zip) or an old toolkit patch (.vpat) contains, and check it
    Info {
        file: PathBuf,
        /// List every resource
        #[arg(long)]
        resources: bool,
    },
    /// List the installed mods in load order
    List { game_dir: PathBuf },
    /// Install mods (.zip) or old toolkit patches (.vpat); `apply` writes them into the game
    Install {
        game_dir: PathBuf,
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Install without enabling
        #[arg(long)]
        disabled: bool,
    },
    /// Sort the load order by the rules of the mods (dependencies first,
    /// then load_after/load_before), keeping the present order where they allow
    Sort {
        game_dir: PathBuf,
        /// Only show the order, do not change it
        #[arg(long)]
        dry_run: bool,
    },
    /// Install the changes of a workspace as a mod, last in the load order
    InstallWorkspace {
        game_dir: PathBuf,
        #[command(flatten)]
        meta: MetaArgs,
        /// Workspace directory [default: <GAME_DIR>/_doctor_toolkit/workspace]
        #[arg(long = "dir", value_name = "DIR")]
        dir: Option<PathBuf>,
    },
    /// Remove installed mods; `apply` then updates the game
    Uninstall {
        game_dir: PathBuf,
        #[arg(required_unless_present = "all")]
        ids: Vec<String>,
        /// Remove every mod
        #[arg(long, conflicts_with = "ids")]
        all: bool,
        /// Remove a mod even though enabled mods need it
        #[arg(long)]
        force: bool,
    },
    /// Enable mods
    Enable {
        game_dir: PathBuf,
        #[arg(required = true)]
        ids: Vec<String>,
        /// Also enable the installed mods they need
        #[arg(long)]
        with_deps: bool,
    },
    /// Disable mods
    Disable {
        game_dir: PathBuf,
        #[arg(required = true)]
        ids: Vec<String>,
        /// Disable a mod even though enabled mods need it
        #[arg(long)]
        force: bool,
    },
    /// Move a mod in the load order (1 is applied first; the last one wins)
    Move {
        game_dir: PathBuf,
        id: String,
        position: usize,
    },
    /// List resources changed by more than one mod and which mod wins, and clashes
    /// of mods that add resources or streams
    Conflicts {
        game_dir: PathBuf,
        /// Include disabled mods
        #[arg(long)]
        all: bool,
    },
    /// Check that the enabled mods fit the game files
    Check { game_dir: PathBuf },
    /// Compare the enabled mods with the game files
    Status { game_dir: PathBuf },
    /// Write the enabled mods into the game, in load order
    Apply {
        game_dir: PathBuf,
        /// Overwrite files changed by another program
        #[arg(long)]
        force: bool,
        /// Apply although the game's originals differ from what the mods expect
        #[arg(long)]
        ignore_compatibility: bool,
    },
    /// Put back the originals of every file the mod manager wrote; the mods stay installed
    Unapply {
        game_dir: PathBuf,
        /// Also restore files changed by another program since
        #[arg(long)]
        force: bool,
    },
    /// Write an installed mod as a mod file (e.g. a converted .vpat patch)
    Export {
        game_dir: PathBuf,
        id: String,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Take over a game patched by the old toolkit: adopt its backups as the
    /// originals and install its patches as mods
    ImportOld { game_dir: PathBuf },
    /// Executable code of mods (format version 5): without ID, the state of
    /// the code of every mod and the files the mod manager has in the game;
    /// with ID, what the mod would install — and your answer. Nothing
    /// executable is installed before `--allow`
    Native {
        game_dir: PathBuf,
        /// The mod
        id: Option<String>,
        /// Agree to the code of the mod as it is now (each file by its
        /// SHA-256); `mods apply` installs it
        #[arg(long, requires = "id", conflicts_with_all = ["data_only", "forget"])]
        allow: bool,
        /// Use the mod without its code: only its data is written
        #[arg(long, requires = "id", conflicts_with = "forget")]
        data_only: bool,
        /// Forget the answer: the code leaves the game and waits again
        #[arg(long, requires = "id")]
        forget: bool,
    },
}

/// `integrity`.
#[derive(Args)]
pub struct IntegrityArgs {
    game_dir: PathBuf,
    /// Also hash the files without backups and compare them with the reference list (slow)
    #[arg(long)]
    full: bool,
    /// Save the hashes of the original files as the reference list (do this on an unmodified game)
    #[arg(long, conflicts_with = "full")]
    save_reference: bool,
    /// Copy the old toolkit's backups to this toolkit's backups where it has none
    #[arg(long, conflicts_with_all = ["full", "save_reference"])]
    adopt_old_backups: bool,
}

pub fn run(command: ModsCommand) -> Result<()> {
    match command {
        ModsCommand::Build {
            game_dir,
            output,
            meta,
            preview,
            dir,
            keys,
        } => {
            let game = GameInstance::open(&game_dir)?;
            let ws = open_workspace(&game, dir.as_deref())?;
            let keys = match_keys(&ws, &keys)?;
            let preview = preview
                .map(|path| {
                    let data =
                        std::fs::read(&path).with_context(|| format!("{}", path.display()))?;
                    Ok::<_, anyhow::Error>(Preview::from_image(data)?)
                })
                .transpose()?;
            let info = meta.info_for(&game, &ws, keys.as_deref())?;
            let report =
                mods::build_from_workspace(&ws, &info, keys.as_deref(), preview.as_ref(), &output)?;
            out!(
                "built {} ({} {}, format {}): {} resource(s){}, {} byte(s) of data, file {} byte(s)",
                output.display(),
                info.id,
                info.version,
                report.format_version,
                report.resources,
                adds_note(report.added, report.new_streams),
                report.payload_bytes,
                report.file_size
            );
            if report.native_modules > 0 {
                out!(
                    "the mod carries {} module(s) of executable code: the mod manager asks \
                     the player before it installs them",
                    report.native_modules
                );
            }
            if report.delta_kinds > 0 && !info.whole_resources {
                out!(
                    "{} of {} resource(s) that have deltas are stored as deltas: they merge \
                     with other mods (`--no-deltas` keeps every resource whole, for older mod \
                     managers)",
                    report.deltas,
                    report.delta_kinds
                );
            }
            // The first note of a mod with stale changes is their warning.
            for (i, note) in report.notes.iter().enumerate() {
                match i == 0 && !report.stale.is_empty() {
                    true => out!("warning: {note}"),
                    false => out!("note: {note}"),
                }
            }
            Ok(())
        }
        ModsCommand::Info { file, resources } => info(&file, resources),
        ModsCommand::List { game_dir } => {
            let (_, mods) = open(&game_dir)?;
            list(&mods);
            Ok(())
        }
        ModsCommand::Install {
            game_dir,
            files,
            disabled,
        } => {
            let (game, mut mods) = open(&game_dir)?;
            for file in &files {
                let report = mods.install_file(&game, file, !disabled)?;
                print_install(&report);
                ask_about_code(&game, &mods, &report.id)?;
            }
            out!(
                "run `doctor-toolkit mods apply {}` to write the mods into the game",
                quote(game.root())
            );
            Ok(())
        }
        ModsCommand::InstallWorkspace {
            game_dir,
            meta,
            dir,
        } => {
            let (game, mut mods) = open(&game_dir)?;
            let ws = open_workspace(&game, dir.as_deref())?;
            let info = meta.info_for(&game, &ws, None)?;
            let report = mods.install_workspace(&game, &ws, &info, None)?;
            print_install(&report);
            ask_about_code(&game, &mods, &report.id)?;
            Ok(())
        }
        ModsCommand::Uninstall {
            game_dir,
            ids,
            all,
            force,
        } => {
            let (game, mut mods) = open(&game_dir)?;
            let ids = if all {
                mods.mods().iter().map(|m| m.id.clone()).collect()
            } else {
                ids
            };
            if !force {
                refuse_needed(&mods, &ids, "uninstall")?;
            }
            for id in &ids {
                mods.uninstall(id)?;
                out!("uninstalled {id}");
            }
            hint_apply(&game, &mods)
        }
        ModsCommand::Enable {
            game_dir,
            ids,
            with_deps,
        } => enable(&game_dir, &ids, with_deps),
        ModsCommand::Disable {
            game_dir,
            ids,
            force,
        } => {
            let (game, mut mods) = open(&game_dir)?;
            if !force {
                refuse_needed(&mods, &ids, "disable")?;
            }
            for id in &ids {
                mods.set_enabled(id, false)?;
                out!("disabled {id}");
            }
            hint_apply(&game, &mods)
        }
        ModsCommand::Move {
            game_dir,
            id,
            position,
        } => {
            let (game, mut mods) = open(&game_dir)?;
            if position == 0 {
                bail!("positions start at 1");
            }
            for warning in mods.move_mod(&id, position - 1)? {
                out!("warning: {warning}");
            }
            list(&mods);
            hint_apply(&game, &mods)
        }
        ModsCommand::Sort { game_dir, dry_run } => {
            let (game, mut mods) = open(&game_dir)?;
            let report = mods.sort(dry_run)?;
            for rule in &report.dropped {
                out!("cannot keep: {rule} (it would close a circle of rules)");
            }
            if !report.changed {
                out!("the load order already follows the rules");
                return Ok(());
            }
            if dry_run {
                out!("the sorted order would be:");
                for (i, id) in report.order.iter().enumerate() {
                    out!("{:>3}  {id}", i + 1);
                }
                return Ok(());
            }
            list(&mods);
            hint_apply(&game, &mods)
        }
        ModsCommand::Conflicts { game_dir, all } => {
            let (game, mods) = open(&game_dir)?;
            let conflicts = mods.conflicts(&game, !all)?;
            let clashes = mods.clashes(!all);
            if conflicts.is_empty() {
                out!("no resource is changed by more than one mod");
            }
            for clash in &clashes {
                let kind = if clash.is_fatal() {
                    "cannot be applied together"
                } else {
                    "note"
                };
                out!("{kind}: {}", clash.describe());
            }
            for conflict in &conflicts {
                out!(
                    "{}  {}  -> {} wins{}",
                    conflict.key,
                    conflict.mods.join(", "),
                    conflict.winner(),
                    if conflict.same_data {
                        " (same data)"
                    } else if conflict.patch {
                        " (a patch on top of the others)"
                    } else if !conflict.elements.is_empty() {
                        " (the mods merge; set by both:)"
                    } else {
                        ""
                    }
                );
                for element in &conflict.elements {
                    out!("    {element}");
                }
            }
            Ok(())
        }
        ModsCommand::Check { game_dir } => {
            let (game, mods) = open(&game_dir)?;
            let issues = mods.check(&game)?;
            let clashes: Vec<_> = mods
                .clashes(true)
                .into_iter()
                .filter(|c| c.is_fatal())
                .collect();
            if issues.is_empty() && clashes.is_empty() {
                out!("every enabled mod fits the game");
            }
            for issue in &issues {
                out!("{}", issue.describe());
            }
            for clash in &clashes {
                out!("cannot be applied together: {}", clash.describe());
            }
            Ok(())
        }
        ModsCommand::Status { game_dir } => {
            let (game, mods) = open(&game_dir)?;
            status(&game, &mods)
        }
        ModsCommand::Apply {
            game_dir,
            force,
            ignore_compatibility,
        } => {
            let (game, mut mods) = open(&game_dir)?;
            let report = mods.apply(
                &game,
                ModApplyOptions {
                    force,
                    ignore_compatibility,
                },
            )?;
            print_report(&report);
            for path in &mods.native_report().removed {
                out!("removed   {path}");
            }
            Ok(())
        }
        ModsCommand::Unapply { game_dir, force } => {
            let (game, mut mods) = open(&game_dir)?;
            let report = mods.unapply(&game, force)?;
            print_report(&report);
            for path in &mods.native_report().removed {
                out!("removed   {path}");
            }
            Ok(())
        }
        ModsCommand::Native {
            game_dir,
            id,
            allow,
            data_only,
            forget,
        } => native_command(&game_dir, id.as_deref(), allow, data_only, forget),
        ModsCommand::Export {
            game_dir,
            id,
            output,
        } => {
            let (_, mods) = open(&game_dir)?;
            let size = mods.export(&id, &output)?;
            out!("wrote {} ({size} bytes)", output.display());
            Ok(())
        }
        ModsCommand::ImportOld { game_dir } => {
            let (game, mut mods) = open(&game_dir)?;
            let import = mods.import_old_toolkit(&game)?;
            for path in &import.adopted {
                out!("adopted the old toolkit's backup of {path} as the original");
            }
            for (name, result) in &import.installed {
                match result {
                    Ok(report) => print_install(report),
                    Err(error) => out!("cannot install {name}: {error}"),
                }
            }
            out!(
                "the game still has the old toolkit's files; run `doctor-toolkit mods apply --force {}` \
                 to rewrite them from the originals with the installed mods",
                quote(game.root())
            );
            Ok(())
        }
    }
}

fn open(game_dir: &Path) -> Result<(GameInstance, ModManager)> {
    let game = GameInstance::open(game_dir)?;
    let mods = ModManager::open(&game)?;
    warn_interrupted(&game, &mods);
    Ok((game, mods))
}

/// Tells when the last write into the game did not finish.
fn warn_interrupted(game: &GameInstance, mods: &ModManager) {
    let streams = mods.interrupted(game).unwrap_or_default();
    if streams.is_empty() {
        return;
    }
    eprintln!(
        "warning: the last write of the mods into the game did not finish ({}); the game files \
         may not match each other. Run `doctor-toolkit mods unapply {}` to put the originals \
         back from the backups, or `mods apply` to write the mods anew",
        streams.join(", "),
        quote(game.root())
    );
}

fn open_workspace(game: &GameInstance, dir: Option<&Path>) -> Result<Workspace> {
    let dir = dir.map_or_else(|| Workspace::default_dir(game.root()), Path::to_path_buf);
    if !Workspace::exists(&dir) {
        bail!("no workspace in {}", dir.display());
    }
    Ok(Workspace::open(&dir)?)
}

/// `None` for all changes, otherwise the keys matching the arguments.
fn match_keys(ws: &Workspace, args: &[String]) -> Result<Option<Vec<ResourceKey>>> {
    if args.is_empty() {
        return Ok(None);
    }
    let mut keys = Vec::new();
    for arg in args {
        let found: Vec<_> = ws
            .changes()
            .filter(|c| {
                arg.parse::<ResourceKey>().is_ok_and(|k| k == c.key)
                    || c.key.to_string().contains(arg.as_str())
            })
            .map(|c| c.key.clone())
            .collect();
        if found.is_empty() {
            bail!("no change matches `{arg}`");
        }
        keys.extend(found);
    }
    keys.sort();
    keys.dedup();
    Ok(Some(keys))
}

fn quote(path: &Path) -> String {
    let text = path.display().to_string();
    if text.contains(' ') {
        format!("\"{text}\"")
    } else {
        text
    }
}

fn enable(game_dir: &Path, ids: &[String], with_deps: bool) -> Result<()> {
    let (game, mut mods) = open(game_dir)?;
    for id in ids {
        if with_deps {
            for dep in mods.enable_with_dependencies(id)? {
                out!("enabled {dep} (needed by {id})");
            }
        } else {
            mods.set_enabled(id, true)?;
            let off = mods.disabled_dependencies(id);
            if !off.is_empty() {
                out!(
                    "note: {id} needs {}, which {} switched off; `mods enable --with-deps` \
                     enables them too",
                    off.join(", "),
                    if off.len() == 1 { "is" } else { "are" }
                );
            }
        }
        out!("enabled {id}");
    }
    hint_apply(&game, &mods)
}

/// Refuses to switch off or remove mods that other enabled mods need.
fn refuse_needed(mods: &ModManager, ids: &[String], verb: &str) -> Result<()> {
    for id in ids {
        let needed_by: Vec<String> = mods
            .dependents(id)
            .into_iter()
            .filter(|d| !ids.contains(d))
            .collect();
        if !needed_by.is_empty() {
            bail!(
                "{id} is needed by the enabled mod(s) {}; {verb} them too, or use --force",
                needed_by.join(", ")
            );
        }
    }
    Ok(())
}

/// After a mod is installed: what its code is and how to answer.
fn ask_about_code(game: &GameInstance, mods: &ModManager, id: &str) -> Result<()> {
    let Some(question) = native::question(mods, mods.native_host(), id)? else {
        return Ok(());
    };
    if native::consent(mods, id)? == native::Consent::Code {
        out!("  the executable code of {id} is the one you agreed to before");
        return Ok(());
    }
    for line in question.lines() {
        out!("  {line}");
    }
    out!(
        "  to agree: `doctor-toolkit mods native {} {id} --allow`{}",
        quote(game.root()),
        if question.required {
            "; to refuse, disable or uninstall the mod"
        } else {
            "; to use the mod without its code: `--data-only`"
        }
    );
    Ok(())
}

fn native_command(
    game_dir: &Path,
    id: Option<&str>,
    allow: bool,
    data_only: bool,
    forget: bool,
) -> Result<()> {
    let (game, mut mods) = open(game_dir)?;
    let Some(id) = id else {
        let states = native::states(&mods, mods.native_host())?;
        let with_code: Vec<_> = mods
            .mods()
            .iter()
            .filter(|m| m.manifest.native.is_some())
            .collect();
        if with_code.is_empty() {
            out!("no installed mod has executable code");
        }
        for m in with_code {
            let state = states
                .iter()
                .find(|(i, _)| *i == m.id)
                .map_or_else(|| "the mod is disabled".to_owned(), |(_, s)| s.to_string());
            out!(
                "{}  {} {}: {state}",
                m.id,
                m.manifest.name,
                m.manifest.version
            );
            for module in m.manifest.native.iter().flat_map(|n| &n.modules) {
                out!(
                    "    {}  {} byte(s)  sha256 {}",
                    module.path(),
                    module.payload.size,
                    module.payload.sha256
                );
            }
        }
        for (path, owner) in native::written(&mods)? {
            out!("in the game: {path}  ({owner})");
        }
        let host = mods.native_host();
        if let Err(why) = &host.edition {
            out!("the game: {why}");
        }
        match &host.loader {
            Some(loader) => out!("ASI loader: {loader}"),
            None => out!(
                "ASI loader: none in the game folder{}",
                if host.loader_binary.is_some() {
                    "; the toolkit's is installed with the first module"
                } else {
                    ""
                }
            ),
        }
        return Ok(());
    };
    let Some(question) = native::question(&mods, mods.native_host(), id)? else {
        bail!("mod `{id}` has no executable code");
    };
    if !(allow || data_only || forget) {
        for line in question.lines() {
            out!("{line}");
        }
        out!("your answer: {:?}", native::consent(&mods, id)?);
        return Ok(());
    }
    if data_only && question.required {
        bail!(
            "mod `{id}` does not work without its code: agree with --allow, or disable or \
             uninstall the mod"
        );
    }
    let answer = if allow {
        Some(Answer::Code)
    } else if data_only {
        Some(Answer::Data)
    } else {
        None
    };
    native::set_consent(&mods, id, answer)?;
    // Code that is not agreed to any more leaves the game at once.
    for path in native::prune(&mods)?.removed {
        out!("removed   {path}");
    }
    mods.set_native_host(mods.native_host().clone());
    match answer {
        Some(Answer::Code) => {
            for file in question.modules.iter().chain(&question.toolkit) {
                out!("agreed: {}  sha256 {}", file.path, file.sha256);
            }
            if let Some(obstacle) = &question.obstacle {
                out!("note: {}", obstacle.describe());
            }
            let installed = native::states(&mods, mods.native_host())?
                .iter()
                .any(|(i, s)| i == id && *s == NativeState::Installed);
            if !installed {
                out!(
                    "run `doctor-toolkit mods apply {}` to install it",
                    quote(game.root())
                );
            }
        }
        Some(Answer::Data) => out!("{id}: only its data is used; its code stays out of the game"),
        None => out!("{id}: the answer is forgotten; its code waits for your consent"),
    }
    Ok(())
}

fn hint_apply(game: &GameInstance, mods: &ModManager) -> Result<()> {
    let status = mods.status(game)?;
    if !status.streams.iter().all(|s| s.up_to_date) {
        out!(
            "the game does not have this yet; run `doctor-toolkit mods apply {}`",
            quote(game.root())
        );
    }
    Ok(())
}

fn info(file: &Path, resources: bool) -> Result<()> {
    let head = {
        use std::io::Read;
        let mut head = [0u8; 4];
        let mut f = std::fs::File::open(file).with_context(|| format!("{}", file.display()))?;
        let n = f.read(&mut head)?;
        head[..n].to_vec()
    };
    if vpat::is_vpat(&head) {
        let patch = VpatPatch::open(file)?;
        out!("old toolkit patch (.vpat, format {})", patch.format_version);
        out!("name     {}", patch.name);
        out!("author   {}", patch.author);
        out!("version  {}", patch.version);
        out!("game     {}", patch.game);
        out!("patch id {:08x}", patch.patch_id);
        out!("flags    {:#x}", patch.flags);
        for f in &patch.files {
            out!(
                "  {}  ({} resource(s), stream version {}, flags {:#x})",
                f.path,
                f.resources.len(),
                f.stream_version,
                f.flags
            );
            if resources {
                for r in &f.resources {
                    let key = r.key(&f.stream);
                    let by_type = if r.by_type { ", by type" } else { "" };
                    out!(
                        "    {key}  {} byte(s), {}{by_type}",
                        r.size,
                        r.operation.name()
                    );
                }
            }
        }
        return Ok(());
    }
    let mut archive = ModArchive::open(file)?;
    archive.verify()?;
    let m = archive.manifest().clone();
    out!("mod {} {} ({})", m.id, m.version, m.name);
    if !m.author.is_empty() {
        out!("author   {}", m.author);
    }
    if let Some(url) = &m.url {
        out!("url      {url}");
    }
    out!("game     {}", m.game.profile);
    if let Some(from) = &m.converted_from {
        out!("converted from {from}");
    }
    for line in m.description.lines() {
        out!("  {line}");
    }
    out!("format   {}", m.format_version);
    relations(&m);
    let operations: std::collections::BTreeMap<_, _> =
        archive.delta_operations()?.into_iter().collect();
    out!(
        "{} resource(s){} in {} stream(s), {} byte(s) of data; every payload checked",
        m.resources.len(),
        adds_note(m.added().count(), m.streams.len()),
        m.streams().len(),
        m.payload_bytes()
    );
    if !operations.is_empty() {
        out!(
            "{} resource(s) are deltas, {} operation(s): they are merged with what other \
             mods do to the same resources",
            operations.len(),
            operations.values().sum::<usize>()
        );
    }
    if let Some(code) = &m.native {
        out!(
            "EXECUTABLE CODE for the edition {} of the game{}{}: installed only after the \
             player agrees",
            code.edition,
            match code.platform_abi {
                Some(abi) => format!(", platform ABI {abi}"),
                None => ", plain ASI plug-in(s)".to_owned(),
            },
            if code.required {
                ", required (the mod does not work without it)"
            } else {
                ""
            }
        );
        for module in &code.modules {
            out!(
                "  code {}  {} byte(s)  sha256 {}",
                module.path(),
                module.payload.size,
                module.payload.sha256
            );
        }
    }
    for file in &m.files {
        out!(
            "  file {}  {} byte(s){}",
            file.key,
            file.payload.size,
            if file.label.is_empty() {
                String::new()
            } else {
                format!("  {}", file.label)
            }
        );
    }
    for stream in &m.streams {
        out!(
            "  new stream {}  hash {:08x}, {} {}, in {}",
            stream.name,
            stream.hash,
            if stream.with_parent && stream.auto_load {
                "loaded with every load of"
            } else if stream.with_parent {
                "loaded with"
            } else {
                "requested, after"
            },
            stream.parent,
            stream.tocs.join(", ")
        );
    }
    if resources {
        for r in &m.resources {
            if let Some(delta) = &r.delta {
                out!(
                    "  {}  {} delta (format {}), {} operation(s), {} byte(s)  {}",
                    r.key,
                    delta.kind,
                    delta.format,
                    operations.get(&r.key).copied().unwrap_or(0),
                    r.payload.size,
                    r.label
                );
                continue;
            }
            match &r.added {
                Some(added) => out!(
                    "  {}  added, {} byte(s){}  {}",
                    r.key,
                    r.payload.size,
                    added
                        .before
                        .as_ref()
                        .map(|b| format!(", before {b}"))
                        .unwrap_or_default(),
                    r.label
                ),
                None => out!(
                    "  {}  {} -> {} byte(s)  {}",
                    r.key,
                    r.original.size,
                    r.payload.size,
                    r.label
                ),
            }
        }
    }
    for warning in archive.warnings().iter().chain(&m.warnings()) {
        out!("warning: {warning}");
    }
    Ok(())
}

/// The fields of format version 3 of a manifest.
fn relations(m: &mods::ModManifest) {
    if !m.game.editions.is_empty() {
        out!("editions {}", m.game.editions.join(", "));
    }
    if let Some(required) = &m.requires_manager {
        out!("needs mod manager {required}");
    }
    for (id, condition) in &m.depends {
        out!("depends on   {id} {condition}");
    }
    for (id, condition) in &m.recommends {
        out!("recommends   {id} {condition}");
    }
    for (id, inc) in &m.incompatible {
        let reason = if inc.reason.is_empty() {
            String::new()
        } else {
            format!(": {}", inc.reason)
        };
        out!("incompatible with {id} {}{reason}", inc.versions);
    }
    if !m.load_after.is_empty() {
        out!("load after   {}", m.load_after.join(", "));
    }
    if !m.load_before.is_empty() {
        out!("load before  {}", m.load_before.join(", "));
    }
}

fn list(mods: &ModManager) {
    if mods.mods().is_empty() {
        out!("no mods installed");
    }
    for (i, m) in mods.mods().iter().enumerate() {
        let state = match (&m.problem, m.enabled) {
            (Some(_), _) => "damaged",
            (None, true) => "enabled",
            (None, false) => "disabled",
        };
        out!(
            "{:>3}  {:<8} {}  {} {}  ({} resource(s){}){}",
            i + 1,
            state,
            m.id,
            m.manifest.name,
            m.manifest.version,
            m.manifest.resources.len(),
            adds_note(m.manifest.added().count(), m.manifest.streams.len()),
            m.problem
                .as_ref()
                .map(|p| format!(": {p}"))
                .unwrap_or_default()
        );
    }
}

/// `, 2 added, 1 new stream(s)` when a mod adds anything.
fn adds_note(added: usize, new_streams: usize) -> String {
    let mut note = String::new();
    if added > 0 {
        note += &format!(", {added} added");
    }
    if new_streams > 0 {
        note += &format!(", {new_streams} new stream(s)");
    }
    note
}

fn print_install(report: &InstallReport) {
    let adds = adds_note(report.added, report.new_streams);
    match &report.replaced_version {
        Some(old) => out!(
            "updated {} ({}) from version {old}: {} resource(s){adds}",
            report.id,
            report.name,
            report.resources
        ),
        None => out!(
            "installed {} ({}): {} resource(s){adds}",
            report.id,
            report.name,
            report.resources
        ),
    }
    for warning in &report.warnings {
        out!("  warning: {warning}");
    }
    for issue in &report.issues {
        if issue.problem.is_notice() {
            out!("  note: {}", issue.describe());
        } else {
            out!("  does not fit: {}", issue.describe());
        }
    }
}

fn status(game: &GameInstance, mods: &ModManager) -> Result<()> {
    let status = mods.status(game)?;
    let enabled = mods.mods().iter().filter(|m| m.enabled).count();
    out!(
        "{} mod(s) installed, {enabled} enabled; {} resource(s) to replace",
        mods.mods().len(),
        status.changes.len()
    );
    for stream in &status.streams {
        let state = match stream.state {
            doctor_toolkit::workspace::StreamState::Original => "original",
            doctor_toolkit::workspace::StreamState::Applied => "written by the mod manager",
            doctor_toolkit::workspace::StreamState::ChangedExternally => {
                "changed by another program"
            }
            doctor_toolkit::workspace::StreamState::Missing => "missing from the game",
        };
        let todo = if stream.up_to_date {
            "up to date"
        } else {
            "needs apply"
        };
        out!("  {:<40} {state}, {todo}", stream.stream);
    }
    if status.streams.iter().all(|s| s.up_to_date) {
        out!("the game is up to date");
    } else {
        out!(
            "run `doctor-toolkit mods apply {}` to update the game",
            quote(game.root())
        );
    }
    Ok(())
}

fn print_report(report: &ApplyReport) {
    if report.streams.is_empty() && report.written.is_empty() {
        out!("nothing to do: the game is up to date");
    }
    for (stream, action) in &report.streams {
        match action {
            StreamAction::Rebuilt(n) => out!("rebuilt  {stream} ({n} resource(s) replaced)"),
            StreamAction::Restored => out!("restored {stream}"),
        }
    }
    for path in &report.backed_up {
        out!("backed up {path}");
    }
    for path in &report.written {
        out!("wrote     {path}");
    }
    for warning in &report.warnings {
        out!("warning: {warning}");
    }
}

pub fn run_integrity(args: IntegrityArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let total = game.streams().len() + game.tocs().len();
    let shown = AtomicUsize::new(0);
    let progress = |done: usize, total: usize| {
        let step = total / 20 + 1;
        if done.is_multiple_of(step) && shown.fetch_max(done, Ordering::Relaxed) < done {
            eprint!("\r{done}/{total} files");
        }
    };
    if args.adopt_old_backups {
        let adopted = integrity::adopt_old_backups(&game)?;
        for path in &adopted {
            out!("adopted {path}");
        }
        out!("{} backup(s) adopted", adopted.len());
        return Ok(());
    }
    if args.save_reference {
        let count = integrity::save_reference(&game, progress)?;
        eprintln!();
        out!(
            "saved the hashes of {count} original file(s) to {}",
            game.root().join(integrity::REFERENCE_FILE).display()
        );
        return Ok(());
    }
    let report = integrity::check(&game, IntegrityOptions { full: args.full }, progress)?;
    if total > 0 {
        eprintln!();
    }
    for file in report.changed() {
        out!("{:<48} {}", file.rel_path, file.condition.describe());
    }
    out!(
        "{} file(s): {} original, {} changed, {} not verified",
        report.files.len(),
        report.count(Condition::Original),
        report.changed().count(),
        report.count(Condition::Unverified)
    );
    if !report.reference && report.count(Condition::Unverified) > 0 {
        out!(
            "files without a backup cannot be verified; on an unmodified game, \
             `integrity --save-reference` records their hashes for later checks"
        );
    } else if report.reference && !args.full && report.count(Condition::Unverified) > 0 {
        out!(
            "`--full` hashes the files without a backup and compares them with the reference list"
        );
    }
    if let Some(old) = &report.old_toolkit {
        out!(
            "the old toolkit is present ({}): {} patch(es) installed, {} backup(s)",
            old.dir.display(),
            old.installed_patches.len(),
            old.backups.len()
        );
        for name in &old.installed_patches {
            out!("  {name}");
        }
        if report.count(Condition::OldToolkit) > 0 {
            out!(
                "`doctor-toolkit mods import-old {}` takes its patches over as mods",
                quote(game.root())
            );
        }
    }
    for path in &report.temp_files {
        out!("leftover of an interrupted write (safe to delete): {path}");
    }
    Ok(())
}
