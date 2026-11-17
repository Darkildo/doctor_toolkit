//! Graphics of a launch (stage 59): the graphics module, Large Address
//! Aware and the Vulkan driver.
//!
//! - **The graphics module** `doctor_render.asi` (crate
//!   `doctor-toolkit-render-asi`, [`doctor_toolkit_live::render`]): the time
//!   of day without lost seconds, vsync at the refresh rate instead of 30
//!   frames a second, a frame limit, the field of view, FXAA and
//!   sharpening. "Play" puts it and `doctor_render.cfg` into `scripts/` of
//!   the game for the launch and takes them away afterwards
//!   ([`RenderInstall`]), like the live module; a player may also keep them
//!   there ([`install_permanent`]).
//! - **Large Address Aware** ([`LaaMirror`]): the loader of the system reads
//!   the flag `0x20` of the executable when it makes the process, so no
//!   module can set it. "Play" makes a mirror of the game folder (symbolic
//!   links, as for stage 46), puts a copy of `godfather2.exe` with the flag
//!   into it and starts that copy; the game's own executable is never
//!   written.
//! - **One Vulkan driver** for DXVK on Linux ([`super::dxvk::driver_files`]).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use doctor_toolkit_live::render::{CONFIG_FILE, LOG_FILE, MODULE_FILE, RenderConfig};

use super::{LaunchMethod, LaunchPlan};
use crate::game::GameInstance;
use crate::live::SCRIPTS_DIR;
use crate::{Error, Result};

/// Graphics of the launches of one game, kept in the editor's settings
/// ([`super::PlayConfig::graphics`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphicsOptions {
    /// Put the graphics module into the game for the launch.
    pub module: bool,
    /// The time of day without lost seconds.
    pub time_of_day: bool,
    /// Vsync at the refresh rate (60 at 60 Hz) instead of 30.
    pub vsync_one: bool,
    /// The game's limiter sleeps instead of spinning.
    pub limiter_sleep: bool,
    /// Frame limit, frames a second; 0 — none.
    pub fps: u32,
    /// Horizontal field of view at 16:9, degrees; 0 — the game's 60.
    pub fov: u32,
    pub fxaa: bool,
    /// Sharpening, 0–100.
    pub sharpen: u32,
    /// Fewer draw calls (stage 101): draws that follow each other with
    /// nothing changed between them go to Direct3D as one.
    pub draw_merge: bool,
    /// Start a copy of the executable with Large Address Aware.
    pub laa: bool,
    /// The memory test of LAA: megabytes left free below 2 GB; 0 — off.
    pub low_memory: u32,
    /// `VK_DRIVER_FILES` for DXVK: `auto`, a driver (`nvidia`, `radeon`,
    /// `intel`) or empty — all drivers.
    pub vulkan_driver: String,
}

impl Default for GraphicsOptions {
    fn default() -> Self {
        let c = RenderConfig::default();
        Self {
            module: false,
            time_of_day: c.time_of_day,
            vsync_one: c.vsync_one,
            limiter_sleep: c.limiter_sleep,
            fps: c.fps,
            fov: c.fov,
            fxaa: c.fxaa,
            sharpen: c.sharpen,
            draw_merge: false,
            laa: false,
            low_memory: 0,
            vulkan_driver: String::new(),
        }
    }
}

impl GraphicsOptions {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The module's configuration.
    pub fn config(&self) -> RenderConfig {
        // Through the text: the module's clamping applies.
        RenderConfig::parse(
            &RenderConfig {
                time_of_day: self.time_of_day,
                vsync_one: self.vsync_one,
                limiter_sleep: self.limiter_sleep,
                fps: self.fps,
                fov: self.fov,
                fxaa: self.fxaa,
                sharpen: self.sharpen,
                draw_merge: u32::from(self.draw_merge),
                low_memory: self.low_memory,
                memory_log: if self.laa { 30 } else { 0 },
                force_vsync: false,
                fov_trace: false,
                ..RenderConfig::default()
            }
            .to_text(),
        )
    }

    /// Whether a launch with these options needs the module.
    pub fn wants_module(&self) -> bool {
        self.module
    }
}

// ------------------------------------------------------------- the module

