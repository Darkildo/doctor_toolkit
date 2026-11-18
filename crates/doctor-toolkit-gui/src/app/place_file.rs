//! A model file placed in a level in one step: the window that shows what
//! the import does and records it with the new object.
//!
//! The level view finds the place (a file dropped on it, or "Place a model
//! from a file…", `level_view::place_file`) and hands the file over here.
//! The window asks for nothing the modder has to look up in the game's
//! data: the *sample* — an object of the loaded zones, chosen by the editor
//! and shown — gives the new object its class and logic, the import its
//! template model, and both the stream they go into
//! (`doctor_toolkit::level::place`). What is left to choose is in the
//! modder's own terms: the name, the collision, the size of the textures.
//! "Place" records the import and the object as one operation.

use std::path::PathBuf;

use doctor_toolkit::import::model::ImportPlan;
use doctor_toolkit::level::place::{self, Sample, SampleKind};
use doctor_toolkit::scene::{self, Scene};
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::{Color32, RichText};

use super::{BrowserApp, Level};
use crate::level_view::place_file::PlaceOptions;
use crate::model_view::ModelPreview;

/// Samples tried for the first plan: the best one may not do as a template
/// (its metamodel keeps its meshes in another stream, say).
const TRIED: usize = 6;
/// Textures longer than this are scaled down unless the modder says
/// otherwise: the size of the game's larger textures, and a zone stays
/// within the budget of its partition of the world grid.
const DEFAULT_LIMIT: usize = 1024;
/// Sizes offered for the textures.
const LIMITS: [usize; 4] = [2048, 1024, 512, 256];

#[derive(Default)]
pub(super) struct PlaceFile {
    state: Option<State>,
    previews: u64,
}

struct State {
    /// The level tab the file was placed in.
    tab: u64,
    path: PathBuf,
    /// The file as it was read.
    scene: Scene,
    /// Longest side of its images, pixels (0: none).
    largest: usize,
    /// The file with its images scaled to a limit: the limit, the scene,
    /// how many images changed.
    scaled: Option<(usize, Scene, usize)>,
    position: [f32; 3],
    /// The place is on the collision of the level.
    on_ground: bool,
    name: String,
    samples: Vec<Sample>,
    sample: usize,
    /// `mesh`, `hull`, `box` or empty (the collision of the sample's model).
    collision: String,
    /// Longest side of the textures; 0 — as in the file.
    limit: usize,
    plan: Option<Result<ImportPlan, String>>,
    preview: Option<ModelPreview>,
}

fn kind_text(kind: SampleKind) -> &'static str {
    match kind {
        SampleKind::Static => tr("place.kind_static"),
        SampleKind::Prop => tr("place.kind_prop"),
        SampleKind::Other => tr("place.kind_other"),
        SampleKind::Terrain => tr("place.kind_terrain"),
    }
}

fn sample_text(sample: &Sample) -> String {
    let mut text = sample.class.clone();
    if !sample.name.is_empty() {
        text += &format!(" «{}»", sample.name);
    }
    trf(
        "place.sample_entry",
        &[
            ("object", &text),
            ("model", &format!("{:08x}", sample.model)),
            ("metres", &format!("{:.0}", sample.distance)),
        ],
    )
}

impl BrowserApp {
    /// A model file was placed in level tab `tab` at `position`: reads it,
    /// chooses the sample and works the import out. With `options` (a
    /// preset) the import is recorded at once; else the window opens.
    pub(super) fn place_file_start(
        &mut self,
        tab: u64,
        path: PathBuf,
        position: [f32; 3],
        on_ground: bool,
        options: Option<PlaceOptions>,
    ) {
        let scene = match scene::read_file(&path) {
            Ok(scene) => scene,
            Err(error) => {
                self.notify(
                    Level::Error,
                    trf(
                        "place.cannot_read",
                        &[("file", &path.display()), ("error", &error)],
                    ),
                );
                return;
            }
        };
        let samples = self
            .levels
            .get(&tab)
            .map(|level| level.place_samples(position))
            .unwrap_or_default();
        if samples.is_empty() {
            self.notify(Level::Error, tr("place.no_sample").to_owned());
            return;
        }
        let largest = place::largest_texture(&scene);
        let auto = options.is_some();
        let options = options.unwrap_or_default();
        let collision = match options.collision.as_deref() {
            Some("none") => String::new(),
            Some(kind) => kind.to_owned(),
            None if samples[0].collision => "hull".to_owned(),
            None => String::new(),
        };
        let mut state = State {
            tab,
            name: options.name.unwrap_or_else(|| place::name_from_file(&path)),
            path,
            scene,
            largest,
            scaled: None,
            position,
            on_ground,
            samples,
            sample: 0,
            collision,
            limit: options.limit.unwrap_or(if largest > DEFAULT_LIMIT {
                DEFAULT_LIMIT
            } else {
                0
            }),
            plan: None,
            preview: None,
        };
        // The first sample that can be the template of the import.
        for sample in 0..state.samples.len().min(TRIED) {
            state.sample = sample;
            self.place_file_plan(&mut state);
            if matches!(state.plan, Some(Ok(_))) {
                break;
            }
        }
        if !matches!(state.plan, Some(Ok(_))) {
            state.sample = 0;
            self.place_file_plan(&mut state);
        }
        if auto {
            if let Err(error) = self.place_file_record(&state) {
                self.notify(
                    Level::Error,
                    trf("place.cannot_place", &[("error", &error)]),
                );
            }
            return;
        }
        self.place_file.state = Some(state);
    }

