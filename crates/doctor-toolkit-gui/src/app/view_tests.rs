//! The level view as in Unreal Engine (stage 54), driven without a window
//! by keys and the mouse on the miniature game of the panel tests (three
//! objects at x = 0, 10, 20): Q W E R and the right button, the frame
//! selection, Ctrl+D, Del and Alt + drag as one operation each with undo,
//! the gizmo on several objects, the context menu, Ctrl+A and Esc, End,
//! the History panel.

use egui::{Event, Key, Modifiers, PointerButton, Pos2, Vec2};

use super::Command;
use super::details::tests::{Fixture, fixture, frame, r};
use super::dock::{DockOp, Pane};
use crate::level_view::{ContextItem, GizmoMode, LevelAction, Lighting, Node, Projection};

/// Ctrl as the window system reports it off the Mac: ctrl and command.
const CTRL: Modifiers = Modifiers {
    ctrl: true,
    command: true,
    ..Modifiers::NONE
};

/// One frame with these input events and modifiers held.
fn frame_with(f: &mut Fixture, mut events: Vec<Event>, modifiers: Modifiers) {
    use eframe::App as _;
    events.insert(0, Event::ModifiersChanged(modifiers));
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

fn key(f: &mut Fixture, key: Key, modifiers: Modifiers) {
    let event = |pressed| Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers,
    };
    frame_with(f, vec![event(true)], modifiers);
    frame_with(f, vec![event(false)], modifiers);
}

fn moved(f: &mut Fixture, at: Pos2, modifiers: Modifiers) {
    frame_with(f, vec![Event::PointerMoved(at)], modifiers);
}

fn button(f: &mut Fixture, at: Pos2, button: PointerButton, pressed: bool, modifiers: Modifiers) {
    frame_with(
        f,
        vec![Event::PointerButton {
            pos: at,
            button,
            pressed,
            modifiers,
        }],
        modifiers,
    );
}

/// A left drag from `a` to `b` in small steps.
fn drag(f: &mut Fixture, a: Pos2, b: Pos2, modifiers: Modifiers) {
    moved(f, a, modifiers);
    button(f, a, PointerButton::Primary, true, modifiers);
    let steps = ((b - a).length() / 3.0).ceil().max(1.0) as usize;
    for i in 1..=steps {
        moved(f, a + (b - a) * (i as f32 / steps as f32), modifiers);
    }
    button(f, b, PointerButton::Primary, false, modifiers);
    frame(&f.ctx, &mut f.app);
}

/// The level view of the fixture, all three objects in sight.
fn view(name: &str) -> Fixture {
    let mut f = fixture(name);
    let level = f.app.levels.get_mut(&f.id).unwrap();
    level.set_selection(Some(r(1)), [r(0), r(1), r(2)].into());
    level.focus_selection();
    level.clear_selection();
    frame(&f.ctx, &mut f.app);
    frame(&f.ctx, &mut f.app);
    let rect = f.app.levels[&f.id].view_rect();
    assert!(rect.width() > 200.0 && rect.height() > 200.0, "{rect:?}");
    // The pointer over the view.
    moved(&mut f, rect.center(), Modifiers::NONE);
    f
}

fn at(f: &Fixture, object: usize) -> Pos2 {
    let level = &f.app.levels[&f.id];
    let m = level.object(r(object)).unwrap().transform.unwrap();
    level
        .screen_of([m[3][0], m[3][1], m[3][2]])
        .expect("in front of the camera")
}

fn x_of(f: &Fixture, object: usize) -> f32 {
    f.app.levels[&f.id]
        .object(r(object))
        .unwrap()
        .transform
        .unwrap()[3][0]
}

fn objects(f: &Fixture) -> usize {
    f.app.levels[&f.id].level().unwrap().groups[0].objects.len()
}

fn head(f: &Fixture) -> usize {
    f.app.workspace.as_ref().unwrap().head()
}

fn undo(f: &mut Fixture) {
    f.app.pending.push(Command::Undo);
    frame(&f.ctx, &mut f.app);
    frame(&f.ctx, &mut f.app);
}

