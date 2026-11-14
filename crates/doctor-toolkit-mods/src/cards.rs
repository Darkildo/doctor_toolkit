//! The list of mods as cards: picture, name, author, version, a short
//! description, the state in words and colour, an on/off switch. Cards are
//! reordered by dragging; their context menu moves, switches and removes.

use std::collections::HashMap;

use egui::{
    Align, Color32, CornerRadius, Layout, Rect, RichText, Sense, Stroke, StrokeKind, UiBuilder,
    vec2,
};

use doctor_toolkit::mods::manager::InstalledMod;

use crate::i18n::{tr, trf};
use crate::state::ModState;
use crate::{ModsPanel, theme, widgets};

const CARD_HEIGHT: f32 = 92.0;
const PICTURE: egui::Vec2 = vec2(136.0, 76.0);

/// What is dragged: the index of a card.
#[derive(Debug, Clone, Copy)]
struct CardDrag(usize);

/// What a card asks for.
enum CardAction {
    Select(String),
    Toggle(String, bool),
    Move(String, usize),
    Remove(String),
}

/// What a card shows.
struct Card {
    id: String,
    name: String,
    version: String,
    author: String,
    summary: String,
    enabled: bool,
    damaged: bool,
    state: ModState,
}

/// The key of the picture of mod `m` in the cache: an update of the mod
/// (the same id, installed again) has a new key, so its picture is read
/// again instead of the old one being shown.
pub(crate) fn preview_key(m: &InstalledMod) -> String {
    format!("{}\n{}\n{}", m.id, m.installed, m.manifest.version)
}

/// The texture of the picture of mod `m`, read once per installation.
pub(crate) fn preview(
    previews: &mut HashMap<String, Option<egui::TextureHandle>>,
    ctx: &egui::Context,
    m: &InstalledMod,
) -> Option<egui::TextureHandle> {
    previews
        .entry(preview_key(m))
        .or_insert_with(|| {
            let data = m.preview()?;
            let (w, h, rgba) = doctor_toolkit::resource::pixels::decode_png(&data).ok()?;
            let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
            Some(ctx.load_texture(
                format!("mod-preview-{}", m.id),
                image,
                egui::TextureOptions::LINEAR,
            ))
        })
        .clone()
}

