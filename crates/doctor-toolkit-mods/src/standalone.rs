//! The window of Open Mod Manager in the look of the game's menus
//! ([`crate::skin`]): the mods in load order on the left, the chosen mod or
//! the check of the game on the right, the saved sets under the list, what
//! happened in a rust note in the corner. The Play button and the logo are
//! drawn by the program over this panel (`main.rs`). The editor's Mods
//! window keeps the cards of [`crate::cards`].

use std::time::{Duration, Instant};

use egui::{
    Align, Align2, Color32, CornerRadius, Layout, Rect, RichText, Sense, Stroke, UiBuilder, pos2,
    vec2,
};

use doctor_toolkit::mods::ModManager;

use crate::cards::{preview, target};
use crate::i18n::{tr, trf};
use crate::skin::{self, Art, Glyph};
use crate::state::ModState;
use crate::{ModsPanel, dialogs};

const ROW: f32 = 48.0;
const THUMB: egui::Vec2 = vec2(58.0, 33.0);
const SIDE: f32 = 380.0;
/// How long a note of success stays.
const NOTE: Duration = Duration::from_secs(6);

/// Which mods the list shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Filter {
    #[default]
    All,
    On,
    Attention,
}

/// The sign and colour of a state that needs the player; `None` for a mod
/// that is simply on, off or not written yet.
pub(crate) fn attention(state: &ModState) -> Option<(Glyph, Color32)> {
    match state {
        ModState::Ready | ModState::NotWritten | ModState::Off => None,
        ModState::Conflict(_) | ModState::Clash(_) => Some((Glyph::Bolt, skin::MUSTARD)),
        ModState::CodeWaiting { .. } | ModState::CodeBlocked(_) => {
            Some((Glyph::Shield, skin::BLOOD))
        }
        ModState::Damaged(_) | ModState::Incompatible(_) => Some((Glyph::Warn, skin::BLOOD)),
        ModState::NeedsMods(_)
        | ModState::Misfit(_)
        | ModState::WrongEdition { .. }
        | ModState::NeedsUpdate(_) => Some((Glyph::Warn, skin::MUSTARD)),
    }
}

/// One mod as the list shows it.
struct Row {
    id: String,
    name: String,
    line: String,
    enabled: bool,
    damaged: bool,
    state: ModState,
    rivals: Vec<String>,
}

/// What the list asks for.
enum Act {
    Select(String),
    Toggle(String, bool),
    Move(String, usize),
    Remove(String),
    Export(String),
}

/// A line of the check of the game.
struct Check {
    glyph: Glyph,
    color: Color32,
    title: String,
    detail: String,
    action: Option<(String, CheckAct)>,
}

#[derive(Clone)]
enum CheckAct {
    Attention,
    Select(String),
    Restore,
}

/// The state of a mod with drag of the list.
#[derive(Debug, Clone, Copy)]
struct RowDrag(usize);

impl ModsPanel {
    /// The pictures of the game for the window (read once per game and
    /// language).
    pub fn art(&mut self, ctx: &egui::Context) -> Art {
        self.art
            .get(ctx, self.game.as_ref(), crate::i18n::language())
            .clone()
    }

    /// Whether the mod manager has written anything into the game.
    pub fn has_written(&self) -> bool {
        self.manager.as_ref().is_some_and(ModManager::has_written)
    }

    /// Shows `text` in the note of the corner (an error stays until closed).
    pub fn note(&mut self, text: String, error: bool) {
        self.message = Some((text, error));
    }

    /// Asks before putting the originals back.
    pub fn ask_restore(&mut self) {
        self.dialog = Some(dialogs::Dialog::Restore);
    }

    /// (enabled, installed) mods.
    pub fn counts(&self) -> (usize, usize) {
        self.manager.as_ref().map_or((0, 0), |m| {
            (
                m.mods().iter().filter(|m| m.enabled).count(),
                m.mods().len(),
            )
        })
    }

