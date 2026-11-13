//! The project browser (stage 36, `docs/project.md`): the entries of a
//! project folder by folder and type with their state (unchanged, modified,
//! built, out of sync), search, a texture preview, "Open in external
//! editor", "Build" into the workspace and "Build mod".

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use doctor_toolkit::navigation::unpack;
use doctor_toolkit::project::config::BuildSettings;
use doctor_toolkit::project::{
    self, BuildOptions, CreateOptions, EntryState, EntryStatus, Kind, Project, ProjectStatus,
};
use doctor_toolkit::workspace::{ResourceKey, Workspace};
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{Color32, RichText};

use super::BrowserApp;
use super::dock::{DockOp, Pane};
use super::shell::Level;

/// The files are checked for changes this often while the browser is shown.
const CHECK_EVERY: Duration = Duration::from_secs(2);

/// State of the project browser.
#[derive(Default)]
pub(crate) struct ProjectPanel {
    project: Option<Project>,
    status: Option<ProjectStatus>,
    /// When the status was computed, and for which workspace state.
    checked: Option<(Instant, (usize, usize))>,
    filter: String,
    only_changed: bool,
    selected: Option<String>,
    /// The texture preview of the selected entry.
    preview: Option<(String, Option<egui::TextureHandle>)>,
    /// "New project" form.
    new_streams: String,
    new_from_workspace: bool,
    /// Last build, one line.
    last_build: Option<String>,
}

impl ProjectPanel {
    fn stale(&self, ws: (usize, usize)) -> bool {
        self.checked
            .is_none_or(|(at, seen)| seen != ws || at.elapsed() >= CHECK_EVERY)
    }
}

/// Colour of a state.
fn state_color(state: &EntryState) -> Color32 {
    match state {
        EntryState::Unchanged => Color32::GRAY,
        EntryState::Built => Color32::from_rgb(80, 170, 90),
        EntryState::Modified => Color32::from_rgb(220, 160, 40),
        EntryState::OutOfSync => Color32::from_rgb(90, 140, 230),
        EntryState::Missing(_) | EntryState::ViewChanged => Color32::from_rgb(220, 70, 70),
    }
}

fn state_text(state: &EntryState) -> &'static str {
    match state {
        EntryState::Unchanged => tr("project.state_unchanged"),
        EntryState::Built => tr("project.state_built"),
        EntryState::Modified => tr("project.state_modified"),
        EntryState::OutOfSync => tr("project.state_out_of_sync"),
        EntryState::Missing(_) => tr("project.state_missing"),
        EntryState::ViewChanged => tr("project.state_view_changed"),
    }
}

impl BrowserApp {
    /// Shows the project browser.
    pub(super) fn open_project_tab(&mut self) {
        self.dock_ops.push(DockOp::ShowPanel(Pane::Project));
    }

    /// The workspace state the status depends on.
    fn project_ws_mark(&self) -> (usize, usize) {
        self.workspace
            .as_ref()
            .map_or((0, 0), |ws| (ws.head(), ws.len()))
    }

    fn project_refresh(&mut self) {
        let mark = self.project_ws_mark();
        let Some(project) = &self.project.project else {
            return;
        };
        let fresh;
        let ws = match &self.workspace {
            Some(ws) => ws,
            None => match self.game.as_ref().map(|g| Workspace::for_game(g, None)) {
                Some(Ok(ws)) => {
                    fresh = ws;
                    &fresh
                }
                _ => return,
            },
        };
        match project.status(ws) {
            Ok(status) => self.project.status = Some(status),
            Err(error) => {
                self.project.status = None;
                self.notify(
                    Level::Error,
                    trf("project.cannot_read", &[("error", &error)]),
                );
            }
        }
        self.project.checked = Some((Instant::now(), mark));
    }

