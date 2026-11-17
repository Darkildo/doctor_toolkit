//! The last crash of the game, shown by the mod manager and the editor
//! (stage 99): one line with the time and the place, and buttons that open
//! the report and its folder. The reports are written by the crash module
//! (`doctor_toolkit::crash`).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::RichText;

use doctor_toolkit::crash::{self, CrashReport};

use crate::i18n::{tr, trf};

/// How often the folder of the reports is looked at.
const INTERVAL: Duration = Duration::from_secs(2);

/// Opens a file or a folder with the desktop's default program.
pub fn open_path(path: &Path) -> Result<(), String> {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(drop)
        .map_err(|e| format!("{program}: {e}"))
}

/// Follows the crash reports of one game.
pub struct CrashWatch {
    root: PathBuf,
    last: Option<CrashReport>,
    /// The newest report the person was told about.
    told: Option<String>,
    checked: Instant,
    /// Why the report could not be opened.
    error: Option<String>,
}

impl CrashWatch {
    /// Starts with the reports that are there: they are old news.
    pub fn new(root: &Path) -> Self {
        let last = crash::last(root);
        Self {
            root: root.to_path_buf(),
            told: last.as_ref().map(|r| r.name.clone()),
            last,
            checked: Instant::now(),
            error: None,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The newest report.
    pub fn last(&self) -> Option<&CrashReport> {
        self.last.as_ref()
    }

    /// Looks for a new report (at most every two seconds; `now` skips the
    /// wait, for the moment the game exits). Returns it once.
    pub fn poll(&mut self, now: bool) -> Option<&CrashReport> {
        if !now && self.checked.elapsed() < INTERVAL {
            return None;
        }
        self.checked = Instant::now();
        self.last = crash::last(&self.root);
        let name = self.last.as_ref().map(|r| r.name.clone());
        if name.is_some() && name != self.told {
            self.told = name;
            return self.last.as_ref();
        }
        None
    }

    /// What a new crash is announced with.
    pub fn announcement(report: &CrashReport) -> String {
        trf(
            "crash.new",
            &[("what", &what(report)), ("path", &report.path.display())],
        )
    }

    /// Asks once whether the crash module may be kept in the game folder:
    /// shown until the player answers; nothing is installed before that.
    fn ask(&mut self, ui: &mut egui::Ui) {
        if crash::consent(&self.root) != crash::Consent::Unasked {
            return;
        }
        let mut answer = None;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(tr("crash.ask"));
            ui.horizontal(|ui| {
                if ui.button(tr("crash.ask_yes")).clicked() {
                    answer = Some(true);
                }
                if ui.button(tr("crash.ask_no")).clicked() {
                    answer = Some(false);
                }
            });
        });
        if let Some(yes) = answer {
            self.error = answered(&self.root, yes).err();
        }
    }

    /// The question about the crash module, the line of the last crash with
    /// its buttons; nothing when the player answered and the game has no
    /// report.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.ask(ui);
        let Some(report) = &self.last else {
            return;
        };
        let mut open = None;
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(trf(
                    "crash.last",
                    &[("when", &when(report)), ("what", &what(report))],
                ))
                .color(crate::WARN),
            )
            .on_hover_text(report.line());
            if ui
                .small_button(tr("crash.open_report"))
                .on_hover_text(tr("crash.open_report_hint"))
                .clicked()
            {
                open = Some(report.path.clone());
            }
            if ui
                .small_button(tr("crash.open_folder"))
                .on_hover_text(tr("crash.open_folder_hint"))
                .clicked()
            {
                open = Some(crash::dir(&self.root));
            }
        });
        if let Some(path) = open {
            self.error = open_path(&path).err();
        }
        if let Some(error) = &self.error {
            ui.colored_label(crate::BAD, trf("crash.open_failed", &[("error", error)]));
        }
    }
}

/// When a crash happened, as the person's clock showed it: the report
/// has `… UTC (… local)`.
fn when(report: &CrashReport) -> String {
    let time = &report.summary.time;
    time.split_once('(')
        .and_then(|(_, local)| local.strip_suffix(" local)"))
        .unwrap_or(time)
        .to_owned()
}

/// The place of a crash in a few words: the name the knowledge base gives
/// it, else the module and the offset.
fn what(report: &CrashReport) -> String {
    let s = &report.summary;
    let place = match s.address {
        Some(a) => format!("{a:#010x} {}", s.place),
        None => s.exception.clone(),
    };
    match report.place_name() {
        Some(name) => {
            let short: String = name.chars().take(90).collect();
            let dots = if name.chars().count() > 90 { "…" } else { "" };
            format!("{place} — {short}{dots}")
        }
        None => place,
    }
}

/// Records the answer to the question about the crash module; with "yes"
/// the module is installed at once when it can be.
fn answered(root: &Path, yes: bool) -> Result<(), String> {
    crash::set_disabled(root, !yes).map_err(|e| e.to_string())?;
    if yes {
        // What cannot be done now (no loader, another edition) is said
        // again before the game starts.
        let _ = crash::ensure(root);
    }
    Ok(())
}

/// The switch of the crash reports of the game in `root`, for a menu.
/// Returns a line for the person when something changed.
pub fn switch(ui: &mut egui::Ui, root: &Path) -> Option<String> {
    let mut on = crash::consent(root) == crash::Consent::Yes;
    let before = on;
    ui.checkbox(&mut on, tr("crash.enabled"))
        .on_hover_text(tr("crash.enabled_hint"));
    if on == before {
        return None;
    }
    let result = if on {
        crash::set_disabled(root, false).map(|()| {
            crash::ensure(root)
                .note()
                .unwrap_or_else(|| tr("crash.switched_on").to_owned())
        })
    } else {
        crash::uninstall(root).map(|_| tr("crash.switched_off").to_owned())
    };
    Some(result.unwrap_or_else(|e| e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPORT: &str = "doctor_crash 0.0.6 on PC 1.0.789.0 (build of 2009-10-05)\n\
time: 2026-11-16 12:55:00 UTC (2026-11-16 15:55:00 local)\n\n[exception]\n\
code: 0xC0000005 ACCESS_VIOLATION (read of 0x0000000c)\n\
address: 0x0042138b godfather2.exe+0x0002138b\n\
known: [known-crashes] падение при `-stream` с двумя и более потоками\n";

    #[test]
    fn a_new_report_is_told_once() {
        let dir = tempfile::tempdir().unwrap();
        let crashes = crash::dir(dir.path());
        std::fs::create_dir_all(&crashes).unwrap();
        std::fs::write(crashes.join("crash-20261116-125500.txt"), REPORT).unwrap();
        // What was there when the game was opened is not news.
        let mut watch = CrashWatch::new(dir.path());
        assert!(watch.last().is_some());
        assert!(watch.poll(true).is_none());
        std::fs::write(crashes.join("crash-20261116-130000.txt"), REPORT).unwrap();
        // Not looked at before the interval passed.
        assert!(watch.poll(false).is_none());
        let new = watch.poll(true).expect("the new report");
        assert_eq!(new.name, "crash-20261116-130000");
        assert!(what(new).starts_with("0x0042138b godfather2.exe+0x0002138b — [known-crashes]"));
        assert!(watch.poll(true).is_none());
        assert_eq!(when(watch.last().unwrap()), "2026-11-16 15:55:00");
    }
}