    pub(crate) fn show_standalone(&mut self, ui: &mut egui::Ui) {
        let art = self.art(ui.ctx());
        let Some(manager) = &self.manager else {
            ui.add_space(16.0);
            if self.running.is_none() {
                ui.label(RichText::new(tr("mods.unreadable")).color(skin::BLOOD));
            }
            self.notes(ui.ctx());
            return;
        };
        if manager.mods().is_empty() {
            self.first_run(ui, &art);
        } else {
            let area = ui.available_rect_before_wrap();
            let wide = area.width() >= 820.0;
            if wide {
                let split = area.right() - SIDE;
                let left = Rect::from_min_max(area.min, pos2(split - 1.0, area.bottom()));
                let right = Rect::from_min_max(pos2(split, area.top()), area.max);
                ui.painter()
                    .rect_filled(right, CornerRadius::ZERO, skin::SOOT);
                ui.painter()
                    .vline(split - 0.5, area.y_range(), Stroke::new(1.0, skin::RULE));
                ui.scope_builder(UiBuilder::new().max_rect(left), |ui| {
                    self.list_pane(ui, &art)
                });
                ui.scope_builder(
                    UiBuilder::new().max_rect(right.shrink2(vec2(16.0, 14.0))),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("gf2-side")
                            .auto_shrink([false, false])
                            .show(ui, |ui| self.side_pane(ui, &art));
                    },
                );
                ui.allocate_rect(area, Sense::hover());
            } else {
                egui::ScrollArea::vertical()
                    .id_salt("gf2-narrow")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.tools(ui);
                        self.rows(ui, &art);
                        self.sets_line(ui);
                        ui.add_space(10.0);
                        self.side_pane(ui, &art);
                    });
            }
        }
        self.notes(ui.ctx());
    }

    // ---------------------------------------------------------- the list

    fn list_pane(&mut self, ui: &mut egui::Ui, art: &Art) {
        let full = ui.max_rect();
        let foot = 40.0;
        let top = Rect::from_min_max(full.min, pos2(full.right(), full.bottom() - foot));
        let bottom = Rect::from_min_max(pos2(full.left(), full.bottom() - foot), full.max);
        ui.scope_builder(UiBuilder::new().max_rect(top), |ui| {
            self.tools(ui);
            egui::ScrollArea::vertical()
                .id_salt("gf2-rows")
                .auto_shrink([false, false])
                .show(ui, |ui| self.rows(ui, art));
        });
        ui.painter()
            .hline(full.x_range(), bottom.top(), Stroke::new(1.0, skin::RULE));
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(bottom.shrink2(vec2(14.0, 6.0)))
                .layout(Layout::left_to_right(Align::Center)),
            |ui| self.sets_line(ui),
        );
    }

    fn tools(&mut self, ui: &mut egui::Ui) {
        let idle = self.running.is_none() && self.manager.is_some();
        let (all, attention) = self.manager.as_ref().map_or((0, 0), |m| {
            let n = m
                .mods()
                .iter()
                .filter(|x| attention(&ModState::of(x, m, &self.overview)).is_some())
                .count();
            (m.mods().len(), n)
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.spacing_mut().item_spacing.x = 10.0;
            skin::glyph(ui, Glyph::Search, skin::BONE_DIM, 15.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .align(Align2::LEFT_CENTER)
                    .min_size(vec2(0.0, 24.0))
                    .hint_text(tr("gf2.search"))
                    .desired_width(150.0)
                    .frame(egui::Frame::NONE),
            );
            ui.add_space(6.0);
            for (filter, label) in [
                (Filter::All, trf("gf2.filter_all", &[("n", &all)])),
                (Filter::On, tr("gf2.filter_on").to_owned()),
                (
                    Filter::Attention,
                    trf("gf2.filter_attention", &[("n", &attention)]),
                ),
            ] {
                if self.filter_tab(ui, &label, self.filter == filter).clicked() {
                    self.filter = filter;
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(12.0);
                self.menu(ui, idle);
                if skin::button(ui, tr("mods.sort"), idle && all > 1, false)
                    .on_hover_text(tr("mods.sort_hint"))
                    .clicked()
                {
                    self.sort(ui.ctx());
                }
                if skin::button(ui, tr("mods.install"), idle, false)
                    .on_hover_text(tr("mods.install_hint"))
                    .clicked()
                {
                    self.pick_and_install();
                }
            });
        });
        ui.add_space(6.0);
        let y = ui.cursor().top();
        ui.painter()
            .hline(ui.max_rect().x_range(), y, Stroke::new(1.0, skin::RULE));
        ui.add_space(4.0);
    }

    fn filter_tab(&self, ui: &mut egui::Ui, label: &str, on: bool) -> egui::Response {
        let font = skin::caps(ui.ctx(), 13.0);
        let color = if on { skin::BONE } else { skin::BONE_DIM };
        let galley = ui
            .painter()
            .layout_no_wrap(label.to_uppercase(), font, color);
        let (rect, response) =
            ui.allocate_exact_size(galley.size() + vec2(8.0, 8.0), Sense::click());
        let color = if response.hovered() {
            skin::CREAM
        } else {
            color
        };
        ui.painter()
            .galley(rect.min + vec2(4.0, 2.0), galley, color);
        if on {
            ui.painter().hline(
                rect.x_range(),
                rect.bottom() - 1.0,
                Stroke::new(2.0, skin::MUSTARD),
            );
        }
        response
    }

    fn pick_and_install(&mut self) {
        if let Some(paths) = rfd::FileDialog::new()
            .set_title(tr("mods.install_title"))
            .add_filter(tr("mods.filter_mods"), &["zip", "vpat"])
            .pick_files()
        {
            self.offer_install(paths);
        }
    }

    fn rows(&mut self, ui: &mut egui::Ui, art: &Art) {
        let Some(manager) = &self.manager else { return };
        let needle = self.search.trim().to_lowercase();
        let all: Vec<Row> = manager
            .mods()
            .iter()
            .map(|m| {
                let state = ModState::of(m, manager, &self.overview);
                let rivals = match &state {
                    ModState::Conflict(r) => r.iter().map(|r| r.name.clone()).collect(),
                    ModState::Clash(names) => names.clone(),
                    _ => Vec::new(),
                };
                let mut line = m.manifest.version.clone();
                if !m.manifest.author.is_empty() {
                    line += &format!(" · {}", m.manifest.author);
                }
                Row {
                    id: m.id.clone(),
                    name: m.manifest.name.clone(),
                    line,
                    enabled: m.enabled,
                    damaged: m.problem.is_some(),
                    state,
                    rivals,
                }
            })
            .collect();
        let pictures: Vec<Option<egui::TextureHandle>> = manager
            .mods()
            .iter()
            .map(|m| preview(&mut self.previews, ui.ctx(), m))
            .collect();
        let shown: Vec<usize> = (0..all.len())
            .filter(|&i| {
                let r = &all[i];
                let text = needle.is_empty()
                    || r.name.to_lowercase().contains(&needle)
                    || r.line.to_lowercase().contains(&needle);
                let filter = match self.filter {
                    Filter::All => true,
                    Filter::On => r.enabled,
                    Filter::Attention => attention(&r.state).is_some(),
                };
                text && filter
            })
            .collect();
        let idle = self.running.is_none();
        // Order changes by dragging only when every mod is in the list.
        let reorder = idle && shown.len() == all.len();
        let ctx = ui.ctx().clone();
        let dragging = egui::DragAndDrop::payload::<RowDrag>(&ctx).map(|d| d.0);
        let highlight = self.highlight.take();
        let mut act = None;
        let mut rects: Vec<(usize, Rect)> = Vec::new();
        ui.spacing_mut().item_spacing.y = 2.0;
        if shown.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(tr("gf2.nothing_found")).color(skin::BONE_DIM));
            });
        }
        for &i in &shown {
            let row = &all[i];
            let width = ui.available_width() - 12.0;
            let (rect, response) =
                ui.allocate_exact_size(vec2(width, ROW), Sense::click_and_drag());
            let rect = rect.translate(vec2(6.0, 0.0));
            rects.push((i, rect));
            if reorder {
                response.dnd_set_drag_payload(RowDrag(i));
            }
            let selected = self.selected.as_deref() == Some(row.id.as_str());
            let lit = highlight
                .as_ref()
                .is_some_and(|names: &Vec<String>| names.contains(&row.name));
            let painter = ui.painter().clone();
            if selected {
                skin::paint_brush(&painter, rect.shrink2(vec2(0.0, 3.0)), art);
            } else if lit {
                painter.rect_filled(rect, CornerRadius::ZERO, skin::MUSTARD.gamma_multiply(0.14));
                painter.vline(
                    rect.left() + 1.5,
                    rect.y_range(),
                    Stroke::new(3.0, skin::MUSTARD),
                );
            } else if response.hovered() {
                painter.rect_filled(rect, CornerRadius::ZERO, skin::SOOT);
            }
            let ink = if selected { skin::INK } else { skin::BONE };
            let dim = if selected {
                Color32::from_rgb(0x3d, 0x34, 0x09)
            } else {
                skin::BONE_DIM
            };
            let fade = if row.enabled || selected { 1.0 } else { 0.45 };
            let alpha = if dragging == Some(i) { 0.4 } else { 1.0 };
            let mut x = rect.left() + 8.0;
            let cy = rect.center().y;
            // The handle: six dots.
            if reorder {
                for (dx, dy) in [
                    (0.0, -5.0),
                    (5.0, -5.0),
                    (0.0, 0.0),
                    (5.0, 0.0),
                    (0.0, 5.0),
                    (5.0, 5.0),
                ] {
                    painter.circle_filled(
                        pos2(x + 2.0 + dx, cy + dy),
                        1.2,
                        dim.gamma_multiply(alpha),
                    );
                }
            }
            x += 16.0;
            painter.text(
                pos2(x + 16.0, cy),
                Align2::RIGHT_CENTER,
                (i + 1).to_string(),
                skin::caps(&ctx, 15.0),
                if selected { dim } else { skin::SEPIA },
            );
            x += 24.0;
            let thumb = Rect::from_min_size(pos2(x, cy - THUMB.y / 2.0), THUMB);
            painter.rect_filled(
                thumb.expand(2.0),
                CornerRadius::ZERO,
                if selected { skin::OXBLOOD } else { skin::EMBER },
            );
            match &pictures[i] {
                Some(t) => {
                    skin::paint_cover(&painter, thumb, t, Color32::WHITE.gamma_multiply(fade))
                }
                None => skin::paint_card(&painter, thumb, art),
            }
            if !row.enabled && !selected {
                painter.rect_filled(thumb, CornerRadius::ZERO, Color32::from_black_alpha(120));
            }
            x = thumb.right() + 12.0;
            // Signs and the switch on the right.
            let mut right = rect.right() - 8.0;
            let switch = Rect::from_min_size(pos2(right - 36.0, cy - 9.0), vec2(36.0, 18.0));
            right = switch.left() - 10.0;
            let mut sign = None;
            if let Some((g, color)) = attention(&row.state) {
                let r = Rect::from_min_size(pos2(right - 24.0, cy - 10.0), vec2(24.0, 20.0));
                let color = if selected { skin::INK } else { color };
                painter.rect_stroke(
                    r,
                    CornerRadius::ZERO,
                    Stroke::new(1.0, color),
                    egui::StrokeKind::Inside,
                );
                skin::paint_glyph(&painter, r.shrink2(vec2(6.0, 4.0)), g, color);
                sign = Some(r);
                right = r.left() - 8.0;
            }
            let text_w = (right - x).max(40.0);
            let name = painter.layout(
                row.name.clone(),
                skin::display(&ctx, 16.5),
                ink.gamma_multiply(fade),
                text_w,
            );
            let name_h = name.size().y.min(22.0);
            painter
                .with_clip_rect(Rect::from_min_max(
                    pos2(x, rect.top()),
                    pos2(right, rect.bottom()),
                ))
                .galley(pos2(x, cy - name_h + 1.0), name, ink);
            painter
                .with_clip_rect(Rect::from_min_max(
                    pos2(x, rect.top()),
                    pos2(right, rect.bottom()),
                ))
                .text(
                    pos2(x, cy + 3.0),
                    Align2::LEFT_TOP,
                    &row.line,
                    egui::FontId::proportional(12.0),
                    dim.gamma_multiply(fade),
                );
            // The switch as a widget of its own, in a child that does not move
            // the cursor of the list.
            let mut on = row.enabled;
            let mut child = ui.new_child(UiBuilder::new().max_rect(switch));
            let toggled = skin::toggle(&mut child, &mut on, idle && !row.damaged).on_hover_text(
                if row.enabled {
                    tr("mods.switch_off")
                } else {
                    tr("mods.switch_on")
                },
            );
            if toggled.changed() {
                act = Some(Act::Toggle(row.id.clone(), on));
            }
            if let Some(r) = sign {
                let hover = ui.interact(r, ui.scope_id().with(("gf2-sign", i)), Sense::hover());
                if hover.hovered() {
                    let mut names = row.rivals.clone();
                    names.push(row.name.clone());
                    self.highlight = Some(names);
                }
                hover.on_hover_text(format!("{}\n{}", row.state.text(), row.state.hint()));
            }
            if response.clicked() {
                act = Some(Act::Select(row.id.clone()));
            }
            if response.dragged() && reorder {
                ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            }
            response.context_menu(|ui| {
                let count = all.len();
                ui.add_enabled_ui(idle, |ui| {
                    if reorder {
                        let moves = [
                            (tr("mods.move_up"), i > 0, i.saturating_sub(1)),
                            (tr("mods.move_down"), i + 1 < count, i + 1),
                            (tr("mods.move_first"), i > 0, 0),
                            (tr("mods.move_last"), i + 1 < count, count - 1),
                        ];
                        for (label, can, to) in moves {
                            if ui.add_enabled(can, egui::Button::new(label)).clicked() {
                                act = Some(Act::Move(row.id.clone(), to));
                                ui.close();
                            }
                        }
                        ui.separator();
                    }
                    let label = if row.enabled {
                        tr("mods.switch_off")
                    } else {
                        tr("mods.switch_on")
                    };
                    if ui
                        .add_enabled(!row.damaged, egui::Button::new(label))
                        .clicked()
                    {
                        act = Some(Act::Toggle(row.id.clone(), !row.enabled));
                        ui.close();
                    }
                    if ui
                        .add_enabled(!row.damaged, egui::Button::new(tr("mods.export")))
                        .on_hover_text(tr("mods.export_hint"))
                        .clicked()
                    {
                        act = Some(Act::Export(row.id.clone()));
                        ui.close();
                    }
                    if ui.button(tr("mods.remove")).clicked() {
                        act = Some(Act::Remove(row.id.clone()));
                        ui.close();
                    }
                });
            });
        }
        // A dragged mod dropped between others.
        if let (Some(from), Some(pos)) = (dragging, ctx.pointer_interact_pos())
            && let (Some(first), Some(last)) = (rects.first(), rects.last())
        {
            let area = Rect::from_min_max(first.1.min, last.1.max).expand2(vec2(0.0, 20.0));
            if area.contains(pos) {
                let slot = rects.iter().filter(|(_, r)| r.center().y < pos.y).count();
                let y = if slot == 0 {
                    first.1.top() - 1.0
                } else if slot == rects.len() {
                    last.1.bottom() + 1.0
                } else {
                    (rects[slot - 1].1.bottom() + rects[slot].1.top()) / 2.0
                };
                if target(from, slot).is_some() {
                    ui.painter()
                        .hline(first.1.x_range(), y, Stroke::new(3.0, skin::RUST));
                }
                if ctx.input(|i| i.pointer.any_released())
                    && egui::DragAndDrop::take_payload::<RowDrag>(&ctx).is_some()
                    && let Some(to) = target(from, slot)
                {
                    act = Some(Act::Move(all[from].id.clone(), to));
                }
            }
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.add(
                egui::Label::new(
                    RichText::new(tr("mods.order_hint"))
                        .small()
                        .color(skin::BONE_DIM),
                )
                .wrap(),
            );
        });
        match act {
            Some(Act::Select(id)) => {
                self.selected = if self.selected.as_deref() == Some(id.as_str()) {
                    None
                } else {
                    Some(id)
                };
            }
            Some(Act::Toggle(id, on)) => self.set_enabled(&ctx, &id, on),
            Some(Act::Move(id, to)) => {
                self.selected = Some(id.clone());
                self.move_mod(&ctx, &id, to);
            }
            Some(Act::Remove(id)) => self.ask_remove(&id),
            Some(Act::Export(id)) => {
                if let Some(out) = rfd::FileDialog::new()
                    .set_title(tr("mods.export_title"))
                    .set_file_name(format!("{id}.zip"))
                    .add_filter(tr("mods.filter_mods"), &["zip"])
                    .save_file()
                {
                    self.export_mod(&ctx, &id, out);
                }
            }
            None => {}
        }
    }

    fn sets_line(&mut self, ui: &mut egui::Ui) {
        let idle = self.running.is_none() && self.manager.is_some();
        let current = self.overview.current_set.clone();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            skin::caps_label(ui, tr("mods.set"), skin::SEPIA);
            ui.add_space(6.0);
            for name in self.overview.sets.clone() {
                let on = current.as_deref() == Some(name.as_str());
                let font = egui::FontId::proportional(13.0);
                let color = if on { skin::BONE } else { skin::BONE_DIM };
                let galley = ui.painter().layout_no_wrap(name.clone(), font, color);
                let sense = if idle { Sense::click() } else { Sense::hover() };
                let (rect, response) =
                    ui.allocate_exact_size(galley.size() + vec2(14.0, 10.0), sense);
                let color = if response.hovered() {
                    skin::CREAM
                } else {
                    color
                };
                ui.painter()
                    .galley(rect.min + vec2(7.0, 3.0), galley, color);
                if on {
                    ui.painter().hline(
                        rect.x_range(),
                        rect.bottom() - 1.0,
                        Stroke::new(2.0, skin::RUST),
                    );
                }
                if response.on_hover_text(tr("mods.set_hint")).clicked() && !on {
                    self.use_set(ui.ctx(), &name);
                }
            }
            if current.is_none() && !self.overview.sets.is_empty() {
                ui.label(
                    RichText::new(tr("mods.set_unsaved"))
                        .italics()
                        .color(skin::BONE_DIM),
                );
            }
            ui.add_space(8.0);
            if ui
                .add_enabled(
                    idle,
                    egui::Button::new(RichText::new(tr("mods.set_save")).color(skin::BONE_DIM))
                        .frame(false),
                )
                .clicked()
            {
                self.dialog = Some(dialogs::Dialog::SaveSet(
                    current.clone().unwrap_or_default(),
                ));
            }
            if let Some(name) = &current
                && ui
                    .add_enabled(
                        idle,
                        egui::Button::new(
                            RichText::new(trf("mods.set_delete", &[("name", name)]))
                                .color(skin::BONE_DIM),
                        )
                        .frame(false),
                    )
                    .clicked()
            {
                self.dialog = Some(dialogs::Dialog::DeleteSet(name.clone()));
            }
        });
    }

    // ------------------------------------------------------ the side pane

    fn side_pane(&mut self, ui: &mut egui::Ui, art: &Art) {
        let chosen = self
            .selected
            .clone()
            .filter(|id| self.manager.as_ref().is_some_and(|m| m.get(id).is_some()));
        match chosen {
            Some(id) => self.chosen_mod(ui, art, &id),
            None => self.game_check(ui, false),
        }
    }

    fn chosen_mod(&mut self, ui: &mut egui::Ui, art: &Art, id: &str) {
        let Some(manager) = &self.manager else { return };
        let Some(m) = manager.get(id) else { return };
        let state = ModState::of(m, manager, &self.overview);
        let picture = preview(&mut self.previews, ui.ctx(), m);
        let idle = self.running.is_none();
        let name = m.manifest.name.clone();
        let version = m.manifest.version.clone();
        let author = m.manifest.author.clone();
        let url = m.manifest.url.clone();
        let description = m.manifest.description.clone();
        let enabled = m.enabled;
        let damaged = m.problem.is_some();
        let mut close = false;
        let mut ask: Option<Act> = None;
        let mut with_deps = false;
        let mut code = None;
        ui.horizontal(|ui| {
            skin::caps_label(ui, tr("gf2.mod"), skin::SEPIA);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (rect, r) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                let color = if r.hovered() {
                    skin::CREAM
                } else {
                    skin::BONE_DIM
                };
                skin::paint_glyph(ui.painter(), rect, Glyph::Close, color);
                if r.on_hover_text(tr("gf2.close")).clicked() {
                    close = true;
                }
            });
        });
        ui.add_space(4.0);
        // The picture in the rust frame of a portrait of the game.
        let width = ui.available_width();
        let (frame, _) = ui.allocate_exact_size(vec2(width, 186.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(
            frame.translate(vec2(0.0, 3.0)),
            CornerRadius::ZERO,
            skin::OXBLOOD,
        );
        p.rect_filled(frame, CornerRadius::ZERO, skin::RUST);
        p.rect_stroke(
            frame.shrink(3.0),
            CornerRadius::ZERO,
            Stroke::new(2.0, skin::EMBER),
            egui::StrokeKind::Inside,
        );
        let inner = frame.shrink(6.0);
        match &picture {
            Some(t) => skin::paint_cover(p, inner, t, Color32::WHITE),
            None => skin::paint_card(p, inner, art),
        }
        ui.add_space(10.0);
        ui.add(
            egui::Label::new(
                RichText::new(&name)
                    .font(skin::display(ui.ctx(), 27.0))
                    .color(skin::BONE),
            )
            .wrap(),
        );
        let mut line = version.clone();
        if !author.is_empty() {
            line += &format!(" · {author}");
        }
        ui.label(RichText::new(line).color(skin::BONE_DIM));
        if let Some(url) = &url {
            ui.hyperlink(url);
        }
        if let Some((g, color)) = attention(&state) {
            ui.add_space(4.0);
            skin::tag(ui, Some(g), &state.text(), color);
            ui.add(egui::Label::new(RichText::new(state.hint()).color(color)).wrap());
        }
        if !description.is_empty() {
            ui.add_space(6.0);
            ui.add(
                egui::Label::new(
                    RichText::new(&description).color(Color32::from_rgb(0xd8, 0xcf, 0xba)),
                )
                .wrap(),
            );
        }
        if let Some(manager) = &self.manager
            && let Some(m) = manager.get(id)
            && crate::details::dependencies(ui, manager, m, idle)
        {
            with_deps = true;
        }
        if let Some(view) = self.overview.code.iter().find(|c| c.question.id == id)
            && let Some(answer) = crate::native::section_ui(ui, view, idle)
        {
            code = Some(answer);
        }
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            let label = if enabled {
                tr("mods.switch_off")
            } else {
                tr("mods.switch_on")
            };
            if skin::button(ui, label, idle && !damaged, true).clicked() {
                ask = Some(Act::Toggle(id.to_owned(), !enabled));
            }
            if skin::button(ui, tr("mods.remove"), idle, false).clicked() {
                ask = Some(Act::Remove(id.to_owned()));
            }
            if skin::button(ui, tr("mods.export"), idle && !damaged, false)
                .on_hover_text(tr("mods.export_hint"))
                .clicked()
            {
                ask = Some(Act::Export(id.to_owned()));
            }
        });
        if let ModState::Conflict(rivals) = &state {
            self.rule(ui);
            skin::caps_label(ui, tr("gf2.overlaps"), skin::SEPIA);
            for r in rivals {
                ui.horizontal_wrapped(|ui| {
                    skin::glyph(ui, Glyph::Bolt, skin::MUSTARD, 14.0);
                    let key = if r.wins {
                        "gf2.overridden"
                    } else {
                        "gf2.overrides"
                    };
                    ui.label(trf(key, &[("name", &r.name), ("n", &r.resources)]));
                });
            }
            ui.label(
                RichText::new(tr("gf2.overlap_rule"))
                    .small()
                    .color(skin::BONE_DIM),
            );
        }
        self.rule(ui);
        self.technical_sections(ui);
        let ctx = ui.ctx().clone();
        if close {
            self.selected = None;
        }
        if with_deps {
            self.enable_with_dependencies(&ctx, id);
        }
        if let Some(answer) = code {
            self.code_answered(&ctx, id, answer);
        }
        match ask {
            Some(Act::Toggle(id, on)) => self.set_enabled(&ctx, &id, on),
            Some(Act::Remove(id)) => self.ask_remove(&id),
            Some(Act::Export(id)) => {
                if let Some(out) = rfd::FileDialog::new()
                    .set_title(tr("mods.export_title"))
                    .set_file_name(format!("{id}.zip"))
                    .add_filter(tr("mods.filter_mods"), &["zip"])
                    .save_file()
                {
                    self.export_mod(&ctx, &id, out);
                }
            }
            _ => {}
        }
    }

    fn rule(&self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        let y = ui.cursor().top();
        ui.painter()
            .hline(ui.max_rect().x_range(), y, Stroke::new(1.0, skin::RULE));
        ui.add_space(8.0);
    }

    /// The check of the game: edition, loader, what of the mods needs the
    /// player, interrupted writes, crash reports.
    fn game_check(&mut self, ui: &mut egui::Ui, first_run: bool) {
        let mut checks = Vec::new();
        if let Some(game) = &self.game {
            let root = game.root();
            match self.edition.as_ref() {
                Some((name, true)) => checks.push(Check {
                    glyph: Glyph::Check,
                    color: skin::OLIVE,
                    title: trf("gf2.edition", &[("edition", name)]),
                    detail: tr("gf2.edition_ok").to_owned(),
                    action: None,
                }),
                Some((name, false)) => checks.push(Check {
                    glyph: Glyph::Warn,
                    color: skin::MUSTARD,
                    title: trf("gf2.edition", &[("edition", name)]),
                    detail: tr("gf2.edition_data").to_owned(),
                    action: None,
                }),
                None => {}
            }
            if doctor_toolkit::live::loader::find(root).is_some() {
                checks.push(Check {
                    glyph: Glyph::Check,
                    color: skin::OLIVE,
                    title: tr("gf2.loader_ok").to_owned(),
                    detail: tr("gf2.loader_ok_hint").to_owned(),
                    action: None,
                });
            } else {
                checks.push(Check {
                    glyph: Glyph::Warn,
                    color: skin::MUSTARD,
                    title: tr("gf2.loader_missing").to_owned(),
                    detail: tr("gf2.loader_missing_hint").to_owned(),
                    action: None,
                });
            }
        }
        if let Some(reason) = &self.write_lock {
            checks.push(Check {
                glyph: Glyph::Warn,
                color: skin::MUSTARD,
                title: tr("mods.game_running").to_owned(),
                detail: reason.clone(),
                action: None,
            });
        }
        if !self.overview.interrupted.is_empty() {
            checks.push(Check {
                glyph: Glyph::Warn,
                color: skin::BLOOD,
                title: tr("gf2.interrupted").to_owned(),
                detail: self.overview.interrupted.join(", "),
                action: Some((tr("mods.unapply").to_owned(), CheckAct::Restore)),
            });
        }
        if !self.overview.workspace_held.is_empty() {
            checks.push(Check {
                glyph: Glyph::Warn,
                color: skin::BLOOD,
                title: tr("mods.workspace_held").to_owned(),
                detail: String::new(),
                action: None,
            });
        } else if self.overview.foreign_changes() {
            checks.push(Check {
                glyph: Glyph::Warn,
                color: skin::BLOOD,
                title: tr("mods.foreign_changes").to_owned(),
                detail: String::new(),
                action: None,
            });
        }
        if let Some(manager) = &self.manager {
            let mut conflicts = Vec::new();
            let mut code = Vec::new();
            let mut other = Vec::new();
            for m in manager.mods() {
                let state = ModState::of(m, manager, &self.overview);
                match state {
                    ModState::Conflict(_) | ModState::Clash(_) => conflicts.push(m),
                    ModState::CodeWaiting { .. } | ModState::CodeBlocked(_) => code.push(m),
                    s if attention(&s).is_some() => other.push(m),
                    _ => {}
                }
            }
            let names = |list: &[&doctor_toolkit::mods::manager::InstalledMod]| {
                list.iter()
                    .map(|m| m.manifest.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            if !conflicts.is_empty() {
                checks.push(Check {
                    glyph: Glyph::Bolt,
                    color: skin::MUSTARD,
                    title: trf("gf2.conflicts", &[("n", &conflicts.len())]),
                    detail: names(&conflicts),
                    action: Some((tr("gf2.show").to_owned(), CheckAct::Attention)),
                });
            }
            if let Some(first) = code.first() {
                checks.push(Check {
                    glyph: Glyph::Shield,
                    color: skin::BLOOD,
                    title: tr("gf2.code").to_owned(),
                    detail: names(&code),
                    action: Some((
                        tr("gf2.decide").to_owned(),
                        CheckAct::Select(first.id.clone()),
                    )),
                });
            }
            if !other.is_empty() {
                checks.push(Check {
                    glyph: Glyph::Warn,
                    color: skin::MUSTARD,
                    title: trf("gf2.problems", &[("n", &other.len())]),
                    detail: names(&other),
                    action: Some((tr("gf2.show").to_owned(), CheckAct::Attention)),
                });
            }
        }
        skin::caps_label(
            ui,
            if first_run {
                tr("gf2.ready_title")
            } else {
                tr("gf2.check_title")
            },
            skin::SEPIA,
        );
        ui.add_space(6.0);
        let mut chosen = None;
        for check in &checks {
            if let Some(act) = check_line(ui, check) {
                chosen = Some(act);
            }
            ui.add_space(4.0);
        }
        // Crash reports: the question once, then the last report.
        if let Some(watch) = &mut self.crash {
            if self.write_lock.is_some() {
                ui.ctx().request_repaint_after(Duration::from_secs(2));
            }
            if let Some(report) = watch.poll(false) {
                self.message = Some((crate::crash::CrashWatch::announcement(report), true));
            }
            ui.add_space(6.0);
            watch.ui(ui);
        }
        match chosen {
            Some(CheckAct::Attention) => self.filter = Filter::Attention,
            Some(CheckAct::Select(id)) => self.selected = Some(id),
            Some(CheckAct::Restore) => self.ask_restore(),
            None => {}
        }
    }

    // ------------------------------------------------------- first run

    fn first_run(&mut self, ui: &mut egui::Ui, art: &Art) {
        let area = ui.available_rect_before_wrap();
        let wide = area.width() >= 820.0;
        let (left, right) = if wide {
            let split = area.right() - 400.0;
            (
                Rect::from_min_max(area.min, pos2(split, area.bottom())),
                Some(Rect::from_min_max(pos2(split, area.top()), area.max)),
            )
        } else {
            (area, None)
        };
        let drop = left.shrink(22.0);
        let drop = Rect::from_min_size(drop.min, vec2(drop.width(), drop.height().min(380.0)));
        let p = ui.painter().clone();
        skin::dashed_frame(&p, drop, skin::RULE);
        let card =
            Rect::from_center_size(pos2(drop.center().x, drop.top() + 86.0), vec2(84.0, 84.0));
        p.rect_filled(
            card.expand(4.0).translate(vec2(0.0, 4.0)),
            CornerRadius::ZERO,
            skin::OXBLOOD,
        );
        p.rect_filled(card.expand(4.0), CornerRadius::ZERO, skin::RUST);
        skin::paint_card(&p, card, art);
        let mut install = false;
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    pos2(drop.left() + 20.0, card.bottom() + 18.0),
                    drop.max - vec2(20.0, 10.0),
                ))
                .layout(Layout::top_down(Align::Center)),
            |ui| {
                ui.label(
                    RichText::new(tr("mods.empty_title"))
                        .font(skin::display(ui.ctx(), 30.0))
                        .color(skin::BONE),
                );
                ui.add_space(6.0);
                ui.add(
                    egui::Label::new(RichText::new(tr("gf2.empty_text")).color(skin::BONE_DIM))
                        .wrap(),
                );
                ui.add_space(12.0);
                if skin::button(ui, tr("mods.install"), self.running.is_none(), true).clicked() {
                    install = true;
                }
            },
        );
        if install {
            self.pick_and_install();
        }
        if let Some(right) = right {
            ui.painter()
                .rect_filled(right, CornerRadius::ZERO, skin::SOOT);
            ui.painter()
                .vline(right.left(), right.y_range(), Stroke::new(1.0, skin::RULE));
            ui.scope_builder(
                UiBuilder::new().max_rect(right.shrink2(vec2(18.0, 18.0))),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("gf2-first")
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.game_check(ui, true));
                },
            );
        } else {
            ui.allocate_rect(
                Rect::from_min_max(area.min, pos2(area.right(), drop.bottom() + 10.0)),
                Sense::hover(),
            );
            self.game_check(ui, true);
            return;
        }
        ui.allocate_rect(area, Sense::hover());
    }

    // ------------------------------------------------------------ notes

    /// What happened, in a rust note at the bottom right: the running job,
    /// the last result (an error stays until closed) and its undo.
    fn notes(&mut self, ctx: &egui::Context) {
        let running = self.running.as_ref().map(|r| r.label.clone());
        if let Some((text, _)) = &self.message {
            if self.note_shown.as_ref().map(|(t, _)| t) != Some(text) {
                self.note_shown = Some((text.clone(), Instant::now()));
            }
        } else {
            self.note_shown = None;
        }
        let message = self.message.clone().filter(|(_, error)| {
            *error
                || self
                    .note_shown
                    .as_ref()
                    .is_some_and(|(_, at)| at.elapsed() < NOTE)
        });
        if let Some((_, at)) = &self.note_shown
            && at.elapsed() < NOTE
        {
            ctx.request_repaint_after(
                NOTE.saturating_sub(at.elapsed()) + Duration::from_millis(50),
            );
        }
        let undo = self
            .undo
            .as_ref()
            .map(|u| trf("mods.undo", &[("what", &u.label)]));
        if running.is_none() && message.is_none() {
            return;
        }
        let mut close = false;
        let mut do_undo = false;
        egui::Area::new(egui::Id::unique("gf2-note"))
            .anchor(Align2::RIGHT_BOTTOM, vec2(-20.0, -18.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let error = message.as_ref().is_some_and(|(_, e)| *e);
                let fill = if running.is_some() {
                    skin::ASH
                } else if error {
                    Color32::from_rgb(0x6a, 0x10, 0x06)
                } else {
                    skin::RUST
                };
                let back = ui.painter().add(egui::Shape::Noop);
                let inner = egui::Frame::NONE
                    .inner_margin(egui::Margin {
                        left: 24,
                        right: 22,
                        top: 10,
                        bottom: 12,
                    })
                    .show(ui, |ui| {
                        ui.set_max_width(460.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;
                            if let Some(label) = &running {
                                ui.spinner();
                                ui.label(RichText::new(label).color(skin::BONE));
                            } else if let Some((text, _)) = &message {
                                ui.add(
                                    egui::Label::new(RichText::new(text).color(skin::CREAM)).wrap(),
                                );
                                if let Some(label) = &undo
                                    && ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new(label.to_uppercase())
                                                    .font(skin::caps(ui.ctx(), 12.5))
                                                    .color(Color32::from_rgb(0xff, 0xe0, 0x8a)),
                                            )
                                            .frame(false),
                                        )
                                        .on_hover_text(tr("mods.undo_hint"))
                                        .clicked()
                                {
                                    do_undo = true;
                                }
                                let (r, resp) =
                                    ui.allocate_exact_size(vec2(14.0, 14.0), Sense::click());
                                skin::paint_glyph(ui.painter(), r, Glyph::Close, skin::CREAM);
                                if resp.on_hover_text(tr("gf2.close")).clicked() {
                                    close = true;
                                }
                            }
                        });
                    });
                let r = inner.response.rect;
                let slant = 10.0;
                ui.painter().set(
                    back,
                    egui::Shape::convex_polygon(
                        vec![
                            r.left_top(),
                            r.right_top(),
                            pos2(r.right() - slant, r.bottom()),
                            pos2(r.left() + slant, r.bottom()),
                        ],
                        fill,
                        Stroke::NONE,
                    ),
                );
                ui.painter().hline(
                    (r.left() + slant)..=(r.right() - slant),
                    r.bottom() - 2.0,
                    Stroke::new(4.0, skin::OXBLOOD),
                );
            });
        if close {
            self.message = None;
        }
        if do_undo {
            self.undo(ctx);
        }
    }
}

/// A line of the check; its button, when clicked.
fn check_line(ui: &mut egui::Ui, check: &Check) -> Option<CheckAct> {
    let mut chosen = None;
    let frame = egui::Frame::NONE
        .fill(skin::COAL)
        .inner_margin(egui::Margin {
            left: 10,
            right: 8,
            top: 7,
            bottom: 7,
        });
    let response = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            skin::glyph(ui, check.glyph, check.color, 16.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                let w = ui.available_width() - if check.action.is_some() { 84.0 } else { 0.0 };
                ui.set_max_width(w.max(80.0));
                ui.label(RichText::new(&check.title).color(skin::BONE));
                if !check.detail.is_empty() {
                    ui.add(
                        egui::Label::new(
                            RichText::new(&check.detail).small().color(skin::BONE_DIM),
                        )
                        .wrap(),
                    );
                }
            });
            if let Some((label, act)) = &check.action {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if skin::button(ui, label, true, false).clicked() {
                        chosen = Some(act.clone());
                    }
                });
            }
        });
    });
    let r = response.response.rect;
    ui.painter()
        .vline(r.left() + 1.5, r.y_range(), Stroke::new(3.0, check.color));
    chosen
}
