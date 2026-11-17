//! The collision of a level as a physics world (feature `walk`): what the
//! pointer and the feet of a player meet, without the game.
//!
//! - [`physics`]: the Havok shapes as `rapier3d` colliders, rays, the
//!   character controller and the straight walk from A to B;
//! - [`build`]: the collision of the placed objects of a level (and the
//!   ground of its districts) put into one world, parsed once per `hkx`;
//! - here: [`Scene`], the world of some zones as the workspace has them,
//!   and what the mouse does in the level view said as data — the point on
//!   the ground a ray hits ([`ground_point`]), the ground below a point or
//!   an object ([`Scene::ground_below`], [`Scene::drop_height`]), a walk
//!   ([`Scene::walk`]).
//!
//! The level view of the editor (End, dropping a model, the Walk panel) and
//! the MCP server (`ground`, `on_ground` of `move_object` and `add_object`,
//! `walk_route`) use the same world and the same queries.

pub mod build;
pub mod physics;

use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;

use crate::game::{GameInstance, ResourceIndex};
use crate::workspace::Workspace;
use crate::{Error, Result};

use super::{Catalog, Level, LevelSource, Location, Lookup, Overrides, Scope};
use build::{Area, Cache, Layer, Stats};
use physics::{BodySize, Outcome, Owner, V3, World};

/// How far the ray of a pointer reaches, metres.
pub const REACH: f32 = 2000.0;

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn normalize(a: V3) -> Option<V3> {
    let len = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    (len > 1e-6).then(|| scale(a, 1.0 / len))
}

/// The point on the ground for the ray from `origin` along `dir` (unit
/// length): the first collider it hits when that faces up; on a wall or
/// under a roof, straight down from just in front of the point it hits (the
/// object stands on the ground in front of the wall). `None` when the ray
/// hits nothing within `reach`.
pub fn ground_point(world: &World, origin: V3, dir: V3, reach: f32) -> Option<V3> {
    let (toi, normal) = world.cast(origin, dir, reach)?;
    let hit = add(origin, scale(dir, toi));
    if normal[1] > 0.5 {
        return Some(hit);
    }
    // Under a roof: down from just below it; at a wall: back from it along
    // the ray's horizontal direction.
    let above = if normal[1] < -0.5 {
        [hit[0], hit[1] - 0.05, hit[2]]
    } else {
        let flat = normalize([dir[0], 0.0, dir[2]]).unwrap_or([0.0; 3]);
        let back = sub(hit, scale(flat, toi.min(0.3)));
        [back[0], back[1] + 0.5, back[2]]
    };
    match world.cast(above, [0.0, -1.0, 0.0], 1000.0) {
        Some((down, _)) => Some([above[0], above[1] - down, above[2]]),
        None => Some(hit),
    }
}

/// What to load for a [`Scene`].
#[derive(Debug, Clone, PartialEq)]
pub struct SceneOptions {
    /// Also load the `medium` zones of the districts of the streams: the
    /// game keeps the terrain collision of a district there.
    pub ground: bool,
    /// Build only collision within this many metres of the centre.
    pub area: Option<Area>,
}

impl Default for SceneOptions {
    fn default() -> Self {
        Self {
            ground: true,
            area: None,
        }
    }
}

/// How a walk ended, for an answer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WalkReport {
    /// `reached`, `stuck`, `fell` or `timed_out`.
    pub outcome: &'static str,
    /// Where the feet were at the end: at B on the plan when reached, else
    /// at the point of the problem.
    pub end: V3,
    /// B as it was put on the ground.
    pub target: V3,
    /// Feet positions, one per 0.1 s.
    pub path: Vec<V3>,
    /// Length of the path walked, metres.
    pub walked: f32,
    /// Seconds of the walk.
    pub seconds: f32,
    /// The object nearest in front of the player where it got stuck:
    /// `(group, object)` of the level, when a ray along the way hits one.
    pub blocked_by: Option<(usize, usize)>,
}

