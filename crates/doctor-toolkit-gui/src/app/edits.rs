//! Workspace integration of the browser: edits are recorded in the workspace,
//! views show the workspace version, and the Changes panel applies them.

use std::path::{Path, PathBuf};

use doctor_toolkit::resource::lh2::StringTable;
use doctor_toolkit::resource::texture::TexturePair;
use doctor_toolkit::workspace::{ApplyOptions, ChangeState, ResourceKey, Workspace, WorkspaceLock};
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::RichText;

use super::dock::{DockOp, Pane};
use super::{BrowserApp, FileSel, Level};
use crate::changes_view::{APPLIED, CONFLICT, ChangesAction, PENDING};
use crate::nav::Location;
use crate::views::ViewAction;

/// How the editor names itself to programs that find the workspace locked.
const LOCK_HOLDER: &str = doctor_toolkit::bridge::EDITOR;

impl BrowserApp {
    /// Opens the game's workspace if it has one; it is created on the first edit.
    pub(super) fn open_workspace(&mut self) {
        self.load_workspace();
        self.refresh_status();
        // Another workspace: other edits, made on what they were made on.
        self.check_stale();
    }

    /// The folder of the workspace the editor works with: the one it was
    /// given ([`Self::workspace_at`]), else the game's own. Every part of
    /// the editor that opens the workspace or reads beside it takes the
    /// folder from here. `None` without a game.
    pub(super) fn workspace_dir(&self) -> Option<PathBuf> {
        let game = self.game.as_ref()?;
        Some(match &self.workspace_at {
            // Not relative to wherever the editor was started from.
            Some(dir) => std::path::absolute(dir).unwrap_or_else(|_| dir.clone()),
            None => Workspace::default_dir(game.root()),
        })
    }

    /// The workspace of the open game as it is on disk, or a new one that
    /// is not saved: for reading when the editor has none open yet.
    pub(super) fn workspace_for_reading(&self) -> Result<Workspace, String> {
        let game = self.game.as_ref().ok_or(tr("edits.no_game"))?;
        Workspace::for_game(game, self.workspace_dir().as_deref()).map_err(|e| e.to_string())
    }

    /// [`Self::open_workspace`] without comparing it with the game.
    pub(super) fn load_workspace(&mut self) {
        self.workspace = None;
        self.ws_lock = None;
        self.ws_status = None;
        let Some(dir) = self.workspace_dir() else {
            return;
        };
        if Workspace::exists(&dir) {
            match WorkspaceLock::acquire(&dir, LOCK_HOLDER) {
                Ok(lock) => self.ws_lock = Some(lock),
                // Edits take the lock later (`workspace_mut`).
                Err(error) => self.status = trf("edits.workspace_busy", &[("error", &error)]),
            }
            match Workspace::open(&dir) {
                Ok(ws) => self.workspace = Some(ws),
                Err(error) => {
                    self.status = trf("edits.cannot_open_workspace", &[("error", &error)])
                }
            }
        }
    }

    /// Compares the workspace with the game again and updates the marks.
    pub(super) fn refresh_status(&mut self) {
        // A stream or a resource of a new stream came or went: the game the
        // views show lists the new streams as they are now.
        self.restage();
        self.changes.invalidate();
        self.ws_status = match (&self.game, &self.workspace) {
            (Some(game), Some(ws)) => match ws.status(game) {
                Ok(status) => Some(status),
                Err(error) => {
                    self.status = trf("edits.cannot_compare", &[("error", &error)]);
                    None
                }
            },
            _ => None,
        };
        self.prune_stale();
        self.mark_changed_rows();
    }

