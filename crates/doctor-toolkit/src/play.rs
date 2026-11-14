//! Starting the game and following it: how to launch it ([`LaunchMethod`],
//! [`plan`]), whether it runs ([`game_processes`], [`GameWatch`]) and when
//! a launched game has exited ([`Session`]).
//!
//! The programs (`doctor-toolkit play`, the editor's Play button) apply the
//! workspace before the launch and revert it after the game exits when
//! asked to ([`PlayConfig`]); while the game runs they refuse to write into
//! its files. Since stage 46 they rather let the live module give the game
//! the changed files from a staged copy ([`overlay`]): nothing is written
//! into the game's files and nothing has to be put back.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::profile::GameProfile;
use crate::{Error, Result};

/// How to start the game.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum LaunchMethod {
    /// Chosen from where the game is: see [`auto_method`].
    #[default]
    Auto,
    /// A Steam game, also one run by Proton: `steam -applaunch ID`, or
    /// `steam://rungameid/ID` when the `steam` command is not found.
    Steam { app_id: u32 },
    /// A non-Steam game added to Steam: `steam://rungameid/…`.
    SteamShortcut { app_id: u32 },
    /// `wine godfather2.exe` in the game folder.
    Wine {
        /// `WINEPREFIX`; the default prefix when empty.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prefix: Option<PathBuf>,
        /// The `wine` program; `wine` from `PATH` when empty.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wine: Option<String>,
        /// `WINEDLLOVERRIDES`; when empty and the game has an ASI loader
        /// (`dinput8.dll`), `dinput8=n,b` so that it loads.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dll_overrides: Option<String>,
    },
    /// A Proton build run directly: `proton run godfather2.exe` with the
    /// prefix in `compat_data` (`STEAM_COMPAT_DATA_PATH`).
    Proton {
        proton: PathBuf,
        compat_data: PathBuf,
    },
    /// `lutris lutris:rungame/SLUG`.
    Lutris { game: String },
    /// `heroic://launch?appName=APP&runner=RUNNER`.
    Heroic { app: String, runner: String },
    /// `bottles-cli run -b BOTTLE -e godfather2.exe` (Flatpak or not).
    Bottles { bottle: String, flatpak: bool },
    /// The executable itself (Windows).
    Direct,
    /// A command line of your own; `{exe}` is the executable, `{dir}` the
    /// game folder. Run without a shell; quote words with spaces.
    Command { command: String },
}

impl LaunchMethod {
    /// Lower is better, for ordering suggestions.
    pub fn preference(&self) -> u8 {
        match self {
            Self::Steam { .. } => 0,
            Self::Lutris { .. } | Self::Heroic { .. } | Self::Bottles { .. } => 1,
            Self::SteamShortcut { .. } => 2,
            Self::Direct => 3,
            Self::Wine {
                prefix: Some(_), ..
            } => 4,
            Self::Proton { .. } => 5,
            Self::Wine { .. } => 6,
            Self::Command { .. } => 7,
            Self::Auto => 8,
        }
    }

    /// The name of the method: `steam`, `wine`, …
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Steam { .. } => "steam",
            Self::SteamShortcut { .. } => "steam_shortcut",
            Self::Wine { .. } => "wine",
            Self::Proton { .. } => "proton",
            Self::Lutris { .. } => "lutris",
            Self::Heroic { .. } => "heroic",
            Self::Bottles { .. } => "bottles",
            Self::Direct => "direct",
            Self::Command { .. } => "command",
        }
    }

    /// One line for people.
    pub fn describe(&self) -> String {
        match self {
            Self::Auto => "automatic".to_owned(),
            Self::Steam { app_id } => format!("Steam (app {app_id})"),
            Self::SteamShortcut { app_id } => format!("Steam shortcut ({app_id})"),
            Self::Wine { prefix, .. } => match prefix {
                Some(p) => format!("Wine, prefix {}", p.display()),
                None => "Wine, default prefix".to_owned(),
            },
            Self::Proton {
                proton,
                compat_data,
            } => format!("Proton {} with {}", proton.display(), compat_data.display()),
            Self::Lutris { game } => format!("Lutris ({game})"),
            Self::Heroic { app, runner } => format!("Heroic ({runner} {app})"),
            Self::Bottles { bottle, .. } => format!("Bottles ({bottle})"),
            Self::Direct => "the executable".to_owned(),
            Self::Command { command } => format!("command `{command}`"),
        }
    }

    /// Whether the launcher returns at once and the game runs elsewhere:
    /// only the process list tells when it starts and ends.
    pub fn detached(&self) -> bool {
        matches!(
            self,
            Self::Steam { .. }
                | Self::SteamShortcut { .. }
                | Self::Lutris { .. }
                | Self::Heroic { .. }
                | Self::Bottles { .. }
        )
    }
}

