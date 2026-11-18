//! The window of `gf2-unify`: what the program shows when it is started
//! without arguments. Three folders, a language, one button.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;

use doctor_toolkit_unify::locale::{self, LANGUAGES};
use doctor_toolkit_unify::{self as unify, Manifest, Source};

/// Interface language.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ui {
    En,
    Ru,
}

impl Ui {
    fn from_env() -> Self {
        let russian = ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"]
            .iter()
            .filter_map(|v| std::env::var(v).ok())
            .any(|v| v.to_ascii_lowercase().starts_with("ru"));
        if russian || windows_is_russian() {
            Ui::Ru
        } else {
            Ui::En
        }
    }

    fn t<'a>(self, en: &'a str, ru: &'a str) -> &'a str {
        match self {
            Ui::En => en,
            Ui::Ru => ru,
        }
    }
}

#[cfg(windows)]
fn windows_is_russian() -> bool {
    unsafe extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
    }
    // Primary language id 0x19: Russian.
    (unsafe { GetUserDefaultUILanguage() } & 0x3FF) == 0x19
}

#[cfg(not(windows))]
fn windows_is_russian() -> bool {
    false
}

/// What the worker thread tells the window.
#[derive(Default)]
struct Job {
    running: bool,
    done: u64,
    total: u64,
    what: String,
    /// The outcome of the last run.
    result: Option<Result<String, String>>,
}

struct App {
    ui: Ui,
    source: String,
    pack: String,
    output: String,
    language: usize,
    /// The player agreed to a pack that is not a published one.
    trust_unknown: bool,
    /// The last look at the folders: what was asked, when, the answer.
    checked: Option<(CheckKey, Instant, Check)>,
    job: Arc<Mutex<Job>>,
}

type CheckKey = (Ui, String, String, String);

/// How long a look at the folders stays good: a download may go on meanwhile.
const CHECK_AGE: Duration = Duration::from_secs(2);

fn default_pack() -> String {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("pack")));
    let here = std::env::current_dir().ok().map(|dir| dir.join("pack"));
    [beside, here]
        .into_iter()
        .flatten()
        .find(|p| p.join(unify::MANIFEST).is_file())
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

/// What is known about the chosen folders: a line for each, and whether
/// the game can be made.
#[derive(Clone)]
struct Check {
    pack: Result<String, String>,
    /// The pack is not one published with this program.
    unknown_pack: bool,
    source: Result<String, String>,
    output: Result<String, String>,
}

impl Check {
    fn ready(&self, trust_unknown: bool) -> bool {
        self.pack.is_ok()
            && self.source.is_ok()
            && self.output.is_ok()
            && (!self.unknown_pack || trust_unknown)
    }
}

fn gib(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / (1u64 << 30) as f64)
}

