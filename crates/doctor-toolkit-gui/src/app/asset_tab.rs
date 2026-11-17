//! A resource in a tab of its own: an asset editor in the manner of Unreal
//! Engine (stage 56).
//!
//! The tab is a bar of tools — the asset's menu (Show in Content Browser,
//! Show in stream, Copy key, Export…), the tools of its views (wizards, the
//! external editor, the view's own switches as "▾" menus) and "?" with the
//! keys — over the editor, which fills the rest of the tab. The header of
//! the resource (type, ids, sizes), its references and the views that do
//! not fill the tab go to the shared Details panel together with what is
//! selected in the editor (`super::details`, [`DocRef::Tab`]). Resources
//! without an editor that fills the tab scroll as before, under the bar.
//!
//! The same asset commands ([`AssetCmd`]) are in the tab's context menu
//! and in the Content Browser.

use doctor_toolkit_mods::i18n::{tr, trf};
use egui::RichText;

use super::BrowserApp;
use super::browse::DocAction;
use super::details::DocRef;
use super::dock::{DockOp, Pane};
use super::docs::ResourceDoc;
use super::theme::{self, Icon};
use crate::hex;
use crate::nav::Location;
use crate::views::{Place, TabBody, ViewContext};

/// A command on an asset: the same in the menu of its tab, the bar of its
/// editor and the Content Browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetCmd {
    OpenTab,
    ShowInContent,
    ShowInStream,
    CopyKey,
    Export,
    /// A cut-scene shot: open the cut-scene in the level view (stage 62).
    OpenInLevel,
    /// Replace the resource with a file read by its type, or (`raw`) taken
    /// as the stored data (stage 103).
    ImportFile {
        raw: bool,
    },
    /// New collision of this kind for the metamodel, or for the metamodel
    /// that uses this collision (stage 103).
    BuildCollision(doctor_toolkit::resource::hkx::collision::CollisionKind),
}

/// Keys every editor of the tabs shares (`?` of their bars, the shortcut
/// window and the guide): (keys, text key).
pub const COMMON_KEYS: &[(&str, &str)] = &[
    ("F", "keys.frame"),
    ("Home", "keys.frame_all"),
    ("Delete", "keys.delete"),
    ("Ctrl+D", "keys.duplicate"),
    ("Ctrl+A", "keys.select_all"),
    ("Ctrl+C / Ctrl+V", "keys.copy_paste"),
    ("Ctrl+Z / Ctrl+Y", "keys.undo"),
    ("Ctrl+W", "keys.close"),
    ("Ctrl+M", "keys.maximize"),
];

/// The items of an asset's menu; what is clicked goes to `out`.
pub fn asset_menu(ui: &mut egui::Ui, in_tab: bool, type_id: u32, out: &mut Vec<AssetCmd>) {
    let mut item = |ui: &mut egui::Ui, cmd: AssetCmd, text: &str| {
        if ui.button(text).clicked() {
            out.push(cmd);
            ui.close();
        }
    };
    if !in_tab {
        item(ui, AssetCmd::OpenTab, tr("doc.open_tab"));
    }
    item(ui, AssetCmd::ShowInContent, tr("doc.show_in_content"));
    item(ui, AssetCmd::ShowInStream, tr("doc.reveal"));
    item(ui, AssetCmd::CopyKey, tr("doc.copy_key"));
    if type_id == doctor_toolkit::resource::trinity::TYPE_TRINITY_SEQ_MASTER {
        item(ui, AssetCmd::OpenInLevel, tr("cine.open_in_level"));
    }
    ui.separator();
    item(ui, AssetCmd::Export, tr("doc.export"));
    if doctor_toolkit::import::file::is_supported(type_id) {
        item(
            ui,
            AssetCmd::ImportFile { raw: false },
            tr("doc.import_file"),
        );
    }
    item(ui, AssetCmd::ImportFile { raw: true }, tr("doc.import_raw"));
    if type_id == doctor_toolkit::level::TYPE_METAMODEL
        || type_id == doctor_toolkit::resource::hkx::TYPE_HKX
    {
        use doctor_toolkit::resource::hkx::collision::CollisionKind as K;
        ui.menu_button(tr("doc.build_collision"), |ui| {
            ui.label(
                egui::RichText::new(tr("doc.build_collision_hint"))
                    .weak()
                    .small(),
            );
            for (kind, text) in [
                (K::Mesh, "doc.collision_mesh"),
                (K::Hull, "doc.collision_hull"),
                (K::Box, "doc.collision_box"),
            ] {
                if ui.button(tr(text)).clicked() {
                    out.push(AssetCmd::BuildCollision(kind));
                    ui.close();
                }
            }
        });
    }
}

