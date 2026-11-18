//! Making a new zone in the window (stage 103): the forms of Tools → New
//! stream…, New SimGroup…, World grid cell…, Clone resources… and Add
//! resource from a file…, and the actions behind them.
//!
//! Every action is one function of `doctor_toolkit::zone_ops` (the command
//! line calls the same ones) and one undoable operation of the workspace.
//! [`BrowserApp::zone_action`] records it and refreshes the views; the game
//! they show is staged again, so a new stream is a zone of the level view
//! at once. `DOCTOR_TOOLKIT_GUI_ZONE` runs actions hands-free when the game
//! is opened ([`BrowserApp::zone_preset`], `docs/shell.md`); an action that
//! fails there stops the editor with a non-zero exit code.

use std::collections::HashSet;
use std::path::PathBuf;

use doctor_toolkit::resource::simgroup::Guid;
use doctor_toolkit::zone_ops::{
    self, AddResourceParams, CellInfo, CloneParams, ImportModelParams, NewGroupParams,
    NewStreamParams, StreamLoad, ZoneCellParams,
};
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::{Color32, RichText};

use super::BrowserApp;

/// One action that makes or changes a new zone.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ZoneAction {
    AddStream(NewStreamParams),
    RemoveStream(String),
    AddGroup(NewGroupParams),
    AddCell(ZoneCellParams),
    RemoveCell { level: String, stream: String },
    Clone(CloneParams),
    AddResource(AddResourceParams),
    ImportModel(ImportModelParams),
}

/// The forms of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Form {
    #[default]
    Stream,
    Group,
    Cell,
    Clone,
    Resource,
}

impl Form {
    pub const ALL: [Form; 5] = [
        Form::Stream,
        Form::Group,
        Form::Cell,
        Form::Clone,
        Form::Resource,
    ];

    fn key(self) -> &'static str {
        match self {
            Self::Stream => "zone.form.stream",
            Self::Group => "zone.form.group",
            Self::Cell => "zone.form.cell",
            Self::Clone => "zone.form.clone",
            Self::Resource => "zone.form.resource",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::Stream => "zone.hint.stream",
            Self::Group => "zone.hint.group",
            Self::Cell => "zone.hint.cell",
            Self::Clone => "zone.hint.clone",
            Self::Resource => "zone.hint.resource",
        }
    }

    fn from_slug(text: &str) -> Option<Self> {
        Some(match text {
            "stream" => Self::Stream,
            "simgroup" | "group" => Self::Group,
            "cell" => Self::Cell,
            "clone" => Self::Clone,
            "resource" => Self::Resource,
            _ => return None,
        })
    }
}

/// `X,Y,Z` (or `X/Y/Z`, or spaces) as a point.
pub(crate) fn parse_point(text: &str) -> Result<[f32; 3], String> {
    let parts: Vec<&str> = text
        .split([',', '/', ' '])
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() != 3 {
        return Err(trf("zone.bad_point", &[("text", &text)]));
    }
    let mut out = [0.0; 3];
    for (o, p) in out.iter_mut().zip(parts) {
        *o = p
            .parse()
            .map_err(|_| trf("zone.bad_point", &[("text", &text)]))?;
    }
    Ok(out)
}

/// Resource indices separated by commas, spaces or `+`.
pub(crate) fn parse_indices(text: &str) -> Result<Vec<usize>, String> {
    text.split([',', ' ', '+'])
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| {
            p.parse()
                .map_err(|_| trf("zone.bad_index", &[("text", &p)]))
        })
        .collect()
}

fn text_or_none(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// A text field for a stream name with a menu of the workspace's new streams.
fn stream_field(ui: &mut egui::Ui, salt: &str, value: &mut String, new_streams: &[String]) {
    ui.add(
        egui::TextEdit::singleline(value)
            .id_salt(salt)
            .hint_text("gf2/…")
            .desired_width(280.0),
    );
    if !new_streams.is_empty() {
        ui.menu_button("…", |ui| {
            ui.label(RichText::new(tr("zone.new_streams")).weak().small());
            for name in new_streams {
                if ui.button(name).clicked() {
                    *value = name.clone();
                    ui.close();
                }
            }
        })
        .response
        .on_hover_text(tr("zone.new_streams"));
    }
}

fn row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.label(label);
    ui.horizontal(add);
    ui.end_row();
}

