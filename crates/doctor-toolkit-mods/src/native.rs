//! Executable code of mods in the mod manager (stage 81): the mark on the
//! card, the question before any code is installed, the state of the code
//! and the buttons that change the answer. The rules are in
//! `doctor_toolkit::mods::native`: nothing executable is written before the
//! player's answer, the answer is kept for the files as they are (by
//! SHA-256), and a mod may be used without its code unless it says the code
//! is required.

use egui::{Color32, RichText};

use doctor_toolkit::mods::ModManager;
use doctor_toolkit::mods::native::{
    self, Answer, Consent, NativeBlock, NativeReport, NativeState, Question, QuestionFile,
};

use crate::i18n::{tr, trf, trn};
use crate::widgets::{Prominence, action_button};
use crate::{BAD, GOOD, ModsPanel, WARN, theme};

/// The code of one installed mod, as the worker thread found it.
#[derive(Debug, Clone)]
pub(crate) struct CodeView {
    pub question: Question,
    pub consent: Consent,
    /// `None` while the mod is switched off.
    pub state: Option<NativeState>,
}

impl CodeView {
    /// Marks the answer: the mod and the hashes it was given for.
    pub fn key(&self) -> String {
        let mut key = self.question.id.clone();
        for file in &self.question.modules {
            key.push(':');
            key += &file.sha256;
        }
        key
    }

    /// The mod is on and nobody answered about its code yet.
    pub fn waits(&self) -> bool {
        self.state.is_some() && self.consent == Consent::Unasked
    }
}

/// The code of every installed mod that has code (worker thread).
pub(crate) fn views(manager: &ModManager) -> Vec<CodeView> {
    let host = manager.native_host();
    let states = native::states(manager, host).unwrap_or_default();
    manager
        .mods()
        .iter()
        .filter(|m| m.manifest.native.is_some() && m.problem.is_none())
        .filter_map(|m| {
            Some(CodeView {
                question: native::question(manager, host, &m.id).ok()??,
                consent: native::consent(manager, &m.id).ok()?,
                state: states
                    .iter()
                    .find(|(id, _)| *id == m.id)
                    .map(|(_, state)| state.clone()),
            })
        })
        .collect()
}

/// Why the code is not installed, in the interface language.
pub(crate) fn block_words(block: &NativeBlock) -> String {
    match block {
        NativeBlock::Unasked => tr("native.block_unasked").to_owned(),
        NativeBlock::Refused => tr("native.block_refused").to_owned(),
        NativeBlock::Edition(why) => trf(
            "native.block_edition",
            &[("edition", &native::EDITION), ("why", why)],
        ),
        NativeBlock::NoLoader => tr("native.block_no_loader").to_owned(),
        NativeBlock::NoPlatform => tr("native.block_no_platform").to_owned(),
        NativeBlock::Abi { required } => trf(
            "native.block_abi",
            &[("required", required), ("have", &native::PLATFORM_ABI)],
        ),
        NativeBlock::FileTaken { file } => trf("native.block_file_taken", &[("file", file)]),
        NativeBlock::Damaged(why) => trf("native.block_damaged", &[("why", why)]),
    }
}

/// The state of the code in a few words, and its colour.
pub(crate) fn state_words(view: &CodeView) -> (String, Color32) {
    match &view.state {
        None => (tr("native.state_off").to_owned(), Color32::GRAY),
        Some(NativeState::Installed) => (tr("native.state_installed").to_owned(), GOOD),
        Some(NativeState::NotWritten) => (tr("native.state_agreed").to_owned(), theme::FOCUS),
        Some(NativeState::Blocked(block)) => (
            block_words(block),
            if view.question.required { BAD } else { WARN },
        ),
    }
}

