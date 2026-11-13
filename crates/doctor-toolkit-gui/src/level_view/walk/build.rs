//! Building the walking world of a loaded level: the collision of every
//! placed object, parsed once per `hkx` (cached between builds), placed by
//! the object matrices, optionally only around a point.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use doctor_toolkit::level::{LevelSource, PlacedCollision};
use rapier3d::prelude::{Aabb, Pose, Vector};
use rayon::prelude::*;

use super::physics::{Affine, Owner, Parts, Placed, V3, World, physics_parts};

/// Collision of one `hkx` in its model space with its bounds.
pub struct Model {
    pub parts: Arc<Parts>,
    pub bounds: Option<Aabb>,
}

/// Parsed collisions by `hkx` id (`None`: not found or not readable).
#[derive(Default)]
pub struct Cache {
    models: Mutex<HashMap<u32, Option<Arc<Model>>>>,
}

impl Cache {
    pub fn clear(&self) {
        self.models
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    fn get(&self, id: u32) -> Option<Option<Arc<Model>>> {
        self.models
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .cloned()
    }
}

fn model(source: &LevelSource, id: u32) -> Option<Arc<Model>> {
    let physics = source.physics(id).ok()?;
    let parts = physics_parts(&physics, &Affine::IDENTITY);
    let bounds = parts
        .iter()
        .map(|(pose, shape)| shape.compute_aabb(pose))
        .reduce(|a, b| merged(&a, &b));
    Some(Arc::new(Model {
        parts: Arc::new(parts),
        bounds,
    }))
}

fn merged(a: &Aabb, b: &Aabb) -> Aabb {
    Aabb::new(a.mins.min(b.mins), a.maxs.max(b.maxs))
}

/// Corners of `b` mapped by `at`, as a box.
fn placed_bounds(b: &Aabb, at: &Affine) -> Aabb {
    let mut lo = Vector::splat(f32::MAX);
    let mut hi = Vector::splat(f32::MIN);
    for i in 0..8 {
        let c = [
            if i & 1 == 0 { b.mins.x } else { b.maxs.x },
            if i & 2 == 0 { b.mins.y } else { b.maxs.y },
            if i & 4 == 0 { b.mins.z } else { b.maxs.z },
        ];
        let p = at.apply(c);
        let p = Vector::new(p[0], p[1], p[2]);
        lo = lo.min(p);
        hi = hi.max(p);
    }
    Aabb::new(lo, hi)
}

/// Only collision whose bounds come within `radius` metres of `centre`
/// (horizontally) is built.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub centre: V3,
    pub radius: f32,
}

impl Area {
    fn reaches(&self, b: &Aabb) -> bool {
        let dx = (b.mins.x - self.centre[0])
            .max(self.centre[0] - b.maxs.x)
            .max(0.0);
        let dz = (b.mins.z - self.centre[2])
            .max(self.centre[2] - b.maxs.z)
            .max(0.0);
        dx * dx + dz * dz <= self.radius * self.radius
    }
}

/// Counts and times of a build.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    /// Collisions of placed objects.
    pub placements: usize,
    /// Placed in the world.
    pub built: usize,
    /// Outside the area.
    pub far: usize,
    /// With a scale (rebuilt with it, not shared).
    pub scaled: usize,
    /// `hkx` parsed by this build and found in the cache.
    pub parsed: usize,
    pub cached: usize,
    /// `hkx` not found or not readable.
    pub failed: usize,
    pub colliders: usize,
    pub parse_time: Duration,
    pub world_time: Duration,
}

/// Collision placements of one level: the loaded one (layer 0) or the
/// ground zones loaded for walking only (layer 1).
pub struct Layer<'a> {
    pub source: &'a LevelSource,
    pub placements: &'a [PlacedCollision],
}

