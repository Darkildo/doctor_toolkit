//! The Changes panel: workspace changes, their state in the game, diffs and history.

use doctor_toolkit::workspace::{
    ChangeDetail, ChangeState, FileKey, ResourceKey, SemanticDiff, Status, StreamState, Workspace,
};
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{Color32, RichText, Sense};
use egui_extras::{Column, TableBuilder};

const ROW_HEIGHT: f32 = 18.0;
pub const PENDING: Color32 = Color32::from_rgb(0xe0, 0xa0, 0x30);
pub const APPLIED: Color32 = Color32::from_rgb(0x60, 0xc0, 0x60);
pub const CONFLICT: Color32 = Color32::from_rgb(0xe0, 0x50, 0x50);

/// What the user asked the changes window to do.
pub enum ChangesAction {
    Undo,
    Redo,
    /// Write the changes into the game; `true` overwrites foreign changes.
    Apply(bool),
    /// Put the original files back into the game.
    Unapply,
    /// Remove these changes from the workspace.
    Revert(Vec<ResourceKey>),
    /// Remove these file changes (streamed sounds, movies) from the workspace.
    RevertFiles(Vec<FileKey>),
    /// Show the resource in its stream.
    Open(ResourceKey),
}

#[derive(Default)]
pub struct ChangesWindow {
    selected: Option<ResourceKey>,
    detail: Option<(ResourceKey, Result<ChangeDetail, String>)>,
    /// A button that needs a second click was clicked once.
    confirm: Option<&'static str>,
}

impl ChangesWindow {
    /// Forgets the cached diff, e.g. after the workspace changed.
    pub fn invalidate(&mut self) {
        self.detail = None;
    }

