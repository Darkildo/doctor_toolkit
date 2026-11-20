//! Launch settings shared by the editor and Open Mod Manager: DXVK and the
//! graphics of a launch (the graphics module, Large Address Aware, the Vulkan
//! driver; stage 59). They live in the editor's `settings.toml`, in
//! `games."<game folder>".play` ([`PlayConfig`]): the editor's Play options
//! and the mod manager's "Launch settings" change the same values, and the
//! game starts the same from both.
//!
//! The mod manager writes only the keys `dxvk` and `graphics` of that table
//! and keeps the rest of the file; the editor takes them from the file when
//! it opens its Play options, starts the game and saves its settings, so
//! neither program puts back an old value of the other.

use std::path::{Path, PathBuf};

use egui::RichText;

use doctor_toolkit::play::render::GraphicsOptions;
use doctor_toolkit::play::{self, LaunchMethod, LaunchPlan, PlayConfig};

use crate::i18n::{tr, trf};

/// The editor's settings file in the settings folder.
pub const SETTINGS_FILE: &str = "settings.toml";

/// What both programs share of the launch of one game.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shared {
    pub dxvk: bool,
    pub graphics: GraphicsOptions,
}

impl Shared {
    pub fn of(config: &PlayConfig) -> Self {
        Self {
            dxvk: config.dxvk,
            graphics: config.graphics.clone(),
        }
    }

    /// Puts these values into `config`; returns whether it changed.
    pub fn put_into(&self, config: &mut PlayConfig) -> bool {
        let changed = config.dxvk != self.dxvk || config.graphics != self.graphics;
        config.dxvk = self.dxvk;
        config.graphics = self.graphics.clone();
        changed
    }
}

fn read_table(path: &Path) -> Option<toml::Table> {
    std::fs::read_to_string(path).ok()?.parse().ok()
}

/// The key of the game in `root` in the table `games`: the folder as the
/// editor writes it, or another spelling of the same folder.
fn game_key(games: &toml::Table, root: &Path) -> Option<String> {
    let exact = root.to_string_lossy().into_owned();
    if games.contains_key(&exact) {
        return Some(exact);
    }
    let real = std::fs::canonicalize(root).ok()?;
    games
        .keys()
        .find(|k| std::fs::canonicalize(k).is_ok_and(|p| p == real))
        .cloned()
}

/// How the game in `root` is played, from `settings.toml` in the settings
/// folder `config`; the defaults when the file or the game has none.
pub fn load(config: &Path, root: &Path) -> PlayConfig {
    let Some(table) = read_table(&config.join(SETTINGS_FILE)) else {
        return PlayConfig::default();
    };
    let Some(games) = table.get("games").and_then(|g| g.as_table()) else {
        return PlayConfig::default();
    };
    game_key(games, root)
        .and_then(|key| games.get(&key)?.as_table()?.get("play").cloned())
        .and_then(|play| play.try_into().ok())
        .unwrap_or_default()
}

/// The shared values of the game in `root` as the file has them; `None`
/// when the file or the game has no settings yet.
pub fn load_shared(config: &Path, root: &Path) -> Option<Shared> {
    let table = read_table(&config.join(SETTINGS_FILE))?;
    let games = table.get("games")?.as_table()?;
    game_key(games, root)?;
    Some(Shared::of(&load(config, root)))
}

/// Writes the shared values of the game in `root` into `settings.toml` in
/// `config`, keeping every other key of the file.
pub fn save_shared(config: &Path, root: &Path, shared: &Shared) -> std::io::Result<()> {
    let path = config.join(SETTINGS_FILE);
    let mut table = if path.exists() {
        std::fs::read_to_string(&path)?
            .parse::<toml::Table>()
            .map_err(std::io::Error::other)?
    } else {
        toml::Table::new()
    };
    let games = table
        .entry("games")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .ok_or_else(|| std::io::Error::other("`games` of settings.toml is not a table"))?;
    let key = game_key(games, root).unwrap_or_else(|| root.to_string_lossy().into_owned());
    let game = games
        .entry(key)
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .ok_or_else(|| std::io::Error::other("the game's settings are not a table"))?;
    let play = game
        .entry("play")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .ok_or_else(|| std::io::Error::other("the game's `play` is not a table"))?;
    if shared.dxvk {
        play.insert("dxvk".into(), toml::Value::Boolean(true));
    } else {
        play.remove("dxvk");
    }
    if shared.graphics.is_default() {
        play.remove("graphics");
    } else {
        play.insert(
            "graphics".into(),
            toml::Value::try_from(&shared.graphics).map_err(std::io::Error::other)?,
        );
    }
    let text = toml::to_string_pretty(&table).map_err(std::io::Error::other)?;
    std::fs::create_dir_all(config)?;
    let tmp: PathBuf = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)
}

