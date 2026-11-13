//! The browsing tabs and panels: the file list, stream tabs, tables of
//! contents, all resources, search tabs, the Details panel and resource tabs.

use egui::{Align, Color32, RichText, Sense};
use egui_extras::{Column, TableBuilder};

use doctor_toolkit::resource::names::{mem_policies, type_tags};
use doctor_toolkit_mods::i18n::{tr, trf, trn};

use super::dock::{DockOp, Pane};
use super::docs::ResourceDoc;
use super::{BrowserApp, FileSel, ROW_HEIGHT, StreamTab, find_file, human_size};
use crate::hex;
use crate::jobs;
use crate::nav::{Location, NavAction, Search};
use crate::views::ViewContext;

/// What the user did in a resource view.
enum DocAction {
    Nav(NavAction),
    View(crate::views::ViewAction),
    /// Open the resource in its own tab.
    Pin,
    /// Show the resource in its stream.
    Reveal,
    LoadHex,
}

impl BrowserApp {
    /// Draws a pane of the dock.
    pub(super) fn pane_ui(&mut self, ui: &mut egui::Ui, pane: &mut Pane) {
        match pane {
            Pane::Welcome => self.welcome_ui(ui),
            Pane::Files => self.file_list(ui),
            Pane::Details => self.details_ui(ui),
            Pane::Changes => self.changes_pane(ui),
            Pane::Log => self.log_ui(ui),
            Pane::Index => self.index_view(ui),
            Pane::Stream { id, path } => self.stream_pane(ui, *id, path.clone()),
            Pane::Resource { id, key } => self.resource_pane(ui, *id, key),
            Pane::Search { id, query } => self.search_pane(ui, *id, query),
            Pane::Level { id } => self.level_pane(ui, *id),
            Pane::Tasks => self.tasks_ui(ui),
            Pane::Project => self.project_pane(ui),
            Pane::Game => self.game_pane(ui),
            Pane::Wizard { id, kind } => self.wizard_pane(ui, *id, *kind),
            Pane::Logic { id, scope } => self.logic_pane(ui, *id, scope),
        }
    }

