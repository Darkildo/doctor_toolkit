//! The bar over the level view (stage 54), as in the corner of a viewport
//! of Unreal Engine: the projection (perspective, from above, the front,
//! the side), the lighting (lit, unlit, wireframe), "Show" — what is drawn
//! (the same switches as the Zones panel's "Show"), and on the right the
//! snapping steps, the camera speed and "?" with the keys. Under it the
//! statistics of the frame when "Stats" is on, the walk and short notes.

use doctor_toolkit::resource::simgroup_edit::Transform;
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::{Color32, Rect, RichText, Vec2};

use super::camera::{Projection, SPEED_RANGE};
use super::{LevelView, SNAP_ANGLES, SNAP_SCALES, SNAP_STEPS, math, scene::LodMode};
use crate::app::theme::{self, Icon, Tone};

/// Switches of the bar that are not elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Bar {
    /// The statistics of the frame and the objects under the pointer.
    pub stats: bool,
    /// A grid on the ground.
    pub grid: bool,
}

/// How the scene is lit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lighting {
    Lit,
    /// Textures and colours as they are, no light.
    Unlit,
    /// The edges of the triangles over the scene.
    Wireframe,
}

impl Lighting {
    pub const ALL: [Lighting; 3] = [Self::Lit, Self::Unlit, Self::Wireframe];

    fn label(self) -> &'static str {
        match self {
            Self::Lit => tr("view.lit"),
            Self::Unlit => tr("view.unlit"),
            Self::Wireframe => tr("view.wireframe"),
        }
    }
}

fn projection_label(p: Projection) -> &'static str {
    match p {
        Projection::Perspective => tr("view.perspective"),
        Projection::Top => tr("view.top"),
        Projection::Front => tr("view.front"),
        Projection::Side => tr("view.side"),
    }
}

/// A translucent backing so the buttons read over any scene.
fn backing(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(Color32::from_black_alpha(150))
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(3, 2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(add);
        });
}

impl LevelView {
    /// The camera as before stage 54 (the setting "Camera: as before").
    pub fn set_classic_camera(&mut self, on: bool) {
        self.classic = on;
    }

    #[cfg(test)]
    pub fn projection(&self) -> Projection {
        self.camera.projection
    }

    /// Switches the projection; an orthographic view centres on the
    /// selection or on what is ahead.
    pub fn set_projection(&mut self, projection: Projection) {
        let distance = self
            .gizmo_target()
            .map(|t| math::length(math::sub(t.origin, self.camera.eye)))
            .filter(|d| *d > 1.0)
            .unwrap_or(40.0);
        self.camera.set_projection(projection, distance);
        if projection.is_ortho() && self.walk.active {
            self.walk.exit();
        }
        self.refill = true;
    }

    pub fn lighting(&self) -> Lighting {
        let o = &self.scene.options;
        if o.wireframe {
            Lighting::Wireframe
        } else if o.lighting {
            Lighting::Lit
        } else {
            Lighting::Unlit
        }
    }

    pub fn set_lighting(&mut self, lighting: Lighting) {
        let o = &mut self.scene.options;
        o.wireframe = lighting == Lighting::Wireframe;
        o.lighting = lighting != Lighting::Unlit;
    }

    #[cfg(test)]
    pub fn camera_speed(&self) -> f32 {
        self.camera.speed
    }

    pub fn set_camera_speed(&mut self, speed: f32) {
        self.camera.speed = speed.clamp(*SPEED_RANGE.start(), *SPEED_RANGE.end());
    }

    /// The switches of the bar.
    #[cfg(test)]
    pub fn view_bar(&self) -> Bar {
        self.bar
    }

    #[cfg(test)]
    pub fn set_view_bar(&mut self, bar: Bar) {
        self.bar = bar;
    }

    /// Where the view was drawn last (for tests driving it).
    #[cfg(test)]
    pub fn view_rect(&self) -> Rect {
        self.view_rect
    }

