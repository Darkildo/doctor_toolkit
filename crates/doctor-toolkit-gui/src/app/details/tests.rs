//! The level panels driven without a window on the miniature game of
//! `doctor_toolkit::testing`, with a SimGroup of three placed objects in the
//! workspace: the Outliner's tree and selection, the eye and the lock, the
//! Details edits with undo, and the panels drawn in the Level Editor layout.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::level::Location;
use doctor_toolkit::resource::simgroup::{
    ATTR_LOAD_MATRIX, CLASS_ENTITY, CLASS_SYSTEM_COMMANDS, Guid, SimGroup, Value,
};
use doctor_toolkit::resource::simgroup_edit::Transform;
use doctor_toolkit::testing as mini;

use super::super::dock::Pane;
use super::super::{BrowserApp, Command};
use super::Focus;
use crate::level_view::{Mark, Node, ObjRef};
use crate::settings::{Settings, Store};

struct Fixture {
    dir: PathBuf,
    ctx: egui::Context,
    app: BrowserApp,
    /// The level tab.
    id: u64,
    group: Location,
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

/// Three objects of two classes, placed at x = 0, 10, 20.
fn placed_group() -> SimGroup {
    let mut g = SimGroup::empty();
    for (n, class) in [0x1111_1111u32, 0x2222_2222, 0x1111_1111]
        .into_iter()
        .enumerate()
    {
        let mut guid = [n as u8 + 1; 16];
        guid[8..].copy_from_slice(b"PANELS53");
        let o = g
            .push_object(
                class,
                Guid(guid),
                &[(CLASS_SYSTEM_COMMANDS, 2), (CLASS_ENTITY, 2)],
            )
            .unwrap();
        let mut m = doctor_toolkit::level::IDENTITY;
        m[3][0] = n as f32 * 10.0;
        g.set_class_attr(
            o,
            CLASS_SYSTEM_COMMANDS,
            ATTR_LOAD_MATRIX,
            Some(Value::Matrix(m)),
        )
        .unwrap();
    }
    SimGroup::parse(&g.encode()).unwrap()
}

/// An editor without a window, the level editor open with the zone of the
/// miniature game loaded.
fn fixture(name: &str) -> Fixture {
    let dir = std::env::temp_dir().join(format!("dt-panels-{name}-{}", std::process::id()));
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
    app.level_preset_used = true;
    let game = Arc::new(GameInstance::open(&dir).unwrap());
    let (index, errors) = game.build_index(|_| {});
    assert!(errors.is_empty(), "{errors:?}");
    let group = Location {
        stream: game.find_stream(mini::LEVEL).unwrap(),
        resource: mini::SIMGROUP,
    };
    app.game = Some(game.clone());
    app.index = Some(Arc::new(index));
    app.workspace_mut()
        .unwrap()
        .set_resource(
            &game,
            group.stream,
            group.resource,
            placed_group().encode(),
            "three objects",
        )
        .unwrap();
    app.pending.push(Command::LevelEditor);
    frame(&ctx, &mut app);
    frame(&ctx, &mut app);
    let id = *app.levels.keys().next().expect("a level tab");
    app.levels.get_mut(&id).unwrap().show_object(group, 0);
    let started = Instant::now();
    loop {
        frame(&ctx, &mut app);
        let level = &app.levels[&id];
        if level.loaded() && !level.busy() {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the zone loads"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    frame(&ctx, &mut app);
    Fixture {
        dir,
        ctx,
        app,
        id,
        group,
    }
}

fn r(object: usize) -> ObjRef {
    ObjRef { group: 0, object }
}

#[test]
fn outliner_follows_the_level_and_its_selection() {
    let mut f = fixture("outliner");
    let (app, id) = (&mut f.app, f.id);
    assert_eq!(app.panel_level, Some(id));
    let level = &app.levels[&id];
    assert_eq!(level.primary(), Some(r(0)), "show_object selected it");
    // The objects the workspace changed: all three are new to the game's
    // SimGroup (other GUIDs).
    assert_eq!(level.mark(r(1)), Some(Mark::New));
    assert_eq!(app.inspect, Focus::Level(id), "Details follows the level");
    // The Outliner unfolded the zone and class of the selected object.
    let outline = app.outliners.get_mut(&id).expect("drawn");
    let rows = outline.rows().to_vec();
    assert!(rows.len() >= 4, "{rows:?}");
    assert!(
        rows.iter()
            .any(|&row| outline.node(row) == Node::Object(r(0)))
    );
    // Search by number.
    outline.search = "#1".into();
    let found: Vec<Node> = outline
        .rows()
        .to_vec()
        .into_iter()
        .map(|row| outline.node(row))
        .filter(|n| matches!(n, Node::Object(_)))
        .collect();
    assert_eq!(found, [Node::Object(r(1))]);
    outline.search.clear();

    // A selection of several objects; a resource selected later takes
    // Details over, a level selection takes it back.
    let level = app.levels.get_mut(&id).unwrap();
    level.set_selection(Some(r(2)), [r(0), r(2)].into());
    frame(&f.ctx, &mut f.app);
    assert_eq!(f.app.levels[&id].selection().len(), 2);
    f.app.inspect = Focus::Resource;
    f.app
        .levels
        .get_mut(&id)
        .unwrap()
        .set_selection(Some(r(1)), [r(1)].into());
    frame(&f.ctx, &mut f.app);
    assert_eq!(f.app.inspect, Focus::Level(id));
}

#[test]
fn eye_and_lock_work_by_zone_class_and_object() {
    let mut f = fixture("eye");
    let level = f.app.levels.get_mut(&f.id).unwrap();
    let stream = f.group.stream;
    level.set_hidden(Node::Object(r(1)), true);
    assert!(level.is_hidden(Node::Object(r(1))) && level.is_object_hidden(r(1)));
    assert!(!level.is_object_hidden(r(0)));
    level.set_hidden(Node::Class(stream, 0x1111_1111), true);
    assert!(level.is_object_hidden(r(0)) && level.is_object_hidden(r(2)));
    assert!(level.is_hidden(Node::Class(stream, 0x1111_1111)));
    level.set_hidden(Node::Class(stream, 0x1111_1111), false);
    level.set_hidden(Node::Zone(stream), true);
    assert!(level.is_hidden(Node::Zone(stream)) && level.is_object_hidden(r(0)));
    level.show_all();
    assert!(!level.is_object_hidden(r(1)));
    level.set_locked(Node::Zone(stream), true);
    assert!(level.is_object_locked(r(2)) && level.is_locked(Node::Zone(stream)));
    level.set_locked(Node::Zone(stream), false);
    level.set_locked(Node::Object(r(0)), true);
    assert!(level.is_object_locked(r(0)) && !level.is_object_locked(r(1)));
    // Removing object 0 moves the others up: they keep their eye and lock.
    level.set_locked(Node::Object(r(2)), true);
    level.set_hidden(Node::Object(r(2)), true);
    level.set_selection(Some(r(0)), [r(0)].into());
    let remove = level.edit_selection(true).unwrap();
    f.app.level_action(f.id, remove);
    let level = &f.app.levels[&f.id];
    assert_eq!(level.level().unwrap().groups[0].objects.len(), 2);
    assert!(level.is_object_locked(r(1)) && level.is_object_hidden(r(1)));
    assert!(!level.is_object_locked(r(0)));
}

#[test]
fn details_edits_are_recorded_and_undone() {
    let mut f = fixture("details");
    let id = f.id;
    let ws_head = |app: &BrowserApp| app.workspace.as_ref().unwrap().head();
    let before = ws_head(&f.app);
    // Transform: X + 2.5, a turn of 90° about Y.
    let level = &f.app.levels[&id];
    let stored = level.object(r(1)).unwrap().transform.unwrap();
    let mut values = Transform::from_matrix(&stored);
    assert!(super::object::transform_action(level, r(1), &values).is_none());
    values.position[0] += 2.5;
    values.rotation[1] = 90.0;
    let action = super::object::transform_action(level, r(1), &values).unwrap();
    f.app.level_action(id, action);
    assert_eq!(ws_head(&f.app), before + 1, "one operation");
    let moved = f.app.levels[&id].object(r(1)).unwrap().transform.unwrap();
    assert!((moved[3][0] - 12.5).abs() < 1e-4, "{moved:?}");
    assert!((Transform::from_matrix(&moved).rotation[1] - 90.0).abs() < 1e-3);
    assert_eq!(f.app.levels[&id].mark(r(1)), Some(Mark::New));
    // Undo puts it back.
    f.app.pending.push(Command::Undo);
    frame(&f.ctx, &mut f.app);
    let back = f.app.levels[&id].object(r(1)).unwrap().transform.unwrap();
    assert_eq!(back, stored);
    // Duplicating two objects is one operation, undone at once.
    let before = ws_head(&f.app);
    let level = f.app.levels.get_mut(&id).unwrap();
    level.set_selection(Some(r(0)), [r(0), r(2)].into());
    let action = level.edit_selection(false).unwrap();
    f.app.level_action(id, action);
    assert_eq!(ws_head(&f.app), before + 1);
    assert_eq!(
        f.app.levels[&id].level().unwrap().groups[0].objects.len(),
        5
    );
    f.app.pending.push(Command::Undo);
    frame(&f.ctx, &mut f.app);
    assert_eq!(
        f.app.levels[&id].level().unwrap().groups[0].objects.len(),
        3
    );
    // The model: Details makes the SimGroup with the new model id.
    let level = &f.app.levels[&id];
    let save = super::object::model_action(level, r(0), f.group, 0x1234_5678)
        .unwrap()
        .expect("a change");
    match save {
        crate::level_view::LevelAction::Save { data, .. } => {
            let g = SimGroup::parse(&data).unwrap();
            assert_eq!(g.model(&g.objects[0]), Some(0x1234_5678));
        }
        _ => panic!("a SimGroup edit"),
    }
}

#[test]
fn panels_draw_in_the_level_layout_with_a_copy_of_details() {
    let mut f = fixture("draw");
    for pane in [Pane::Outliner, Pane::Zones, Pane::Details] {
        assert!(
            f.app
                .dock
                .as_ref()
                .is_some_and(|t| super::super::dock::find(t, |p| *p == pane).is_some()),
            "{pane:?}"
        );
    }
    // Every panel in front once, with an object selected.
    for command in [Command::ToggleZones, Command::ToggleOutliner] {
        f.app.pending.push(command);
        frame(&f.ctx, &mut f.app);
        frame(&f.ctx, &mut f.app);
    }
    // Another Details, pinned to the object the first one shows.
    f.app.pending.push(Command::NewDetails);
    frame(&f.ctx, &mut f.app);
    frame(&f.ctx, &mut f.app);
    let (&copy, inspector) = f.app.inspectors.copies.iter().next().expect("a copy");
    assert!(matches!(
        inspector.pinned,
        Some(super::Pinned::Object { object: 0, .. })
    ));
    assert!(
        f.app
            .dock
            .as_ref()
            .is_some_and(|t| super::super::dock::find(t, |p| p.id() == Some(copy)).is_some())
    );
    // The selection moves on; the copy stays on object 0.
    let level = f.app.levels.get_mut(&f.id).unwrap();
    level.set_selection(Some(r(2)), [r(2)].into());
    frame(&f.ctx, &mut f.app);
    let main = f.app.shown(&f.app.inspectors.main, None);
    let pinned = f.app.shown(&f.app.inspectors.copies[&copy], None);
    assert!(matches!(main, super::Shown::Object(_, o) if o == r(2)));
    assert!(matches!(pinned, super::Shown::Object(_, o) if o == r(0)));
}
