//! Drawers (stage 52): panels that are not tiles of the dock but slide up
//! over the bottom of the window when their button in the status bar is
//! pressed, and hide again on a second press, Esc or a click elsewhere —
//! the Content Drawer of Unreal Engine (Ctrl+Space). A panel's tab menu
//! makes it a drawer; the drawer's "Dock in layout" puts it back.

use egui::{Color32, Rect, RichText, Sense, Stroke, pos2, vec2};

use doctor_toolkit_mods::i18n::tr;

use super::BrowserApp;
use super::dock::{self, DockOp, Pane};
use super::theme::{self, Icon, Tone};

/// Height of a drawer in parts of the dock's, at most and at least.
const HEIGHT: (f32, f32) = (0.2, 0.85);

/// The drawers and the one that is open.
pub struct Drawers {
    pub panes: Vec<Pane>,
    pub open: Option<Pane>,
    /// Height in parts of the dock's.
    pub height: f32,
    /// The buttons of the status bar this frame: a click there is not a
    /// click elsewhere.
    pub buttons: Vec<Rect>,
}

impl Default for Drawers {
    fn default() -> Self {
        Self {
            panes: Vec::new(),
            open: None,
            height: 0.45,
            buttons: Vec::new(),
        }
    }
}

impl Drawers {
    /// The drawers of a layout; the open one closes if it is not among them.
    pub fn set(&mut self, panes: Vec<Pane>) {
        if self.open.as_ref().is_some_and(|p| !panes.contains(p)) {
            self.open = None;
        }
        self.panes = panes;
    }

    pub fn contains(&self, pane: &Pane) -> bool {
        self.panes.contains(pane)
    }

    pub fn is_open(&self, pane: &Pane) -> bool {
        self.open.as_ref() == Some(pane)
    }

    pub fn toggle(&mut self, pane: &Pane) {
        self.open = if self.is_open(pane) {
            None
        } else {
            Some(pane.clone())
        };
    }

    pub fn add(&mut self, pane: Pane) {
        if !self.contains(&pane) {
            self.panes.push(pane);
        }
    }

    pub fn remove(&mut self, pane: &Pane) {
        self.panes.retain(|p| p != pane);
        if self.is_open(pane) {
            self.open = None;
        }
    }
}

impl BrowserApp {
    /// Ctrl+Space: the Content Browser — its drawer, its tile, or a new
    /// drawer.
    pub(super) fn toggle_content(&mut self) {
        let pane = Pane::ContentBrowser;
        let docked = self
            .dock
            .as_ref()
            .is_some_and(|tree| dock::find(tree, |p| *p == pane).is_some());
        if self.drawers.contains(&pane) {
            self.drawers.toggle(&pane);
        } else if docked {
            self.dock_ops.push(DockOp::TogglePanel(pane));
        } else {
            self.drawers.add(pane.clone());
            self.drawers.open = Some(pane);
        }
    }

    /// The buttons of the drawers in the status bar.
    pub(super) fn drawer_buttons(&mut self, ui: &mut egui::Ui) {
        self.drawers.buttons.clear();
        let mut panes = self.drawers.panes.clone();
        // The Content Drawer comes first even when it is docked.
        if !panes.contains(&Pane::ContentBrowser) {
            panes.insert(0, Pane::ContentBrowser);
        }
        if !panes.contains(&Pane::Log) {
            panes.push(Pane::Log);
        }
        for pane in panes {
            let (icon, text) = match &pane {
                Pane::ContentBrowser => (Icon::Folder, tr("drawer.content").to_owned()),
                Pane::Log => (Icon::Log, self.notes.title_for(tr("drawer.log"))),
                other => (Icon::Folder, self.pane_title(other)),
            };
            let on = self.drawers.is_open(&pane);
            let response = theme::tool_button(ui, Some(icon), &text, on, Tone::Flat);
            let hint = match pane {
                Pane::ContentBrowser => format!(
                    "{} ({})",
                    tr("drawer.content_hint"),
                    super::Command::ContentDrawer
                        .shortcut_text(ui.ctx())
                        .unwrap_or_default()
                ),
                Pane::Log => tr("status.log_hint").to_owned(),
                _ => tr("drawer.hint").to_owned(),
            };
            let response = response.on_hover_text(hint);
            self.drawers.buttons.push(response.rect);
            if response.clicked() {
                match pane {
                    Pane::ContentBrowser => self.toggle_content(),
                    p if self.drawers.contains(&p) => self.drawers.toggle(&p),
                    p => self.dock_ops.push(DockOp::ShowPanel(p)),
                }
            }
        }
    }

