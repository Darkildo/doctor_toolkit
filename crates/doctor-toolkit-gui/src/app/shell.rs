//! The shell around the tabs: notifications (status bar, pop-ups, the Log
//! panel and the log file), the status bar with every running job, the
//! welcome screen, preferences, the keyboard cheat sheet, About and help.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime};

use egui::{Color32, RichText};

use doctor_toolkit_mods::i18n::{language_choice, tr, trf};

use super::commands::{COMMANDS, Menu, Scope};
use super::dock::{DockOp, Pane};
use super::{BrowserApp, Command};
use crate::settings::{Renderer, Theme};

/// Entries the Log panel keeps.
const LOG_LIMIT: usize = 1000;
/// How long a pop-up stays.
const TOAST_TIME: Duration = Duration::from_secs(8);
/// Pop-ups shown at once.
const TOAST_LIMIT: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Info,
    Warning,
    Error,
}

impl Level {
    /// The level of a message from its wording, for messages set as plain
    /// status text by the views and older code; knows the English and the
    /// Russian wording.
    pub fn of(text: &str) -> Self {
        let lower = text.to_lowercase();
        let starts = |p: &str| lower.starts_with(p);
        if starts("cannot")
            || starts("error")
            || lower.contains(" failed")
            || lower.contains("internal error")
            || starts("не удалось")
            || starts("ошибка")
            || lower.contains("внутренняя ошибка")
        {
            Self::Error
        } else if starts("warning")
            || lower.contains("; warning")
            || lower.contains("not found")
            || starts("предупреждение")
            || lower.contains("; предупреждение")
            || lower.contains("не найден")
        {
            Self::Warning
        } else {
            Self::Info
        }
    }

    fn color(self, visuals: &egui::Visuals) -> Color32 {
        match self {
            Self::Info => visuals.text_color(),
            Self::Warning => doctor_toolkit_mods::WARN,
            Self::Error => doctor_toolkit_mods::BAD,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Info => tr("log.info"),
            Self::Warning => tr("log.warning"),
            Self::Error => tr("log.error"),
        }
    }
}

pub struct Entry {
    pub time: SystemTime,
    pub level: Level,
    pub text: String,
}

struct Toast {
    level: Level,
    text: String,
    shown: Instant,
}

/// Messages of this run.
#[derive(Default)]
pub struct Notes {
    pub entries: VecDeque<Entry>,
    toasts: Vec<Toast>,
    /// Status texts already recorded, to notice new ones.
    last_status: String,
    last_nav: Option<String>,
    /// Warnings and errors since the Log panel was last looked at.
    unseen: usize,
    filter: String,
    pub show_info: bool,
    /// Write warnings and errors to the log file (not in tests).
    pub log_to_file: bool,
}

impl Notes {
    /// Notes of the editor: everything shown, problems in the log file.
    pub fn for_app() -> Self {
        Self {
            log_to_file: true,
            show_info: true,
            ..Self::default()
        }
    }

    pub fn push(&mut self, level: Level, text: &str) {
        if text.is_empty() {
            return;
        }
        self.entries.push_back(Entry {
            time: SystemTime::now(),
            level,
            text: text.to_owned(),
        });
        while self.entries.len() > LOG_LIMIT {
            self.entries.pop_front();
        }
        if level >= Level::Warning {
            self.unseen += 1;
            self.toasts.push(Toast {
                level,
                text: text.to_owned(),
                shown: Instant::now(),
            });
            let excess = self.toasts.len().saturating_sub(TOAST_LIMIT);
            self.toasts.drain(..excess);
            if self.log_to_file {
                let tag = if level == Level::Error {
                    "error"
                } else {
                    "warning"
                };
                doctor_toolkit::report::log(&format!("{tag}: {text}"));
            }
        }
    }

    /// Marks a status text as recorded.
    pub fn seen_status(&mut self, text: &str) {
        text.clone_into(&mut self.last_status);
    }

    /// Records status texts set since the last frame.
    pub fn track(&mut self, status: &str, nav: Option<&str>) {
        if status != self.last_status {
            self.seen_status(status);
            self.push(Level::of(status), status);
        }
        if nav != self.last_nav.as_deref() {
            self.last_nav = nav.map(str::to_owned);
            if let Some(text) = nav {
                self.push(Level::of(text), text);
            }
        }
    }

    /// Title of the Log tab, with the number of new problems.
    pub fn title(&self) -> String {
        self.title_for(tr("pane.log"))
    }

