//! "Open in external editor": resources linked to working files that other
//! programs edit. Every save is imported into the workspace (rapid saves as
//! one undoable operation, `doctor_toolkit::external::EditLink`) and the
//! views of the resource are loaded again.

use std::time::{Duration, Instant};

use doctor_toolkit::external::{
    EditLink, EditorRole, Format, LinkEvent, ModelContext, Poll, Target,
};
use doctor_toolkit::workspace::ResourceKey;
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::RichText;

use super::BrowserApp;
use super::shell::Level;
use crate::external_view::LinkInfo;
use crate::nav::Location;

/// Without file events, the files are checked this often.
const CHECK_EVERY: Duration = Duration::from_secs(1);

/// The links of the open game.
pub(crate) struct Links {
    links: Vec<EditLink>,
    /// What the views and the status bar show.
    pub(crate) infos: Vec<LinkInfo>,
    /// Repaints the window when a working file changes.
    ctx: egui::Context,
    /// `DOCTOR_TOOLKIT_GUI_EXTERNAL=<format>`: link the resource shown in
    /// Details once (screenshots; the program of the settings is started).
    preset: Option<String>,
}

impl Links {
    pub(crate) fn new(ctx: &egui::Context) -> Self {
        Self {
            links: Vec::new(),
            infos: Vec::new(),
            ctx: ctx.clone(),
            preset: std::env::var("DOCTOR_TOOLKIT_GUI_EXTERNAL").ok(),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.links.clear();
        self.infos.clear();
    }

    fn refresh_infos(&mut self) {
        self.infos = self
            .links
            .iter()
            .map(|link| LinkInfo {
                key: link.target.key.clone(),
                path: link.path().to_owned(),
                state: describe(link),
            })
            .collect();
    }
}

/// A short line about the last save of a link.
fn describe(link: &EditLink) -> String {
    let last = match &link.last_event {
        None => String::new(),
        Some((_, LinkEvent::Imported { imported, .. })) => imported.label.clone(),
        Some((_, LinkEvent::Unchanged)) => tr("external.unchanged").to_owned(),
        Some((_, LinkEvent::Failed(error))) => trf("external.failed", &[("error", error)]),
        Some((_, LinkEvent::Missing)) => tr("external.missing").to_owned(),
    };
    let count = trn("external.saves_imported", link.imports as u64, &[]);
    if last.is_empty() {
        count
    } else {
        format!("{count}; {last}")
    }
}

impl BrowserApp {
    /// The program of the settings for `format`; empty for the desktop's.
    fn editor_command(&self, format: Format) -> String {
        let editors = &self.settings.external_editors;
        match format.editor_role() {
            EditorRole::Image => editors.image.clone(),
            EditorRole::Model => editors.model.clone(),
            EditorRole::Text => editors.text.clone(),
        }
    }