    /// Works the import of `state` out with its current choices.
    fn place_file_plan(&mut self, state: &mut State) {
        state.preview = None;
        let plan = self.place_file_make_plan(state);
        if let Ok(plan) = &plan
            && let Ok(parsed) = plan.preview()
            && let Some(assets) = self.assets.as_mut()
        {
            self.place_file.previews += 1;
            // Keys of previews of this window; the import window and the
            // wizards count theirs from other ends.
            let key = u64::MAX / 4 - self.place_file.previews;
            state.preview = Some(ModelPreview::from_model(
                &parsed,
                key,
                &mut plan.assets(assets),
                self.gpu.as_ref(),
            ));
        }
        state.plan = Some(plan);
    }

    fn place_file_make_plan(&mut self, state: &mut State) -> Result<ImportPlan, String> {
        let (Some(game), Some(index)) = (self.game.clone(), self.index.clone()) else {
            return Err(tr("edits.no_game").to_owned());
        };
        // The plan reads the workspace: the materials an earlier import
        // added, the ids that are taken.
        self.workspace_mut()?;
        if state.limit > 0 && state.limit < state.largest {
            if state.scaled.as_ref().map(|s| s.0) != Some(state.limit) {
                let mut scene = state.scene.clone();
                let changed = place::limit_textures(&mut scene, state.limit);
                state.scaled = Some((state.limit, scene, changed));
            }
        } else {
            state.scaled = None;
        }
        let scene = state.scaled.as_ref().map_or(&state.scene, |s| &s.1);
        let sample = &state.samples[state.sample];
        let collision = doctor_toolkit::zone_ops::parse_collision(Some(&state.collision))
            .map_err(|e| e.to_string())?
            .filter(|_| sample.collision);
        let ws = self.workspace.as_ref().ok_or(tr("import.no_workspace"))?;
        let assets = self
            .assets
            .get_or_insert_with(|| doctor_toolkit::game::GameAssets::new(game.clone(), &index));
        place::plan(
            &game,
            &index,
            ws,
            assets,
            scene,
            &state.name,
            sample,
            collision,
        )
        .map_err(|e| e.to_string())
    }

    /// Records the import of `state` and the new object as one operation,
    /// and shows the object selected in its level tab.
    fn place_file_record(&mut self, state: &State) -> Result<(), String> {
        let (Some(game), Some(index)) = (self.game.clone(), self.index.clone()) else {
            return Err(tr("edits.no_game").to_owned());
        };
        let plan = match &state.plan {
            Some(Ok(plan)) => plan,
            Some(Err(error)) => return Err(error.clone()),
            None => return Err(tr("import.choose_file").to_owned()),
        };
        let sample = &state.samples[state.sample];
        let taken = self
            .levels
            .get(&state.tab)
            .map(|level| level.guids())
            .unwrap_or_default();
        let ws = self.workspace_mut()?;
        let placed = place::record(&game, &index, ws, plan, sample, state.position, &|guid| {
            taken.contains(guid)
        })
        .map_err(|e| e.to_string())?;
        self.status = trf(
            "place.placed",
            &[
                ("name", &state.name),
                ("object", &placed.object),
                ("model", &format!("{:08x}", placed.model)),
            ],
        );
        self.after_level_edit_selecting(state.tab, &[(placed.group, placed.object)]);
        Ok(())
    }