    /// `label` with the number of new problems.
    pub fn title_for(&self, label: &str) -> String {
        if self.unseen > 0 {
            format!("{label} ({})", self.unseen)
        } else {
            label.to_owned()
        }
    }

    /// Pop-ups for warnings and errors, above the status bar.
    pub fn toasts(&mut self, ctx: &egui::Context) {
        self.toasts.retain(|t| t.shown.elapsed() < TOAST_TIME);
        if self.toasts.is_empty() {
            return;
        }
        let mut dismissed = None;
        egui::Area::new(egui::Id::unique("toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, [-12.0, -36.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_max_width(420.0);
                for (i, toast) in self.toasts.iter().enumerate() {
                    let color = toast.level.color(ui.visuals());
                    let response = egui::Frame::popup(ui.style())
                        .stroke(egui::Stroke::new(1.5, color))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(toast.level.label()).color(color).strong());
                                ui.add(egui::Label::new(&toast.text).wrap());
                            });
                        })
                        .response
                        .interact(egui::Sense::click())
                        .on_hover_text(tr("toast.dismiss"));
                    if response.clicked() {
                        dismissed = Some(i);
                    }
                    ui.add_space(4.0);
                }
            });
        if let Some(i) = dismissed {
            self.toasts.remove(i);
        }
        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

/// Windows of the shell and whether they are open.
#[derive(Default)]
pub struct Windows {
    pub preferences: bool,
    pub about: bool,
    pub shortcuts: bool,
    /// Restore originals waits for confirmation.
    pub confirm_restore: bool,
}

/// `DOCTOR_TOOLKIT_GUI_SHELL=<window>[,…]`: opens shell windows for screenshots;
/// `layout=level|resources|<name>` applies a layout, `drawer=content|log|changes|history`
/// opens a drawer, `layouts` the window of the user's layouts, `content` a
/// Content Browser docked among the documents.
pub fn apply_capture_preset(app: &mut BrowserApp) {
    let Ok(spec) = std::env::var("DOCTOR_TOOLKIT_GUI_SHELL") else {
        return;
    };
    for item in spec.split(',').map(str::trim) {
        let (name, value) = item.split_once('=').unwrap_or((item, ""));
        match name {
            "palette" => {
                app.palette.open(super::palette::Mode::All);
                app.palette.text = value.to_owned();
            }
            "streams" => app.palette.open(super::palette::Mode::Streams),
            "prefs" => app.windows.preferences = true,
            "about" => app.windows.about = true,
            "shortcuts" => app.windows.shortcuts = true,
            "welcome" => app.dock_ops.push(DockOp::ShowPanel(Pane::Welcome)),
            "play" => app.play.options_open = true,
            "log" => app.dock_ops.push(DockOp::ShowPanel(Pane::Log)),
            "index" => app.dock_ops.push(DockOp::ShowPanel(Pane::Index)),
            "pin" => app.pin_on_capture = true,
            "max" => {
                app.pin_on_capture = true;
                app.maximize_pinned = true;
            }
            "layout" => {
                let id = match value {
                    "level" => super::layouts::LayoutId::Builtin(super::layouts::Builtin::Level),
                    "cinematics" => {
                        super::layouts::LayoutId::Builtin(super::layouts::Builtin::Cinematics)
                    }
                    "resources" => {
                        super::layouts::LayoutId::Builtin(super::layouts::Builtin::Resources)
                    }
                    name => super::layouts::LayoutId::User(name.to_owned()),
                };
                app.dock_ops.push(DockOp::ApplyLayout(id));
            }
            "drawer" => {
                let pane = match value {
                    "log" => Pane::Log,
                    "changes" => Pane::Changes,
                    "history" => Pane::History,
                    "timeline" => Pane::Timeline,
                    _ => Pane::ContentBrowser,
                };
                app.drawers.add(pane.clone());
                app.drawers.open = Some(pane);
            }
            "layouts" => app.layout_window.open = true,
            // A Content Browser docked among the documents (stage 55).
            "content" => {
                let id = app.new_pane_id();
                app.dock_ops
                    .push(DockOp::ShowPanel(Pane::ContentExtra { id }));
            }
            "notify" => {
                app.notes
                    .push(Level::Warning, "Example warning: shown as a pop-up");
                app.notes
                    .push(Level::Error, "Cannot write example: an error pop-up");
            }
            _ => eprintln!("DOCTOR_TOOLKIT_GUI_SHELL: unknown item {name}"),
        }
    }
}

