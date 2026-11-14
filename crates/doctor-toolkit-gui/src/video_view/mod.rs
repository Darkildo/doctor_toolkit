//! Movies: the game's `.vp6` movies with info, a player (built-in VP6
//! decoder, sound in sync), WAV/PNG export and replacement. A document tab
//! since stage 56 (`Pane::Movies`; a window before): the list and the
//! player in the tab, information, export and import in Details.

mod player;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use doctor_toolkit::resource::audio::wav;
use doctor_toolkit::resource::video::import::{self, AudioSource};
use doctor_toolkit::resource::video::{self, BuildOptions, MovieFile, MovieFrames, MovieInfo, vp6};
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::RichText;

use player::Player;

struct Loaded {
    rel_path: String,
    data: Arc<Vec<u8>>,
    info: Result<MovieInfo, String>,
}

#[derive(Default)]
pub struct MoviesWindow {
    pub open: bool,
    root: Option<PathBuf>,
    movies: Vec<MovieFile>,
    selected: Option<usize>,
    loaded: Option<Loaded>,
    player: Option<Player>,
    /// Frame to show when the player starts.
    start_frame: usize,
    /// Start playing as soon as the player exists (double click, preset).
    autoplay: bool,
    /// The picture over the whole editor window.
    full: bool,
    volume: Option<(f32, bool)>,
    quantizer: u8,
    audio: AudioChoice,
    job: Option<Receiver<Result<MovieImport, String>>>,
    message: String,
    /// `DOCTOR_TOOLKIT_GUI_MOVIES=<name>[:<frame>][,play][,full]` selects a
    /// movie at start.
    pending: Option<(String, usize)>,
}

/// A movie built by Import; the app records it in the workspace.
pub struct MovieImport {
    /// Path relative to the game directory (`movies/common/mainmenu.vp6`).
    pub rel_path: String,
    pub data: Vec<u8>,
    pub label: String,
    pub message: String,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum AudioChoice {
    #[default]
    Keep,
    None,
    File,
}

impl MoviesWindow {
    pub fn new() -> Self {
        let env = std::env::var("DOCTOR_TOOLKIT_GUI_MOVIES").ok();
        let mut options = env.as_deref().unwrap_or("").split(',');
        let target = options.next().unwrap_or("");
        let flags: Vec<&str> = options.collect();
        let pending = Some(target)
            .filter(|s| !s.is_empty() && *s != "1")
            .map(|s| match s.rsplit_once(':') {
                Some((name, frame)) if frame.parse::<usize>().is_ok() => {
                    (name.to_owned(), frame.parse().unwrap_or(0))
                }
                _ => (s.to_owned(), 0),
            });
        Self {
            open: env.is_some(),
            autoplay: flags.contains(&"play"),
            full: flags.contains(&"full"),
            quantizer: 52,
            pending,
            ..Default::default()
        }
    }

    /// Waiting for a picture (screenshots wait for it).
    pub fn busy(&self) -> bool {
        self.open && self.player.as_ref().is_some_and(Player::busy)
    }

    /// Keeps the list, the player and an import going while the Movies tab
    /// is open (stage 56: a tab, a window before). Returns a finished import
    /// for the app to record.
    pub fn update(&mut self, ctx: &egui::Context, root: &Path) -> Option<MovieImport> {
        if !self.open {
            if let Some(p) = &mut self.player {
                p.stop();
            }
            return None;
        }
        let mut finished = None;
        if self.root.as_deref() != Some(root) {
            self.root = Some(root.to_owned());
            self.movies = video::game_movies(root).unwrap_or_default();
            self.selected = None;
            self.loaded = None;
            self.player = None;
            if let Some((name, frame)) = self.pending.take() {
                self.selected = self.movies.iter().position(|m| m.rel_path.contains(&name));
                self.start_frame = frame;
            }
        }
        if let Some(rx) = &self.job {
            match rx.try_recv() {
                Ok(result) => {
                    self.message = match result {
                        Ok(done) => {
                            let message = done.message.clone();
                            finished = Some(done);
                            message
                        }
                        Err(e) => trf("movies.import_failed", &[("error", &e)]),
                    };
                    self.job = None;
                    // Re-read the replaced movie.
                    self.loaded = None;
                    self.player = None;
                    self.movies = video::game_movies(root).unwrap_or_default();
                }
                Err(_) => ctx.request_repaint_after(std::time::Duration::from_millis(200)),
            }
        }
        self.load_selected(ctx);
        if let Some(p) = &mut self.player {
            p.update(ctx);
        }
        if self.full && self.player.is_some() {
            self.full_window(ctx);
        } else {
            self.full = false;
        }
        finished
    }