/// How one game is played, kept in the editor's settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayConfig {
    pub method: LaunchMethod,
    /// Write the workspace into the game before starting it.
    pub apply_before: bool,
    /// Put the original files back after the game exits.
    pub revert_after: bool,
    /// The game's own launch options: start right in a level
    /// ([`start::StartOptions`], written to `cmd.txt` for the launch).
    #[serde(skip_serializing_if = "start::StartOptions::is_empty")]
    pub start: start::StartOptions,
    /// The live link: put the module into the game for the launch
    /// ([`crate::live`], stage 34).
    #[serde(skip_serializing_if = "crate::live::LiveOptions::is_default")]
    pub live: crate::live::LiveOptions,
    /// Draw with the graphics card under Wine: DXVK's `d3d9.dll` in the
    /// game folder for the launch ([`dxvk`]).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub dxvk: bool,
    /// Whether the workspace goes into the game through the live module
    /// instead of being written ([`overlay`], stage 46).
    #[serde(skip_serializing_if = "overlay::OverlayMode::is_default")]
    pub overlay: overlay::OverlayMode,
}

/// The method for a game folder when nothing better is known: the
/// executable on Windows, Wine elsewhere (in the prefix that holds the
/// folder, if any).
pub fn fallback_method(game_root: &Path) -> LaunchMethod {
    if cfg!(windows) {
        return LaunchMethod::Direct;
    }
    let prefix = game_root
        .ancestors()
        .find(|a| a.join("drive_c").is_dir() && a.join("system.reg").is_file())
        .map(Path::to_path_buf);
    LaunchMethod::Wine {
        prefix,
        wine: None,
        dll_overrides: None,
    }
}

/// [`LaunchMethod::Auto`] made concrete: the first method detection
/// suggests for this folder ([`crate::install::Install::launch`]), else
/// [`fallback_method`].
pub fn auto_method(game_root: &Path, installs: &[crate::install::Install]) -> LaunchMethod {
    let real = std::fs::canonicalize(game_root).unwrap_or_else(|_| game_root.to_path_buf());
    installs
        .iter()
        .find(|i| std::fs::canonicalize(&i.path).unwrap_or_else(|_| i.path.clone()) == real)
        .and_then(|i| i.launch.first().cloned())
        .unwrap_or_else(|| fallback_method(game_root))
}

/// A command to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub program: OsString,
    pub args: Vec<OsString>,
    /// Variables added to the environment.
    pub env: Vec<(String, OsString)>,
    /// Working directory.
    pub dir: PathBuf,
    /// The launcher returns at once; see [`LaunchMethod::detached`].
    pub detached: bool,
}

impl LaunchPlan {
    /// The command as a shell would show it, for logs and `--dry-run`.
    pub fn command_line(&self) -> String {
        let quote = |s: &std::ffi::OsStr| {
            let s = s.to_string_lossy();
            if s.is_empty() || s.contains([' ', '"', '\'', '&', '?', ';', '$']) {
                format!("'{}'", s.replace('\'', "'\\''"))
            } else {
                s.into_owned()
            }
        };
        let mut parts: Vec<String> = self
            .env
            .iter()
            .map(|(k, v)| format!("{k}={}", quote(v)))
            .collect();
        parts.push(quote(&self.program));
        parts.extend(self.args.iter().map(|a| quote(a)));
        format!("cd {} && {}", quote(self.dir.as_os_str()), parts.join(" "))
    }
}

