//! Details of game-logic resources: a SimGroup object browser and editor,
//! and a JSON tree for the other types (`metamodel`, `variabledictionary`,
//! `partsdb`, `graph`).
//!
//! Edits are not written here: [`LogicView::show`] returns the new data of
//! the resource and the app records it in the workspace, then loads the view
//! again from the workspace, with the game's original for comparison.

use std::collections::{HashMap, HashSet};

use doctor_toolkit::game::StreamFile;
use doctor_toolkit::resource::logic::{self, TYPE_SIMGROUP};
use doctor_toolkit::resource::logic_import;
use doctor_toolkit::resource::names::sim_classes;
use doctor_toolkit::resource::simgroup::{
    ATTR_LOAD_MATRIX, CLASS_SYSTEM_COMMANDS, Guid, Kind, Schema, SimGroup, Value,
};
use doctor_toolkit::resource::simgroup_edit::{self, Transform, edit_text, parse_value};
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{Color32, RichText};

use crate::changes_view::{CONFLICT, PENDING};

/// Arrays longer than this are shown in collapsible chunks.
const CHUNK: usize = 100;

/// What the user asked for.
pub enum LogicAction {
    /// Record new data for the resource.
    Save { data: Vec<u8>, label: String },
    /// Show the SimGroup in the logic graph editor, with this object selected.
    OpenGraph { object: Option<usize> },
}

/// Selection and filter, kept when the view is loaded again after an edit.
pub struct ViewState {
    selected: Option<usize>,
    filter: String,
    show_unset: bool,
}

pub struct LogicView {
    type_id: u32,
    /// The data shown: the workspace version when the resource is edited.
    data: Vec<u8>,
    kind: Kind_,
}

enum Kind_ {
    Sim(Box<SimView>),
    Json(serde_json::Value),
    Error(String),
}

impl LogicView {
    /// Loads resource `index` if it is a game-logic resource.
    pub fn load(stream: &StreamFile, index: usize) -> Option<Self> {
        let type_id = stream.layout().resources[index].header.type_id;
        if !logic::is_supported(type_id) {
            return None;
        }
        let data = stream.resource_data(index).map_err(|e| e.to_string());
        Some(Self::from_data(type_id, data, None))
    }

    /// A view of `data`; `original` is the game's version when `data` is an edit.
    pub fn from_data(
        type_id: u32,
        data: Result<Vec<u8>, String>,
        original: Option<Vec<u8>>,
    ) -> Self {
        let data = match data {
            Ok(data) => data,
            Err(error) => {
                return Self {
                    type_id,
                    data: Vec::new(),
                    kind: Kind_::Error(error),
                };
            }
        };
        let kind = if type_id == TYPE_SIMGROUP {
            match SimGroup::parse(&data) {
                Ok(group) => {
                    let original = original.and_then(|d| SimGroup::parse(&d).ok());
                    Kind_::Sim(Box::new(SimView::new(group, original)))
                }
                Err(error) => Kind_::Error(error.to_string()),
            }
        } else {
            match logic::to_json(type_id, &data).expect("supported type") {
                Ok(json) => Kind_::Json(json),
                Err(error) => Kind_::Error(error.to_string()),
            }
        };
        Self {
            type_id,
            data,
            kind,
        }
    }

    /// The selection, to carry over to a view loaded after an edit.
    pub fn state(&self) -> Option<ViewState> {
        match &self.kind {
            Kind_::Sim(view) => Some(ViewState {
                selected: view.selected,
                filter: view.filter.clone(),
                show_unset: view.show_unset,
            }),
            _ => None,
        }
    }

    pub fn restore(&mut self, state: ViewState) {
        if let Kind_::Sim(view) = &mut self.kind {
            view.filter = state.filter;
            view.refilter();
            view.selected = state.selected.filter(|&s| s < view.rows.len());
            view.scroll_to_selected = true;
            view.show_unset = state.show_unset;
        }
    }

    /// Selects SimGroup object `index` and scrolls the list to it.
    pub fn select_object(&mut self, index: usize) {
        if let Kind_::Sim(view) = &mut self.kind
            && index < view.rows.len()
        {
            if !view.visible.contains(&index) {
                view.filter.clear();
                view.refilter();
            }
            view.select(Some(index));
            view.scroll_to_selected = true;
        }
    }