/// Opens a file, folder or address with the desktop's default program.
pub fn open_external(target: &std::ffi::OsStr) -> Result<(), String> {
    let mut command = if cfg!(windows) {
        let mut c = std::process::Command::new("explorer");
        c.arg(target);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(target);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(target);
        c
    };
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(drop)
        .map_err(|e| e.to_string())
}

/// The user guide: next to the program (release archive), installed with it,
/// or in the source tree it was built from.
pub fn guide_path() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_owned));
    let mut candidates = Vec::new();
    if let Some(dir) = &exe_dir {
        candidates.push(dir.join("docs/guide/README.md"));
        candidates.push(dir.join("../share/doc/doctor-toolkit/guide/README.md"));
    }
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/guide/README.md"));
    candidates.into_iter().find(|p| p.is_file())
}

/// The guide on the project page, when no copy is on disk.
pub const GUIDE_URL: &str = "https://github.com/Darkildo/doctor_toolkit/tree/main/docs/guide";

impl BrowserApp {
    pub(super) fn open_guide(&mut self) {
        let target = guide_path()
            .map(PathBuf::into_os_string)
            .unwrap_or_else(|| GUIDE_URL.into());
        if let Err(error) = open_external(&target) {
            self.notify(
                Level::Error,
                trf(
                    "status.cannot_open",
                    &[("what", &target.to_string_lossy()), ("error", &error)],
                ),
            );
        }
    }

    pub(super) fn open_log_file(&mut self) {
        let Some(path) = doctor_toolkit::report::log_path() else {
            self.notify(Level::Warning, tr("status.no_log").to_owned());
            return;
        };
        // An empty log may not exist yet: show its folder then.
        let target = if path.is_file() {
            path
        } else {
            path.parent().map(Path::to_owned).unwrap_or(path)
        };
        if let Err(error) = open_external(target.as_os_str()) {
            self.notify(
                Level::Error,
                trf(
                    "status.cannot_open",
                    &[("what", &target.display()), ("error", &error)],
                ),
            );
        }
    }

    // ------------------------------------------------------------ status bar

