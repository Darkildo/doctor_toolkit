//! Details of an object of a level: the header and the groups Transform,
//! Model, Add object, Logic, Collision and Advanced.
//!
//! Nothing is written here: every change is a [`LevelAction`] the app
//! records in the workspace, one operation each (Ctrl+Z takes it back).
//! The X, Y, Z fields drag with the mouse (or take typed values): while a
//! field is dragged the view shows the object there, and letting go (or
//! Enter) records the move.

use std::collections::{HashMap, HashSet};

use doctor_toolkit::level::edit::ModelChoice;
use doctor_toolkit::level::{Found, Location, RowMatrix, TYPE_METAMODEL};
use doctor_toolkit::resource::hkx::collision::CollisionKind;
use doctor_toolkit::resource::names::sim_classes;
use doctor_toolkit::resource::simgroup::{Guid, Value};
use doctor_toolkit::resource::simgroup_edit::Transform;
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{Color32, RichText, Sense, Vec2};

use super::super::BrowserApp;
use super::super::dnd::DraggedAsset;
use super::super::theme;
use crate::level_view::{LevelAction, LevelView, Mark, ObjRef};
use crate::logic_view::{LogicAction, LogicView};

/// Most models listed at once by the model chooser.
const MAX_CANDIDATES: usize = 5000;

/// What a Details panel keeps about the object it shows.
#[derive(Default)]
pub struct ObjectState {
    transform: Option<Draft>,
    /// The SimGroup editor of the object's group: level, group, revision.
    logic: Option<(u64, usize, u64, LogicView)>,
    /// References from and to the object: level, object, group revision.
    references: Option<(u64, ObjRef, u64, References)>,
    /// "Remove" was clicked once for these objects.
    confirm_remove: bool,
    model_search: String,
    /// Models the object's zone can show: level and stream they are for.
    candidates: Option<(u64, usize, Vec<Candidate>)>,
    /// The last problem (a refused model, a wrong value).
    problem: String,
}

/// The transform being edited: of the object shown, and of the other
/// selected objects with it (stage 54: one value for all, "—" where they
/// differ).
struct Draft {
    level: u64,
    /// The objects edited, the one shown first.
    objects: Vec<ObjRef>,
    /// The revisions of their groups the stored values are from.
    revisions: Vec<u64>,
    stored: Vec<Transform>,
    values: Vec<Transform>,
}

/// A model the chooser offers.
struct Candidate {
    id: u32,
    label: String,
    search: String,
}

#[derive(Default)]
struct References {
    /// Objects of the group this one names in its attributes.
    to: Vec<ObjRef>,
    /// Objects of the group that name this one.
    from: Vec<ObjRef>,
}

impl BrowserApp {
    /// Details of object `r` of level view `id`; returns the edits to record.
    pub(super) fn object_details(
        &mut self,
        ui: &mut egui::Ui,
        state: &mut ObjectState,
        id: u64,
        r: ObjRef,
    ) -> Vec<LevelAction> {
        let names = |stream: usize, resource: usize| -> Option<String> {
            let index = self.nav.index.as_ref()?;
            let at = index.locate(stream, resource)?;
            index.record(at).name().map(str::to_owned)
        };
        let mut names_cache: HashMap<(usize, usize), Option<String>> = HashMap::new();
        let mut name_of = |stream: usize, resource: usize| {
            names_cache
                .entry((stream, resource))
                .or_insert_with(|| names(stream, resource))
                .clone()
        };
        let Some(level) = self.levels.get_mut(&id) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        object_ui(ui, state, id, level, r, &mut name_of, &mut out);
        out
    }
}

