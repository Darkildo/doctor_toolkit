//! Presets and saved sets of zones (stage 113), without a window, on the
//! miniature game with its town: a preset sets the choice; a set saved in a
//! project is the project's and comes back with it, and without a project
//! it is in the settings of the game; the level tab opens with what was
//! loaded last, with the camera where it stood.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use doctor_toolkit::level::ZoneSet;
use doctor_toolkit::project::{EditorCamera, EditorState, PROJECT_FILE, ProjectConfig};
use doctor_toolkit::testing as mini;

use super::*;
use crate::settings::{Settings, Store};

fn editor(settings: Settings) -> (egui::Context, BrowserApp) {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = BrowserApp::new(&cc, None, None, None, settings, Store { dir: None });
    app.notes.log_to_file = false;
    app.level_preset_used = true;
    (ctx, app)
}

fn frame(ctx: &egui::Context, app: &mut BrowserApp) {
    use eframe::App as _;
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 800.0),
        )),
        ..Default::default()
    };
    let mut eframe_frame = eframe::Frame::_new_kittest();
    let mut output = ctx.run_ui(input, |ui| app.ui(ui, &mut eframe_frame));
    output.textures_delta.clear();
}

/// Draws frames until the game being opened is open.
#[track_caller]
fn wait_for_game(ctx: &egui::Context, app: &mut BrowserApp) {
    let started = Instant::now();
    while app.job.is_some() || app.game.is_none() {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the game did not open: {}",
            app.status
        );
        frame(ctx, app);
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// An editor with these settings on the game in `dir`, opened as the
/// window opens it.
fn open(dir: &Path, settings: Settings) -> (egui::Context, BrowserApp) {
    let (ctx, mut app) = editor(settings);
    app.open_game(dir.to_owned(), &ctx);
    wait_for_game(&ctx, &mut app);
    (ctx, app)
}

/// Opens a level tab; returns its id.
fn level_tab(ctx: &egui::Context, app: &mut BrowserApp) -> u64 {
    let before: Vec<u64> = app.levels.keys().copied().collect();
    let id = app.new_pane_id();
    app.dock_ops
        .push(dock::DockOp::Open(dock::Pane::Level { id }));
    app.dock_ops.push(dock::DockOp::Focus(id));
    frame(ctx, app);
    frame(ctx, app);
    assert!(app.levels.contains_key(&id) && !before.contains(&id));
    id
}

/// Draws frames until level tab `id` has its zones loaded.
#[track_caller]
fn wait(ctx: &egui::Context, app: &mut BrowserApp, id: u64) {
    let started = Instant::now();
    loop {
        frame(ctx, app);
        let level = &app.levels[&id];
        if level.loaded() && !level.busy() {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the zones load: {} (loaded {}, busy {}; {})",
            level.status_text(),
            level.loaded(),
            level.busy(),
            app.status
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    frame(ctx, app);
}

/// The Load button of the Zones panel of level tab `id`.
fn load(ctx: &egui::Context, app: &mut BrowserApp, id: u64) {
    app.levels
        .get_mut(&id)
        .unwrap()
        .load_chosen(app.workspace.as_ref());
    wait(ctx, app, id);
}

fn chosen(app: &BrowserApp, id: u64) -> Vec<String> {
    app.levels[&id].chosen_names()
}

struct Folders {
    base: PathBuf,
    game: PathBuf,
    project: PathBuf,
}

impl Drop for Folders {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn folders(name: &str) -> Folders {
    let base = std::env::temp_dir().join(format!("dt-zone-sets-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let folders = Folders {
        game: base.join("game"),
        project: base.join("Yard Mod"),
        base,
    };
    std::fs::create_dir_all(&folders.game).unwrap();
    mini::write_game(&folders.game).unwrap();
    mini::write_town(&folders.game).unwrap();
    folders
}

/// The town at near detail: its one zone (the root stream of the miniature
/// town has no objects, so it is not a zone).
const WHOLE_TOWN: [&str; 1] = [mini::YARD];

#[test]
fn a_preset_sets_the_choice() {
    let at = folders("preset");
    let (ctx, mut app) = open(&at.game, Settings::default());
    let id = level_tab(&ctx, &mut app);
    // Nothing was loaded before: the tab opens empty.
    assert!(chosen(&app, id).is_empty() && !app.levels[&id].loaded());

    let level = app.levels.get_mut(&id).unwrap();
    assert!(!level.choose_preset("gf2/nowhere", None, false, false));
    assert!(!level.choose_preset("gf2/town", Some("nowhere"), false, false));
    assert!(level.chosen_names().is_empty());
    // The level by its last component; the town has no districts, missions
    // or checkpoints, so the switches add nothing.
    assert!(level.choose_preset("town", None, true, true));
    assert_eq!(chosen(&app, id), WHOLE_TOWN);
    // The choice alone: nothing is loaded and nothing remembered.
    frame(&ctx, &mut app);
    assert!(!app.levels[&id].loaded());
    assert!(app.settings.game(&at.game).last_zones.is_empty());
    // The Zones panel draws its presets, its sets and the tree, and
    // drawing changes nothing.
    app.save_zone_set(ZoneSet {
        name: "yard".into(),
        streams: vec![mini::YARD.into()],
    });
    app.zone_sets_tick();
    let panel = egui::Context::default();
    for _ in 0..2 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(320.0, 900.0),
            )),
            ..Default::default()
        };
        let level = app.levels.get_mut(&id).unwrap();
        let mut output = panel.run_ui(input, |ui| level.zones_panel(ui, None));
        output.textures_delta.clear();
    }
    assert_eq!(chosen(&app, id), WHOLE_TOWN);
    assert!(
        app.levels
            .get_mut(&id)
            .unwrap()
            .take_zone_requests()
            .is_empty()
    );

    // Loaded: remembered by the preset, not by its streams.
    load(&ctx, &mut app, id);
    assert_eq!(app.settings.game(&at.game).last_zones, ["preset:gf2/town"]);
    assert_eq!(app.levels[&id].level().unwrap().streams.len(), 1);

    // Hands-free: a word of `DOCTOR_TOOLKIT_GUI_LEVEL` names the preset.
    app.open_zones_in_level(vec!["preset:town".into()]);
    frame(&ctx, &mut app);
    frame(&ctx, &mut app);
    let other = *app.levels.keys().find(|k| **k != id).expect("a level tab");
    wait(&ctx, &mut app, other);
    assert_eq!(chosen(&app, other), WHOLE_TOWN);
    // A second tab does not open with the last choice by itself.
    let third = level_tab(&ctx, &mut app);
    assert!(chosen(&app, third).is_empty());
}

#[test]
fn a_saved_set_is_the_projects_or_the_games() {
    let at = folders("sets");
    let (ctx, mut app) = open(&at.game, Settings::default());
    let id = level_tab(&ctx, &mut app);
    let save = |app: &mut BrowserApp, name: &str| {
        let level = app.levels.get_mut(&id).unwrap();
        assert!(level.choose_preset("gf2/town", None, false, false));
        level.save_set(name);
        app.zone_sets_tick();
    };

    // Without a project: in the settings of the game.
    save(&mut app, "the town");
    let town = ZoneSet {
        name: "the town".into(),
        streams: WHOLE_TOWN.map(str::to_owned).to_vec(),
    };
    assert_eq!(
        app.settings.game(&at.game).zone_sets,
        std::slice::from_ref(&town)
    );
    assert_eq!(app.zone_sets.sets, std::slice::from_ref(&town));
    assert_eq!(app.zone_sets.project, None);

    // With a project: in its `project.toml`, and the game's set is not
    // shown.
    app.new_document(&at.project, false, &ctx).unwrap();
    assert!(app.zone_sets.sets.is_empty());
    assert_eq!(app.zone_sets.project.as_deref(), Some("Yard Mod"));
    save(&mut app, "yard work");
    let text = std::fs::read_to_string(at.project.join(PROJECT_FILE)).unwrap();
    assert!(text.contains("[[zones]]\nname = \"yard work\""), "{text}");
    assert_eq!(app.settings.game(&at.game).zone_sets.len(), 1);
    assert_eq!(app.zone_sets.sets.len(), 1);
    // Saved again under the same name: replaced.
    save(&mut app, "yard work");
    assert_eq!(
        ProjectConfig::read(&at.project).unwrap().zones.len(),
        1,
        "a set of the same name is replaced"
    );

    // The project closed: the game's sets; opened again: its own, and the
    // set chooses its zones.
    app.close_document(&ctx);
    assert_eq!(app.zone_sets.sets, std::slice::from_ref(&town));
    frame(&ctx, &mut app);
    assert_eq!(app.levels[&id].saved_sets().sets, [town]);
    assert!(!app.levels.get_mut(&id).unwrap().choose_set("yard work"));
    app.open_document(at.project.clone(), &ctx);
    assert_eq!(app.zone_sets.sets.len(), 1);
    assert_eq!(app.zone_sets.sets[0].name, "yard work");
    frame(&ctx, &mut app);
    let level = app.levels.get_mut(&id).unwrap();
    level.choose_preset("gf2/level", None, false, false);
    assert_eq!(level.chosen_names(), [mini::LEVEL]);
    assert!(level.choose_set("yard work"));
    assert_eq!(level.chosen_names(), WHOLE_TOWN);
    assert_eq!(level.zones_note(), "");

    // A stream the game does not have any more is skipped and named.
    ProjectConfig::write_zones(
        &at.project,
        &[ZoneSet {
            name: "yard work".into(),
            streams: vec![mini::YARD.into(), "gf2/town/demolished".into()],
        }],
    )
    .unwrap();
    app.reload_zone_sets();
    let level = app.levels.get_mut(&id).unwrap();
    assert!(level.choose_set("yard work"));
    assert_eq!(level.chosen_names(), [mini::YARD]);
    assert!(
        level.zones_note().contains("gf2/town/demolished"),
        "{}",
        level.zones_note()
    );

    // Removed.
    app.remove_zone_set("yard work");
    assert!(app.zone_sets.sets.is_empty());
    assert!(ProjectConfig::read(&at.project).unwrap().zones.is_empty());
    assert!(
        !std::fs::read_to_string(at.project.join(PROJECT_FILE))
            .unwrap()
            .contains("zones")
    );
}

/// Two zones of two levels: no preset names them together.
const BOTH: [&str; 2] = [mini::LEVEL, mini::YARD];

#[test]
fn the_level_tab_opens_with_what_was_loaded_last() {
    let at = folders("last");
    let settings = {
        let (ctx, mut app) = open(&at.game, Settings::default());
        let id = level_tab(&ctx, &mut app);
        // A choice that is no preset is remembered by its streams.
        app.save_zone_set(ZoneSet {
            name: "both".into(),
            streams: BOTH.map(str::to_owned).to_vec(),
        });
        frame(&ctx, &mut app);
        assert!(app.levels.get_mut(&id).unwrap().choose_set("both"));
        load(&ctx, &mut app, id);
        assert_eq!(app.settings.game(&at.game).last_zones, BOTH);
        assert_eq!(app.settings.game(&at.game).last_camera, None);
        // The window closes: the camera is remembered too.
        app.remember_level_camera();
        let camera = app.settings.game(&at.game).last_camera;
        assert_eq!(camera, app.levels[&id].camera_state());
        assert!(camera.is_some());
        app.settings.clone()
    };

    // The next run: the tab made after the game is open loads them.
    let mut settings = settings;
    let camera = EditorCamera {
        eye: [3.0, 9.0, -4.0],
        yaw: 1.25,
        pitch: -0.5,
    };
    settings.game_mut(&at.game).last_camera = Some(camera);
    let (ctx, mut app) = open(&at.game, settings.clone());
    let id = level_tab(&ctx, &mut app);
    wait(&ctx, &mut app, id);
    assert_eq!(chosen(&app, id), BOTH);
    assert_eq!(app.levels[&id].level().unwrap().streams.len(), 2);
    assert_eq!(app.levels[&id].camera_state(), Some(camera));
    assert_eq!(app.levels[&id].zones_note(), "");
    // Restoring is not loading anew: the camera stays remembered.
    assert_eq!(app.settings.game(&at.game).last_camera, Some(camera));

    // A tab that is there before the game is open loads them when it is; a
    // stream that is gone is skipped and named.
    settings.game_mut(&at.game).last_zones =
        vec!["preset:gf2/town".into(), "gf2/town/demolished".into()];
    // The editor would open the last game by itself, before the tab.
    settings.reopen_last = false;
    let (ctx, mut app) = editor(settings);
    let id = level_tab(&ctx, &mut app);
    assert!(chosen(&app, id).is_empty());
    app.open_game(at.game.clone(), &ctx);
    wait_for_game(&ctx, &mut app);
    wait(&ctx, &mut app, id);
    assert_eq!(chosen(&app, id), WHOLE_TOWN);
    // Loaded again, the choice is remembered without what is gone.
    load(&ctx, &mut app, id);
    assert_eq!(app.settings.game(&at.game).last_zones, ["preset:gf2/town"]);
    assert!(
        app.levels[&id].zones_note().contains("gf2/town/demolished"),
        "{}",
        app.levels[&id].zones_note()
    );
}

#[test]
fn a_project_opens_with_its_last_zones() {
    let at = folders("project-last");
    let editor_file = at.project.join(".doctor/editor.json");
    {
        let (ctx, mut app) = open(&at.game, Settings::default());
        let id = level_tab(&ctx, &mut app);
        app.new_document(&at.project, false, &ctx).unwrap();
        assert!(!editor_file.exists());
        let level = app.levels.get_mut(&id).unwrap();
        assert!(level.choose_preset("gf2/town", None, false, false));
        load(&ctx, &mut app, id);
        // The project's, on this computer; not the game's.
        let state = EditorState::read(&at.project);
        assert_eq!(state.zones, ["preset:gf2/town"]);
        assert_eq!(state.camera, None);
        assert!(app.settings.game(&at.game).last_zones.is_empty());
        let ignored = std::fs::read_to_string(at.project.join(".gitignore")).unwrap();
        assert!(ignored.contains("/.doctor/editor.json"), "{ignored}");
        // Closing the project remembers the camera.
        let camera = app.levels[&id].camera_state();
        app.close_document(&ctx);
        assert_eq!(EditorState::read(&at.project).camera, camera);
        assert!(camera.is_some());
    }

    // An editor with no game open and a level tab: the project opens its
    // game, and the tab shows what the project had loaded.
    let camera = EditorCamera {
        eye: [-2.0, 7.0, 11.0],
        yaw: -0.75,
        pitch: -0.3,
    };
    let mut state = EditorState::read(&at.project);
    state.camera = Some(camera);
    state.write(&at.project).unwrap();
    let (ctx, mut app) = editor(Settings::default());
    let id = level_tab(&ctx, &mut app);
    app.open_document(at.project.clone(), &ctx);
    wait_for_game(&ctx, &mut app);
    assert_eq!(app.document_name(), Some("Yard Mod"));
    wait(&ctx, &mut app, id);
    assert_eq!(chosen(&app, id), WHOLE_TOWN);
    assert_eq!(app.levels[&id].camera_state(), Some(camera));

    // The project closed: the game alone had nothing loaded, and what is
    // shown stays. Opened again for the open game, an empty tab loads it.
    app.close_document(&ctx);
    assert!(app.levels[&id].loaded());
    let (ctx, mut app) = open(&at.game, Settings::default());
    let id = level_tab(&ctx, &mut app);
    assert!(chosen(&app, id).is_empty());
    app.open_document(at.project.clone(), &ctx);
    assert_eq!(app.document_name(), Some("Yard Mod"));
    wait(&ctx, &mut app, id);
    assert_eq!(chosen(&app, id), WHOLE_TOWN);
}