fn check(ui: Ui, pack: &str, source: &str, output: &str) -> Check {
    let manifest = if pack.trim().is_empty() {
        Err(ui
            .t(
                "choose the folder with unified.json",
                "укажите папку с файлом unified.json",
            )
            .to_string())
    } else {
        Manifest::load(Path::new(pack.trim())).map_err(|e| e.to_string())
    };
    let opened = match (&manifest, source.trim()) {
        (_, "") => Err(ui
            .t(
                "choose your game disc (a mounted image) or an installed copy",
                "укажите диск игры (смонтированный образ) или установленную копию",
            )
            .to_string()),
        (Err(_), _) => Err(ui
            .t("choose the pack first", "сначала укажите пакет")
            .to_string()),
        (Ok(manifest), path) => Source::open(manifest, Path::new(path)).map_err(|e| e.to_string()),
    };
    let source_line = opened.as_ref().map_err(Clone::clone).and_then(|s| {
        let manifest = manifest
            .as_ref()
            .expect("a source is opened with a manifest");
        let edition = &manifest.editions[s.edition];
        let kind = match (s.disc, ui) {
            (true, Ui::En) => "a disc",
            (true, Ui::Ru) => "диск",
            (false, Ui::En) => "an installed copy",
            (false, Ui::Ru) => "установленная копия",
        };
        // What of the pack this edition needs and the folder lacks.
        let gaps = unify::pack_gaps(manifest, Path::new(pack.trim()), edition);
        if gaps.is_empty() {
            return Ok(format!("{}, {kind}", edition.name));
        }
        let mut parts: Vec<&str> = gaps.iter().filter_map(|g| g.split('/').next()).collect();
        parts.dedup();
        Err(match ui {
            Ui::En => format!(
                "{}, {kind}. The pack lacks {} file(s) this edition needs (e.g. {}): download its \
                 parts {} to the end",
                edition.name,
                gaps.len(),
                gaps[0],
                parts.join(", ")
            ),
            Ui::Ru => format!(
                "{}, {kind}. В пакете не хватает файлов, нужных этому изданию: {} (например, {}). \
                 Докачайте части {}",
                edition.name,
                gaps.len(),
                gaps[0],
                parts.join(", ")
            ),
        })
    });
    let known = match &manifest {
        Ok(_) => unify::known_pack(Path::new(pack.trim())).ok().flatten(),
        Err(_) => None,
    };
    let pack_line = manifest
        .as_ref()
        .map(|m| {
            let size = format!("{} {}", gib(m.total_size()), ui.t("GiB", "ГБ"));
            match (known, ui) {
                (Some(k), Ui::En) => format!(
                    "{} ({size}) — the published pack, release {}",
                    m.name, k.release
                ),
                (Some(k), Ui::Ru) => format!(
                    "{} ({size}) — опубликованный пакет, выпуск {}",
                    m.name, k.release
                ),
                (None, _) => format!("{} ({size})", m.name),
            }
        })
        .map_err(Clone::clone);
    let output_line = match output.trim() {
        "" => Err(ui
            .t(
                "choose a new folder for the game",
                "укажите новую папку для игры",
            )
            .to_string()),
        path if !source.trim().is_empty() && Path::new(path) == Path::new(source.trim()) => Err(ui
            .t(
                "it must be another folder than your copy",
                "это должна быть другая папка, не ваша копия",
            )
            .to_string()),
        _ => Ok(match &manifest {
            Ok(m) => format!(
                "{} {} {}",
                ui.t("needs about", "нужно около"),
                gib(m.total_size()),
                ui.t("GiB of free space", "ГБ свободного места")
            ),
            Err(_) => String::new(),
        }),
    };
    Check {
        unknown_pack: manifest.is_ok() && known.is_none(),
        pack: pack_line,
        source: source_line,
        output: output_line,
    }
}

impl App {
    fn new(preset: Preset) -> Self {
        let ui = Ui::from_env();
        let text = |path: Option<PathBuf>| path.map(|p| p.display().to_string());
        Self {
            ui,
            source: text(preset.source).unwrap_or_default(),
            pack: text(preset.pack).unwrap_or_else(default_pack),
            output: text(preset.output).unwrap_or_default(),
            language: if ui == Ui::Ru { 2 } else { 0 },
            trust_unknown: false,
            checked: None,
            job: Arc::default(),
        }
    }

    /// What is known about the folders; looked at again when a field
    /// changes and every [`CHECK_AGE`].
    fn check(&mut self) -> Check {
        let key = (
            self.ui,
            self.pack.clone(),
            self.source.clone(),
            self.output.clone(),
        );
        if let Some((old, at, check)) = &self.checked
            && *old == key
            && at.elapsed() < CHECK_AGE
        {
            return check.clone();
        }
        let check = check(self.ui, &self.pack, &self.source, &self.output);
        self.checked = Some((key, Instant::now(), check.clone()));
        check
    }

    fn start(&self, ctx: &egui::Context) {
        let (pack, source, output) = (
            PathBuf::from(self.pack.trim()),
            PathBuf::from(self.source.trim()),
            PathBuf::from(self.output.trim()),
        );
        let code = LANGUAGES[self.language].code;
        let job = self.job.clone();
        let ctx = ctx.clone();
        *job.lock().expect("job state") = Job {
            running: true,
            ..Job::default()
        };
        std::thread::spawn(move || {
            let mut progress = |done: u64, total: u64, what: &str| {
                let mut state = job.lock().expect("job state");
                (state.done, state.total) = (done, total);
                state.what = what.to_string();
                drop(state);
                ctx.request_repaint();
            };
            let result = unify::apply(&pack, &source, &output, &mut progress)
                .map_err(|e| e.to_string())
                .map(|report| {
                    let language = locale::set_language(code).unwrap_or_else(|e| e.to_string());
                    format!(
                        "{}\n{} / {} / {}\n{language}",
                        output.display(),
                        report.from_source,
                        report.from_pack,
                        report.archives_built
                    )
                });
            let mut state = job.lock().expect("job state");
            state.running = false;
            state.result = Some(result);
            drop(state);
            ctx.request_repaint();
        });
    }
}