/// The first paragraph of a description, on one line.
pub(crate) fn summary(description: &str) -> String {
    description
        .split("\n\n")
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where a card moved from `from` to the gap `slot` (0: before the first
/// card) ends up; `None` when it stays.
pub(crate) fn target(from: usize, slot: usize) -> Option<usize> {
    let to = if slot > from { slot - 1 } else { slot };
    (to != from).then_some(to)
}

impl ModsPanel {
    pub(crate) fn cards(&mut self, ui: &mut egui::Ui) {
        let Some(manager) = &self.manager else { return };
        if manager.mods().is_empty() {
            self.empty_list(ui);
            return;
        }
        let cards: Vec<Card> = manager
            .mods()
            .iter()
            .map(|m| Card {
                id: m.id.clone(),
                name: m.manifest.name.clone(),
                version: m.manifest.version.clone(),
                author: m.manifest.author.clone(),
                summary: summary(&m.manifest.description),
                enabled: m.enabled,
                damaged: m.problem.is_some(),
                state: ModState::of(m, manager, &self.overview),
            })
            .collect();
        let pictures: Vec<Option<egui::TextureHandle>> = manager
            .mods()
            .iter()
            .map(|m| preview(&mut self.previews, ui.ctx(), m))
            .collect();
        let idle = self.running.is_none();
        let count = cards.len();
        let mut action = None;
        let mut rects: Vec<Rect> = Vec::with_capacity(count);
        let ctx = ui.ctx().clone();
        let dragging = egui::DragAndDrop::payload::<CardDrag>(&ctx).map(|d| d.0);
        ui.spacing_mut().item_spacing.y = 6.0;
        for (i, card) in cards.iter().enumerate() {
            let width = ui.available_width();
            let (rect, response) =
                ui.allocate_exact_size(vec2(width, CARD_HEIGHT), Sense::click_and_drag());
            rects.push(rect);
            if idle {
                response.dnd_set_drag_payload(CardDrag(i));
            }
            let selected = self.selected.as_deref() == Some(card.id.as_str());
            let visuals = ui.visuals();
            let fill = if response.hovered() || selected {
                visuals.widgets.hovered.weak_bg_fill
            } else {
                visuals.faint_bg_color
            };
            let stroke = if selected {
                Stroke::new(1.5, theme::FOCUS)
            } else {
                visuals.widgets.noninteractive.bg_stroke
            };
            let alpha = if dragging == Some(i) { 0.45 } else { 1.0 };
            ui.painter().rect(
                rect,
                CornerRadius::same(6),
                fill.gamma_multiply(alpha),
                stroke,
                StrokeKind::Inside,
            );
            let inner = rect.shrink2(vec2(8.0, 8.0));
            // The handle.
            let handle = Rect::from_min_size(inner.min, vec2(14.0, inner.height()));
            widgets::paint_handle(
                ui,
                handle,
                response.hovered() && ui.rect_contains_pointer(handle),
            );
            if response.hovered() && ui.rect_contains_pointer(handle) && idle {
                ctx.set_cursor_icon(egui::CursorIcon::Grab);
            }
            // The picture.
            let picture = Rect::from_min_size(
                egui::pos2(handle.right() + 6.0, inner.center().y - PICTURE.y / 2.0),
                PICTURE,
            );
            match &pictures[i] {
                Some(texture) => widgets::paint_picture(ui, picture, texture),
                None => widgets::paint_placeholder(ui, picture, &card.name),
            }
            if !card.enabled {
                ui.painter().rect_filled(
                    picture,
                    CornerRadius::same(4),
                    Color32::from_black_alpha(110),
                );
            }
            // Text, state and switch.
            let rest =
                Rect::from_min_max(egui::pos2(picture.right() + 12.0, inner.top()), inner.max);
            ui.scope_builder(
                UiBuilder::new()
                    .max_rect(rest)
                    .layout(Layout::right_to_left(Align::Center)),
                |ui| {
                    let mut on = card.enabled;
                    let switch = widgets::switch(ui, &mut on, idle && !card.damaged).on_hover_text(
                        if card.enabled {
                            tr("mods.switch_off")
                        } else {
                            tr("mods.switch_on")
                        },
                    );
                    if switch.changed() {
                        action = Some(CardAction::Toggle(card.id.clone(), on));
                    }
                    ui.add_space(10.0);
                    let color = card.state.color();
                    widgets::chip(ui, &card.state.text(), color).on_hover_text(card.state.hint());
                    if matches!(
                        card.state,
                        ModState::Conflict(_)
                            | ModState::Clash(_)
                            | ModState::Misfit(_)
                            | ModState::Damaged(_)
                    ) {
                        widgets::warning_icon(ui, color).on_hover_text(card.state.hint());
                    }
                    ui.add_space(12.0);
                    ui.with_layout(Layout::top_down(Align::Min), |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            let mut name = RichText::new(&card.name).size(16.0).strong();
                            if !card.enabled {
                                name = name.weak();
                            }
                            ui.add(egui::Label::new(name).truncate().selectable(false));
                            ui.add(
                                egui::Label::new(RichText::new(&card.version).weak())
                                    .selectable(false),
                            );
                        });
                        if !card.author.is_empty() {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(trf("mods.by", &[("author", &card.author)]))
                                        .weak(),
                                )
                                .truncate()
                                .selectable(false),
                            );
                        }
                        if !card.summary.is_empty() {
                            ui.add(egui::Label::new(&card.summary).truncate().selectable(false));
                        }
                    });
                },
            );
            if response.clicked() {
                action = Some(CardAction::Select(card.id.clone()));
            }
            if response.dragged() {
                ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                if let Some(pos) = ctx.pointer_interact_pos() {
                    egui::Area::new(egui::Id::unique("mod-card-drag"))
                        .order(egui::Order::Tooltip)
                        .fixed_pos(pos + vec2(12.0, -10.0))
                        .interactable(false)
                        .show(&ctx, |ui| {
                            egui::Frame::popup(ui.style()).show(ui, |ui| {
                                ui.strong(&card.name);
                            });
                        });
                }
            }
            response.context_menu(|ui| {
                ui.add_enabled_ui(idle, |ui| {
                    let moves = [
                        (tr("mods.move_up"), i > 0, i.saturating_sub(1)),
                        (tr("mods.move_down"), i + 1 < count, i + 1),
                        (tr("mods.move_first"), i > 0, 0),
                        (tr("mods.move_last"), i + 1 < count, count - 1),
                    ];
                    for (label, can, to) in moves {
                        if ui.add_enabled(can, egui::Button::new(label)).clicked() {
                            action = Some(CardAction::Move(card.id.clone(), to));
                            ui.close();
                        }
                    }
                    ui.separator();
                    let label = if card.enabled {
                        tr("mods.switch_off")
                    } else {
                        tr("mods.switch_on")
                    };
                    if ui
                        .add_enabled(!card.damaged, egui::Button::new(label))
                        .clicked()
                    {
                        action = Some(CardAction::Toggle(card.id.clone(), !card.enabled));
                        ui.close();
                    }
                    if ui.button(tr("mods.remove")).clicked() {
                        action = Some(CardAction::Remove(card.id.clone()));
                        ui.close();
                    }
                });
            });
        }
        // Dropping a dragged card between others.
        if let (Some(from), Some(pos)) = (dragging, ctx.pointer_interact_pos())
            && let (Some(first), Some(last)) = (rects.first(), rects.last())
        {
            let area = Rect::from_min_max(first.min, last.max).expand2(vec2(0.0, 24.0));
            if area.contains(pos) {
                let slot = rects.iter().filter(|r| r.center().y < pos.y).count();
                let y = if slot == 0 {
                    first.top() - 3.0
                } else if slot == count {
                    last.bottom() + 3.0
                } else {
                    (rects[slot - 1].bottom() + rects[slot].top()) / 2.0
                };
                if target(from, slot).is_some() {
                    ui.painter()
                        .hline(first.x_range(), y, Stroke::new(3.0, theme::FOCUS));
                }
                if ctx.input(|i| i.pointer.any_released())
                    && egui::DragAndDrop::take_payload::<CardDrag>(&ctx).is_some()
                    && let Some(to) = target(from, slot)
                {
                    action = Some(CardAction::Move(cards[from].id.clone(), to));
                }
            }
        }
        ui.add_space(2.0);
        ui.add(egui::Label::new(RichText::new(tr("mods.order_hint")).weak().small()).wrap());
        match action {
            Some(CardAction::Select(id)) => self.selected = Some(id),
            Some(CardAction::Toggle(id, on)) => self.set_enabled(&ctx, &id, on),
            Some(CardAction::Move(id, to)) => {
                self.selected = Some(id.clone());
                self.move_mod(&ctx, &id, to);
            }
            Some(CardAction::Remove(id)) => self.ask_remove(&id),
            None => {}
        }
    }

    /// No mods yet: where to get them.
    fn empty_list(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(width, 150.0), Sense::hover());
        let stroke = Stroke::new(1.5, ui.visuals().widgets.inactive.bg_stroke.color);
        ui.painter().rect(
            rect,
            CornerRadius::same(8),
            ui.visuals().faint_bg_color,
            stroke,
            StrokeKind::Inside,
        );
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(rect.shrink(16.0))
                .layout(Layout::top_down(Align::Center)),
            |ui| {
                ui.add_space(22.0);
                ui.label(RichText::new(tr("mods.empty_title")).size(18.0).strong());
                ui.add_space(6.0);
                let hint = if self.standalone {
                    tr("mods.empty_hint_drop")
                } else {
                    tr("mods.empty_hint")
                };
                ui.label(RichText::new(hint).weak());
            },
        );
    }
}
