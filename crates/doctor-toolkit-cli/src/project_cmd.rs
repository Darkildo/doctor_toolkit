//! `project` subcommands: a mod as a folder of source files (`docs/project.md`).

use std::io::Write as _;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::mods::ModManager;
use doctor_toolkit::mods::manager::{ModApplyOptions, Payloads};
use doctor_toolkit::navigation::GameIndex;
use doctor_toolkit::navigation::index::default_cache_file;
use doctor_toolkit::project::{
    self, BaseDifference, BuildOptions, EntryState, NewProject, Project, ProjectConfig,
    ProjectStatus,
};
use doctor_toolkit::workspace::{ApplyOptions, Owner, Workspace, WorkspaceLock};

macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

/// Which project and game.
#[derive(Args)]
pub struct Target {
    /// Project folder [default: the current folder]
    #[arg(default_value = ".")]
    project: PathBuf,
    /// Game folder [default: the one in project.toml]
    #[arg(long)]
    game: Option<PathBuf>,
    /// Workspace directory [default: from project.toml, else <GAME>/_doctor_toolkit/workspace]
    #[arg(long, value_name = "DIR")]
    workspace_dir: Option<PathBuf>,
}

impl Target {
    fn open(&self) -> Result<(Project, GameInstance, Workspace)> {
        let project = Project::open(&self.project)
            .with_context(|| format!("{} is not a project", self.project.display()))?;
        let game = project.game(self.game.as_deref())?;
        let ws = match &self.workspace_dir {
            Some(dir) => Workspace::for_game(&game, Some(dir))?,
            None => project.workspace(&game)?,
        };
        Ok((project, game, ws))
    }
}