fn object_ui(
    ui: &mut egui::Ui,
    state: &mut ObjectState,
    id: u64,
    level: &mut LevelView,
    r: ObjRef,
    names: &mut dyn FnMut(usize, usize) -> Option<String>,
    out: &mut Vec<LevelAction>,
) {
    let (Some(object), Some((location, _))) = (level.object(r).cloned(), level.location_of(r))
    else {
        return;
    };
    let Some((game, _)) = level.game() else {
        return;
    };
    let game = game.clone();
    let stream_name = game.streams()[location.stream].stream_name();
    let class = object
        .class_name
        .map_or_else(|| format!("{:08x}", object.class), str::to_owned);
    let locked = level.is_object_locked(r);

    // Header: what it is, where it is, marks, the common edits.
    ui.horizontal_wrapped(|ui| {
        ui.strong(if object.name.is_empty() {
            class.clone()
        } else {
            format!("{} · {class}", object.name)
        });
        match level.mark(r) {
            Some(Mark::New) => badge(ui, tr("outliner.new"), theme::AXIS_Y),
            Some(Mark::Edited) => badge(ui, tr("outliner.edited"), theme::ACCENT),
            None => {}
        }
        if locked {
            badge(ui, tr("details.locked"), theme::FOCUS);
        }
        if level.is_object_hidden(r) {
            badge(ui, tr("details.hidden"), Color32::GRAY);
        }
    });
    ui.label(
        RichText::new(trf(
            "level.object_place",
            &[
                ("stream", &stream_name),
                ("resource", &location.resource),
                ("object", &r.object),
            ],
        ))
        .weak()
        .small(),
    );
    let selected = level.selection().len();
    if selected > 1 {
        ui.label(
            RichText::new(trf("details.many_selected", &[("count", &selected)]))
                .weak()
                .small(),
        );
    }
    ui.horizontal_wrapped(|ui| {
        if ui
            .button(tr("details.focus"))
            .on_hover_text(tr("details.focus_hint"))
            .clicked()
        {
            level.focus_on(r);
        }
        if ui
            .button(tr("level.duplicate"))
            .on_hover_text(tr("level.duplicate_hint"))
            .clicked()
            && let Some(a) = level.edit_selection(false)
        {
            out.push(a);
        }
        let text = if state.confirm_remove {
            tr("level.remove_confirm")
        } else {
            tr("level.remove")
        };
        if ui.button(text).clicked() {
            if state.confirm_remove {
                out.extend(level.edit_selection(true));
                state.confirm_remove = false;
            } else {
                state.confirm_remove = true;
            }
        }
    });
    if !level.is_selected(r) {
        state.confirm_remove = false;
    }
    if !state.problem.is_empty() {
        ui.colored_label(Color32::from_rgb(230, 120, 80), &state.problem);
    }

    if object.transform.is_some() {
        group(
            ui,
            "details-transform",
            tr("details.transform"),
            true,
            |ui| {
                transform_ui(ui, state, id, level, r, locked, out);
            },
        );
    }
    group(ui, "details-model", tr("details.model"), true, |ui| {
        model_ui(ui, state, id, level, r, location, names, out);
    });
    let add_open = std::mem::take(&mut level.add.open);
    egui::CollapsingHeader::new(RichText::new(tr("level.add_object")).strong())
        .id_salt("details-add")
        .default_open(level.add.fixed)
        .open(add_open.then_some(true))
        .show(ui, |ui| add_ui(ui, level, r, location, out));
    group(ui, "details-logic", tr("details.logic"), true, |ui| {
        logic_ui(ui, state, id, level, r, location, out);
    });
    if level.live_edit.reads {
        group(ui, "details-live", tr("level.live_title"), true, |ui| {
            out.extend(level.live_section(ui, r));
        });
    }
    group(
        ui,
        "details-collision",
        tr("details.collision"),
        false,
        |ui| {
            collision_ui(ui, level, &object);
        },
    );
    group(
        ui,
        "details-advanced",
        tr("details.advanced"),
        false,
        |ui| {
            advanced_ui(ui, state, id, level, r, location, out);
        },
    );
}

/// A group of properties that folds.
fn group(ui: &mut egui::Ui, salt: &str, title: &str, open: bool, add: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(RichText::new(title).strong())
        .id_salt(salt)
        .default_open(open)
        .show(ui, add);
}

fn badge(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, color))
        .corner_radius(3)
        .inner_margin(egui::Margin::symmetric(4, 0))
        .show(ui, |ui| {
            ui.label(RichText::new(text).small().color(color));
        });
}

// ---------------------------------------------------------------- transform

/// The objects the Transform group edits: `r`, and with it every other
/// selected object that is placed and not locked (when `r` is selected).
fn transform_objects(level: &LevelView, r: ObjRef) -> Vec<ObjRef> {
    let mut objects = vec![r];
    if level.is_selected(r) {
        objects.extend(level.selection().into_iter().filter(|&o| {
            o != r
                && !level.is_object_locked(o)
                && level.object(o).is_some_and(|x| x.transform.is_some())
        }));
    }
    objects
}

/// A transform as the fields show it: no "-0.0".
fn shown(m: &RowMatrix) -> Transform {
    let mut t = Transform::from_matrix(m);
    for v in t
        .position
        .iter_mut()
        .chain(&mut t.rotation)
        .chain(&mut t.scale)
    {
        *v += 0.0;
    }
    t
}