/// The module file to install: `DOCTOR_RENDER_MODULE`, `doctor_render.asi`
/// next to the running program, or the build of this repository
/// (`tools/build-render-module.sh` → `target/live/doctor_render.asi`).
pub fn module_binary() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("DOCTOR_RENDER_MODULE") {
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

/// Where the toolkit records what it installed for a launch.
const STATE_DIR: &str = "_doctor_toolkit/render";
const WRITTEN_MODULE: &str = "doctor_render.asi.written";
const WRITTEN_CONFIG: &str = "doctor_render.cfg.written";
/// Marks that `scripts/` was created by the toolkit.
const MADE_SCRIPTS: &str = "scripts.made";
/// The module's log of the last launch, kept after the files are taken
/// away.
pub const LAST_LOG: &str = "_doctor_toolkit/render/last.log";

/// The module in a game folder for one launch; [`RenderInstall::restore`]
/// (or [`restore_stale`] after a crash) takes it away: files someone
/// changed in between stay, the log moves to [`LAST_LOG`].
#[derive(Debug)]
pub struct RenderInstall {
    root: PathBuf,
    restored: bool,
}

impl RenderInstall {
    /// Installs `module` (the DLL bytes) with `config` into `root`.
    pub fn write(root: &Path, module: &[u8], config: &RenderConfig) -> Result<Self> {
        restore_stale(root)?;
        let scripts = root.join(SCRIPTS_DIR);
        let state = root.join(STATE_DIR);
        std::fs::create_dir_all(&state).map_err(|e| Error::io(&state, e))?;
        let target = scripts.join(MODULE_FILE);
        if target.exists() {
            return Err(Error::InvalidArgument(format!(
                "{} exists and was not installed for this launch (the player's own: `render uninstall` takes it away)",
                target.display()
            )));
        }
        if !scripts.is_dir() {
            std::fs::create_dir_all(&scripts).map_err(|e| Error::io(&scripts, e))?;
            crate::atomic::write(&state.join(MADE_SCRIPTS), b"")?;
        }
        let text = config.to_text();
        crate::atomic::write(&state.join(WRITTEN_MODULE), module)?;
        crate::atomic::write(&state.join(WRITTEN_CONFIG), text.as_bytes())?;
        crate::atomic::write(&target, module)?;
        crate::atomic::write(&scripts.join(CONFIG_FILE), text.as_bytes())?;
        let _ = std::fs::remove_file(scripts.join(LOG_FILE));
        Ok(Self {
            root: root.to_path_buf(),
            restored: false,
        })
    }

    /// Takes the files away; returns whether the game folder changed.
    pub fn restore(mut self) -> Result<bool> {
        self.restored = true;
        restore_stale(&self.root)
    }
}

impl Drop for RenderInstall {
    fn drop(&mut self) {
        if !self.restored {
            let _ = restore_stale(&self.root);
        }
    }
}

/// Takes away a module the toolkit installed for an earlier launch.
pub fn restore_stale(root: &Path) -> Result<bool> {
    let state = root.join(STATE_DIR);
    let written_module = state.join(WRITTEN_MODULE);
    let written_config = state.join(WRITTEN_CONFIG);
    if !written_module.exists() && !written_config.exists() {
        return Ok(false);
    }
    let scripts = root.join(SCRIPTS_DIR);
    let mut changed = false;
    for (target, written) in [
        (scripts.join(MODULE_FILE), &written_module),
        (scripts.join(CONFIG_FILE), &written_config),
    ] {
        let mine = std::fs::read(written).ok();
        if mine.is_some() && std::fs::read(&target).ok() == mine {
            std::fs::remove_file(&target).map_err(|e| Error::io(&target, e))?;
            changed = true;
        }
    }
    let log = scripts.join(LOG_FILE);
    if log.is_file() {
        let keep = root.join(LAST_LOG);
        let _ = std::fs::remove_file(&keep);
        std::fs::rename(&log, &keep).map_err(|e| Error::io(&log, e))?;
        changed = true;
    }
    if state.join(MADE_SCRIPTS).exists() {
        if std::fs::remove_dir(&scripts).is_ok() {
            changed = true;
        }
        let _ = std::fs::remove_file(state.join(MADE_SCRIPTS));
    }
    for path in [&written_module, &written_config] {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io(path, e)),
        }
    }
    Ok(changed)
}