    /// The selected SimGroup object.
    pub fn selected_object(&self) -> Option<usize> {
        match &self.kind {
            Kind_::Sim(view) => view.selected,
            _ => None,
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, status: &mut String) -> Option<LogicAction> {
        let mut action = self.json_buttons(ui, status);
        match &mut self.kind {
            Kind_::Sim(view) => action = action.or(view.show(ui)),
            Kind_::Json(json) => {
                egui::ScrollArea::both()
                    .id_salt("logic-json")
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        json_tree(
                            ui,
                            "resource",
                            json,
                            ui.scope_id().with("logic-json-root"),
                            true,
                        );
                    });
            }
            Kind_::Error(error) => {
                ui.colored_label(Color32::RED, error.as_str());
            }
        }
        action
    }

    /// Export to and import from the JSON form of `logic-export`.
    fn json_buttons(&self, ui: &mut egui::Ui, status: &mut String) -> Option<LogicAction> {
        if matches!(self.kind, Kind_::Error(_)) {
            return None;
        }
        let mut action = None;
        ui.horizontal(|ui| {
            if let Kind_::Sim(view) = &self.kind
                && ui
                    .button(tr("logic.open_from_level"))
                    .on_hover_text(tr("logic.open_from_group_hint"))
                    .clicked()
            {
                action = Some(LogicAction::OpenGraph {
                    object: view.selected,
                });
            }
            if ui.button(tr("logic.export_json")).clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("JSON", &["json"])
                    .set_file_name("resource.json")
                    .save_file()
            {
                let text = logic::to_json(self.type_id, &self.data)
                    .expect("supported type")
                    .map_err(|e| e.to_string())
                    .and_then(|j| serde_json::to_string_pretty(&j).map_err(|e| e.to_string()));
                *status = match text
                    .and_then(|text| std::fs::write(&path, text + "\n").map_err(|e| e.to_string()))
                {
                    Ok(()) => trf("logic.saved_file", &[("path", &path.display())]),
                    Err(error) => trf("logic.cannot_export", &[("error", &error)]),
                };
            }
            if logic_import::is_supported(self.type_id)
                && ui
                    .button(tr("logic.import_json"))
                    .on_hover_text(tr("logic.import_json_hint"))
                    .clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("JSON", &["json"])
                    .pick_file()
            {
                let result = std::fs::read_to_string(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
                    .and_then(|doc| {
                        logic_import::apply_json(self.type_id, &self.data, &doc)
                            .map_err(|e| e.to_string())
                    });
                match result {
                    Ok(imported) if imported.changes.is_empty() => {
                        *status = trf("logic.import_no_changes", &[("path", &path.display())]);
                    }
                    Ok(imported) => {
                        let label = trf(
                            "logic.import_label",
                            &[
                                (
                                    "file",
                                    &path.file_name().unwrap_or_default().to_string_lossy(),
                                ),
                                (
                                    "values",
                                    &trn("count.values", imported.changes.len() as u64, &[]),
                                ),
                            ],
                        );
                        action = Some(LogicAction::Save {
                            data: imported.data,
                            label,
                        });
                    }
                    Err(error) => *status = trf("logic.cannot_import", &[("error", &error)]),
                }
            }
        });
        action
    }
}

/// Precomputed text of one object row.
struct Row {
    class: String,
    name: String,
    position: String,
    /// Lower-case text the filter matches against.
    search: String,
}

/// Address of one attribute: object, packet slot, attribute index.
type AttrKey = (usize, usize, u16);

struct SimView {
    group: SimGroup,
    rows: Vec<Row>,
    by_guid: HashMap<Guid, usize>,
    filter: String,
    visible: Vec<usize>,
    selected: Option<usize>,
    scroll_to_selected: bool,
    transforms: usize,
    /// Objects that differ from the game's original.
    edited: HashSet<usize>,
    /// Attributes that differ from the game's original.
    edited_attrs: HashSet<AttrKey>,
    /// Text typed into value fields and not applied yet.
    drafts: HashMap<AttrKey, String>,
    errors: HashMap<AttrKey, String>,
    /// The transform being edited: object and values.
    transform: Option<(usize, Transform)>,
    show_unset: bool,
}

