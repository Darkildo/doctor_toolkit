//! Models dropped into the view from the Content Browser (stage 55).
//!
//! While a model (a [`DraggedAsset`] metamodel) is dragged over the view,
//! the ray under the pointer is cast against the Havok collision of the
//! loaded zones (the world of [`super::walk`], built in the background on the
//! first drag); from the point it hits, a second ray goes straight down to
//! the ground. A ghost — the model's bounding box standing on that point —
//! shows where it goes. On release the view returns the same
//! [`LevelAction::Add`] as "Add object": one undoable operation of the
//! workspace. The new object copies the selected object, or else the
//! nearest object with a model (its class, logic and SimGroup); the model
//! is used as it is when the game finds it from that SimGroup's stream, or
//! cloned into the stream otherwise.
//!
//! A drop before the collision is built waits for it (the ghost says so).
//!
//! The payload has no name or size: the Content Browser leaves them in a
//! [`Ghost`] next to it ([`set_ghost`]).

use std::sync::Arc;

use doctor_toolkit::level::edit::ModelChoice;
use doctor_toolkit::level::{Found, Level, Location, TYPE_METAMODEL};
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::{Color32, Pos2, Rect, Stroke};

use super::walk::physics::{V3, World};
use super::{LevelAction, LevelView};
use crate::app::dnd::DraggedAsset;
use crate::render::{View, math};

/// Farthest point a drop ray looks at, metres.
const REACH: f32 = 3000.0;

/// A drop that waits for the collision world.
#[derive(Clone)]
struct Pending {
    asset: DraggedAsset,
    origin: V3,
    dir: V3,
}

/// What the ghost of a dragged model shows: its name and its bounds.
#[derive(Debug, Clone, PartialEq)]
pub struct Ghost {
    pub asset: DraggedAsset,
    pub label: String,
    /// Bounds of the model in its own space (from its preview).
    pub bounds: Option<[[f32; 3]; 2]>,
}

fn ghost_id() -> egui::Id {
    egui::Id::unique("level-drop-ghost")
}

/// Leaves the name and bounds of the asset being dragged for the ghost.
pub fn set_ghost(ctx: &egui::Context, ghost: Ghost) {
    ctx.data_mut(|d| d.insert_temp(ghost_id(), ghost));
}

/// The ghost of `asset`, if its source left one.
fn ghost(ctx: &egui::Context, asset: &DraggedAsset) -> Option<Ghost> {
    ctx.data(|d| d.get_temp::<Ghost>(ghost_id()))
        .filter(|g| g.asset == *asset)
}

/// A dragged metamodel.
fn model(asset: Option<std::sync::Arc<DraggedAsset>>) -> Option<DraggedAsset> {
    asset.filter(|a| a.type_id == TYPE_METAMODEL).map(|a| *a)
}

/// The point on the ground for the ray from `origin` along `dir` (unit
/// length): the first collider it hits when that faces up; on a wall or
/// under a roof, straight down from just in front of the point it hits (the
/// object stands on the ground in front of the wall). `None` when the ray
/// hits nothing.
pub fn ground_point(world: &World, origin: V3, dir: V3) -> Option<V3> {
    doctor_toolkit::level::walk::ground_point(world, origin, dir, REACH)
}

/// The object a dropped model copies: the selected one; else the nearest
/// object that shows the same model already (its class and logic suit the
/// model); else the nearest one with a model and a place (or with a place
/// only, when no object has a model). Only objects that may show the model
/// are taken (an object set up to break needs a model that breaks,
/// `level::compat`).
pub fn template(
    level: &Level,
    selected: Option<(usize, usize)>,
    at: V3,
    id: u32,
    model: Option<&doctor_toolkit::resource::metamodel::MetaModel>,
) -> Option<(usize, usize)> {
    let fits = |g: usize, o: usize| {
        model.is_none_or(|mm| {
            doctor_toolkit::level::compat::model_problem(&level.groups[g].group, o, mm).is_none()
        })
    };
    let has_place = |g: usize, o: usize| {
        level
            .groups
            .get(g)?
            .objects
            .get(o)?
            .transform
            .map(|_| (g, o))
    };
    if let Some((g, o)) = selected
        && let Some(found) = has_place(g, o)
        && fits(g, o)
    {
        return Some(found);
    }
    // (another model, no model, distance²): the same model first.
    type Rank = (bool, bool, f32);
    let mut best: Option<(Rank, (usize, usize))> = None;
    for (g, group) in level.groups.iter().enumerate() {
        for (o, object) in group.objects.iter().enumerate() {
            let Some(m) = object.transform else {
                continue;
            };
            if !fits(g, o) {
                continue;
            }
            let d: f32 = (0..3).map(|i| (m[3][i] - at[i]).powi(2)).sum();
            let rank = (object.model != Some(id), object.model.is_none(), d);
            if best.is_none_or(|(b, _)| rank < b) {
                best = Some((rank, (g, o)));
            }
        }
    }
    best.map(|(_, found)| found)
}

