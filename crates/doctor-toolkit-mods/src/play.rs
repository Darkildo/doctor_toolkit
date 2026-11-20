//! "Play with mods" of Open Mod Manager (stage 46 for players): the game
//! starts with the enabled mods without writing them into its folder. The
//! mods are applied to a staged copy outside the game
//! ([`doctor_toolkit::play::overlay`]) and the live module gives the game the
//! changed files; after the game exits the module and the copy are taken
//! away. When the module cannot be used, a window says what is missing and
//! offers to write the mods into the game instead.
//!
//! The launch method, DXVK and the graphics (the graphics module, Large
//! Address Aware, the Vulkan driver) come from the settings shared with the
//! editor ([`crate::launch`]), and are put in the same order as the editor's
//! Play does.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use egui::RichText;

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::live;
use doctor_toolkit::play::overlay::{self, Overlay, Source, Streams};
use doctor_toolkit::play::{self, PlayConfig, Session, SessionState};

use crate::i18n::{tr, trf};
use crate::widgets::{Prominence, action_button};

/// Why the mods cannot be played without writing into the game, in words
/// for players.
pub fn obstacle(root: &Path) -> Option<String> {
    let why = overlay::availability(root).err()?;
    Some(if live::module_binary().is_none() {
        tr("play_mods.need_module").to_owned()
    } else if live::loader::find(root).is_none() {
        tr("play_mods.need_loader").to_owned()
    } else if play::find_executable(root).is_none() {
        tr("play_mods.need_exe").to_owned()
    } else {
        trf("play_mods.need_other", &[("why", &why)])
    })
}

/// A launched game and what goes away after it.
struct Launched {
    session: Session,
    root: PathBuf,
    module: Option<live::ModuleInstall>,
    overlay: Option<Overlay>,
    dxvk: Option<play::dxvk::DxvkInstall>,
    graphics: Option<play::render::GraphicsLaunch>,
    /// What the launch could not do (DXVK, graphics), for the player.
    notes: Vec<String>,
}

impl Launched {
    /// Takes DXVK, the graphics, the module and the staged copy away.
    fn clean_up(&mut self) {
        if let Some(install) = self.dxvk.take()
            && let Err(error) = install.restore()
        {
            doctor_toolkit::report::log(&format!("d3d9.dll not taken away: {error}"));
        }
        // Before the live module: the LAA mirror hands its logs back.
        if let Some(g) = self.graphics.take() {
            match g.finish(&self.root) {
                Ok(lines) => lines.iter().for_each(|l| doctor_toolkit::report::log(l)),
                Err(error) => {
                    doctor_toolkit::report::log(&format!("graphics not taken away: {error}"))
                }
            }
        }
        if let Some(module) = self.module.take()
            && let Err(error) = module.restore()
        {
            doctor_toolkit::report::log(&format!("live module not taken away: {error}"));
        }
        if let Some(o) = self.overlay.take()
            && let Err(error) = o.remove()
        {
            doctor_toolkit::report::log(&format!("staged files not removed: {error}"));
        }
    }
}

/// What the player chose in the window of [`Player`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayChoice {
    /// Write the mods into the game, then start it.
    WriteAndPlay,
}

/// Starts the game with the mods and follows it.
#[derive(Default)]
pub struct Player {
    starting: Option<Receiver<Result<Launched, String>>>,
    launched: Option<Launched>,
    /// Why playing without writing is not possible (the window is open).
    obstacle: Option<String>,
    /// The last word about the game: started, exited, failed.
    message: Option<(String, bool)>,
    /// Whether the resources of the mods are put into the game's streams
    /// in memory (stage 100) instead of rebuilding the streams.
    pub in_memory: bool,
    /// The settings folder: the launch settings shared with the editor
    /// ([`crate::launch`]); `None` — the defaults.
    pub config: Option<PathBuf>,
}

impl Player {
    /// What the Play button says instead while the game starts or runs.
    pub fn busy_label(&self) -> Option<String> {
        if self.starting.is_some() {
            Some(tr("play_mods.starting").to_owned())
        } else if self.launched.is_some() {
            Some(tr("play_mods.running").to_owned())
        } else {
            None
        }
    }

    /// The last word about the game, and whether it is an error.
    pub fn message(&self) -> Option<(&str, bool)> {
        self.message.as_ref().map(|(m, e)| (m.as_str(), *e))
    }