    /// Finds the resources of the open streams that the workspace changes.
    pub(super) fn mark_changed_rows(&mut self) {
        let Some(game) = self.game.clone() else {
            return;
        };
        let ids: Vec<u64> = self.streams.keys().copied().collect();
        for id in ids {
            let Some(index) = self
                .streams
                .get(&id)
                .and_then(|t| t.stream.as_ref())
                .map(|s| s.game_index)
            else {
                continue;
            };
            let file = self.files.get(&game, index).ok();
            let Some(stream) = self.streams.get_mut(&id).and_then(|t| t.stream.as_mut()) else {
                continue;
            };
            stream.changed.clear();
            let (Some(ws), Some(file)) = (&self.workspace, file) else {
                continue;
            };
            let name = game.streams()[index].stream_name();
            for change in ws.changes().filter(|c| c.key.stream == name) {
                if let Some(i) = change.key.find_in(file.layout()) {
                    stream.changed.insert(i);
                }
            }
        }
    }

    /// The workspace key of a resource.
    pub(super) fn key_of(&self, loc: Location) -> Option<ResourceKey> {
        let game = self.game.as_ref()?;
        let header = match self.files.peek(loc.stream) {
            Some(file) => file.layout().resources.get(loc.resource)?.header,
            None if !self.files.is_released() => {
                game.open_stream(loc.stream)
                    .ok()?
                    .layout()
                    .resources
                    .get(loc.resource)?
                    .header
            }
            None => return None,
        };
        Some(ResourceKey::new(
            &game.streams()[loc.stream].stream_name(),
            &header,
        ))
    }

    pub(super) fn workspace_mut(&mut self) -> Result<&mut Workspace, String> {
        if self.workspace.is_none() {
            self.workspace = Some(self.workspace_for_reading()?);
        }
        let ws = self.workspace.as_mut().expect("just created");
        if self.ws_lock.is_none() {
            let lock = WorkspaceLock::acquire(ws.dir(), LOCK_HOLDER).map_err(|e| e.to_string())?;
            // Another program may have written it while the editor had no lock.
            ws.reload().map_err(|e| e.to_string())?;
            self.ws_lock = Some(lock);
        }
        Ok(ws)
    }

    /// Records what a view of resource `loc` asked for.
    pub(super) fn view_action(&mut self, loc: Location, action: ViewAction) {
        match action {
            ViewAction::SaveData { data, label } => self.save_resource(loc, data, &label),
            ViewAction::SaveStrings(table) => self.save_string_table(loc, &table),
            ViewAction::AddString {
                name,
                text,
                everywhere,
                translations,
            } => {
                if everywhere && !translations.is_empty() {
                    let translations = translations.into_iter().collect();
                    self.add_string_translated(&name, &text, &translations);
                } else {
                    self.add_string(loc, name, text, everywhere);
                }
            }
            ViewAction::EditStrings(edits) => {
                self.edit_strings_everywhere(&edits);
            }
            ViewAction::ImportTexture { pair, path } => {
                self.import_texture(loc.stream, pair, &path)
            }
            ViewAction::OpenLogicGraph { object } => {
                let Some(game) = &self.game else { return };
                let scope = format!(
                    "group:{}#{}",
                    game.streams()[loc.stream].stream_name(),
                    loc.resource
                );
                let group = doctor_toolkit::level::Location {
                    stream: loc.stream,
                    resource: loc.resource,
                };
                self.open_logic_tab(&scope, object.map(|o| (group, o)));
            }
            ViewAction::Apply { label, run } => self.apply_view_edit(&label, run),
            ViewAction::Open { stream, resource } => {
                self.open_location(Location { stream, resource })
            }
            ViewAction::OpenExternal { format } => self.open_external(loc, format),
            ViewAction::OpenInTab(mode) => {
                let id = self.open_resource_tab(loc);
                if let (Some(mode), Some(doc)) = (mode, self.docs.get_mut(&id)) {
                    for (_, view) in &mut doc.views {
                        view.set_mode(mode);
                    }
                }
            }
            ViewAction::Unlink(key) => self.unlink_external(&key),
            ViewAction::OpenInLevel => self.cine_open = Some(loc),
            ViewAction::OpenSound { page, mix } => self.open_sound_page(page, mix),
            ViewAction::StartWizard(task) => {
                if let Some(kind) = super::wizard::Kind::from_slug(task) {
                    let start = super::wizard::Start {
                        loc: Some(loc),
                        model: None,
                    };
                    self.start_wizard(kind, start);
                }
            }
        }
    }