fn transform_ui(
    ui: &mut egui::Ui,
    state: &mut ObjectState,
    id: u64,
    level: &mut LevelView,
    r: ObjRef,
    locked: bool,
    out: &mut Vec<LevelAction>,
) {
    if level.object(r).and_then(|o| o.transform).is_none() {
        return;
    }
    let has_matrix = level.object(r).is_some_and(|o| o.has_matrix);
    let objects = transform_objects(level, r);
    let revisions: Vec<u64> = objects
        .iter()
        .map(|o| level.group_revision(o.group))
        .collect();
    let fresh = state
        .transform
        .as_ref()
        .is_none_or(|d| d.level != id || d.objects != objects || d.revisions != revisions);
    if fresh {
        let stored: Vec<Transform> = objects
            .iter()
            .filter_map(|&o| level.object(o)?.transform)
            .map(|m| shown(&m))
            .collect();
        state.transform = Some(Draft {
            level: id,
            objects: objects.clone(),
            revisions,
            values: stored.clone(),
            stored,
        });
    }
    let draft = state.transform.as_mut().expect("set above");
    let many = draft.objects.len() > 1;
    let mut changed = false;
    let mut release = false;
    ui.add_enabled_ui(!locked, |ui| {
        egui::Grid::new(("details-transform-grid", id))
            .num_columns(4)
            .spacing([4.0, 3.0])
            .show(ui, |ui| {
                // The names keep their width; the fields share the rest.
                let font = egui::TextStyle::Body.resolve(ui.style());
                let label_width = [
                    tr("details.location"),
                    tr("details.rotation"),
                    tr("details.scale"),
                ]
                .iter()
                .map(|t| {
                    ui.painter()
                        .layout_no_wrap((*t).to_owned(), font.clone(), Color32::WHITE)
                        .size()
                        .x
                })
                .fold(0.0, f32::max)
                    + 4.0;
                let height = ui.spacing().interact_size.y;
                let width = ((ui.available_width() - label_width) / 3.0 - 10.0).clamp(44.0, 140.0);
                for (field, name, speed, decimals, suffix) in [
                    (0, tr("details.location"), 0.05, 3, ""),
                    (1, tr("details.rotation"), 0.5, 2, "°"),
                    (2, tr("details.scale"), 0.01, 3, ""),
                ] {
                    ui.allocate_ui_with_layout(
                        Vec2::new(label_width, height),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.set_min_width(label_width);
                            ui.add(egui::Label::new(name).wrap_mode(egui::TextWrapMode::Extend));
                        },
                    );
                    for axis in 0..3 {
                        let get = |t: &Transform| match field {
                            0 => t.position[axis],
                            1 => t.rotation[axis],
                            _ => t.scale[axis],
                        };
                        let first = get(&draft.values[0]);
                        let mixed = draft.values.iter().any(|t| (get(t) - first).abs() > 1e-4);
                        let mut value = first;
                        let response =
                            axis_field(ui, axis, &mut value, speed, decimals, suffix, width, mixed);
                        if response.changed() && value != first {
                            // A drag moves every object by as much; a typed
                            // value is given to all.
                            let shift = response.dragged();
                            for t in &mut draft.values {
                                let v = match field {
                                    0 => &mut t.position[axis],
                                    1 => &mut t.rotation[axis],
                                    _ => &mut t.scale[axis],
                                };
                                *v = if shift { *v + value - first } else { value };
                            }
                            changed = true;
                        }
                        release |= response.drag_stopped() || response.lost_focus();
                    }
                    ui.end_row();
                }
            });
    });
    if many {
        ui.add(
            egui::Label::new(
                RichText::new(trf(
                    "details.transform_many",
                    &[("count", &draft.objects.len())],
                ))
                .weak()
                .small(),
            )
            .wrap(),
        );
    }
    if locked {
        ui.label(RichText::new(tr("details.locked_hint")).weak().small());
    } else if !has_matrix {
        ui.label(RichText::new(tr("logic.no_matrix")).weak().small());
    }
    if changed {
        let previews = draft
            .objects
            .iter()
            .zip(&draft.values)
            .filter_map(|(&o, values)| {
                let stored = level.object(o)?.transform?;
                Some((
                    o,
                    matrix_of(&stored, &Transform::from_matrix(&stored), values),
                ))
            })
            .collect();
        level.set_previews(previews);
    }
    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
    if escape && draft.values != draft.stored {
        draft.values = draft.stored.clone();
        level.set_previews(Vec::new());
    } else if release {
        level.set_previews(Vec::new());
        out.extend(transforms_action(level, &draft.objects, &draft.values));
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !locked && draft.values != draft.stored,
                egui::Button::new(tr("logic.reset")).small(),
            )
            .clicked()
        {
            draft.values = draft.stored.clone();
            level.set_previews(Vec::new());
        }
    });
}

