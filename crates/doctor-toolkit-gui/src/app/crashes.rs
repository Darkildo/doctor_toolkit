//! Tools → Crash reports (stage 103): the reports of the crash module of
//! the open game — the list, newest first, and the text of one with the
//! names of the messages added — as `crash-report list|show` and the MCP
//! tool `crash_reports` give them. Read only; whether the module is in the
//! game stays the player's choice in the mod manager.
//!
//! `DOCTOR_TOOLKIT_GUI_SHELL=crashes[=<N or name>]` opens the window (and a
//! report: 1 is the newest) without input.

use doctor_toolkit::crash::{self, CrashReport};
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::RichText;

use super::BrowserApp;
use super::shell::Level;

/// State of the window.
#[derive(Default)]
pub(crate) struct CrashWindow {
    pub open: bool,
    /// The reports of the game, newest first.
    pub reports: Vec<CrashReport>,
    /// Name of the report shown and its text.
    pub shown: Option<(String, String)>,
}

impl BrowserApp {
    /// Opens the window with the reports of the open game read again;
    /// returns how many there are.
    pub(crate) fn open_crash_reports(&mut self) -> usize {
        let Some(game) = &self.game else {
            return 0;
        };
        self.crashes.reports = crash::reports(game.root());
        self.crashes.open = true;
        self.crashes.reports.len()
    }

    /// Shows report `which` (its number, 1 the newest, or its name); an
    /// error when there is none or it cannot be read.
    pub(crate) fn show_crash_report(&mut self, which: &str) -> bool {
        let count = self.open_crash_reports();
        let picked = crash::pick(self.crashes.reports.clone(), Some(which));
        let result = picked
            .ok_or_else(|| trf("crashes.none_such", &[("which", &which), ("count", &count)]))
            .and_then(|report| {
                std::fs::read_to_string(&report.path)
                    .map(|text| (report.name.clone(), crash::annotate(&text)))
                    .map_err(|e| {
                        trf(
                            "crashes.cannot_read",
                            &[("path", &report.path.display()), ("error", &e)],
                        )
                    })
            });
        match result {
            Ok(shown) => {
                self.crashes.shown = Some(shown);
                true
            }
            Err(error) => {
                self.notify(Level::Error, error);
                false
            }
        }
    }