    /// The Movies tab closed: the sound stops.
    pub fn close(&mut self) {
        self.open = false;
        if let Some(p) = &mut self.player {
            p.stop();
        }
    }

    /// The Movies tab: the list on the left, the picture and the play bar;
    /// the information, export and import under them, or in Details when
    /// `details` (it is open and shows them).
    pub fn ui(&mut self, ui: &mut egui::Ui, details: bool) {
        egui::Panel::left(ui.scope_id().with("movie_list"))
            .resizable(true)
            .default_size(240.0)
            .show(ui, |ui| self.list(ui));
        egui::CentralPanel::default().show(ui, |ui| self.picture_ui(ui, details));
    }

    /// The selected movie's information and its export and import, for
    /// the Details panel.
    pub fn details_ui(&mut self, ui: &mut egui::Ui) {
        let Some(movie) = self.selected.and_then(|i| self.movies.get(i)).cloned() else {
            ui.label(RichText::new(tr("movies.select")).weak());
            return;
        };
        ui.strong(&movie.rel_path);
        self.info(ui);
        ui.separator();
        self.actions(ui, &movie);
    }

    /// The picture and the play bar over the editor window.
    fn full_window(&mut self, ctx: &egui::Context) {
        let Some(player) = &mut self.player else {
            return;
        };
        let mut exit = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        egui::Area::new(egui::Id::unique("movie_full"))
            .order(egui::Order::Foreground)
            .fixed_pos(ctx.content_rect().min)
            .show(ctx, |ui| {
                let rect = ctx.content_rect();
                ui.set_min_size(rect.size());
                ui.painter().rect_filled(rect, 0.0, egui::Color32::BLACK);
                egui::Frame::NONE.inner_margin(6.0).show(ui, |ui| {
                    ui.set_width(rect.width() - 12.0);
                    let size = egui::vec2(rect.width() - 12.0, (rect.height() - 70.0).max(50.0));
                    if player.picture(ui, size) {
                        exit = true;
                    }
                    if player.controls(ui, true) {
                        exit = true;
                    }
                });
            });
        if exit {
            self.full = false;
        }
    }

    /// Reads the selected movie and starts its player.
    fn load_selected(&mut self, ctx: &egui::Context) {
        let Some(movie) = self.selected.and_then(|i| self.movies.get(i)).cloned() else {
            return;
        };
        if self
            .loaded
            .as_ref()
            .is_some_and(|l| l.rel_path == movie.rel_path)
        {
            return;
        }
        let data = Arc::new(std::fs::read(&movie.path).unwrap_or_default());
        let info = video::info(&data).map_err(|e| e.to_string());
        let has_video = info.as_ref().is_ok_and(|i| i.video.is_some());
        if let Some(p) = &self.player {
            self.volume = Some(p.volume());
        }
        self.player = None;
        if has_video {
            let (volume, muted) = self.volume.unwrap_or((1.0, false));
            let mut player = Player::new(
                ctx,
                &movie.rel_path,
                data.clone(),
                std::mem::take(&mut self.start_frame),
                volume,
                muted,
            );
            if std::mem::take(&mut self.autoplay) {
                player.play(ctx);
            }
            self.player = Some(player);
        }
        self.loaded = Some(Loaded {
            rel_path: movie.rel_path.clone(),
            data,
            info,
        });
    }

    fn select(&mut self, index: usize) {
        if self.selected != Some(index) {
            self.selected = Some(index);
            self.start_frame = 0;
        }
    }