/// The matrix of the edited values: a move keeps the stored rotation rows
/// exactly.
fn matrix_of(stored: &RowMatrix, split: &Transform, values: &Transform) -> RowMatrix {
    let mut m = *stored;
    if values.rotation != split.rotation || values.scale != split.scale {
        m = values.to_matrix();
    }
    m[3][..3].copy_from_slice(&values.position);
    m
}

/// The edit that places object `r` by the values of the Transform group;
/// `None` when they are its own.
pub(super) fn transform_action(
    level: &LevelView,
    r: ObjRef,
    values: &Transform,
) -> Option<LevelAction> {
    let stored = level.object(r)?.transform?;
    let split = Transform::from_matrix(&stored);
    if *values == split {
        return None;
    }
    let (group, object) = level.location_of(r)?;
    Some(LevelAction::SetTransform {
        group,
        object,
        matrix: matrix_of(&stored, &split, values),
    })
}

/// The edits that place every object by its values: one, or one
/// operation for several.
pub(super) fn transforms_action(
    level: &LevelView,
    objects: &[ObjRef],
    values: &[Transform],
) -> Option<LevelAction> {
    let mut actions: Vec<LevelAction> = objects
        .iter()
        .zip(values)
        .filter_map(|(&r, v)| transform_action(level, r, v))
        .collect();
    match actions.len() {
        0 => None,
        1 => actions.pop(),
        n => Some(LevelAction::Many {
            actions,
            label: trn("view.transformed_label", n as u64, &[]),
        }),
    }
}

/// One value along an axis: the axis's colour as a bar on the left, the
/// value dragged with the mouse or typed; "—" when the objects edited
/// together differ (`mixed`).
#[allow(clippy::too_many_arguments)]
fn axis_field(
    ui: &mut egui::Ui,
    axis: usize,
    value: &mut f32,
    speed: f64,
    decimals: usize,
    suffix: &str,
    width: f32,
    mixed: bool,
) -> egui::Response {
    let (letter, color) = match axis {
        0 => ("X", theme::AXIS_X),
        1 => ("Y", theme::AXIS_Y),
        _ => ("Z", theme::AXIS_Z),
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let height = ui.spacing().interact_size.y;
        let (bar, _) = ui.allocate_exact_size(Vec2::new(3.0, height), Sense::hover());
        ui.painter().rect_filled(bar, 1.0, color);
        let mut field = egui::DragValue::new(value)
            .speed(speed)
            .max_decimals(decimals)
            .suffix(if mixed { "" } else { suffix });
        if mixed {
            field = field.custom_formatter(|_, _| "—".to_owned());
        }
        ui.add_sized([width, height], field).on_hover_text(letter)
    })
    .inner
}

// -------------------------------------------------------------------- model