/// The method of `config` made concrete for the game in `root`.
pub fn method(
    config: &PlayConfig,
    root: &Path,
    installs: &[doctor_toolkit::install::Install],
) -> LaunchMethod {
    match &config.method {
        LaunchMethod::Auto => play::auto_method(root, installs),
        m => m.clone(),
    }
}

// ------------------------------------------------------------------ DXVK

/// [`play::dxvk::check`] for one launch method.
pub type DxvkCheck = Result<play::dxvk::Dxvk, play::dxvk::Problem>;

/// The text of why DXVK cannot be used.
pub fn dxvk_problem(problem: &play::dxvk::Problem) -> String {
    use play::dxvk::Problem;
    match problem {
        Problem::NotLinux => tr("dxvk.not_linux").to_owned(),
        Problem::ProtonHasIt => tr("dxvk.proton").to_owned(),
        Problem::Method(kind) => trf("dxvk.method", &[("method", kind)]),
        Problem::NoDxvk => tr("dxvk.no_dxvk").to_owned(),
        Problem::NoLoader => tr("dxvk.no_loader").to_owned(),
        Problem::NoDriver { software_only } => {
            let mut text = tr("dxvk.no_driver").to_owned();
            if *software_only {
                text += tr("dxvk.software_only");
            }
            text
        }
        Problem::Occupied(path) => trf("dxvk.occupied", &[("file", &path.display().to_string())]),
    }
}

/// The state of DXVK under its check box; returns whether it can be used.
pub fn dxvk_ui(ui: &mut egui::Ui, enabled: bool, check: &DxvkCheck) -> bool {
    match check {
        Ok(found) => {
            if enabled {
                ui.label(
                    RichText::new(trf("dxvk.found", &[("source", &found.source)]))
                        .weak()
                        .small(),
                );
            }
            true
        }
        Err(problem) => {
            if enabled {
                ui.colored_label(crate::WARN, dxvk_problem(problem));
            }
            false
        }
    }
}

/// Takes the DXVK variables of [`play::dxvk::apply_env`] out of a plan
/// again (DXVK could not be installed after all).
pub fn remove_dxvk_env(plan: &mut LaunchPlan) {
    plan.env.retain(|(k, _)| !k.starts_with("DXVK_"));
    for (key, value) in &mut plan.env {
        if key == "WINEDLLOVERRIDES" {
            let kept: Vec<String> = value
                .to_string_lossy()
                .split(';')
                .filter(|p| !p.is_empty() && *p != "d3d9=n,b")
                .map(str::to_owned)
                .collect();
            *value = kept.join(";").into();
        }
    }
    plan.env
        .retain(|(k, v)| !(k == "WINEDLLOVERRIDES" && v.is_empty()));
}

// -------------------------------------------------------------- graphics