/// How the dropped metamodel `id` is used: as it is when the game finds it
/// from the stream of the SimGroup (`reachable`), else a clone of the
/// game's resource.
pub fn model_choice(id: u32, found: Option<Found>, reachable: bool) -> Option<ModelChoice> {
    match found? {
        Found::Game(..) | Found::Override(..) if reachable => Some(ModelChoice::Existing(id)),
        Found::Game(at, _) => Some(ModelChoice::Clone(at)),
        Found::Override(..) => Some(ModelChoice::Existing(id)),
    }
}

impl LevelView {
    /// Shows where a dragged model would go and returns the Add of a drop.
    pub(super) fn drop_ui(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        rect: Rect,
        view: &View,
    ) -> Option<LevelAction> {
        let pending_id = egui::Id::unique(("level-drop", Arc::as_ptr(&self.catalog) as usize));
        let pending: Option<Pending> = ui.ctx().data(|d| d.get_temp(pending_id));
        let hovered = model(response.dnd_hover_payload::<DraggedAsset>());
        let released = model(response.dnd_release_payload::<DraggedAsset>());
        if pending.is_none() && hovered.is_none() && released.is_none() {
            return None;
        }
        let pointer = response
            .hover_pos()
            .or(ui.input(|i| i.pointer.latest_pos()));
        let ray = pointer.map(|p| {
            let uv = (p - rect.min) / rect.size();
            view.ray([uv.x, uv.y])
        });
        let (level, input) = (self.level.as_ref()?, self.input.as_ref()?);
        let centre = ray.as_ref().map_or(self.camera.eye, |r| r.origin);
        let world = self
            .walk
            .world_for(level, &self.catalog, &input.parents, centre);
        let ready = world.is_some();
        let point = |origin: V3, dir: V3| world.and_then(|w| ground_point(w, origin, dir));

        if let Some(p) = pending {
            if !ready {
                ui.ctx().request_repaint();
                return None;
            }
            ui.ctx().data_mut(|d| d.remove::<Pending>(pending_id));
            let at = point(p.origin, p.dir);
            return self.drop_action(&p.asset, at, p.origin, p.dir);
        }
        let ray = ray?;
        if let Some(asset) = released {
            if !ready {
                self.status = tr("drop.waiting").to_owned();
                ui.ctx().data_mut(|d| {
                    d.insert_temp(
                        pending_id,
                        Pending {
                            asset,
                            origin: ray.origin,
                            dir: ray.dir,
                        },
                    )
                });
                return None;
            }
            let at = point(ray.origin, ray.dir);
            return self.drop_action(&asset, at, ray.origin, ray.dir);
        }
        // The ghost.
        let asset = hovered?;
        let ghost = ghost(ui.ctx(), &asset);
        let at = point(ray.origin, ray.dir);
        let painter = ui.painter_at(rect);
        let to_screen = |p: V3| {
            view.project(p).map(|(uv, _)| {
                Pos2::new(
                    rect.min.x + uv[0] * rect.width(),
                    rect.min.y + uv[1] * rect.height(),
                )
            })
        };
        let accent = crate::app::theme::ACCENT;
        // The text goes under the point on the ground, or under the label
        // the Content Browser shows at the pointer.
        let (text, anchor, align) = match at {
            Some(at) => {
                if let Some([min, max]) = ghost.as_ref().and_then(|g| g.bounds) {
                    ghost_box(&painter, &to_screen, at, min, max, accent);
                }
                let marker = to_screen(at);
                if let Some(p) = marker {
                    painter.circle_stroke(p, 6.0, Stroke::new(2.0, accent));
                    painter.circle_filled(p, 2.0, accent);
                }
                let text = trf(
                    "drop.here",
                    &[
                        ("x", &format!("{:.1}", at[0])),
                        ("y", &format!("{:.1}", at[1])),
                        ("z", &format!("{:.1}", at[2])),
                    ],
                );
                (
                    text,
                    marker.map(|p| p + egui::vec2(0.0, 10.0)),
                    egui::Align2::CENTER_TOP,
                )
            }
            None => (
                if ready {
                    tr("drop.no_ground").to_owned()
                } else {
                    tr("drop.building").to_owned()
                },
                pointer.map(|p| p + egui::vec2(16.0, 40.0)),
                egui::Align2::LEFT_TOP,
            ),
        };
        if let Some(p) = anchor {
            let galley =
                painter.layout_no_wrap(text, egui::FontId::proportional(12.0), Color32::WHITE);
            let rect = align.anchor_size(p, galley.size()).expand(3.0);
            painter.rect_filled(rect, 3.0, Color32::from_black_alpha(160));
            painter.galley(rect.min + egui::vec2(3.0, 3.0), galley, Color32::WHITE);
        }

        if !ready {
            ui.ctx().request_repaint();
        }
        None
    }

