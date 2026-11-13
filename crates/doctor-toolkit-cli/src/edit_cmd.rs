//! `edit`: a resource in another program, imported back on every save.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Args;

use doctor_toolkit::external::{
    EditLink, EditorRole, Format, LinkEvent, ModelContext, ModelOptions, Target,
};
use doctor_toolkit::game::{GameAssets, GameInstance};
use doctor_toolkit::workspace::{ApplyOptions, ResourceKey};

use crate::workspace_cmd::{self, EditMode};

macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

#[derive(Args)]
pub struct Edit {
    game_dir: PathBuf,
    /// The resource: its key (`stream:type:ids`, as `nav search` and the
    /// editor show it) or `STREAM:INDEX`
    key: String,
    /// Program that edits the file; `{file}` stands for the file. Default:
    /// the program of `[external_editors]` in the settings for the type,
    /// else the desktop's program for the file type
    #[arg(long = "with", value_name = "CMD")]
    with: Option<String>,
    /// File format: png or dds (textures), csv or json (string tables), json
    /// (logic), lua (scripts), glb (models). Default: the first one
    #[arg(long)]
    format: Option<String>,
    /// Folder for the working file [default: a temporary folder]
    #[arg(long, value_name = "DIR")]
    file_dir: Option<PathBuf>,
    /// Do not start a program; only export the file and watch it
    #[arg(long)]
    no_open: bool,
    /// Stop when the program exits (for editors that stay in the foreground)
    #[arg(long)]
    wait: bool,
    /// Models: import as a new model set with this name instead of replacing
    /// the model; later saves update the same set
    #[arg(long, value_name = "NAME")]
    new: Option<String>,
    /// Models: point this resource of the stream (SimGroup, metamodel) at
    /// the new set (repeatable; needs --new)
    #[arg(long = "retarget", value_name = "INDEX", requires = "new")]
    retarget: Vec<usize>,
    /// Saves less than this many seconds apart make one undoable operation
    #[arg(long, value_name = "SECONDS", default_value_t = 60)]
    coalesce: u64,
    #[command(flatten)]
    mode: EditMode,
}

impl Edit {
    /// Whether every save is also written into the game.
    pub fn is_direct(&self) -> bool {
        self.mode.direct
    }
}

pub fn edit(args: Edit) -> Result<()> {
    let game = Arc::new(GameInstance::open(&args.game_dir)?);
    let format = args
        .format
        .as_deref()
        .map(str::parse::<Format>)
        .transpose()?;
    let target = find_target(&game, &args.key, format)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;

    // Models need the game index for skeletons, materials and free ids.
    let mut models: Models = target.is_model().then(|| {
        out!("indexing the game for the model…");
        let (index, _) = game.build_index(|_| {});
        let assets = GameAssets::new(game.clone(), &index);
        (index, assets)
    });

    let dir = args
        .file_dir
        .clone()
        .unwrap_or_else(|| EditLink::default_dir(&target));
    let mut link = EditLink::create(
        &game,
        Some(&ws),
        target.clone(),
        &dir,
        cx(&mut models),
        None,
    )?;
    link.options = ModelOptions {
        new_name: args.new.clone(),
        retarget: args.retarget.clone(),
    };
    link.coalesce = Duration::from_secs(args.coalesce);
    out!("{} -> {}", target.key, link.path().display());
    if let Some(error) = link.event_error() {
        out!("note: no file events ({error}); checking the file twice a second");
    }
    if !args.no_open {
        let command = args
            .with
            .clone()
            .or_else(|| configured_editor(target.format))
            .unwrap_or_default();
        link.open_editor(&command)
            .with_context(|| format!("cannot open {}", link.path().display()))?;
        out!(
            "opened with {}",
            if command.is_empty() {
                "the desktop's program"
            } else {
                &command
            }
        );
    }
    if !args.mode.direct {
        out!(
            "saves are recorded in workspace {}; `workspace apply` writes them into the game",
            ws.dir().display()
        );
    }
    out!("watching; save the file to import it, Ctrl+C to stop");

    let mut finishing = false;
    loop {
        let now = Instant::now();
        if let Some(event) = link.check(&game, &mut ws, cx(&mut models), now) {
            report(&event);
            if args.mode.direct
                && let LinkEvent::Imported { imported, .. } = &event
                && imported.seq.is_some()
            {
                let report = ws.apply(
                    &game,
                    ApplyOptions {
                        force: false,
                        streams: Some(vec![target.key.stream.clone()]),
                    },
                )?;
                out!("  written into the game: {} file(s)", report.written.len());
            }
        }
        if args.wait && !finishing && link.editor_running() == Some(false) {
            out!("the program exited");
            finishing = true;
        }
        // After the program exits, the last save is waited for and imported.
        if finishing && !link.is_pending() {
            break;
        }
        std::thread::sleep(Duration::from_millis(if link.is_pending() {
            50
        } else {
            250
        }));
    }
    out!("{} save(s) imported", link.imports);
    Ok(())
}

