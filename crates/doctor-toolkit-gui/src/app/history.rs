//! The History panel (stage 54), as Undo History in Unreal Engine: every
//! operation of the workspace, oldest first, with the current place marked;
//! a click on an operation undoes the ones after it or redoes the undone
//! ones up to it. A panel like the others: docked, a drawer, closed.

use doctor_toolkit::workspace::Workspace;
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{RichText, Sense};

use super::BrowserApp;
use super::theme;

/// Height of a row of the list.
const ROW: f32 = 20.0;

/// One row: the start (before every operation) or an operation.
struct Row {
    /// Operations in effect after this row (0: the start).
    head: usize,
    label: String,
    /// Resources the operation changed.
    resources: Option<usize>,
}

fn rows(ws: &Workspace) -> Vec<Row> {
    let mut rows = vec![Row {
        head: 0,
        label: tr("history.start").to_owned(),
        resources: None,
    }];
    rows.extend(ws.operations().iter().enumerate().map(|(i, op)| Row {
        head: i + 1,
        label: op.label.clone(),
        resources: Some(op.edits.len() + op.stream_edits.len() + op.file_edits.len()),
    }));
    rows
}

impl BrowserApp {
    /// The History panel.
    pub(super) fn history_pane(&mut self, ui: &mut egui::Ui) {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        let Some(ws) = self.workspace.as_ref() else {
            ui.add_space(8.0);
            ui.vertical_centered(|ui| ui.label(RichText::new(tr("history.no_workspace")).weak()));
            return;
        };
        let head = ws.head();
        let rows = rows(ws);
        ui.label(RichText::new(tr("history.hint")).weak().small());
        if rows.len() == 1 {
            ui.label(RichText::new(tr("history.empty")).weak());
        }
        let mut target = None;
        egui::ScrollArea::vertical()
            .id_salt("history")
            .auto_shrink(false)
            .stick_to_bottom(true)
            .show_rows(ui, ROW, rows.len(), |ui, range| {
                for row in &rows[range] {
                    let current = row.head == head;
                    let undone = row.head > head;
                    let (rect, response) = ui
                        .allocate_exact_size(egui::vec2(ui.available_width(), ROW), Sense::click());
                    let visuals = ui.visuals();
                    if current {
                        ui.painter()
                            .rect_filled(rect, 2.0, theme::ACCENT.gamma_multiply(0.35));
                    } else if response.hovered() {
                        ui.painter()
                            .rect_filled(rect, 2.0, visuals.widgets.hovered.weak_bg_fill);
                    }
                    let color = if undone {
                        visuals.weak_text_color()
                    } else {
                        visuals.text_color()
                    };
                    let mut text = RichText::new(&row.label).color(color);
                    if undone {
                        text = text.italics();
                    }
                    if current {
                        text = text.strong();
                    }
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(rect.shrink2(egui::vec2(6.0, 0.0)))
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    );
                    child.add(egui::Label::new(text).selectable(false));
                    if let Some(n) = row.resources {
                        child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(trn("count.resources", n as u64, &[]))
                                    .weak()
                                    .small(),
                            );
                        });
                    }
                    let hint = if undone {
                        tr("history.undone_hint")
                    } else {
                        tr("history.done_hint")
                    };
                    if response.on_hover_text(hint).clicked() && !current {
                        target = Some(row.head);
                    }
                }
            });
        if let Some(target) = target {
            self.go_to_history(target);
        }
    }

    /// Undoes or redoes operations until `head` of them are in effect.
    pub(super) fn go_to_history(&mut self, head: usize) {
        let Some(ws) = self.workspace.as_mut() else {
            return;
        };
        let mut steps = 0u64;
        let mut error = None;
        while ws.head() != head {
            let step = if ws.head() > head {
                ws.undo()
            } else {
                ws.redo()
            };
            match step {
                Ok(Some(_)) => steps += 1,
                Ok(None) => break,
                Err(e) => {
                    error = Some(e);
                    break;
                }
            }
        }
        self.status = match error {
            Some(e) => trf("history.cannot", &[("error", &e)]),
            None => trn("history.moved", steps, &[]),
        };
        if steps > 0 {
            self.after_edit(None);
        }
    }
}