impl SimView {
    fn new(group: SimGroup, original: Option<SimGroup>) -> Self {
        let rows: Vec<Row> = group
            .objects
            .iter()
            .map(|o| {
                let class = o
                    .class_name()
                    .map_or_else(|| format!("{:08x}", o.class), str::to_owned);
                let name = group.object_name(o).unwrap_or_default().to_owned();
                let position = group
                    .transform(o)
                    .map(|m| format!("{:.1} {:.1} {:.1}", m[3][0], m[3][1], m[3][2]))
                    .unwrap_or_default();
                let search = format!("{class} {name} {}", o.guid).to_lowercase();
                Row {
                    class,
                    name,
                    position,
                    search,
                }
            })
            .collect();
        let by_guid = group
            .objects
            .iter()
            .enumerate()
            .map(|(i, o)| (o.guid, i))
            .collect();
        let transforms = group
            .objects
            .iter()
            .filter(|o| group.transform(o).is_some())
            .count();
        let (mut edited, mut edited_attrs) = (HashSet::new(), HashSet::new());
        if let Some(original) = &original {
            let (attrs, other) = simgroup_edit::compare(original, &group);
            for a in attrs {
                edited.insert(a.object);
                edited_attrs.insert((a.object, a.slot, a.index));
            }
            edited.extend(other);
        }
        let visible = (0..rows.len()).collect();
        Self {
            group,
            rows,
            by_guid,
            filter: String::new(),
            visible,
            selected: None,
            scroll_to_selected: false,
            transforms,
            edited,
            edited_attrs,
            drafts: HashMap::new(),
            errors: HashMap::new(),
            transform: None,
            show_unset: false,
        }
    }

    fn refilter(&mut self) {
        let needle = self.filter.to_lowercase();
        self.visible = (0..self.rows.len())
            .filter(|&i| needle.is_empty() || self.rows[i].search.contains(&needle))
            .collect();
    }

    fn select(&mut self, object: Option<usize>) {
        if object != self.selected {
            self.selected = object;
            self.drafts.clear();
            self.errors.clear();
            self.transform = None;
        }
    }