/// Builds the world from the collision placements of the layers.
pub fn build(layers: &[Layer], area: Option<Area>, cache: &Cache) -> (World, Stats) {
    let started = Instant::now();
    let mut stats = Stats {
        placements: layers.iter().map(|l| l.placements.len()).sum(),
        ..Stats::default()
    };
    // Workspace collisions change with edits: never cached.
    let mut wanted: Vec<(u32, usize)> = layers
        .iter()
        .enumerate()
        .flat_map(|(i, l)| l.placements.iter().map(move |p| (p.hkx, i)))
        .filter(|&(id, i)| layers[i].source.collision_overridden(id) || cache.get(id).is_none())
        .collect();
    wanted.sort_unstable();
    wanted.dedup_by_key(|w| w.0);
    stats.parsed = wanted.len();
    let parsed: Vec<(u32, bool, Option<Arc<Model>>)> = wanted
        .par_iter()
        .map(|&(id, i)| {
            let source = layers[i].source;
            (id, source.collision_overridden(id), model(source, id))
        })
        .collect();
    let mut models: HashMap<u32, Option<Arc<Model>>> = HashMap::new();
    {
        let mut shared = cache.models.lock().unwrap_or_else(|e| e.into_inner());
        for (id, overridden, m) in parsed {
            if !overridden {
                shared.insert(id, m.clone());
            }
            models.insert(id, m);
        }
        for p in layers.iter().flat_map(|l| l.placements) {
            models
                .entry(p.hkx)
                .or_insert_with(|| shared.get(&p.hkx).cloned().flatten());
        }
    }
    stats.cached = models.len() - stats.parsed.min(models.len());
    stats.failed = models.values().filter(|m| m.is_none()).count();
    stats.parse_time = started.elapsed();

    let started = Instant::now();
    let mut placed = Vec::new();
    for (layer, l) in layers.iter().enumerate() {
        for p in l.placements {
            let Some(Some(model)) = models.get(&p.hkx) else {
                continue;
            };
            let at = Affine::from_rows(&p.matrix);
            if let (Some(area), Some(b)) = (area, &model.bounds)
                && !area.reaches(&placed_bounds(b, &at))
            {
                stats.far += 1;
                continue;
            }
            let owner = Owner {
                layer,
                object: (p.group, p.object),
                hkx: p.hkx,
            };
            match at.rigid() {
                Some(pose) => placed.push(Placed {
                    owner,
                    parts: model.parts.clone(),
                    pose,
                }),
                None => {
                    stats.scaled += 1;
                    let Ok(physics) = l.source.physics(p.hkx) else {
                        continue;
                    };
                    placed.push(Placed {
                        owner,
                        parts: Arc::new(physics_parts(&physics, &at)),
                        pose: Pose::IDENTITY,
                    });
                }
            }
        }
    }
    stats.built = placed.len();
    let world = World::new(&placed);
    stats.colliders = world.collider_count();
    stats.world_time = started.elapsed();
    (world, stats)
}

/// The district of a zone: the path before `/high/`, `/medium/`, `/low/`
/// or `/interior/` (`gf2/miami/local/bonita`).
fn district(name: &str) -> Option<&str> {
    ["/high/", "/medium/", "/low/", "/interior/"]
        .iter()
        .find_map(|part| name.find(part).map(|i| &name[..i]))
}

