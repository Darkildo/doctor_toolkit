//! The windows of the panel: questions before installing, writing,
//! restoring and removing; the name of a set to save; the progress and the
//! result of writing and of the integrity check.

use std::path::PathBuf;

use egui::{RichText, vec2};

use crate::i18n::{tr, trf, trn};
use crate::widgets::{Prominence, action_button};
use crate::{BAD, GOOD, JobKind, ModsPanel, Report};

/// A window of the panel.
pub(crate) enum Dialog {
    /// Install these files (opened from the system)?
    Install(Vec<PathBuf>),
    /// Write the enabled mods into the game?
    Write,
    /// Put the originals back?
    Restore,
    /// Remove this mod?
    Remove { id: String, name: String },
    /// The name to save the set of mods under.
    SaveSet(String),
    /// Forget this set?
    DeleteSet(String),
    /// The result of writing, restoring or checking.
    Report(Report),
}

/// What the buttons of a window asked for.
enum Answer {
    Install(Vec<PathBuf>),
    Write,
    Restore,
    Remove(String),
    SaveSet(String),
    DeleteSet(String),
}

/// The heading and the text of a question.
fn question(ui: &mut egui::Ui, title: &str, text: &str) {
    ui.label(RichText::new(title).size(18.0).strong());
    ui.add_space(6.0);
    ui.add(egui::Label::new(text).wrap());
    ui.add_space(10.0);
}

/// Confirm and Cancel buttons, right-aligned; returns (confirmed, cancelled).
fn buttons(ui: &mut egui::Ui, confirm: &str, enabled: bool) -> (bool, bool) {
    let mut out = (false, false);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        out.0 = action_button(ui, enabled, confirm, Prominence::Main).clicked();
        out.1 = ui.button(tr("mods.cancel")).clicked();
    });
    out
}