#[derive(Subcommand)]
pub enum ProjectCommand {
    /// Make a project: empty, from streams of the game, or from the changes of the workspace
    ///
    /// Without `--stream`, `--from-workspace` and `--move-workspace`: an empty
    /// project with a workspace of its own (`<PROJECT>/workspace`), filled by
    /// editing — the document of the editor.
    ///
    /// Every resource of the chosen streams becomes a file in a common format
    /// (PNG, JSON, Lua, glTF, ...) under `src/<stream>.str/<type>/<name>`;
    /// resources equal in several streams go to `shared/` once. The changes
    /// of a workspace come with the resources it adds and the streams it
    /// creates.
    New {
        game_dir: PathBuf,
        /// The project folder to make (new or empty)
        project: PathBuf,
        /// Name of the project and its mod [default: the folder name]
        #[arg(long)]
        name: Option<String>,
        /// Mod id [default: made from the name]
        #[arg(long)]
        id: Option<String>,
        #[arg(long, default_value = "")]
        author: String,
        /// Stream to take, e.g. `gf2/frontend` (repeatable)
        #[arg(long = "stream")]
        streams: Vec<String>,
        /// Only resources of this type: a tag such as `tg4h` (repeatable)
        #[arg(long = "type")]
        types: Vec<String>,
        /// Take what the workspace changes, as it has it; the project builds into that workspace
        #[arg(long, conflicts_with = "streams")]
        from_workspace: bool,
        /// Move the workspace into the project as its own (edits and history;
        /// the game's workspace is gone) and write its sources. Refused while
        /// its changes are in the game files: `workspace revert --game` first
        #[arg(long, conflicts_with_all = ["streams", "from_workspace", "types"])]
        move_workspace: bool,
        /// With --move-workspace: do not write sources (`project sources` writes them later)
        #[arg(long, requires = "move_workspace")]
        no_sources: bool,
        /// Workspace directory [default: <GAME_DIR>/_doctor_toolkit/workspace]
        #[arg(long, value_name = "DIR")]
        workspace_dir: Option<PathBuf>,
        /// Textures as DDS instead of PNG
        #[arg(long)]
        dds: bool,
        /// Model format: glb, fbx, obj, usd, usda, usdc or usdz
        #[arg(long, default_value = "glb")]
        model_format: String,
        /// Keep resources equal in several streams in each stream's folder
        #[arg(long)]
        no_shared: bool,
        /// Name files by id only (do not open the game index)
        #[arg(long)]
        no_index: bool,
    },
    /// Show what changed since the last build
    Status {
        #[command(flatten)]
        target: Target,
        /// List every entry, also unchanged ones
        #[arg(long)]
        all: bool,
    },
    /// Build the changed files into the workspace (one undoable operation)
    Build {
        #[command(flatten)]
        target: Target,
        /// Only entries whose path starts with this (repeatable)
        #[arg(long)]
        only: Vec<String>,
        /// Build every entry, changed or not (textures and models are encoded again)
        #[arg(long)]
        force: bool,
        /// Only list what would be built
        #[arg(long)]
        dry_run: bool,
        /// Also write the workspace into the game
        #[arg(long)]
        apply: bool,
    },
    /// Write sources for everything the project's own workspace holds
    ///
    /// New entries under `src/` for the changes that have none yet (resources
    /// replaced and added, the description of the streams the workspace
    /// creates), the workspace's version of the entries that are out of sync
    /// (as `project pull`, models too), and nothing left of what the workspace
    /// no longer adds. Files changed and not built are left as they are.
    Sources {
        #[command(flatten)]
        target: Target,
        /// Name files by id only (do not open the game index); models are
        /// then written without the materials and textures of the game
        #[arg(long)]
        no_index: bool,
    },
    /// Write the workspace's version of out-of-sync entries into their files
    Pull {
        #[command(flatten)]
        target: Target,
        /// Only entries whose path starts with this
        only: Vec<String>,
    },
    /// Build a mod (format 2) of the project's changes in the workspace
    Mod {
        #[command(flatten)]
        target: Target,
        /// The mod file to write
        #[arg(short, long)]
        output: PathBuf,
        /// A PNG or JPEG picture shown in the mod manager
        #[arg(long)]
        preview: Option<PathBuf>,
        /// Build first
        #[arg(long)]
        build: bool,
    },
    /// Install the project's changes into the mod manager as a mod, last in the load order
    Install {
        #[command(flatten)]
        target: Target,
        /// Build first
        #[arg(long)]
        build: bool,
    },
    /// Rebase the edits of the project made on other data than its base gives now
    ///
    /// The edits of a project are made on the game with the mods of its
    /// base. When a base mod is updated (`project base` says "changed since
    /// the project was made on it"), an edit still holds what the mod gave
    /// then, and the mod of the project would take the update back. The
    /// command finds such edits and rebases them as one undoable operation:
    /// SimGroups, string tables, the parts database and the world grid by
    /// their operations (what no longer applies is listed by object or
    /// string); any other resource needs `--keep KEY` (yours stays) or
    /// `--take-base KEY` (the base's). Bring the mods of the game to the
    /// base first (`project base --apply`), and record the base again
    /// afterwards (`project base --record`).
    Rebase {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        rebase: crate::workspace_cmd::RebaseArgs,
    },
    /// Show the base of the project — the mods it is made on — and how the game stands against it
    ///
    /// The base is `[base]` of project.toml: the mods of the mod manager the
    /// project is made on, each with the versions the project works with,
    /// the version it was made on and the hash of that version's manifest.
    /// The mod of the project depends on them. Without an option the
    /// command prints the base and what differs in the game.
    Base {
        /// Project folder [default: the current folder]
        #[arg(default_value = ".")]
        project: PathBuf,
        /// Game folder [default: the one the project names]
        #[arg(long)]
        game: Option<PathBuf>,
        /// Take the base from the mods written into the game now (and write project.toml)
        #[arg(long)]
        record: bool,
        /// Fail when the mods of the game differ from the base
        #[arg(long)]
        check: bool,
        /// Bring the mods to the base: save the arrangement they have as a
        /// set, enable the mods of the base in its order (an exclusive base
        /// switches the others off) and write the mods into the game
        #[arg(long)]
        apply: bool,
        /// With --apply: overwrite streams changed by another program
        #[arg(long, requires = "apply")]
        force: bool,
        /// With --apply: write although the game's originals differ from what the mods expect
        #[arg(long, requires = "apply")]
        ignore_compatibility: bool,
    },
}

impl ProjectCommand {
    /// Whether the command can change the files of the game.
    pub fn may_write_game(&self) -> bool {
        matches!(
            self,
            Self::Build { apply: true, .. } | Self::Base { apply: true, .. }
        )
    }
}

