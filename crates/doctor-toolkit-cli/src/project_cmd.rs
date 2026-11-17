//! `project` subcommands: a mod as a folder of source files (`docs/project.md`).

use std::io::Write as _;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::navigation::GameIndex;
use doctor_toolkit::navigation::index::default_cache_file;
use doctor_toolkit::project::{self, BuildOptions, EntryState, NewProject, Project, ProjectStatus};
use doctor_toolkit::workspace::{ApplyOptions, Workspace};

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
    /// Make a project from streams of the game, or from the changes of the workspace
    ///
    /// Every resource of the chosen streams becomes a file in a common format
    /// (PNG, JSON, Lua, glTF, ...) under `src/<stream>.str/<type>/<name>`;
    /// resources equal in several streams go to `shared/` once.
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
        #[arg(long = "stream", required_unless_present = "from_workspace")]
        streams: Vec<String>,
        /// Only resources of this type: a tag such as `tg4h` (repeatable)
        #[arg(long = "type")]
        types: Vec<String>,
        /// Take the resources the workspace changes, as it has them
        #[arg(long, conflicts_with = "streams")]
        from_workspace: bool,
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
}

impl ProjectCommand {
    /// Whether the command can change the files of the game.
    pub fn may_write_game(&self) -> bool {
        matches!(self, Self::Build { apply: true, .. })
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
            workspace_dir,
            dds,
            model_format,
            no_shared,
            no_index,
        } => {
            let game = GameInstance::open(&game_dir)?;
            let ws = Workspace::for_game(&game, workspace_dir.as_deref())?;
            let index = if no_index {
                None
            } else {
                let started = Instant::now();
                let (index, _) = GameIndex::open(
                    &game,
                    default_cache_file(game.root()).as_deref(),
                    &|done, total| {
                        if (done.is_multiple_of(100) || done == total)
                            && started.elapsed().as_millis() > 300
                        {
                            eprint!("\rindexing: {done}/{total} streams");
                        }
                    },
                );
                if started.elapsed().as_millis() > 300 {
                    eprintln!();
                }
                Some(index)
            };
            let started = Instant::now();
            let (made, report) = project::create_from(
                &game,
                &ws,
                index.as_ref(),
                &project,
                &NewProject {
                    name,
                    id,
                    author,
                    streams,
                    types,
                    from_workspace,
                    dds,
                    model_format: Some(model_format),
                    no_shared,
                },
                workspace_dir,
            )?;
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
            let report = project.build_mod(&ws, &output, preview.as_ref())?;
            out!(
                "{}: mod {} with {} resource(s) ({} added), {:.1} MiB",
                output.display(),
                project.config.project.id,
                report.resources,
                report.added,
                report.file_size as f64 / (1 << 20) as f64
            );
            Ok(())
        }
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