    fn list(&mut self, ui: &mut egui::Ui) {
        if self.movies.is_empty() {
            ui.label(RichText::new(tr("movies.none")).weak());
            return;
        }
        let mut clicked = None;
        let mut double = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (i, m) in self.movies.iter().enumerate() {
                let name = m.rel_path.trim_start_matches("movies/");
                let text = trf(
                    "movies.entry",
                    &[
                        ("name", &name),
                        ("size", &format!("{:.1}", m.len as f64 / 1048576.0)),
                    ],
                );
                let r = ui
                    .selectable_label(self.selected == Some(i), text)
                    .on_hover_text(tr("movies.entry_hint"));
                if r.double_clicked() {
                    double = Some(i);
                } else if r.clicked() {
                    clicked = Some(i);
                }
            }
        });
        if let Some(i) = clicked {
            self.select(i);
        }
        if let Some(i) = double {
            self.select(i);
            if let Some(p) = &mut self.player
                && self.loaded.as_ref().map(|l| &l.rel_path) == Some(&p.rel_path)
                && self.movies.get(i).map(|m| &m.rel_path) == Some(&p.rel_path)
            {
                p.play(ui.ctx());
            } else {
                self.autoplay = true;
            }
        }
    }

    fn picture_ui(&mut self, ui: &mut egui::Ui, details: bool) {
        let Some(movie) = self.selected.and_then(|i| self.movies.get(i)).cloned() else {
            ui.label(RichText::new(tr("movies.select")).weak());
            return;
        };
        if self.loaded.is_none() {
            return;
        }
        ui.heading(&movie.rel_path);
        if !details {
            self.info(ui);
            // Export and import below, the player fills the rest.
            egui::Panel::bottom(ui.scope_id().with("movie_actions"))
                .show(ui, |ui| self.actions(ui, &movie));
        }
        if let Some(player) = &mut self.player {
            let controls = 64.0;
            let size = egui::vec2(
                ui.available_width(),
                (ui.available_height() - controls).max(120.0),
            );
            if player.picture(ui, size) {
                self.full = true;
            }
            if player.controls(ui, false) {
                self.full = true;
            }
        }
    }

    /// Video and audio of the selected movie.
    fn info(&self, ui: &mut egui::Ui) {
        let Some(loaded) = self.loaded.as_ref() else {
            return;
        };
        match &loaded.info {
            Ok(info) => {
                if let Some(v) = &info.video {
                    ui.label(trf(
                        "movies.video_info",
                        &[
                            ("width", &v.header.width),
                            ("height", &v.header.height),
                            ("frames", &trn("count.frames", v.frames as u64, &[])),
                            ("keys", &v.key_frames),
                            ("fps", &format!("{:.3}", v.fps)),
                            ("seconds", &format!("{:.1}", v.duration)),
                        ],
                    ));
                }
                match &info.audio {
                    Some(a) => ui.label(trf(
                        "movies.audio_info",
                        &[
                            ("codec", &a.codec),
                            (
                                "channels",
                                &trn("sound.channels", a.header.channels as u64, &[]),
                            ),
                            ("rate", &a.header.sample_rate),
                            ("seconds", &format!("{:.1}", a.duration)),
                        ],
                    )),
                    None => ui.label(tr("movies.audio_none")),
                };
            }
            Err(e) => {
                ui.colored_label(egui::Color32::LIGHT_RED, e);
            }
        }
    }

    fn actions(&mut self, ui: &mut egui::Ui, movie: &MovieFile) {
        ui.add_space(4.0);
        let busy = self.job.is_some();
        ui.horizontal(|ui| {
            if ui.button(tr("movies.export_audio")).clicked() {
                self.message = self.export_audio(movie);
            }
            if ui
                .add_enabled(
                    self.player.is_some(),
                    egui::Button::new(tr("movies.export_frame")),
                )
                .on_hover_text(tr("movies.export_frame_hint"))
                .clicked()
            {
                self.message = self.export_frame(movie);
            }
        });
        ui.horizontal(|ui| {
            ui.label(tr("movies.new_audio"));
            ui.radio_value(&mut self.audio, AudioChoice::Keep, tr("movies.audio_keep"));
            ui.radio_value(&mut self.audio, AudioChoice::None, tr("movies.audio_drop"));
            ui.radio_value(&mut self.audio, AudioChoice::File, tr("movies.audio_file"));
            ui.add(egui::Slider::new(&mut self.quantizer, 20..=63).text(tr("movies.quality")));
        });
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!busy, |ui| {
                if ui
                    .button(tr("movies.import_video"))
                    .on_hover_text(tr("movies.import_video_hint"))
                    .clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .set_title(tr("movies.new_video_title"))
                        .pick_file()
                {
                    self.start_import(movie, Some(path));
                }
                if ui
                    .button(tr("movies.replace_audio"))
                    .on_hover_text(tr("movies.replace_audio_hint"))
                    .clicked()
                {
                    self.audio = AudioChoice::File;
                    self.start_import(movie, None);
                }
            });
            if busy {
                ui.spinner();
                ui.label(tr("movies.encoding"));
            }
        });
        if !self.message.is_empty() {
            ui.label(&self.message);
        }
    }

    fn export_audio(&self, movie: &MovieFile) -> String {
        let Some(loaded) = &self.loaded else {
            return String::new();
        };
        let pcm = match video::decode_audio(&loaded.data) {
            Ok(Some(p)) => p,
            Ok(None) => return tr("movies.no_audio_track").to_owned(),
            Err(e) => return trf("movies.audio_failed", &[("error", &e)]),
        };
        let stem = file_stem(movie);
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("{stem}.wav"))
            .save_file()
        else {
            return String::new();
        };
        match std::fs::write(&path, wav(&pcm.samples, pcm.channels, pcm.sample_rate)) {
            Ok(()) => trf("movies.saved", &[("path", &path.display())]),
            Err(e) => trf("movies.save_failed", &[("error", &e)]),
        }
    }

    /// The frame at the play head as PNG.
    fn export_frame(&self, movie: &MovieFile) -> String {
        let Some(player) = &self.player else {
            return String::new();
        };
        let frame = player.frame;
        let png = MovieFrames::new(player.data().as_slice())
            .and_then(|mut f| f.display(frame))
            .and_then(|pic| pic.to_png());
        let png = match png {
            Ok(p) => p,
            Err(e) => return trf("movies.frame_failed", &[("error", &e)]),
        };
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("{}_{frame:05}.png", file_stem(movie)))
            .save_file()
        else {
            return String::new();
        };
        match std::fs::write(&path, png) {
            Ok(()) => trf("movies.saved", &[("path", &path.display())]),
            Err(e) => trf("movies.save_failed", &[("error", &e)]),
        }
    }

    fn start_import(&mut self, movie: &MovieFile, video_path: Option<PathBuf>) {
        let audio = match self.audio {
            AudioChoice::Keep => AudioSource::Keep,
            AudioChoice::None => AudioSource::None,
            AudioChoice::File => match rfd::FileDialog::new()
                .set_title(tr("movies.new_audio_title"))
                .pick_file()
            {
                Some(p) => AudioSource::File(p),
                None => return,
            },
        };
        let Some(loaded) = &self.loaded else { return };
        let original = loaded.data.as_ref().clone();
        let rel_path = movie.rel_path.clone();
        let quantizer = self.quantizer;
        let (tx, rx) = channel();
        self.job = Some(rx);
        self.message.clear();
        std::thread::spawn(move || {
            let run = || -> Result<MovieImport, String> {
                let mut opts = BuildOptions {
                    encoder: vp6::EncoderConfig {
                        quantizer,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let imported =
                    import::build_from_files(video_path.as_deref(), &audio, &original, &mut opts)
                        .map_err(|e| e.to_string())?;
                let mut message = trf(
                    "movies.built",
                    &[
                        ("path", &rel_path),
                        (
                            "size",
                            &format!("{:.1}", imported.movie.len() as f64 / 1048576.0),
                        ),
                    ],
                );
                for w in imported.warnings {
                    message.push('\n');
                    message.push_str(&trf("movies.warning", &[("warning", &w)]));
                }
                Ok(MovieImport {
                    label: trf("movies.label", &[("path", &rel_path)]),
                    rel_path,
                    data: imported.movie,
                    message,
                })
            };
            let _ = tx.send(run());
        });
    }
}

fn file_stem(movie: &MovieFile) -> String {
    Path::new(&movie.rel_path)
        .file_stem()
        .map_or("movie".into(), |s| s.to_string_lossy().into_owned())
}