#[allow(clippy::too_many_arguments)]
fn model_ui(
    ui: &mut egui::Ui,
    state: &mut ObjectState,
    id: u64,
    level: &mut LevelView,
    r: ObjRef,
    location: Location,
    names: &mut dyn FnMut(usize, usize) -> Option<String>,
    out: &mut Vec<LevelAction>,
) {
    let model = level.object(r).and_then(|o| o.model);
    let label = |id: u32, names: &mut dyn FnMut(usize, usize) -> Option<String>| {
        let found = level
            .level()
            .and_then(|l| l.source.find(TYPE_METAMODEL, id));
        let name = found.and_then(|f| match f {
            Found::Game(at, _) => names(at.stream, at.resource),
            Found::Override(..) => None,
        });
        match name {
            Some(name) => format!("{id:08x} {name}"),
            None => format!("{id:08x}"),
        }
    };
    let current = model.map(|m| label(m, names));
    // The field takes a model dragged from a list of assets.
    let (_, dropped) = ui.dnd_drop_zone::<DraggedAsset, _>(
        egui::Frame::new().inner_margin(4).corner_radius(3),
        |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(tr("details.model_field"));
                ui.monospace(current.as_deref().unwrap_or(tr("details.no_model")));
            });
            ui.label(RichText::new(tr("details.model_drop_hint")).weak().small());
        },
    );
    if let Some(asset) = dropped {
        match set_model(level, r, location, *asset) {
            Ok(Some(a)) => {
                state.problem.clear();
                out.push(a);
            }
            Ok(None) => {}
            Err(e) => state.problem = e,
        }
    }
    if let Some(m) = model {
        ui.horizontal(|ui| {
            let side = 112.0;
            if !level.model_preview(ui, m, Vec2::splat(side)) {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
                ui.painter()
                    .rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    tr("details.no_preview"),
                    egui::FontId::proportional(11.0),
                    ui.visuals().weak_text_color(),
                );
            }
            ui.vertical(|ui| {
                if let Some(info) = model_info(level, m) {
                    ui.add(egui::Label::new(RichText::new(info).small()).wrap());
                }
                if ui
                    .button(tr("level.retexture"))
                    .on_hover_text(tr("level.retexture_hint"))
                    .clicked()
                {
                    out.push(LevelAction::StartWizard {
                        task: "retexture",
                        model: Some(m),
                    });
                }
            });
        });
    }
    egui::CollapsingHeader::new(tr("details.choose_model"))
        .id_salt(("details-choose-model", id))
        .show(ui, |ui| {
            let stale = state
                .candidates
                .as_ref()
                .is_none_or(|(l, s, _)| *l != id || *s != location.stream);
            if stale {
                let list = candidates(level, location.stream, names);
                state.candidates = Some((id, location.stream, list));
            }
            let (_, _, list) = state.candidates.as_ref().expect("set above");
            ui.add(
                egui::TextEdit::singleline(&mut state.model_search)
                    .hint_text(tr("details.model_search"))
                    .desired_width(ui.available_width()),
            );
            let needle = state.model_search.trim().to_lowercase();
            let shown: Vec<&Candidate> = list
                .iter()
                .filter(|c| needle.is_empty() || c.search.contains(&needle))
                .collect();
            ui.label(
                RichText::new(trn("details.models_found", shown.len() as u64, &[]))
                    .weak()
                    .small(),
            );
            let row = ui.spacing().interact_size.y;
            let mut chosen = None;
            egui::ScrollArea::vertical()
                .id_salt(("details-models", id))
                .max_height(220.0)
                .auto_shrink([false, true])
                .show_rows(ui, row, shown.len(), |ui, range| {
                    for c in &shown[range] {
                        let on = model == Some(c.id);
                        if ui
                            .add(
                                egui::Button::selectable(on, RichText::new(&c.label).monospace())
                                    .wrap_mode(egui::TextWrapMode::Truncate),
                            )
                            .clicked()
                            && !on
                        {
                            chosen = Some(c.id);
                        }
                    }
                });
            if let Some(m) = chosen {
                match model_action(level, r, location, m) {
                    Ok(Some(a)) => {
                        state.problem.clear();
                        out.push(a);
                    }
                    Ok(None) => {}
                    Err(e) => state.problem = e,
                }
            }
        });
}

/// Meshes, trees and nested models of a model, and where it is.
fn model_info(level: &LevelView, model: u32) -> Option<String> {
    let l = level.level()?;
    let m = l.models.get(&model)?;
    let (game, _) = level.game()?;
    Some(trf(
        "level.model_info",
        &[
            ("model", &format!("{model:08x}")),
            (
                "meshes",
                &trn("count.meshes", m.desc.pieces.len() as u64, &[]),
            ),
            ("trees", &trn("count.trees", m.desc.trees.len() as u64, &[])),
            (
                "nested",
                &trn(
                    "count.nested_metamodels",
                    m.desc.metamodels.saturating_sub(1) as u64,
                    &[],
                ),
            ),
            (
                "where",
                &match m.found {
                    None => tr("level.model_not_found").to_owned(),
                    Some(f) => trf(
                        "level.model_in",
                        &[("stream", &game.streams()[f.stream()].stream_name())],
                    ),
                },
            ),
        ],
    ))
}

/// Streams a zone reaches: itself and its ancestors.
fn reachable_streams(level: &LevelView, stream: usize) -> HashSet<usize> {
    let mut out = HashSet::from([stream]);
    let (Some((game, _)), Some(parents)) = (level.game(), level.stream_parents()) else {
        return out;
    };
    let name = game.streams()[stream].stream_name();
    for parent in doctor_toolkit::game::ancestors(parents, &name) {
        if let Some(s) = game.find_stream(&parent) {
            out.insert(s);
        }
    }
    out
}