    pub(super) fn refilter_files(&mut self) {
        let Some(game) = &self.game else { return };
        let filter = self.file_filter.to_ascii_lowercase();
        let matches = |path: &str| filter.is_empty() || path.to_ascii_lowercase().contains(&filter);
        self.file_rows = game
            .tocs()
            .iter()
            .enumerate()
            .filter(|(_, f)| matches(&f.rel_path))
            .map(|(i, _)| FileSel::Toc(i))
            .chain(
                game.streams()
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| matches(&f.rel_path))
                    .map(|(i, _)| FileSel::Stream(i)),
            )
            .collect();
    }

    pub(super) fn refilter_index(&mut self) {
        let Some(index) = &self.index else { return };
        self.index_rows = match self.index_type {
            None => (0..index.entries.len()).collect(),
            Some(t) => (0..index.entries.len())
                .filter(|&i| index.entries[i].header.type_id == t)
                .collect(),
        };
    }

    // ------------------------------------------------------------- file list

    fn file_list(&mut self, ui: &mut egui::Ui) {
        let Some(game) = self.game.clone() else {
            ui.label(RichText::new(tr("files.no_game")).weak());
            if ui.button(tr("cmd.open_game")).clicked() {
                self.pending.push(super::Command::OpenGame);
            }
            return;
        };
        let edit = egui::TextEdit::singleline(&mut self.file_filter)
            .hint_text(tr("files.filter"))
            .desired_width(f32::INFINITY);
        if ui.add(edit).changed() {
            self.refilter_files();
        }
        ui.label(
            RichText::new(format!(
                "{} · {}",
                trn("count.files", self.file_rows.len() as u64, &[]),
                tr("files.hint")
            ))
            .weak(),
        );
        let mut clicked = None;
        TableBuilder::new(ui)
            .id_salt("files")
            .striped(true)
            .sense(Sense::click())
            .column(Column::remainder().clip(true))
            .column(Column::auto())
            .body(|body| {
                body.rows(ROW_HEIGHT, self.file_rows.len(), |mut row| {
                    let sel = self.file_rows[row.index()];
                    let file = match sel {
                        FileSel::Stream(i) => &game.streams()[i],
                        FileSel::Toc(i) => &game.tocs()[i],
                    };
                    row.set_selected(self.selected_file == Some(sel));
                    row.col(|ui| {
                        let text = if matches!(sel, FileSel::Toc(_)) {
                            RichText::new(&file.rel_path).color(Color32::from_rgb(0xd0, 0xa0, 0x40))
                        } else {
                            RichText::new(&file.rel_path)
                        };
                        ui.add(egui::Label::new(text).truncate().selectable(false));
                    });
                    row.col(|ui| {
                        ui.add(
                            egui::Label::new(RichText::new(human_size(file.len)).weak())
                                .selectable(false),
                        );
                    });
                    let response = row.response();
                    let new_tab =
                        response.middle_clicked() || (response.clicked() && ui_command(&response));
                    if new_tab {
                        clicked = Some((sel, true));
                    } else if response.clicked() {
                        clicked = Some((sel, false));
                    }
                    response.context_menu(|ui| {
                        if ui.button(tr("files.open_new_tab")).clicked() {
                            clicked = Some((sel, true));
                        }
                    });
                });
            });
        if let Some((sel, new_tab)) = clicked {
            self.open_file(sel, new_tab);
        }
    }

    // ----------------------------------------------------------- stream tabs

    fn stream_pane(&mut self, ui: &mut egui::Ui, id: u64, path: Option<String>) {
        self.streams.entry(id).or_insert_with(|| StreamTab {
            restore: path,
            ..StreamTab::default()
        });
        // A tab of a saved layout opens its file once the game is open.
        if let Some(game) = self.game.clone()
            && let Some(tab) = self.streams.get_mut(&id)
            && tab.sel.is_none()
            && let Some(path) = tab.restore.take()
        {
            match find_file(&game, &path) {
                Some(sel) => self.show_file(id, sel),
                None => {
                    ui.label(trf("browse.not_in_game", &[("path", &path)]));
                    return;
                }
            }
        }
        let Some(tab) = self.streams.get(&id) else {
            return;
        };
        if self.game.is_none() {
            ui.label(RichText::new(tr("files.no_game")).weak());
        } else if tab.stream.is_some() {
            self.stream_view(ui, id);
        } else if tab.toc.is_some() {
            self.toc_view(ui, id);
        } else if self.files.is_released() {
            ui.label(RichText::new(tr("status.files_written")).weak());
        } else {
            ui.label(RichText::new(tr("stream.choose")).weak());
        }
    }

    fn stream_view(&mut self, ui: &mut egui::Ui, id: u64) {
        let Some(game) = self.game.clone() else {
            return;
        };
        let Some(stream_index) = self
            .streams
            .get(&id)
            .and_then(|t| t.stream.as_ref())
            .map(|s| s.game_index)
        else {
            return;
        };
        let file = match self.files.get(&game, stream_index) {
            Ok(file) => file,
            Err(error) => {
                ui.label(RichText::new(error).weak());
                return;
            }
        };
        let layout = file.layout();
        let tab = self.streams.get(&id).expect("checked above");
        let stream = tab.stream.as_ref().expect("checked above");
        ui.horizontal(|ui| {
            ui.strong(&game.streams()[stream_index].rel_path);
            ui.label(trf(
                "browse.stream_summary",
                &[
                    (
                        "resources",
                        &trn("count.resources", layout.resources.len() as u64, &[]),
                    ),
                    ("data", &human_size(layout.total_data_size())),
                    ("disk", &human_size(layout.file_len as u64)),
                ],
            ));
        });
        let mut new_filter = None;
        let mut extract_all = false;
        ui.horizontal(|ui| {
            let current = stream
                .type_filter
                .map_or(tr("browse.all_types").to_owned(), |t| {
                    type_tags().display(t)
                });
            egui::ComboBox::from_id_salt("stream-type")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(stream.type_filter.is_none(), tr("browse.all_types"))
                        .clicked()
                    {
                        new_filter = Some(None);
                    }
                    for &(t, count) in &stream.types {
                        let label = format!("{} ({count})", type_tags().display(t));
                        if ui
                            .selectable_label(stream.type_filter == Some(t), label)
                            .clicked()
                        {
                            new_filter = Some(Some(t));
                        }
                    }
                });
            extract_all = ui
                .add_enabled(
                    self.job.is_none(),
                    egui::Button::new(trf(
                        "browse.extract_shown",
                        &[("count", &stream.rows.len())],
                    )),
                )
                .on_hover_text(tr("browse.extract_shown_hint"))
                .clicked();
            self.nav.export_selected_button(ui, &game);
        });
        if let Some(filter) = new_filter
            && let Some(stream) = self.streams.get_mut(&id).and_then(|t| t.stream.as_mut())
        {
            stream.set_filter(&file, filter);
        }
        if extract_all {
            let mut dialog = rfd::FileDialog::new().set_title(tr("browse.extract_into"));
            if let Some(dir) = &self.settings.export_dirs.extract {
                dialog = dialog.set_directory(dir);
            }
            if let Some(dir) = dialog.pick_folder()
                && let Some(stream) = self.streams.get(&id).and_then(|t| t.stream.as_ref())
            {
                self.settings.export_dirs.extract = Some(dir.clone());
                self.job = Some(jobs::extract(
                    file.clone(),
                    stream.rows.clone(),
                    dir,
                    ui.ctx(),
                    &self.tx,
                ));
            }
        }

        let tab = self.streams.get_mut(&id).expect("checked above");
        let scroll_to = tab.scroll_to.take();
        let selected = tab.selected;
        let stream = tab.stream.as_ref().expect("checked above");
        let mut clicked = None;
        let mut pin = None;
        let modifiers = ui.input(|i| i.modifiers);
        let picked = |resource: usize| Location {
            stream: stream_index,
            resource,
        };
        let multi = self.nav.selected.len() > 1;
        let mut table = TableBuilder::new(ui)
            .id_salt("resources")
            .striped(true)
            .sense(Sense::click())
            .column(Column::exact(48.0))
            .column(Column::initial(120.0).resizable(true).clip(true))
            .column(Column::initial(70.0).resizable(true).clip(true))
            .column(Column::exact(76.0))
            .column(Column::exact(76.0))
            .column(Column::exact(76.0))
            .column(Column::remainder().at_least(60.0));
        if let Some(target) = scroll_to
            && let Some(row) = stream.rows.iter().position(|&r| r == target)
        {
            table = table.scroll_to_row(row, Some(Align::Center));
        }
        table
            .header(ROW_HEIGHT + 2.0, |mut header| {
                header_cells(
                    &mut header,
                    &[
                        "#",
                        tr("browse.col_type"),
                        tr("browse.col_policy"),
                        tr("browse.col_id"),
                        tr("browse.col_size"),
                        tr("browse.col_stored"),
                        tr("browse.col_storage"),
                    ],
                );
            })
            .body(|body| {
                body.rows(ROW_HEIGHT, stream.rows.len(), |mut row| {
                    let index = stream.rows[row.index()];
                    let h = &layout.resources[index].header;
                    row.set_selected(
                        selected == Some(index)
                            || (multi && self.nav.selected.contains(&picked(index))),
                    );
                    row.col(|ui| {
                        let label = if stream.changed.contains(&index) {
                            RichText::new(format!("* {index}")).color(crate::changes_view::PENDING)
                        } else {
                            RichText::new(index.to_string())
                        };
                        ui.add(egui::Label::new(label).truncate().selectable(false))
                            .on_hover_text_at_pointer(if stream.changed.contains(&index) {
                                tr("browse.changed_in_workspace")
                            } else {
                                ""
                            });
                    });
                    let cells = [
                        type_tags().display(h.type_id),
                        mem_policies().display(h.mem_policy),
                        format!("{:08x}", h.ids[3]),
                        h.data_size.to_string(),
                        layout.stored_size(index).to_string(),
                        if layout.is_compressed(index) {
                            "RefPack".to_owned()
                        } else {
                            "raw".to_owned()
                        },
                    ];
                    for cell in cells {
                        row.col(|ui| {
                            ui.add(egui::Label::new(cell).truncate().selectable(false));
                        });
                    }
                    let response = row.response();
                    if response.double_clicked() || response.middle_clicked() {
                        pin = Some(index);
                    } else if response.clicked() {
                        clicked = Some(index);
                    }
                    response.context_menu(|ui| {
                        if ui.button(tr("doc.open_tab")).clicked() {
                            pin = Some(index);
                        }
                    });
                });
            });
        if let Some(index) = clicked {
            let rows: Vec<Location> = stream.rows.iter().map(|&r| picked(r)).collect();
            if self.nav.pick(picked(index), modifiers, &rows) {
                self.nav.visit(picked(index));
                self.select_resource(id, Some(index));
            }
        }
        if let Some(index) = pin {
            self.focus_stream = Some(id);
            self.open_resource_tab(picked(index));
        }
    }

    fn toc_view(&mut self, ui: &mut egui::Ui, id: u64) {
        let Some(game) = self.game.clone() else {
            return;
        };
        let Some((toc_index, table)) = self.streams.get(&id).and_then(|t| t.toc.as_ref()) else {
            return;
        };
        ui.strong(&game.tocs()[*toc_index].rel_path);
        let table = match table {
            Ok(table) => table,
            Err(error) => {
                ui.colored_label(Color32::RED, error);
                return;
            }
        };
        ui.label(trf(
            "browse.toc_summary",
            &[
                (
                    "streams",
                    &trn("count.streams", table.files.len() as u64, &[]),
                ),
                ("names", &trn("count.names", table.names.len() as u64, &[])),
            ],
        ));
        let mut open = None;
        TableBuilder::new(ui)
            .id_salt("toc")
            .striped(true)
            .sense(Sense::click())
            .column(Column::exact(80.0))
            .column(Column::exact(100.0))
            .column(Column::exact(70.0))
            .column(Column::initial(380.0).resizable(true))
            .column(Column::remainder())
            .header(ROW_HEIGHT + 2.0, |mut header| {
                header_cells(
                    &mut header,
                    &[
                        tr("browse.col_hash"),
                        tr("browse.col_data_size"),
                        tr("browse.col_flags"),
                        tr("browse.col_stream"),
                        tr("browse.col_depends"),
                    ],
                );
            })
            .body(|body| {
                body.rows(ROW_HEIGHT, table.files.len(), |mut row| {
                    let i = row.index();
                    let file = &table.files[i];
                    let name = table.file_name(i);
                    let deps: Vec<String> = table.file_deps(i).map(|n| n.name_lossy()).collect();
                    let cells = [
                        format!("{:08x}", name.hash),
                        file.data_size.to_string(),
                        file.flags.map(|f| f.to_string()).join(","),
                        name.name_lossy(),
                        deps.join(", "),
                    ];
                    for cell in cells {
                        row.col(|ui| {
                            ui.add(egui::Label::new(cell).truncate().selectable(false));
                        });
                    }
                    if row.response().double_clicked() {
                        open = game.find_stream(&name.name_lossy());
                    }
                });
            });
        ui.label(RichText::new(tr("browse.toc_hint")).weak());
        if let Some(stream) = open {
            self.show_file(id, FileSel::Stream(stream));
            self.selected_file = Some(FileSel::Stream(stream));
        }
    }

    // ---------------------------------------------------------- all resources

    fn index_view(&mut self, ui: &mut egui::Ui) {
        let (Some(game), Some(index)) = (self.game.clone(), self.index.clone()) else {
            ui.label(RichText::new(tr("files.no_game")).weak());
            return;
        };
        let mut new_type = None;
        ui.horizontal(|ui| {
            let current = self
                .index_type
                .map_or(tr("browse.all_types").to_owned(), |t| {
                    type_tags().display(t)
                });
            egui::ComboBox::from_id_salt("index-type")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(self.index_type.is_none(), tr("browse.all_types"))
                        .clicked()
                    {
                        new_type = Some(None);
                    }
                    for t in &self.type_summary {
                        let label = format!(
                            "{} ({}, {})",
                            type_tags().display(t.type_id),
                            t.count,
                            human_size(t.data_bytes)
                        );
                        if ui
                            .selectable_label(self.index_type == Some(t.type_id), label)
                            .clicked()
                        {
                            new_type = Some(Some(t.type_id));
                        }
                    }
                });
            ui.label(trn("count.resources", self.index_rows.len() as u64, &[]));
            let shown: Vec<Location> = self
                .index_rows
                .iter()
                .map(|&i| Location {
                    stream: index.entries[i].stream as usize,
                    resource: index.entries[i].resource as usize,
                })
                .collect();
            if ui
                .add_enabled(
                    !shown.is_empty(),
                    egui::Button::new(trf("browse.export_shown", &[("count", &shown.len())])),
                )
                .on_hover_text(tr("browse.export_shown_hint"))
                .clicked()
            {
                self.nav.export(&game, shown);
            }
            self.nav.export_selected_button(ui, &game);
        });
        if let Some(t) = new_type {
            self.index_type = t;
            self.refilter_index();
        }
        let mut open = None;
        let mut clicked = None;
        let modifiers = ui.input(|i| i.modifiers);
        let location = |row: usize| {
            let entry = &index.entries[row];
            Location {
                stream: entry.stream as usize,
                resource: entry.resource as usize,
            }
        };
        TableBuilder::new(ui)
            .id_salt("index")
            .striped(true)
            .sense(Sense::click())
            .column(Column::initial(360.0).resizable(true).clip(true))
            .column(Column::exact(56.0))
            .column(Column::initial(150.0).resizable(true))
            .column(Column::exact(90.0))
            .column(Column::exact(90.0))
            .column(Column::remainder())
            .header(ROW_HEIGHT + 2.0, |mut header| {
                header_cells(
                    &mut header,
                    &[
                        tr("browse.col_stream"),
                        "#",
                        tr("browse.col_type"),
                        tr("browse.col_size"),
                        tr("browse.col_stored"),
                        tr("browse.col_id"),
                    ],
                );
            })
            .body(|body| {
                body.rows(ROW_HEIGHT, self.index_rows.len(), |mut row| {
                    let entry = &index.entries[self.index_rows[row.index()]];
                    row.set_selected(
                        self.nav
                            .selected
                            .contains(&location(self.index_rows[row.index()])),
                    );
                    let cells = [
                        game.streams()[entry.stream as usize].rel_path.clone(),
                        entry.resource.to_string(),
                        type_tags().display(entry.header.type_id),
                        entry.header.data_size.to_string(),
                        entry.stored_size.to_string(),
                        format!("{:08x}", entry.header.ids[3]),
                    ];
                    for cell in cells {
                        row.col(|ui| {
                            ui.add(egui::Label::new(cell).truncate().selectable(false));
                        });
                    }
                    if row.response().double_clicked() {
                        open = Some((entry.stream as usize, entry.resource as usize));
                    } else if row.response().clicked() {
                        clicked = Some(self.index_rows[row.index()]);
                    }
                });
            });
        ui.label(RichText::new(tr("browse.index_hint")).weak());
        if let Some(row) = clicked {
            let rows: Vec<Location> = self.index_rows.iter().map(|&r| location(r)).collect();
            self.nav.pick(location(row), modifiers, &rows);
        }
        if let Some((stream, resource)) = open {
            let target = Location { stream, resource };
            self.nav.visit(target);
            self.open_location(target);
        }
    }

    // ------------------------------------------------------------ search tabs

    fn search_pane(&mut self, ui: &mut egui::Ui, id: u64, query: &mut String) {
        let Some(game) = self.game.clone() else {
            ui.label(RichText::new(tr("files.no_game")).weak());
            return;
        };
        let search = self
            .searches
            .entry(id)
            .or_insert_with(|| Search::new(query.clone()));
        if search.pending && self.nav.index.is_some() {
            search.run(&mut self.nav);
        }
        if ui.rect_contains_pointer(ui.max_rect()) && ui.input(|i| i.pointer.any_pressed()) {
            self.focus_search = Some(id);
        }
        let action = search.view(ui, &mut self.nav, &game);
        query.clone_from(&search.query);
        if let Some(action) = action {
            self.focus_search = Some(id);
            self.nav_action(action);
        }
    }

    /// Shows a resource in its stream: in the stream tab used last, or a new one.
    pub(super) fn open_location(&mut self, target: Location) {
        self.open_file(FileSel::Stream(target.stream), false);
        let Some(id) = self.focus_stream else { return };
        let Some(game) = self.game.clone() else {
            return;
        };
        if let Some(tab) = self.streams.get_mut(&id)
            && let Some(stream) = &mut tab.stream
            && !stream.rows.contains(&target.resource)
            && let Ok(file) = self.files.get(&game, target.stream)
        {
            stream.set_filter(&file, None);
        }
        self.select_resource(id, Some(target.resource));
        if let Some(tab) = self.streams.get_mut(&id) {
            tab.scroll_to = Some(target.resource);
        }
    }

    // ---------------------------------------------------- details and tabs

    fn details_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut doc) = self.details.take() else {
            ui.label(RichText::new(tr("details.select")).weak());
            return;
        };
        let action = egui::ScrollArea::vertical()
            .id_salt("details scroll")
            .auto_shrink(false)
            .show(ui, |ui| self.doc_ui(ui, &mut doc, false))
            .inner;
        // An action may load the doc again: put it back first.
        if self.details.is_none() {
            self.details = Some(doc);
        }
        if let Some(action) = action {
            let loc = self.details.as_ref().map(|d| d.loc);
            self.doc_action(loc, action);
        }
    }

    fn resource_pane(&mut self, ui: &mut egui::Ui, id: u64, key: &str) {
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
        let action = egui::ScrollArea::vertical()
            .id_salt(("resource scroll", id))
            .auto_shrink(false)
            .show(ui, |ui| self.doc_ui(ui, &mut doc, true))
            .inner;
        let loc = doc.loc;
        self.docs.entry(id).or_insert(doc);
        if let Some(action) = action {
            self.doc_action(Some(loc), action);
        }
    }

    /// The resource with a workspace key in this game.
    fn locate_key(&mut self, key: &str) -> Option<Location> {
        let game = self.game.clone()?;
        let key: doctor_toolkit::workspace::ResourceKey = key.parse().ok()?;
        let stream = game.find_stream(&key.stream)?;
        let file = self.files.get(&game, stream).ok()?;
        let resource = key.find_in(file.layout())?;
        Some(Location { stream, resource })
    }

    fn doc_action(&mut self, loc: Option<Location>, action: DocAction) {
        match action {
            DocAction::Nav(action) => self.nav_action(action),
            DocAction::View(action) => {
                if let Some(loc) = loc {
                    self.view_action(loc, action);
                }
            }
            DocAction::Pin => {
                if let Some(loc) = loc {
                    self.open_resource_tab(loc);
                }
            }
            DocAction::Reveal => {
                if let Some(loc) = loc {
                    self.nav.visit(loc);
                    self.open_location(loc);
                }
            }
            DocAction::LoadHex => {}
        }
    }

    /// Title of a resource tab: type, name or id.
    pub(super) fn doc_title(&self, doc: &ResourceDoc) -> String {
        let name = self.nav.index.as_ref().and_then(|index| {
            let id = index.locate(doc.loc.stream, doc.loc.resource)?;
            index.record(id).name().map(str::to_owned)
        });
        let kind = type_tags().display(doc.type_id);
        match name {
            Some(name) => format!("{kind} {name}"),
            None => format!("{kind} #{}", doc.loc.resource),
        }
    }

    /// Header, references and views of a resource.
    fn doc_ui(
        &mut self,
        ui: &mut egui::Ui,
        doc: &mut ResourceDoc,
        pinned: bool,
    ) -> Option<DocAction> {
        let game = self.game.clone()?;
        if let Some(error) = &doc.error {
            ui.colored_label(Color32::RED, error);
            if self.files.is_released() {
                return None;
            }
            if ui.button(tr("doc.retry")).clicked() {
                *doc = self.load_doc(doc.loc, None);
            }
            return None;
        }
        let file = match self.files.get(&game, doc.loc.stream) {
            Ok(file) => file,
            Err(error) => {
                ui.label(RichText::new(error).weak());
                return None;
            }
        };
        let index = doc.loc.resource;
        let layout = file.layout();
        let Some(h) = layout.resources.get(index).map(|r| r.header) else {
            ui.colored_label(Color32::RED, tr("doc.not_found"));
            return None;
        };
        let mut action = None;
        ui.horizontal(|ui| {
            ui.heading(trf("browse.resource", &[("index", &index)]));
            if pinned {
                ui.label(RichText::new(&game.streams()[doc.loc.stream].rel_path).weak());
                if ui
                    .small_button(tr("doc.reveal"))
                    .on_hover_text(tr("doc.reveal_hint"))
                    .clicked()
                {
                    action = Some(DocAction::Reveal);
                }
            } else if ui
                .small_button(tr("doc.open_tab"))
                .on_hover_text(tr("doc.open_tab_hint"))
                .clicked()
            {
                action = Some(DocAction::Pin);
            }
        });
        egui::Grid::new("details")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                let rows = [
                    (
                        tr("browse.col_type"),
                        format!("{} ({:08x})", type_tags().display(h.type_id), h.type_id),
                    ),
                    (
                        tr("browse.memory_policy"),
                        format!(
                            "{} ({:08x})",
                            mem_policies().display(h.mem_policy),
                            h.mem_policy
                        ),
                    ),
                    (tr("browse.alignment"), format!("{:#x}", h.alignment)),
                    (
                        tr("browse.ids"),
                        h.ids.map(|id| format!("{id:08x}")).join(" "),
                    ),
                    (
                        tr("browse.col_data_size"),
                        format!("{} ({})", h.data_size, human_size(h.data_size as u64)),
                    ),
                    (
                        tr("browse.stored_size"),
                        layout.stored_size(index).to_string(),
                    ),
                    (
                        tr("browse.blocks"),
                        layout.data_blocks(index).count().to_string(),
                    ),
                    (
                        tr("browse.header_offset"),
                        format!(
                            "{:#x}",
                            layout.blocks[layout.resources[index].header_block].offset
                        ),
                    ),
                ];
                for (name, value) in rows {
                    ui.label(name);
                    ui.monospace(value);
                    ui.end_row();
                }
            });
        ui.horizontal(|ui| {
            if ui.button(tr("browse.save_data")).clicked() {
                let mut dialog =
                    rfd::FileDialog::new().set_file_name(jobs::resource_file_name(&file, index));
                if let Some(dir) = &self.settings.export_dirs.export {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(path) = dialog.save_file() {
                    let result = file
                        .resource_data(index)
                        .map_err(|e| e.to_string())
                        .and_then(|data| std::fs::write(&path, data).map_err(|e| e.to_string()));
                    match result {
                        Ok(()) => self.status = trf("browse.saved", &[("path", &path.display())]),
                        Err(error) => self.notify(
                            super::Level::Error,
                            trf("browse.cannot_save", &[("error", &error)]),
                        ),
                    }
                }
            }
            if doc.hex.is_none() && ui.button(tr("browse.load_hex")).clicked() {
                action = Some(DocAction::LoadHex);
            }
        });
        if matches!(action, Some(DocAction::LoadHex)) {
            self.load_hex(doc);
        }
        if let Some(nav) = self.nav.refs_panel(ui, doc.loc) {
            return Some(DocAction::Nav(nav));
        }
        ui.separator();
        self.model_import_button(ui, doc.loc, doc.type_id);
        let mut cx = ViewContext {
            status: &mut self.status,
            links: &self.external.infos,
        };
        for (_, view) in &mut doc.views {
            if let Some(a) = view.show(ui, &mut cx) {
                action = Some(DocAction::View(a));
            }
            ui.separator();
        }
        if let Some(action @ DocAction::View(_)) = action {
            return Some(action);
        }
        match &doc.hex {
            Some(Ok(data)) => {
                hex::show(ui, egui::IdSalt::new(("hex", file.path(), index)), data);
            }
            Some(Err(error)) => {
                ui.colored_label(Color32::RED, error);
            }
            None if doc.hides_hex() => {}
            None => {
                ui.label(RichText::new(tr("browse.too_large")).weak());
            }
        }
        action
    }
}

/// Whether a click asks for a new tab (Ctrl or Cmd held).
fn ui_command(response: &egui::Response) -> bool {
    response.ctx.input(|i| i.modifiers.command)
}

/// Fills a header row with titles that are truncated instead of wrapped.
fn header_cells(header: &mut egui_extras::TableRow<'_, '_>, titles: &[&str]) {
    for title in titles {
        header.col(|ui| {
            ui.add(
                egui::Label::new(RichText::new(*title).strong())
                    .truncate()
                    .selectable(false),
            );
        });
    }
}

impl BrowserApp {
    /// Opens the "All resources" tab.
    pub(super) fn show_index(&mut self) {
        self.dock_ops.push(DockOp::ShowPanel(Pane::Index));
    }
}