/// What writing or restoring did with the code, for the result window.
pub(crate) fn report_words(report: &NativeReport) -> Option<String> {
    let mut lines = Vec::new();
    if !report.written.is_empty() {
        lines.push(trn(
            "native.report_installed",
            report.written.len() as u64,
            &[("files", &report.written.join(", "))],
        ));
    }
    if !report.removed.is_empty() {
        lines.push(trn(
            "native.report_removed",
            report.removed.len() as u64,
            &[("files", &report.removed.join(", "))],
        ));
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// What the player chose in the question or the details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodeAnswer {
    Allow,
    DataOnly,
    /// Switch the mod off.
    Disable,
    /// Forget the answer: the question comes again.
    Forget,
    /// Open the question.
    Ask,
    /// Close the question without an answer.
    Later,
}

fn file_rows(ui: &mut egui::Ui, files: &[QuestionFile]) {
    for file in files {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&file.path).strong().monospace());
            ui.label(
                RichText::new(trn("native.size_bytes", file.size, &[]))
                    .weak()
                    .small(),
            );
        });
        ui.label(
            RichText::new(format!("SHA-256 {}", file.sha256))
                .monospace()
                .small()
                .weak(),
        );
    }
}

/// The body of the question; returns the button pressed.
pub(crate) fn question_ui(ui: &mut egui::Ui, view: &CodeView, busy: bool) -> Option<CodeAnswer> {
    let q = &view.question;
    ui.set_width(640.0);
    ui.label(
        RichText::new(trf("native.ask_title", &[("name", &q.name)]))
            .size(18.0)
            .strong()
            .color(WARN),
    );
    ui.add_space(6.0);
    let author = if q.author.is_empty() {
        tr("native.author_unknown").to_owned()
    } else {
        q.author.clone()
    };
    ui.add(
        egui::Label::new(trf(
            "native.ask_text",
            &[
                ("name", &q.name),
                ("version", &q.version),
                ("author", &author),
            ],
        ))
        .wrap(),
    );
    ui.add_space(8.0);
    ui.label(RichText::new(tr("native.ask_files")).strong());
    file_rows(ui, &q.modules);
    if !q.toolkit.is_empty() {
        ui.add_space(6.0);
        ui.label(RichText::new(tr("native.ask_toolkit")).strong());
        file_rows(ui, &q.toolkit);
    }
    ui.add_space(8.0);
    if let Some(obstacle) = &q.obstacle {
        ui.add(
            egui::Label::new(
                RichText::new(trf(
                    "native.ask_obstacle",
                    &[("why", &block_words(obstacle))],
                ))
                .color(BAD),
            )
            .wrap(),
        );
    }
    ui.add(
        egui::Label::new(
            RichText::new(if q.required {
                tr("native.ask_required")
            } else {
                tr("native.ask_optional")
            })
            .weak(),
        )
        .wrap(),
    );
    ui.add(egui::Label::new(RichText::new(tr("native.ask_remembered")).weak()).wrap());
    ui.add_space(10.0);
    let mut answer = None;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        // The careful choice is the prominent one.
        if !q.required
            && action_button(ui, !busy, tr("native.data_only"), Prominence::Main)
                .on_hover_text(tr("native.data_only_hint"))
                .clicked()
        {
            answer = Some(CodeAnswer::DataOnly);
        }
        if ui
            .add_enabled(!busy, egui::Button::new(tr("native.allow")))
            .on_hover_text(tr("native.allow_hint"))
            .clicked()
        {
            answer = Some(CodeAnswer::Allow);
        }
        if view.state.is_some()
            && ui
                .add_enabled(!busy, egui::Button::new(tr("native.disable")))
                .clicked()
        {
            answer = Some(CodeAnswer::Disable);
        }
        if ui.button(tr("native.later")).clicked() {
            answer = Some(CodeAnswer::Later);
        }
    });
    answer
}

