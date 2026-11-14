//! `detect` and `play`: finding installed games and starting one.

use std::io::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, ValueEnum};

use doctor_toolkit::edition::Edition;
use doctor_toolkit::game::{GameInstance, looks_like_game};
use doctor_toolkit::install::{self, Install, SearchRoots};
use doctor_toolkit::play::dxvk;
use doctor_toolkit::play::overlay::{self, Choice, Overlay, OverlayMode, Source};
use doctor_toolkit::play::start::{self, CmdFile, StartOptions};
use doctor_toolkit::play::{self, LaunchMethod, Session, SessionState};
use doctor_toolkit::workspace::{ApplyOptions, Workspace};

/// Writes a line to stdout and exits quietly when the pipe is closed.
macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

/// `detect`.
#[derive(Args)]
pub struct DetectArgs {
    /// Check these folders (game folders or Wine prefixes) instead of
    /// searching the computer
    dirs: Vec<PathBuf>,
    /// Print JSON
    #[arg(long)]
    json: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum MethodArg {
    Auto,
    Steam,
    SteamShortcut,
    Wine,
    Proton,
    Lutris,
    Heroic,
    Bottles,
    Direct,
    Command,
}

/// `play`.
#[derive(Args)]
pub struct PlayArgs {
    game_dir: PathBuf,
    /// How to start the game [default: auto — from where the game was found]
    #[arg(long, value_enum)]
    method: Option<MethodArg>,
    /// Steam application id (`steam`, `steam-shortcut`) [default: 24830]
    #[arg(long, value_name = "ID")]
    app_id: Option<u32>,
    /// Wine prefix (`wine`) or Proton's compatdata folder (`proton`)
    #[arg(long, value_name = "DIR")]
    prefix: Option<PathBuf>,
    /// The `wine` program (`wine`)
    #[arg(long, value_name = "PROGRAM")]
    wine: Option<String>,
    /// `WINEDLLOVERRIDES` (`wine`); `dinput8=n,b` when the game has an ASI loader
    #[arg(long, value_name = "LIST")]
    dll_overrides: Option<String>,
    /// Proton folder or script (`proton`)
    #[arg(long, value_name = "DIR")]
    proton: Option<PathBuf>,
    /// Lutris game slug, Heroic app name or Bottles bottle
    #[arg(long, value_name = "NAME")]
    launcher_game: Option<String>,
    /// Heroic runner (`heroic`)
    #[arg(long, default_value = "sideload")]
    heroic_runner: String,
    /// Run Bottles through Flatpak (`bottles`)
    #[arg(long)]
    flatpak: bool,
    /// Command line (`command`): `{exe}` is the executable, `{dir}` the game folder
    #[arg(long, value_name = "LINE")]
    command: Option<String>,
    /// Write the workspace into the game first
    #[arg(long)]
    apply: bool,
    /// Wait for the game to exit, then put the original files back (implies `--wait`)
    #[arg(long)]
    revert_after: bool,
    /// Wait for the game to exit
    #[arg(long)]
    wait: bool,
    /// Print how the game would be started and do nothing
    #[arg(long)]
    dry_run: bool,
    /// Workspace directory [default: <GAME_DIR>/_doctor_toolkit/workspace]
    #[arg(long, value_name = "DIR")]
    workspace_dir: Option<PathBuf>,
    /// Start right in this stream, without menus, profile or save, e.g.
    /// `gf2/cuba` or `mp_mall/mp_mall` (repeat for more streams under one
    /// folder). Written to cmd.txt in the game folder and taken away when
    /// the game exits (implies `--wait`)
    #[arg(long, value_name = "PATH")]
    stream: Vec<String>,
    /// Appear at this PlayerSpawn object (its GUID as `--list-spawns` prints it)
    #[arg(long, value_name = "GUID", requires = "stream")]
    spawn: Option<String>,
    /// Print the PlayerSpawn objects of the `--stream` streams and exit
    #[arg(long, requires = "stream")]
    list_spawns: bool,
    /// Run the game in a window (`-windowed` in cmd.txt)
    #[arg(long, conflicts_with = "fullscreen")]
    windowed: bool,
    /// Run the game full screen (`-fullscreen` in cmd.txt)
    #[arg(long)]
    fullscreen: bool,
    /// Window size, e.g. 1024x768 (`-resolution` in cmd.txt)
    #[arg(long, value_name = "WxH", value_parser = parse_resolution)]
    resolution: Option<(u32, u32)>,
    /// Put the live link module into the game for this launch
    /// (`scripts/doctor_live.asi`, needs the ASI loader) and take it away
    /// when the game exits (implies `--wait`); see `doctor-toolkit live`
    #[arg(long)]
    live: bool,
    /// With `--live`: allow teleport, messages and Lua
    #[arg(long, requires = "live")]
    live_writes: bool,
    /// With `--live`: port on 127.0.0.1 [default: a free one from 47801]
    #[arg(long, requires = "live")]
    live_port: Option<u16>,
    /// Draw with the graphics card under Wine: DXVK's 32-bit d3d9.dll (of
    /// Lutris, Proton, Bottles, Heroic or `DOCTOR_DXVK`) in the game folder
    /// for this launch, taken away when the game exits (implies `--wait`;
    /// needs 32-bit Vulkan)
    #[arg(long)]
    dxvk: bool,
    /// Play the enabled mods of the mod manager without writing them into
    /// the game: the live module gives the game their files (implies
    /// `--wait`); see `--write-game`
    #[arg(long, conflicts_with_all = ["apply", "revert_after"])]
    mods: bool,
    /// Write the workspace and `cmd.txt` into the game folder (and put them
    /// back after the game exits) instead of letting the live module give
    /// the game those files. By default the module is used when it is
    /// built, the game has the ASI loader and the edition fits
    #[arg(long, conflicts_with_all = ["no_game_writes", "mods"])]
    write_game: bool,
    /// Never write the workspace or `cmd.txt` into the game folder: fail
    /// when the live module cannot give the game those files
    #[arg(long)]
    no_game_writes: bool,
}

fn parse_resolution(text: &str) -> std::result::Result<(u32, u32), String> {
    let bad = || format!("`{text}`: expected WIDTHxHEIGHT, e.g. 1024x768");
    let (w, h) = text.split_once(['x', 'X']).ok_or_else(bad)?;
    let side = |s: &str| s.trim().parse::<u32>().ok().filter(|&n| n > 0);
    Ok((side(w).ok_or_else(bad)?, side(h).ok_or_else(bad)?))
}

impl PlayArgs {
    /// Whether the command writes into the game.
    pub fn writes(&self) -> bool {
        (self.apply
            || self.revert_after
            || self.live
            || self.dxvk
            || self.mods
            || !self.start_options().is_empty())
            && !self.dry_run
            && !self.list_spawns
    }

