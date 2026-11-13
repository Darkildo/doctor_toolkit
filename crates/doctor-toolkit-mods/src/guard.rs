//! Keeps a window open when a frame panics, and the command line flags both
//! graphical programs share.
//!
//! The library returns errors instead of panicking; a panic is a bug. Without
//! this guard it would close the window and lose unsaved work. With it the
//! window shows the error, the log file ([`doctor_toolkit::report`]) gets the
//! details and the user can carry on or quit.

use std::panic::{AssertUnwindSafe, catch_unwind};

use egui::RichText;

use doctor_toolkit::report;

use crate::i18n::{tr, trf};

/// An app whose frames are protected from panics.
///
/// Every method of [`eframe::App`] except `on_exit` is passed on; `on_exit`
/// changes its signature with eframe's `glow` feature.
pub struct Guarded<A> {
    app: A,
    failure: Option<String>,
}

impl<A> Guarded<A> {
    pub fn new(app: A) -> Self {
        Self { app, failure: None }
    }
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned())
}

impl<A: eframe::App> Guarded<A> {
    fn failed(&mut self, payload: &(dyn std::any::Any + Send)) {
        // The panic hook has logged the details and the backtrace.
        self.failure = Some(panic_text(payload));
    }

    fn show_failure(&mut self, ui: &mut egui::Ui) {
        let Some(message) = self.failure.clone() else {
            return;
        };
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading(RichText::new(tr("guard.title")).color(crate::BAD));
            ui.label(trf("guard.text", &[("message", &message)]));
            if let Some(path) = report::log_path() {
                ui.label(trf("guard.details", &[("path", &path.display())]));
            }
            ui.horizontal(|ui| {
                if ui.button(tr("guard.continue")).clicked() {
                    self.failure = None;
                }
                if ui.button(tr("guard.quit")).clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
    }
}

impl<A: eframe::App> eframe::App for Guarded<A> {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if self.failure.is_some() {
            return;
        }
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| self.app.logic(ctx, frame))) {
            self.failed(payload.as_ref());
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if self.failure.is_some() {
            self.show_failure(ui);
            return;
        }
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| self.app.ui(ui, frame))) {
            self.failed(payload.as_ref());
            ui.ctx().request_repaint();
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.app.save(storage);
    }

    fn auto_save_interval(&self) -> std::time::Duration {
        self.app.auto_save_interval()
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        self.app.clear_color(visuals)
    }

    fn persist_egui_memory(&self) -> bool {
        self.app.persist_egui_memory()
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.app.raw_input_hook(ctx, raw_input);
    }
}

/// Handles `--version` and `--help` of a graphical program. Returns `true`
/// when one was given and printed; the program should then exit.
pub fn print_version_or_help(program: &str, usage: &str) -> bool {
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => {
            println!("{}", report::version_line(program));
            true
        }
        Some("--help" | "-h") => {
            println!("{}\n\nUsage: {usage}", report::version_line(program));
            if let Some(path) = report::log_path() {
                println!("\nLog file: {}", path.display());
            }
            true
        }
        _ => false,
    }
}