/// "New stream…".
#[derive(Debug, Clone, Default)]
pub(crate) struct StreamForm {
    pub name: String,
    pub parent: String,
    pub load: StreamLoad,
    pub guid: String,
}

impl StreamForm {
    pub fn params(&self) -> NewStreamParams {
        NewStreamParams {
            name: self.name.trim().to_owned(),
            parent: self.parent.trim().to_owned(),
            load: self.load,
            tocs: Vec::new(),
            guid: text_or_none(&self.guid),
        }
    }

    pub fn load_label(load: StreamLoad) -> &'static str {
        tr(match load {
            StreamLoad::OnDemand => "zone.load.on_demand",
            StreamLoad::WithParent => "zone.load.with_parent",
            StreamLoad::AutoLoad => "zone.load.auto_load",
        })
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, salt: &str) {
        egui::Grid::new(("zone-stream", salt))
            .num_columns(2)
            .show(ui, |ui| {
                row(ui, tr("zone.stream.name"), |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.name)
                            .hint_text("gf2/doctor/yard")
                            .desired_width(280.0),
                    );
                });
                row(ui, tr("zone.stream.parent"), |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.parent)
                            .hint_text("gf2/cuba")
                            .desired_width(280.0),
                    )
                    .on_hover_text(tr("zone.stream.parent_hint"));
                });
                row(ui, tr("zone.stream.load"), |ui| {
                    egui::ComboBox::from_id_salt(("zone-load", salt))
                        .selected_text(Self::load_label(self.load))
                        .width(280.0)
                        .show_ui(ui, |ui| {
                            for load in StreamLoad::ALL {
                                ui.selectable_value(&mut self.load, load, Self::load_label(load));
                            }
                        });
                });
                row(ui, tr("zone.stream.guid"), |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.guid)
                            .hint_text(tr("zone.stream.guid_hint"))
                            .desired_width(280.0),
                    );
                });
            });
    }
}

/// "World grid cell…".
#[derive(Debug, Clone)]
pub(crate) struct CellForm {
    pub level: String,
    pub stream: String,
    pub min: String,
    pub max: String,
    pub lod: String,
    pub name: String,
    pub on_enable: bool,
}

impl Default for CellForm {
    fn default() -> Self {
        Self {
            level: String::new(),
            stream: String::new(),
            min: String::new(),
            max: String::new(),
            lod: "high".into(),
            name: String::new(),
            on_enable: false,
        }
    }
}

impl CellForm {
    pub fn params(&self) -> Result<ZoneCellParams, String> {
        Ok(ZoneCellParams {
            level: self.level.trim().to_owned(),
            stream: self.stream.trim().to_owned(),
            min: parse_point(&self.min)?,
            max: parse_point(&self.max)?,
            lod: Some(self.lod.clone()),
            name: text_or_none(&self.name),
            on_enable: self.on_enable,
            guid: None,
        })
    }

    /// The fields; the stream is left out when the caller fixes it.
    pub fn ui(&mut self, ui: &mut egui::Ui, salt: &str, new_streams: Option<&[String]>) {
        egui::Grid::new(("zone-cell", salt))
            .num_columns(2)
            .show(ui, |ui| {
                row(ui, tr("zone.cell.level"), |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.level)
                            .hint_text("gf2/cuba")
                            .desired_width(280.0),
                    )
                    .on_hover_text(tr("zone.cell.level_hint"));
                });
                if let Some(new_streams) = new_streams {
                    row(ui, tr("zone.cell.stream"), |ui| {
                        stream_field(ui, "zone-cell-stream", &mut self.stream, new_streams);
                    });
                }
                row(ui, tr("zone.cell.min"), |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.min)
                            .hint_text("X, Y, Z")
                            .desired_width(180.0),
                    );
                });
                row(ui, tr("zone.cell.max"), |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.max)
                            .hint_text("X, Y, Z")
                            .desired_width(180.0),
                    );
                });
                row(ui, tr("zone.cell.lod"), |ui| {
                    egui::ComboBox::from_id_salt(("zone-lod", salt))
                        .selected_text(tr(lod_key(&self.lod)))
                        .show_ui(ui, |ui| {
                            for lod in ["high", "medium", "low", "map"] {
                                ui.selectable_value(
                                    &mut self.lod,
                                    lod.to_owned(),
                                    tr(lod_key(lod)),
                                );
                            }
                        });
                });
                row(ui, tr("zone.cell.name"), |ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(180.0))
                        .on_hover_text(tr("zone.cell.name_hint"));
                    ui.checkbox(&mut self.on_enable, tr("zone.cell.on_enable"))
                        .on_hover_text(tr("zone.cell.on_enable_hint"));
                });
            });
    }
}