    /// A world point on the screen of the view, as last drawn.
    #[cfg(test)]
    pub fn screen_of(&self, p: [f32; 3]) -> Option<egui::Pos2> {
        let rect = self.view_rect;
        let (uv, _) = self.camera.view(rect.aspect_ratio()).project(p)?;
        Some(egui::Pos2::new(
            rect.min.x + uv[0] * rect.width(),
            rect.min.y + uv[1] * rect.height(),
        ))
    }

    /// The bar over the view.
    pub(super) fn bar_ui(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let area = Rect::from_min_max(
            rect.left_top() + Vec2::splat(6.0),
            egui::pos2(rect.right() - 6.0, rect.top() + 34.0),
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(area)
                .layout(egui::Layout::left_to_right(egui::Align::Min)),
            |ui| {
                backing(ui, |ui| self.left_buttons(ui));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    backing(ui, |ui| {
                        // Right to left inside too: the help is outermost.
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            self.right_buttons(ui)
                        });
                    });
                });
            },
        );
    }

    /// Projection, lighting, Show.
    fn left_buttons(&mut self, ui: &mut egui::Ui) {
        let projection = self.camera.projection;
        let button = theme::tool_button(
            ui,
            None,
            &format!("{} ▾", projection_label(projection)),
            false,
            Tone::Flat,
        )
        .on_hover_text(tr("view.projection_hint"));
        let mut chosen = None;
        egui::Popup::menu(&button).show(|ui| {
            for p in Projection::ALL {
                if ui
                    .selectable_label(p == projection, projection_label(p))
                    .clicked()
                {
                    chosen = Some(p);
                }
            }
        });
        if let Some(p) = chosen {
            self.set_projection(p);
        }

        let lighting = self.lighting();
        let button = theme::tool_button(
            ui,
            None,
            &format!("{} ▾", lighting.label()),
            false,
            Tone::Flat,
        )
        .on_hover_text(tr("view.lighting_hint"));
        let mut chosen = None;
        let before = (self.filter.lod, self.hide_tiny);
        egui::Popup::menu(&button).show(|ui| {
            for l in Lighting::ALL {
                if ui.selectable_label(l == lighting, l.label()).clicked() {
                    chosen = Some(l);
                }
            }
            ui.separator();
            let mut game = crate::render::game_look();
            let shading = ui
                .horizontal(|ui| {
                    ui.radio_value(&mut game, true, tr("view.shading_game"))
                        .changed()
                        | ui.radio_value(&mut game, false, tr("view.shading_simple"))
                            .changed()
                })
                .inner;
            if shading {
                crate::render::set_game_look(game);
            }
            if game {
                ui.add(
                    egui::Slider::new(&mut self.hours, 0.0..=24.0)
                        .text(tr("view.time_of_day"))
                        .custom_formatter(|h, _| {
                            let m = (h * 60.0).round() as u32 % (24 * 60);
                            format!("{:02}:{:02}", m / 60, m % 60)
                        }),
                )
                .on_hover_text(tr("view.time_of_day_hint"));
            }
            ui.label(RichText::new(tr("view.shading_hint")).weak().small());
            ui.separator();
            let o = &mut self.scene.options;
            ui.checkbox(&mut o.textures, tr("level.textures"));
            ui.checkbox(&mut o.backface_culling, tr("level.cull"));
            ui.separator();
            ui.label(RichText::new("LOD").weak());
            ui.radio_value(
                &mut self.filter.lod,
                LodMode::Distance,
                tr("level.lod_distance"),
            );
            ui.radio_value(
                &mut self.filter.lod,
                LodMode::Finest,
                tr("level.lod_finest"),
            );
            ui.checkbox(&mut self.hide_tiny, tr("level.skip_tiny"))
                .on_hover_text(tr("level.skip_tiny_hint"));
        });
        if let Some(l) = chosen {
            self.set_lighting(l);
        }
        if before != (self.filter.lod, self.hide_tiny) {
            self.refill = true;
        }

        let button = theme::tool_button(
            ui,
            None,
            &format!("{} ▾", tr("view.show")),
            false,
            Tone::Flat,
        )
        .on_hover_text(tr("view.show_hint"));
        egui::Popup::menu(&button)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| self.show_menu(ui));
    }

    /// "Show": the switches of the Zones panel's "Show" and the view's own.
    fn show_menu(&mut self, ui: &mut egui::Ui) {
        let before = (
            self.filter.meshes,
            self.filter.markers,
            self.filter.trees,
            self.filter.points,
            self.nav.visible,
        );
        ui.checkbox(&mut self.filter.meshes, tr("level.models"));
        ui.checkbox(&mut self.filter.markers, tr("level.markers"))
            .on_hover_text(tr("level.markers_hint"));
        ui.checkbox(&mut self.filter.trees, tr("level.trees"))
            .on_hover_text(tr("level.trees_hint"));
        ui.checkbox(&mut self.filter.points, tr("level.ai_points"))
            .on_hover_text(tr("level.ai_points_hint"));
        ui.checkbox(&mut self.show_collision, tr("level.collision"))
            .on_hover_text(tr("level.collision_hint"));
        ui.checkbox(&mut self.nav.visible, tr("view.navigation"))
            .on_hover_text(tr("view.navigation_hint"));
        ui.separator();
        ui.checkbox(&mut self.bar.grid, tr("view.grid"));
        ui.checkbox(&mut self.bar.stats, tr("view.stats"))
            .on_hover_text(tr("view.stats_hint"));
        if !self.filter.hidden.is_empty() {
            ui.separator();
            if ui
                .button(tr("zones.show_hidden"))
                .on_hover_text(tr("zones.show_hidden_hint"))
                .clicked()
            {
                self.show_all();
            }
        }
        let after = (
            self.filter.meshes,
            self.filter.markers,
            self.filter.trees,
            self.filter.points,
            self.nav.visible,
        );
        if before != after {
            self.refill = true;
            self.nav.generation += 1;
        }
    }

    /// Help, camera speed, snapping steps (laid out right to left).
    fn right_buttons(&mut self, ui: &mut egui::Ui) {
        let help = theme::tool_button(ui, None, "?", false, Tone::Flat)
            .on_hover_text(tr("view.help_hint"));
        egui::Popup::menu(&help).show(|ui| self.help(ui));

        let speed = self.camera.speed;
        let text = trf(
            "view.speed",
            &[("speed", &format!("{:.0}", speed.max(1.0)))],
        );
        let button = theme::tool_button(ui, None, &format!("{text} ▾"), false, Tone::Flat)
            .on_hover_text(tr("view.speed_hint"));
        egui::Popup::menu(&button).show(|ui| {
            ui.label(RichText::new(tr("view.speed_title")).weak());
            let mut speed = self.camera.speed;
            if ui
                .add(
                    egui::Slider::new(&mut speed, SPEED_RANGE)
                        .logarithmic(true)
                        .suffix(tr("view.speed_unit")),
                )
                .changed()
            {
                self.set_camera_speed(speed);
            }
        });

        let mut snap = self.snap;
        let mut changed = false;
        for (kind, text) in [
            (2, format!("×{} ▾", snap.scale)),
            (1, format!("{}° ▾", snap.angle)),
            (
                0,
                format!("{} ▾", trf("toolbar.metres", &[("m", &snap.step)])),
            ),
        ] {
            let button = theme::tool_button(ui, None, &text, false, Tone::Flat).on_hover_text(tr(
                match kind {
                    0 => "toolbar.snap_move",
                    1 => "toolbar.snap_rotate",
                    _ => "toolbar.snap_scale",
                },
            ));
            egui::Popup::menu(&button).show(|ui| match kind {
                0 => {
                    for step in SNAP_STEPS {
                        changed |= ui
                            .selectable_value(
                                &mut snap.step,
                                step,
                                trf("toolbar.metres", &[("m", &step)]),
                            )
                            .changed();
                    }
                }
                1 => {
                    for angle in SNAP_ANGLES {
                        changed |= ui
                            .selectable_value(&mut snap.angle, angle, format!("{angle}°"))
                            .changed();
                    }
                }
                _ => {
                    for scale in SNAP_SCALES {
                        changed |= ui
                            .selectable_value(&mut snap.scale, scale, format!("×{scale}"))
                            .changed();
                    }
                }
            });
        }
        if theme::tool_button(ui, Some(Icon::Snap), "", snap.on, Tone::Flat)
            .on_hover_text(tr("toolbar.snap_hint"))
            .clicked()
        {
            snap.on = !snap.on;
            changed = true;
        }
        if changed {
            self.snap = snap;
        }
    }

    /// The keys and the mouse of the view.
    fn help(&self, ui: &mut egui::Ui) {
        ui.set_min_width(360.0);
        ui.strong(if self.classic {
            tr("view.help_title_classic")
        } else {
            tr("view.help_title")
        });
        let text = if self.classic {
            tr("view.help_classic")
        } else {
            tr("view.help_ue")
        };
        egui::Grid::new("level-view-help")
            .num_columns(2)
            .spacing([12.0, 2.0])
            .striped(true)
            .show(ui, |ui| {
                for line in text.lines() {
                    let (keys, what) = line.split_once(" — ").unwrap_or((line, ""));
                    ui.label(RichText::new(keys).monospace());
                    ui.label(what);
                    ui.end_row();
                }
            });
        ui.label(RichText::new(tr("view.help_setting")).weak().small());
    }

    /// Under the bar: the statistics (with "Stats"), the walk, a note; the
    /// frame being drawn.
    pub(super) fn hud(&self, ui: &egui::Ui, rect: Rect) {
        let painter = ui.painter_at(rect);
        if let Some(m) = &self.marquee {
            let frame = m.rect();
            painter.rect_filled(frame, 0.0, theme::FOCUS.gamma_multiply(0.15));
            painter.rect_stroke(
                frame,
                0.0,
                egui::Stroke::new(1.0, theme::FOCUS),
                egui::StrokeKind::Inside,
            );
        }
        let mut text = String::new();
        if self.bar.stats {
            text = trf(
                "level.overlay_stats",
                &[
                    ("drawn", &self.stats.instances),
                    ("culled", &self.stats.culled),
                    ("calls", &self.stats.draw_calls),
                    (
                        "ms",
                        &format!("{:.2}", self.stats.cpu_time.as_secs_f64() * 1000.0),
                    ),
                ],
            );
            if let Some(level) = &self.level {
                for r in [self.filter.hovered, self.filter.selected]
                    .into_iter()
                    .flatten()
                {
                    let o = &level.groups[r.group].objects[r.object];
                    let mut line = format!("\n{} {}", o.class_name.unwrap_or("?"), o.name);
                    if let Some(m) = self.object_transform(r) {
                        let t = Transform::from_matrix(&m);
                        line += &trf(
                            "level.overlay_at",
                            &[
                                (
                                    "position",
                                    &format!(
                                        "{:.2} {:.2} {:.2}",
                                        t.position[0], t.position[1], t.position[2]
                                    ),
                                ),
                                ("yaw", &format!("{:.1}", t.rotation[1])),
                            ],
                        );
                    }
                    text += &line;
                }
                let selected = self.filter.selection.len();
                if selected > 1 {
                    text += &format!(
                        "\n{}",
                        trf("details.many_selected", &[("count", &selected)])
                    );
                }
            }
        }
        if let Some(level) = self.level.as_ref().filter(|_| self.walk.active) {
            if !text.is_empty() {
                text += "\n";
            }
            text += &self.walk.hud(level);
        }
        let mut at = rect.left_top() + Vec2::new(8.0, 40.0);
        if !text.is_empty() {
            let galley =
                painter.layout_no_wrap(text, egui::FontId::monospace(11.0), Color32::WHITE);
            painter.rect_filled(
                Rect::from_min_size(at, galley.size()).expand(4.0),
                3.0,
                Color32::from_black_alpha(150),
            );
            let height = galley.size().y;
            painter.galley(at, galley, Color32::WHITE);
            at.y += height + 12.0;
        }
        if let Some(note) = self.current_note() {
            let galley = painter.layout_no_wrap(
                note.to_owned(),
                egui::FontId::proportional(13.0),
                Color32::WHITE,
            );
            painter.rect_filled(
                Rect::from_min_size(at, galley.size()).expand(5.0),
                3.0,
                theme::FOCUS.gamma_multiply(0.8),
            );
            painter.galley(at, galley, Color32::WHITE);
        }
    }
}