    /// The Add of a model dropped at `at` (or, with no ground under the
    /// pointer, 10 m along the ray).
    fn drop_action(
        &mut self,
        asset: &DraggedAsset,
        at: Option<V3>,
        origin: V3,
        dir: V3,
    ) -> Option<LevelAction> {
        let id = asset.id;
        let position = match at {
            Some(at) => at,
            None => {
                self.status = tr("drop.no_ground_placed").to_owned();
                math::add(origin, math::scale(dir, 10.0))
            }
        };
        let level = self.level.as_ref()?;
        let input = self.input.as_ref()?;
        let selected = self.filter.selected.map(|r| (r.group, r.object));
        let parsed = level.source.metamodel(id);
        let Some((g, o)) = template(level, selected, position, id, parsed.as_deref()) else {
            self.status = tr("drop.no_template").to_owned();
            return None;
        };
        let group: Location = level.groups[g].location;
        let found = level.source.find(TYPE_METAMODEL, id);
        let game = &input.game;
        let stream_name = game.streams()[group.stream].stream_name();
        let ancestors = doctor_toolkit::game::ancestors(&input.parents, &stream_name);
        let reachable = found.is_some_and(|f| {
            f.stream() == group.stream
                || ancestors.contains(&game.streams()[f.stream()].stream_name())
        });
        let Some(model) = model_choice(id, found, reachable) else {
            self.status = trf("drop.not_found", &[("model", &format!("{id:08x}"))]);
            return None;
        };
        Some(LevelAction::Add {
            group,
            template: (group, o),
            model,
            position,
        })
    }
}

#[cfg(test)]
impl LevelView {
    /// A view of the game with the zones `streams` loaded at once (and the
    /// resources of `overrides` in place of the game's), for tests.
    pub(crate) fn with_level_for_test(
        game: Arc<doctor_toolkit::game::GameInstance>,
        index: Arc<doctor_toolkit::game::ResourceIndex>,
        streams: &[usize],
        overrides: doctor_toolkit::level::Overrides,
        ctx: &egui::Context,
    ) -> Self {
        let mut view = Self::new(None, ctx);
        view.open_game(game, index);
        let input = view.input.as_ref().expect("opened");
        let scope = doctor_toolkit::level::Scope::new(&input.game, &input.parents, streams);
        let source = doctor_toolkit::level::LevelSource::new(
            input.game.clone(),
            input.lookup.clone(),
            overrides,
            scope,
        );
        let level = Level::load(Arc::new(source), streams).expect("the level loads");
        view.group_generation = vec![0; level.groups.len()];
        view.level = Some(Box::new(level));
        view.rebuild_items();
        view
    }

    /// What dropping `asset` with the ground at `at` (or none) does.
    pub(crate) fn drop_for_test(
        &mut self,
        asset: &DraggedAsset,
        at: Option<V3>,
        origin: V3,
        dir: V3,
    ) -> (Option<LevelAction>, String) {
        let action = self.drop_action(asset, at, origin, dir);
        (action, self.status.clone())
    }
}