fn lod_key(lod: &str) -> &'static str {
    match lod {
        "medium" => "zone.lod.medium",
        "low" => "zone.lod.low",
        "map" => "zone.lod.map",
        _ => "zone.lod.high",
    }
}

/// State of the window.
#[derive(Default)]
pub(crate) struct ZoneForms {
    pub open: bool,
    pub form: Form,
    pub stream: StreamForm,
    pub cell: CellForm,
    // New SimGroup.
    pub group_stream: String,
    pub group_copy: String,
    pub group_objects: String,
    // Clone resources.
    pub clone_stream: String,
    pub clone_indices: String,
    pub clone_to: String,
    pub clone_retarget: String,
    pub clone_keep: bool,
    // Add resource.
    pub res_stream: String,
    pub res_type: String,
    pub res_file: String,
    pub res_ids: String,
    pub res_policy: String,
    pub res_compress: bool,
    /// The cells of the level of the cell form, as listed last.
    cells: Option<(String, Result<Vec<CellInfo>, String>)>,
    /// What the last action said.
    pub done: Option<String>,
    pub error: Option<String>,
    /// `DOCTOR_TOOLKIT_GUI_ZONE` was read.
    preset_used: bool,
    /// Zones to load in level tabs that are about to be made.
    pub open_in_level: Vec<(u64, Vec<String>)>,
}

impl ZoneForms {
    fn group_params(&self) -> Result<NewGroupParams, String> {
        let objects = parse_indices(&self.group_objects)?;
        Ok(NewGroupParams {
            stream: self.group_stream.trim().to_owned(),
            copy_of: text_or_none(&self.group_copy),
            objects: (!objects.is_empty()).then_some(objects),
        })
    }

    fn clone_params(&self) -> Result<CloneParams, String> {
        Ok(CloneParams {
            stream: self.clone_stream.trim().to_owned(),
            indices: parse_indices(&self.clone_indices)?,
            to: text_or_none(&self.clone_to),
            seed: None,
            keep_references: self.clone_keep,
            retarget: parse_indices(&self.clone_retarget)?,
        })
    }

    fn resource_params(&self) -> AddResourceParams {
        AddResourceParams {
            stream: self.res_stream.trim().to_owned(),
            type_tag: self.res_type.trim().to_owned(),
            file: PathBuf::from(self.res_file.trim()),
            ids: text_or_none(&self.res_ids),
            before: None,
            policy: text_or_none(&self.res_policy),
            alignment: None,
            compress: self.res_compress,
        }
    }
}

impl BrowserApp {
    /// Opens the window on `form`.
    pub(super) fn open_zone_form(&mut self, form: Form) {
        self.zone.open = true;
        self.zone.form = form;
        self.zone.done = None;
        self.zone.error = None;
    }

    /// Opens the clone form for resource `resource` of `stream`.
    pub(super) fn open_clone_form(&mut self, stream: &str, resource: usize) {
        self.open_zone_form(Form::Clone);
        self.zone.clone_stream = stream.to_owned();
        self.zone.clone_indices = resource.to_string();
    }

    /// Opens the form `form` with `stream` as the stream it is about.
    pub(super) fn open_zone_form_for(&mut self, form: Form, stream: &str) {
        self.open_zone_form(form);
        match form {
            Form::Group => self.zone.group_stream = stream.to_owned(),
            Form::Cell => self.zone.cell.stream = stream.to_owned(),
            Form::Resource => self.zone.res_stream = stream.to_owned(),
            Form::Clone => self.zone.clone_to = stream.to_owned(),
            Form::Stream => {}
        }
    }