    /// The drawers (Content Drawer, Output Log), the live link, running
    /// jobs with their progress and cancel buttons and the last message;
    /// on the right the last edit to undo and the edition of the game.
    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            self.drawer_buttons(ui);
            self.link_button(ui);
            ui.separator();
            let mut jobs: Vec<(String, Option<f32>, Option<&std::sync::atomic::AtomicBool>)> =
                Vec::new();
            if let Some(job) = &self.job {
                jobs.push((
                    job.label.clone(),
                    Some(job.progress.fraction()),
                    Some(&job.progress.cancel),
                ));
            }
            for (label, progress, cancellable) in self.nav.jobs() {
                jobs.push((
                    label.to_owned(),
                    Some(progress.fraction()),
                    cancellable.then_some(&progress.cancel),
                ));
            }
            if let Some(label) = self.mods.busy_label().filter(|_| self.mods_open) {
                jobs.push((label.to_owned(), None, None));
            }
            if self.levels.values().any(|l| l.busy()) {
                jobs.push((tr("status.loading_level").to_owned(), None, None));
            }
            for (label, progress) in self.play.jobs() {
                jobs.push((label, progress, None));
            }
            if let Some((label, progress, cancel)) = jobs.first() {
                ui.spinner();
                ui.label(label);
                if let Some(fraction) = progress {
                    ui.add(
                        egui::ProgressBar::new(*fraction)
                            .show_percentage()
                            .desired_width(180.0),
                    );
                }
                if let Some(cancel) = cancel
                    && ui.small_button(tr("status.cancel")).clicked()
                {
                    cancel.store(true, Ordering::Relaxed);
                }
                if jobs.len() > 1 {
                    let more = ui.small_button(format!("+{}", jobs.len() - 1));
                    let popup_id = ui.make_persistent_id("jobs-popup");
                    egui::Popup::from_response(&more)
                        .id(popup_id)
                        .open_memory(more.clicked().then_some(egui::SetOpenCommand::Toggle))
                        .show(|ui| {
                            ui.set_min_width(320.0);
                            for (label, progress, cancel) in &jobs {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(label);
                                    if let Some(fraction) = progress {
                                        ui.add(
                                            egui::ProgressBar::new(*fraction)
                                                .show_percentage()
                                                .desired_width(120.0),
                                        );
                                    }
                                    if let Some(cancel) = cancel
                                        && ui.small_button(tr("status.cancel")).clicked()
                                    {
                                        cancel.store(true, Ordering::Relaxed);
                                    }
                                });
                            }
                        });
                }
                ui.separator();
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                self.edition_label(ui);
                self.undo_button(ui);
                self.external_status(ui);
                if let Some(status) = &self.nav.status {
                    ui.add(egui::Label::new(RichText::new(status).weak()).truncate());
                }
                let level = Level::of(&self.status);
                let text = if self.status.is_empty() && self.game.is_none() {
                    tr("status.start")
                } else {
                    self.status.as_str()
                };
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(text).color(level.color(ui.visuals())))
                            .truncate(),
                    );
                });
            });
        });
    }

    /// The state of the live link; a click shows the game tab.
    fn link_button(&mut self, ui: &mut egui::Ui) {
        let (text, color) = if self.play.live.connected() {
            (tr("status.link_up"), Some(doctor_toolkit_mods::GOOD))
        } else if self.play.launched() {
            (tr("status.link_waiting"), Some(doctor_toolkit_mods::WARN))
        } else {
            (tr("status.link_down"), None)
        };
        let response = super::theme::tool_button(
            ui,
            Some(super::theme::Icon::Link),
            text,
            false,
            super::theme::Tone::Flat,
        )
        .on_hover_text(tr("status.link_hint"));
        if let Some(color) = color {
            let dot = egui::pos2(response.rect.right() - 4.0, response.rect.top() + 5.0);
            ui.painter().circle_filled(dot, 2.5, color);
        }
        if response.clicked() && self.play.game_view.is_running() {
            self.show_game_tab();
        }
    }

    /// "Undo: <the last edit>" (Ctrl+Z).
    fn undo_button(&mut self, ui: &mut egui::Ui) {
        let label = self.workspace.as_ref().and_then(|ws| ws.undo_label());
        match label {
            Some(label) => {
                // Long labels (wizards) are cut: the status bar is one line.
                let short: String = if label.chars().count() > 32 {
                    label.chars().take(31).chain(['…']).collect()
                } else {
                    label.to_owned()
                };
                let text = trf("status.undo", &[("label", &short)]);
                let hint = format!(
                    "{} ({})",
                    Command::Undo.label(),
                    Command::Undo.shortcut_text(ui.ctx()).unwrap_or_default()
                );
                if super::theme::tool_button(
                    ui,
                    Some(super::theme::Icon::Undo),
                    &text,
                    false,
                    super::theme::Tone::Flat,
                )
                .on_hover_text(format!("{label}\n{hint}"))
                .clicked()
                {
                    self.pending.push(Command::Undo);
                }
            }
            None if self.game.is_some() => {
                ui.label(RichText::new(tr("status.nothing_to_undo")).weak());
            }
            None => {}
        }
    }

    /// The edition of the game, short; the whole summary and the folder as
    /// the hint.
    fn edition_label(&mut self, ui: &mut egui::Ui) {
        let Some(game) = self.game.clone() else {
            return;
        };
        let edition = self.edition();
        let short = edition.map_or_else(|| game.profile().name.to_owned(), |e| e.name());
        let hint = format!(
            "{}\n{}",
            edition.map_or_else(|| game.profile().name.to_owned(), |e| e.summary()),
            game.root().display()
        );
        ui.label(RichText::new(short).weak()).on_hover_text(hint);
    }

    // -------------------------------------------------------------- log panel

    pub(super) fn log_ui(&mut self, ui: &mut egui::Ui) {
        self.notes.unseen = 0;
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.notes.filter)
                    .hint_text(tr("log.filter"))
                    .desired_width(200.0),
            );
            ui.checkbox(&mut self.notes.show_info, tr("log.show_info"));
            if ui.button(tr("log.clear")).clicked() {
                self.notes.entries.clear();
            }
            if ui
                .button(tr("cmd.show_log"))
                .on_hover_text(
                    doctor_toolkit::report::log_path()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                )
                .clicked()
            {
                self.pending.push(Command::ShowLog);
            }
        });
        let filter = self.notes.filter.to_lowercase();
        let show_info = self.notes.show_info;
        let rows: Vec<&Entry> = self
            .notes
            .entries
            .iter()
            .rev()
            .filter(|e| show_info || e.level > Level::Info)
            .filter(|e| filter.is_empty() || e.text.to_lowercase().contains(&filter))
            .collect();
        if rows.is_empty() {
            ui.label(RichText::new(tr("log.empty")).weak());
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("log-entries")
            .auto_shrink(false)
            .show_rows(ui, 18.0, rows.len(), |ui, range| {
                for entry in &rows[range] {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(clock(entry.time)).weak().monospace());
                        ui.label(
                            RichText::new(entry.level.label())
                                .color(entry.level.color(ui.visuals())),
                        );
                        ui.add(egui::Label::new(&entry.text).truncate())
                            .on_hover_text(&entry.text);
                    });
                }
            });
    }

    // ---------------------------------------------------------------- welcome

    pub(super) fn welcome_ui(&mut self, ui: &mut egui::Ui) {
        let mut open = None;
        egui::ScrollArea::vertical()
            .id_salt("welcome")
            .auto_shrink(false)
            .show(ui, |ui| {
                egui::Frame::NONE.inner_margin(16).show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(24.0);
                        let icon = self.icon_texture(ui.ctx());
                        ui.add(egui::Image::new(&icon).fit_to_exact_size(egui::vec2(96.0, 96.0)));
                        ui.heading(RichText::new("doctor_toolkit").size(28.0));
                        ui.label(RichText::new(tr("welcome.tagline")).weak());
                        ui.label(
                            RichText::new(doctor_toolkit::report::version_line(
                                "doctor-toolkit-gui",
                            ))
                            .weak()
                            .small(),
                        );
                        ui.add_space(16.0);
                    });
                    ui.columns(2, |cols| {
                        let ui = &mut cols[0];
                        ui.heading(tr("welcome.start"));
                        if let Some(game) = self.game.clone() {
                            ui.label(trf("welcome.game_open", &[("name", &game.profile().name)]));
                            ui.label(
                                RichText::new(game.root().display().to_string())
                                    .weak()
                                    .small(),
                            );
                            for command in [
                                Command::OpenStream,
                                Command::Palette,
                                Command::NewSearch,
                                Command::AllResources,
                                Command::LevelEditor,
                                Command::Mods,
                            ] {
                                self.welcome_button(ui, command);
                            }
                            ui.add_space(12.0);
                            ui.heading(tr("wizard.start_title"));
                            self.task_buttons(ui, false);
                        } else {
                            self.welcome_button(ui, Command::OpenGame);
                            ui.label(RichText::new(tr("welcome.open_hint")).weak());
                        }
                        if let Some(dir) = self.welcome_detected(ui) {
                            open = Some(dir);
                        }
                        ui.add_space(12.0);
                        ui.heading(tr("welcome.help"));
                        self.welcome_button(ui, Command::UserGuide);
                        self.welcome_button(ui, Command::Shortcuts);
                        self.welcome_button(ui, Command::Preferences);
                        let ui = &mut cols[1];
                        ui.heading(tr("menu.recent"));
                        if self.settings.recent_games.is_empty() {
                            ui.label(RichText::new(tr("welcome.no_recent")).weak());
                        }
                        let mut forget = None;
                        for (i, dir) in self.settings.recent_games.iter().enumerate() {
                            ui.horizontal(|ui| {
                                let exists = dir.is_dir();
                                let name = dir.file_name().map_or_else(
                                    || dir.display().to_string(),
                                    |n| n.to_string_lossy().into_owned(),
                                );
                                let response = ui
                                    .add_enabled(exists, egui::Link::new(name))
                                    .on_hover_text(dir.display().to_string())
                                    .on_disabled_hover_text(tr("welcome.missing"));
                                if response.clicked() {
                                    open = Some(dir.clone());
                                }
                                if ui
                                    .small_button("×")
                                    .on_hover_text(tr("welcome.forget"))
                                    .clicked()
                                {
                                    forget = Some(i);
                                }
                            });
                            if let Some(parent) = dir.parent() {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(parent.display().to_string()).weak().small(),
                                    )
                                    .truncate(),
                                );
                            }
                        }
                        if let Some(i) = forget {
                            self.settings.recent_games.remove(i);
                        }
                    });
                });
            });
        if let Some(dir) = open {
            self.open_game(dir, ui.ctx());
        }
    }

    fn welcome_button(&mut self, ui: &mut egui::Ui, command: Command) {
        let mut button = egui::Button::new(command.label()).min_size(egui::vec2(220.0, 0.0));
        if let Some(text) = command.shortcut_text(ui.ctx()) {
            button = button.shortcut_text(text);
        }
        if ui.add_enabled(self.enabled(command), button).clicked() {
            self.pending.push(command);
        }
    }

    /// The program icon as a texture, made once.
    fn icon_texture(&mut self, ctx: &egui::Context) -> egui::TextureHandle {
        self.icon
            .get_or_insert_with(|| {
                use doctor_toolkit_mods::icon::{Variant, rgba};
                let size = 128;
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [size, size],
                    &rgba(Variant::Editor, size as u32),
                );
                ctx.load_texture("app-icon", image, egui::TextureOptions::LINEAR)
            })
            .clone()
    }

    // ---------------------------------------------------------------- windows

    pub(super) fn show_windows(&mut self, ctx: &egui::Context) {
        self.preferences_window(ctx);
        self.about_window(ctx);
        self.shortcuts_window(ctx);
        self.confirm_restore_window(ctx);
    }

    fn preferences_window(&mut self, ctx: &egui::Context) {
        let mut open = self.windows.preferences;
        egui::Window::new(tr("prefs.title"))
            .open(&mut open)
            .default_width(520.0)
            .resizable(true)
            .show(ctx, |ui| {
                let s = &mut self.settings;
                egui::Grid::new("prefs")
                    .num_columns(2)
                    .spacing([16.0, 8.0])
                    .show(ui, |ui| {
                        ui.label(tr("prefs.theme"));
                        ui.horizontal(|ui| {
                            for (theme, key) in [
                                (Theme::Dark, "prefs.dark"),
                                (Theme::Light, "prefs.light"),
                                (Theme::System, "prefs.system"),
                            ] {
                                if ui.radio_value(&mut s.theme, theme, tr(key)).changed() {
                                    ctx.set_theme(theme.preference());
                                }
                            }
                        });
                        ui.end_row();
                        ui.label(tr("prefs.scale"));
                        let mut scale = ctx.zoom_factor();
                        if ui
                            .add(egui::Slider::new(&mut scale, 0.5..=2.5).step_by(0.05))
                            .on_hover_text(tr("prefs.scale_hint"))
                            .changed()
                        {
                            ctx.set_zoom_factor(scale);
                        }
                        ui.end_row();
                        ui.label(tr("prefs.language"));
                        language_choice(ui, &mut s.language);
                        ui.end_row();
                        ui.label(tr("prefs.start"));
                        ui.checkbox(&mut s.reopen_last, tr("prefs.reopen_last"));
                        ui.end_row();
                        ui.label(tr("prefs.renderer"));
                        ui.horizontal(|ui| {
                            for (renderer, key) in [
                                (Renderer::Auto, "prefs.renderer_auto"),
                                (Renderer::Wgpu, "prefs.renderer_wgpu"),
                                (Renderer::Glow, "prefs.renderer_glow"),
                            ] {
                                ui.radio_value(&mut s.renderer, renderer, tr(key));
                            }
                        })
                        .response
                        .on_hover_text(tr("prefs.renderer_hint"));
                        ui.end_row();
                        ui.label(tr("prefs.level"));
                        ui.checkbox(&mut s.level.classic_camera, tr("prefs.classic_camera"))
                            .on_hover_text(tr("prefs.classic_camera_hint"));
                        ui.end_row();
                    });
                ui.separator();
                ui.strong(tr("prefs.editors"));
                ui.label(RichText::new(tr("prefs.editors_hint")).weak());
                egui::Grid::new("prefs-editors")
                    .num_columns(2)
                    .show(ui, |ui| {
                        let editors = &mut s.external_editors;
                        for (key, value) in [
                            ("prefs.editor_image", &mut editors.image),
                            ("prefs.editor_model", &mut editors.model),
                            ("prefs.editor_text", &mut editors.text),
                            ("prefs.editor_audio", &mut editors.audio),
                        ] {
                            ui.label(tr(key));
                            ui.add(
                                egui::TextEdit::singleline(value)
                                    .hint_text(tr("prefs.editor_default"))
                                    .desired_width(300.0),
                            );
                            ui.end_row();
                        }
                    });
                ui.separator();
                ui.strong(tr("prefs.folders"));
                egui::Grid::new("prefs-folders")
                    .num_columns(3)
                    .show(ui, |ui| {
                        let dirs = &mut s.export_dirs;
                        for (key, value) in [
                            ("prefs.dir_export", &mut dirs.export),
                            ("prefs.dir_extract", &mut dirs.extract),
                            ("prefs.dir_pack", &mut dirs.pack),
                            ("prefs.dir_import", &mut dirs.import),
                        ] {
                            ui.label(tr(key));
                            ui.label(
                                value
                                    .as_ref()
                                    .map_or_else(|| "—".to_owned(), |p| p.display().to_string()),
                            );
                            ui.horizontal(|ui| {
                                if ui.small_button(tr("prefs.choose")).clicked()
                                    && let Some(dir) = rfd::FileDialog::new().pick_folder()
                                {
                                    *value = Some(dir);
                                }
                                if value.is_some() && ui.small_button(tr("prefs.forget")).clicked()
                                {
                                    *value = None;
                                }
                            });
                            ui.end_row();
                        }
                    });
                ui.separator();
                ui.horizontal(|ui| match self.store.settings_path() {
                    Some(path) => {
                        ui.label(RichText::new(path.display().to_string()).weak().small());
                        if ui.small_button(tr("prefs.open_folder")).clicked()
                            && let Some(dir) = path.parent()
                        {
                            let _ = std::fs::create_dir_all(dir);
                            let _ = open_external(dir.as_os_str());
                        }
                    }
                    None => {
                        ui.label(RichText::new(tr("prefs.not_saved")).weak().small());
                    }
                });
            });
        if self.windows.preferences && !open {
            self.nav.export_dir = self.settings.export_dirs.export.clone();
            self.save_settings();
        }
        self.windows.preferences = open;
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        let mut open = self.windows.about;
        egui::Window::new(tr("about.title"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let icon = self.icon_texture(ui.ctx());
                    ui.add(egui::Image::new(&icon).fit_to_exact_size(egui::vec2(64.0, 64.0)));
                    ui.vertical(|ui| {
                        ui.heading("doctor_toolkit");
                        ui.label(tr("about.text"));
                    });
                });
                ui.separator();
                egui::Grid::new("about").num_columns(2).show(ui, |ui| {
                    let rows = [
                        (
                            "about.version",
                            doctor_toolkit::report::version_line("doctor-toolkit-gui"),
                        ),
                        ("about.commit", crate::build_info::COMMIT.to_owned()),
                        ("about.built", crate::build_info::summary()),
                        (
                            "about.edition",
                            self.edition_summary().unwrap_or_else(|| "—".to_owned()),
                        ),
                        ("about.license", env!("CARGO_PKG_LICENSE").to_owned()),
                        ("about.repository", env!("CARGO_PKG_REPOSITORY").to_owned()),
                        (
                            "about.renderer",
                            if self.gpu.is_some() {
                                "wgpu"
                            } else {
                                "OpenGL (glow)"
                            }
                            .to_owned(),
                        ),
                        (
                            "about.log",
                            doctor_toolkit::report::log_path()
                                .map_or_else(|| "—".to_owned(), |p| p.display().to_string()),
                        ),
                        (
                            "about.settings",
                            self.store.settings_path().map_or_else(
                                || tr("prefs.not_saved").to_owned(),
                                |p| p.display().to_string(),
                            ),
                        ),
                    ];
                    for (key, value) in rows {
                        ui.label(tr(key));
                        ui.add(egui::Label::new(RichText::new(value).monospace()).wrap());
                        ui.end_row();
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button(tr("cmd.guide")).clicked() {
                        self.pending.push(Command::UserGuide);
                    }
                    if ui.button(tr("cmd.show_log")).clicked() {
                        self.pending.push(Command::ShowLog);
                    }
                    if ui.button(tr("about.copy")).clicked() {
                        ui.ctx().copy_text(format!(
                            "{} commit {} ({})",
                            doctor_toolkit::report::version_line("doctor-toolkit-gui"),
                            crate::build_info::COMMIT,
                            crate::build_info::summary()
                        ));
                    }
                });
            });
        self.windows.about = open;
    }

    fn shortcuts_window(&mut self, ctx: &egui::Context) {
        let mut open = self.windows.shortcuts;
        egui::Window::new(tr("shortcuts.title"))
            .open(&mut open)
            .default_size([560.0, 560.0])
            .resizable(true)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for menu in Menu::ALL {
                        let specs: Vec<_> = COMMANDS
                            .iter()
                            .filter(|s| s.menu == menu && !s.shortcuts.is_empty())
                            .collect();
                        if specs.is_empty() {
                            continue;
                        }
                        ui.strong(tr(menu.key()));
                        egui::Grid::new(("shortcuts", menu.key()))
                            .num_columns(2)
                            .striped(true)
                            .min_col_width(220.0)
                            .show(ui, |ui| {
                                for spec in specs {
                                    ui.label(tr(spec.key));
                                    let keys: Vec<String> = spec
                                        .shortcuts
                                        .iter()
                                        .map(|s| ctx.format_shortcut(s))
                                        .collect();
                                    let mut text = keys.join(", ");
                                    if spec.scope == Scope::NotInText {
                                        text += &format!("  ({})", tr("shortcuts.not_in_text"));
                                    }
                                    ui.monospace(text);
                                    ui.end_row();
                                }
                            });
                        ui.add_space(6.0);
                    }
                    ui.strong(tr("shortcuts.elsewhere"));
                    egui::Grid::new("shortcuts-other")
                        .num_columns(2)
                        .striped(true)
                        .min_col_width(220.0)
                        .show(ui, |ui| {
                            for (key, keys) in [
                                ("shortcuts.cycle_tabs", "Ctrl+Tab, Ctrl+Shift+Tab"),
                                ("shortcuts.close_tab_mouse", tr("shortcuts.keys_close_tab")),
                                ("shortcuts.maximize_tab", tr("shortcuts.keys_maximize_tab")),
                                ("shortcuts.multi_select", tr("shortcuts.keys_multi_select")),
                                ("shortcuts.open_tab", tr("shortcuts.keys_open_tab")),
                                ("shortcuts.palette_keys", tr("shortcuts.keys_palette")),
                                ("shortcuts.model", tr("shortcuts.keys_model")),
                                ("shortcuts.level", tr("shortcuts.keys_level")),
                                ("shortcuts.level_keys", tr("shortcuts.keys_level_keys")),
                            ] {
                                ui.label(tr(key));
                                ui.monospace(keys);
                                ui.end_row();
                            }
                        });
                    // Stage 56: the keys every asset editor shares, then
                    // each editor's own (the "?" of their bars).
                    ui.add_space(6.0);
                    ui.strong(tr("shortcuts.editors"));
                    super::theme::keys_grid(ui, super::asset_tab::COMMON_KEYS);
                    for (title, keys) in [
                        ("shortcuts.editor_model", crate::model_view::KEYS),
                        ("shortcuts.editor_logic", crate::logic_editor::KEYS),
                        ("shortcuts.editor_anim_graph", crate::anim_graph_view::KEYS),
                        ("shortcuts.editor_script", crate::script_editor::KEYS),
                    ] {
                        ui.add_space(4.0);
                        ui.label(RichText::new(tr(title)).weak());
                        super::theme::keys_grid(ui, keys);
                    }
                });
            });
        self.windows.shortcuts = open;
    }

    fn confirm_restore_window(&mut self, ctx: &egui::Context) {
        if !self.windows.confirm_restore {
            return;
        }
        let mut close = false;
        egui::Modal::new(egui::Id::unique("confirm-restore")).show(ctx, |ui| {
            ui.set_max_width(440.0);
            ui.heading(tr("restore.title"));
            ui.label(tr("restore.hint"));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(tr("restore.confirm")).clicked() {
                    close = true;
                    self.restore_originals();
                }
                if ui.button(tr("dialog.cancel")).clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.windows.confirm_restore = false;
        }
    }
}