    /// Runs an edit a view prepared on the workspace.
    fn apply_view_edit(&mut self, label: &str, run: crate::views::WorkspaceEdit) {
        let Some(game) = self.game.clone() else {
            return;
        };
        let result = self
            .workspace_mut()
            .and_then(|ws| run(&game, ws).map_err(|e| e.to_string()));
        match result {
            Ok(()) => self.status = trf("edits.recorded", &[("label", &label)]),
            Err(error) => self.notify(
                Level::Error,
                trf(
                    "edits.cannot_record",
                    &[("what", &label), ("error", &error)],
                ),
            ),
        }
        self.after_edit(None);
    }

    /// Records a texture imported from an image file.
    pub(super) fn import_texture(&mut self, stream: usize, pair: TexturePair, path: &Path) {
        let Some(game) = self.game.clone() else {
            return;
        };
        if let Some(dir) = path.parent() {
            self.settings.export_dirs.import = Some(dir.to_owned());
        }
        let is_png = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("png"));
        let result = std::fs::read(path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                self.workspace_mut()?
                    .import_texture(&game, stream, pair, &bytes, is_png)
                    .map_err(|e| e.to_string())
            });
        match result {
            Ok(texture) => {
                self.status = trf(
                    "edits.recorded_import",
                    &[
                        ("name", &texture.header.name),
                        ("width", &texture.header.width),
                        ("height", &texture.header.height),
                    ],
                );
            }
            Err(error) => self.notify(
                Level::Error,
                trf("edits.cannot_import", &[("error", &error)]),
            ),
        }
        self.after_edit(Some(stream));
    }

    /// Records new data for a resource (SimGroup and other logic edits, apx
    /// imports); the views keep their selection.
    pub(super) fn save_resource(&mut self, loc: Location, data: Vec<u8>, label: &str) {
        let Some(game) = self.game.clone() else {
            return;
        };
        let index = self.index.clone();
        let result = self.workspace_mut().and_then(|ws| {
            if let Some(index) = &index {
                let problems = doctor_toolkit::level::compat::check_resource_data(
                    &game,
                    index,
                    Some(ws),
                    (loc.stream, loc.resource),
                    &data,
                )
                .map_err(|e| e.to_string())?;
                if let Some(problem) = problems.into_iter().next() {
                    return Err(problem);
                }
            }
            ws.set_resource(&game, loc.stream, loc.resource, data, label)
                .map_err(|e| e.to_string())
        });
        match result {
            Ok(true) => self.status = trf("edits.recorded", &[("label", &label)]),
            Ok(false) => self.status = tr("edits.no_change_data").to_owned(),
            Err(error) => self.notify(
                Level::Error,
                trf("edits.cannot_record_edit", &[("error", &error)]),
            ),
        }
        self.after_edit(Some(loc.stream));
    }

    /// Records an edited string table.
    pub(super) fn save_string_table(&mut self, loc: Location, table: &StringTable) {
        let Some(game) = self.game.clone() else {
            return;
        };
        let result = self.workspace_mut().and_then(|ws| {
            ws.set_string_table(&game, loc.stream, loc.resource, table)
                .map_err(|e| e.to_string())
        });
        match result {
            Ok(true) => self.status = tr("edits.recorded_strings").to_owned(),
            Ok(false) => self.status = tr("edits.no_change_strings").to_owned(),
            Err(error) => self.notify(
                Level::Error,
                trf("edits.cannot_save_strings", &[("error", &error)]),
            ),
        }
        self.after_edit(Some(loc.stream));
    }

    /// Records a new version of a game file outside the streams (a movie).
    pub(super) fn record_file(
        &mut self,
        key: doctor_toolkit::workspace::FileKey,
        data: Vec<u8>,
        label: &str,
    ) {
        let Some(game) = self.game.clone() else {
            return;
        };
        let result = self.workspace_mut().and_then(|ws| {
            ws.set_file(&game, &key, data, label)
                .map_err(|e| e.to_string())
        });
        match result {
            Ok(_) => self.status = trf("edits.recorded", &[("label", &label)]),
            Err(error) => self.notify(
                Level::Error,
                trf("edits.cannot_record", &[("what", &key), ("error", &error)]),
            ),
        }
        self.after_edit(None);
    }

    /// Removes changes of files outside the streams from the workspace.
    fn revert_files(&mut self, keys: &[doctor_toolkit::workspace::FileKey]) {
        let Some(ws) = self.workspace.as_mut() else {
            return;
        };
        let label = trn("edits.revert_files", keys.len() as u64, &[]);
        match ws.revert_files(keys, &label) {
            Ok(true) => self.status = trf("edits.in_workspace", &[("label", &label)]),
            Ok(false) => self.status = tr("edits.nothing_to_revert").to_owned(),
            Err(error) => self.notify(
                Level::Error,
                trf("edits.cannot_revert", &[("error", &error)]),
            ),
        }
        self.after_edit(None);
    }

    /// Adds a string to string table `loc`, or to the tables of every language.
    pub(super) fn add_string(
        &mut self,
        loc: Location,
        name: String,
        text: String,
        everywhere: bool,
    ) {
        use doctor_toolkit::text::{self, NewString, TableRef};
        let Some(game) = self.game.clone() else {
            return;
        };
        let index = match &self.index {
            Some(index) => index.clone(),
            None => std::sync::Arc::new(game.build_index(|_| {}).0),
        };
        let new = NewString {
            name,
            text,
            ..Default::default()
        };
        let result = self.workspace_mut().and_then(|ws| {
            let tables = if everywhere {
                text::language_tables(&game, &index).map_err(|e| e.to_string())?
            } else {
                vec![TableRef {
                    stream: loc.stream,
                    resource: loc.resource,
                    language: None,
                }]
            };
            let plan =
                text::plan_add(&game, ws, &index, &tables, &new).map_err(|e| e.to_string())?;
            plan.commit(ws, &game).map_err(|e| e.to_string())?;
            Ok(plan)
        });
        match result {
            Ok(plan) => {
                self.status = trf(
                    "edits.string_added",
                    &[
                        ("label", &plan.label),
                        ("name", &plan.added.name),
                        ("hash", &format_args!("{:08x}", plan.added.hash)),
                    ],
                )
            }
            Err(error) => self.notify(
                Level::Error,
                trf("edits.cannot_add_string", &[("error", &error)]),
            ),
        }
        self.after_edit(None);
    }

    /// After the workspace changed: write into the game in direct mode, then
    /// refresh the marks and the views of `stream` (of all streams for `None`).
    pub(super) fn after_edit(&mut self, stream: Option<usize>) {
        if self.direct {
            self.apply(false);
        } else {
            self.refresh_status();
            self.reload_docs(stream);
            for level in self.levels.values_mut() {
                level.refresh(self.workspace.as_ref(), None);
            }
            for logic in self.logics.values_mut() {
                logic.refresh();
            }
        }
        self.reload_cine();
    }

    pub(super) fn apply(&mut self, force: bool) {
        let Some(game) = self.game.clone() else {
            return;
        };
        if self.workspace.is_none() || self.project_refuses_write() || self.refuse_write() {
            return;
        }
        // Release the memory-mapped streams before their files are replaced.
        self.release_files();
        let options = ApplyOptions {
            force,
            streams: None,
        };
        let ws = self.workspace.as_mut().expect("checked above");
        let (level, message) = match ws.apply(&game, options) {
            Ok(report) if report.streams.is_empty() => {
                (Level::Info, tr("edits.up_to_date").to_owned())
            }
            Ok(report) => {
                let mut text = trf(
                    "edits.applied",
                    &[
                        (
                            "streams",
                            &trn("count.streams", report.streams.len() as u64, &[]),
                        ),
                        (
                            "files",
                            &trn("count.files", report.written.len() as u64, &[]),
                        ),
                    ],
                );
                let mut level = Level::Info;
                if let Some(warning) = report.warnings.first() {
                    text += &trf("edits.applied_warning", &[("warning", warning)]);
                    level = Level::Warning;
                }
                (level, text)
            }
            Err(error) => (
                Level::Error,
                trf("edits.cannot_apply", &[("error", &error)]),
            ),
        };
        self.refresh_status();
        self.files_changed();
        self.notify(level, message);
    }

    pub(super) fn unapply(&mut self) {
        let Some(game) = self.game.clone() else {
            return;
        };
        if self.workspace.is_none() || self.refuse_write() {
            return;
        }
        self.release_files();
        let ws = self.workspace.as_mut().expect("checked above");
        let (level, message) = match ws.unapply(&game, false) {
            Ok(report) => (
                Level::Info,
                trn("edits.unapplied", report.streams.len() as u64, &[]),
            ),
            Err(error) => (
                Level::Error,
                trf("edits.cannot_unapply", &[("error", &error)]),
            ),
        };
        self.refresh_status();
        self.files_changed();
        self.notify(level, message);
    }

    /// Puts back every game file this tool changed, from its backups.
    pub(super) fn restore_originals(&mut self) {
        let Some(game) = self.game.clone() else {
            return;
        };
        if self.refuse_write() {
            return;
        }
        self.release_files();
        let result = game.restore_backups();
        self.refresh_status();
        self.files_changed();
        match result {
            Ok(files) => self.notify(Level::Info, trn("edits.restored", files.len() as u64, &[])),
            Err(error) => self.notify(
                Level::Error,
                trf("edits.cannot_restore", &[("error", &error)]),
            ),
        }
    }

    pub(super) fn undo(&mut self) {
        let Some(ws) = self.workspace.as_mut() else {
            self.status = tr("edits.nothing_to_undo").to_owned();
            return;
        };
        self.status = match ws.undo() {
            Ok(Some(label)) => trf("edits.undone", &[("label", &label)]),
            Ok(None) => tr("edits.nothing_to_undo").to_owned(),
            Err(error) => trf("edits.cannot_undo", &[("error", &error)]),
        };
        self.after_edit(None);
        // An undone rebase leaves its edits stale again.
        self.check_stale();
    }

    pub(super) fn redo(&mut self) {
        let Some(ws) = self.workspace.as_mut() else {
            self.status = tr("edits.nothing_to_redo").to_owned();
            return;
        };
        self.status = match ws.redo() {
            Ok(Some(label)) => trf("edits.redone", &[("label", &label)]),
            Ok(None) => tr("edits.nothing_to_redo").to_owned(),
            Err(error) => trf("edits.cannot_redo", &[("error", &error)]),
        };
        self.after_edit(None);
        self.check_stale();
    }

    fn revert(&mut self, keys: &[ResourceKey]) {
        let Some(ws) = self.workspace.as_mut() else {
            return;
        };
        let label = if keys.len() == 1 {
            trf("edits.revert_one", &[("key", &keys[0])])
        } else {
            trn("edits.revert_changes", keys.len() as u64, &[])
        };
        self.status = match ws.revert(keys, &label) {
            Ok(true) => trf("edits.in_workspace", &[("label", &label)]),
            Ok(false) => tr("edits.nothing_to_revert").to_owned(),
            Err(error) => trf("edits.cannot_revert", &[("error", &error)]),
        };
        self.after_edit(None);
    }

    /// Shows a resource by key in its stream.
    pub(super) fn open_key(&mut self, key: &ResourceKey) {
        let Some(game) = self.game.clone() else {
            return;
        };
        let Some(stream) = game.find_stream(&key.stream) else {
            self.notify(
                Level::Warning,
                trf("edits.stream_missing", &[("stream", &key.stream)]),
            );
            return;
        };
        let index = self
            .files
            .get(&game, stream)
            .ok()
            .and_then(|file| key.find_in(file.layout()));
        match index {
            Some(resource) => self.open_location(Location { stream, resource }),
            None => self.open_file(FileSel::Stream(stream), false),
        }
    }

    /// The change indicator of the toolbar (direct mode is in the menu of
    /// "Apply to game", stage 52).
    pub(super) fn workspace_bar(&mut self, ui: &mut egui::Ui) {
        let unsaved = self.details.iter().any(|d| d.has_unsaved_edits())
            || self.docs.values().any(|d| d.has_unsaved_edits());
        let (text, color) = match &self.ws_status {
            Some(s) if !s.conflicts().is_empty() => (
                trf(
                    "edits.bar_conflict",
                    &[("count", &s.count(ChangeState::Conflict))],
                ),
                Some(CONFLICT),
            ),
            Some(s) if !s.is_up_to_date() => (
                trf(
                    "edits.bar_pending",
                    &[(
                        "count",
                        &(s.count(ChangeState::Pending) + s.pending_reverts.len()),
                    )],
                ),
                Some(PENDING),
            ),
            Some(s) if !s.changes.is_empty() => (
                trf(
                    "edits.bar_applied",
                    &[("count", &(s.changes.len() + s.files.len()))],
                ),
                Some(APPLIED),
            ),
            _ => (tr("pane.changes").to_owned(), None),
        };
        let mut label = RichText::new(text);
        if let Some(color) = color {
            label = label.color(color);
        }
        if ui
            .button(label)
            .on_hover_text(tr("edits.bar_hint"))
            .clicked()
        {
            self.dock_ops.push(DockOp::ShowPanel(Pane::Changes));
        }
        if unsaved {
            ui.label(RichText::new(tr("edits.unsaved")).color(PENDING))
                .on_hover_text(tr("edits.unsaved_hint"));
        }
        if self.direct {
            ui.label(RichText::new(tr("edits.direct")).color(PENDING))
                .on_hover_text(tr("edits.direct_hint"));
        }
    }

    /// The Changes panel and what it asks for.
    pub(super) fn changes_pane(&mut self, ui: &mut egui::Ui) {
        // What the game changed under a stale edit is compared when the
        // panel shows that edit, not every frame.
        let game = self.disk_game();
        let own = self.own_mod_id();
        let workspace = self.workspace.as_ref();
        let base_detail = |key: &ResourceKey| {
            let (Some(game), Some(ws)) = (&game, workspace) else {
                return Err(tr("edits.no_game").to_owned());
            };
            let given = super::document::given_of(game, ws, own.as_deref())?;
            ws.base_detail(game, &given, key).map_err(|e| e.to_string())
        };
        let stale = crate::changes_view::StaleChanges {
            stale: &self.base.stale,
            detail: &base_detail,
        };
        let action = self
            .changes
            .ui(ui, workspace, self.ws_status.as_ref(), &stale);
        use super::document::Rebase;
        match action {
            Some(ChangesAction::Rebase(None)) => self.rebase(Rebase::All),
            Some(ChangesAction::Rebase(Some(keys))) => self.rebase(Rebase::Merge(keys)),
            Some(ChangesAction::KeepMine(keys)) => self.rebase(Rebase::KeepMine(keys)),
            Some(ChangesAction::TakeBase(keys)) => self.rebase(Rebase::TakeBase(keys)),
            Some(ChangesAction::Undo) => self.undo(),
            Some(ChangesAction::Redo) => self.redo(),
            // Overwriting foreign changes was confirmed in the panel.
            Some(ChangesAction::Apply(true)) => self.apply(true),
            Some(ChangesAction::Apply(false)) => self.apply_checked(),
            Some(ChangesAction::Unapply) => self.unapply(),
            Some(ChangesAction::Revert(keys)) => self.revert(&keys),
            Some(ChangesAction::RevertFiles(keys)) => self.revert_files(&keys),
            Some(ChangesAction::Open(key)) => self.open_key(&key),
            None => {}
        }
    }
}
