//! A model file placed in a level in one step, without a window, on the
//! town of the miniature game: the editor chooses the sample object, the
//! import and the new object are one operation, a preset does the same
//! with nobody asked.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::level::TYPE_METAMODEL;
use doctor_toolkit::resource::hkx::TYPE_HKX;
use doctor_toolkit::testing as mini;

use super::*;
use crate::settings::{Settings, Store};

struct Fixture {
    dir: PathBuf,
    ctx: egui::Context,
    app: BrowserApp,
    /// The level tab with the yard loaded.
    tab: u64,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
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

/// Draws frames until `done`.
#[track_caller]
fn frames_until(f: &mut Fixture, what: &str, done: impl Fn(&BrowserApp) -> bool) {
    let started = Instant::now();
    loop {
        frame(&f.ctx, &mut f.app);
        if done(&f.app) {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "{what}: {}",
            f.app.status
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    frame(&f.ctx, &mut f.app);
}

fn fixture(name: &str) -> Fixture {
    let dir = std::env::temp_dir().join(format!("dt-place-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    mini::write_game(&dir).unwrap();
    mini::write_town(&dir).unwrap();
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
    app.level_preset_used = true;
    let game = Arc::new(GameInstance::open(&dir).unwrap());
    let (index, errors) = game.build_index(|_| {});
    assert!(errors.is_empty(), "{errors:?}");
    app.game = Some(game);
    app.index = Some(Arc::new(index));
    app.open_workspace();
    app.open_zones_in_level(vec![mini::YARD.to_owned()]);
    frame(&ctx, &mut app);
    frame(&ctx, &mut app);
    let tab = *app.levels.keys().next().expect("a level tab");
    let mut f = Fixture { dir, ctx, app, tab };
    frames_until(&mut f, "the yard loads", move |app| {
        let level = &app.levels[&tab];
        level.loaded() && !level.busy()
    });
    f
}

fn model_file(dir: &Path, name: &str) -> PathBuf {
    let file = dir.join(name);
    std::fs::write(&file, mini::box_glb([1.0, 2.0, 1.0], mini::MATERIAL_NAME)).unwrap();
    file
}

fn added(app: &BrowserApp, type_id: u32) -> usize {
    app.workspace.as_ref().map_or(0, |ws| {
        ws.changes()
            .filter(|c| c.is_added() && c.key.type_id == type_id)
            .count()
    })
}

/// Places and models of the objects of the yard as the level view has them.
fn objects(f: &Fixture) -> Vec<([f32; 3], Option<u32>)> {
    let level = f.app.levels[&f.tab].level().expect("loaded");
    level
        .groups
        .iter()
        .flat_map(|g| &g.objects)
        .filter_map(|o| {
            let m = o.transform?;
            Some(([m[3][0], m[3][1], m[3][2]], o.model))
        })
        .collect()
}

fn head(app: &BrowserApp) -> usize {
    app.workspace.as_ref().map_or(0, |ws| ws.head())
}

#[test]
fn a_model_file_is_placed_in_one_step() {
    let mut f = fixture("window");
    let tab = f.tab;
    let before = objects(&f);
    assert_eq!(before.len(), 1, "the crate of the yard");
    let crate_model = before[0].1.expect("the crate shows a model");
    let file = model_file(&f.dir, "Gas Pump (01).glb");
    let at = [9.0, 0.0, 6.0];

    // The window: the editor chose the crate as the sample and worked the
    // import out; nothing is recorded yet.
    let samples = f.app.levels[&tab].place_samples(at);
    assert_eq!(samples.len(), 1, "{samples:?}");
    assert!(samples[0].collision, "the crate's model has collision");
    f.app.place_file_start(tab, file, at, true, None);
    assert!(f.app.place_file_open_for_test(), "{}", f.app.status);
    assert_eq!(head(&f.app), 0);

    f.app.place_file_confirm_for_test().unwrap();
    assert!(!f.app.place_file_open_for_test());
    assert_eq!(
        head(&f.app),
        1,
        "the import and the object are one operation"
    );
    assert_eq!(added(&f.app, TYPE_METAMODEL), 1);
    // The crate's model has collision: the new one gets a hull of its own.
    assert_eq!(added(&f.app, TYPE_HKX), 1);
    assert!(f.app.status.contains("gas_pump_01"), "{}", f.app.status);

    frames_until(&mut f, "the yard shows the new object", move |app| {
        let level = &app.levels[&tab];
        level.loaded() && !level.busy() && level.level().is_some_and(|l| l.guids().len() == 2)
    });
    let after = objects(&f);
    assert_eq!(after.len(), 2);
    let new = after
        .iter()
        .find(|(p, _)| (0..3).all(|i| (p[i] - at[i]).abs() < 1e-3))
        .expect("the object stands where the file was placed");
    assert!(new.1.is_some() && new.1 != Some(crate_model), "{new:?}");

    // One undo takes both back.
    f.app.undo();
    assert_eq!(added(&f.app, TYPE_METAMODEL), 0);
    frames_until(&mut f, "the object is gone", move |app| {
        let level = &app.levels[&tab];
        level.loaded() && !level.busy() && level.level().is_some_and(|l| l.guids().len() == 1)
    });
}

#[test]
fn a_preset_places_a_file_with_nobody_asked() {
    let mut f = fixture("preset");
    let tab = f.tab;
    let file = model_file(&f.dir, "box.glb");
    let action = format!(
        "file:{}:at=9/0/6:name=doctor_pump:collision=none",
        file.display()
    );
    f.app
        .levels
        .get_mut(&tab)
        .unwrap()
        .queue_preset_actions(&[action.as_str()]);
    frames_until(&mut f, "the preset places the file", |app| head(app) == 1);
    assert!(!f.app.place_file_open_for_test());
    assert_eq!(added(&f.app, TYPE_METAMODEL), 1);
    assert_eq!(added(&f.app, TYPE_HKX), 0, "collision=none");
    assert!(f.app.status.contains("doctor_pump"), "{}", f.app.status);
    assert!(f.app.preset_error().is_none(), "{:?}", f.app.preset_error());

    // A file that cannot be read stops a run without input with an error.
    let missing = f.dir.join("missing.glb");
    let action = format!("file:{}", missing.display());
    f.app
        .levels
        .get_mut(&tab)
        .unwrap()
        .queue_preset_actions(&[action.as_str()]);
    frames_until(&mut f, "the error", |app| app.preset_error().is_some());
    assert_eq!(head(&f.app), 1);
}