    /// Starts `game` with its enabled mods, without writing into it; when
    /// that is not possible, opens the window that says why.
    pub fn play_with_mods(&mut self, game: Arc<GameInstance>, ctx: &egui::Context) {
        if self.starting.is_some() || self.launched.is_some() {
            return;
        }
        if let Some(why) = obstacle(game.root()) {
            self.obstacle = Some(why);
            return;
        }
        self.spawn(game, true, ctx);
    }

    /// Starts `game` as it is (the mods were written into it).
    pub fn play_written(&mut self, game: Arc<GameInstance>, ctx: &egui::Context) {
        if self.starting.is_some() || self.launched.is_some() {
            return;
        }
        self.spawn(game, false, ctx);
    }

    fn spawn(&mut self, game: Arc<GameInstance>, staged: bool, ctx: &egui::Context) {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        let streams = Streams::in_memory(self.in_memory);
        let config = self
            .config
            .as_deref()
            .map(|c| crate::launch::load(c, game.root()))
            .unwrap_or_default();
        std::thread::spawn(move || {
            let _ = tx.send(launch(&game, staged, streams, &config).map_err(|e| e.to_string()));
            ctx.request_repaint();
        });
        self.starting = Some(rx);
        self.message = None;
    }

    /// Follows the game; `running` are the game's processes.
    pub fn poll(&mut self, running: &[u32]) {
        if let Some(rx) = &self.starting
            && let Ok(result) = rx.try_recv()
        {
            self.starting = None;
            match result {
                Ok(launched) => {
                    let mut text = tr("play_mods.started").to_owned();
                    for note in &launched.notes {
                        text.push('\n');
                        text.push_str(note);
                    }
                    self.launched = Some(launched);
                    self.message = Some((text, false));
                }
                Err(error) => {
                    self.message = Some((trf("play_mods.failed", &[("error", &error)]), true));
                }
            }
        }
        let Some(launched) = &mut self.launched else {
            return;
        };
        match launched.session.poll(running) {
            SessionState::Starting | SessionState::Running(_) => {}
            state => {
                launched.clean_up();
                self.launched = None;
                self.message = Some(match state {
                    SessionState::NotStarted(reason) => {
                        (trf("play_mods.not_started", &[("reason", &reason)]), true)
                    }
                    _ => (tr("play_mods.exited").to_owned(), false),
                });
            }
        }
    }

    /// The window that says why the mods cannot be played without writing
    /// them into the game.
    pub fn show_window(&mut self, ctx: &egui::Context) -> Option<PlayChoice> {
        let why = self.obstacle.clone()?;
        let mut choice = None;
        let mut close = false;
        let response = crate::widgets::modal(ctx, "play-mods").show(ctx, |ui| {
            ui.set_width(480.0);
            ui.label(
                RichText::new(tr("play_mods.cannot_title"))
                    .size(18.0)
                    .strong(),
            );
            ui.add_space(6.0);
            ui.add(egui::Label::new(why).wrap());
            ui.add_space(6.0);
            ui.add(egui::Label::new(tr("play_mods.cannot_offer")).wrap());
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if action_button(ui, true, tr("play_mods.write_and_play"), Prominence::Main)
                    .clicked()
                {
                    choice = Some(PlayChoice::WriteAndPlay);
                    close = true;
                }
                if ui.button(tr("mods.cancel")).clicked() {
                    close = true;
                }
            });
        });
        if close || response.should_close() {
            self.obstacle = None;
        }
        choice
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // The program closes while the game runs: the files stay until the
        // game exits; the next start of a toolkit program takes them away.
        if let Some(launched) = &mut self.launched {
            if let Some(module) = launched.module.take() {
                std::mem::forget(module);
            }
            if let Some(o) = launched.overlay.take() {
                o.keep();
            }
            // DXVK and the graphics module stay as well; the next launch of
            // a toolkit program takes them away (`restore_stale`).
            if let Some(install) = launched.dxvk.take() {
                std::mem::forget(install);
            }
            if let Some(g) = launched.graphics.take() {
                std::mem::forget(g);
            }
        }
    }
}

