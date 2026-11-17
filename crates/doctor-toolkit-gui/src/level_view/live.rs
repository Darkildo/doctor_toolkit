//! The running game in the level view (stage 35): the editor's camera
//! follows the game's ("game → editor"), and the editor's camera goes to
//! the game — as the game's own camera (the free camera of the module) or
//! by moving the player there ("editor → game"). Both writes go through
//! the confirmation of the Link panel.
//!
//! Stage 47: the selected object in the running game (same GUID) — its
//! state there, the gizmo moving it in the game too, hide and show, undo in
//! the game, and the moves of the game written into the workspace.

use doctor_toolkit::live::proto::{CameraView, Guid, Matrix, ObjectState};
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::{Rect, RichText, Vec2};

use super::LevelAction;
use super::camera::FlyCamera;
use crate::render::math;

/// What the Link panel knows of editing the running game, for the level
/// tabs (set by the app every frame).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LiveEdit {
    /// The module edits the world (protocol 6).
    pub reads: bool,
    /// And writes are on.
    pub writes: bool,
    /// The state in the game of the object last asked for.
    pub state: Option<ObjectState>,
    /// How many edits of the link are not undone, and how many of them are
    /// moves.
    pub edits: usize,
    pub moves: usize,
    /// The gizmo's moves go to the game without asking (allowed once).
    pub gizmo_allowed: bool,
}

/// An edit of the running game from a level tab.
#[derive(Debug, Clone, PartialEq)]
pub enum ObjectAction {
    /// Read the state of this object in the game.
    Get(Guid),
    /// Give it this matrix; `gizmo`: from a drag of the gizmo.
    Move(Guid, Matrix, bool),
    /// Hide (`false`) or show it.
    Enable(Guid, bool),
    /// Stop (`false`) or start its logic: its listener takes no messages
    /// while stopped (stage 103).
    Logic(Guid, bool),
    /// Undo the latest edit of the link (of this object).
    Undo(Option<Guid>),
    /// Write the moves of the link into the workspace.
    ToWorkspace,
}

/// A row matrix of the level as the link sends it.
pub fn flat(m: &doctor_toolkit::level::model::RowMatrix) -> Matrix {
    std::array::from_fn(|i| m[i / 4][i % 4])
}

/// The section of the details of the selected object while the link edits
/// the world: its state in the game and the actions.
pub fn object_section(
    ui: &mut egui::Ui,
    guid: Guid,
    matrix: Option<Matrix>,
    gizmo_too: &mut bool,
    info: &LiveEdit,
) -> Option<ObjectAction> {
    let mut action = None;
    let state = info.state.as_ref().filter(|s| s.guid == guid);
    match state {
        None => {
            ui.label(RichText::new(tr("level.live_not_in_game")).weak().small());
        }
        Some(s) if !s.supported() => {
            ui.label(
                RichText::new(trf("level.live_unsupported", &[("why", &s.unsupported)]))
                    .weak()
                    .small(),
            );
        }
        Some(s) => {
            let place = s
                .position()
                .map(|p| format!("({:.2}, {:.2}, {:.2})", p[0], p[1], p[2]))
                .unwrap_or_default();
            let key = if s.hidden_by_link {
                "level.live_hidden"
            } else if s.shown() == Some(true) {
                "level.live_shown"
            } else {
                "level.live_inactive"
            };
            ui.label(RichText::new(trf(key, &[("place", &place)])).small());
        }
    }
    let can = info.writes && state.is_some_and(ObjectState::supported);
    ui.horizontal_wrapped(|ui| {
        if ui
            .button(tr("level.live_refresh"))
            .on_hover_text(tr("level.live_refresh_hint"))
            .clicked()
        {
            action = Some(ObjectAction::Get(guid));
        }
        ui.add_enabled(
            info.writes,
            egui::Checkbox::new(gizmo_too, tr("level.live_gizmo")),
        )
        .on_hover_text(tr("level.live_gizmo_hint"));
        if let Some(m) = matrix
            && ui
                .add_enabled(can, egui::Button::new(tr("level.live_place")))
                .on_hover_text(tr("level.live_place_hint"))
                .clicked()
        {
            action = Some(ObjectAction::Move(guid, m, false));
        }
        let hidden = state.is_some_and(|s| s.hidden_by_link);
        let (key, hint) = if hidden {
            ("level.live_show", "level.live_show_hint")
        } else {
            ("level.live_hide", "level.live_hide_hint")
        };
        if ui
            .add_enabled(can, egui::Button::new(tr(key)))
            .on_hover_text(tr(hint))
            .clicked()
        {
            action = Some(ObjectAction::Enable(guid, hidden));
        }
        // The logic of the object: its listener of messages, on or off.
        if let Some(s) = state.filter(|s| s.supported()) {
            let mut on = s.listening;
            if ui
                .add_enabled(can, egui::Checkbox::new(&mut on, tr("level.live_logic")))
                .on_hover_text(tr("level.live_logic_hint"))
                .changed()
            {
                action = Some(ObjectAction::Logic(guid, on));
            }
        }
        if ui
            .add_enabled(
                info.writes && info.edits > 0,
                egui::Button::new(tr("level.live_undo")),
            )
            .on_hover_text(tr("level.live_undo_hint"))
            .clicked()
        {
            action = Some(ObjectAction::Undo(Some(guid)));
        }
        if ui
            .add_enabled(info.moves > 0, egui::Button::new(tr("level.live_keep")))
            .on_hover_text(trf("level.live_keep_hint", &[("count", &info.moves)]))
            .clicked()
        {
            action = Some(ObjectAction::ToWorkspace);
        }
    });
    action
}

