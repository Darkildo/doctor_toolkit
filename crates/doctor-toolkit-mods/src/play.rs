//! "Play with mods" of Open Mod Manager (stage 46 for players): the game
//! starts with the enabled mods without writing them into its folder. The
//! mods are applied to a staged copy outside the game
//! ([`doctor_toolkit::play::overlay`]) and the live module gives the game the
//! changed files; after the game exits the module and the copy are taken
//! away. When the module cannot be used, a window says what is missing and
//! offers to write the mods into the game instead.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use egui::RichText;

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::live;
use doctor_toolkit::play::overlay::{self, Overlay, Source};
use doctor_toolkit::play::{self, Session, SessionState};

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
    module: Option<live::ModuleInstall>,
    overlay: Option<Overlay>,
}

impl Launched {
    /// Takes the module and the staged copy away.
    fn clean_up(&mut self) {
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
        std::thread::spawn(move || {
            let _ = tx.send(launch(&game, staged).map_err(|e| e.to_string()));
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
                    self.launched = Some(launched);
                    self.message = Some((tr("play_mods.started").to_owned(), false));
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
        }
    }
}

/// Stages the mods (when `staged`), installs the module and starts the game.
fn launch(game: &GameInstance, staged: bool) -> doctor_toolkit::Result<Launched> {
    let root = game.root();
    let running = play::game_processes();
    if !running.is_empty() {
        return Err(doctor_toolkit::Error::InvalidArgument(
            tr("play_mods.already_running").to_owned(),
        ));
    }
    let installs = doctor_toolkit::install::detect(&crate::games::search_roots());
    let method = play::auto_method(root, &installs);
    let plan = play::plan(&method, root)?;
    play::start::restore_stale(root)?;
    live::restore_stale(root)?;
    overlay::remove_stale(root)?;
    let (overlay, module) = if staged {
        let dir = overlay::default_dir(root).ok_or_else(|| {
            doctor_toolkit::Error::InvalidArgument("no folder for the staged files".into())
        })?;
        let built = Overlay::build(game, Some(&Source::Mods), None, &dir)?;
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
        };
        let module = live::ModuleInstall::write(root, &bytes, &config)?;
        (Some(built), Some(module))
    } else {
        (None, None)
    };
    let session = Session::start(&plan)?;
    doctor_toolkit::report::log(&format!(
        "play with mods: {} ({})",
        method.describe(),
        if staged { "staged" } else { "written" }
    ));
    Ok(Launched {
        session,
        module,
        overlay,
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