/// Whether the player keeps the module in the game (`scripts/
/// doctor_render.asi` not installed for a launch).
pub fn permanent(root: &Path) -> bool {
    root.join(SCRIPTS_DIR).join(MODULE_FILE).is_file()
        && !root.join(STATE_DIR).join(WRITTEN_MODULE).is_file()
}

/// Puts the module and its configuration into `scripts/` of the game for
/// good (the player's choice: `render install`); a launch then leaves them
/// alone. Refuses while a launch of the toolkit has its own there.
pub fn install_permanent(root: &Path, module: &[u8], config: &RenderConfig) -> Result<PathBuf> {
    if root.join(STATE_DIR).join(WRITTEN_MODULE).is_file() {
        return Err(Error::InvalidArgument(
            "the module is installed for a running launch; install after the game exits".into(),
        ));
    }
    let scripts = root.join(SCRIPTS_DIR);
    std::fs::create_dir_all(&scripts).map_err(|e| Error::io(&scripts, e))?;
    let target = scripts.join(MODULE_FILE);
    crate::atomic::write(&target, module)?;
    crate::atomic::write(&scripts.join(CONFIG_FILE), config.to_text().as_bytes())?;
    Ok(target)
}

/// Takes away the module the player keeps in the game (and its
/// configuration and log). Returns whether there was one.
pub fn uninstall_permanent(root: &Path) -> Result<bool> {
    if root.join(STATE_DIR).join(WRITTEN_MODULE).is_file() {
        return restore_stale(root);
    }
    let scripts = root.join(SCRIPTS_DIR);
    let mut found = false;
    for name in [MODULE_FILE, CONFIG_FILE, LOG_FILE] {
        let path = scripts.join(name);
        if path.is_file() {
            std::fs::remove_file(&path).map_err(|e| Error::io(&path, e))?;
            found = true;
        }
    }
    Ok(found)
}

/// The module's log: of the running or last launch, or of the player's
/// own module.
pub fn last_log(root: &Path) -> Option<String> {
    std::fs::read_to_string(root.join(SCRIPTS_DIR).join(LOG_FILE))
        .or_else(|_| std::fs::read_to_string(root.join(LAST_LOG)))
        .ok()
}

// ------------------------------------------------------ Large Address Aware

/// `IMAGE_FILE_LARGE_ADDRESS_AWARE` of `IMAGE_FILE_HEADER.Characteristics`.
pub const LARGE_ADDRESS_AWARE: u16 = 0x0020;

/// The offset of `Characteristics` in a PE file.
fn characteristics_at(exe: &[u8]) -> Option<usize> {
    let pe = u32::from_le_bytes(exe.get(0x3C..0x40)?.try_into().ok()?) as usize;
    (exe.get(..2)? == b"MZ" && exe.get(pe..pe + 4)? == b"PE\0\0").then_some(pe + 4 + 18)
}

/// Whether a PE file has the flag.
pub fn is_large_address_aware(exe: &[u8]) -> Option<bool> {
    let at = characteristics_at(exe)?;
    let c = u16::from_le_bytes(exe.get(at..at + 2)?.try_into().ok()?);
    Some(c & LARGE_ADDRESS_AWARE != 0)
}

/// A copy of a PE file with the flag set. The game's executable has no
/// checksum (0), so nothing else changes.
pub fn with_large_address_aware(exe: &[u8]) -> Result<Vec<u8>> {
    let at = characteristics_at(exe)
        .filter(|&at| at + 2 <= exe.len())
        .ok_or_else(|| Error::InvalidArgument("not a PE executable".into()))?;
    let mut out = exe.to_vec();
    let c = u16::from_le_bytes([out[at], out[at + 1]]) | LARGE_ADDRESS_AWARE;
    out[at..at + 2].copy_from_slice(&c.to_le_bytes());
    Ok(out)
}