/// Stages the mods (when `staged`), installs the module and starts the game.
fn launch(
    game: &GameInstance,
    staged: bool,
    streams: Streams,
    config: &PlayConfig,
) -> doctor_toolkit::Result<Launched> {
    let root = game.root();
    let running = play::game_processes();
    if !running.is_empty() {
        return Err(doctor_toolkit::Error::InvalidArgument(
            tr("play_mods.already_running").to_owned(),
        ));
    }
    // The code of the mods the player agreed to (stage 81): staged data
    // still needs its code in the game folder. Before the plan of the
    // launch: a loader installed now changes it (the DLL override of Wine).
    for note in doctor_toolkit::mods::native::prepare(game) {
        doctor_toolkit::report::log(&note);
    }
    let installs = doctor_toolkit::install::detect(&crate::games::search_roots());
    let method = crate::launch::method(config, root, &installs);
    let mut plan = play::plan(&method, root)?;
    let mut notes = Vec::new();
    // Hardware rendering under Wine; when it cannot be used the game still
    // starts, with a word why.
    let dxvk_found = if config.dxvk {
        match play::dxvk::check(&method, root) {
            Ok(found) => {
                play::dxvk::apply_env(&mut plan, root);
                Some(found)
            }
            Err(problem) => {
                notes.push(trf(
                    "dxvk.not_used",
                    &[("reason", &crate::launch::dxvk_problem(&problem))],
                ));
                None
            }
        }
    } else {
        None
    };
    play::start::restore_stale(root)?;
    live::restore_stale(root)?;
    overlay::remove_stale(root)?;
    let (overlay, module) = if staged {
        let dir = overlay::default_dir(root).ok_or_else(|| {
            doctor_toolkit::Error::InvalidArgument("no folder for the staged files".into())
        })?;
        let built = Overlay::build_with(game, Some(&Source::Mods), None, &dir, streams)?;
        let path = live::module_binary().ok_or_else(|| {
            doctor_toolkit::Error::InvalidArgument("the live module is not found".into())
        })?;
        let bytes = std::fs::read(&path).map_err(|e| doctor_toolkit::Error::io(&path, e))?;
        let config = live::Config {
            port: live::free_port(),
            token: live::new_token(),
            writes: false,
            fix_input: true,
            journal: false,
            files_log: false,
            redirects: built.redirects().to_vec(),
            resources: built.resources().to_vec(),
            resources_dump: false,
        };
        let module = live::ModuleInstall::write(root, &bytes, &config)?;
        (Some(built), Some(module))
    } else {
        (None, None)
    };
    play::dxvk::restore_stale(root)?;
    let dxvk = match dxvk_found.map(|found| play::dxvk::DxvkInstall::write(root, &found)) {
        None => None,
        Some(Ok(install)) => Some(install),
        Some(Err(error)) => {
            notes.push(trf("dxvk.not_used", &[("reason", &error)]));
            // The override alone would make Wine look for a d3d9.dll that
            // is not there: start without it.
            crate::launch::remove_dxvk_env(&mut plan);
            None
        }
    };
    // The crash module and the list of mods for its reports (stage 99).
    for note in doctor_toolkit::crash::prepare(
        game,
        "doctor-toolkit-mods",
        if staged {
            "mods staged outside the game"
        } else {
            "written into the game"
        },
        true,
    ) {
        doctor_toolkit::report::log(&note);
    }
    // Graphics last (stage 59): with LAA the mirror links everything put
    // into the game folder above.
    let vulkan = dxvk.as_ref().map(|_| play::dxvk::vulkan32());
    let graphics =
        play::render::prepare(game, &method, &mut plan, &config.graphics, vulkan.as_ref())?;
    for note in &graphics.notes {
        doctor_toolkit::report::log(note);
    }
    notes.extend(graphics.notes.iter().cloned());
    let session = Session::start(&plan)?;
    doctor_toolkit::report::log(&format!(
        "play with mods: {} ({})",
        method.describe(),
        match (staged, streams) {
            (false, _) => "written",
            (true, Streams::Rebuilt) => "staged, streams rebuilt",
            (true, Streams::Patched) => "staged, resources in memory",
        }
    ));
    Ok(Launched {
        session,
        root: root.to_path_buf(),
        module,
        overlay,
        dxvk,
        graphics: (!graphics.is_empty()).then_some(graphics),
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_game_without_the_loader_is_explained() {
        let dir = tempfile::tempdir().unwrap();
        doctor_toolkit::testing::write_game(dir.path()).unwrap();
        let why = obstacle(dir.path()).expect("the mini game cannot load the module");
        assert!(!why.is_empty());
    }
}