/// `HH:MM:SS` of a time, in UTC (the log file uses UTC too).
fn clock(time: SystemTime) -> String {
    let secs = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600 % 24,
        secs / 60 % 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_from_wording() {
        assert_eq!(Level::of("Cannot apply: disk full"), Level::Error);
        assert_eq!(Level::of("Export failed: permission denied"), Level::Error);
        assert_eq!(
            Level::of("Applied: 2 stream(s) written; warning: x"),
            Level::Warning
        );
        assert_eq!(Level::of("Stream `x` not found"), Level::Warning);
        assert_eq!(Level::of("Recorded: move object"), Level::Info);
        assert_eq!(Level::of("Не удалось применить: диск полон"), Level::Error);
        assert_eq!(
            Level::of("Применено: записано 2 потока; предупреждение: x"),
            Level::Warning
        );
        assert_eq!(Level::of("Поток `x` не найден"), Level::Warning);
        assert_eq!(Level::of("Записано: перемещение"), Level::Info);
    }

    #[test]
    fn notes_keep_problems_and_pop_them_up() {
        let mut notes = Notes::default();
        notes.track("Opened game", None);
        notes.track("Opened game", None);
        notes.track("Cannot import: bad file", Some("Game index: 10 resources"));
        assert_eq!(notes.entries.len(), 3);
        assert_eq!(notes.unseen, 1);
        assert_eq!(notes.toasts.len(), 1);
        assert!(notes.title().contains('1'));
        for i in 0..(LOG_LIMIT + 10) {
            notes.push(Level::Info, &format!("message {i}"));
        }
        assert_eq!(notes.entries.len(), LOG_LIMIT);
    }

    #[test]
    fn clock_is_utc_time_of_day() {
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(86_400 * 3 + 3_723);
        assert_eq!(clock(t), "01:02:03");
    }
}