/// The editor's fly camera at the game's camera: same eye, direction and
/// vertical field of view.
pub fn fly_from_game(game: &CameraView, camera: &mut FlyCamera) -> bool {
    let f = game.pose.forward;
    let len = math::length(f);
    if len.is_nan() || len <= 1e-4 || game.pose.position.iter().any(|v| !v.is_finite()) {
        return false;
    }
    let f = math::scale(f, 1.0 / len);
    let before = *camera;
    camera.eye = game.pose.position;
    camera.pitch = f[1].clamp(-1.0, 1.0).asin().clamp(-1.55, 1.55);
    // FlyCamera: yaw 0 looks along −Z, forward = (−sin, ·, −cos).
    camera.yaw = (-f[0]).atan2(-f[2]);
    if let Some(fov) = game.fov.filter(|v| (5.0..170.0).contains(v)) {
        camera.fov_y = fov.to_radians();
    }
    *camera != before
}

/// The game's camera at the editor's: eye and direction (the field of view
/// stays the game's).
pub fn game_from_fly(camera: &FlyCamera, fov: Option<f32>) -> CameraView {
    CameraView {
        pose: doctor_toolkit::live::pose_looking(camera.eye, camera.forward()),
        fov,
    }
}

/// Where the player goes for "editor → game" by teleport: below the camera,
/// facing where it looks (the game puts him on the ground).
pub fn teleport_from_fly(camera: &FlyCamera) -> ([f32; 3], f32) {
    let f = camera.forward();
    (camera.eye, f[0].atan2(f[2]))
}

/// The bar over the viewport while the link is up: follow the game's
/// camera, send the editor's camera to the game.
pub fn bar(
    ui: &mut egui::Ui,
    viewport: Rect,
    marks: &crate::app::LiveMarks,
    follow: &mut bool,
    camera: &FlyCamera,
) -> Option<LevelAction> {
    let mut action = None;
    let bar = Rect::from_min_size(
        viewport.left_bottom() + Vec2::new(8.0, -34.0),
        Vec2::new((viewport.width() - 16.0).max(10.0), 28.0),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| {
        egui::Frame::popup(ui.style())
            .inner_margin(egui::Margin::symmetric(6, 2))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tr("debug.game")).strong());
                    ui.add_enabled(
                        marks.game_camera.is_some(),
                        egui::Checkbox::new(follow, tr("debug.follow")),
                    )
                    .on_hover_text(tr("debug.follow_hint"));
                    if ui
                        .add_enabled(
                            marks.player.is_some() && !*follow,
                            egui::Button::new(tr("debug.teleport_here")),
                        )
                        .on_hover_text(tr("debug.teleport_here_hint"))
                        .clicked()
                    {
                        let (position, yaw) = teleport_from_fly(camera);
                        action = Some(LevelAction::LiveTeleport { position, yaw });
                    }
                    if marks.can_free_camera {
                        if ui
                            .add_enabled(!*follow, egui::Button::new(tr("debug.camera_to_game")))
                            .on_hover_text(tr("debug.camera_to_game_hint"))
                            .clicked()
                        {
                            let fov = marks.game_camera.and_then(|c| c.fov);
                            action =
                                Some(LevelAction::LiveCamera(Some(game_from_fly(camera, fov))));
                        }
                        if marks.free_camera
                            && ui
                                .button(tr("debug.camera_release"))
                                .on_hover_text(tr("debug.camera_release_hint"))
                                .clicked()
                        {
                            action = Some(LevelAction::LiveCamera(None));
                        }
                    }
                });
            });
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_toolkit::live::proto::Pose;

    #[test]
    fn the_editor_camera_looks_where_the_game_camera_does() {
        let game = CameraView {
            pose: doctor_toolkit::live::pose_looking([10.0, 2.0, -5.0], [0.6, -0.2, 0.8]),
            fov: Some(36.0),
        };
        let mut fly = FlyCamera::default();
        assert!(fly_from_game(&game, &mut fly));
        let f = fly.forward();
        let g = math::scale(game.pose.forward, 1.0 / math::length(game.pose.forward));
        for (a, b) in f.iter().zip(g) {
            assert!((a - b).abs() < 1e-4, "{f:?} {g:?}");
        }
        assert_eq!(fly.eye, [10.0, 2.0, -5.0]);
        assert!((fly.fov_y.to_degrees() - 36.0).abs() < 1e-3);
        // Again: nothing changes.
        assert!(!fly_from_game(&game, &mut fly));
        // And back: the game's camera at the editor's, a world matrix.
        let back = game_from_fly(&fly, game.fov);
        for (a, b) in back.pose.forward.iter().zip(g) {
            assert!((a - b).abs() < 1e-4);
        }
        let p: Pose = back.pose;
        assert!((p.up[1]) > 0.0);
        let (at, yaw) = teleport_from_fly(&fly);
        assert_eq!(at, fly.eye);
        assert!((yaw - g[0].atan2(g[2])).abs() < 1e-4);
        // A broken camera is ignored.
        let mut bad = game;
        bad.pose.forward = [0.0; 3];
        assert!(!fly_from_game(&bad, &mut fly));
    }
}