/// The section "Graphics" of the launch settings (stage 59).
pub fn graphics_ui(
    ui: &mut egui::Ui,
    g: &mut GraphicsOptions,
    method: &LaunchMethod,
    root: &Path,
    dxvk: bool,
) {
    ui.strong(tr("graphics.title"));
    ui.checkbox(&mut g.module, tr("graphics.module"))
        .on_hover_text(tr("graphics.module_hint"));
    if g.module {
        ui.indent("graphics_module", |ui| {
            if play::render::permanent(root) {
                ui.label(RichText::new(tr("graphics.permanent")).weak());
            } else if play::render::module_binary().is_none() {
                ui.colored_label(crate::WARN, tr("graphics.not_built"));
            }
            ui.checkbox(&mut g.time_of_day, tr("graphics.time_of_day"))
                .on_hover_text(tr("graphics.time_of_day_hint"));
            ui.checkbox(&mut g.vsync_one, tr("graphics.vsync_one"));
            ui.checkbox(&mut g.limiter_sleep, tr("graphics.limiter_sleep"));
            ui.horizontal(|ui| {
                ui.label(tr("graphics.fps"))
                    .on_hover_text(tr("graphics.fps_hint"));
                ui.add(egui::DragValue::new(&mut g.fps).range(0..=1000).speed(1.0));
            });
            ui.horizontal(|ui| {
                ui.label(tr("graphics.fov"))
                    .on_hover_text(tr("graphics.fov_hint"));
                ui.add(egui::DragValue::new(&mut g.fov).range(0..=120).speed(1.0));
            });
            ui.checkbox(&mut g.fxaa, tr("graphics.fxaa"))
                .on_hover_text(tr("graphics.post_hint"));
            ui.horizontal(|ui| {
                ui.label(tr("graphics.sharpen"))
                    .on_hover_text(tr("graphics.post_hint"));
                ui.add(egui::Slider::new(&mut g.sharpen, 0..=100));
            });
            ui.checkbox(&mut g.draw_merge, tr("graphics.draw_merge"))
                .on_hover_text(tr("graphics.draw_merge_hint"));
        });
    }
    ui.checkbox(&mut g.laa, tr("graphics.laa"))
        .on_hover_text(tr("graphics.laa_hint"));
    if g.laa
        && let Some(why) = play::render::laa_problem(method)
    {
        ui.colored_label(
            crate::WARN,
            trf("graphics.laa_problem", &[("reason", &why)]),
        );
    }
    if dxvk {
        ui.horizontal(|ui| {
            ui.label(tr("graphics.vulkan_driver"))
                .on_hover_text(tr("graphics.vulkan_driver_hint"));
            let name = |v: &str| match v {
                "" | "all" => tr("graphics.vulkan_all").to_owned(),
                "auto" => tr("graphics.vulkan_auto").to_owned(),
                other => other.to_owned(),
            };
            egui::ComboBox::from_id_salt("play_vulkan_driver")
                .selected_text(name(&g.vulkan_driver))
                .show_ui(ui, |ui| {
                    for v in ["", "auto", "nvidia", "radeon", "intel"] {
                        ui.selectable_value(&mut g.vulkan_driver, v.to_owned(), name(v));
                    }
                });
        });
    }
}

// ----------------------------------------------------------------- window

/// The window "Launch settings" of Open Mod Manager: DXVK and the graphics
/// of the game, shared with the editor's Play options.
#[derive(Default)]
pub struct LaunchWindow {
    open: bool,
    root: Option<PathBuf>,
    shared: Shared,
    method: Option<LaunchMethod>,
    dxvk: Option<DxvkCheck>,
}

impl LaunchWindow {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Opens the window for the game in `root` with the values of the file.
    pub fn open(&mut self, config: Option<&Path>, root: &Path) {
        let play = config.map(|c| load(c, root)).unwrap_or_default();
        let installs = doctor_toolkit::install::detect(&crate::games::search_roots());
        let method = method(&play, root, &installs);
        self.dxvk = Some(play::dxvk::check(&method, root));
        self.method = Some(method);
        self.shared = Shared::of(&play);
        self.root = Some(root.to_path_buf());
        self.open = true;
    }