    /// The changes as a panel of the main window.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        ws: Option<&Workspace>,
        status: Option<&Status>,
    ) -> Option<ChangesAction> {
        let Some(ws) = ws else {
            ui.label(RichText::new(tr("changes.empty")).weak());
            return None;
        };
        self.contents(ui, ws, status)
    }

    fn contents(
        &mut self,
        ui: &mut egui::Ui,
        ws: &Workspace,
        status: Option<&Status>,
    ) -> Option<ChangesAction> {
        let mut action = None;
        ui.label(RichText::new(ws.dir().display().to_string()).weak());
        if let Some(status) = status {
            ui.horizontal_wrapped(|ui| {
                ui.label(trf(
                    "changes.total",
                    &[(
                        "changes",
                        &trn(
                            "count.changes",
                            (status.changes.len() + status.files.len()) as u64,
                            &[],
                        ),
                    )],
                ));
                ui.colored_label(
                    PENDING,
                    trf(
                        "changes.pending",
                        &[("count", &status.count(ChangeState::Pending))],
                    ),
                );
                ui.colored_label(
                    APPLIED,
                    trf(
                        "changes.applied",
                        &[("count", &status.count(ChangeState::Applied))],
                    ),
                );
                let conflicts = status.count(ChangeState::Conflict);
                if conflicts > 0 {
                    ui.colored_label(
                        CONFLICT,
                        trf("changes.in_conflict", &[("count", &conflicts)]),
                    );
                }
                if !status.pending_reverts.is_empty() {
                    ui.colored_label(
                        PENDING,
                        trf(
                            "changes.to_restore",
                            &[("count", &status.pending_reverts.len())],
                        ),
                    );
                }
                if !status.fingerprint_matches {
                    ui.colored_label(CONFLICT, tr("changes.other_build"));
                }
            });
        }
        if ws.file_changes().next().is_some() {
            ui.label(RichText::new(tr("changes.files_outside")).strong());
            for change in ws.file_changes() {
                ui.horizontal(|ui| {
                    let state = status.and_then(|s| {
                        s.files
                            .iter()
                            .find(|(k, _)| *k == change.key)
                            .map(|(_, st)| *st)
                    });
                    let (text, color) = match state {
                        Some(ChangeState::Applied) => (tr("changes.state_applied"), APPLIED),
                        Some(ChangeState::Conflict) => (tr("changes.state_conflict"), CONFLICT),
                        _ => (tr("changes.state_pending"), PENDING),
                    };
                    ui.colored_label(color, text);
                    if ui.small_button(tr("changes.revert")).clicked() {
                        action = Some(ChangesAction::RevertFiles(vec![change.key.clone()]));
                    }
                    ui.monospace(change.key.to_string());
                    ui.label(
                        RichText::new(trf(
                            "changes.file_sizes",
                            &[
                                ("old", &change.original.size),
                                ("new", &change.payload.size),
                                ("label", &change.label),
                            ],
                        ))
                        .weak(),
                    );
                });
            }
        }
        let up_to_date = status.is_none_or(Status::is_up_to_date);
        let conflicts = status.map_or(0, |s| s.conflicts().len());
        ui.horizontal_wrapped(|ui| {
            let undo = ws
                .undo_label()
                .map(|l| trf("changes.undo_label", &[("label", &l)]));
            if ui
                .add_enabled(undo.is_some(), egui::Button::new(tr("cmd.undo")))
                .on_hover_text(
                    undo.unwrap_or_else(|| tr("edits.nothing_to_undo").into()) + " (Ctrl+Z)",
                )
                .clicked()
            {
                action = Some(ChangesAction::Undo);
            }
            let redo = ws
                .redo_label()
                .map(|l| trf("changes.redo_label", &[("label", &l)]));
            if ui
                .add_enabled(redo.is_some(), egui::Button::new(tr("cmd.redo")))
                .on_hover_text(
                    redo.unwrap_or_else(|| tr("edits.nothing_to_redo").into()) + " (Ctrl+Shift+Z)",
                )
                .clicked()
            {
                action = Some(ChangesAction::Redo);
            }
            ui.separator();
            if conflicts > 0 {
                if self
                    .confirm_button(ui, "force", tr("changes.apply_force"))
                    .on_hover_text(tr("changes.apply_force_hint"))
                    .clicked_twice
                {
                    action = Some(ChangesAction::Apply(true));
                }
            } else if ui
                .add_enabled(!up_to_date, egui::Button::new(tr("changes.apply")))
                .on_hover_text(tr("changes.apply_hint"))
                .clicked()
            {
                action = Some(ChangesAction::Apply(false));
            }
            let in_game =
                status.is_some_and(|s| s.streams.iter().any(|st| st.state == StreamState::Applied));
            if ui
                .add_enabled(in_game, egui::Button::new(tr("changes.unapply")))
                .on_hover_text(tr("changes.unapply_hint"))
                .clicked()
            {
                action = Some(ChangesAction::Unapply);
            }
            ui.separator();
            if self
                .confirm_button(ui, "revert-all", tr("changes.discard_all"))
                .on_hover_text(tr("changes.discard_all_hint"))
                .clicked_twice
            {
                action = Some(ChangesAction::Revert(
                    ws.changes().map(|c| c.key.clone()).collect(),
                ));
            }
        });
        ui.separator();

        let changes: Vec<_> = ws.changes().collect();
        let mut clicked = None;
        let mut open_key = None;
        let height = (ui.available_height() * 0.45).max(120.0);
        TableBuilder::new(ui)
            .id_salt("changes")
            .striped(true)
            .sense(Sense::click())
            .max_scroll_height(height)
            .column(Column::exact(70.0))
            .column(Column::initial(200.0).resizable(true).clip(true))
            .column(Column::exact(60.0))
            .column(Column::exact(80.0))
            .column(Column::exact(130.0))
            .column(Column::remainder().clip(true))
            .header(ROW_HEIGHT + 2.0, |mut header| {
                for title in [
                    tr("changes.col_state"),
                    tr("browse.col_stream"),
                    tr("browse.col_type"),
                    tr("browse.col_id"),
                    tr("browse.col_size"),
                    tr("changes.col_operation"),
                ] {
                    header.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                body.rows(ROW_HEIGHT, changes.len(), |mut row| {
                    let change = changes[row.index()];
                    let key = &change.key;
                    row.set_selected(self.selected.as_ref() == Some(key));
                    let state = status.and_then(|s| s.state_of(key));
                    row.col(|ui| {
                        let (text, color) = match state {
                            Some(ChangeState::Applied) => (tr("changes.state_applied"), APPLIED),
                            Some(ChangeState::Conflict) => (tr("changes.state_conflict"), CONFLICT),
                            Some(ChangeState::Pending) => (tr("changes.state_pending"), PENDING),
                            None => ("?", Color32::GRAY),
                        };
                        ui.add(
                            egui::Label::new(RichText::new(text).color(color)).selectable(false),
                        );
                    });
                    let cells = [
                        key.stream.clone(),
                        key.type_name(),
                        format!("{:08x}", key.name_id()),
                        format!("{} -> {}", change.original.size, change.payload.size),
                        change.label.clone(),
                    ];
                    for cell in cells {
                        row.col(|ui| {
                            ui.add(egui::Label::new(cell).truncate().selectable(false));
                        });
                    }
                    let response = row.response();
                    if response.clicked() {
                        clicked = Some(key.clone());
                    }
                    if response.double_clicked() {
                        open_key = Some(key.clone());
                    }
                });
            });
        if let Some(key) = clicked {
            self.selected = Some(key);
        }
        if let Some(key) = open_key {
            action = Some(ChangesAction::Open(key));
        }
        if let Some(status) = status
            && !status.pending_reverts.is_empty()
        {
            ui.label(
                RichText::new(trf(
                    "changes.removed_pending",
                    &[(
                        "keys",
                        &status
                            .pending_reverts
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", "),
                    )],
                ))
                .color(PENDING),
            );
        }
        ui.separator();

        if let Some(key) = self.selected.clone() {
            if ws.change(&key).is_none() {
                self.selected = None;
            } else {
                if let Some(a) = self.detail_view(ui, ws, &key) {
                    action = Some(a);
                }
                ui.separator();
            }
        }
        egui::CollapsingHeader::new(trf("changes.history", &[("count", &ws.operations().len())]))
            .id_salt("changes-history")
            .default_open(false)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("history")
                    .max_height(160.0)
                    .show(ui, |ui| {
                        for (i, op) in ws.operations().iter().enumerate().rev() {
                            let text = format!(
                                "{:>4}  {}  ({})",
                                op.seq,
                                op.label,
                                trn("count.resources", op.edits.len() as u64, &[])
                            );
                            if i < ws.head() {
                                ui.monospace(text);
                            } else {
                                ui.label(
                                    RichText::new(text + tr("changes.undone"))
                                        .monospace()
                                        .weak(),
                                );
                            }
                        }
                    });
            });
        action
    }

    fn detail_view(
        &mut self,
        ui: &mut egui::Ui,
        ws: &Workspace,
        key: &ResourceKey,
    ) -> Option<ChangesAction> {
        let mut action = None;
        if self.detail.as_ref().is_none_or(|(k, _)| k != key) {
            let detail = ws
                .detail(key)
                .map_err(|e| e.to_string())
                .and_then(|d| d.ok_or_else(|| tr("changes.not_changed").to_owned()));
            self.detail = Some((key.clone(), detail));
        }
        ui.horizontal_wrapped(|ui| {
            ui.monospace(key.to_string());
            if ui.button(tr("changes.open")).clicked() {
                action = Some(ChangesAction::Open(key.clone()));
            }
            if ui
                .button(tr("changes.revert_this"))
                .on_hover_text(tr("changes.revert_this_hint"))
                .clicked()
            {
                action = Some(ChangesAction::Revert(vec![key.clone()]));
            }
        });
        let Some((_, detail)) = &self.detail else {
            return action;
        };
        let detail = match detail {
            Ok(detail) => detail,
            Err(error) => {
                ui.colored_label(CONFLICT, error);
                return action;
            }
        };
        let bytes = &detail.bytes;
        ui.label(trf(
            "changes.bytes_diff",
            &[
                ("old", &bytes.old_len),
                ("new", &bytes.new_len),
                (
                    "changed",
                    &trn("count.bytes", bytes.changed_bytes as u64, &[]),
                ),
                (
                    "ranges",
                    &format!(
                        "{}{}",
                        bytes.ranges.len(),
                        if bytes.truncated { "+" } else { "" }
                    ),
                ),
            ],
        ));
        match &detail.semantic {
            Some(SemanticDiff::Texture(texture)) => {
                ui.label(trf(
                    "changes.texture",
                    &[
                        ("name", &texture.name),
                        ("old", &texture.old),
                        ("new", &texture.new),
                    ],
                ));
            }
            Some(SemanticDiff::Strings(strings)) => {
                ui.label(trf(
                    "changes.strings",
                    &[
                        (
                            "changed",
                            &trn("count.strings", strings.changed.len() as u64, &[]),
                        ),
                        ("added", &strings.added.len()),
                        ("removed", &strings.removed.len()),
                    ],
                ));
                egui::ScrollArea::vertical()
                    .id_salt("string-diff")
                    .max_height(180.0)
                    .show(ui, |ui| {
                        egui::Grid::new("string-diff-grid")
                            .striped(true)
                            .show(ui, |ui| {
                                for change in &strings.changed {
                                    ui.monospace(format!("{:08x}", change.hash));
                                    ui.label(RichText::new(&change.language_name).weak());
                                    ui.label(RichText::new(&change.old).color(CONFLICT));
                                    ui.label(RichText::new(&change.new).color(APPLIED));
                                    ui.end_row();
                                }
                            });
                    });
            }
            Some(SemanticDiff::Logic(logic)) => {
                ui.label(trf(
                    "changes.logic",
                    &[
                        ("values", &logic.values.len()),
                        ("objects", &logic.changed.len()),
                        (
                            "truncated",
                            &if logic.truncated {
                                tr("changes.truncated")
                            } else {
                                ""
                            },
                        ),
                    ],
                ));
                for line in &logic.other {
                    ui.colored_label(PENDING, line);
                }
                egui::ScrollArea::vertical()
                    .id_salt("logic-diff")
                    .max_height(180.0)
                    .show(ui, |ui| {
                        egui::Grid::new("logic-diff-grid")
                            .striped(true)
                            .show(ui, |ui| {
                                for change in &logic.values {
                                    ui.label(&change.place);
                                    ui.label(RichText::new(&change.old).color(CONFLICT));
                                    ui.label(RichText::new(&change.new).color(APPLIED));
                                    ui.end_row();
                                }
                            });
                    });
            }
            Some(SemanticDiff::Apx(apx)) => {
                let size = |s: Option<usize>| {
                    s.map_or_else(|| tr("changes.none").to_owned(), |s| s.to_string())
                };
                for block in &apx.blocks {
                    ui.label(trf(
                        "changes.apx_block",
                        &[
                            ("tag", &block.tag),
                            ("ordinal", &block.ordinal),
                            ("old", &size(block.old_size)),
                            ("new", &size(block.new_size)),
                        ],
                    ));
                }
                for texture in &apx.textures {
                    ui.label(trf(
                        "changes.apx_texture",
                        &[
                            ("name", &texture.name),
                            ("old", &texture.old),
                            ("new", &texture.new),
                        ],
                    ));
                }
            }
            None => {
                let ranges: Vec<String> = bytes
                    .ranges
                    .iter()
                    .take(8)
                    .map(|r| format!("{:#x}..{:#x}", r.start, r.end))
                    .collect();
                if !ranges.is_empty() {
                    ui.label(
                        RichText::new(trf("changes.ranges", &[("ranges", &ranges.join(", "))]))
                            .monospace(),
                    );
                }
            }
        }
        action
    }

    /// A button that acts on the second click.
    fn confirm_button(
        &mut self,
        ui: &mut egui::Ui,
        id: &'static str,
        label: &str,
    ) -> ConfirmResponse {
        let armed = self.confirm == Some(id);
        let text = if armed { tr("changes.confirm") } else { label };
        let response = ui.button(text);
        let mut clicked_twice = false;
        if response.clicked() {
            if armed {
                self.confirm = None;
                clicked_twice = true;
            } else {
                self.confirm = Some(id);
            }
        } else if armed && ui.input(|i| i.pointer.any_click()) && !response.hovered() {
            self.confirm = None;
        }
        ConfirmResponse {
            response,
            clicked_twice,
        }
    }
}

struct ConfirmResponse {
    response: egui::Response,
    clicked_twice: bool,
}

impl ConfirmResponse {
    fn on_hover_text(self, text: &str) -> Self {
        Self {
            response: self.response.on_hover_text(text),
            clicked_twice: self.clicked_twice,
        }
    }
}