pub fn run(command: ProjectCommand) -> Result<()> {
    match command {
        ProjectCommand::New {
            game_dir,
            project,
            name,
            id,
            author,
            streams,
            types,
            from_workspace,
            move_workspace,
            no_sources,
            workspace_dir,
            dds,
            model_format,
            no_shared,
            no_index,
        } => {
            let game = GameInstance::open(&game_dir)?;
            let new = NewProject {
                name,
                id,
                author,
                streams,
                types,
                from_workspace,
                dds,
                model_format: Some(model_format),
                no_shared,
            };
            if move_workspace {
                return new_from_moved_workspace(
                    &game,
                    &project,
                    new,
                    workspace_dir,
                    no_sources,
                    no_index,
                );
            }
            let empty = new.streams.is_empty() && !from_workspace;
            if empty && workspace_dir.is_some() {
                bail!(
                    "an empty project has its own workspace in {}; --workspace-dir goes with \
                     --stream, --from-workspace or --move-workspace",
                    project.join(project::document::WORKSPACE_DIR).display()
                );
            }
            let ws = Workspace::for_game(&game, workspace_dir.as_deref())?;
            // An empty project names no file: the index is not needed.
            let index = if no_index || empty {
                None
            } else {
                Some(open_index(&game))
            };
            let started = Instant::now();
            let (made, report) =
                project::create_from(&game, &ws, index.as_ref(), &project, &new, workspace_dir)?;
            if empty {
                out!(
                    "{}: an empty project {} with its own workspace in {}",
                    made.root().display(),
                    made.config.project.id,
                    made.workspace_dir().unwrap_or_default().display()
                );
                out!(
                    "edit it in the editor (File → Open project) or with --workspace-dir {}; \
                     `doctor-toolkit project sources {}` writes its sources",
                    quote(&made.workspace_dir().unwrap_or_default()),
                    quote(made.root())
                );
                return Ok(());
            }
            out!(
                "{}: {} resource(s) as {} file(s), {} entr{} ({} in shared/), {:.1} MiB in {:.1} s",
                made.root().display(),
                report.resources,
                report.files,
                report.entries,
                if report.entries == 1 { "y" } else { "ies" },
                report.shared,
                report.bytes as f64 / (1 << 20) as f64,
                started.elapsed().as_secs_f64()
            );
            for note in &report.notes {
                out!("  note: {note}");
            }
            Ok(())
        }
        ProjectCommand::Status { target, all } => {
            let (project, game, ws) = target.open()?;
            let status = project.status(&ws)?;
            print_status(&project, &status, all);
            for warning in project.game_warnings(&game) {
                out!("warning: {warning}");
            }
            if let Some(warning) = project::stale_warning(&project.stale(&game, &ws)?) {
                out!("warning: {warning}");
            }
            Ok(())
        }
        ProjectCommand::Rebase {
            target,
            rebase: args,
        } => {
            let (project, game, mut ws) = target.open()?;
            if !args.check && ws.is_saved() {
                crate::workspace_cmd::lock(ws.dir())?;
                ws.reload()?;
            }
            crate::workspace_cmd::rebase(&game, &mut ws, Some(&project), &args)?;
            if !args.check
                && project
                    .config
                    .check_base(&game)?
                    .iter()
                    .any(|d| matches!(d, BaseDifference::Changed { .. }))
            {
                out!(
                    "`doctor-toolkit project base {} --record` records the base as the game \
                     has it now",
                    quote(project.root())
                );
            }
            Ok(())
        }
        ProjectCommand::Build {
            target,
            only,
            force,
            dry_run,
            apply,
        } => {
            let (project, game, mut ws) = target.open()?;
            if apply && ws.owner() == Owner::Project {
                // Said before the build, not after it.
                bail!(
                    "the project has its own workspace, which is not written into the game \
                     files by itself: build without --apply, then `doctor-toolkit project \
                     install {}` and `doctor-toolkit mods apply {}`",
                    quote(project.root()),
                    quote(game.root())
                );
            }
            if !dry_run {
                crate::workspace_cmd::lock(ws.dir())?;
                // Read again under the lock: another program may have written it meanwhile.
                ws.reload()?;
            }
            build(&project, &game, &mut ws, only, force, dry_run)?;
            if apply && !dry_run {
                let report = ws.apply(&game, ApplyOptions::default())?;
                out!("applied: {} file(s) written", report.written.len());
                for warning in &report.warnings {
                    out!("warning: {warning}");
                }
            }
            Ok(())
        }
        ProjectCommand::Sources { target, no_index } => {
            let (mut project, game, ws) = target.open()?;
            let index = (!no_index).then(|| open_index(&game));
            let started = Instant::now();
            let report = project.export_workspace(&game, &ws, index.as_ref())?;
            print_sources(&project, &report, started);
            Ok(())
        }
        ProjectCommand::Pull { target, only } => {
            let (mut project, game, ws) = target.open()?;
            let pulled = project.pull(&game, &ws, &only)?;
            if pulled.is_empty() {
                out!("no entry is out of sync with the workspace");
            }
            for id in pulled {
                out!("{id}: taken from the workspace");
            }
            Ok(())
        }
        ProjectCommand::Mod {
            target,
            output,
            preview,
            build: build_first,
        } => {
            let (project, game, mut ws) = target.open()?;
            if build_first {
                build(&project, &game, &mut ws, Vec::new(), false, false)?;
            }
            let preview = match preview {
                Some(path) => Some(doctor_toolkit::mods::Preview::from_image(
                    std::fs::read(&path).with_context(|| path.display().to_string())?,
                )?),
                None => None,
            };
            // As it is installed: with the mods of its base (or, without a
            // base, the mods written into the game it was made on) as
            // dependencies and their data as the originals.
            let report = project.build_mod_on(&game, &ws, &output, preview.as_ref())?;
            out!(
                "{}: mod {} with {} resource(s) ({} added), {:.1} MiB",
                output.display(),
                project.config.project.id,
                report.resources,
                report.added,
                report.file_size as f64 / (1 << 20) as f64
            );
            // The first note of a mod with stale changes is their warning.
            for (i, note) in report.notes.iter().enumerate() {
                match i == 0 && !report.stale.is_empty() {
                    true => out!("warning: {note}"),
                    false => out!("note: {note}"),
                }
            }
            Ok(())
        }
        ProjectCommand::Install {
            target,
            build: build_first,
        } => {
            let (project, game, mut ws) = target.open()?;
            if build_first {
                crate::workspace_cmd::lock(ws.dir())?;
                ws.reload()?;
                build(&project, &game, &mut ws, Vec::new(), false, false)?;
            }
            let mut mods = ModManager::open(&game)?;
            let report = project.install_into(&game, &mut mods, &ws, Payloads::Copied)?;
            match &report.replaced_version {
                Some(old) => out!(
                    "updated mod {} ({}) from version {old}: {} resource(s), last in the load order",
                    report.id,
                    report.name,
                    report.resources
                ),
                None => out!(
                    "installed mod {} ({}): {} resource(s), last in the load order",
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
            out!(
                "run `doctor-toolkit mods apply {}` to write the mods into the game",
                quote(game.root())
            );
            Ok(())
        }
        ProjectCommand::Base {
            project,
            game,
            record,
            check,
            apply,
            force,
            ignore_compatibility,
        } => {
            let root = project;
            let project = Project::open(&root)
                .with_context(|| format!("{} is not a project", root.display()))?;
            let game = project.game(game.as_deref())?;
            if record {
                let recorded = ProjectConfig::record_base(&root, &game)?;
                out!(
                    "recorded the base of {} from the game: {} mod(s)",
                    project.config.project.id,
                    recorded.base.mods.len()
                );
                for id in &recorded.not_written {
                    out!("  left out: {id} is enabled but not written into the game");
                }
            }
            let config = ProjectConfig::read(&root)?;
            if apply {
                let Some(base) = &config.base else {
                    bail!(
                        "the project names no base; `doctor-toolkit project base {} --record` \
                         takes it from the mods in the game",
                        quote(&root)
                    );
                };
                let mut mods = ModManager::open(&game)?;
                let options = ModApplyOptions {
                    force,
                    ignore_compatibility,
                };
                let brought =
                    base.bring(&game, &mut mods, &config.project.name, options, &mut |_| {})?;
                match (&brought.set, brought.saved) {
                    (Some(set), true) => out!(
                        "the arrangement before is saved as the set `{set}` of the mod manager \
                         (to go back: choose the set in the Mods window, then write the mods)"
                    ),
                    (Some(set), false) => {
                        out!("the arrangement before is the set `{set}` of the mod manager")
                    }
                    (None, _) => {}
                }
                out!(
                    "the mods are brought to the base: {} file(s) written",
                    brought.report.written.len()
                );
                for warning in &brought.report.warnings {
                    out!("  warning: {warning}");
                }
            }
            match &config.base {
                Some(base) => {
                    out!(
                        "base of {} ({}): {} mod(s){}",
                        config.project.name,
                        config.project.id,
                        base.mods.len(),
                        if base.exclusive {
                            ", exclusive: no other mod is in the game"
                        } else {
                            ", other mods may be enabled"
                        }
                    );
                    for m in &base.mods {
                        let made_on = match (m.built_on.is_empty(), m.sha256.get(..12)) {
                            (true, _) => String::new(),
                            (false, Some(sha)) => format!(", made on {} ({sha}…)", m.built_on),
                            (false, None) => format!(", made on {}", m.built_on),
                        };
                        out!("  {} {}{made_on}", m.id, m.version);
                    }
                }
                None if config.dependencies.mods.is_empty() => out!(
                    "{} names no base; --record takes it from the mods in the game",
                    config.project.id
                ),
                None => out!(
                    "{} names no base; it needs the mod(s) {} ([dependencies] mods)",
                    config.project.id,
                    config.dependencies.mods.join(", ")
                ),
            }
            let differences = config.check_base(&game)?;
            for difference in &differences {
                out!("differs: {}", difference.describe());
            }
            if config.base.is_some() && differences.is_empty() {
                out!("the game holds the base");
            }
            if differences.iter().any(BaseDifference::brought) && !apply {
                out!(
                    "`doctor-toolkit project base {} --apply` brings the mods to the base",
                    quote(&root)
                );
            }
            if check && !differences.is_empty() {
                bail!(
                    "the mods of the game differ from the base of the project in {} way(s)",
                    differences.len()
                );
            }
            Ok(())
        }
    }
}

/// The index of the game, with its progress on stderr when it takes long.
fn open_index(game: &GameInstance) -> GameIndex {
    let started = Instant::now();
    let (index, _) = GameIndex::open(
        game,
        default_cache_file(game.root()).as_deref(),
        &|done, total| {
            if (done.is_multiple_of(100) || done == total) && started.elapsed().as_millis() > 300 {
                eprint!("\rindexing: {done}/{total} streams");
            }
        },
    );
    if started.elapsed().as_millis() > 300 {
        eprintln!();
    }
    index
}

/// `project new --move-workspace`: an empty project with its own workspace,
/// the workspace of the game (or of `--workspace-dir`) moved into it, and
/// its sources.
fn new_from_moved_workspace(
    game: &GameInstance,
    root: &std::path::Path,
    new: NewProject,
    workspace_dir: Option<PathBuf>,
    no_sources: bool,
    no_index: bool,
) -> Result<()> {
    let from = workspace_dir.unwrap_or_else(|| Workspace::default_dir(game.root()));
    if !Workspace::exists(&from) {
        bail!(
            "there is no workspace in {}; `project new` without --move-workspace makes an empty \
             project",
            from.display()
        );
    }
    // Said before the project folder is made.
    let held = Workspace::open_read_only(&from)?.held_streams(game)?;
    if !held.is_empty() {
        bail!(
            "the workspace has its changes in {} stream(s) of the game; put the originals back \
             first: `doctor-toolkit workspace revert {} --game`",
            held.len(),
            quote(game.root())
        );
    }
    // The editor or a server writing it now is named; the lock is let go
    // again, because the folder is moved.
    drop(WorkspaceLock::acquire(&from, "the command line")?);
    let textures = if new.dds { "dds" } else { "png" };
    let models = new.model_format.clone().unwrap_or_else(|| "glb".into());
    let (mut made, _) = project::create_from(
        game,
        &Workspace::for_game(game, Some(&from))?,
        None,
        root,
        &NewProject {
            streams: Vec::new(),
            from_workspace: false,
            ..new
        },
        None,
    )?;
    // The formats its sources are written in.
    if made.config.build.textures != textures || made.config.build.models != models {
        made.config.build.textures = textures.into();
        made.config.build.models = models;
        made.config.write(made.root())?;
    }
    let adopted = project::adopt_workspace(game, &from, &made)?;
    out!(
        "{}: project {} with the workspace of {} moved into it: {} change(s), {} operation(s) \
         of history",
        made.root().display(),
        made.config.project.id,
        from.display(),
        adopted.changes,
        adopted.operations
    );
    if no_sources {
        return Ok(());
    }
    let ws = made.workspace(game)?;
    let index = (!no_index).then(|| open_index(game));
    let started = Instant::now();
    let report = made.export_workspace(game, &ws, index.as_ref())?;
    print_sources(&made, &report, started);
    Ok(())
}

fn print_sources(project: &Project, report: &project::SourcesReport, started: Instant) {
    for id in &report.added {
        out!("{id}: written");
    }
    for id in &report.pulled {
        out!("{id}: taken from the workspace");
    }
    for id in &report.removed {
        out!("{id}: removed (the workspace no longer has it)");
    }
    for stream in &report.streams {
        out!("stream {stream}: described (a stream the project creates)");
    }
    for note in &report.notes {
        out!("  note: {note}");
    }
    out!(
        "{}: {} entr{} written ({} file(s), {:.1} MiB), {} taken from the workspace, {} removed \
         in {:.1} s",
        project.root().display(),
        report.added.len(),
        if report.added.len() == 1 { "y" } else { "ies" },
        report.files,
        report.bytes as f64 / (1 << 20) as f64,
        report.pulled.len(),
        report.removed.len(),
        started.elapsed().as_secs_f64()
    );
}

fn quote(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    if text.contains(' ') {
        format!("\"{text}\"")
    } else {
        text
    }
}

fn build(
    project: &Project,
    game: &GameInstance,
    ws: &mut Workspace,
    only: Vec<String>,
    force: bool,
    dry_run: bool,
) -> Result<()> {
    let started = Instant::now();
    let report = project.build(
        game,
        ws,
        &BuildOptions {
            dry_run,
            force,
            only,
        },
    )?;
    for warning in &report.warnings {
        out!("warning: {warning}");
    }
    for built in &report.built {
        if dry_run {
            out!("would build {} ({})", built.id, built.notes.join(", "));
            continue;
        }
        out!(
            "built {}: {} resource(s) changed",
            built.id,
            built.changed.len()
        );
        for note in &built.notes {
            out!("  {note}");
        }
    }
    for error in &report.errors {
        out!("error: {error}");
    }
    out!(
        "{} built, {} up to date, {} failed in {:.2} s",
        report.built.len(),
        report.up_to_date,
        report.errors.len(),
        started.elapsed().as_secs_f64()
    );
    if let Some(dir) = &report.workspace {
        out!(
            "recorded in workspace {} as one operation; `workspace undo` takes the build back",
            dir.display()
        );
    }
    if !report.errors.is_empty() {
        bail!("{} file(s) failed to build", report.errors.len());
    }
    Ok(())
}

fn print_status(project: &Project, status: &ProjectStatus, all: bool) {
    let p = &project.config.project;
    out!(
        "{} ({} {}), {} entr{}",
        p.name,
        p.id,
        p.version,
        status.entries.len(),
        if status.entries.len() == 1 {
            "y"
        } else {
            "ies"
        }
    );
    for entry in &status.entries {
        if !all && entry.state == EntryState::Unchanged {
            continue;
        }
        let detail = match &entry.state {
            EntryState::Missing(file) => format!(" ({file})"),
            _ => String::new(),
        };
        let patch = entry
            .patch
            .as_ref()
            .map_or_else(String::new, |p| format!(" + {p}"));
        out!(
            "  {:<12} {:<10} {}{patch}{detail}",
            entry.state.label(),
            entry.kind.label(),
            entry.id
        );
    }
    for stream in &status.new_streams {
        if all || !stream.built {
            out!(
                "  {:<12} {:<10} {}/stream.toml ({})",
                if stream.built { "built" } else { "modified" },
                "stream",
                stream.dir,
                stream.stream
            );
        }
    }
    for (dir, stream) in &status.removed_streams {
        out!("  {:<12} {:<10} {dir} ({stream})", "removed", "stream");
    }
    for file in &status.unknown_files {
        out!("  {:<12} {:<10} {file}", "not built", "unknown");
    }
    for warning in &status.warnings {
        out!("warning: {warning}");
    }
    let count = |s: EntryState| status.count(&s);
    out!(
        "{} unchanged, {} built, {} modified, {} out of sync{}",
        count(EntryState::Unchanged),
        count(EntryState::Built),
        count(EntryState::Modified),
        count(EntryState::OutOfSync),
        if status.to_build() > 0 {
            format!("; `project build` builds {}", status.to_build())
        } else {
            String::new()
        }
    );
}