    pub(super) fn project_open(&mut self, dir: PathBuf) {
        match Project::open(&dir) {
            Ok(project) => {
                if let Some(game) = &self.game {
                    let same = project.config.game.path.as_ref().is_some_and(|p| {
                        std::fs::canonicalize(p).ok() == std::fs::canonicalize(game.root()).ok()
                    });
                    if !same {
                        self.notify(Level::Warning, tr("project.other_game").to_owned());
                    }
                }
                self.status = trf("project.opened", &[("name", &project.config.project.name)]);
                self.project.project = Some(project);
                self.project.selected = None;
                self.project.preview = None;
                self.project.checked = None;
                self.project_refresh();
            }
            Err(error) => self.notify(
                Level::Error,
                trf("project.cannot_open", &[("error", &error)]),
            ),
        }
    }

    fn project_new(&mut self, dir: PathBuf) {
        let Some(game) = self.game.clone() else {
            return;
        };
        let ws = match self.workspace.take() {
            Some(ws) => ws,
            None => match Workspace::for_game(&game, None) {
                Ok(ws) => ws,
                Err(error) => {
                    self.notify(Level::Error, error.to_string());
                    return;
                }
            },
        };
        let streams: Vec<String> = self
            .project
            .new_streams
            .split([',', ' ', '\n'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        let selection = if self.project.new_from_workspace {
            project::workspace_selection(&game, &ws)
        } else {
            unpack::select(&game, &streams, &[])
        };
        let name = dir
            .file_name()
            .map_or_else(|| "Project".into(), |n| n.to_string_lossy().into_owned());
        let result = selection.and_then(|selection| {
            if selection.is_empty() {
                return Err(doctor_toolkit::Error::InvalidArgument(
                    tr("project.nothing").to_owned(),
                ));
            }
            project::create(
                &game,
                &ws,
                self.nav.index.as_deref(),
                &dir,
                &CreateOptions {
                    name,
                    id: None,
                    author: String::new(),
                    selection,
                    settings: BuildSettings::default(),
                },
                &|_, _| {},
                &AtomicBool::new(false),
            )
        });
        self.workspace = Some(ws);
        match result {
            Ok((_, report)) => {
                self.notify(
                    Level::Info,
                    trf(
                        "project.created",
                        &[
                            ("path", &dir.display()),
                            ("files", &report.files),
                            ("resources", &report.resources),
                        ],
                    ),
                );
                for note in report.notes.iter().take(20) {
                    self.notify(Level::Warning, note.clone());
                }
                self.project_open(dir);
            }
            Err(error) => self.notify(
                Level::Error,
                trf("project.cannot_create", &[("error", &error)]),
            ),
        }
    }

    fn project_build(&mut self, force: bool) {
        let (Some(game), Some(project)) = (self.game.clone(), self.project.project.clone()) else {
            return;
        };
        let started = Instant::now();
        let result = self.workspace_mut().and_then(|ws| {
            project
                .build(
                    &game,
                    ws,
                    &BuildOptions {
                        force,
                        ..BuildOptions::default()
                    },
                )
                .map_err(|e| e.to_string())
        });
        match result {
            Ok(report) => {
                for error in &report.errors {
                    self.notify(Level::Error, error.clone());
                }
                for warning in &report.warnings {
                    self.notify(Level::Warning, warning.clone());
                }
                for note in report.built.iter().flat_map(|b| &b.notes) {
                    self.notes.push(Level::Info, note);
                }
                let line = trf(
                    "project.built",
                    &[
                        ("built", &report.built.len()),
                        ("failed", &report.errors.len()),
                        (
                            "seconds",
                            &format!("{:.2}", started.elapsed().as_secs_f64()),
                        ),
                    ],
                );
                self.notify(
                    if report.errors.is_empty() {
                        Level::Info
                    } else {
                        Level::Warning
                    },
                    line.clone(),
                );
                self.project.last_build = Some(line);
                self.after_edit(None);
            }
            Err(error) => self.notify(
                Level::Error,
                trf("project.cannot_build", &[("error", &error)]),
            ),
        }
        self.project.checked = None;
    }

    fn project_mod(&mut self) {
        let Some(project) = self.project.project.clone() else {
            return;
        };
        let Some(out) = rfd::FileDialog::new()
            .set_title(tr("project.mod_dialog"))
            .add_filter("ZIP", &["zip"])
            .set_file_name(format!("{}.zip", project.config.project.id))
            .save_file()
        else {
            return;
        };
        let Some(ws) = &self.workspace else {
            self.notify(Level::Warning, tr("project.build_first").to_owned());
            return;
        };
        match project.build_mod(ws, &out, None) {
            Ok(report) => self.notify(
                Level::Info,
                trf(
                    "project.mod_written",
                    &[("path", &out.display()), ("resources", &report.resources)],
                ),
            ),
            Err(error) => self.notify(
                Level::Error,
                trf("project.cannot_mod", &[("error", &error)]),
            ),
        }
    }

    /// Opens a source file with the program of the settings for its kind.
    fn project_open_external(&mut self, path: PathBuf) {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let editors = &self.settings.external_editors;
        let command = match ext.as_str() {
            "png" | "dds" => editors.image.clone(),
            "glb" | "gltf" | "fbx" | "obj" | "usd" | "usda" | "usdc" | "usdz" => {
                editors.model.clone()
            }
            "json" | "lua" | "toml" | "csv" => editors.text.clone(),
            _ => String::new(),
        };
        match doctor_toolkit::external::launch(&command, &path) {
            Ok(_) => self.status = trf("project.opened_file", &[("path", &path.display())]),
            Err(error) => self.notify(
                Level::Error,
                trf("project.cannot_launch", &[("error", &error)]),
            ),
        }
    }

    /// The project browser tab.
    pub(super) fn project_pane(&mut self, ui: &mut egui::Ui) {
        if self.project.project.is_some() && self.project.stale(self.project_ws_mark()) {
            self.project_refresh();
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(tr("project.open"))
                .on_hover_text(tr("project.open_hint"))
                .clicked()
                && let Some(dir) = rfd::FileDialog::new()
                    .set_title(tr("project.open"))
                    .pick_folder()
            {
                self.project_open(dir);
            }
            let has = self.project.project.is_some();
            let to_build = self
                .project
                .status
                .as_ref()
                .map_or(0, ProjectStatus::to_build);
            let game = self.game.is_some();
            if ui
                .add_enabled(
                    has && game,
                    egui::Button::new(trn("project.build", to_build as u64, &[])),
                )
                .on_hover_text(tr("project.build_hint"))
                .clicked()
            {
                self.project_build(false);
            }
            if ui
                .add_enabled(has && game, egui::Button::new(tr("project.build_all")))
                .on_hover_text(tr("project.build_all_hint"))
                .clicked()
            {
                self.project_build(true);
            }
            if ui
                .add_enabled(has, egui::Button::new(tr("project.mod")))
                .on_hover_text(tr("project.mod_hint"))
                .clicked()
            {
                self.project_mod();
            }
            if ui
                .add_enabled(has, egui::Button::new(tr("project.refresh")))
                .clicked()
            {
                self.project.checked = None;
            }
            if has
                && ui.button(tr("project.show_folder")).clicked()
                && let Some(p) = &self.project.project
            {
                let _ = doctor_toolkit::external::open_with_desktop(p.root().as_os_str());
            }
        });
        if self.project.project.is_none() {
            self.project_new_form(ui);
            return;
        }
        ui.separator();
        let Some(project) = self.project.project.clone() else {
            return;
        };
        let config = &project.config;
        ui.horizontal_wrapped(|ui| {
            ui.strong(&config.project.name);
            ui.label(
                RichText::new(format!(
                    "{} {} — {}",
                    config.project.id,
                    config.project.version,
                    project.root().display()
                ))
                .weak(),
            );
        });
        let Some(status) = self.project.status.clone() else {
            return;
        };
        ui.horizontal_wrapped(|ui| {
            for state in [
                EntryState::Unchanged,
                EntryState::Modified,
                EntryState::Built,
                EntryState::OutOfSync,
            ] {
                ui.label(
                    RichText::new(format!("{} {}", status.count(&state), state_text(&state)))
                        .color(state_color(&state)),
                );
            }
            if let Some(line) = &self.project.last_build {
                ui.label(RichText::new(line).weak());
            }
        });
        for warning in &status.warnings {
            ui.label(RichText::new(warning).color(Color32::from_rgb(220, 160, 40)));
        }
        ui.horizontal(|ui| {
            ui.label(tr("project.search"));
            ui.text_edit_singleline(&mut self.project.filter);
            ui.checkbox(&mut self.project.only_changed, tr("project.only_changed"));
        });
        ui.separator();
        let filter = self.project.filter.to_lowercase();
        let wanted = |e: &EntryStatus| {
            (!self.project.only_changed || e.state != EntryState::Unchanged)
                && (filter.is_empty()
                    || e.id.to_lowercase().contains(&filter)
                    || e.name.to_lowercase().contains(&filter)
                    || e.type_tag.to_lowercase().contains(&filter))
        };
        let shown: Vec<&EntryStatus> = status.entries.iter().filter(|e| wanted(e)).collect();
        let mut clicked: Option<String> = None;
        let available = ui.available_height();
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width((ui.available_width() * 0.55).max(260.0));
                egui::ScrollArea::vertical()
                    .id_salt("project_tree")
                    .max_height(available)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let mut folders: Vec<&str> = shown.iter().map(|e| e.dir.as_str()).collect();
                        folders.dedup();
                        for folder in folders {
                            let in_folder: Vec<&&EntryStatus> =
                                shown.iter().filter(|e| e.dir == folder).collect();
                            let changed = in_folder
                                .iter()
                                .filter(|e| e.state != EntryState::Unchanged)
                                .count();
                            let title = if changed > 0 {
                                format!("{folder} ({} / {changed})", in_folder.len())
                            } else {
                                format!("{folder} ({})", in_folder.len())
                            };
                            egui::CollapsingHeader::new(title)
                                .id_salt(folder)
                                .default_open(!filter.is_empty() || changed > 0)
                                .show(ui, |ui| {
                                    let mut types: Vec<&str> =
                                        in_folder.iter().map(|e| e.type_tag.as_str()).collect();
                                    types.sort_unstable();
                                    types.dedup();
                                    for tag in types {
                                        let of_type: Vec<&&&EntryStatus> = in_folder
                                            .iter()
                                            .filter(|e| e.type_tag == tag)
                                            .collect();
                                        egui::CollapsingHeader::new(format!(
                                            "{tag} ({})",
                                            of_type.len()
                                        ))
                                        .id_salt((folder, tag))
                                        .default_open(
                                            !filter.is_empty()
                                                || of_type
                                                    .iter()
                                                    .any(|e| e.state != EntryState::Unchanged),
                                        )
                                        .show(ui, |ui| {
                                            for e in of_type {
                                                let file = e.id.rsplit('/').next().unwrap_or(&e.id);
                                                let selected =
                                                    self.project.selected.as_deref() == Some(&e.id);
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        RichText::new("●")
                                                            .color(state_color(&e.state)),
                                                    )
                                                    .on_hover_text(state_text(&e.state));
                                                    if ui.selectable_label(selected, file).clicked()
                                                    {
                                                        clicked = Some(e.id.clone());
                                                    }
                                                });
                                            }
                                        });
                                    }
                                });
                        }
                        for file in &status.unknown_files {
                            ui.label(
                                RichText::new(trf("project.unknown_file", &[("path", file)]))
                                    .weak(),
                            );
                        }
                    });
            });
            ui.separator();
            ui.vertical(|ui| {
                self.project_details(ui, &project, &status);
            });
        });
        if let Some(id) = clicked {
            self.project.selected = Some(id);
        }
    }

    fn project_new_form(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.label(tr("project.intro"));
        ui.add_space(6.0);
        ui.strong(tr("project.new"));
        ui.checkbox(
            &mut self.project.new_from_workspace,
            tr("project.from_workspace"),
        );
        ui.add_enabled_ui(!self.project.new_from_workspace, |ui| {
            ui.label(tr("project.streams"));
            ui.add(
                egui::TextEdit::multiline(&mut self.project.new_streams)
                    .hint_text("gf2/frontend, gf2/text/dd7d7050")
                    .desired_rows(2),
            );
        });
        let can = self.game.is_some()
            && (self.project.new_from_workspace || !self.project.new_streams.trim().is_empty());
        if ui
            .add_enabled(can, egui::Button::new(tr("project.create")))
            .on_hover_text(tr("project.create_hint"))
            .clicked()
            && let Some(dir) = rfd::FileDialog::new()
                .set_title(tr("project.create"))
                .pick_folder()
        {
            self.project_new(dir);
        }
    }

    fn project_details(&mut self, ui: &mut egui::Ui, project: &Project, status: &ProjectStatus) {
        let Some(id) = self.project.selected.clone() else {
            ui.label(RichText::new(tr("project.select")).weak());
            return;
        };
        let (Some((entry, dir)), Some(state)) = (
            project.entry(&id),
            status.entries.iter().find(|e| e.id == id),
        ) else {
            return;
        };
        ui.strong(&entry.name);
        ui.label(
            RichText::new(format!(
                "{} — {}",
                state_text(&state.state),
                entry.kind.label()
            ))
            .color(state_color(&state.state)),
        );
        egui::Grid::new("project_entry")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label(tr("project.type"));
                ui.label(format!("{} {}", entry.type_tag, entry.ids));
                ui.end_row();
                ui.label(tr("project.policy"));
                ui.label(&entry.policy);
                ui.end_row();
                ui.label(tr("project.files"));
                ui.vertical(|ui| {
                    for f in &entry.files {
                        ui.label(f);
                    }
                });
                ui.end_row();
                if let Some(patch) = &state.patch {
                    ui.label(tr("project.patch"));
                    ui.label(patch);
                    ui.end_row();
                }
            });
        ui.label(tr("project.resources"));
        let mut open: Option<ResourceKey> = None;
        for key in &entry.keys {
            if ui.link(key.to_string()).clicked() {
                open = Some(key.clone());
            }
        }
        let main = project.root().join(dir).join(&entry.files[0]);
        ui.horizontal_wrapped(|ui| {
            if entry.kind != Kind::View
                && ui
                    .button(tr("project.external"))
                    .on_hover_text(tr("project.external_hint"))
                    .clicked()
            {
                self.project_open_external(main.clone());
            }
            if ui.button(tr("project.open_resource")).clicked() {
                open = entry.keys.first().cloned();
            }
        });
        if let Some(key) = open {
            self.open_key(&key);
        }
        // A preview of a PNG texture.
        if entry.kind == Kind::Texture && main.extension().is_some_and(|e| e == "png") {
            let fresh = self
                .project
                .preview
                .as_ref()
                .is_none_or(|(path, _)| *path != id);
            if fresh {
                let texture = std::fs::read(&main)
                    .ok()
                    .and_then(|png| doctor_toolkit::resource::pixels::decode_png(&png).ok())
                    .map(|(w, h, rgba)| {
                        let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
                        ui.ctx()
                            .load_texture(format!("project:{id}"), image, Default::default())
                    });
                self.project.preview = Some((id.clone(), texture));
            }
            if let Some((_, Some(texture))) = &self.project.preview {
                let size = texture.size_vec2();
                let scale = (256.0 / size.x.max(size.y)).min(1.0);
                ui.add(egui::Image::new(texture).fit_to_exact_size(size * scale));
            }
        }
    }
}