    /// The window, when open.
    pub(super) fn crash_window(&mut self, ctx: &egui::Context) {
        if !self.crashes.open {
            return;
        }
        let Some(game) = self.game.clone() else {
            return;
        };
        let mut open = true;
        let mut show = None;
        let mut reload = false;
        egui::Window::new(tr("crashes.title"))
            .open(&mut open)
            .default_size([760.0, 520.0])
            .show(ctx, |ui| {
                let root = game.root();
                let module = match crash::installed(root) {
                    crash::Installed::Ours => tr("crashes.module_ours"),
                    crash::Installed::Foreign => tr("crashes.module_foreign"),
                    crash::Installed::No => tr("crashes.module_none"),
                };
                ui.horizontal_wrapped(|ui| {
                    ui.label(trf(
                        "crashes.head",
                        &[("count", &self.crashes.reports.len()), ("module", &module)],
                    ));
                    if ui.button(tr("project.refresh")).clicked() {
                        reload = true;
                    }
                    if ui.button(tr("crashes.folder")).clicked() {
                        let dir = crash::dir(root);
                        if let Err(error) = super::shell::open_external(dir.as_os_str()) {
                            self.status = trf(
                                "status.cannot_open",
                                &[("what", &dir.display()), ("error", &error)],
                            );
                        }
                    }
                });
                ui.label(RichText::new(tr("crashes.hint")).weak().small());
                if self.crashes.reports.is_empty() {
                    ui.label(RichText::new(tr("crashes.none")).weak());
                    return;
                }
                egui::Panel::left("crash-list")
                    .resizable(true)
                    .default_size(300.0)
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("crash-list-scroll")
                            .show(ui, |ui| {
                                for report in &self.crashes.reports {
                                    let selected = self
                                        .crashes
                                        .shown
                                        .as_ref()
                                        .is_some_and(|(name, _)| *name == report.name);
                                    if ui
                                        .selectable_label(selected, report.line())
                                        .on_hover_text(report.path.display().to_string())
                                        .clicked()
                                    {
                                        show = Some(report.name.clone());
                                    }
                                }
                            });
                    });
                egui::ScrollArea::both()
                    .id_salt("crash-text")
                    .auto_shrink([false, false])
                    .show(ui, |ui| match &self.crashes.shown {
                        Some((_, text)) => {
                            ui.add(
                                egui::Label::new(RichText::new(text).monospace().small())
                                    .selectable(true),
                            );
                        }
                        None => {
                            ui.label(RichText::new(tr("crashes.select")).weak());
                        }
                    });
            });
        self.crashes.open = open;
        if reload {
            self.open_crash_reports();
        }
        if let Some(name) = show {
            self.show_crash_report(&name);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use doctor_toolkit::game::GameInstance;
    use doctor_toolkit::testing as mini;

    use super::*;
    use crate::app::Command;
    use crate::settings::{Settings, Store};

    /// A report as the crash module writes it.
    const REPORT: &str = "doctor_crash 0.0.6 on PC 1.0.789.0 (build of 2009-10-05)\n\
time: 2026-11-16 12:55:00 UTC (2026-11-16 15:55:00 local)\n\n[exception]\n\
code: 0xC0000005 ACCESS_VIOLATION (read of 0x0000000c)\n\
address: 0x0042138b godfather2.exe+0x0002138b\n\n\
[messages]\n# The last 48 in order, newest first:\n\
-0.001 s  #7  0xc2010fb7 iMsgStreamBeginLoad  data 0x0389a848  thread 0x24  from 0x00402f78 x\n\n\
[streams]\nloaded: gf2\\cuba.str\n";

    #[test]
    fn crash_reports_are_listed_and_shown() {
        let dir = std::env::temp_dir().join(format!("dt-crashes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        mini::write_game(&dir).unwrap();
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = BrowserApp::new(
            &cc,
            None,
            None,
            None,
            Settings::default(),
            Store { dir: None },
        );
        app.notes.log_to_file = false;
        let game = Arc::new(GameInstance::open(&dir).unwrap());
        app.game = Some(game.clone());
        // No report yet: the window opens empty, asking for one is an error.
        app.run(Command::CrashReports, &ctx);
        assert!(app.crashes.open && app.crashes.reports.is_empty());
        assert!(!app.show_crash_report("1"));
        assert!(app.preset_error().is_some());
        // Two reports: newest first; by number and by name.
        let reports = crash::dir(game.root());
        std::fs::create_dir_all(&reports).unwrap();
        std::fs::write(reports.join("crash-20261116-125500.txt"), REPORT).unwrap();
        let later = REPORT.replace("12:55:00 UTC", "13:10:00 UTC");
        std::fs::write(reports.join("crash-20261116-131000.txt"), later).unwrap();
        assert_eq!(app.open_crash_reports(), 2);
        assert_eq!(app.crashes.reports[0].name, "crash-20261116-131000");
        assert!(app.show_crash_report("1"));
        let (name, text) = app.crashes.shown.clone().unwrap();
        assert_eq!(name, "crash-20261116-131000");
        assert!(
            text.contains("ACCESS_VIOLATION") && text.contains("13:10:00"),
            "{text}"
        );
        assert!(app.show_crash_report("crash-20261116-125500"));
        assert_eq!(
            app.crashes.shown.as_ref().unwrap().0,
            "crash-20261116-125500"
        );
        assert!(!app.show_crash_report("7"), "no such report");
        // The window draws.
        use eframe::App as _;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            ..Default::default()
        };
        let mut frame = eframe::Frame::_new_kittest();
        let mut output = ctx.run_ui(input, |ui| app.ui(ui, &mut frame));
        output.textures_delta.clear();
        assert!(app.crashes.open);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