/// Name of the Wine desktop of a game run on an X server of its own.
pub const VIRTUAL_DESKTOP: &str = "doctor_toolkit";

/// Runs the game of a Wine or Proton `plan` in a Wine virtual desktop of
/// `size` (`explorer /desktop=doctor_toolkit,WxH godfather2.exe`).
/// On a bare X server of that size the game stops at "Your Graphics Card
/// does not meet the minimum requirements" (checked on the bench); in a
/// virtual desktop it starts, and its mode changes stay inside it. Other
/// plans (a script of one's own, launchers) are left as they are.
pub fn in_virtual_desktop(plan: &mut LaunchPlan, size: [u32; 2]) {
    let program = std::path::Path::new(&plan.program)
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if !(program.starts_with("wine") || program == "proton") {
        return;
    }
    let Some(at) = plan
        .args
        .iter()
        .position(|a| a.to_string_lossy().to_ascii_lowercase().ends_with(".exe"))
    else {
        return;
    };
    if at > 0 && plan.args[at - 1].to_string_lossy().starts_with("/desktop=") {
        return;
    }
    let desktop = format!("/desktop={VIRTUAL_DESKTOP},{}x{}", size[0], size[1]);
    plan.args
        .splice(at..at, ["explorer".into(), desktop.into()]);
}

/// The game executable in `root`.
pub fn find_executable(root: &Path) -> Option<PathBuf> {
    let profile = GameProfile::detect(root)?;
    std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .find(|e| {
            e.file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(profile.executable)
        })
        .map(|e| e.path())
}

/// The command that starts the game in `root` with `method`
/// ([`LaunchMethod::Auto`] must be resolved first, see [`auto_method`]).
pub fn plan(method: &LaunchMethod, root: &Path) -> Result<LaunchPlan> {
    let exe = find_executable(root).ok_or_else(|| Error::NotAGame {
        path: root.to_path_buf(),
    })?;
    let mut plan = LaunchPlan {
        program: OsString::new(),
        args: Vec::new(),
        env: Vec::new(),
        dir: root.to_path_buf(),
        detached: method.detached(),
    };
    match method {
        LaunchMethod::Auto => {
            return Err(Error::InvalidArgument(
                "the automatic launch method is resolved before planning".into(),
            ));
        }
        LaunchMethod::Steam { app_id } => {
            if let Some(steam) = find_program("steam") {
                plan.program = steam.into();
                plan.args = vec!["-applaunch".into(), app_id.to_string().into()];
            } else {
                open_url(&mut plan, &format!("steam://rungameid/{app_id}"));
            }
        }
        LaunchMethod::SteamShortcut { app_id } => {
            // The 64-bit game id of a shortcut: app id, then 0x02000000.
            let id = (u64::from(*app_id) << 32) | 0x0200_0000;
            open_url(&mut plan, &format!("steam://rungameid/{id}"));
        }
        LaunchMethod::Wine {
            prefix,
            wine,
            dll_overrides,
        } => {
            plan.program = wine.clone().unwrap_or_else(|| "wine".to_owned()).into();
            plan.args = vec![exe.clone().into()];
            if let Some(prefix) = prefix {
                plan.env.push(("WINEPREFIX".into(), prefix.clone().into()));
            }
            let overrides = dll_overrides.clone().or_else(|| {
                root.join("dinput8.dll")
                    .is_file()
                    .then(|| "dinput8=n,b".to_owned())
            });
            if let Some(overrides) = overrides.filter(|o| !o.is_empty()) {
                plan.env.push(("WINEDLLOVERRIDES".into(), overrides.into()));
            }
        }
        LaunchMethod::Proton {
            proton,
            compat_data,
        } => {
            let script = if proton.is_dir() {
                proton.join("proton")
            } else {
                proton.clone()
            };
            plan.program = script.into();
            plan.args = vec!["run".into(), exe.clone().into()];
            plan.env
                .push(("STEAM_COMPAT_DATA_PATH".into(), compat_data.clone().into()));
            let client = std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join(".steam/steam"))
                .unwrap_or_default();
            plan.env
                .push(("STEAM_COMPAT_CLIENT_INSTALL_PATH".into(), client.into()));
        }
        LaunchMethod::Lutris { game } => {
            let url = format!("lutris:rungame/{game}");
            match find_program("lutris") {
                Some(lutris) => {
                    plan.program = lutris.into();
                    plan.args = vec![url.into()];
                }
                None => {
                    plan.program = "flatpak".into();
                    plan.args = vec!["run".into(), "net.lutris.Lutris".into(), url.into()];
                }
            }
        }
        LaunchMethod::Heroic { app, runner } => {
            let url = format!(
                "heroic://launch?appName={}&runner={}",
                url_encode(app),
                url_encode(runner)
            );
            open_url(&mut plan, &url);
        }
        LaunchMethod::Bottles { bottle, flatpak } => {
            let mut args: Vec<OsString> = Vec::new();
            if *flatpak {
                plan.program = "flatpak".into();
                args.extend([
                    "run".into(),
                    "--command=bottles-cli".into(),
                    "com.usebottles.bottles".into(),
                ]);
            } else {
                plan.program = "bottles-cli".into();
            }
            args.extend([
                "run".into(),
                "-b".into(),
                bottle.into(),
                "-e".into(),
                exe.clone().into(),
            ]);
            plan.args = args;
        }
        LaunchMethod::Direct => {
            plan.program = exe.clone().into();
        }
        LaunchMethod::Command { command } => {
            let words = split_words(command)?;
            let fill = |w: &str| {
                w.replace("{exe}", &exe.to_string_lossy())
                    .replace("{dir}", &root.to_string_lossy())
            };
            let mut words = words.iter().map(|w| OsString::from(fill(w)));
            plan.program = words
                .next()
                .ok_or_else(|| Error::InvalidArgument("the launch command is empty".into()))?;
            plan.args = words.collect();
        }
    }
    Ok(plan)
}