#[test]
fn q_w_e_r_choose_the_gizmo_and_the_right_button_flies() {
    let mut f = view("keys");
    fn level(f: &Fixture) -> &crate::level_view::LevelView {
        &f.app.levels[&f.id]
    }
    for (k, mode) in [
        (Key::W, GizmoMode::Move),
        (Key::E, GizmoMode::Rotate),
        (Key::R, GizmoMode::Scale),
        (Key::Q, GizmoMode::Select),
    ] {
        key(&mut f, k, Modifiers::NONE);
        assert_eq!(level(&f).gizmo_mode(), mode, "{k:?}");
    }
    key(&mut f, Key::Space, Modifiers::NONE);
    assert_eq!(level(&f).gizmo_mode(), GizmoMode::Move);
    key(&mut f, Key::Space, Modifiers::NONE);
    assert_eq!(level(&f).gizmo_mode(), GizmoMode::Rotate);
    // Without the right button W does not fly.
    let eye = level(&f).camera().eye;
    let w = |pressed| Event::Key {
        key: Key::W,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    frame_with(&mut f, vec![w(true)], Modifiers::NONE);
    for _ in 0..5 {
        frame_with(&mut f, Vec::new(), Modifiers::NONE);
    }
    frame_with(&mut f, vec![w(false)], Modifiers::NONE);
    assert_eq!(level(&f).camera().eye, eye, "W chose the gizmo only");
    assert_eq!(level(&f).gizmo_mode(), GizmoMode::Move);
    // With the right button held W flies forward and the mode stays.
    let centre = level(&f).view_rect().center();
    button(
        &mut f,
        centre,
        PointerButton::Secondary,
        true,
        Modifiers::NONE,
    );
    key(&mut f, Key::R, Modifiers::NONE);
    frame_with(&mut f, vec![w(true)], Modifiers::NONE);
    for _ in 0..5 {
        frame_with(&mut f, Vec::new(), Modifiers::NONE);
    }
    frame_with(&mut f, vec![w(false)], Modifiers::NONE);
    button(
        &mut f,
        centre,
        PointerButton::Secondary,
        false,
        Modifiers::NONE,
    );
    let camera = level(&f).camera();
    let travelled = crate::render::math::sub(camera.eye, eye);
    assert!(
        crate::render::math::dot(travelled, camera.forward()) > 0.1,
        "flew forward: {travelled:?}"
    );
    assert_eq!(level(&f).gizmo_mode(), GizmoMode::Move, "R did not switch");
    // The camera as before: W flies without the button, 1 2 3 choose.
    f.app.settings.level.classic_camera = true;
    frame(&f.ctx, &mut f.app);
    let eye = level(&f).camera().eye;
    frame_with(&mut f, vec![w(true)], Modifiers::NONE);
    for _ in 0..5 {
        frame_with(&mut f, Vec::new(), Modifiers::NONE);
    }
    frame_with(&mut f, vec![w(false)], Modifiers::NONE);
    assert_ne!(level(&f).camera().eye, eye);
    assert_eq!(level(&f).gizmo_mode(), GizmoMode::Move);
    key(&mut f, Key::Num2, Modifiers::NONE);
    assert_eq!(level(&f).gizmo_mode(), GizmoMode::Rotate);
}

#[test]
fn a_frame_selects_what_is_inside_it() {
    let mut f = view("frame");
    let view = f.app.levels.get_mut(&f.id).unwrap();
    view.set_gizmo_mode(GizmoMode::Select);
    // The objects are markers (no model); shading as in the game hides them.
    view.set_markers(true);
    frame(&f.ctx, &mut f.app);
    let (a, b, c) = (at(&f, 0), at(&f, 1), at(&f, 2));
    let around = |p: Pos2, q: Pos2| {
        let min = p.min(q) - Vec2::splat(12.0);
        let max = p.max(q) + Vec2::splat(12.0);
        (min, max)
    };
    let selection = |f: &Fixture| f.app.levels[&f.id].selection();
    let (from, to) = around(a, b);
    drag(&mut f, from, to, Modifiers::NONE);
    assert_eq!(selection(&f), [r(0), r(1)].into());
    // Shift adds, Ctrl takes away.
    let (from, to) = around(c, c);
    drag(&mut f, from, to, Modifiers::SHIFT);
    assert_eq!(selection(&f), [r(0), r(1), r(2)].into());
    let (from, to) = around(a, a);
    drag(&mut f, from, to, CTRL);
    assert_eq!(selection(&f), [r(1), r(2)].into());
    // Locked and hidden objects are not framed.
    let level = f.app.levels.get_mut(&f.id).unwrap();
    level.set_locked(Node::Object(r(1)), true);
    level.set_hidden(Node::Object(r(2)), true);
    let (from, to) = around(a, c);
    drag(&mut f, from, to, Modifiers::NONE);
    assert_eq!(selection(&f), [r(0)].into());
    // Ctrl+A: everything shown and not locked; Esc: nothing.
    let level = f.app.levels.get_mut(&f.id).unwrap();
    level.set_hidden(Node::Object(r(2)), false);
    key(&mut f, Key::A, CTRL);
    assert_eq!(selection(&f), [r(0), r(2)].into());
    key(&mut f, Key::Escape, Modifiers::NONE);
    assert!(selection(&f).is_empty());
}

#[test]
fn ctrl_d_del_and_alt_drag_are_one_operation_each() {
    let mut f = view("copies");
    // Ctrl+D: two copies, one operation, the copies selected.
    f.app
        .levels
        .get_mut(&f.id)
        .unwrap()
        .set_selection(Some(r(2)), [r(0), r(2)].into());
    let before = head(&f);
    key(&mut f, Key::D, CTRL);
    frame(&f.ctx, &mut f.app);
    assert_eq!(head(&f), before + 1);
    assert_eq!(objects(&f), 5);
    assert_eq!(f.app.levels[&f.id].selection(), [r(3), r(4)].into());
    undo(&mut f);
    assert_eq!(objects(&f), 3);
    // Del: one operation.
    f.app
        .levels
        .get_mut(&f.id)
        .unwrap()
        .set_selection(Some(r(1)), [r(0), r(1)].into());
    key(&mut f, Key::Delete, Modifiers::NONE);
    frame(&f.ctx, &mut f.app);
    assert_eq!(head(&f), before + 1);
    assert_eq!(objects(&f), 1);
    undo(&mut f);
    assert_eq!(objects(&f), 3);

    // The gizmo moves both selected objects: one operation.
    let level = f.app.levels.get_mut(&f.id).unwrap();
    level.set_gizmo_mode(GizmoMode::Move);
    level.set_selection(Some(r(1)), [r(0), r(1)].into());
    frame(&f.ctx, &mut f.app);
    let (start, end) = {
        let level = &f.app.levels[&f.id];
        (
            level.gizmo_point(0, 0.6).unwrap(),
            level.gizmo_point(0, 1.6).unwrap(),
        )
    };
    drag(&mut f, start, end, Modifiers::NONE);
    frame(&f.ctx, &mut f.app);
    assert_eq!(head(&f), before + 1, "one operation for both");
    let d0 = x_of(&f, 0);
    let d1 = x_of(&f, 1) - 10.0;
    assert!(d0 > 0.1 && (d0 - d1).abs() < 1e-3, "{d0} {d1}");
    assert_eq!(objects(&f), 3);
    undo(&mut f);
    assert_eq!((x_of(&f, 0), x_of(&f, 1)), (0.0, 10.0));

    // Alt + drag: copies move, the objects stay; one operation.
    let before = head(&f);
    frame(&f.ctx, &mut f.app);
    let (start, end) = {
        let level = &f.app.levels[&f.id];
        (
            level.gizmo_point(0, 0.6).unwrap(),
            level.gizmo_point(0, 1.6).unwrap(),
        )
    };
    drag(&mut f, start, end, Modifiers::ALT);
    frame(&f.ctx, &mut f.app);
    assert_eq!(head(&f), before + 1, "one operation for both copies");
    assert_eq!(objects(&f), 5);
    assert_eq!((x_of(&f, 0), x_of(&f, 1)), (0.0, 10.0), "the objects stay");
    let (c0, c1) = (x_of(&f, 3), x_of(&f, 4) - 10.0);
    assert!(c0 > 0.1 && (c0 - c1).abs() < 1e-3, "{c0} {c1}");
    assert_eq!(f.app.levels[&f.id].selection(), [r(3), r(4)].into());
    undo(&mut f);
    assert_eq!(objects(&f), 3);
}

#[test]
fn the_context_menu_of_the_view() {
    let mut f = view("context");
    let level = f.app.levels.get_mut(&f.id).unwrap();
    level.set_selection(Some(r(1)), [r(1)].into());
    let centre = level.view_rect().center();
    // The right button without a drag opens it.
    button(
        &mut f,
        centre,
        PointerButton::Secondary,
        true,
        Modifiers::NONE,
    );
    button(
        &mut f,
        centre,
        PointerButton::Secondary,
        false,
        Modifiers::NONE,
    );
    let ctx = f.ctx.clone();
    let level = f.app.levels.get_mut(&f.id).unwrap();
    assert!(level.context().is_some(), "the menu is open");
    assert_eq!(level.selection(), [r(1)].into(), "nothing under it: kept");
    let duplicate = level.context_command(ContextItem::Duplicate, &ctx).unwrap();
    f.app.level_action(f.id, duplicate);
    assert_eq!(objects(&f), 4);
    // Show in the Outliner.
    let level = f.app.levels.get_mut(&f.id).unwrap();
    let show = level.context_command(ContextItem::ShowInOutliner, &ctx);
    assert!(matches!(show, Some(LevelAction::ShowInOutliner)));
    f.app.level_action(f.id, show.unwrap());
    assert!(f.app.dock_ops.contains(&DockOp::ShowPanel(Pane::Outliner)));
    frame(&f.ctx, &mut f.app);
    // Show logic, play from here, select all.
    let level = f.app.levels.get_mut(&f.id).unwrap();
    assert!(matches!(
        level.context_command(ContextItem::ShowLogic, &ctx),
        Some(LevelAction::ShowLogic { .. })
    ));
    assert!(matches!(
        level.context_command(ContextItem::PlayHere, &ctx),
        Some(LevelAction::PlayHere { .. })
    ));
    assert!(
        level
            .context_command(ContextItem::SelectAll, &ctx)
            .is_none()
    );
    assert_eq!(level.selection().len(), 4);
    assert!(level.context_command(ContextItem::CopyGuid, &ctx).is_none());
}

#[test]
fn end_waits_for_the_collision_and_tells_what_it_found() {
    let mut f = view("ground");
    let level = f.app.levels.get_mut(&f.id).unwrap();
    level.set_selection(Some(r(0)), [r(0)].into());
    let before = head(&f);
    key(&mut f, Key::End, Modifiers::NONE);
    let started = std::time::Instant::now();
    while f.app.levels[&f.id].drop_waits() {
        assert!(started.elapsed().as_secs() < 30, "the collision builds");
        std::thread::sleep(std::time::Duration::from_millis(10));
        frame(&f.ctx, &mut f.app);
    }
    // The miniature game has no collision: nothing below, no edit.
    assert_eq!(head(&f), before);
    assert!(f.app.levels[&f.id].note().is_some());
}

#[test]
fn projections_lighting_and_the_bar() {
    let mut f = view("bar");
    let level = f.app.levels.get_mut(&f.id).unwrap();
    level.set_selection(Some(r(1)), [r(1)].into());
    level.set_projection(Projection::Top);
    assert_eq!(level.projection(), Projection::Top);
    frame(&f.ctx, &mut f.app);
    // From above the objects lie left to right, the middle one centred.
    let (a, b, c) = (at(&f, 0), at(&f, 1), at(&f, 2));
    let rect = f.app.levels[&f.id].view_rect();
    assert!(a.x < b.x && b.x < c.x);
    assert!(
        (b - rect.center()).length() < 2.0,
        "{b:?} {:?}",
        rect.center()
    );
    let level = f.app.levels.get_mut(&f.id).unwrap();
    level.set_projection(Projection::Perspective);
    level.set_lighting(Lighting::Unlit);
    assert_eq!(level.lighting(), Lighting::Unlit);
    level.set_lighting(Lighting::Wireframe);
    assert_eq!(level.lighting(), Lighting::Wireframe);
    let mut bar = level.view_bar();
    bar.grid = true;
    bar.stats = true;
    level.set_view_bar(bar);
    level.set_camera_speed(1e9);
    assert_eq!(level.camera_speed(), 2000.0);
    frame(&f.ctx, &mut f.app);
}

#[test]
fn the_history_panel_goes_back_and_forth() {
    let mut f = view("history");
    let first = head(&f);
    for x in [1.0, 2.0, 3.0] {
        let level = &f.app.levels[&f.id];
        let mut m = level.object(r(0)).unwrap().transform.unwrap();
        m[3][0] = x;
        let (group, object) = level.location_of(r(0)).unwrap();
        f.app.level_action(
            f.id,
            LevelAction::SetTransform {
                group,
                object,
                matrix: m,
            },
        );
    }
    assert_eq!(head(&f), first + 3);
    f.app.go_to_history(first + 1);
    assert_eq!(head(&f), first + 1);
    assert_eq!(x_of(&f, 0), 1.0);
    f.app.go_to_history(first + 3);
    assert_eq!(x_of(&f, 0), 3.0);
    f.app.go_to_history(0);
    assert_eq!(head(&f), 0, "back to the start");
    f.app.go_to_history(first + 3);
    assert_eq!(x_of(&f, 0), 3.0);
    // The panel: a drawer of the Level Editor, opened by its command.
    assert_eq!(f.app.checked(Command::ToggleHistory), Some(false));
    f.app.pending.push(Command::ToggleHistory);
    frame(&f.ctx, &mut f.app);
    frame(&f.ctx, &mut f.app);
    assert_eq!(f.app.checked(Command::ToggleHistory), Some(true));
}