    /// The open drawer, over the bottom of the dock (`dock` is the dock's
    /// rectangle). Esc or a click elsewhere closes it.
    pub(super) fn drawer_ui(&mut self, ctx: &egui::Context, dock: Rect) {
        let Some(mut pane) = self.drawers.open.clone() else {
            return;
        };
        // Over the documents: the panels at the sides stay visible.
        let documents = self.dock.as_ref().and_then(|tree| {
            let shown = dock::shown_documents(tree);
            let tile = self
                .active_doc
                .filter(|t| shown.contains(t))
                .or(shown.first().copied())?;
            let group = tree.tiles.parent_of(tile)?;
            tree.tiles.rect(group)
        });
        let x_range = documents
            .filter(|r| r.width() > 240.0 && self.maximized.is_none())
            .map_or(dock.x_range(), |r| r.x_range());
        let height = (dock.height() * self.drawers.height).max(120.0);
        let rect = Rect::from_x_y_ranges(x_range, (dock.bottom() - height)..=dock.bottom());

        // Closed by Esc (not while the palette has it) or a click elsewhere.
        let escape = !self.palette.open
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let elsewhere = ctx.input(|i| {
            i.pointer.any_pressed()
                && i.pointer.interact_pos().is_some_and(|p| {
                    !rect.contains(p) && !self.drawers.buttons.iter().any(|b| b.contains(p))
                })
        }) && !egui::Popup::is_any_open(ctx);
        // An asset dragged out of the drawer: it gets out of the way of
        // the view it is dropped on.
        let dragged_out = egui::DragAndDrop::has_payload_of_type::<super::dnd::DraggedAsset>(ctx)
            && ctx.input(|i| i.pointer.latest_pos().is_some_and(|p| !rect.contains(p)));
        if escape || elsewhere || dragged_out {
            self.drawers.open = None;
            return;
        }

        let title = self.pane_title(&pane);
        let mut close = false;
        let mut dock_it = false;
        egui::Area::new(egui::Id::unique("drawer"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.min)
            .constrain(false)
            .show(ctx, |ui| {
                let visuals = ui.visuals().clone();
                ui.painter().rect_filled(rect, 0.0, visuals.panel_fill);
                ui.painter().add(egui::Shape::rect_filled(
                    rect.translate(vec2(0.0, -6.0)).with_max_y(rect.top()),
                    0.0,
                    Color32::from_black_alpha(if visuals.dark_mode { 70 } else { 20 }),
                ));
                ui.painter().hline(
                    rect.x_range(),
                    rect.top() + 1.0,
                    Stroke::new(2.0, theme::ACCENT),
                );
                // The top edge resizes.
                let edge = Rect::from_x_y_ranges(rect.x_range(), rect.top()..=rect.top() + 5.0);
                let resize = ui
                    .interact(edge, egui::Id::unique("drawer-resize"), Sense::drag())
                    .on_hover_cursor(egui::CursorIcon::ResizeVertical);
                if resize.dragged() {
                    let delta = resize.drag_delta().y / dock.height().max(1.0);
                    self.drawers.height = (self.drawers.height - delta).clamp(HEIGHT.0, HEIGHT.1);
                }
                // Title bar.
                let bar =
                    Rect::from_x_y_ranges(rect.x_range(), rect.top() + 2.0..=rect.top() + 28.0);
                ui.painter()
                    .rect_filled(bar, 0.0, theme::tab_bar_fill(&visuals));
                let mut bar_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(bar.shrink2(vec2(8.0, 2.0)))
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                bar_ui.label(RichText::new(&title).strong());
                bar_ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button("×")
                        .on_hover_text(tr("drawer.close"))
                        .clicked()
                    {
                        close = true;
                    }
                    if ui
                        .small_button(tr("drawer.dock"))
                        .on_hover_text(tr("drawer.dock_hint"))
                        .clicked()
                    {
                        dock_it = true;
                    }
                });
                // The pane.
                let body =
                    Rect::from_min_max(pos2(rect.left(), bar.bottom()), rect.max).shrink(4.0);
                let mut body_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(body)
                        .id_salt(("drawer", format!("{pane:?}"))),
                );
                body_ui.set_clip_rect(body);
                self.pane_ui(&mut body_ui, &mut pane);
                // The drawer takes the pointer from what is under it.
                ui.allocate_rect(rect, Sense::hover());
            });
        if close {
            self.drawers.open = None;
        }
        if dock_it {
            self.drawers.open = None;
            self.dock_ops.push(DockOp::DockDrawer(pane));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drawers_open_one_at_a_time() {
        let mut drawers = Drawers::default();
        drawers.set(vec![Pane::ContentBrowser, Pane::Log]);
        drawers.toggle(&Pane::Log);
        assert!(drawers.is_open(&Pane::Log));
        drawers.toggle(&Pane::ContentBrowser);
        assert!(drawers.is_open(&Pane::ContentBrowser) && !drawers.is_open(&Pane::Log));
        drawers.toggle(&Pane::ContentBrowser);
        assert_eq!(drawers.open, None);
        drawers.toggle(&Pane::Log);
        drawers.remove(&Pane::Log);
        assert_eq!(drawers.open, None, "a drawer docked is closed");
        drawers.toggle(&Pane::ContentBrowser);
        drawers.set(vec![Pane::Changes]);
        assert_eq!(drawers.open, None, "a layout without it closes it");
        drawers.add(Pane::Changes);
        assert_eq!(drawers.panes, [Pane::Changes]);
    }
}
