//! A cut-scene in the level view (stage 62), driven without a window on the
//! miniature game with its cut-scene (`doctor_toolkit::testing::
//! write_cutscene`: one shot of two seconds, a camera moving from x = 0 to
//! x = 10 at z = 8, an actor walking from x = 0 to x = 4 between 1 and 2 s):
//! opening it, the gizmo keying the camera with undo, a key from the view,
//! the play head shared with the cut-scene tab and the Timeline, Play
//! moving the actor, the Preview panel, deleting keys.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use doctor_toolkit::cutscene::edit::{Target, key_times};
use doctor_toolkit::game::GameInstance;
use doctor_toolkit::testing as mini;
use egui::{Event, Modifiers, PointerButton, Pos2};

use super::BrowserApp;
use super::Command;
use super::details::tests::frame;
use super::dock::Pane;
use crate::level_view::cine::{CineAction, Item};
use crate::level_view::{GizmoMode, LevelView};
use crate::nav::Location;
use crate::settings::{Settings, Store};

struct Fixture {
    dir: PathBuf,
    ctx: egui::Context,
    app: BrowserApp,
    /// The level tab.
    id: u64,
    /// The shot.
    shot: Location,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The editor with the miniature game, its cut-scene opened in the level
/// view and the zones loaded.
fn fixture(name: &str) -> Fixture {
    let dir = std::env::temp_dir().join(format!("dt-cine-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    mini::write_game(&dir).unwrap();
    mini::write_cutscene(&dir).unwrap();
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
    let shot = Location {
        stream: game.find_stream(mini::CUTSCENE).unwrap(),
        resource: 0,
    };
    app.game = Some(game);
    app.index = Some(Arc::new(index));
    app.cine_open = Some(shot);
    frame(&ctx, &mut app);
    frame(&ctx, &mut app);
    let id = app
        .cine
        .as_ref()
        .and_then(|c| c.level)
        .expect("a level tab");
    let started = Instant::now();
    loop {
        frame(&ctx, &mut app);
        let level = &app.levels[&id];
        if level.loaded() && !level.busy() {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the zones load"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    frame(&ctx, &mut app);
    frame(&ctx, &mut app);
    Fixture {
        dir,
        ctx,
        app,
        id,
        shot,
    }
}

fn level(f: &Fixture) -> &LevelView {
    &f.app.levels[&f.id]
}

fn seek(f: &mut Fixture, t: f32) {
    f.app.cine_action(f.id, CineAction::Seek(t));
    frame(&f.ctx, &mut f.app);
}

/// The camera of the cut-scene as the session has it, at `t`.
fn camera_at(f: &Fixture, t: f32) -> [f32; 6] {
    f.app
        .cine
        .as_ref()
        .unwrap()
        .cutscene
        .camera_at(t)
        .unwrap()
        .values
}

fn head(f: &Fixture) -> usize {
    f.app.workspace.as_ref().map_or(0, |ws| ws.head())
}

fn undo(f: &mut Fixture) {
    f.app.pending.push(Command::Undo);
    frame(&f.ctx, &mut f.app);
    frame(&f.ctx, &mut f.app);
}

fn camera_keys(f: &Fixture) -> Vec<f32> {
    let c = &f.app.cine.as_ref().unwrap().cutscene;
    key_times(c.shots[0].block(), Target::Camera)
}

/// One frame with these input events.
fn frame_with(f: &mut Fixture, events: Vec<Event>) {
    use eframe::App as _;
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            Pos2::ZERO,
            egui::vec2(1280.0, 800.0),
        )),
        events,
        ..Default::default()
    };
    let mut eframe_frame = eframe::Frame::_new_kittest();
    let mut output = f.ctx.run_ui(input, |ui| f.app.ui(ui, &mut eframe_frame));
    output.textures_delta.clear();
}

fn drag(f: &mut Fixture, a: Pos2, b: Pos2) {
    let button = |pos, pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    frame_with(f, vec![Event::PointerMoved(a)]);
    frame_with(f, vec![button(a, true)]);
    let steps = ((b - a).length() / 3.0).ceil().max(1.0) as usize;
    for i in 1..=steps {
        frame_with(
            f,
            vec![Event::PointerMoved(a + (b - a) * (i as f32 / steps as f32))],
        );
    }
    frame_with(f, vec![button(b, false)]);
    frame(&f.ctx, &mut f.app);
}

#[test]
fn a_cutscene_opens_in_the_level_view_with_its_panels() {
    let mut f = fixture("open");
    let cine = f.app.cine.as_ref().unwrap();
    assert_eq!(cine.cutscene.name, "igc99");
    let layer = level(&f)
        .cine
        .as_ref()
        .expect("the cut-scene over the level");
    assert!(layer.status.is_empty(), "the zones are chosen");
    // The zones of the cut-scene: the folder stream at least.
    let streams = &level(&f).level().unwrap().streams;
    assert!(streams.contains(&f.shot.stream), "{streams:?}");
    // The Cinematics layout: the Timeline and the Preview in the dock.
    let tree = f.app.dock.as_ref().unwrap();
    for pane in [Pane::Timeline, Pane::CutscenePreview, Pane::Details] {
        assert!(
            super::dock::find(tree, |p| *p == pane).is_some(),
            "{pane:?}"
        );
    }
    // The camera and the actor are items of the shot at the play head.
    seek(&mut f, 1.0);
    let items: Vec<Item> = level(&f)
        .cine
        .as_ref()
        .unwrap()
        .items()
        .into_iter()
        .map(|(i, _)| i)
        .collect();
    assert!(items.contains(&Item::Camera));
    assert!(items.contains(&Item::Actor(mini::CUTSCENE_ACTOR)));
    // Selecting the camera: Details shows it, the level's objects are not
    // selected.
    f.app
        .levels
        .get_mut(&f.id)
        .unwrap()
        .cine_select(Some(Item::Camera));
    frame(&f.ctx, &mut f.app);
    frame(&f.ctx, &mut f.app);
    assert_eq!(level(&f).primary(), None);
    assert_eq!(f.app.inspect, super::details::Focus::Level(f.id));
    let shown = f.app.shown(&f.app.inspectors.main, None);
    assert!(
        matches!(shown, super::details::Shown::Cutscene(id) if id == f.id),
        "{shown:?}"
    );
    // The Outliner's branch, the Timeline and the Preview draw.
    f.app.cine_select_for_tests();
    // Closing takes it off.
    f.app.cine_action(f.id, CineAction::Close);
    frame(&f.ctx, &mut f.app);
    assert!(f.app.cine.is_none() && level(&f).cine.is_none());
}

#[test]
fn the_gizmo_keys_the_camera_and_undo_takes_it_back() {
    let mut f = fixture("gizmo");
    seek(&mut f, 1.0);
    {
        let l = f.app.levels.get_mut(&f.id).unwrap();
        l.set_gizmo_mode(GizmoMode::Move);
        l.cine_select(Some(Item::Camera));
        // F: the view flies to it.
        l.focus_selection();
    }
    frame(&f.ctx, &mut f.app);
    frame(&f.ctx, &mut f.app);
    assert_eq!(camera_keys(&f), vec![0.0, 2.0]);
    let before = head(&f);
    let (a, b) = (
        level(&f)
            .gizmo_point(0, 0.6)
            .expect("the gizmo on the camera"),
        level(&f).gizmo_point(0, 1.6).unwrap(),
    );
    drag(&mut f, a, b);
    frame(&f.ctx, &mut f.app);
    assert_eq!(head(&f), before + 1, "one operation");
    let x = camera_at(&f, 1.0)[0];
    assert!(x > 5.05, "moved along x: {x}");
    assert_eq!(camera_at(&f, 1.0)[2], 8.0, "z stays");
    assert_eq!(
        camera_keys(&f),
        vec![0.0, 1.0, 2.0],
        "a key at the play head"
    );
    // On the key now: another drag changes it, no new key.
    let (a, b) = (
        level(&f).gizmo_point(0, 0.6).unwrap(),
        level(&f).gizmo_point(0, 1.2).unwrap(),
    );
    drag(&mut f, a, b);
    assert_eq!(head(&f), before + 2);
    assert_eq!(camera_keys(&f), vec![0.0, 1.0, 2.0]);
    assert!(camera_at(&f, 1.0)[0] > x);
    // Undo twice: back to the game's keys.
    undo(&mut f);
    undo(&mut f);
    assert_eq!(camera_keys(&f), vec![0.0, 2.0]);
    assert!((camera_at(&f, 1.0)[0] - 5.0).abs() < 1e-4);
    // The actor: moved by its gizmo (a key of its own path).
    f.app
        .levels
        .get_mut(&f.id)
        .unwrap()
        .cine_select(Some(Item::Actor(mini::CUTSCENE_ACTOR)));
    seek(&mut f, 1.5);
    f.app.levels.get_mut(&f.id).unwrap().focus_selection();
    frame(&f.ctx, &mut f.app);
    let (a, b) = (
        level(&f)
            .gizmo_point(2, 0.6)
            .expect("the gizmo on the actor"),
        level(&f).gizmo_point(2, 1.6).unwrap(),
    );
    drag(&mut f, a, b);
    let c = &f.app.cine.as_ref().unwrap().cutscene;
    let keys = key_times(
        c.shots[0].block(),
        Target::Actor(doctor_toolkit::resource::trinity::Guid(
            mini::CUTSCENE_ACTOR,
        )),
    );
    assert_eq!(keys, vec![1.0, 1.5, 2.0]);
    let at = doctor_toolkit::cutscene::actor_state(
        &c.shots[0],
        &doctor_toolkit::resource::trinity::Guid(mini::CUTSCENE_ACTOR),
        1.5,
    )
    .unwrap();
    assert!(at.position[2] > 0.05, "moved along z: {:?}", at.position);
    // Delete removes that key (Delete in the view does the same).
    f.app.cine_action(
        f.id,
        CineAction::DeleteKey(Item::Actor(mini::CUTSCENE_ACTOR)),
    );
    frame(&f.ctx, &mut f.app);
    let c = &f.app.cine.as_ref().unwrap().cutscene;
    let keys = key_times(
        c.shots[0].block(),
        Target::Actor(doctor_toolkit::resource::trinity::Guid(
            mini::CUTSCENE_ACTOR,
        )),
    );
    assert_eq!(keys, vec![1.0, 2.0]);
}

#[test]
fn a_camera_key_from_the_view_and_the_lens_in_details() {
    let mut f = fixture("from-view");
    seek(&mut f, 0.5);
    // The view's camera somewhere else, looking along −X (its Z axis is +X).
    let matrix = [
        0.0, 0.0, -1.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        1.0, 0.0, 0.0, 0.0, //
        -3.0, 2.5, 4.0, 1.0,
    ];
    let before = head(&f);
    f.app
        .cine_action(f.id, CineAction::KeyFromView { matrix, fov: 30.0 });
    frame(&f.ctx, &mut f.app);
    assert_eq!(head(&f), before + 1, "one operation");
    let c = f
        .app
        .cine
        .as_ref()
        .unwrap()
        .cutscene
        .camera_at(0.5)
        .unwrap();
    assert!((c.values[0] + 3.0).abs() < 1e-3, "{:?}", c.values);
    assert!((c.values[1] - 2.5).abs() < 1e-3);
    assert!((c.values[2] - 4.0).abs() < 1e-3);
    let m = c.matrix();
    // The camera's Z axis is the view's (+X): it looks the same way.
    assert!((m[8] - 1.0).abs() < 1e-3, "{m:?}");
    assert!(
        (c.vertical_fov() - 30.0).abs() < 0.05,
        "{}",
        c.vertical_fov()
    );
    // From the level view itself: its camera.
    let (view, fov) = level(&f).view_camera();
    f.app.pending.push(Command::CutsceneKeyFromView);
    frame(&f.ctx, &mut f.app);
    let c = f
        .app
        .cine
        .as_ref()
        .unwrap()
        .cutscene
        .camera_at(0.5)
        .unwrap();
    assert!((c.values[0] - view[12]).abs() < 1e-3);
    assert!((c.vertical_fov() - fov).abs() < 0.05);
    // The focal length in millimetres.
    f.app.cine_action(f.id, CineAction::Focal(70.0));
    frame(&f.ctx, &mut f.app);
    let c = f
        .app
        .cine
        .as_ref()
        .unwrap()
        .cutscene
        .camera_at(0.5)
        .unwrap();
    assert!((c.fov - 70.0).abs() < 1e-3);
    assert!(
        (c.vertical_fov() - 20.57).abs() < 0.05,
        "{}",
        c.vertical_fov()
    );
}

#[test]
fn the_play_head_is_shared_and_play_moves_the_actor() {
    let mut f = fixture("play");
    // The tab of the shot follows the play head of the level view.
    let tab = f.app.open_resource_tab(f.shot);
    frame(&f.ctx, &mut f.app);
    seek(&mut f, 1.25);
    frame(&f.ctx, &mut f.app);
    let playhead = |f: &Fixture| {
        let doc = &f.app.docs[&tab];
        doc.views
            .iter()
            .find_map(|(_, v)| crate::cutscene_view::playhead_of(v.state()?))
    };
    assert_eq!(playhead(&f), Some(1.25));
    // A tab moving it moves the level view's.
    {
        let mut cursor = f.app.cine_cursor.lock().unwrap();
        let c = cursor.as_mut().unwrap();
        c.time = 0.5;
        c.moved = true;
    }
    frame(&f.ctx, &mut f.app);
    assert_eq!(f.app.cine.as_ref().unwrap().timeline.playhead, 0.5);
    assert_eq!(level(&f).cine.as_ref().unwrap().time, 0.5);
    // Play: the play head and the actor move.
    seek(&mut f, 1.0);
    let actor = Item::Actor(mini::CUTSCENE_ACTOR);
    let x0 = level(&f).cine.as_ref().unwrap().matrix(actor).unwrap()[12];
    f.app.pending.push(Command::CutscenePlay);
    frame(&f.ctx, &mut f.app);
    assert!(f.app.cine.as_ref().unwrap().playing);
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(400) {
        std::thread::sleep(Duration::from_millis(20));
        frame(&f.ctx, &mut f.app);
    }
    let t = f.app.cine.as_ref().unwrap().timeline.playhead;
    assert!(t > 1.05, "the play head moves: {t}");
    let x1 = level(&f).cine.as_ref().unwrap().matrix(actor).unwrap()[12];
    assert!(x1 > x0 + 0.1, "the actor walks: {x0} → {x1}");
    assert_eq!(playhead(&f), Some(level(&f).cine.as_ref().unwrap().time));
    // Pause from the tab's side.
    f.app.cine_cursor.lock().unwrap().as_mut().unwrap().toggle = true;
    frame(&f.ctx, &mut f.app);
    assert!(!f.app.cine.as_ref().unwrap().playing);
}

#[test]
fn through_the_shot_camera_and_the_preview_panel() {
    let mut f = fixture("through");
    seek(&mut f, 1.0);
    f.app
        .levels
        .get_mut(&f.id)
        .unwrap()
        .cine
        .as_mut()
        .unwrap()
        .through_shot = true;
    frame(&f.ctx, &mut f.app);
    frame(&f.ctx, &mut f.app);
    let (m, fov) = level(&f).view_camera();
    assert!(
        (m[12] - 5.0).abs() < 1e-3 && (m[14] - 8.0).abs() < 1e-3,
        "{m:?}"
    );
    // 50 mm: 28.5° high.
    assert!((fov - 28.50).abs() < 0.05, "{fov}");
    // The Preview panel: a picture of the shot camera (no GPU here: its
    // message), made once and kept.
    f.app
        .dock_ops
        .push(super::dock::DockOp::ShowPanel(Pane::CutscenePreview));
    frame(&f.ctx, &mut f.app);
    frame(&f.ctx, &mut f.app);
    assert!(f.app.cine.as_ref().unwrap().has_preview());
}

impl BrowserApp {
    /// Draws the Outliner, the Timeline and the Preview panels once each.
    fn cine_select_for_tests(&mut self) {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            self.outliner_pane(ui);
            self.timeline_pane(ui);
            self.cutscene_preview_pane(ui);
        });
        output.textures_delta.clear();
    }
}