    /// Exports resource `loc`, opens it in another program and watches it.
    /// A resource linked already is opened again.
    pub(super) fn open_external(&mut self, loc: Location, format: Format) {
        let Some(game) = self.game.clone() else {
            return;
        };
        let target = match Target::at(&game, loc.stream, loc.resource, Some(format)) {
            Ok(target) => target,
            Err(error) => {
                self.notify(
                    Level::Error,
                    trf("external.cannot_link", &[("error", &error)]),
                );
                return;
            }
        };
        let command = self.editor_command(format);
        let program = if command.trim().is_empty() {
            tr("external.desktop_program").to_owned()
        } else {
            command.clone()
        };
        let links = &mut self.external.links;
        if let Some(link) = links
            .iter_mut()
            .find(|l| l.target.key == target.key && l.target.format == format)
        {
            let result = link.open_editor(&command);
            let path = link.path().display().to_string();
            match result {
                Ok(()) => self.notify(
                    Level::Info,
                    trf("external.opened", &[("file", &path), ("program", &program)]),
                ),
                Err(error) => self.notify(
                    Level::Error,
                    trf("external.cannot_open", &[("error", &error)]),
                ),
            }
            return;
        }
        // Another format of the same resource replaces its link.
        links.retain(|l| l.target.key != target.key);
        if target.is_model()
            && self.assets.is_none()
            && let Some(index) = &self.index
        {
            self.assets = Some(doctor_toolkit::game::GameAssets::new(game.clone(), index));
        }
        let models = match (&self.index, &mut self.assets) {
            (Some(index), Some(assets)) if target.is_model() => Some(ModelContext {
                index: index.as_ref(),
                assets,
            }),
            _ => None,
        };
        let dir = EditLink::default_dir(&target);
        let waker = self.external.ctx.clone();
        let wake: Box<dyn Fn() + Send + Sync> = Box::new(move || waker.request_repaint());
        let created = EditLink::create(
            &game,
            self.workspace.as_ref(),
            target,
            &dir,
            models,
            Some(wake),
        );
        let mut link = match created {
            Ok(link) => link,
            Err(error) => {
                self.notify(
                    Level::Error,
                    trf("external.cannot_link", &[("error", &error)]),
                );
                return;
            }
        };
        let path = link.path().display().to_string();
        let opened = link.open_editor(&command);
        self.external.links.push(link);
        self.external.refresh_infos();
        match opened {
            Ok(()) => self.notify(
                Level::Info,
                trf(
                    "external.linked_now",
                    &[("file", &path), ("program", &program)],
                ),
            ),
            Err(error) => self.notify(
                Level::Warning,
                trf(
                    "external.linked_not_opened",
                    &[("file", &path), ("error", &error)],
                ),
            ),
        }
    }

    /// Stops watching the working file of `key`; the file stays.
    pub(super) fn unlink_external(&mut self, key: &ResourceKey) {
        let before = self.external.links.len();
        self.external.links.retain(|l| &l.target.key != key);
        if self.external.links.len() != before {
            self.external.refresh_infos();
            self.notify(Level::Info, trf("external.unlinked", &[("key", key)]));
        }
    }

    /// Checks the working files and imports the saves. Called every frame.
    pub(super) fn poll_external(&mut self, ctx: &egui::Context) {
        if self.external.preset.is_some()
            && let Some(loc) = self.details.as_ref().map(|d| d.loc)
        {
            let text = self.external.preset.take().unwrap_or_default();
            match text.parse::<Format>() {
                Ok(format) => self.open_external(loc, format),
                Err(error) => eprintln!("DOCTOR_TOOLKIT_GUI_EXTERNAL: {error}"),
            }
        }
        if self.external.links.is_empty() {
            return;
        }
        let Some(game) = self.game.clone() else {
            self.external.clear();
            return;
        };
        let now = Instant::now();
        let mut saved = Vec::new();
        let mut touched = false;
        for (i, link) in self.external.links.iter_mut().enumerate() {
            match link.poll(now) {
                Ok(Poll::Idle) => {}
                Ok(Poll::Waiting(wait)) => ctx.request_repaint_after(wait),
                Ok(Poll::Changed(bytes)) => saved.push((i, bytes)),
                Ok(Poll::Unchanged) => {
                    link.last_event = Some((now, LinkEvent::Unchanged));
                    touched = true;
                }
                Ok(Poll::Missing) => {
                    link.last_event = Some((now, LinkEvent::Missing));
                    touched = true;
                }
                Err(error) => {
                    link.last_event = Some((now, LinkEvent::Failed(error.to_string())));
                    touched = true;
                }
            }
        }
        for (i, bytes) in saved {
            touched = true;
            if let Err(error) = self.workspace_mut() {
                self.notify(
                    Level::Error,
                    trf("external.cannot_import", &[("error", &error)]),
                );
                continue;
            }
            let is_model = self.external.links[i].target.is_model();
            if is_model
                && self.assets.is_none()
                && let Some(index) = &self.index
            {
                self.assets = Some(doctor_toolkit::game::GameAssets::new(game.clone(), index));
            }
            let models = match (&self.index, &mut self.assets) {
                (Some(index), Some(assets)) if is_model => Some(ModelContext {
                    index: index.as_ref(),
                    assets,
                }),
                _ => None,
            };
            let ws = self.workspace.as_mut().expect("made above");
            let link = &mut self.external.links[i];
            let stream = link.target.stream;
            let file = link
                .path()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            match link.import(&game, ws, &bytes, models, now) {
                LinkEvent::Imported { imported, merged } => {
                    let text = match (imported.seq, merged) {
                        (None, _) => trf("external.no_change", &[("file", &file)]),
                        (Some(_), true) => trf(
                            "external.imported_merged",
                            &[("file", &file), ("label", &imported.label)],
                        ),
                        (Some(_), false) => trf(
                            "external.imported",
                            &[("file", &file), ("label", &imported.label)],
                        ),
                    };
                    let warnings: Vec<&String> = imported
                        .notes
                        .iter()
                        .filter(|n| n.starts_with("warning"))
                        .collect();
                    match warnings.first() {
                        Some(first) => self.notify(Level::Warning, format!("{text}; {first}")),
                        None => self.notify(Level::Info, text),
                    }
                    self.after_edit(Some(stream));
                }
                LinkEvent::Failed(error) => self.notify(
                    Level::Error,
                    trf(
                        "external.import_failed",
                        &[("file", &file), ("error", &error)],
                    ),
                ),
                LinkEvent::Unchanged | LinkEvent::Missing => {}
            }
        }
        if touched {
            self.external.refresh_infos();
        }
        // The file times are checked even when no events come.
        ctx.request_repaint_after(CHECK_EVERY);
    }

