//! `mods` subcommands (building, installing and applying mods) and `integrity`.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::mods::integrity::{self, Condition, IntegrityOptions};
use doctor_toolkit::mods::manager::{InstallReport, ModApplyOptions};
use doctor_toolkit::mods::vpat::{self, VpatPatch};
use doctor_toolkit::mods::{self, ModArchive, ModInfo, ModManager, Preview};
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
}

impl MetaArgs {
    fn info(&self) -> ModInfo {
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
        }
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
    },
    /// Enable mods
    Enable {
        game_dir: PathBuf,
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Disable mods
    Disable {
        game_dir: PathBuf,
        #[arg(required = true)]
        ids: Vec<String>,
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
            let info = meta.info();
            let report =
                mods::build_from_workspace(&ws, &info, keys.as_deref(), preview.as_ref(), &output)?;
            out!(
                "built {} ({} {}): {} resource(s){}, {} byte(s) of data, file {} byte(s)",
                output.display(),
                info.id,
                info.version,
                report.resources,
                adds_note(report.added, report.new_streams),
                report.payload_bytes,
                report.file_size
            );
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
            let report = mods.install_workspace(&game, &ws, &meta.info(), None)?;
            print_install(&report);
            Ok(())
        }
        ModsCommand::Uninstall { game_dir, ids, all } => {
            let (game, mut mods) = open(&game_dir)?;
            let ids = if all {
                mods.mods().iter().map(|m| m.id.clone()).collect()
            } else {
                ids
            };
            for id in &ids {
                mods.uninstall(id)?;
                out!("uninstalled {id}");
            }
            hint_apply(&game, &mods)
        }
        ModsCommand::Enable { game_dir, ids } => set_enabled(&game_dir, &ids, true),
        ModsCommand::Disable { game_dir, ids } => set_enabled(&game_dir, &ids, false),
        ModsCommand::Move {
            game_dir,
            id,
            position,
        } => {
            let (game, mut mods) = open(&game_dir)?;
            if position == 0 {
                bail!("positions start at 1");
            }
            mods.move_mod(&id, position - 1)?;
            list(&mods);
            hint_apply(&game, &mods)
        }
        ModsCommand::Conflicts { game_dir, all } => {
            let (_, mods) = open(&game_dir)?;
            let conflicts = mods.conflicts(!all);
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
                    } else {
                        ""
                    }
                );
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
            Ok(())
        }
        ModsCommand::Unapply { game_dir, force } => {
            let (game, mut mods) = open(&game_dir)?;
            let report = mods.unapply(&game, force)?;
            print_report(&report);
            Ok(())
        }
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
    Ok((game, mods))
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

fn set_enabled(game_dir: &Path, ids: &[String], enabled: bool) -> Result<()> {
    let (game, mut mods) = open(game_dir)?;
    for id in ids {
        mods.set_enabled(id, enabled)?;
        out!("{} {id}", if enabled { "enabled" } else { "disabled" });
    }
    hint_apply(&game, &mods)
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
    out!(
        "{} resource(s){} in {} stream(s), {} byte(s) of data; every payload checked",
        m.resources.len(),
        adds_note(m.added().count(), m.streams.len()),
        m.streams().len(),
        m.payload_bytes()
    );
    for stream in &m.streams {
        out!(
            "  new stream {}  hash {:08x}, {} {}, in {}",
            stream.name,
            stream.hash,
            if stream.with_parent {
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
    for warning in archive.warnings() {
        out!("warning: {warning}");
    }
    Ok(())
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
        out!("  does not fit: {}", issue.describe());
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
    if report.streams.is_empty() {
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