    /// Draws the window; saves into `config` what changed. Returns an error
    /// to show when saving failed.
    pub fn show(&mut self, ctx: &egui::Context, config: Option<&Path>) -> Option<String> {
        if !self.open {
            return None;
        }
        let (Some(root), Some(method)) = (self.root.clone(), self.method.clone()) else {
            self.open = false;
            return None;
        };
        let before = self.shared.clone();
        let mut open = true;
        egui::Window::new(tr("launch.title"))
            .open(&mut open)
            .default_width(520.0)
            .vscroll(true)
            .show(ctx, |ui| {
                ui.add(egui::Label::new(RichText::new(tr("launch.shared")).weak().small()).wrap());
                ui.separator();
                ui.strong(tr("dxvk.title"));
                ui.checkbox(&mut self.shared.dxvk, tr("dxvk.enabled"))
                    .on_hover_text(tr("dxvk.enabled_hint"));
                if let Some(check) = &self.dxvk {
                    dxvk_ui(ui, self.shared.dxvk, check);
                }
                ui.label(RichText::new(tr("graphics.dxvk_note")).weak().small());
                ui.separator();
                graphics_ui(
                    ui,
                    &mut self.shared.graphics,
                    &method,
                    &root,
                    self.shared.dxvk,
                );
                ui.separator();
                ui.add(egui::Label::new(RichText::new(tr("launch.when")).weak().small()).wrap());
            });
        if !open {
            self.open = false;
        }
        if self.shared != before
            && let Some(dir) = config
            && let Err(error) = save_shared(dir, &root, &self.shared)
        {
            return Some(error.to_string());
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shared_values_keep_the_rest_of_the_editors_settings() {
        let dir = tempfile::tempdir().unwrap();
        let root = Path::new("/games/gf2");
        std::fs::write(
            dir.path().join(SETTINGS_FILE),
            "theme = \"light\"\n\n[games.\"/games/gf2\"]\nin_tab = false\n\n\
             [games.\"/games/gf2\".play]\napply_before = true\n\n\
             [games.\"/games/gf2\".play.method]\nmethod = \"direct\"\n",
        )
        .unwrap();
        assert_eq!(load_shared(dir.path(), root), Some(Shared::default()));
        let shared = Shared {
            dxvk: true,
            graphics: GraphicsOptions {
                laa: true,
                fps: 60,
                ..GraphicsOptions::default()
            },
        };
        save_shared(dir.path(), root, &shared).unwrap();
        assert_eq!(load_shared(dir.path(), root), Some(shared.clone()));
        let play = load(dir.path(), root);
        assert!(play.apply_before, "the editor's other values stay");
        assert_eq!(play.method, LaunchMethod::Direct);
        let text = std::fs::read_to_string(dir.path().join(SETTINGS_FILE)).unwrap();
        assert!(text.contains("theme = \"light\""), "{text}");
        assert!(text.contains("in_tab = false"), "{text}");
        // Back to the defaults: the keys go away.
        save_shared(dir.path(), root, &Shared::default()).unwrap();
        let text = std::fs::read_to_string(dir.path().join(SETTINGS_FILE)).unwrap();
        assert!(
            !text.contains("graphics") && !text.contains("dxvk"),
            "{text}"
        );
        assert!(load(dir.path(), root).apply_before);
    }

    #[test]
    fn a_game_without_settings_gets_them() {
        let dir = tempfile::tempdir().unwrap();
        let root = Path::new("/games/other");
        assert_eq!(load_shared(dir.path(), root), None);
        let shared = Shared {
            dxvk: true,
            graphics: GraphicsOptions::default(),
        };
        save_shared(dir.path(), root, &shared).unwrap();
        assert_eq!(load_shared(dir.path(), root), Some(shared));
    }

    #[test]
    fn a_failed_dxvk_install_leaves_the_asi_loader_override() {
        let root = Path::new("/game");
        let mut plan = LaunchPlan {
            program: "wine".into(),
            args: vec![],
            env: vec![("WINEDLLOVERRIDES".into(), "dinput8=n,b".into())],
            dir: root.into(),
            detached: false,
        };
        play::dxvk::apply_env(&mut plan, root);
        remove_dxvk_env(&mut plan);
        assert_eq!(
            plan.env,
            [("WINEDLLOVERRIDES".to_owned(), "dinput8=n,b".into())]
        );
        // Without the loader nothing is left.
        let mut plan = LaunchPlan {
            env: vec![],
            ..plan
        };
        play::dxvk::apply_env(&mut plan, root);
        remove_dxvk_env(&mut plan);
        if std::env::var_os("WINEDLLOVERRIDES").is_none() {
            assert!(plan.env.is_empty(), "{:?}", plan.env);
        }
    }
}