impl ModsPanel {
    pub(crate) fn dialogs(&mut self, ctx: &egui::Context) {
        self.progress_window(ctx);
        let Some(mut dialog) = self.dialog.take() else {
            return;
        };
        let mut keep = true;
        let mut answer = None;
        let enabled = self
            .manager
            .as_ref()
            .map_or(0, |m| m.mods().iter().filter(|m| m.enabled).count());
        let busy = self.running.is_some();
        let lock = self.write_lock.clone();
        let response = crate::widgets::modal(ctx, "mods-dialog").show(ctx, |ui| {
            ui.set_width(440.0);
            match &mut dialog {
                Dialog::Install(paths) => {
                    let names: Vec<String> = paths
                        .iter()
                        .map(|p| {
                            p.file_name().map_or_else(
                                || p.display().to_string(),
                                |n| n.to_string_lossy().into_owned(),
                            )
                        })
                        .collect();
                    question(
                        ui,
                        &trn("mods.ask_install_title", paths.len() as u64, &[]),
                        tr("mods.ask_install_text"),
                    );
                    for name in &names {
                        ui.label(RichText::new(format!("•  {name}")).strong());
                    }
                    ui.add_space(10.0);
                    let (yes, no) = buttons(ui, tr("mods.install_now"), !busy);
                    if yes {
                        answer = Some(Answer::Install(paths.clone()));
                    }
                    keep &= !(yes || no);
                }
                Dialog::Write => {
                    question(
                        ui,
                        tr("mods.ask_write_title"),
                        &format!(
                            "{}\n\n{}",
                            trn("mods.ask_write_text", enabled as u64, &[]),
                            tr("mods.ask_write_note")
                        ),
                    );
                    if let Some(reason) = &lock {
                        ui.colored_label(BAD, reason);
                    }
                    let (yes, no) = buttons(ui, tr("mods.write_now"), !busy && lock.is_none());
                    if yes {
                        answer = Some(Answer::Write);
                    }
                    keep &= !(yes || no);
                }
                Dialog::Restore => {
                    question(
                        ui,
                        tr("mods.ask_restore_title"),
                        tr("mods.ask_restore_text"),
                    );
                    if let Some(reason) = &lock {
                        ui.colored_label(BAD, reason);
                    }
                    let (yes, no) = buttons(ui, tr("mods.restore_now"), !busy && lock.is_none());
                    if yes {
                        answer = Some(Answer::Restore);
                    }
                    keep &= !(yes || no);
                }
                Dialog::Remove { id, name } => {
                    question(
                        ui,
                        &trf("mods.ask_remove_title", &[("name", name)]),
                        tr("mods.ask_remove_text"),
                    );
                    let (yes, no) = buttons(ui, tr("mods.remove_now"), !busy);
                    if yes {
                        answer = Some(Answer::Remove(id.clone()));
                    }
                    keep &= !(yes || no);
                }
                Dialog::SaveSet(name) => {
                    question(ui, tr("mods.ask_set_title"), tr("mods.ask_set_text"));
                    let edit = ui.add(
                        egui::TextEdit::singleline(name)
                            .hint_text(tr("mods.set_name_hint"))
                            .desired_width(f32::INFINITY),
                    );
                    if !edit.has_focus() && name.is_empty() {
                        edit.request_focus();
                    }
                    let entered =
                        edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.add_space(10.0);
                    let valid = !name.trim().is_empty() && name.trim().chars().count() <= 64;
                    let (yes, no) = buttons(ui, tr("mods.save_now"), valid && !busy);
                    if (yes || entered) && valid && !busy {
                        answer = Some(Answer::SaveSet(name.trim().to_owned()));
                        keep = false;
                    }
                    keep &= !(yes || no);
                }
                Dialog::DeleteSet(name) => {
                    question(
                        ui,
                        &trf("mods.ask_delete_set_title", &[("name", name)]),
                        tr("mods.ask_delete_set_text"),
                    );
                    let (yes, no) = buttons(ui, tr("mods.delete_now"), !busy);
                    if yes {
                        answer = Some(Answer::DeleteSet(name.clone()));
                    }
                    keep &= !(yes || no);
                }
                Dialog::Report(report) => {
                    let color = if report.error { BAD } else { GOOD };
                    ui.label(
                        RichText::new(&report.title)
                            .size(18.0)
                            .strong()
                            .color(color),
                    );
                    ui.add_space(6.0);
                    ui.add(egui::Label::new(&report.text).wrap());
                    ui.add_space(10.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(egui::Button::new(tr("mods.ok")).min_size(vec2(80.0, 28.0)))
                            .clicked()
                        {
                            keep = false;
                        }
                    });
                }
            }
        });
        if response.should_close() {
            keep = false;
        }
        if keep {
            self.dialog = Some(dialog);
        }
        match answer {
            Some(Answer::Install(paths)) => self.install_files(ctx, paths),
            Some(Answer::Write) => self.apply(ctx),
            Some(Answer::Restore) => self.unapply(ctx),
            Some(Answer::Remove(id)) => self.remove(ctx, &id),
            Some(Answer::SaveSet(name)) => self.save_set(ctx, &name),
            Some(Answer::DeleteSet(name)) => self.delete_set(ctx, &name),
            None => {}
        }
    }

    /// The progress of writing into the game.
    fn progress_window(&self, ctx: &egui::Context) {
        let Some(running) = &self.running else { return };
        if running.kind != JobKind::Writing {
            return;
        }
        let progress = running
            .progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        crate::widgets::modal(ctx, "mods-progress").show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new(&running.label).size(18.0).strong());
            ui.add_space(8.0);
            match progress {
                Some((done, total, stream)) if total > 0 => {
                    ui.add(
                        egui::ProgressBar::new(done as f32 / total as f32)
                            .text(trf("mods.progress", &[("done", &done), ("total", &total)]))
                            .animate(true),
                    );
                    if !stream.is_empty() {
                        ui.label(RichText::new(stream).weak().small());
                    }
                }
                _ => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(tr("mods.progress_checking"));
                    });
                }
            }
            ui.add_space(4.0);
            ui.label(RichText::new(tr("mods.progress_note")).weak());
        });
    }
}