impl BrowserApp {
    /// Whether the shared Details panel is open and follows the selection,
    /// so that an editor in a tab leaves its own side column out. Taken at
    /// the start of the frame (`details_open`).
    pub(super) fn details_follow(&self) -> bool {
        self.details_open
    }

    /// Works out [`Self::details_follow`] for the frame: Details is shown in
    /// the dock (not under a maximized tab) or as the open drawer, and is
    /// not pinned.
    pub(super) fn track_details_open(&mut self) {
        let pinned = self.inspectors.main.pinned.is_some();
        let drawer = self.drawers.open == Some(Pane::Details);
        let shown = self.maximized.is_none()
            && self.dock.as_ref().is_some_and(|tree| {
                super::dock::find(tree, |p| *p == Pane::Details)
                    .is_some_and(|t| super::dock::is_shown(tree, t))
            });
        self.details_open = !pinned && (drawer || shown);
    }

    /// Details follow a document tab (a click into it, opening it).
    pub(super) fn focus_document(&mut self, pane: &Pane) {
        if let Some(doc) = DocRef::of(pane) {
            self.inspect = super::details::Focus::Document(doc);
        }
    }

    /// Whether an editor still exists.
    pub(super) fn has_document(&self, doc: DocRef) -> bool {
        match doc {
            DocRef::Tab(id) => self.docs.contains_key(&id) || self.logics.contains_key(&id),
            DocRef::Script => self.script_editor.open,
            DocRef::Movies => self.movies.open,
        }
    }