/// The collision world of some zones of the game as the workspace has
/// them, with the levels it was built from.
pub struct Scene {
    pub level: Level,
    /// The ground zones loaded besides (layer 1 of the world).
    pub ground: Option<Level>,
    pub world: World,
    pub stats: Stats,
}

/// Edited SimGroups of `groups` and edited or added models, meshes and
/// collisions of the workspace, keyed as a level looks them up.
pub fn overrides(game: &GameInstance, ws: Option<&Workspace>, groups: &[Location]) -> Overrides {
    use crate::workspace::ResourceKey;
    let mut out = Overrides::default();
    let Some(ws) = ws else { return out };
    let types = [
        super::TYPE_METAMODEL,
        crate::resource::mesh::TYPE_MESH,
        crate::resource::mesh::TYPE_MESH_VOLATILE,
        crate::resource::hkx::TYPE_HKX,
    ];
    for change in ws.changes() {
        let t = change.key.type_id;
        if !types.contains(&t) {
            continue;
        }
        let Some(stream) = game.find_stream(&change.key.stream) else {
            continue;
        };
        if let Ok(Some(data)) = ws.data(&change.key) {
            out.by_id
                .insert((t, change.key.ids[3]), (stream, Arc::new(data)));
        }
    }
    let mut streams: HashMap<usize, Option<Arc<crate::game::StreamFile>>> = HashMap::new();
    for &g in groups {
        let file = streams
            .entry(g.stream)
            .or_insert_with(|| game.open_stream(g.stream).ok().map(Arc::new));
        let Some(file) = file else { continue };
        let Some(resource) = file.layout().resources.get(g.resource) else {
            continue;
        };
        let key = ResourceKey::new(&game.streams()[g.stream].stream_name(), &resource.header);
        if let Ok(Some(data)) = ws.data(&key) {
            out.groups.insert(g, Arc::new(data));
        }
    }
    out
}

impl Scene {
    /// Loads `streams` (zones with SimGroups) as the workspace has them and
    /// builds their collision; with [`SceneOptions::ground`] also the
    /// ground zones of their districts.
    pub fn load(
        game: &Arc<GameInstance>,
        index: &ResourceIndex,
        ws: Option<&Workspace>,
        streams: &[usize],
        options: &SceneOptions,
    ) -> Result<Self> {
        if streams.is_empty() {
            return Err(Error::InvalidArgument(
                "name at least one stream to take the collision from".into(),
            ));
        }
        let lookup = Arc::new(Lookup::new(index));
        let parents = game.stream_parents()?;
        let load = |streams: &[usize]| -> Result<Level> {
            let groups: Vec<Location> = streams
                .iter()
                .flat_map(|&s| {
                    lookup
                        .in_stream(s, super::TYPE_SIMGROUP)
                        .map(move |r| Location {
                            stream: s,
                            resource: r,
                        })
                })
                .collect();
            let source = Arc::new(LevelSource::new(
                game.clone(),
                lookup.clone(),
                overrides(game, ws, &groups),
                Scope::new(game, &parents, streams),
            ));
            Level::load(source, streams)
        };
        let level = load(streams)?;
        let ground = if options.ground {
            let catalog = Catalog::new(game, index);
            let zones = build::ground_zones(
                catalog.zones.iter().map(|z| (z.stream, z.name.as_str())),
                streams,
            );
            if zones.is_empty() {
                None
            } else {
                Some(load(&zones)?)
            }
        } else {
            None
        };
        let placements = level.collision_placements();
        let ground_placements = ground
            .as_ref()
            .map(Level::collision_placements)
            .unwrap_or_default();
        let mut layers = vec![Layer {
            source: &level.source,
            placements: &placements,
        }];
        if let Some(ground) = &ground {
            layers.push(Layer {
                source: &ground.source,
                placements: &ground_placements,
            });
        }
        let (world, stats) = build::build(&layers, options.area, &Cache::default());
        Ok(Self {
            level,
            ground,
            world,
            stats,
        })
    }

    /// `(group, object)` of the level for an object of a game SimGroup.
    pub fn object(&self, group: Location, object: usize) -> Option<(usize, usize)> {
        let g = self.level.groups.iter().position(|g| g.location == group)?;
        (object < self.level.groups[g].objects.len()).then_some((g, object))
    }