/// The bounding box of the model standing at `at`, as lines.
fn ghost_box(
    painter: &egui::Painter,
    to_screen: &dyn Fn(V3) -> Option<Pos2>,
    at: V3,
    min: V3,
    max: V3,
    color: Color32,
) {
    let corner = |i: usize| {
        [
            at[0] + if i & 1 == 0 { min[0] } else { max[0] },
            at[1] + if i & 2 == 0 { min[1] } else { max[1] },
            at[2] + if i & 4 == 0 { min[2] } else { max[2] },
        ]
    };
    let stroke = Stroke::new(1.5, color.gamma_multiply(0.85));
    for (a, b) in [
        (0, 1),
        (2, 3),
        (4, 5),
        (6, 7),
        (0, 2),
        (1, 3),
        (4, 6),
        (5, 7),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ] {
        if let (Some(p), Some(q)) = (to_screen(corner(a)), to_screen(corner(b))) {
            painter.line_segment([p, q], stroke);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::walk::physics::{Owner, Placed};
    use super::*;
    use rapier3d::prelude::{Pose, SharedShape};

    /// A floor 40 × 40 m with its top at y = 2, a wall 10 m high at x = 5
    /// and a roof at y = 8.
    fn world() -> World {
        let placed = |shape: SharedShape, centre: V3| Placed {
            owner: Owner {
                layer: 0,
                object: (0, 0),
                hkx: 1,
            },
            parts: Arc::new(vec![(Pose::IDENTITY, shape)]),
            pose: Pose::from_translation(rapier3d::prelude::Vector::new(
                centre[0], centre[1], centre[2],
            )),
        };
        World::new(&[
            placed(SharedShape::cuboid(20.0, 1.0, 20.0), [0.0, 1.0, 0.0]),
            placed(SharedShape::cuboid(0.5, 5.0, 20.0), [5.5, 7.0, 0.0]),
            placed(SharedShape::cuboid(2.0, 0.1, 2.0), [0.0, 8.0, 0.0]),
        ])
    }

    fn close(a: V3, b: V3) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-3)
    }

    #[test]
    fn a_drop_stands_on_the_ground() {
        let world = world();
        // Looking down at the floor from above: the point it hits.
        let at = ground_point(&world, [-10.0, 50.0, 0.0], [0.0, -1.0, 0.0]).unwrap();
        assert!(close(at, [-10.0, 2.0, 0.0]), "{at:?}");
        // On the roof when it is hit from above.
        let at = ground_point(&world, [0.0, 50.0, 0.0], [0.0, -1.0, 0.0]).unwrap();
        assert!(close(at, [0.0, 8.1, 0.0]), "{at:?}");
        // At an angle onto the floor.
        let dir = math::normalize_or([1.0, -1.0, 0.0], [0.0, -1.0, 0.0]);
        let at = ground_point(&world, [-10.0, 12.0, 3.0], dir).unwrap();
        assert!(close(at, [0.0, 2.0, 3.0]), "{at:?}");
        // Onto the wall: in front of it, on the floor.
        let at = ground_point(&world, [-5.0, 6.0, 1.0], [1.0, 0.0, 0.0]).unwrap();
        assert!(
            (at[1] - 2.0).abs() < 1e-3 && at[0] < 5.0 && at[0] > 4.0,
            "{at:?}"
        );
        // Up at a roof: the ground below it.
        let at = ground_point(&world, [0.0, 3.0, 0.0], [0.0, 1.0, 0.0]).unwrap();
        assert!(close(at, [0.0, 2.0, 0.0]), "{at:?}");
        // Into the sky: nothing.
        assert_eq!(
            ground_point(&world, [10.0, 50.0, 0.0], [0.0, 1.0, 0.0]),
            None
        );
    }

    #[test]
    fn models_are_used_or_cloned() {
        let at = Location {
            stream: 3,
            resource: 7,
        };
        assert_eq!(
            model_choice(9, Some(Found::Game(at, true)), true),
            Some(ModelChoice::Existing(9))
        );
        assert_eq!(
            model_choice(9, Some(Found::Game(at, false)), false),
            Some(ModelChoice::Clone(at))
        );
        assert_eq!(
            model_choice(9, Some(Found::Override(3, true)), false),
            Some(ModelChoice::Existing(9))
        );
        assert_eq!(model_choice(9, None, true), None);
    }
}
