//! The running game in the level view (stage 35): the editor's camera
//! follows the game's ("game → editor"), and the editor's camera goes to
//! the game — as the game's own camera (the free camera of the module) or
//! by moving the player there ("editor → game"). Both writes go through
//! the confirmation of the Link panel.

use doctor_toolkit::live::proto::CameraView;
use doctor_toolkit_mods::i18n::tr;
use egui::{Rect, RichText, Vec2};

use super::LevelAction;
use super::camera::FlyCamera;
use crate::render::math;

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