    /// The game SimGroup and object number of `(group, object)` of the level.
    pub fn location(&self, object: (usize, usize)) -> Option<(Location, usize)> {
        Some((self.level.groups.get(object.0)?.location, object.1))
    }

    /// The height of the ground below `from` (within 2 km), passing over
    /// the colliders of the level objects `skip` names.
    pub fn ground_below(&self, from: V3, skip: &dyn Fn((usize, usize)) -> bool) -> Option<f32> {
        let pass = |o: Owner| o.layer == 0 && skip(o.object);
        self.world
            .below_except(from, 2000.0, &pass)
            .map(|toi| from[1] - toi)
    }

    /// How far to move an object along Y so that it stands on the ground
    /// at `target`: the lowest point of its own collision (its origin when
    /// it has none) comes onto the first collider below it, or — when
    /// nothing is below — onto the one above which it hangs. `object` is
    /// its place in the level, when it is one of its objects, and `origin`
    /// where it stands there now; its own colliders are passed over.
    /// `None` when there is no ground.
    pub fn drop_height(
        &self,
        object: Option<(usize, usize)>,
        origin: V3,
        target: V3,
    ) -> Option<f32> {
        let moved = sub(target, origin);
        let (x, bottom, z) = match object.and_then(|o| self.world.object_bounds(0, o)) {
            Some(b) => (
                (b.mins.x + b.maxs.x) / 2.0 + moved[0],
                b.mins.y + moved[1],
                (b.mins.z + b.maxs.z) / 2.0 + moved[2],
            ),
            None => (target[0], target[1], target[2]),
        };
        let skip = |o: (usize, usize)| Some(o) == object;
        let ground = self
            .ground_below([x, bottom + 0.05, z], &skip)
            .or_else(|| self.ground_below([x, bottom + 1000.0, z], &skip))?;
        Some(ground - bottom)
    }

    /// A straight walk of a player from `from` to `to` (feet positions;
    /// each is put on the ground below it first, when there is one within
    /// 50 m).
    pub fn walk(&self, body: &BodySize, from: V3, to: V3) -> WalkReport {
        let settle = |p: V3| -> V3 {
            match self.world.below([p[0], p[1] + 1.0, p[2]], 50.0) {
                Some((toi, _)) => [p[0], p[1] + 1.0 - toi, p[2]],
                None => p,
            }
        };
        let (from, to) = (settle(from), settle(to));
        let (outcome, path) = physics::walk_to(&self.world, body, from, to);
        let walked: f32 = path
            .windows(2)
            .map(|w| {
                let d = sub(w[1], w[0]);
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
            })
            .sum();
        let (name, end) = match outcome {
            // Where the feet are: B is reached by the distance on the
            // plan, and the caller sees how far above or below it.
            Outcome::Reached => ("reached", path.last().copied().unwrap_or(to)),
            Outcome::Stuck(p) => ("stuck", p),
            Outcome::Fell(p) => ("fell", p),
            Outcome::TimedOut(p) => ("timed_out", p),
        };
        // What stands in the way: a ray at chest height towards B.
        let blocked_by = (name == "stuck")
            .then(|| {
                let chest = [end[0], end[1] + body.height * 0.6, end[2]];
                let dir = normalize([to[0] - end[0], 0.0, to[2] - end[2]])?;
                let ray = rapier3d::prelude::Ray::new(
                    rapier3d::prelude::Vector::new(chest[0], chest[1], chest[2]),
                    rapier3d::prelude::Vector::new(dir[0], dir[1], dir[2]),
                );
                let (handle, _) = self.world.queries().cast_ray(&ray, 3.0, true)?;
                self.world
                    .owner(handle)
                    .filter(|o| o.layer == 0)
                    .map(|o| o.object)
            })
            .flatten();
        WalkReport {
            outcome: name,
            end,
            target: to,
            seconds: path.len().saturating_sub(1) as f32 * 0.1,
            path,
            walked,
            blocked_by,
        }
    }
}