    fn show(&mut self, ui: &mut egui::Ui) -> Option<LogicAction> {
        let edited = match self.edited.len() {
            0 => String::new(),
            n => trf("logic.edited_count", &[("n", &n)]),
        };
        ui.label(trf(
            "logic.group_summary",
            &[
                (
                    "objects",
                    &trn("count.objects", self.group.objects.len() as u64, &[]),
                ),
                (
                    "packets",
                    &trn("count.packets", self.group.packets.len() as u64, &[]),
                ),
                ("transforms", &self.transforms),
                ("group", &format!("{:08x}", self.group.group_hash)),
                ("edited", &edited),
            ],
        ));
        ui.horizontal(|ui| {
            ui.label(tr("logic.filter"));
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.filter).hint_text(tr("logic.filter_hint")),
                )
                .changed()
            {
                self.refilter();
            }
            ui.label(RichText::new(trf("logic.shown", &[("n", &self.visible.len())])).weak());
        });
        let row_height = ui
            .text_style_height(&egui::TextStyle::Monospace)
            .max(ui.spacing().interact_size.y);
        let list_height = (ui.available_height() * 0.45).max(160.0);
        let mut clicked = None;
        let mut area = egui::ScrollArea::vertical()
            .id_salt("simgroup-objects")
            .auto_shrink([false, true])
            .max_height(list_height);
        if self.scroll_to_selected
            && let Some(pos) = self
                .selected
                .and_then(|s| self.visible.iter().position(|&i| i == s))
        {
            area = area
                .vertical_scroll_offset(pos as f32 * (row_height + ui.spacing().item_spacing.y));
            self.scroll_to_selected = false;
        }
        area.show_rows(ui, row_height, self.visible.len(), |ui, range| {
            for &i in &self.visible[range] {
                let row = &self.rows[i];
                let edited = self.edited.contains(&i);
                let mark = if edited { '*' } else { ' ' };
                let mut text = format!("{mark}{i:6}  {:<22} {}", row.class, row.position);
                if !row.name.is_empty() {
                    text = format!("{text}  {}", row.name);
                }
                let mut text = RichText::new(text).monospace();
                if edited {
                    text = text.color(PENDING);
                }
                let label = egui::Button::selectable(self.selected == Some(i), text)
                    .wrap_mode(egui::TextWrapMode::Truncate);
                if ui.add(label).clicked() {
                    clicked = Some(i);
                }
            }
        });
        if clicked.is_some() {
            self.select(clicked);
        }
        ui.separator();
        let Some(selected) = self.selected else {
            ui.label(RichText::new(tr("logic.select_object")).weak());
            return None;
        };
        let mut jump = None;
        let mut action = None;
        egui::ScrollArea::vertical()
            .id_salt("simgroup-object")
            .auto_shrink(false)
            .show(ui, |ui| {
                (jump, action) = self.object_details(ui, selected);
            });
        if let Some(target) = jump {
            if !self.visible.contains(&target) {
                self.filter.clear();
                self.refilter();
            }
            self.select(Some(target));
            self.scroll_to_selected = true;
        }
        action
    }

    /// The edit of one attribute as new resource data; `None` when nothing
    /// changes or the value is refused (the error is shown at the field).
    fn commit(&mut self, key: AttrKey, value: Option<Value>) -> Option<LogicAction> {
        let (object, slot, index) = key;
        let mut group = self.group.clone();
        match group.set_attr(object, slot, index, value) {
            Ok(false) => {
                self.drafts.remove(&key);
                None
            }
            Ok(true) => {
                let packet = &group.packets[group.objects[object].packets[slot]];
                let class = packet
                    .class_name
                    .map_or_else(|| packet.class.to_string(), str::to_owned);
                let name = Schema::gf2()
                    .get(packet.class.0, index)
                    .and_then(|i| i.name)
                    .map_or_else(|| index.to_string(), str::to_owned);
                let label = trf(
                    "logic.set_attr_label",
                    &[
                        ("class", &class),
                        ("name", &name),
                        ("object", &object),
                        ("object_class", &self.rows[object].class),
                    ],
                );
                Some(LogicAction::Save {
                    data: group.encode(),
                    label,
                })
            }
            Err(error) => {
                self.errors.insert(key, error.to_string());
                None
            }
        }
    }

    /// Shows one object; returns the object the user jumped to and an edit.
    fn object_details(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
    ) -> (Option<usize>, Option<LogicAction>) {
        let object = self.group.objects[index].clone();
        let mut jump = None;
        let mut action = None;
        ui.horizontal(|ui| {
            ui.strong(trf(
                "logic.object_title",
                &[("index", &index), ("class", &self.rows[index].class)],
            ));
            if self.edited.contains(&index) {
                ui.label(RichText::new(tr("logic.edited")).color(PENDING))
                    .on_hover_text(tr("logic.edited_hint"));
            }
        });
        egui::Grid::new(("simgroup-object-grid", index))
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                let mut line = |name: &str, value: String| {
                    ui.label(name);
                    ui.monospace(value);
                    ui.end_row();
                };
                line("GUID", object.guid.to_string());
                line(
                    tr("logic.field_class"),
                    format!("{} ({:08x})", self.rows[index].class, object.class),
                );
                if !self.rows[index].name.is_empty() {
                    line(tr("logic.field_name"), self.rows[index].name.clone());
                }
                line(
                    tr("logic.field_flags"),
                    format!("{:#04x} {:#04x}", object.flags, object.flags2),
                );
                if let Some(model) = self.group.model(&object) {
                    line(
                        tr("logic.field_model"),
                        trf("logic.metamodel_id", &[("id", &format!("{model:08x}"))]),
                    );
                }
                for link in &object.links {
                    let text = match link.resource_id() {
                        Some(id) => trf("logic.resource_id", &[("id", &format!("{id:08x}"))]),
                        None => link.to_string(),
                    };
                    line(tr("logic.field_needs"), text);
                }
            });
        if self.group.slot_of(index, CLASS_SYSTEM_COMMANDS).is_ok() {
            action = self.transform_editor(ui, index);
        }
        if let Some(c) = object.components {
            let names: Vec<String> = self.group.packets[c]
                .attrs
                .iter()
                .map(|a| {
                    sim_classes()
                        .get(a.raw)
                        .map_or_else(|| format!("{:08x}", a.raw), str::to_owned)
                })
                .collect();
            ui.label(trf("logic.components", &[("names", &names.join(", "))]));
        }
        ui.checkbox(&mut self.show_unset, tr("logic.show_unset"))
            .on_hover_text(tr("logic.show_unset_hint"));
        for (slot, &p) in object.packets.iter().enumerate() {
            let packet = self.group.packets[p].clone();
            let class = packet
                .class_name
                .map_or_else(|| packet.class.to_string(), str::to_owned);
            let shared = match self.group.packet_users(p) {
                1 => String::new(),
                n => trf(
                    "logic.shared_by",
                    &[("objects", &trn("count.objects", n as u64, &[]))],
                ),
            };
            let header = trf(
                "logic.packet_header",
                &[
                    ("class", &class),
                    ("set", &packet.attrs.len()),
                    ("total", &packet.attr_count),
                    ("shared", &shared),
                ],
            );
            egui::CollapsingHeader::new(header)
                .id_salt(("simgroup-packet", index, slot))
                .default_open(!packet.attrs.is_empty())
                .show(ui, |ui| {
                    egui::Grid::new(("simgroup-attrs", index, slot))
                        .num_columns(4)
                        .striped(true)
                        .show(ui, |ui| {
                            for attr in 0..packet.attr_count {
                                let current = packet.attrs.iter().find(|a| a.index == attr);
                                if current.is_none() && !self.show_unset {
                                    continue;
                                }
                                let key = (index, slot, attr);
                                let kind = self
                                    .group
                                    .attr_kind(packet.class.0, attr)
                                    .or_else(|| current.and_then(|a| scalar_kind(&a.value)));
                                let name = Schema::gf2()
                                    .get(packet.class.0, attr)
                                    .and_then(|i| i.name)
                                    .unwrap_or("");
                                let mut number = RichText::new(attr.to_string()).monospace();
                                if self.edited_attrs.contains(&key) {
                                    number = number.color(PENDING);
                                }
                                ui.label(number);
                                ui.label(name);
                                ui.label(RichText::new(kind.map_or("?", kind_name)).weak());
                                ui.horizontal(|ui| {
                                    let value = current.map(|a| &a.value);
                                    let (target, edit) =
                                        self.value_cell(ui, key, packet.class.0, kind, value);
                                    jump = jump.or(target);
                                    action = action.take().or(edit);
                                });
                                ui.end_row();
                            }
                        });
                });
        }
        (jump, action)
    }

    /// The value of one attribute as an edit field, with a link to the
    /// object a GUID refers to. Enter or leaving the field applies the text.
    fn value_cell(
        &mut self,
        ui: &mut egui::Ui,
        key: AttrKey,
        class: u32,
        kind: Option<Kind>,
        current: Option<&Value>,
    ) -> (Option<usize>, Option<LogicAction>) {
        if class == CLASS_SYSTEM_COMMANDS && key.2 == ATTR_LOAD_MATRIX {
            ui.label(RichText::new(tr("logic.see_transform")).weak());
            return (None, None);
        }
        let Some(kind) = kind else {
            ui.monospace(current.map_or_else(String::new, |v| v.to_string()))
                .on_hover_text(tr("logic.unknown_kind"));
            return (None, None);
        };
        let mut jump = None;
        let mut action = None;
        let shown = current.map_or_else(String::new, edit_text);
        let draft = self.drafts.entry(key).or_insert_with(|| shown.clone());
        let width = match kind {
            Kind::Matrix | Kind::Guid | Kind::String => 260.0,
            _ => 140.0,
        };
        let mut edit = egui::TextEdit::singleline(draft)
            .desired_width(width)
            .font(egui::TextStyle::Monospace)
            .hint_text(tr("logic.default"));
        if self.errors.contains_key(&key) {
            edit = edit.text_color(CONFLICT);
        }
        let hint = match kind {
            Kind::Guid => tr("logic.hint_guid"),
            Kind::Vec3 => tr("logic.hint_vec3"),
            Kind::Matrix => tr("logic.hint_matrix"),
            Kind::Hash => tr("logic.hint_hash"),
            _ => tr("logic.hint_value"),
        };
        let response = ui.add(edit).on_hover_text(hint);
        if response.changed() {
            self.errors.remove(&key);
        }
        if response.lost_focus() {
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.drafts.insert(key, shown.clone());
                self.errors.remove(&key);
            } else if self.drafts.get(&key) != Some(&shown) {
                let text = self.drafts[&key].clone();
                match parse_value(kind, &text) {
                    Ok(value) => action = self.commit(key, value),
                    Err(error) => {
                        self.errors.insert(key, error.to_string());
                    }
                }
            }
        }
        if let Some(error) = self.errors.get(&key) {
            ui.colored_label(CONFLICT, error);
        }
        if let Some(Value::Guid(g)) = current {
            if let Some(&target) = self.by_guid.get(g) {
                let text = format!("→ {}", self.rows[target].class);
                if ui.link(text).on_hover_text(g.to_string()).clicked() {
                    jump = Some(target);
                }
            } else if let Some(id) = g.resource_id() {
                ui.label(
                    RichText::new(trf("logic.resource_id", &[("id", &format!("{id:08x}"))])).weak(),
                );
            }
        }
        (jump, action)
    }

    /// Position, rotation and scale of an object with a `CSystemCommands` packet.
    fn transform_editor(&mut self, ui: &mut egui::Ui, index: usize) -> Option<LogicAction> {
        let matrix = self.group.transform(&self.group.objects[index]);
        let current = matrix
            .as_ref()
            .map(Transform::from_matrix)
            .unwrap_or_default();
        if self.transform.as_ref().is_none_or(|(i, _)| *i != index) {
            self.transform = Some((index, current));
        }
        let mut action = None;
        ui.label(RichText::new(tr("logic.transform")).strong())
            .on_hover_text(tr("logic.transform_hint"));
        let (_, draft) = self.transform.as_mut().expect("set above");
        egui::Grid::new(("simgroup-transform", index))
            .num_columns(4)
            .show(ui, |ui| {
                for (name, values, speed) in [
                    (tr("logic.position"), &mut draft.position, 0.1),
                    (tr("logic.rotation"), &mut draft.rotation, 1.0),
                    (tr("logic.scale"), &mut draft.scale, 0.01),
                ] {
                    ui.label(name);
                    for v in values.iter_mut() {
                        ui.add(egui::DragValue::new(v).speed(speed).max_decimals(4));
                    }
                    ui.end_row();
                }
            });
        let draft = *draft;
        let changed = draft != current;
        ui.horizontal(|ui| {
            if matrix.is_none() {
                ui.label(RichText::new(tr("logic.no_matrix")).weak());
            }
            if ui
                .add_enabled(changed, egui::Button::new(tr("logic.apply_transform")))
                .clicked()
            {
                let mut m = matrix.unwrap_or_else(|| Transform::default().to_matrix());
                // A move keeps the stored rotation rows exactly.
                if draft.rotation != current.rotation || draft.scale != current.scale {
                    m = draft.to_matrix();
                }
                m[3][..3].copy_from_slice(&draft.position);
                let mut group = self.group.clone();
                match group.set_transform(index, m) {
                    Ok(true) => {
                        let label = trf(
                            "logic.move_label",
                            &[("index", &index), ("class", &self.rows[index].class)],
                        );
                        action = Some(LogicAction::Save {
                            data: group.encode(),
                            label,
                        });
                    }
                    Ok(false) => {}
                    Err(error) => {
                        ui.colored_label(CONFLICT, error.to_string());
                    }
                }
            }
            if ui
                .add_enabled(changed, egui::Button::new(tr("logic.reset")))
                .clicked()
            {
                self.transform = Some((index, current));
            }
        });
        action
    }
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Int => "int",
        Kind::Float => "float",
        Kind::Hash => "hash",
        Kind::String => "string",
        Kind::Guid => "guid",
        Kind::Vec3 => "vec3",
        Kind::Matrix => "matrix",
    }
}