    /// The window of a placed model file.
    pub(super) fn show_place_file(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.place_file.state.take() else {
            return;
        };
        let mut open = true;
        let (mut replan, mut place, mut cancel) = (false, false, false);
        egui::Window::new(tr("place.title"))
            .open(&mut open)
            .default_size([720.0, 680.0])
            .resizable(true)
            .show(ctx, |ui| {
                egui::Grid::new("place file").num_columns(2).show(ui, |ui| {
                    ui.label(tr("import.file"));
                    ui.monospace(state.path.display().to_string());
                    ui.end_row();

                    ui.label(tr("place.where"));
                    let [x, y, z] = state.position;
                    let at = format!("{x:.1}, {y:.1}, {z:.1}");
                    ui.label(if state.on_ground {
                        trf("place.at_ground", &[("at", &at)])
                    } else {
                        trf("place.at_air", &[("at", &at)])
                    });
                    ui.end_row();

                    ui.label(tr("import.name"));
                    replan |= ui
                        .text_edit_singleline(&mut state.name)
                        .on_hover_text(tr("place.name_hint"))
                        .lost_focus();
                    ui.end_row();

                    ui.label(tr("place.like"))
                        .on_hover_text(tr("place.like_hint"));
                    egui::ComboBox::from_id_salt("place file sample")
                        .width(420.0)
                        .selected_text(sample_text(&state.samples[state.sample]))
                        .show_ui(ui, |ui| {
                            for (i, sample) in state.samples.iter().enumerate() {
                                replan |= ui
                                    .selectable_value(&mut state.sample, i, sample_text(sample))
                                    .on_hover_text(kind_text(sample.kind))
                                    .changed();
                            }
                        })
                        .response
                        .on_hover_text(tr("place.like_hint"));
                    ui.end_row();

                    ui.label("");
                    ui.label(
                        RichText::new(kind_text(state.samples[state.sample].kind))
                            .weak()
                            .small(),
                    );
                    ui.end_row();

                    ui.label(tr("import.collision"));
                    let label = |kind: &str| match kind {
                        "mesh" => tr("import.collision_mesh"),
                        "hull" => tr("import.collision_hull"),
                        "box" => tr("import.collision_box"),
                        _ => tr("place.collision_none"),
                    };
                    let can = state.samples[state.sample].collision;
                    ui.add_enabled_ui(can, |ui| {
                        egui::ComboBox::from_id_salt("place file collision")
                            .selected_text(label(if can { &state.collision } else { "" }))
                            .show_ui(ui, |ui| {
                                for kind in ["hull", "mesh", "box", ""] {
                                    replan |= ui
                                        .selectable_value(
                                            &mut state.collision,
                                            kind.to_owned(),
                                            label(kind),
                                        )
                                        .changed();
                                }
                            });
                    })
                    .response
                    .on_disabled_hover_text(tr("place.no_collision_part"));
                    ui.end_row();

                    if state.largest > 0 {
                        ui.label(tr("place.textures"));
                        let label = |limit: usize| match limit {
                            0 => trf("place.textures_as_file", &[("size", &state.largest)]),
                            n => trf("place.textures_limit", &[("size", &n)]),
                        };
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt("place file textures")
                                .selected_text(label(state.limit))
                                .show_ui(ui, |ui| {
                                    replan |= ui
                                        .selectable_value(&mut state.limit, 0, label(0))
                                        .changed();
                                    for limit in LIMITS {
                                        if limit < state.largest {
                                            replan |= ui
                                                .selectable_value(
                                                    &mut state.limit,
                                                    limit,
                                                    label(limit),
                                                )
                                                .changed();
                                        }
                                    }
                                })
                                .response
                                .on_hover_text(tr("place.textures_hint"));
                            if let Some((_, _, changed)) = &state.scaled {
                                ui.label(
                                    RichText::new(trf(
                                        "place.textures_scaled",
                                        &[("count", changed)],
                                    ))
                                    .weak()
                                    .small(),
                                );
                            }
                        });
                        ui.end_row();
                    }
                });
                ui.separator();
                match &state.plan {
                    None => {}
                    Some(Err(error)) => {
                        ui.colored_label(Color32::RED, error);
                    }
                    Some(Ok(plan)) => super::model_import::summary(ui, plan),
                }
                ui.horizontal(|ui| {
                    place = ui
                        .add_enabled(
                            matches!(state.plan, Some(Ok(_))),
                            egui::Button::new(RichText::new(tr("place.place")).strong()),
                        )
                        .on_hover_text(tr("place.place_hint"))
                        .clicked();
                    cancel = ui.button(tr("dialog.cancel")).clicked();
                });
                if let Some(preview) = &mut state.preview {
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_salt("place file preview")
                        .show(ui, |ui| preview.show(ui));
                }
            });
        if replan {
            self.place_file_plan(&mut state);
        }
        if place {
            match self.place_file_record(&state) {
                Ok(()) => return,
                Err(error) => self.notify(
                    Level::Error,
                    trf("place.cannot_place", &[("error", &error)]),
                ),
            }
        }
        if open && !cancel {
            self.place_file.state = Some(state);
        }
    }

    /// "Place a model from a file…": asks for the file; the level tab in
    /// front places it where the middle of its view looks.
    pub(super) fn place_file_dialog(&mut self) {
        let Some(tab) = self.front_level() else {
            return;
        };
        let Some(path) = super::model_import::model_file_dialog()
            .set_title(tr("place.choose_title"))
            .pick_file()
        else {
            return;
        };
        if let Some(level) = self.levels.get_mut(&tab) {
            level.request_place_file(path);
        }
    }

    /// Whether the window of a placed file is open.
    #[cfg(test)]
    pub(super) fn place_file_open_for_test(&self) -> bool {
        self.place_file.state.is_some()
    }

    /// Presses "Place" of the open window.
    #[cfg(test)]
    pub(super) fn place_file_confirm_for_test(&mut self) -> Result<(), String> {
        let state = self.place_file.state.take().ok_or("no window")?;
        self.place_file_record(&state)
    }
}