    /// The names of the streams the workspace creates.
    pub(super) fn new_stream_names(&self) -> Vec<String> {
        self.workspace
            .as_ref()
            .map(|ws| ws.new_streams().map(|s| s.name.clone()).collect())
            .unwrap_or_default()
    }

    /// Records `action` in the workspace: one undoable operation. Returns
    /// its label. The views are not told ([`Self::zone_action`] does that).
    pub(super) fn zone_record(&mut self, action: ZoneAction) -> Result<String, String> {
        let (Some(game), Some(index)) = (self.game.clone(), self.index.clone()) else {
            return Err(tr("edits.no_game").to_owned());
        };
        let taken: HashSet<Guid> = self.levels.values().flat_map(|l| l.guids()).collect();
        let mut assets =
            match &action {
                ZoneAction::ImportModel(_) => Some(self.assets.take().unwrap_or_else(|| {
                    doctor_toolkit::game::GameAssets::new(game.clone(), &index)
                })),
                _ => None,
            };
        let ws = self.workspace_mut()?;
        let last = |ws: &doctor_toolkit::workspace::Workspace| {
            ws.operations()
                .get(ws.head().wrapping_sub(1))
                .map(|op| op.label.clone())
                .unwrap_or_default()
        };
        let text = |e: doctor_toolkit::Error| e.to_string();
        let result = match action {
            ZoneAction::AddStream(p) => zone_ops::add_stream(&game, ws, &p)
                .map(|_| last(ws))
                .map_err(text),
            ZoneAction::RemoveStream(name) => zone_ops::remove_stream(ws, &name)
                .map(|_| last(ws))
                .map_err(text),
            ZoneAction::AddGroup(p) => {
                zone_ops::add_simgroup(&game, &index, ws, &p, &|g| taken.contains(g))
                    .map(|_| last(ws))
                    .map_err(text)
            }
            ZoneAction::AddCell(p) => zone_ops::add_zone_cell(&game, ws, &p)
                .map(|_| last(ws))
                .map_err(text),
            ZoneAction::RemoveCell { level, stream } => {
                zone_ops::remove_zone_cell(&game, ws, &level, &stream)
                    .map(|_| last(ws))
                    .map_err(text)
            }
            ZoneAction::Clone(p) => zone_ops::clone_resources(&game, &index, ws, &p)
                .map(|done| done.label)
                .map_err(text),
            ZoneAction::AddResource(p) => zone_ops::add_resource(&game, &index, ws, &p)
                .map(|_| last(ws))
                .map_err(text),
            ZoneAction::ImportModel(p) => doctor_toolkit::scene::read_file(&p.file)
                .and_then(|scene| {
                    let assets = assets.as_mut().expect("made above for an import");
                    let plan = zone_ops::plan_model_import(&game, &index, ws, assets, &scene, &p)?;
                    plan.commit(ws, &game)?;
                    Ok(plan.label)
                })
                .map_err(text),
        };
        if let Some(assets) = assets {
            self.assets = Some(assets);
        }
        result
    }

    /// Records `action` and shows the views the result. Returns what the
    /// status line says.
    pub(super) fn zone_action(&mut self, action: ZoneAction) -> Result<String, String> {
        let label = self.zone_record(action)?;
        let message = trf("zone.recorded", &[("label", &label)]);
        self.after_edit(None);
        self.status = message.clone();
        Ok(message)
    }

    /// Opens a level tab that loads `zones`.
    pub(super) fn open_zones_in_level(&mut self, zones: Vec<String>) {
        let id = self.new_pane_id();
        self.zone.open_in_level.push((id, zones));
        self.dock_ops
            .push(super::dock::DockOp::Open(super::dock::Pane::Level { id }));
        self.dock_ops.push(super::dock::DockOp::Focus(id));
    }