/// Zones that hold the ground of the loaded ones: the `medium` zones of
/// their districts that are not loaded. The game keeps the terrain
/// collision of a district in its `medium` zones (the `high` terrain zones
/// have only the collision of props), so walking on `high` zones alone
/// would fall through the streets.
pub fn ground_zones<'a>(
    zones: impl IntoIterator<Item = (usize, &'a str)> + Clone,
    loaded: &[usize],
) -> Vec<usize> {
    let mut districts: Vec<&str> = zones
        .clone()
        .into_iter()
        .filter(|(s, _)| loaded.contains(s))
        .filter_map(|(_, name)| district(name))
        .collect();
    districts.sort_unstable();
    districts.dedup();
    zones
        .into_iter()
        .filter(|(s, _)| !loaded.contains(s))
        .filter(|(_, name)| {
            district(name).is_some_and(|d| districts.contains(&d))
                && name[district(name).map_or(0, str::len)..].starts_with("/medium/")
        })
        .map(|(s, _)| s)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level_view::walk::physics::{BodySize, Player};
    use doctor_toolkit::game::GameInstance;
    use doctor_toolkit::level::{Catalog, Level, Lookup, Overrides, Scope};

    #[test]
    fn ground_comes_from_the_medium_zones_of_the_districts() {
        let zones = [
            (0, "gf2/miami/local/bonita/high/terrain06"),
            (1, "gf2/miami/local/bonita/medium/zone1"),
            (2, "gf2/miami/local/bonita/medium/zone4"),
            (3, "gf2/miami/local/bonita/low/zone1"),
            (4, "gf2/miami/local/bebe/medium/zone2"),
            (5, "gf2/miami/local/bonita/interior/venue_1"),
            (6, "gf2/miami/global/objectives/x"),
        ];
        assert_eq!(ground_zones(zones, &[0]), vec![1, 2]);
        assert_eq!(ground_zones(zones, &[5, 1]), vec![2]);
        assert!(ground_zones(zones, &[6]).is_empty());
        assert!(ground_zones(zones, &[1, 2]).is_empty());
    }

    /// Loads `streams` of a game like the level view.
    fn load(game: &Arc<GameInstance>, lookup: &Arc<Lookup>, streams: &[usize]) -> Level {
        load_with(game, lookup, streams, Overrides::default())
    }

    fn load_with(
        game: &Arc<GameInstance>,
        lookup: &Arc<Lookup>,
        streams: &[usize],
        overrides: Overrides,
    ) -> Level {
        let parents = game.stream_parents().unwrap();
        let scope = Scope::new(game, &parents, streams);
        let source = Arc::new(LevelSource::new(
            game.clone(),
            lookup.clone(),
            overrides,
            scope,
        ));
        Level::load(source, streams).unwrap()
    }

    /// An object copied in the workspace (an edited SimGroup, not applied)
    /// and lifted 40 m takes part in the walk: it is listed as the user's,
    /// and its collision stands where the copy is.
    #[test]
    fn corpus_workspace_objects_take_part() {
        use crate::level_view::walk::own_objects;

        let Some(dir) = std::env::var_os("EARS_GF2_DIR") else {
            return;
        };
        let game = Arc::new(GameInstance::open(dir).unwrap());
        let (index, _) = game.build_index(|_| {});
        let catalog = Catalog::new(&game, &index);
        let lookup = Arc::new(Lookup::new(&index));
        let streams: Vec<usize> = catalog
            .under("gf2/miami/local/bonita/high")
            .map(|z| z.stream)
            .collect();
        let level = load(&game, &lookup, &streams);
        assert!(own_objects(&level).is_empty());
        // A prop with collision (not terrain).
        let p = level
            .collision_placements()
            .into_iter()
            .find(|p| {
                let o = &level.groups[p.group].objects[p.object];
                o.has_matrix && o.class_name != Some("ZoneRenderLOD")
            })
            .unwrap();
        let group = &level.groups[p.group];
        let mut edited = group.group.clone();
        let guids = level.guids();
        let guid = doctor_toolkit::resource::simgroup_edit::new_object_guid("walk test", |g| {
            guids.contains(g)
        });
        let copy = edited.duplicate_object(p.object, guid).unwrap();
        let mut m = group.objects[p.object].transform.unwrap();
        m[3][1] += 40.0;
        edited.set_transform(copy, m).unwrap();
        let mut overrides = Overrides::default();
        overrides
            .groups
            .insert(group.location, Arc::new(edited.encode()));
        let level = load_with(&game, &lookup, &streams, overrides);
        let own = own_objects(&level);
        assert_eq!(own.len(), 1, "{own:?}");
        assert!(own[0].new && own[0].collisions > 0, "{own:?}");
        assert_eq!(own[0].object, (p.group, copy));

        let placements = level.collision_placements();
        let (world, _) = build(
            &[Layer {
                source: &level.source,
                placements: &placements,
            }],
            None,
            &Cache::default(),
        );
        // The copy's collision is 40 m above the original's.
        let original = world.object_bounds(0, (p.group, p.object)).unwrap();
        let lifted = world.object_bounds(0, (p.group, copy)).unwrap();
        assert!((lifted.mins.y - original.mins.y - 40.0).abs() < 1e-2);
        assert!((lifted.maxs.x - original.maxs.x).abs() < 1e-2);
        // Straight down from high above everything: a 5 × 5 grid over the
        // copy finds it.
        let b = world.bounds.unwrap();
        let mut hit = 0;
        for i in 0..25 {
            let t = |lo: f32, hi: f32, k: usize| lo + (hi - lo) * (k as f32 + 0.5) / 5.0;
            let at = [
                t(lifted.mins.x, lifted.maxs.x, i % 5),
                b.maxs.y + 10.0,
                t(lifted.mins.z, lifted.maxs.z, i / 5),
            ];
            if let Some((_, Some(owner))) = world.below(at, b.maxs.y - b.mins.y + 20.0)
                && owner.object == (p.group, copy)
            {
                hit += 1;
            }
        }
        eprintln!(
            "copy of {:?} lifted 40 m: {hit} of 25 rays from above hit it",
            level.groups[p.group].objects[p.object].name
        );
        assert!(hit > 0);
    }

    /// The collision of zones (`DOCTOR_TOOLKIT_WALK_ZONES`, by default the
    /// `high` zones of a Miami district) with the ground of their medium
    /// zones loads, is cached, and holds the player up.
    #[test]
    fn corpus_zone_collision_holds_the_player() {
        let Some(dir) = std::env::var_os("EARS_GF2_DIR") else {
            return;
        };
        let zones = std::env::var("DOCTOR_TOOLKIT_WALK_ZONES")
            .unwrap_or_else(|_| "gf2/miami/local/bonita/high".into());
        let game = Arc::new(GameInstance::open(dir).unwrap());
        let (index, _) = game.build_index(|_| {});
        let catalog = Catalog::new(&game, &index);
        let lookup = Arc::new(Lookup::new(&index));
        let streams: Vec<usize> = zones
            .split('+')
            .flat_map(|z| catalog.under(z).map(|z| z.stream).collect::<Vec<_>>())
            .collect();
        assert!(!streams.is_empty(), "no zone at {zones}");
        let started = Instant::now();
        let level = load(&game, &lookup, &streams);
        let ground_streams = ground_zones(
            catalog.zones.iter().map(|z| (z.stream, z.name.as_str())),
            &streams,
        );
        let ground = load(&game, &lookup, &ground_streams);
        let loaded = started.elapsed();
        let placements = level.collision_placements();
        let ground_placements = ground.collision_placements();
        let layers = [
            Layer {
                source: &level.source,
                placements: &placements,
            },
            Layer {
                source: &ground.source,
                placements: &ground_placements,
            },
        ];
        let cache = Cache::default();
        let started = Instant::now();
        let (world, stats) = build(&layers, None, &cache);
        let first = started.elapsed();
        let started = Instant::now();
        let (_, again) = build(&layers, None, &cache);
        let second = started.elapsed();
        eprintln!(
            "{zones}: {} zone(s) and {} ground zone(s) read in {loaded:?}; {stats:?}; \
             first build {first:?}, cached {second:?} ({again:?})",
            streams.len(),
            ground_streams.len()
        );
        assert!(stats.built > 0 && stats.failed == 0, "{stats:?}");
        assert_eq!(again.parsed, 0);
        assert_eq!(again.colliders, stats.colliders);
        // Within 100 m of a point only part of it is built.
        let first = level
            .groups
            .iter()
            .flat_map(|g| &g.objects)
            .find(|o| o.has_matrix)
            .and_then(|o| o.transform)
            .unwrap();
        let centre = [first[3][0], first[3][1], first[3][2]];
        let area = Area {
            centre,
            radius: 100.0,
        };
        let (_, near) = build(&layers, Some(area), &cache);
        eprintln!("within 100 m of {centre:?}: {near:?}");
        assert!(near.far > 0 && near.built + near.far == stats.built);

        // Ground under the placed objects that have meshes (not terrain),
        // with and without the medium zones.
        let (alone, _) = build(&layers[..1], None, &cache);
        let mut tried = 0;
        let mut found = 0;
        let mut found_alone = 0;
        for g in &level.groups {
            for o in &g.objects {
                let (Some(m), true) = (o.transform, o.has_matrix) else {
                    continue;
                };
                let Some(model) = o.model.and_then(|id| level.models.get(&id)) else {
                    continue;
                };
                if model.desc.pieces.is_empty() {
                    continue;
                }
                tried += 1;
                let at = [m[3][0], m[3][1] + 20.0, m[3][2]];
                found += usize::from(world.below(at, 60.0).is_some());
                found_alone += usize::from(alone.below(at, 60.0).is_some());
            }
        }
        eprintln!(
            "ground under {found} of {tried} objects with meshes \
             ({found_alone} without the ground zones)"
        );
        assert!(found * 10 >= tried * 9, "{found} of {tried}");

        // The player dropped at some of these lands and stays up.
        let body = BodySize::default();
        let mut dropped = 0;
        let mut landed = 0;
        let started = Instant::now();
        for g in level.groups.iter().take(20) {
            for o in g.objects.iter().filter(|o| o.has_matrix).take(3) {
                let m = o.transform.unwrap();
                let top = [m[3][0], m[3][1] + 20.0, m[3][2]];
                let Some((toi, _)) = world.below(top, 60.0) else {
                    continue;
                };
                dropped += 1;
                let mut player = Player::new([top[0], top[1] - toi + 1.0, top[2]]);
                for _ in 0..120 {
                    player.step(&world, &body, [0.0, 0.0], false, 1.0 / 60.0);
                }
                landed += usize::from(player.grounded);
            }
        }
        eprintln!(
            "dropped {dropped}, landed {landed}, {:?} per 2 s of simulation",
            started.elapsed() / dropped.max(1) as u32
        );
        assert!(landed * 10 >= dropped * 9, "{landed} of {dropped}");
    }
}