fn scalar_kind(value: &Value) -> Option<Kind> {
    match value {
        Value::Int(_) => Some(Kind::Int),
        Value::Float(_) => Some(Kind::Float),
        Value::Hash(_) => Some(Kind::Hash),
        _ => None,
    }
}

/// A short label for a JSON object: its `name` field when it has one.
fn summary(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => match map.get("name") {
            Some(serde_json::Value::String(name)) => format!("{name:?}"),
            _ => format!("{{{}}}", map.len()),
        },
        serde_json::Value::Array(items) => format!("[{}]", items.len()),
        other => other.to_string(),
    }
}

/// Shows `value` as a collapsible tree; only expanded nodes are laid out.
fn json_tree(ui: &mut egui::Ui, key: &str, value: &serde_json::Value, id: egui::Id, open: bool) {
    match value {
        serde_json::Value::Object(map) => {
            egui::CollapsingHeader::new(format!("{key}: {}", summary(value)))
                .id_salt(id)
                .default_open(open)
                .show(ui, |ui| {
                    for (k, v) in map {
                        json_tree(ui, k, v, id.with(k), false);
                    }
                });
        }
        serde_json::Value::Array(items) => {
            egui::CollapsingHeader::new(format!("{key}: [{}]", items.len()))
                .id_salt(id)
                .default_open(open)
                .show(ui, |ui| {
                    if items.len() <= CHUNK {
                        for (i, v) in items.iter().enumerate() {
                            json_tree(ui, &i.to_string(), v, id.with(i), false);
                        }
                        return;
                    }
                    for start in (0..items.len()).step_by(CHUNK) {
                        let end = (start + CHUNK).min(items.len());
                        let chunk_id = id.with(("chunk", start));
                        egui::CollapsingHeader::new(format!("[{start}..{end})"))
                            .id_salt(chunk_id)
                            .show(ui, |ui| {
                                for (i, v) in items[start..end].iter().enumerate() {
                                    let i = start + i;
                                    json_tree(ui, &i.to_string(), v, chunk_id.with(i), false);
                                }
                            });
                    }
                });
        }
        scalar => {
            ui.horizontal(|ui| {
                ui.label(format!("{key}:"));
                ui.monospace(scalar.to_string());
            });
        }
    }
}