/// The section of the selected mod: what its code is, where it stands, and
/// the buttons that change the answer.
pub(crate) fn section_ui(ui: &mut egui::Ui, view: &CodeView, idle: bool) -> Option<CodeAnswer> {
    let mut answer = None;
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        crate::widgets::chip(ui, tr("native.badge"), WARN).on_hover_text(tr("native.badge_hint"));
        let (words, color) = state_words(view);
        ui.add(egui::Label::new(RichText::new(words).color(color)).wrap());
    });
    file_rows(ui, &view.question.modules);
    ui.horizontal(|ui| match view.consent {
        Consent::Unasked => {
            if ui
                .add_enabled(idle, egui::Button::new(tr("native.answer")))
                .clicked()
            {
                answer = Some(CodeAnswer::Ask);
            }
        }
        Consent::Code | Consent::DataOnly => {
            ui.label(
                RichText::new(if view.consent == Consent::Code {
                    tr("native.you_allowed")
                } else {
                    tr("native.you_refused")
                })
                .weak(),
            );
            if ui
                .add_enabled(idle, egui::Button::new(tr("native.forget")).small())
                .on_hover_text(tr("native.forget_hint"))
                .clicked()
            {
                answer = Some(CodeAnswer::Forget);
            }
        }
    });
    answer
}

impl ModsPanel {
    /// The code of mod `id`, when it has any.
    pub(crate) fn code_of(&self, id: &str) -> Option<&CodeView> {
        self.overview.code.iter().find(|c| c.question.id == id)
    }

    /// Opens the question about the code of the first enabled mod nobody
    /// answered about, once for each build of its code; "Decide later"
    /// leaves the card waiting, and the details open the question again.
    pub(crate) fn offer_code_question(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        let Some(view) = self
            .overview
            .code
            .iter()
            .find(|c| c.waits() && !self.code_asked.contains(&c.key()))
        else {
            return;
        };
        self.code_asked.insert(view.key());
        self.dialog = Some(crate::dialogs::Dialog::Code(Box::new(view.clone())));
    }

    /// Closes the question about the code of a mod without an answer, as
    /// "Decide later" does.
    pub fn close_code_question(&mut self) {
        if matches!(self.dialog, Some(crate::dialogs::Dialog::Code(_))) {
            self.dialog = None;
        }
    }

    /// Opens the question about the code of mod `id`.
    pub fn ask_code(&mut self, id: &str) {
        if let Some(view) = self.code_of(id).cloned() {
            self.code_asked.insert(view.key());
            self.dialog = Some(crate::dialogs::Dialog::Code(Box::new(view)));
        }
    }

    /// Records the player's answer about the code of mod `id`. Allowing
    /// installs nothing yet: "Write to game" does. Refusing and forgetting
    /// take the code out of the game at once.
    pub fn answer_code(&mut self, ctx: &egui::Context, id: &str, allow: Option<bool>) {
        let name = self.name_of(id);
        let id = id.to_owned();
        self.run(ctx, tr("native.job_answer"), false, move |_, manager| {
            let answer = allow.map(|yes| if yes { Answer::Code } else { Answer::Data });
            native::set_consent(manager, &id, answer).map_err(|e| e.to_string())?;
            let removed = native::prune(manager)
                .map_err(|e| e.to_string())?
                .removed
                .len();
            let mut message = match allow {
                Some(true) => trf("native.allowed", &[("name", &name)]),
                Some(false) => trf("native.refused", &[("name", &name)]),
                None => trf("native.forgot", &[("name", &name)]),
            };
            if removed > 0 {
                message.push(' ');
                message += &trn("native.removed_now", removed as u64, &[]);
            }
            Ok(message)
        });
    }

    pub(crate) fn code_answered(&mut self, ctx: &egui::Context, id: &str, answer: CodeAnswer) {
        match answer {
            CodeAnswer::Allow => self.answer_code(ctx, id, Some(true)),
            CodeAnswer::DataOnly => self.answer_code(ctx, id, Some(false)),
            CodeAnswer::Forget => self.answer_code(ctx, id, None),
            CodeAnswer::Disable => self.switch(ctx, id, false),
            CodeAnswer::Ask => self.ask_code(id),
            CodeAnswer::Later => {}
        }
    }
}