/// The models a zone can show: `metamodel`s of the zone and its ancestors,
/// and the ones the workspace added.
fn candidates(
    level: &LevelView,
    stream: usize,
    names: &mut dyn FnMut(usize, usize) -> Option<String>,
) -> Vec<Candidate> {
    let Some((game, index)) = level.game() else {
        return Vec::new();
    };
    let streams = reachable_streams(level, stream);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for e in &index.entries {
        if e.header.type_id != TYPE_METAMODEL || !streams.contains(&(e.stream as usize)) {
            continue;
        }
        let id = e.header.ids[3];
        if !seen.insert(id) {
            continue;
        }
        let name = names(e.stream as usize, e.resource as usize).unwrap_or_default();
        let place = game.streams()[e.stream as usize].stream_name();
        let label = format!("{id:08x} {name}");
        let search = format!("{id:08x} {name} {place}").to_lowercase();
        out.push(Candidate { id, label, search });
        if out.len() >= MAX_CANDIDATES {
            break;
        }
    }
    if let Some(l) = level.level() {
        for &(t, id) in l.source.overrides.by_id.keys() {
            if t == TYPE_METAMODEL && seen.insert(id) {
                let label = format!("{id:08x} {}", tr("details.workspace_model"));
                out.push(Candidate {
                    id,
                    search: label.to_lowercase(),
                    label,
                });
            }
        }
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

/// A model dropped on the field: only a `metamodel` the zone reaches.
fn set_model(
    level: &LevelView,
    r: ObjRef,
    location: Location,
    asset: DraggedAsset,
) -> Result<Option<LevelAction>, String> {
    if asset.type_id != TYPE_METAMODEL {
        return Err(tr("details.drop_not_model").to_owned());
    }
    if !reachable_streams(level, location.stream).contains(&asset.stream) {
        let stream = level
            .game()
            .map(|(g, _)| g.streams()[asset.stream].stream_name())
            .unwrap_or_default();
        return Err(trf("level.note_unreachable", &[("stream", &stream)]));
    }
    model_action(level, r, location, asset.id)
}

/// The edit that gives object `r` model `model`.
pub(super) fn model_action(
    level: &LevelView,
    r: ObjRef,
    location: Location,
    model: u32,
) -> Result<Option<LevelAction>, String> {
    let l = level.level().ok_or_default()?;
    let mut group = l.groups[r.group].group.clone();
    match group.set_model(r.object, Some(model)) {
        Ok(false) => Ok(None),
        Ok(true) => {
            let class = level.object(r).and_then(|o| o.class_name).unwrap_or("?");
            Ok(Some(LevelAction::Save {
                group: location,
                data: group.encode(),
                label: trf(
                    "details.set_model_label",
                    &[
                        ("object", &r.object),
                        ("class", &class),
                        ("model", &format!("{model:08x}")),
                    ],
                ),
            }))
        }
        Err(e) => Err(e.to_string()),
    }
}

trait OkOrDefault<T> {
    fn ok_or_default(self) -> Result<T, String>;
}

impl<T> OkOrDefault<T> for Option<T> {
    fn ok_or_default(self) -> Result<T, String> {
        self.ok_or_else(|| tr("level.details_load_first").to_owned())
    }
}

// --------------------------------------------------------------- add object

/// "Add object": a model placed in front of the camera, this object its
/// template.
fn add_ui(
    ui: &mut egui::Ui,
    level: &mut LevelView,
    r: ObjRef,
    location: Location,
    out: &mut Vec<LevelAction>,
) {
    let has_transform = level.object(r).is_some_and(|o| o.transform.is_some());
    ui.label(RichText::new(tr("level.add_object_hint")).weak().small());
    ui.horizontal_wrapped(|ui| {
        ui.label(tr("level.metamodel_id"));
        ui.add(egui::TextEdit::singleline(&mut level.add.model).desired_width(90.0));
        ui.checkbox(&mut level.add.clone, tr("level.clone"))
            .on_hover_text(tr("level.clone_hint"));
        let clone = level.add.clone;
        ui.add_enabled(
            clone,
            egui::Checkbox::new(&mut level.add.collision, tr("level.own_collision")),
        )
        .on_hover_text(tr("level.own_collision_hint"));
    });
    let Some((game, _)) = level.game() else {
        return;
    };
    let game = game.clone();
    let id = u32::from_str_radix(level.add.model.trim(), 16).ok();
    let found = id.and_then(|id| level.level()?.source.find(TYPE_METAMODEL, id));
    let reachable =
        found.is_some_and(|f| reachable_streams(level, location.stream).contains(&f.stream()));
    let note = match (id, found) {
        (None, _) => tr("level.note_type_id").to_owned(),
        (Some(_), None) => tr("level.note_not_found").to_owned(),
        (Some(_), Some(f)) if level.add.clone => match f {
            Found::Game(..) => tr("level.note_clone_here").to_owned(),
            Found::Override(..) => tr("level.note_clone_game_only").to_owned(),
        },
        (Some(_), Some(_)) if reachable => tr("level.note_reachable").to_owned(),
        (Some(_), Some(f)) => trf(
            "level.note_unreachable",
            &[("stream", &game.streams()[f.stream()].stream_name())],
        ),
    };
    ui.label(RichText::new(note).small());
    if !level.add.fixed
        && ui
            .small_button(tr("level.place_wizard"))
            .on_hover_text(tr("level.place_wizard_hint"))
            .clicked()
    {
        out.push(LevelAction::StartWizard {
            task: "place",
            model: id,
        });
    }
    let can_add = matches!(
        (found, level.add.clone),
        (Some(Found::Game(..)), true) | (Some(_), false)
    );
    if ui
        .add_enabled(
            can_add && has_transform,
            egui::Button::new(tr("level.add_in_front")),
        )
        .clicked()
        && let (Some(id), Some(f)) = (id, found)
    {
        let model = match (level.add.clone, f) {
            (true, Found::Game(at, _)) if level.add.collision => {
                ModelChoice::CloneWithCollision(at, CollisionKind::Mesh, None)
            }
            (true, Found::Game(at, _)) => ModelChoice::Clone(at),
            _ => ModelChoice::Existing(id),
        };
        out.push(LevelAction::Add {
            group: location,
            template: (location, r.object),
            model,
            position: level.in_front(),
        });
    }
}

// -------------------------------------------------------------------- logic

fn logic_ui(
    ui: &mut egui::Ui,
    state: &mut ObjectState,
    id: u64,
    level: &mut LevelView,
    r: ObjRef,
    location: Location,
    out: &mut Vec<LevelAction>,
) {
    ui.horizontal_wrapped(|ui| {
        if ui
            .button(tr("logic.open_from_level"))
            .on_hover_text(tr("logic.open_from_level_hint"))
            .clicked()
        {
            out.push(LevelAction::ShowLogic {
                group: location,
                object: r.object,
            });
        }
        if ui
            .button(tr("outliner.open_simgroup"))
            .on_hover_text(tr("outliner.open_simgroup_hint"))
            .clicked()
        {
            out.push(LevelAction::OpenResource { group: location });
        }
    });
    let Some(l) = level.level() else { return };
    let group = &l.groups[r.group].group;
    let Some(o) = group.objects.get(r.object) else {
        return;
    };
    let revision = level.group_revision(r.group);
    let fresh = state
        .references
        .as_ref()
        .is_none_or(|(lv, obj, rev, _)| *lv != id || *obj != r || *rev != revision);
    if fresh {
        state.references = Some((id, r, revision, references(level, r)));
    }
    let (_, _, _, refs) = state.references.as_ref().expect("set above");
    let mut jump = None;
    egui::Grid::new(("details-logic-grid", id))
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            for link in &o.links {
                ui.label(tr("logic.field_needs"));
                ui.monospace(match link.resource_id() {
                    Some(id) => trf("logic.resource_id", &[("id", &format!("{id:08x}"))]),
                    None => link.to_string(),
                });
                ui.end_row();
            }
            if let Some(c) = o.components {
                let names: Vec<String> = group.packets[c]
                    .attrs
                    .iter()
                    .map(|a| {
                        sim_classes()
                            .get(a.raw)
                            .map_or_else(|| format!("{:08x}", a.raw), str::to_owned)
                    })
                    .collect();
                ui.label(tr("details.components"));
                ui.label(names.join(", "));
                ui.end_row();
            }
            for (title, list) in [
                (tr("details.refers_to"), &refs.to),
                (tr("details.referred_by"), &refs.from),
            ] {
                if list.is_empty() {
                    continue;
                }
                ui.label(title);
                ui.vertical(|ui| {
                    for &other in list.iter().take(50) {
                        let text = object_label(level, other);
                        if ui.link(text).clicked() {
                            jump = Some(other);
                        }
                    }
                    if list.len() > 50 {
                        ui.label(
                            RichText::new(trf("details.more", &[("count", &(list.len() - 50))]))
                                .weak(),
                        );
                    }
                });
                ui.end_row();
            }
            let packets: Vec<String> = group
                .object_packets(o)
                .map(|p| {
                    p.class_name
                        .map_or_else(|| p.class.to_string(), str::to_owned)
                })
                .collect();
            ui.label(tr("details.packets"));
            ui.label(packets.join(", "));
            ui.end_row();
        });
    if let Some(other) = jump {
        level.set_selection(Some(other), [other].into());
    }
}

