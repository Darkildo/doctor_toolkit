//! The Zones panel of the shell (stage 53): which zones the level view
//! loads, what it shows, walking and navigation graphs. Up to stage 52 this
//! was the left side of the level tab.

use std::sync::atomic::Ordering;

use doctor_toolkit::workspace::Workspace;
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{Color32, RichText};

use super::scene::LodMode;
use super::{LevelView, math, walk};

impl LevelView {
    /// The Zones panel: zones to load, what the view shows, walking and
    /// navigation graphs.
    pub fn zones_panel(&mut self, ui: &mut egui::Ui, ws: Option<&Workspace>) {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        let load = egui::ScrollArea::vertical()
            .id_salt("level-zones-scroll")
            .auto_shrink(false)
            .show(ui, |ui| self.zones_ui(ui))
            .inner;
        if load {
            let ctx = ui.ctx().clone();
            self.start_load(&ctx, ws);
        }
    }

    /// Returns whether to load the chosen zones.
    fn zones_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let mut load = false;
        ui.horizontal(|ui| {
            let busy = self.job.is_some();
            let text = trf(
                "level.load_zones",
                &[("zones", &trn("count.zones", self.chosen.len() as u64, &[]))],
            );
            if ui
                .add_enabled(!self.chosen.is_empty() && !busy, egui::Button::new(text))
                .on_hover_text(tr("level.load_zones_hint"))
                .clicked()
            {
                load = true;
            }
            if ui.button(tr("level.clear")).clicked() {
                self.chosen.clear();
            }
            if self.level.is_some()
                && ui
                    .button(tr("gametab.play_here"))
                    .on_hover_text(tr("gametab.play_here_hint"))
                    .clicked()
            {
                self.play_here = true;
            }
        });
        if let Some(job) = &self.job {
            let total = job.total.load(Ordering::Relaxed).max(1);
            let done = job.done.load(Ordering::Relaxed);
            ui.add(egui::ProgressBar::new(done as f32 / total as f32).text(trf(
                "level.meshes_progress",
                &[("done", &done), ("total", &total)],
            )));
        }
        if !self.status.is_empty() {
            ui.label(RichText::new(&self.status).weak());
        }
        if self.level.is_some() {
            self.coverage_ui(ui);
        }
        ui.add(
            egui::TextEdit::singleline(&mut self.zone_filter)
                .hint_text(tr("level.filter_zones"))
                .desired_width(ui.available_width()),
        );
        self.zone_list(ui);
        if self.level.is_none() {
            return load;
        }
        egui::CollapsingHeader::new(RichText::new(tr("zones.show")).strong())
            .id_salt("level-show")
            .default_open(true)
            .show(ui, |ui| self.show_options(ui));
        egui::CollapsingHeader::new(RichText::new(tr("zones.walk")).strong())
            .id_salt("level-walk")
            .show(ui, |ui| {
                let selected = self.filter.selected.and_then(|r| self.object_transform(r));
                match self.walk.panel(ui, &self.camera, selected.is_some()) {
                    Some(walk::PanelAction::EnterAtCamera) => self.enter_walk(self.camera.eye),
                    Some(walk::PanelAction::EnterAtSelected) => {
                        if let Some(m) = selected {
                            // Beside the object, on what is below.
                            self.enter_walk([m[3][0] + 1.5, m[3][1] + 2.0, m[3][2]]);
                        }
                    }
                    None => {}
                }
            });
        egui::CollapsingHeader::new(RichText::new(tr("navgraph.title")).strong())
            .id_salt("level-navgraph")
            .default_open(self.nav.visible)
            .show(ui, |ui| {
                let object_at = self
                    .filter
                    .selected
                    .and_then(|r| self.object_transform(r))
                    .map(|m| [m[3][0], m[3][1], m[3][2]]);
                let target = math::add(self.camera.eye, math::scale(self.camera.forward(), 10.0));
                match self.nav.panel(ui, object_at, target) {
                    Some(Ok(a)) => self.nav_action = Some(a),
                    Some(Err(e)) => self.status = e,
                    None => {}
                }
                if self.nav.selected.is_some() && self.filter.selected.is_some() {
                    self.clear_selection();
                }
            });
        load
    }

    /// What was loaded: zones, objects, models, meshes, problems.
    fn coverage_ui(&self, ui: &mut egui::Ui) {
        let c = self.coverage;
        ui.label(
            RichText::new(trf(
                "level.coverage",
                &[
                    ("zones", &trn("count.zones", c.streams as u64, &[])),
                    ("scope", &c.scope_streams),
                    ("objects", &trn("count.objects", c.objects as u64, &[])),
                    ("placed", &c.placed),
                    ("with_model", &c.with_model),
                    ("drawn", &c.rendered),
                    (
                        "instances",
                        &trn("count.instances", c.instances as u64, &[]),
                    ),
                    ("meshes", &trn("count.meshes", c.meshes as u64, &[])),
                    ("missing", &c.meshes_missing),
                ],
            ))
            .small(),
        )
        .on_hover_text(&self.info);
        if !self.failed.is_empty() {
            ui.label(
                RichText::new(trf(
                    "level.meshes_failed",
                    &[
                        (
                            "meshes",
                            &trn("count.meshes", self.failed.len() as u64, &[]),
                        ),
                        ("first", &format!("{:08x}", self.failed[0].0)),
                        ("error", &self.failed[0].1),
                    ],
                ))
                .color(Color32::from_rgb(230, 120, 80))
                .small(),
            );
        }
    }

    /// Zones of every level, by folder, with check boxes.
    fn zone_list(&mut self, ui: &mut egui::Ui) {
        let filter = self.zone_filter.to_ascii_lowercase();
        let catalog = self.catalog.clone();
        egui::ScrollArea::vertical()
            .id_salt("level-zones")
            .max_height(ui.available_height() * if self.level.is_some() { 0.4 } else { 1.0 })
            .show(ui, |ui| {
                for level in catalog.levels() {
                    let zones: Vec<_> = catalog
                        .under(level)
                        .filter(|z| filter.is_empty() || z.name.contains(&filter))
                        .collect();
                    if zones.is_empty() {
                        continue;
                    }
                    let chosen = zones
                        .iter()
                        .filter(|z| self.chosen.contains(&z.stream))
                        .count();
                    let header = match chosen {
                        0 => format!("{level} ({})", zones.len()),
                        n => trf(
                            "level.zones_header_on",
                            &[("level", &level), ("count", &zones.len()), ("on", &n)],
                        ),
                    };
                    egui::CollapsingHeader::new(header)
                        .id_salt(("level", level))
                        .default_open(!filter.is_empty())
                        .show(ui, |ui| {
                            let mut dirs: Vec<&str> = zones.iter().map(|z| z.dir()).collect();
                            dirs.dedup();
                            for dir in dirs {
                                let in_dir: Vec<_> =
                                    zones.iter().filter(|z| z.dir() == dir).collect();
                                let all = in_dir.iter().all(|z| self.chosen.contains(&z.stream));
                                let mut check = all;
                                let label = format!("{dir}/ ({})", in_dir.len());
                                ui.horizontal(|ui| {
                                    if ui.checkbox(&mut check, "").changed() {
                                        for z in &in_dir {
                                            if check {
                                                self.chosen.insert(z.stream);
                                            } else {
                                                self.chosen.remove(&z.stream);
                                            }
                                        }
                                    }
                                    egui::CollapsingHeader::new(label)
                                        .id_salt(("dir", dir))
                                        .show(ui, |ui| {
                                            for z in &in_dir {
                                                self.zone_check(ui, z);
                                            }
                                        });
                                });
                            }
                        });
                }
            });
    }

    fn zone_check(&mut self, ui: &mut egui::Ui, z: &doctor_toolkit::level::Zone) {
        let mut on = self.chosen.contains(&z.stream);
        let name = z.name.rsplit('/').next().unwrap_or(&z.name);
        let text = trf(
            "level.zone_entry",
            &[
                ("name", &name),
                ("meshes", &trn("count.meshes", z.meshes as u64, &[])),
                ("mb", &format!("{:.1}", z.bytes as f64 / 1e6)),
            ],
        );
        if ui.checkbox(&mut on, text).changed() {
            if on {
                self.chosen.insert(z.stream);
            } else {
                self.chosen.remove(&z.stream);
            }
        }
    }

    /// What the view shows: kinds of things, levels of detail, drawing
    /// options and classes.
    fn show_options(&mut self, ui: &mut egui::Ui) {
        let before = (
            self.filter.meshes,
            self.filter.markers,
            self.filter.trees,
            self.filter.points,
            self.filter.lod,
            self.filter.hidden_classes.len(),
        );
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.filter.meshes, tr("level.models"));
            ui.checkbox(&mut self.filter.markers, tr("level.markers"))
                .on_hover_text(tr("level.markers_hint"));
            ui.checkbox(&mut self.filter.trees, tr("level.trees"))
                .on_hover_text(tr("level.trees_hint"));
            ui.checkbox(&mut self.filter.points, tr("level.ai_points"))
                .on_hover_text(tr("level.ai_points_hint"));
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("LOD");
            ui.selectable_value(
                &mut self.filter.lod,
                LodMode::Distance,
                tr("level.lod_distance"),
            );
            ui.selectable_value(
                &mut self.filter.lod,
                LodMode::Finest,
                tr("level.lod_finest"),
            );
            if ui
                .checkbox(&mut self.hide_tiny, tr("level.skip_tiny"))
                .on_hover_text(tr("level.skip_tiny_hint"))
                .changed()
            {
                self.refill = true;
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.scene.options.textures, tr("level.textures"));
            ui.checkbox(&mut self.scene.options.lighting, tr("level.lighting"));
            ui.checkbox(&mut self.scene.options.backface_culling, tr("level.cull"));
            ui.checkbox(&mut self.scene.options.wireframe, tr("level.wireframe"));
            ui.checkbox(&mut self.show_collision, tr("level.collision"))
                .on_hover_text(tr("level.collision_hint"));
        });
        if !self.filter.hidden.is_empty()
            && ui
                .button(tr("zones.show_hidden"))
                .on_hover_text(tr("zones.show_hidden_hint"))
                .clicked()
        {
            self.show_all();
        }
        egui::CollapsingHeader::new(trf("level.classes", &[("count", &self.classes.len())]))
            .id_salt("level-classes")
            .show(ui, |ui| {
                ui.label(RichText::new(tr("zones.classes_hint")).weak().small());
                ui.horizontal(|ui| {
                    if ui.small_button(tr("level.show_all")).clicked() {
                        self.filter.hidden_classes.clear();
                    }
                    if ui.small_button(tr("level.hide_all")).clicked() {
                        self.filter.hidden_classes = self.classes.iter().map(|c| c.0).collect();
                    }
                });
                egui::ScrollArea::vertical()
                    .id_salt("level-class-list")
                    .max_height(260.0)
                    .show(ui, |ui| {
                        for (class, name, count) in &self.classes {
                            let mut on = !self.filter.hidden_classes.contains(class);
                            if ui.checkbox(&mut on, format!("{name} ({count})")).changed() {
                                if on {
                                    self.filter.hidden_classes.remove(class);
                                } else {
                                    self.filter.hidden_classes.insert(*class);
                                }
                            }
                        }
                    });
            });
        let after = (
            self.filter.meshes,
            self.filter.markers,
            self.filter.trees,
            self.filter.points,
            self.filter.lod,
            self.filter.hidden_classes.len(),
        );
        if before != after {
            self.refill = true;
        }
    }
}