/// Why LAA cannot be used with `method`, if it cannot: the launchers start
/// their own executable.
pub fn laa_problem(method: &LaunchMethod) -> Option<String> {
    match method {
        LaunchMethod::Wine { .. }
        | LaunchMethod::Proton { .. }
        | LaunchMethod::Direct
        | LaunchMethod::Command { .. } => None,
        LaunchMethod::Steam { .. } | LaunchMethod::SteamShortcut { .. } => Some(
            "Steam starts its own executable (Proton gives 32-bit games 4 GB by itself: PROTON_FORCE_LARGE_ADDRESS_AWARE)"
                .into(),
        ),
        other => Some(format!(
            "the launcher ({}) starts its own executable",
            other.kind()
        )),
    }
}

/// The mirror of a game folder for a launch with LAA: `<dir>/game` holds a
/// symbolic link to every file of the game, except the executable — a copy
/// with the flag. Made after everything else of the launch is in the game
/// folder (the live module, DXVK, the graphics module), so that it links
/// them too. The modules write their logs into the mirror;
/// [`LaaMirror::finish`] moves them where their launches look.
#[derive(Debug)]
pub struct LaaMirror {
    overlay: super::overlay::Overlay,
    root: PathBuf,
}

/// The folder of the LAA mirror of the game in `root`.
pub fn laa_dir(root: &Path) -> Option<PathBuf> {
    let dir = super::overlay::default_dir(root)?;
    let name = dir.file_name()?.to_string_lossy().into_owned();
    Some(dir.with_file_name(format!("{name}-laa")))
}

/// Removes the mirror of an earlier launch. Returns whether there was one.
pub fn remove_stale_mirror(root: &Path) -> Result<bool> {
    let Some(dir) = laa_dir(root) else {
        return Ok(false);
    };
    let found = std::fs::symlink_metadata(&dir).is_ok();
    if found {
        std::fs::remove_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
    }
    Ok(found)
}

impl LaaMirror {
    /// Mirrors `game` into `dir` with the executable flagged.
    pub fn build(game: &GameInstance, dir: &Path) -> Result<Self> {
        let exe = super::find_executable(game.root()).ok_or_else(|| Error::NotAGame {
            path: game.root().to_path_buf(),
        })?;
        let data = std::fs::read(&exe).map_err(|e| Error::io(&exe, e))?;
        let flagged = with_large_address_aware(&data)?;
        let overlay = super::overlay::Overlay::build(game, None, None, dir)?;
        let root = dir.join("game");
        let target = root.join(exe.file_name().unwrap_or_default());
        // A real file in place of the link: atomic::write renames over it.
        crate::atomic::write(&target, &flagged)?;
        Ok(Self { overlay, root })
    }

    /// The game folder the launch runs in.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Points a plan made for `game_root` at the mirror: the executable and
    /// the working folder.
    pub fn retarget(&self, plan: &mut LaunchPlan, game_root: &Path) {
        retarget(plan, game_root, &self.root);
    }

    /// After the game exited: the modules' logs from the mirror into the
    /// game folder (where their launches take them from), then the mirror
    /// goes.
    pub fn finish(self, game_root: &Path) -> Result<()> {
        for name in [crate::live::proto::LOG_FILE, LOG_FILE] {
            let log = self.root.join(SCRIPTS_DIR).join(name);
            let to = game_root.join(SCRIPTS_DIR).join(name);
            if std::fs::symlink_metadata(&log).is_ok_and(|m| m.is_file())
                && game_root.join(SCRIPTS_DIR).is_dir()
            {
                let _ = std::fs::copy(&log, &to);
            }
        }
        // The crash module wrote its reports into the mirror (stage 99).
        crate::crash::collect_from(&self.root, game_root);
        self.overlay.remove()
    }
}

/// [`LaaMirror::retarget`] for given folders.
pub fn retarget(plan: &mut LaunchPlan, game_root: &Path, mirror_root: &Path) {
    let swap = |word: &OsString| -> Option<OsString> {
        let path = Path::new(word);
        let rel = path.strip_prefix(game_root).ok()?;
        rel.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
            .then(|| mirror_root.join(rel).into_os_string())
    };
    if let Some(p) = swap(&plan.program) {
        plan.program = p;
    }
    for arg in &mut plan.args {
        if let Some(a) = swap(arg) {
            *arg = a;
        }
    }
    if plan.dir == game_root {
        plan.dir = mirror_root.to_path_buf();
    }
}