/// "Class name" of an object, for links.
fn object_label(level: &LevelView, r: ObjRef) -> String {
    match level.object(r) {
        Some(o) => format!("{} {} #{}", o.class_name.unwrap_or("?"), o.name, o.index),
        None => format!("#{}", r.object),
    }
}

/// Objects of the group that `r` names in its attributes, and that name it.
fn references(level: &LevelView, r: ObjRef) -> References {
    let mut refs = References::default();
    let Some(l) = level.level() else { return refs };
    let group = &l.groups[r.group].group;
    let by_guid: HashMap<Guid, usize> = group
        .objects
        .iter()
        .enumerate()
        .map(|(i, o)| (o.guid, i))
        .collect();
    let guids_of = |o| -> Vec<Guid> {
        group
            .object_packets(o)
            .flat_map(|p| &p.attrs)
            .filter_map(|a| match a.value {
                Value::Guid(g) => Some(g),
                _ => None,
            })
            .collect()
    };
    let me = &group.objects[r.object];
    for g in guids_of(me) {
        if let Some(&i) = by_guid.get(&g)
            && i != r.object
        {
            refs.to.push(ObjRef {
                group: r.group,
                object: i,
            });
        }
    }
    for (i, o) in group.objects.iter().enumerate() {
        if i != r.object && guids_of(o).contains(&me.guid) {
            refs.from.push(ObjRef {
                group: r.group,
                object: i,
            });
        }
    }
    refs.to.dedup();
    refs
}