    /// The window of the forms.
    pub(super) fn show_zone_forms(&mut self, ctx: &egui::Context) {
        if !self.zone.open {
            return;
        }
        let new_streams = self.new_stream_names();
        let mut open = true;
        let mut run: Option<Result<ZoneAction, String>> = None;
        let mut list_cells = false;
        let mut show_zone: Option<String> = None;
        egui::Window::new(tr("zone.title"))
            .open(&mut open)
            .default_size([620.0, 420.0])
            .resizable(true)
            .show(ctx, |ui| {
                let forms = &mut self.zone;
                ui.horizontal_wrapped(|ui| {
                    for form in Form::ALL {
                        if ui
                            .selectable_label(forms.form == form, tr(form.key()))
                            .clicked()
                        {
                            forms.form = form;
                            forms.done = None;
                            forms.error = None;
                        }
                    }
                });
                ui.separator();
                ui.label(RichText::new(tr(forms.form.hint())).weak());
                ui.add_space(4.0);
                match forms.form {
                    Form::Stream => {
                        forms.stream.ui(ui, "form");
                        if ui.button(tr("zone.record")).clicked() {
                            run = Some(Ok(ZoneAction::AddStream(forms.stream.params())));
                        }
                        if !new_streams.is_empty() {
                            ui.separator();
                            ui.label(RichText::new(tr("zone.new_streams")).strong());
                            for name in &new_streams {
                                ui.horizontal(|ui| {
                                    ui.monospace(name);
                                    if ui.small_button(tr("zone.show_in_level")).clicked() {
                                        show_zone = Some(name.clone());
                                    }
                                    if ui
                                        .small_button(tr("zone.remove_stream"))
                                        .on_hover_text(tr("zone.remove_stream_hint"))
                                        .clicked()
                                    {
                                        run = Some(Ok(ZoneAction::RemoveStream(name.clone())));
                                    }
                                });
                            }
                        }
                    }
                    Form::Group => {
                        egui::Grid::new("zone-group").num_columns(2).show(ui, |ui| {
                            row(ui, tr("zone.group.stream"), |ui| {
                                stream_field(
                                    ui,
                                    "zone-group-stream",
                                    &mut forms.group_stream,
                                    &new_streams,
                                );
                            });
                            row(ui, tr("zone.group.copy"), |ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut forms.group_copy)
                                        .hint_text(tr("zone.group.copy_hint"))
                                        .desired_width(280.0),
                                );
                            });
                            row(ui, tr("zone.group.objects"), |ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut forms.group_objects)
                                        .hint_text(tr("zone.group.objects_hint"))
                                        .desired_width(280.0),
                                );
                            });
                        });
                        if ui.button(tr("zone.record")).clicked() {
                            run = Some(forms.group_params().map(ZoneAction::AddGroup));
                        }
                    }
                    Form::Cell => {
                        forms.cell.ui(ui, "form", Some(&new_streams));
                        ui.horizontal(|ui| {
                            if ui.button(tr("zone.record")).clicked() {
                                run = Some(forms.cell.params().map(ZoneAction::AddCell));
                            }
                            if ui
                                .button(tr("zone.cell.list"))
                                .on_hover_text(tr("zone.cell.list_hint"))
                                .clicked()
                            {
                                list_cells = true;
                            }
                        });
                        if let Some((level, cells)) = &forms.cells {
                            ui.separator();
                            match cells {
                                Err(error) => {
                                    ui.colored_label(Color32::from_rgb(230, 90, 80), error);
                                }
                                Ok(cells) => {
                                    ui.label(trf(
                                        "zone.cell.listed",
                                        &[("level", level), ("count", &cells.len())],
                                    ));
                                    egui::ScrollArea::vertical()
                                        .id_salt("zone-cells")
                                        .max_height(180.0)
                                        .show(ui, |ui| {
                                            for c in cells {
                                                ui.horizontal(|ui| {
                                                    let text = format!(
                                                        "#{} {:3}  {}  ({:.0}, {:.0}, {:.0}) r {:.0}",
                                                        c.resource,
                                                        c.cell,
                                                        c.stream,
                                                        c.centre[0],
                                                        c.centre[1],
                                                        c.centre[2],
                                                        c.radius
                                                    );
                                                    if c.new {
                                                        ui.colored_label(Color32::LIGHT_BLUE, text);
                                                    } else {
                                                        ui.monospace(text);
                                                    }
                                                    if ui
                                                        .small_button(tr("zone.cell.remove"))
                                                        .clicked()
                                                    {
                                                        run = Some(Ok(ZoneAction::RemoveCell {
                                                            level: level.clone(),
                                                            stream: format!("{:08x}", c.hash),
                                                        }));
                                                    }
                                                });
                                            }
                                        });
                                }
                            }
                        }
                    }
                    Form::Clone => {
                        egui::Grid::new("zone-clone").num_columns(2).show(ui, |ui| {
                            row(ui, tr("zone.clone.stream"), |ui| {
                                stream_field(
                                    ui,
                                    "zone-clone-stream",
                                    &mut forms.clone_stream,
                                    &new_streams,
                                );
                            });
                            row(ui, tr("zone.clone.indices"), |ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut forms.clone_indices)
                                        .hint_text("12, 13")
                                        .desired_width(280.0),
                                )
                                .on_hover_text(tr("zone.clone.indices_hint"));
                            });
                            row(ui, tr("zone.clone.to"), |ui| {
                                stream_field(ui, "zone-clone-to", &mut forms.clone_to, &new_streams);
                            });
                            row(ui, tr("zone.clone.retarget"), |ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut forms.clone_retarget)
                                        .desired_width(280.0),
                                )
                                .on_hover_text(tr("zone.clone.retarget_hint"));
                            });
                            row(ui, "", |ui| {
                                ui.checkbox(&mut forms.clone_keep, tr("zone.clone.keep"))
                                    .on_hover_text(tr("zone.clone.keep_hint"));
                            });
                        });
                        if ui.button(tr("zone.record")).clicked() {
                            run = Some(forms.clone_params().map(ZoneAction::Clone));
                        }
                    }
                    Form::Resource => {
                        egui::Grid::new("zone-resource")
                            .num_columns(2)
                            .show(ui, |ui| {
                                row(ui, tr("zone.resource.stream"), |ui| {
                                    stream_field(
                                        ui,
                                        "zone-resource-stream",
                                        &mut forms.res_stream,
                                        &new_streams,
                                    );
                                });
                                row(ui, tr("zone.resource.type"), |ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut forms.res_type)
                                            .hint_text("lh2")
                                            .desired_width(120.0),
                                    )
                                    .on_hover_text(tr("zone.resource.type_hint"));
                                });
                                row(ui, tr("zone.resource.file"), |ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut forms.res_file)
                                            .desired_width(280.0),
                                    );
                                    if ui.button(tr("prefs.choose")).clicked()
                                        && let Some(path) = rfd::FileDialog::new().pick_file()
                                    {
                                        forms.res_file = path.display().to_string();
                                    }
                                });
                                row(ui, tr("zone.resource.ids"), |ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut forms.res_ids)
                                            .hint_text(tr("zone.resource.ids_hint"))
                                            .desired_width(280.0),
                                    );
                                });
                                row(ui, tr("zone.resource.policy"), |ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut forms.res_policy)
                                            .hint_text(tr("zone.resource.policy_hint"))
                                            .desired_width(180.0),
                                    );
                                    ui.checkbox(&mut forms.res_compress, tr("zone.resource.compress"));
                                });
                            });
                        if ui.button(tr("zone.record")).clicked() {
                            run = Some(Ok(ZoneAction::AddResource(forms.resource_params())));
                        }
                    }
                }
                ui.add_space(4.0);
                if let Some(error) = &forms.error {
                    ui.colored_label(Color32::from_rgb(230, 90, 80), error);
                }
                if let Some(done) = &forms.done {
                    ui.label(done);
                }
            });
        if !open {
            self.zone.open = false;
        }
        if let Some(name) = show_zone {
            self.open_zones_in_level(vec![name]);
        }
        if let Some(action) = run {
            let removed_cell = matches!(action, Ok(ZoneAction::RemoveCell { .. }));
            match action.and_then(|a| self.zone_action(a)) {
                Ok(message) => {
                    self.zone.done = Some(message);
                    self.zone.error = None;
                    list_cells |= removed_cell;
                }
                Err(error) => {
                    self.zone.done = None;
                    self.zone.error = Some(trf("zone.failed", &[("error", &error)]));
                }
            }
        }
        if list_cells {
            self.list_zone_cells();
        }
    }

    /// Reads the cells of the level named in the cell form.
    pub(super) fn list_zone_cells(&mut self) {
        let level = self.zone.cell.level.trim().to_owned();
        let cells = match (&self.game, &self.workspace) {
            (Some(game), Some(ws)) => {
                zone_ops::zone_cells(game, ws, &level).map_err(|e| e.to_string())
            }
            (Some(game), None) => self
                .workspace_for_reading()
                .and_then(|ws| zone_ops::zone_cells(game, &ws, &level).map_err(|e| e.to_string())),
            _ => Err(tr("edits.no_game").to_owned()),
        };
        self.zone.cells = Some((level, cells));
    }

    /// The cells listed last by the cell form.
    #[cfg(test)]
    pub(super) fn listed_cells(&self) -> Option<&Vec<CellInfo>> {
        self.zone.cells.as_ref()?.1.as_ref().ok()
    }

    /// `DOCTOR_TOOLKIT_GUI_ZONE`: actions run once when the game is opened,
    /// before the views get it (`docs/shell.md`). An action that fails ends
    /// the editor with exit code 2: a script must not go on with half a zone.
    pub(super) fn zone_preset(&mut self) {
        if std::mem::replace(&mut self.zone.preset_used, true) {
            return;
        }
        let Ok(spec) = std::env::var("DOCTOR_TOOLKIT_GUI_ZONE") else {
            return;
        };
        if let Err(error) = self.run_zone_script(&spec) {
            eprintln!("zone: {error}");
            std::process::exit(2);
        }
    }

    /// Runs the actions of a preset: `verb:key=value,key=value` joined by
    /// `;`. Points are `X/Y/Z`, lists of indices `1+2`. `form=<name>` opens
    /// the window on a form instead.
    pub(super) fn run_zone_script(&mut self, spec: &str) -> Result<(), String> {
        // While the game is being opened the views do not have it yet.
        let opening = self.staging_is_fresh();
        for item in spec.split(';').map(str::trim).filter(|i| !i.is_empty()) {
            if let Some(form) = item.strip_prefix("form=") {
                let form = Form::from_slug(form).ok_or_else(|| format!("unknown form `{form}`"))?;
                self.open_zone_form(form);
                continue;
            }
            let action = parse_action(item)?;
            self.fill_form(&action);
            let label = self
                .zone_record(action)
                .map_err(|e| format!("`{item}`: {e}"))?;
            eprintln!("zone: {label}");
            self.zone.done = Some(trf("zone.recorded", &[("label", &label)]));
            // The next action may address what this one made.
            if opening {
                self.stage_quietly();
            } else {
                self.after_edit(None);
            }
        }
        Ok(())
    }

    /// Shows the parameters of a preset action in its form (screenshots).
    fn fill_form(&mut self, action: &ZoneAction) {
        let forms = &mut self.zone;
        let point = |p: [f32; 3]| format!("{}, {}, {}", p[0], p[1], p[2]);
        let list = |v: &[usize]| {
            v.iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        };
        match action {
            ZoneAction::AddStream(p) => {
                forms.form = Form::Stream;
                forms.stream = StreamForm {
                    name: p.name.clone(),
                    parent: p.parent.clone(),
                    load: p.load,
                    guid: p.guid.clone().unwrap_or_default(),
                };
            }
            ZoneAction::AddGroup(p) => {
                forms.form = Form::Group;
                forms.group_stream = p.stream.clone();
                forms.group_copy = p.copy_of.clone().unwrap_or_default();
                forms.group_objects = list(p.objects.as_deref().unwrap_or_default());
            }
            ZoneAction::AddCell(p) => {
                forms.form = Form::Cell;
                forms.cell = CellForm {
                    level: p.level.clone(),
                    stream: p.stream.clone(),
                    min: point(p.min),
                    max: point(p.max),
                    lod: p.lod.clone().unwrap_or_else(|| "high".into()),
                    name: p.name.clone().unwrap_or_default(),
                    on_enable: p.on_enable,
                };
            }
            ZoneAction::Clone(p) => {
                forms.form = Form::Clone;
                forms.clone_stream = p.stream.clone();
                forms.clone_indices = list(&p.indices);
                forms.clone_to = p.to.clone().unwrap_or_default();
                forms.clone_retarget = list(&p.retarget);
                forms.clone_keep = p.keep_references;
            }
            ZoneAction::AddResource(p) => {
                forms.form = Form::Resource;
                forms.res_stream = p.stream.clone();
                forms.res_type = p.type_tag.clone();
                forms.res_file = p.file.display().to_string();
                forms.res_ids = p.ids.clone().unwrap_or_default();
                forms.res_policy = p.policy.clone().unwrap_or_default();
                forms.res_compress = p.compress;
            }
            ZoneAction::RemoveStream(_)
            | ZoneAction::RemoveCell { .. }
            | ZoneAction::ImportModel(_) => {}
        }
    }
}