// ----------------------------------------------------------- one launch

/// What a launch did for its graphics; [`GraphicsLaunch::finish`] after the
/// game exited.
#[derive(Debug, Default)]
pub struct GraphicsLaunch {
    module: Option<RenderInstall>,
    mirror: Option<LaaMirror>,
    /// Lines for the person: what was done, what could not be and why.
    pub notes: Vec<String>,
}

impl GraphicsLaunch {
    pub fn is_empty(&self) -> bool {
        self.module.is_none() && self.mirror.is_none()
    }

    /// The folder the game runs in (the mirror with LAA).
    pub fn mirror_root(&self) -> Option<&Path> {
        self.mirror.as_ref().map(|m| m.root())
    }

    /// Takes away what the launch put into the game and removes the
    /// mirror. Returns lines for the person.
    pub fn finish(self, game_root: &Path) -> Result<Vec<String>> {
        let mut out = Vec::new();
        if let Some(m) = self.mirror {
            m.finish(game_root)?;
            out.push("removed the mirror of the game with Large Address Aware".to_owned());
        }
        if let Some(module) = self.module
            && module.restore()?
        {
            out.push(format!(
                "took away the graphics module (its log: {})",
                game_root.join(LAST_LOG).display()
            ));
        }
        Ok(out)
    }
}

/// Prepares the graphics of a launch of `game` with `plan` (made for the
/// game folder by `method`), after everything else of the launch is in the
/// game folder: the graphics module, `VK_DRIVER_FILES` (when `dxvk` is
/// used) and LAA (the plan then starts the mirror). What cannot be done is
/// said in [`GraphicsLaunch::notes`]; the game still starts.
pub fn prepare(
    game: &GameInstance,
    method: &LaunchMethod,
    plan: &mut LaunchPlan,
    options: &GraphicsOptions,
    dxvk: Option<&super::dxvk::Vulkan32>,
) -> Result<GraphicsLaunch> {
    let root = game.root();
    let mut out = GraphicsLaunch::default();
    restore_stale(root)?;
    remove_stale_mirror(root)?;
    if options.wants_module() {
        if permanent(root) {
            out.notes.push(format!(
                "graphics: the player's own {} in scripts/ is used (its doctor_render.cfg)",
                MODULE_FILE
            ));
        } else if crate::live::loader::find(root).is_none() {
            out.notes.push(
                "graphics: no ASI loader (dinput8.dll) in the game folder: the graphics module cannot load"
                    .into(),
            );
        } else {
            match module_binary() {
                None => out.notes.push(
                    "graphics: the module is not built (tools/build-render-module.sh)".into(),
                ),
                Some(path) => {
                    let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
                    let config = options.config();
                    out.module = Some(RenderInstall::write(root, &bytes, &config)?);
                    out.notes
                        .push(format!("graphics: module installed: {}", config.summary()));
                }
            }
        }
    }
    if let Some(vulkan) = dxvk
        && !options.vulkan_driver.trim().is_empty()
    {
        match super::dxvk::apply_driver_env(plan, vulkan, &options.vulkan_driver) {
            Some(files) => out.notes.push(format!(
                "graphics: VK_DRIVER_FILES={}",
                files.to_string_lossy()
            )),
            None => out.notes.push(format!(
                "graphics: no Vulkan driver chosen for `{}` (all drivers load)",
                options.vulkan_driver
            )),
        }
    }
    if options.laa {
        if let Some(why) = laa_problem(method) {
            out.notes
                .push(format!("graphics: no Large Address Aware: {why}"));
        } else {
            let dir = laa_dir(root).ok_or_else(|| {
                Error::InvalidArgument("no folder for the mirror (no HOME or LOCALAPPDATA)".into())
            })?;
            match LaaMirror::build(game, &dir) {
                Ok(mirror) => {
                    mirror.retarget(plan, root);
                    out.notes.push(format!(
                        "graphics: Large Address Aware: the game starts from a mirror with a flagged copy of the executable ({})",
                        mirror.root().display()
                    ));
                    out.mirror = Some(mirror);
                }
                Err(e) => {
                    let _ = remove_stale_mirror(root);
                    out.notes
                        .push(format!("graphics: no Large Address Aware: {e}"));
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
