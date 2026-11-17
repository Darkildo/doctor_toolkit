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
    /// Install these files? With what each of them holds, read and checked
    /// before the question (stage 103).
    Install(Vec<PathBuf>, Vec<FileSummary>),
    /// Write the enabled mods into the game?
    Write,
    /// Put the originals back?
    Restore,
    /// Remove this mod? `needed_by`: names of the enabled mods that need it.
    Remove {
        id: String,
        name: String,
        needed_by: String,
    },
    /// Turn on the mods (`names`) this one needs too?
    EnableNeeded {
        id: String,
        name: String,
        names: String,
    },
    /// Turn off this mod, which enabled mods (`names`) need?
    DisableNeeded {
        id: String,
        name: String,
        names: String,
    },
    /// The name to save the set of mods under.
    SaveSet(String),
    /// Forget this set?
    DeleteSet(String),
    /// The result of writing, restoring or checking.
    Report(Report),
    /// This mod carries executable code: may it be installed?
    Code(Box<crate::native::CodeView>),
}

/// What a mod file holds, or why it cannot be read.
pub(crate) type FileSummary = Result<doctor_toolkit::mods::FileInfo, String>;

/// What a mod file holds, under its name in the question before installing.
fn file_summary(ui: &mut egui::Ui, info: &FileSummary) {
    let info = match info {
        Ok(info) => info,
        Err(error) => {
            ui.colored_label(BAD, trf("mods.info_unreadable", &[("error", error)]));
            return;
        }
    };
    ui.indent(("mod-file-info", &info.name, &info.id), |ui| {
        let mut title = format!("{} {}", info.name, info.version);
        if !info.author.is_empty() {
            title += &trf("mods.info_by", &[("author", &info.author)]);
        }
        ui.label(title);
        if let Some(line) = info.description.lines().next() {
            ui.add(egui::Label::new(RichText::new(line).weak()).wrap());
        }
        let mut contents = trf(
            "mods.info_contents",
            &[
                (
                    "resources",
                    &trn("count.resources", info.resources as u64, &[]),
                ),
                ("added", &info.added),
                (
                    "streams",
                    &trn("count.streams", info.streams.len() as u64, &[]),
                ),
                ("bytes", &trn("count.bytes", info.payload_bytes, &[])),
            ],
        );
        if !info.new_streams.is_empty() {
            contents += "; ";
            contents += &trn("count.new_streams", info.new_streams.len() as u64, &[]);
        }
        ui.add(egui::Label::new(contents).wrap());
        if !info.streams.is_empty() {
            let mut names = info.streams.iter().take(6).cloned().collect::<Vec<_>>();
            if info.streams.len() > names.len() {
                names.push("…".to_owned());
            }
            ui.add(egui::Label::new(RichText::new(names.join(", ")).weak().small()).wrap());
        }
        if info.kind == doctor_toolkit::mods::FileKind::Vpat {
            ui.label(RichText::new(tr("mods.info_vpat")).weak());
        }
        if !info.depends.is_empty() {
            ui.label(trf(
                "mods.info_depends",
                &[("mods", &info.depends.join(", "))],
            ));
        }
        if !info.incompatible.is_empty() {
            ui.label(trf(
                "mods.info_incompatible",
                &[("mods", &info.incompatible.join(", "))],
            ));
        }
        if let Some(code) = &info.code {
            ui.colored_label(
                crate::WARN,
                trf("mods.info_code", &[("modules", &code.modules.join(", "))]),
            );
        }
        for warning in &info.warnings {
            ui.colored_label(crate::WARN, warning);
        }
        if info.verified {
            ui.colored_label(GOOD, tr("mods.info_verified"));
        }
    });
}

/// What the buttons of a window asked for.
enum Answer {
    Install(Vec<PathBuf>),
    Write,
    Restore,
    Remove(String),
    SaveSet(String),
    DeleteSet(String),
    /// Switch a mod on (with what it needs when `true`) or off.
    Switch(String, bool),
    EnableWith(String),
    /// The answer about the executable code of a mod.
    Code(String, crate::native::CodeAnswer),
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
                Dialog::Code(view) => {
                    if let Some(pressed) = crate::native::question_ui(ui, view, busy) {
                        if pressed != crate::native::CodeAnswer::Later {
                            answer = Some(Answer::Code(view.question.id.clone(), pressed));
                        }
                        keep = false;
                    }
                }
                Dialog::Install(paths, infos) => {
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
                    let readable = infos.iter().any(Result::is_ok);
                    egui::ScrollArea::vertical()
                        .max_height(360.0)
                        .show(ui, |ui| {
                            for (i, name) in names.iter().enumerate() {
                                ui.label(RichText::new(format!("•  {name}")).strong());
                                if let Some(info) = infos.get(i) {
                                    file_summary(ui, info);
                                }
                            }
                        });
                    ui.add_space(10.0);
                    let (yes, no) = buttons(ui, tr("mods.install_now"), !busy && readable);
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
                Dialog::Remove {
                    id,
                    name,
                    needed_by,
                } => {
                    let mut text = tr("mods.ask_remove_text").to_owned();
                    if !needed_by.is_empty() {
                        text += "\n\n";
                        text += &trf("mods.ask_remove_needed", &[("mods", needed_by)]);
                    }
                    question(ui, &trf("mods.ask_remove_title", &[("name", name)]), &text);
                    let (yes, no) = buttons(ui, tr("mods.remove_now"), !busy);
                    if yes {
                        answer = Some(Answer::Remove(id.clone()));
                    }
                    keep &= !(yes || no);
                }
                Dialog::EnableNeeded { id, name, names } => {
                    question(
                        ui,
                        &trf("mods.ask_enable_needed_title", &[("name", name)]),
                        &trf(
                            "mods.ask_enable_needed_text",
                            &[("name", name), ("mods", names)],
                        ),
                    );
                    let mut only = false;
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if action_button(ui, !busy, tr("mods.enable_with"), Prominence::Main)
                            .clicked()
                        {
                            answer = Some(Answer::EnableWith(id.clone()));
                            keep = false;
                        }
                        if ui
                            .add_enabled(!busy, egui::Button::new(tr("mods.enable_only")))
                            .clicked()
                        {
                            only = true;
                        }
                        if ui.button(tr("mods.cancel")).clicked() {
                            keep = false;
                        }
                    });
                    if only {
                        answer = Some(Answer::Switch(id.clone(), true));
                        keep = false;
                    }
                }
                Dialog::DisableNeeded { id, name, names } => {
                    question(
                        ui,
                        &trf("mods.ask_disable_needed_title", &[("name", name)]),
                        &trf(
                            "mods.ask_disable_needed_text",
                            &[("name", name), ("mods", names)],
                        ),
                    );
                    let (yes, no) = buttons(ui, tr("mods.disable_now"), !busy);
                    if yes {
                        answer = Some(Answer::Switch(id.clone(), false));
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
            Some(Answer::Switch(id, on)) => self.switch(ctx, &id, on),
            Some(Answer::EnableWith(id)) => self.enable_with_dependencies(ctx, &id),
            Some(Answer::Code(id, pressed)) => self.code_answered(ctx, &id, pressed),
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