    /// The links in the status bar: how many, and a list to manage them.
    pub(super) fn external_status(&mut self, ui: &mut egui::Ui) {
        if self.external.links.is_empty() {
            return;
        }
        let button = ui
            .small_button(
                RichText::new(trf(
                    "external.status",
                    &[("count", &self.external.links.len())],
                ))
                .color(crate::changes_view::PENDING),
            )
            .on_hover_text(tr("external.status_hint"));
        let popup_id = ui.make_persistent_id("external-links");
        let mut unlink = None;
        let mut refresh = None;
        egui::Popup::from_response(&button)
            .id(popup_id)
            .open_memory(button.clicked().then_some(egui::SetOpenCommand::Toggle))
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.set_min_width(420.0);
                for info in &self.external.infos {
                    ui.horizontal(|ui| {
                        ui.strong(info.key.to_string());
                        if ui.small_button(tr("external.unlink")).clicked() {
                            unlink = Some(info.key.clone());
                        }
                        if ui
                            .small_button(tr("external.refresh"))
                            .on_hover_text(tr("external.refresh_hint"))
                            .clicked()
                        {
                            refresh = Some(info.key.clone());
                        }
                        if ui.small_button(tr("external.folder")).clicked()
                            && let Some(dir) = info.path.parent()
                        {
                            let _ = super::shell::open_external(dir.as_os_str());
                        }
                    });
                    ui.label(RichText::new(info.path.display().to_string()).monospace());
                    ui.label(RichText::new(&info.state).weak());
                    ui.separator();
                }
            });
        if let Some(key) = unlink {
            self.unlink_external(&key);
        }
        if let Some(key) = refresh {
            self.refresh_external(&key);
        }
        ui.separator();
    }

    /// Writes the workspace version of a linked resource into its file
    /// (after an undo, say).
    fn refresh_external(&mut self, key: &ResourceKey) {
        let Some(game) = self.game.clone() else {
            return;
        };
        let Some(i) = self
            .external
            .links
            .iter()
            .position(|l| &l.target.key == key)
        else {
            return;
        };
        let is_model = self.external.links[i].target.is_model();
        let models = match (&self.index, &mut self.assets) {
            (Some(index), Some(assets)) if is_model => Some(ModelContext {
                index: index.as_ref(),
                assets,
            }),
            _ => None,
        };
        let link = &mut self.external.links[i];
        let result = link.refresh(&game, self.workspace.as_ref(), models);
        let file = link.path().display().to_string();
        match result {
            Ok(()) => self.notify(Level::Info, trf("external.refreshed", &[("file", &file)])),
            Err(error) => self.notify(
                Level::Error,
                trf("external.cannot_import", &[("error", &error)]),
            ),
        }
    }
}