fn folder_row(
    ui: &mut egui::Ui,
    label: &str,
    hint: &str,
    value: &mut String,
    browse: &str,
    enabled: bool,
) {
    ui.label(egui::RichText::new(label).strong());
    ui.horizontal(|ui| {
        ui.add_enabled_ui(enabled, |ui| {
            if ui.button(browse).clicked()
                && let Some(dir) = rfd::FileDialog::new().set_title(label).pick_folder()
            {
                *value = dir.display().to_string();
            }
            ui.add(
                egui::TextEdit::singleline(value)
                    .hint_text(hint)
                    .desired_width(f32::INFINITY),
            );
        });
    });
}

const WARN: egui::Color32 = egui::Color32::from_rgb(230, 170, 90);

/// Where the pack is taken from.
fn pack_hint(ui: &mut egui::Ui, t: Ui) {
    let magnet = unify::latest_pack().magnet;
    ui.label(t.t(
        "The pack is shared by torrent. Open the link below in a torrent client, tick `common` and the \
         part of your edition (`for-pal` or `for-ntsc`; both when not sure) and download it next to this \
         program: the folder `pack` is then found by itself.",
        "Пакет раздаётся торрентом. Откройте ссылку ниже в торрент-клиенте, отметьте `common` и часть \
         своего издания (`for-pal` или `for-ntsc`; если не уверены — обе) и скачайте рядом с этой \
         программой: папка `pack` тогда находится сама.",
    ));
    ui.horizontal(|ui| {
        if ui
            .button(t.t("Copy the link", "Скопировать ссылку"))
            .clicked()
        {
            ui.ctx().copy_text(magnet.to_string());
        }
        if ui
            .button(t.t("Open in the torrent client", "Открыть в торрент-клиенте"))
            .clicked()
        {
            ui.ctx().open_url(egui::OpenUrl::new_tab(magnet));
        }
        let mut text = magnet;
        ui.add(egui::TextEdit::singleline(&mut text).desired_width(f32::INFINITY));
    });
    ui.add_space(10.0);
}