// ------------------------------------------------------------ view registry

/// Game logic: the SimGroup editor and a JSON tree of the other logic types.
pub const VIEW: crate::views::ViewEntry = crate::views::ViewEntry {
    id: "logic",
    load: load_view,
};

fn load_view(
    source: &mut crate::views::ViewSource<'_>,
) -> Option<Box<dyn crate::views::ResourceViewer>> {
    if !logic::is_supported(source.type_id) {
        return None;
    }
    let view = match source.edited(source.index) {
        Some(data) => LogicView::from_data(source.type_id, data, source.original(source.index)),
        None => LogicView::load(source.stream, source.index)?,
    };
    Some(Box::new(view))
}

impl crate::views::ResourceViewer for LogicView {
    fn show(
        &mut self,
        ui: &mut egui::Ui,
        cx: &mut crate::views::ViewContext<'_>,
    ) -> Option<crate::views::ViewAction> {
        match LogicView::show(self, ui, cx.status)? {
            LogicAction::Save { data, label } => {
                Some(crate::views::ViewAction::SaveData { data, label })
            }
            LogicAction::OpenGraph { object } => {
                Some(crate::views::ViewAction::OpenLogicGraph { object })
            }
        }
    }

    fn hides_hex(&self) -> bool {
        true
    }

    fn state(&self) -> Option<Box<dyn std::any::Any>> {
        LogicView::state(self).map(|s| Box::new(s) as Box<dyn std::any::Any>)
    }