    /// The game's own launch options (`cmd.txt`).
    pub fn start_options(&self) -> StartOptions {
        StartOptions {
            streams: self.stream.clone(),
            spawn: self.spawn.clone(),
            windowed: if self.windowed {
                Some(true)
            } else if self.fullscreen {
                Some(false)
            } else {
                None
            },
            resolution: self.resolution,
        }
    }

    fn overlay_mode(&self) -> OverlayMode {
        if self.write_game {
            OverlayMode::Off
        } else if self.no_game_writes || self.mods {
            OverlayMode::On
        } else {
            OverlayMode::Auto
        }
    }

    fn method(&self) -> Result<LaunchMethod> {
        let name = |what: &str| {
            self.launcher_game
                .clone()
                .with_context(|| format!("`--launcher-game` names the {what}"))
        };
        Ok(match self.method.unwrap_or(MethodArg::Auto) {
            MethodArg::Auto => LaunchMethod::Auto,
            MethodArg::Steam => LaunchMethod::Steam {
                app_id: self.app_id.unwrap_or(install::STEAM_APP_ID),
            },
            MethodArg::SteamShortcut => LaunchMethod::SteamShortcut {
                app_id: self.app_id.context("`--app-id` names the shortcut")?,
            },
            MethodArg::Wine => LaunchMethod::Wine {
                prefix: self.prefix.clone(),
                wine: self.wine.clone(),
                dll_overrides: self.dll_overrides.clone(),
            },
            MethodArg::Proton => LaunchMethod::Proton {
                proton: self
                    .proton
                    .clone()
                    .context("`--proton` names the Proton folder")?,
                compat_data: self
                    .prefix
                    .clone()
                    .context("`--prefix` names the compatdata folder")?,
            },
            MethodArg::Lutris => LaunchMethod::Lutris {
                game: name("Lutris game")?,
            },
            MethodArg::Heroic => LaunchMethod::Heroic {
                app: name("Heroic app")?,
                runner: self.heroic_runner.clone(),
            },
            MethodArg::Bottles => LaunchMethod::Bottles {
                bottle: name("bottle")?,
                flatpak: self.flatpak,
            },
            MethodArg::Direct => LaunchMethod::Direct,
            MethodArg::Command => LaunchMethod::Command {
                command: self
                    .command
                    .clone()
                    .context("`--command` gives the command line")?,
            },
        })
    }
}

/// `detect`: installed copies of the game.
pub fn detect(args: &DetectArgs) -> Result<()> {
    let roots = if args.dirs.is_empty() {
        SearchRoots::from_env()
    } else {
        SearchRoots {
            extra: args.dirs.clone(),
            ..SearchRoots::default()
        }
    };
    let installs = install::detect(&roots);
    let editions: Vec<Edition> = installs
        .iter()
        .map(|i| Edition::identify(&i.path))
        .collect();
    if args.json {
        let list: Vec<serde_json::Value> = installs
            .iter()
            .zip(&editions)
            .map(|(install, edition)| serde_json::json!({ "install": install, "edition": edition }))
            .collect();
        out!("{}", serde_json::to_string_pretty(&list)?);
        return Ok(());
    }
    if installs.is_empty() {
        if args.dirs.iter().any(|d| looks_like_game(d).is_none()) || args.dirs.is_empty() {
            out!("no game found");
            if args.dirs.is_empty() {
                out!(
                    "searched Steam libraries, Wine and Proton prefixes, Lutris, Heroic and Bottles; \
                     `doctor-toolkit detect DIR` checks a folder or a prefix"
                );
            }
        }
        return Ok(());
    }
    for (install, edition) in installs.iter().zip(&editions) {
        print_install(install, edition);
    }
    Ok(())
}

fn print_install(install: &Install, edition: &Edition) {
    out!("{}", install.path.display());
    out!("  game: {}", install.profile);
    out!("  edition: {}", edition.summary());
    if let Some(exe) = &edition.executable {
        out!(
            "  executable: {} {} bytes, sha256 {}, version {}, built {}, sections {}",
            exe.file_name,
            exe.size,
            exe.sha256,
            exe.version.as_deref().unwrap_or("?"),
            exe.built.as_deref().unwrap_or("?"),
            exe.sections.join(" ")
        );
    }
    for origin in &install.found_by {
        out!("  found by {}: {}", origin.kind.label(), origin.detail);
    }
    for (i, method) in install.launch.iter().enumerate() {
        out!(
            "  {} {}",
            if i == 0 { "start with" } else { "      or" },
            method.describe()
        );
    }
}

/// `play`: starts the game, optionally with the workspace applied first and
/// reverted after it exits.
pub fn play(args: &PlayArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    if args.list_spawns {
        return list_spawns(&game, &args.stream);
    }
    let options = args.start_options();
    let cmd_txt = if options.is_empty() {
        None
    } else {
        Some(options.cmd_txt()?)
    };
    let mut method = args.method()?;
    if method == LaunchMethod::Auto {
        let installs = install::detect(&SearchRoots::from_env());
        method = play::auto_method(game.root(), &installs);
    }
    let mut plan = play::plan(&method, game.root())?;
    let dxvk = if args.dxvk {
        let found =
            dxvk::check(&method, game.root()).map_err(|p| anyhow::anyhow!("--dxvk: {p}"))?;
        dxvk::apply_env(&mut plan, game.root());
        out!("dxvk: {} ({})", found.source, found.dll.display());
        Some(found)
    } else {
        None
    };
    out!("method: {}", method.describe());
    out!("command: {}", plan.command_line());
    if let Some(text) = &cmd_txt {
        out!("{}: {}", start::CMD_FILE, text.trim_end());
    }
    // Through the live module, without writing into the game (stage 46),
    // when there is something to bring in.
    let wants = args.apply || args.mods || cmd_txt.is_some();
    let choice = if wants {
        overlay::choose(game.root(), args.overlay_mode())?
    } else {
        Choice::Write(None)
    };
    if let Choice::Write(Some(why)) = &choice {
        out!("note: the game folder is written for this launch: {why}");
    }
    let staged = choice == Choice::Overlay;
    if args.dry_run {
        if staged {
            out!(
                "would give the game {} through the live module, without writing into its folder",
                if args.mods {
                    "the enabled mods"
                } else if args.apply {
                    "the workspace"
                } else {
                    start::CMD_FILE
                }
            );
        } else {
            if args.apply {
                out!("would apply the workspace first");
            }
            if args.revert_after {
                out!("would put the original files back after the game exits");
            }
        }
        return Ok(());
    }
    let running = play::game_processes();
    if !running.is_empty() {
        bail!("the game is already running (process {})", join(&running));
    }
    if start::restore_stale(game.root())? {
        out!("took away the {} of an earlier launch", start::CMD_FILE);
    }
    if doctor_toolkit::live::restore_stale(game.root())? {
        out!("took away the live module of an earlier launch");
    }
    if dxvk::restore_stale(game.root())? {
        out!("took away the {} of an earlier launch", dxvk::DLL);
    }
    if overlay::remove_stale(game.root())? {
        out!("removed the staged files of an earlier launch");
    }
    let ws_dir = args
        .workspace_dir
        .clone()
        .unwrap_or_else(|| Workspace::default_dir(game.root()));
    let mut workspace = None;
    let staged_overlay = if staged {
        let source = if args.mods {
            Some(Source::Mods)
        } else if args.apply && Workspace::exists(&ws_dir) {
            Some(Source::Workspace(ws_dir.clone()))
        } else {
            if args.apply {
                out!("no workspace in {}: nothing to apply", ws_dir.display());
            }
            None
        };
        let dir = overlay::default_dir(game.root())
            .context("no folder for the staged files (no HOME or LOCALAPPDATA)")?;
        let built = Overlay::build(&game, source.as_ref(), cmd_txt.as_deref(), &dir)
            .context("cannot stage the changes; the game was not started")?;
        out!(
            "staged: {} file(s) the game opens from {} instead of its folder ({} stream(s) rebuilt)",
            built.redirects().len(),
            dir.display(),
            built.report().streams.len()
        );
        for warning in &built.report().warnings {
            out!("warning: {warning}");
        }
        Some(built)
    } else {
        None
    };
    if !staged && (args.apply || args.revert_after) {
        if Workspace::exists(&ws_dir) {
            crate::workspace_cmd::lock(&ws_dir)?;
            workspace = Some(Workspace::open(&ws_dir)?);
        } else {
            out!("no workspace in {}: nothing to apply", ws_dir.display());
        }
    }
    if args.apply
        && let Some(ws) = &mut workspace
    {
        let report = ws
            .apply(&game, ApplyOptions::default())
            .context("cannot apply the workspace; the game was not started")?;
        out!(
            "applied: {} stream(s) written, {} file(s)",
            report.streams.len(),
            report.written.len()
        );
        for warning in &report.warnings {
            out!("warning: {warning}");
        }
    }
    let cmd_file = match &cmd_txt {
        Some(_) if !staged => Some(CmdFile::write(game.root(), &options)?),
        _ => None,
    };
    let redirects = staged_overlay
        .as_ref()
        .map(|o| o.redirects().to_vec())
        .unwrap_or_default();
    let live_module = if args.live || staged {
        Some(install_live(args, game.root(), redirects)?)
    } else if !args.stream.is_empty() {
        input_fix(game.root())?
    } else {
        None
    };
    let dxvk_install = match &dxvk {
        Some(found) => Some(dxvk::DxvkInstall::write(game.root(), found)?),
        None => None,
    };
    let mut session = Session::start(&plan)?;
    out!("started");
    if !(args.wait
        || args.revert_after
        || staged_overlay.is_some()
        || cmd_file.is_some()
        || live_module.is_some()
        || dxvk_install.is_some())
    {
        out!("`--wait` waits for the game to exit, `--revert-after` also reverts the workspace");
        return Ok(());
    }
    let mut shown = false;
    let state = loop {
        match session.poll(&play::game_processes()) {
            SessionState::Starting => {}
            SessionState::Running(pids) => {
                if !shown {
                    out!("the game is running (process {})", join(&pids));
                    shown = true;
                }
            }
            done => break done,
        }
        std::thread::sleep(Duration::from_secs(1));
    };
    match &state {
        SessionState::NotStarted(reason) => out!("warning: {reason}"),
        _ => out!("the game has exited"),
    }
    if let Some(file) = cmd_file
        && file.restore()?
    {
        out!("took away {}", start::CMD_FILE);
    }
    if let Some(module) = live_module
        && module.restore()?
    {
        out!(
            "took away the live module (its log: {})",
            game.root().join(doctor_toolkit::live::LAST_LOG).display()
        );
    }
    if let Some(install) = dxvk_install
        && install.restore()?
    {
        out!("took away {}", dxvk::DLL);
    }
    if let Some(o) = staged_overlay {
        let total = o.redirects().len();
        o.remove()?;
        let log = std::fs::read_to_string(game.root().join(doctor_toolkit::live::LAST_LOG))
            .unwrap_or_default();
        out!(
            "removed the staged files; the game opened {} of the {total} redirected file(s), its folder was not written",
            doctor_toolkit::live::redirected_in_log(&log)
        );
    }
    if args.revert_after
        && let Some(ws) = &mut workspace
    {
        let report = ws.unapply(&game, false)?;
        out!(
            "reverted: {} stream(s) put back, {} file(s)",
            report.streams.len(),
            report.written.len()
        );
    }
    Ok(())
}

/// `play --list-spawns`: the PlayerSpawn objects of the streams.
fn list_spawns(game: &GameInstance, streams: &[String]) -> Result<()> {
    for stream in streams {
        let points = start::spawn_points(game, stream)?;
        if points.is_empty() {
            out!("{stream}: no PlayerSpawn objects");
        }
        for point in points {
            let at = point
                .position
                .map(|p| format!("({:.2}, {:.2}, {:.2})", p[0], p[1], p[2]))
                .unwrap_or_else(|| "(no position)".to_owned());
            let mut notes = Vec::new();
            if point.kind == Some(2) {
                notes.push("default".to_owned());
            }
            if let Some(message) = &point.message {
                notes.push(format!("sends {message}"));
            }
            let notes = if notes.is_empty() {
                String::new()
            } else {
                format!(" [{}]", notes.join(", "))
            };
            out!(
                "{} {} {at} {}{notes}",
                point.stream,
                point.guid,
                point.arg()
            );
        }
    }
    Ok(())
}

fn join(pids: &[u32]) -> String {
    pids.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// `play --stream` without `--live`: the module only so that the player
/// can move (`live::install_for_input`).
fn input_fix(root: &std::path::Path) -> Result<Option<doctor_toolkit::live::ModuleInstall>> {
    use doctor_toolkit::live::{self, InputFix};
    Ok(match live::install_for_input(root)? {
        InputFix::Installed(install) => {
            out!(
                "live: module installed for this launch so that the player can move after a start straight into a level (writes off)"
            );
            Some(install)
        }
        InputFix::NoLoader => {
            out!(
                "warning: no ASI loader (dinput8.dll) in the game folder: after a start straight into a level the player cannot move (the game gives him no controller); put Ultimate ASI Loader there or start from a save"
            );
            None
        }
        InputFix::NoModule => {
            out!(
                "warning: the live module is not built (tools/build-live-module.sh): after a start straight into a level the player cannot move"
            );
            None
        }
    })
}

/// `play --live` (or a launch through the module, stage 46): the module and
/// its configuration in the game folder, with the files it redirects.
fn install_live(
    args: &PlayArgs,
    root: &std::path::Path,
    redirects: Vec<doctor_toolkit::live::proto::files::Redirect>,
) -> Result<doctor_toolkit::live::ModuleInstall> {
    use doctor_toolkit::live;
    let path = live::module_binary()
        .context("the live module is not built: run tools/build-live-module.sh")?;
    let bytes = std::fs::read(&path).with_context(|| path.display().to_string())?;
    match live::loader::find(root) {
        Some(l) => out!(
            "live: ASI loader {}{}",
            l.file.display(),
            if l.ultimate {
                " (Ultimate ASI Loader)"
            } else {
                ""
            }
        ),
        None => bail!(
            "no ASI loader (dinput8.dll) in the game folder: the game would not load the live module"
        ),
    }
    let config = live::Config {
        port: args.live_port.unwrap_or_else(live::free_port),
        token: live::new_token(),
        writes: args.live_writes,
        fix_input: true,
        // The journal's hooks only with the live link.
        journal: args.live,
        files_log: false,
        redirects,
    };
    let install = live::ModuleInstall::write(root, &bytes, &config)?;
    out!(
        "live: module installed, port {}, writes {}{}",
        config.port,
        if config.writes { "on" } else { "off" },
        if config.redirects.is_empty() {
            String::new()
        } else {
            format!(", {} file(s) redirected", config.redirects.len())
        }
    );
    Ok(install)
}