/// Opens `url` with the desktop: `xdg-open`, `open` on macOS, `start` on Windows.
fn open_url(plan: &mut LaunchPlan, url: &str) {
    if cfg!(windows) {
        plan.program = "cmd".into();
        plan.args = vec!["/c".into(), "start".into(), "".into(), url.into()];
    } else if cfg!(target_os = "macos") {
        plan.program = "open".into();
        plan.args = vec![url.into()];
    } else {
        plan.program = "xdg-open".into();
        plan.args = vec![url.into()];
    }
}

fn url_encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A program in `PATH`.
pub fn find_program(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// Splits a command line into words: spaces separate, single and double
/// quotes group, a backslash escapes the next character outside single quotes.
pub fn split_words(text: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('\''), c) => word.push(c),
            (_, '\\') => {
                if let Some(next) = chars.next() {
                    word.push(next);
                }
                in_word = true;
            }
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            (None, c) => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return Err(Error::InvalidArgument(format!(
            "unclosed quote in `{text}`"
        )));
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

/// Starts `plan`. Its output goes nowhere; the game keeps running when the
/// toolkit exits.
pub fn launch(plan: &LaunchPlan) -> Result<Child> {
    spawn(plan, &mut command(plan))
}

/// The command of `plan`, not started yet, with its output going nowhere:
/// callers may add to it (the editor runs the game on a display of its own).
pub fn command(plan: &LaunchPlan) -> Command {
    let mut command = Command::new(&plan.program);
    command
        .args(&plan.args)
        .current_dir(&plan.dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (key, value) in &plan.env {
        command.env(key, value);
    }
    command
}

/// Starts `command` made by [`command`] from `plan`.
pub fn spawn(plan: &LaunchPlan, command: &mut Command) -> Result<Child> {
    crate::report::log(&format!("launch: {}", plan.command_line()));
    command.spawn().map_err(|e| {
        Error::io(
            PathBuf::from(&plan.program),
            std::io::Error::new(e.kind(), format!("cannot start the game: {e}")),
        )
    })
}

// --------------------------------------------------------------- processes

/// Process ids of running copies of the game, natively or under Wine and
/// Proton (Linux: `/proc`; Windows: `tasklist`; elsewhere: none found).
///
/// `DOCTOR_TOOLKIT_GAME_PROCESS` replaces the executable names looked for
/// (comma-separated, lower case); tests use it so that a game running
/// elsewhere on the machine does not count.
pub fn game_processes() -> Vec<u32> {
    if let Ok(names) = std::env::var("DOCTOR_TOOLKIT_GAME_PROCESS") {
        let names: Vec<String> = names.split(',').map(|n| n.trim().to_lowercase()).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        return processes_named(&names);
    }
    let names: Vec<&str> = GameProfile::ALL.iter().map(|p| p.executable).collect();
    processes_named(&names)
}

/// Process ids of programs named `names` (lower case, e.g. `godfather2.exe`).
pub fn processes_named(names: &[&str]) -> Vec<u32> {
    if cfg!(target_os = "linux") {
        scan_proc(Path::new("/proc"), names)
    } else if cfg!(windows) {
        tasklist(names)
    } else {
        Vec::new()
    }
}

/// Scans a `/proc`-like directory.
pub fn scan_proc(proc_dir: &Path, names: &[&str]) -> Vec<u32> {
    let mut pids = Vec::new();
    let Ok(entries) = std::fs::read_dir(proc_dir) else {
        return pids;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == std::process::id() {
            continue;
        }
        let cmdline = std::fs::read(entry.path().join("cmdline")).unwrap_or_default();
        let comm = std::fs::read_to_string(entry.path().join("comm")).unwrap_or_default();
        if is_named(&cmdline, comm.trim_end(), names) {
            pids.push(pid);
        }
    }
    pids.sort_unstable();
    pids
}

/// Whether a process with this command line (NUL-separated) and short name
/// is one of `names`. Only the program counts, not its arguments:
/// `wine godfather2.exe` is the Wine loader starting the game, and
/// `doctor-toolkit … godfather2.exe` is not the game. Under Wine the game's
/// own process has the Windows path of the executable as its program.
pub fn is_named(cmdline: &[u8], comm: &str, names: &[&str]) -> bool {
    let program = cmdline.split(|&b| b == 0).next().unwrap_or_default();
    let program = String::from_utf8_lossy(program).to_ascii_lowercase();
    let base = program.rsplit(['/', '\\']).next().unwrap_or_default();
    if names.contains(&base) {
        return true;
    }
    // A kernel thread or a process whose command line is gone: its short
    // name (15 characters at most).
    cmdline.is_empty() && {
        let comm = comm.to_ascii_lowercase();
        names
            .iter()
            .any(|n| !comm.is_empty() && (comm == *n || (comm.len() == 15 && n.starts_with(&comm))))
    }
}

#[cfg(windows)]
fn tasklist(names: &[&str]) -> Vec<u32> {
    let mut pids = Vec::new();
    for name in names {
        let output = Command::new("tasklist")
            .args(["/FI", &format!("IMAGENAME eq {name}"), "/NH", "/FO", "CSV"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        if let Ok(output) = output {
            pids.extend(parse_tasklist(
                &String::from_utf8_lossy(&output.stdout),
                names,
            ));
        }
    }
    pids.sort_unstable();
    pids.dedup();
    pids
}

#[cfg(not(windows))]
fn tasklist(_names: &[&str]) -> Vec<u32> {
    Vec::new()
}

/// Process ids in `tasklist /FO CSV /NH` output: `"name","pid",…`.
pub fn parse_tasklist(output: &str, names: &[&str]) -> Vec<u32> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split("\",\"");
            let name = fields.next()?.trim_start_matches('"').to_ascii_lowercase();
            let pid = fields.next()?.trim_matches('"').parse().ok()?;
            names.contains(&name.as_str()).then_some(pid)
        })
        .collect()
}

/// Follows whether the game runs, on a thread of its own: the editor
/// reads [`Self::running`] every frame without scanning processes.
pub struct GameWatch {
    pids: Arc<Mutex<Vec<u32>>>,
    stop: Arc<AtomicBool>,
}

impl GameWatch {
    /// Scans every `interval`; `changed` is called when the list changes.
    pub fn start(interval: Duration, changed: impl Fn() + Send + 'static) -> Self {
        Self::start_with(interval, game_processes, changed)
    }

    /// Like [`Self::start`] with another scan (for tests).
    pub fn start_with(
        interval: Duration,
        scan: impl Fn() -> Vec<u32> + Send + 'static,
        changed: impl Fn() + Send + 'static,
    ) -> Self {
        let pids = Arc::new(Mutex::new(scan()));
        let stop = Arc::new(AtomicBool::new(false));
        let (shared, stopped) = (Arc::clone(&pids), Arc::clone(&stop));
        std::thread::Builder::new()
            .name("game-watch".into())
            .spawn(move || {
                while !stopped.load(Ordering::Relaxed) {
                    std::thread::sleep(interval);
                    let now = scan();
                    let mut current = shared.lock().unwrap_or_else(|e| e.into_inner());
                    if *current != now {
                        *current = now;
                        drop(current);
                        changed();
                    }
                }
            })
            .ok();
        Self { pids, stop }
    }

    /// A watch that never sees the game (screenshots, tests).
    pub fn disabled() -> Self {
        Self {
            pids: Arc::default(),
            stop: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Process ids of the game, as of the last scan.
    pub fn running(&self) -> Vec<u32> {
        self.pids.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Records a scan made elsewhere (the editor's tests).
    pub fn set_running(&self, pids: Vec<u32>) {
        *self.pids.lock().unwrap_or_else(|e| e.into_inner()) = pids;
    }
}

impl Drop for GameWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Where a launched game is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// The launcher runs; the game is not seen yet.
    Starting,
    /// The game runs: these processes (or the launched program itself).
    Running(Vec<u32>),
    /// The game ran and exited.
    Exited,
    /// The launcher ended, or the start timed out, and the game was never seen.
    NotStarted(String),
}

/// A launched game.
pub struct Session {
    child: Option<Child>,
    detached: bool,
    started: Instant,
    seen: bool,
    /// How long a detached launcher may take to start the game.
    pub start_timeout: Duration,
}

impl Session {
    /// Launches `plan`.
    pub fn start(plan: &LaunchPlan) -> Result<Self> {
        // A program run directly is the game (or brings it up) itself.
        Ok(Self::adopt(launch(plan)?, plan.detached))
    }

    /// Follows a program started by the caller (see [`command`]), as
    /// [`Session::start`] follows the one it starts.
    pub fn adopt(child: Child, detached: bool) -> Self {
        Self {
            child: Some(child),
            detached,
            started: Instant::now(),
            seen: !detached,
            start_timeout: Duration::from_secs(180),
        }
    }

    /// The launched program's process id.
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    /// The state after a process scan that found `running`.
    pub fn poll(&mut self, running: &[u32]) -> SessionState {
        let mut child_alive = false;
        let mut failed = None;
        if let Some(child) = &mut self.child {
            match child.try_wait() {
                Ok(None) => child_alive = true,
                Ok(Some(status)) => {
                    if !status.success() && !self.detached {
                        failed = Some(format!("the launcher exited with {status}"));
                    }
                    self.child = None;
                }
                Err(_) => self.child = None,
            }
        }
        if !running.is_empty() {
            self.seen = true;
            return SessionState::Running(running.to_vec());
        }
        if child_alive && !self.detached {
            return SessionState::Running(self.pid().into_iter().collect());
        }
        if let Some(reason) = failed
            && self.started.elapsed() < Duration::from_secs(5)
        {
            return SessionState::NotStarted(reason);
        }
        if self.seen {
            return SessionState::Exited;
        }
        if self.started.elapsed() > self.start_timeout {
            return SessionState::NotStarted(format!(
                "the game did not start within {} s",
                self.start_timeout.as_secs()
            ));
        }
        SessionState::Starting
    }

    /// Waits for the game to exit, scanning every `interval`.
    pub fn wait(&mut self, interval: Duration) -> SessionState {
        loop {
            match self.poll(&game_processes()) {
                SessionState::Starting | SessionState::Running(_) => std::thread::sleep(interval),
                done => return done,
            }
        }
    }
}

pub mod dxvk;
pub mod overlay;
pub mod start;

#[cfg(test)]
mod tests;