type Models = Option<(doctor_toolkit::game::ResourceIndex, GameAssets)>;

/// The lookups a model link needs, when it is one.
fn cx(models: &mut Models) -> Option<ModelContext<'_>> {
    models
        .as_mut()
        .map(|(index, assets)| ModelContext { index, assets })
}

fn report(event: &LinkEvent) {
    match event {
        LinkEvent::Imported { imported, merged } => {
            match imported.seq {
                Some(_) if *merged => {
                    out!("imported: {} (replaces the previous save)", imported.label)
                }
                Some(_) => out!("imported: {}", imported.label),
                None => out!("imported: no change to the resource"),
            }
            for note in &imported.notes {
                out!("  {note}");
            }
        }
        LinkEvent::Unchanged => out!("saved without changes"),
        LinkEvent::Failed(error) => out!("cannot import: {error}; still watching"),
        LinkEvent::Missing => out!("the file was deleted; save it again to go on"),
    }
}

/// A key, or `STREAM:INDEX`.
fn find_target(game: &GameInstance, text: &str, format: Option<Format>) -> Result<Target> {
    if let Ok(key) = text.parse::<ResourceKey>() {
        return Ok(Target::find(game, &key, format)?);
    }
    if let Some((stream, index)) = text.rsplit_once(':')
        && let Ok(index) = index.trim_start_matches('#').parse::<usize>()
    {
        let s = game
            .find_stream(stream)
            .with_context(|| format!("stream `{stream}` not found"))?;
        return Ok(Target::at(game, s, index, format)?);
    }
    let key: ResourceKey = text.parse()?;
    Ok(Target::find(game, &key, format)?)
}

/// The editor of the GUI settings (`[external_editors]` of
/// `settings.toml`) for the format; `None` when none is set.
fn configured_editor(format: Format) -> Option<String> {
    let path = config_dir()?.join("settings.toml");
    let text = std::fs::read_to_string(path).ok()?;
    let settings: toml::Table = text.parse().ok()?;
    let editors = settings.get("external_editors")?.as_table()?;
    let role = match format.editor_role() {
        EditorRole::Image => "image",
        EditorRole::Model => "model",
        EditorRole::Text => "text",
    };
    let command = editors.get(role)?.as_str()?.trim();
    (!command.is_empty()).then(|| command.to_owned())
}

/// The settings folder of the editor (`DOCTOR_TOOLKIT_CONFIG_DIR`, else the
/// platform's configuration folder), as the GUI finds it.
fn config_dir() -> Option<PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = var("DOCTOR_TOOLKIT_CONFIG_DIR") {
        return Some(dir);
    }
    let base = if cfg!(windows) {
        var("APPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| Path::new(&h).join("Library").join("Application Support"))
    } else {
        var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|h| h.join(".config")))
    };
    base.map(|b| b.join("doctor-toolkit"))
}