    fn restore(&mut self, state: Box<dyn std::any::Any>) {
        if let Ok(state) = state.downcast::<ViewState>() {
            LogicView::restore(self, *state);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use doctor_toolkit::game::GameInstance;

    use super::*;

    /// An edit made in the view comes back marked when the view is loaded
    /// from the edited data with the original.
    #[test]
    fn edits_are_returned_and_marked() {
        let Some(dir) = std::env::var_os("EARS_GF2_DIR") else {
            return;
        };
        let game = GameInstance::open(dir).unwrap();
        let stream = game
            .open_stream(game.find_stream("gf2/frontend").unwrap())
            .unwrap();
        let index = (0..stream.layout().resources.len())
            .find(|&i| stream.layout().resources[i].header.type_id == TYPE_SIMGROUP)
            .unwrap();
        let original = stream.resource_data(index).unwrap();
        let mut view = LogicView::from_data(TYPE_SIMGROUP, Ok(original.clone()), None);
        let Kind_::Sim(sim) = &mut view.kind else {
            panic!("SimGroup view expected");
        };
        // An object with a stored float attribute.
        let (object, slot, attr) = (0..sim.group.objects.len())
            .find_map(|o| {
                sim.group.objects[o]
                    .packets
                    .iter()
                    .enumerate()
                    .find_map(|(slot, &p)| {
                        sim.group.packets[p]
                            .attrs
                            .iter()
                            .find(|a| matches!(a.value, Value::Float(_)))
                            .map(|a| (o, slot, a.index))
                    })
            })
            .unwrap();
        assert!(sim.commit((object, slot, attr), None).is_some());
        let Some(LogicAction::Save { data, label }) =
            sim.commit((object, slot, attr), Some(Value::Float(123.5)))
        else {
            panic!("an edit expected");
        };
        assert!(label.contains(&format!("object {object}")), "{label}");
        let state = view.state().unwrap();
        let mut edited = LogicView::from_data(TYPE_SIMGROUP, Ok(data), Some(original));
        edited.restore(state);
        let Kind_::Sim(sim) = &edited.kind else {
            panic!("SimGroup view expected");
        };
        assert_eq!(sim.edited, HashSet::from([object]));
        assert!(sim.edited_attrs.contains(&(object, slot, attr)));
        // Kind errors stay at the field.
        let Kind_::Sim(sim) = &mut view.kind else {
            unreachable!()
        };
        let string_attr = (0..sim.group.objects.len()).find_map(|o| {
            sim.group.objects[o]
                .packets
                .iter()
                .enumerate()
                .find_map(|(slot, &p)| {
                    sim.group.packets[p]
                        .attrs
                        .iter()
                        .find(|a| matches!(a.value, Value::String(_)))
                        .map(|a| (o, slot, a.index))
                })
        });
        if let Some(key) = string_attr {
            assert!(sim.commit(key, Some(Value::Int(1))).is_none());
            assert!(sim.errors.contains_key(&key));
        }
        frame_time(&mut edited, 3);
    }

    /// Lays out `view` for `frames` frames without a window; returns the slowest frame.
    fn frame_time(view: &mut LogicView, frames: usize) -> Duration {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 1000.0),
            )),
            ..Default::default()
        };
        (0..frames)
            .map(|_| {
                let start = Instant::now();
                let mut output = ctx.run_ui(input(), |ui| {
                    view.show(ui, &mut String::new());
                });
                // No renderer takes the font atlas.
                output.textures_delta.clear();
                start.elapsed()
            })
            .max()
            .unwrap_or_default()
    }

    /// The largest SimGroup of the game (2 MB, 10 000+ objects) stays responsive.
    #[test]
    fn largest_simgroup_lays_out_quickly() {
        let Some(dir) = std::env::var_os("EARS_GF2_DIR") else {
            return;
        };
        let game = GameInstance::open(dir).unwrap();
        let stream = game
            .open_stream(game.find_stream("gf2/globals_global").unwrap())
            .unwrap();
        let index = stream
            .layout()
            .resources
            .iter()
            .enumerate()
            .filter(|(_, r)| r.header.type_id == TYPE_SIMGROUP)
            .max_by_key(|(_, r)| r.header.data_size)
            .unwrap()
            .0;
        let start = Instant::now();
        let mut view = LogicView::load(&stream, index).unwrap();
        let load = start.elapsed();
        let Kind_::Sim(sim) = &mut view.kind else {
            panic!("SimGroup view expected");
        };
        let busiest = (0..sim.group.objects.len())
            .max_by_key(|&i| sim.group.objects[i].packets.len())
            .unwrap();
        sim.selected = Some(busiest);
        let objects = sim.group.objects.len();
        let frame = frame_time(&mut view, 20);
        eprintln!(
            "simgroup #{index}: {objects} objects, loaded in {load:?}, slowest frame {frame:?}"
        );
        assert!(load < Duration::from_secs(2), "{load:?}");
        assert!(frame < Duration::from_millis(50), "{frame:?}");
    }
}