fn status(ui: &mut egui::Ui, line: &Result<String, String>) {
    match line {
        Ok(text) if text.is_empty() => {}
        Ok(text) => drop(ui.label(
            egui::RichText::new(text.as_str()).color(egui::Color32::from_rgb(110, 190, 110)),
        )),
        Err(text) => drop(ui.label(
            egui::RichText::new(text.as_str()).color(egui::Color32::from_rgb(230, 170, 90)),
        )),
    }
    ui.add_space(10.0);
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let t = self.ui;
        let (running, done, total, what, result) = {
            let job = self.job.lock().expect("job state");
            (
                job.running,
                job.done,
                job.total,
                job.what.clone(),
                job.result.clone(),
            )
        };
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(t.t("The Godfather II — the unified game", "The Godfather II — единая игра"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.selectable_value(&mut self.ui, Ui::Ru, "Русский");
                    ui.selectable_value(&mut self.ui, Ui::En, "English");
                });
            });
            ui.label(t.t(
                "Makes a new game folder — version 1.0.789.0 with every language — from your own disc or \
                 installed copy (PAL or NTSC) and the pack. Your copy is only read.",
                "Собирает новую папку игры — версия 1.0.789.0 со всеми языками — из вашего диска или \
                 установленной копии (PAL или NTSC) и пакета. Ваша копия только читается.",
            ));
            ui.add_space(12.0);

            let check = self.check();
            // A download may finish meanwhile: look again soon.
            ui.ctx().request_repaint_after(CHECK_AGE);
            let browse = t.t("Browse…", "Обзор…");
            folder_row(
                ui,
                t.t("1. Your copy of the game", "1. Ваша копия игры"),
                t.t("the disc (a mounted image) or the installed game", "диск (смонтированный образ) или установленная игра"),
                &mut self.source,
                browse,
                !running,
            );
            status(ui, &check.source);
            folder_row(
                ui,
                t.t("2. The pack", "2. Пакет"),
                t.t("the folder with unified.json", "папка с файлом unified.json"),
                &mut self.pack,
                browse,
                !running,
            );
            status(ui, &check.pack);
            if check.pack.is_err() {
                pack_hint(ui, t);
            } else if check.unknown_pack {
                ui.label(
                    egui::RichText::new(t.t(
                        "This is not a pack published with this program: nobody vouches for its files, \
                         the game executable among them.",
                        "Это не тот пакет, что опубликован с этой программой: за его файлы, включая \
                         исполняемый файл игры, никто не ручается.",
                    ))
                    .color(WARN),
                );
                ui.add_enabled_ui(!running, |ui| {
                    ui.checkbox(
                        &mut self.trust_unknown,
                        t.t("I trust where this pack came from", "Я доверяю источнику этого пакета"),
                    );
                });
                pack_hint(ui, t);
            }
            folder_row(
                ui,
                t.t("3. Where to make the game", "3. Куда собрать игру"),
                t.t("a new folder", "новая папка"),
                &mut self.output,
                browse,
                !running,
            );
            status(ui, &check.output);

            ui.label(egui::RichText::new(t.t("4. Language of the game", "4. Язык игры")).strong());
            ui.add_enabled_ui(!running, |ui| {
                egui::ComboBox::from_id_salt("language")
                    .selected_text(LANGUAGES[self.language].name)
                    .show_ui(ui, |ui| {
                        for (i, language) in LANGUAGES.iter().enumerate() {
                            let speech = if language.speech {
                                t.t("text and speech", "текст и озвучка")
                            } else {
                                t.t("text, English speech", "текст, озвучка английская")
                            };
                            ui.selectable_value(&mut self.language, i, format!("{} — {speech}", language.name));
                        }
                    });
            });
            ui.add_space(16.0);

            if running {
                let fraction = if total == 0 { 0.0 } else { done as f32 / total as f32 };
                ui.add(egui::ProgressBar::new(fraction).show_percentage());
                ui.label(format!("{} / {} {}   {what}", gib(done), gib(total), t.t("GiB", "ГБ")));
            } else {
                let button = egui::Button::new(
                    egui::RichText::new(t.t("Make the game", "Собрать игру")).size(16.0),
                )
                .min_size(egui::vec2(180.0, 34.0));
                if ui.add_enabled(check.ready(self.trust_unknown), button).clicked() {
                    self.start(ui.ctx());
                }
            }
            ui.add_space(8.0);
            match result {
                Some(Ok(text)) if !running => {
                    let mut lines = text.lines();
                    ui.label(
                        egui::RichText::new(format!(
                            "{} {}",
                            t.t("Done. The game is in", "Готово. Игра в папке"),
                            lines.next().unwrap_or_default()
                        ))
                        .color(egui::Color32::from_rgb(110, 190, 110))
                        .strong(),
                    );
                    ui.label(format!(
                        "{}: {}",
                        t.t("files from your copy / from the pack / sound archives built", "файлов из вашей копии / из пакета / собрано звуковых архивов"),
                        lines.next().unwrap_or_default()
                    ));
                    ui.label(format!("{}: {}", t.t("Language", "Язык"), lines.collect::<Vec<_>>().join(" ")));
                    ui.label(t.t(
                        "Start the game with godfather2.exe of that folder.",
                        "Игра запускается файлом godfather2.exe из этой папки.",
                    ));
                }
                Some(Err(error)) if !running => {
                    ui.label(
                        egui::RichText::new(t.t("The game could not be made:", "Собрать игру не удалось:"))
                            .color(egui::Color32::from_rgb(230, 110, 100))
                            .strong(),
                    );
                    egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                        ui.add(egui::Label::new(error).selectable(true).wrap());
                    });
                    ui.label(t.t(
                        "Finished files are kept: fix the cause and press the button again.",
                        "Готовые файлы сохранены: устраните причину и нажмите кнопку ещё раз.",
                    ));
                }
                _ => {}
            }
        });
    }
}

/// Folders the window starts with.
#[derive(Default)]
pub struct Preset {
    pub source: Option<PathBuf>,
    pub pack: Option<PathBuf>,
    pub output: Option<PathBuf>,
}

pub fn run(preset: Preset) -> Result<(), String> {
    let title = "The Godfather II — unified game";
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([760.0, 640.0])
            .with_min_inner_size([560.0, 460.0]),
        ..Default::default()
    };
    eframe::run_native(
        title,
        options,
        Box::new(move |creation| {
            // The colours of the status lines are chosen for the dark theme.
            creation.egui_ctx.set_theme(egui::Theme::Dark);
            Ok(Box::new(App::new(preset)))
        }),
    )
    .map_err(|e| e.to_string())
}