/// One action of a preset: `verb:key=value,…`.
pub(crate) fn parse_action(item: &str) -> Result<ZoneAction, String> {
    let (verb, rest) = item.split_once(':').unwrap_or((item, ""));
    let mut fields: Vec<(&str, &str)> = Vec::new();
    for pair in rest.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        fields.push(pair.split_once('=').unwrap_or((pair, "")));
    }
    let get = |key: &str| {
        fields
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| (*v).to_owned())
    };
    let need = |key: &str| get(key).ok_or_else(|| format!("`{item}`: `{key}=` is missing"));
    let flag = |key: &str| fields.iter().any(|(k, _)| *k == key);
    let known = |keys: &[&str]| match fields.iter().find(|(k, _)| !keys.contains(k)) {
        Some((k, _)) => Err(format!("`{item}`: unknown key `{k}`")),
        None => Ok(()),
    };
    Ok(match verb {
        "stream" => {
            known(&["name", "parent", "load", "guid"])?;
            let load = match get("load") {
                None => StreamLoad::OnDemand,
                Some(text) => StreamLoad::from_slug(&text).ok_or_else(|| {
                    format!("`{item}`: load is on_demand, with_parent or auto_load")
                })?,
            };
            ZoneAction::AddStream(NewStreamParams {
                name: need("name")?,
                parent: need("parent")?,
                load,
                tocs: Vec::new(),
                guid: get("guid"),
            })
        }
        "remove-stream" => {
            known(&["name"])?;
            ZoneAction::RemoveStream(need("name")?)
        }
        "simgroup" => {
            known(&["stream", "copy", "objects"])?;
            let objects = get("objects").map(|t| parse_indices(&t)).transpose()?;
            ZoneAction::AddGroup(NewGroupParams {
                stream: need("stream")?,
                copy_of: get("copy"),
                objects,
            })
        }
        "cell" => {
            known(&["level", "stream", "min", "max", "lod", "name", "on_enable"])?;
            ZoneAction::AddCell(ZoneCellParams {
                level: need("level")?,
                stream: need("stream")?,
                min: parse_point(&need("min")?)?,
                max: parse_point(&need("max")?)?,
                lod: get("lod"),
                name: get("name"),
                on_enable: flag("on_enable"),
                guid: None,
            })
        }
        "remove-cell" => {
            known(&["level", "stream"])?;
            ZoneAction::RemoveCell {
                level: need("level")?,
                stream: need("stream")?,
            }
        }
        "clone" => {
            known(&["stream", "index", "to", "retarget", "keep"])?;
            ZoneAction::Clone(CloneParams {
                stream: need("stream")?,
                indices: parse_indices(&need("index")?)?,
                to: get("to"),
                seed: None,
                keep_references: flag("keep"),
                retarget: get("retarget")
                    .map(|t| parse_indices(&t))
                    .transpose()?
                    .unwrap_or_default(),
            })
        }
        "resource" => {
            known(&["stream", "type", "file", "ids", "policy", "compress"])?;
            ZoneAction::AddResource(AddResourceParams {
                stream: need("stream")?,
                type_tag: need("type")?,
                file: PathBuf::from(need("file")?),
                ids: get("ids"),
                before: None,
                policy: get("policy"),
                alignment: None,
                compress: flag("compress"),
            })
        }
        "import" => {
            known(&["file", "template", "name", "stream", "collision"])?;
            ZoneAction::ImportModel(ImportModelParams {
                file: PathBuf::from(need("file")?),
                template: need("template")?,
                name: get("name"),
                stream: get("stream"),
                collision: get("collision"),
                retarget: Vec::new(),
                format: None,
            })
        }
        other => return Err(format!("unknown action `{other}`")),
    })
}