    /// Carries out an asset command.
    pub(super) fn asset_cmd(&mut self, loc: Location, cmd: AssetCmd, ctx: &egui::Context) {
        match cmd {
            AssetCmd::OpenTab => {
                self.open_resource_tab(loc);
            }
            AssetCmd::OpenInLevel => self.cine_open = Some(loc),
            AssetCmd::ShowInContent => {
                if let Some(key) = self.key_of(loc) {
                    self.content.reveal(&key);
                    self.dock_ops.push(DockOp::ShowPanel(Pane::ContentBrowser));
                }
            }
            AssetCmd::ShowInStream => {
                self.nav.visit(loc);
                self.open_location(loc);
            }
            AssetCmd::CopyKey => {
                if let Some(key) = self.key_of(loc) {
                    let text = key.to_string();
                    self.status = trf("content.copied", &[("key", &text)]);
                    ctx.copy_text(text);
                }
            }
            AssetCmd::Export => {
                if let Some(game) = self.game.clone() {
                    self.nav.export(&game, vec![loc]);
                }
            }
            AssetCmd::ImportFile { raw } => {
                let mut dialog = rfd::FileDialog::new();
                if let Some(dir) = &self.settings.export_dirs.import {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(path) = dialog.pick_file() {
                    self.import_resource_file(loc, &path, raw);
                }
            }
            AssetCmd::BuildCollision(kind) => {
                self.build_collision(loc, kind, None);
            }
        }
    }

    /// F for the editor in front, also when the pointer is not over it:
    /// frames the selection (the level view has its own F). Taken before
    /// the widgets, so a view under the pointer does not frame again.
    pub(super) fn document_keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input()
            || !ctx.input(|i| i.key_pressed(egui::Key::F) && !i.modifiers.any())
        {
            return;
        }
        let framed = match self.front_document() {
            Some(Pane::Resource { id, .. }) => self
                .docs
                .get_mut(&id)
                .is_some_and(|doc| doc.views.iter_mut().any(|(_, view)| view.frame_selection())),
            Some(Pane::Logic { id, .. }) => match self.logics.get_mut(&id) {
                Some(editor) => {
                    editor.frame_selection();
                    true
                }
                None => false,
            },
            Some(Pane::Script) => {
                self.script_editor.frame_selection();
                true
            }
            _ => false,
        };
        if framed {
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F));
        }
    }

    /// Asset commands asked for while the dock was drawn (the tab menus).
    pub(super) fn run_asset_cmds(&mut self, ctx: &egui::Context) {
        for (loc, cmd) in std::mem::take(&mut self.asset_cmds) {
            self.asset_cmd(loc, cmd, ctx);
        }
    }

    /// A resource tab.
    pub(super) fn resource_pane(&mut self, ui: &mut egui::Ui, id: u64, key: &str) {
        if !self.docs.contains_key(&id) {
            // A tab of a saved layout: find the resource by its key.
            let Some(loc) = self.locate_key(key) else {
                ui.label(RichText::new(format!("{key}: {}", tr("doc.not_found"))).weak());
                return;
            };
            let doc = self.load_doc(loc, None);
            self.docs.insert(id, doc);
        }
        let Some(mut doc) = self.docs.remove(&id) else {
            return;
        };
        let action = ui
            .push_id(("asset tab", id), |ui| self.asset_tab_ui(ui, id, &mut doc))
            .inner;
        let loc = doc.loc;
        self.docs.entry(id).or_insert(doc);
        if let Some(action) = action {
            self.doc_action(Some(loc), action);
        }
    }

    /// The bar of tools and the editor of a resource tab.
    fn asset_tab_ui(
        &mut self,
        ui: &mut egui::Ui,
        id: u64,
        doc: &mut ResourceDoc,
    ) -> Option<DocAction> {
        if doc.error.is_some() || self.game.is_none() {
            return egui::ScrollArea::vertical()
                .show(ui, |ui| self.doc_ui(ui, doc, true))
                .inner;
        }
        let details = self.details_follow();
        let loc = doc.loc;
        let fill = doc
            .views
            .iter()
            .position(|(_, v)| v.tab_body() == TabBody::Fill);
        let mut keys: Vec<(&str, &str)> = fill
            .map(|i| doc.views[i].1.keys().to_vec())
            .unwrap_or_default();
        keys.extend_from_slice(COMMON_KEYS);
        let mut action = None;
        let mut cmds = Vec::new();
        theme::doc_bar(ui, |ui| {
            theme::menu_button(
                ui,
                Some(Icon::Object),
                tr("doc.asset"),
                tr("doc.asset_hint"),
                |ui| asset_menu(ui, true, doc.type_id, &mut cmds),
            );
            if theme::icon_button(ui, Icon::Browse, tr("doc.show_in_content")) {
                cmds.push(AssetCmd::ShowInContent);
            }
            if theme::icon_button(ui, Icon::Copy, tr("doc.copy_key")) {
                cmds.push(AssetCmd::CopyKey);
            }
            theme::separator(ui);
            // Import and export of a model: one menu, the bar stays one line.
            let type_id = doc.type_id;
            if type_id == doctor_toolkit::resource::mesh::TYPE_MESH
                || type_id == doctor_toolkit::hash::sdbm(b"metamodel")
            {
                theme::menu_button(
                    ui,
                    Some(Icon::Export),
                    tr("doc.model_io"),
                    tr("doc.model_io_hint"),
                    |ui| self.model_import_button(ui, loc, type_id),
                );
            }
            let mut cx = ViewContext {
                status: &mut self.status,
                links: &self.external.infos,
                place: Place::Tab,
                details,
                cursor: Some(self.cine_cursor.clone()),
            };
            for (_, view) in &mut doc.views {
                if let Some(a) = view.toolbar(ui, &mut cx) {
                    action = Some(DocAction::View(a));
                }
            }
            theme::help_button(ui, &keys);
        });
        for cmd in cmds {
            self.asset_cmds.push((loc, cmd));
        }
        let mut cx = ViewContext {
            status: &mut self.status,
            links: &self.external.infos,
            place: Place::Tab,
            details,
            cursor: Some(self.cine_cursor.clone()),
        };
        match fill {
            Some(i) => {
                if let Some(a) = doc.views[i].1.show(ui, &mut cx) {
                    action = Some(DocAction::View(a));
                }
            }
            None => {
                let scrolled = egui::ScrollArea::vertical()
                    .id_salt(("resource scroll", id))
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        let mut action = None;
                        for (_, view) in &mut doc.views {
                            if view.tab_body() != TabBody::Scroll {
                                continue;
                            }
                            if let Some(a) = view.show(ui, &mut cx) {
                                action = Some(DocAction::View(a));
                            }
                            ui.separator();
                        }
                        match &doc.hex {
                            Some(Ok(data)) => {
                                hex::show(ui, egui::IdSalt::new(("hex", id)), data);
                            }
                            Some(Err(error)) => {
                                ui.colored_label(egui::Color32::RED, error);
                            }
                            None => {}
                        }
                        action
                    })
                    .inner;
                if scrolled.is_some() {
                    action = scrolled;
                }
            }
        }
        action
    }

    /// Details of a resource tab: the selection of its editor, the views
    /// that do not fill the tab, the header and the references.
    pub(super) fn resource_tab_details(&mut self, ui: &mut egui::Ui, id: u64) {
        let Some(mut doc) = self.docs.remove(&id) else {
            return;
        };
        let action = self.resource_tab_details_ui(ui, &mut doc);
        let loc = doc.loc;
        self.docs.entry(id).or_insert(doc);
        if let Some(action) = action {
            self.doc_action(Some(loc), action);
        }
    }

    fn resource_tab_details_ui(
        &mut self,
        ui: &mut egui::Ui,
        doc: &mut ResourceDoc,
    ) -> Option<DocAction> {
        let game = self.game.clone()?;
        let mut action = None;
        ui.horizontal(|ui| {
            ui.heading(self.doc_title(doc));
        });
        ui.label(RichText::new(&game.streams()[doc.loc.stream].rel_path).weak());
        let fill = doc.views.iter().any(|(_, v)| v.tab_body() == TabBody::Fill);
        {
            let mut cx = ViewContext {
                status: &mut self.status,
                links: &self.external.infos,
                place: Place::Tab,
                details: true,
                cursor: Some(self.cine_cursor.clone()),
            };
            for (_, view) in &mut doc.views {
                if view.has_inspector() {
                    ui.separator();
                    if let Some(a) = view.inspector(ui, &mut cx) {
                        action = Some(DocAction::View(a));
                    }
                }
            }
        }
        if fill {
            // The views that do not fill the tab live here.
            let mut cx = ViewContext {
                status: &mut self.status,
                links: &self.external.infos,
                place: Place::Details,
                details: true,
                cursor: Some(self.cine_cursor.clone()),
            };
            for (_, view) in &mut doc.views {
                if view.tab_body() == TabBody::Scroll {
                    ui.separator();
                    if let Some(a) = view.show(ui, &mut cx) {
                        action = Some(DocAction::View(a));
                    }
                }
            }
        }
        ui.separator();
        let file = self.files.get(&game, doc.loc.stream).ok()?;
        egui::CollapsingHeader::new(tr("doc.resource_group"))
            .id_salt(("doc resource", doc.loc.stream, doc.loc.resource))
            .default_open(false)
            .show(ui, |ui| {
                if let Some(a) = self.resource_header(ui, doc, &file) {
                    action = Some(a);
                }
            });
        if let Some(nav) = self.nav.refs_panel(ui, doc.loc) {
            return Some(DocAction::Nav(nav));
        }
        action
    }
}