// ---------------------------------------------------------------- collision

fn collision_ui(
    ui: &mut egui::Ui,
    level: &mut LevelView,
    object: &doctor_toolkit::level::LevelObject,
) {
    let mut show = level.shows_collision();
    if ui
        .checkbox(&mut show, tr("level.collision"))
        .on_hover_text(tr("level.collision_hint"))
        .changed()
    {
        level.set_show_collision(show);
    }
    let collisions: Vec<u32> = object
        .model
        .and_then(|m| level.level()?.models.get(&m))
        .map(|m| m.desc.collisions.iter().map(|c| c.resource).collect())
        .unwrap_or_default();
    if collisions.is_empty() {
        ui.label(RichText::new(tr("details.no_collision")).weak());
    } else {
        ui.label(trn(
            "details.collision_shapes",
            collisions.len() as u64,
            &[],
        ));
        for c in collisions.iter().take(20) {
            ui.monospace(format!("hkx {c:08x}"));
        }
    }
}

// ----------------------------------------------------------------- advanced

fn advanced_ui(
    ui: &mut egui::Ui,
    state: &mut ObjectState,
    id: u64,
    level: &mut LevelView,
    r: ObjRef,
    location: Location,
    out: &mut Vec<LevelAction>,
) {
    let Some(l) = level.level() else { return };
    let lo = &l.groups[r.group];
    let Some(o) = lo.group.objects.get(r.object) else {
        return;
    };
    let guid = o.guid.to_string();
    egui::Grid::new(("details-advanced-grid", id))
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            ui.label("GUID");
            ui.horizontal(|ui| {
                ui.monospace(&guid);
                if ui.small_button(tr("outliner.copy_guid")).clicked() {
                    ui.ctx().copy_text(guid.clone());
                }
            });
            ui.end_row();
            ui.label(tr("logic.field_class"));
            ui.monospace(format!("{:08x}", o.class));
            ui.end_row();
            ui.label(tr("logic.field_flags"));
            ui.monospace(format!("{:#04x} {:#04x}", o.flags, o.flags2));
            ui.end_row();
            ui.label(tr("details.offset"));
            ui.monospace(format!("{:#x}", o.offset));
            ui.end_row();
            ui.label(tr("details.simgroup"));
            ui.monospace(format!(
                "#{} · {:08x} · v{}",
                location.resource, lo.group.group_hash, lo.group.version
            ));
            ui.end_row();
            if let Some(m) = level.object(r).and_then(|o| o.transform) {
                ui.label(tr("details.matrix"));
                ui.vertical(|ui| {
                    for row in m {
                        ui.monospace(format!(
                            "{:9.3} {:9.3} {:9.3} {:9.3}",
                            row[0], row[1], row[2], row[3]
                        ));
                    }
                });
                ui.end_row();
            }
        });
    ui.label(RichText::new(tr("details.attributes")).strong());
    let revision = level.group_revision(r.group);
    let fresh = state
        .logic
        .as_ref()
        .is_none_or(|(lv, g, rev, _)| *lv != id || *g != r.group || *rev != revision);
    if fresh {
        state.logic = level
            .logic_view(r.group)
            .map(|view| (id, r.group, revision, view));
    }
    let Some((_, _, _, view)) = &mut state.logic else {
        return;
    };
    let (jump, action) = view.object_attributes(ui, r.object);
    match action {
        Some(LogicAction::Save { data, label }) => out.push(LevelAction::Save {
            group: location,
            data,
            label,
        }),
        Some(LogicAction::OpenGraph { object }) => out.push(LevelAction::ShowLogic {
            group: location,
            object: object.unwrap_or(r.object),
        }),
        None => {}
    }
    if let Some(other) = jump {
        let other = ObjRef {
            group: r.group,
            object: other,
        };
        level.set_selection(Some(other), [other].into());
    }
}
