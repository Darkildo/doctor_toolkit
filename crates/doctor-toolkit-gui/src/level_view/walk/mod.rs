//! Walking a level (stage 37): a first-person capsule with the Havok
//! collision of the loaded zones, gravity, steps and jumps, without the game.
//!
//! - [`physics`]: the shapes as `rapier3d` colliders, the character
//!   controller, the straight walk from A to B;
//! - [`build`]: the collision of a level (and the ground of its districts)
//!   placed in the world, parsed once per `hkx`.
//!
//! The level view owns a [`Walk`]: [`Walk::panel`] in the browser,
//! [`Walk::update`] instead of the fly camera while walking, [`Walk::lines`]
//! and [`Walk::hud`] over the scene. See `docs/levels.md` §11.

#[cfg(test)]
mod build_tests;

// The collision world and the player are the library's (the MCP server
// uses them too).
pub use doctor_toolkit::level::walk::{build, physics};

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use doctor_toolkit::level::{Catalog, Level, LevelGroup, LevelSource, RowMatrix, Scope};
use doctor_toolkit::resource::simgroup::{Guid, SimGroup};
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::RichText;

use super::camera::FlyCamera;
use crate::render::LineVertex;
use build::{Area, Cache, Layer, Stats};
use physics::{BodySize, Outcome, Player, V3, World};

/// Parents of streams in the tables of contents.
pub type Parents = Arc<HashMap<String, Vec<String>>>;

/// The collision world of a level and what it was built from.
pub struct Built {
    pub world: World,
    /// The ground zones loaded for walking (layer 1).
    pub ground: Option<Arc<Level>>,
    ground_streams: Vec<usize>,
    pub stats: Stats,
    pub elapsed: Duration,
    /// The level source it was built for (its address).
    key: usize,
    area: Option<Area>,
}

/// Where the walk found a problem.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MarkKind {
    /// The player pushed on but did not move.
    Stuck,
    /// The player fell below all collision.
    Fell,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mark {
    pub at: V3,
    pub kind: MarkKind,
}

/// An object of the workspace (new, moved or with another model).
#[derive(Debug, Clone, PartialEq)]
pub struct OwnObject {
    pub object: (usize, usize),
    pub label: String,
    pub position: V3,
    /// Collision placements of its model.
    pub collisions: usize,
    pub new: bool,
}

#[derive(Default)]
struct Route {
    /// Check the route once the world is built.
    pending: bool,
    a: Option<V3>,
    b: Option<V3>,
    result: Option<(Outcome, Vec<V3>)>,
    job: Option<Receiver<(Outcome, Vec<V3>)>>,
}

/// What [`Walk::panel`] asks the view to do.
pub enum PanelAction {
    /// Start walking at the camera.
    EnterAtCamera,
    /// Start walking at the selected object.
    EnterAtSelected,
}

pub struct Walk {
    pub active: bool,
    /// Move through walls, no gravity (for debugging).
    pub noclip: bool,
    /// Load the `medium` zones of the districts for their ground.
    pub ground: bool,
    /// Build only collision within this many metres (0: everything).
    pub radius: f32,
    pub body: BodySize,
    player: Player,
    built: Option<Arc<Built>>,
    job: Option<Receiver<Result<Built, String>>>,
    /// Where to start once the world is built: the point and whether it
    /// is a point above which to look for the ground.
    enter_at: Option<V3>,
    cache: Arc<Cache>,
    pub marks: Vec<Mark>,
    route: Route,
    own: Vec<OwnObject>,
    own_set: HashSet<(usize, usize)>,
    /// Seconds without progress while pushing.
    stall: f32,
    last_ground: V3,
    status: String,
    /// Changes of [`Self::lines`].
    pub revision: u64,
    /// The level changed since the world was built.
    stale: bool,
    /// Woken by the build threads.
    ctx: Option<egui::Context>,
}

impl Default for Walk {
    fn default() -> Self {
        Self {
            active: false,
            noclip: false,
            ground: true,
            radius: 0.0,
            body: BodySize::default(),
            player: Player::new([0.0; 3]),
            built: None,
            job: None,
            enter_at: None,
            cache: Arc::new(Cache::default()),
            marks: Vec::new(),
            route: Route::default(),
            own: Vec::new(),
            own_set: HashSet::new(),
            stall: 0.0,
            last_ground: [0.0; 3],
            status: String::new(),
            revision: 0,
            stale: false,
            ctx: None,
        }
    }
}

/// `Class name` of an object of a level.
fn label(level: &Level, (g, o): (usize, usize)) -> String {
    level
        .groups
        .get(g)
        .and_then(|g| g.objects.get(o))
        .map_or_else(String::new, |o| {
            format!("{} {}", o.class_name.unwrap_or("?"), o.name)
        })
}

fn position(m: &RowMatrix) -> V3 {
    [m[3][0], m[3][1], m[3][2]]
}

/// Objects of edited SimGroups that are new or differ from the game files
/// in position or model.
pub fn own_objects(level: &Level) -> Vec<OwnObject> {
    let placements = level.collision_placements();
    let mut collisions: HashMap<(usize, usize), usize> = HashMap::new();
    for p in &placements {
        *collisions.entry((p.group, p.object)).or_default() += 1;
    }
    let mut out = Vec::new();
    for (g, group) in level.groups.iter().enumerate() {
        if !level.source.overrides.groups.contains_key(&group.location) {
            continue;
        }
        let original = level
            .source
            .stream(group.location.stream)
            .and_then(|s| s.resource_data(group.location.resource))
            .and_then(|d| SimGroup::parse(&d));
        let before: HashMap<Guid, (Option<RowMatrix>, Option<u32>)> = match original {
            Ok(o) => LevelGroup::new(group.location, o)
                .objects
                .iter()
                .map(|o| (o.guid, (o.transform, o.model)))
                .collect(),
            Err(_) => HashMap::new(),
        };
        for (i, o) in group.objects.iter().enumerate() {
            let Some(m) = o.transform else { continue };
            let new = !before.contains_key(&o.guid);
            if !new && before[&o.guid] == (o.transform, o.model) {
                continue;
            }
            out.push(OwnObject {
                object: (g, i),
                label: label(level, (g, i)),
                position: position(&m),
                collisions: collisions.get(&(g, i)).copied().unwrap_or(0),
                new,
            });
        }
    }
    out
}

impl Walk {
    pub fn busy(&self) -> bool {
        self.job.is_some()
            || self.route.job.is_some()
            || self.route.pending
            || self.enter_at.is_some()
    }

    /// The level was loaded again or edited: the world is rebuilt (the old
    /// one is used until then).
    pub fn level_changed(&mut self) {
        self.stale = true;
    }

    /// The level was unloaded or another one loaded.
    pub fn reset(&mut self) {
        let keep = (self.noclip, self.ground, self.radius, self.body);
        *self = Self {
            cache: self.cache.clone(),
            ..Self::default()
        };
        (self.noclip, self.ground, self.radius, self.body) = keep;
    }

    /// The game files changed: parsed collisions may be stale.
    pub fn forget_cache(&mut self) {
        self.cache.clear();
    }

    /// Whether the collision of `level` is built and current.
    pub fn ready_for(&self, level: &Level) -> bool {
        !self.stale
            && self
                .built
                .as_ref()
                .is_some_and(|b| b.key == Arc::as_ptr(&level.source) as usize)
    }

    /// Builds the collision of `level` around `centre` when it is not
    /// built (stage 54: End puts objects on the ground). Progress comes
    /// with [`Self::poll_build`].
    pub fn prepare(&mut self, level: &Level, catalog: &Catalog, parents: &Parents, centre: V3) {
        if !self.ready_for(level) && self.job.is_none() {
            self.start_build(level, catalog, parents, centre);
        }
    }

    /// Whether the collision is being built.
    pub fn building(&self) -> bool {
        self.job.is_some()
    }

    /// Takes a finished build (also while not walking).
    pub fn poll_build(&mut self) {
        self.poll();
    }

    /// The height of the ground below `from` (within 2 km), passing over
    /// the colliders of the level objects `skip` names; `None` without a
    /// built world or ground.
    pub fn ground_below(&self, from: V3, skip: &dyn Fn((usize, usize)) -> bool) -> Option<f32> {
        let world = &self.built.as_ref()?.world;
        let pass = |o: physics::Owner| o.layer == 0 && skip(o.object);
        world
            .below_except(from, 2000.0, &pass)
            .map(|toi| from[1] - toi)
    }

    fn start_build(&mut self, level: &Level, catalog: &Catalog, parents: &Parents, centre: V3) {
        let (tx, rx) = channel();
        let source = level.source.clone();
        let key = Arc::as_ptr(&source) as usize;
        let placements = level.collision_placements();
        let ground_streams = if self.ground {
            build::ground_zones(
                catalog.zones.iter().map(|z| (z.stream, z.name.as_str())),
                &level.streams,
            )
        } else {
            Vec::new()
        };
        // The ground zones are loaded once for the same set of streams.
        let previous = self
            .built
            .as_ref()
            .filter(|b| b.ground_streams == ground_streams)
            .and_then(|b| b.ground.clone());
        let area = (self.radius > 0.0).then_some(Area {
            centre,
            radius: self.radius,
        });
        let parents = parents.clone();
        let cache = self.cache.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let started = Instant::now();
            let ground = match previous {
                Some(g) => Some(g),
                None if ground_streams.is_empty() => None,
                None => {
                    let scope = Scope::new(&source.game, &parents, &ground_streams);
                    let ground_source = Arc::new(LevelSource::new(
                        source.game.clone(),
                        source.lookup.clone(),
                        source.overrides.clone(),
                        scope,
                    ));
                    match Level::load(ground_source, &ground_streams) {
                        Ok(level) => Some(Arc::new(level)),
                        Err(error) => {
                            let _ = tx.send(Err(error.to_string()));
                            return;
                        }
                    }
                }
            };
            let ground_placements = ground
                .as_ref()
                .map(|g| g.collision_placements())
                .unwrap_or_default();
            let mut layers = vec![Layer {
                source: &source,
                placements: &placements,
            }];
            if let Some(g) = &ground {
                layers.push(Layer {
                    source: &g.source,
                    placements: &ground_placements,
                });
            }
            let (world, stats) = build::build(&layers, area, &cache);
            let _ = tx.send(Ok(Built {
                world,
                ground,
                ground_streams,
                stats,
                elapsed: started.elapsed(),
                key,
                area,
            }));
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
        self.job = Some(rx);
        self.stale = false;
        self.own = own_objects(level);
        self.own_set = self.own.iter().map(|o| o.object).collect();
        self.status = tr("walk.building").into();
    }

    fn poll(&mut self) {
        if let Some(rx) = &self.job
            && let Ok(result) = rx.try_recv()
        {
            self.job = None;
            match result {
                Ok(built) => {
                    eprintln!(
                        "walk: collision built in {:.0} ms: {:?}",
                        built.elapsed.as_secs_f64() * 1000.0,
                        built.stats
                    );
                    self.status = trf(
                        "walk.built",
                        &[
                            ("objects", &built.stats.built),
                            ("colliders", &built.stats.colliders),
                            (
                                "ground",
                                &built.ground.as_ref().map_or(0, |g| g.streams.len()),
                            ),
                            ("ms", &built.elapsed.as_millis()),
                        ],
                    );
                    self.built = Some(Arc::new(built));
                    self.revision += 1;
                }
                Err(error) => self.status = trf("walk.cannot_build", &[("error", &error)]),
            }
        }
        if self.route.pending && self.built.is_some() && self.job.is_none() {
            self.route.pending = false;
            self.check_route();
        }
        if let Some(rx) = &self.route.job
            && let Ok((outcome, path)) = rx.try_recv()
        {
            self.route.job = None;
            match outcome {
                Outcome::Stuck(at) => self.mark(at, MarkKind::Stuck),
                Outcome::Fell(at) => self.mark(at, MarkKind::Fell),
                _ => {}
            }
            eprintln!("walk: route {outcome:?}, {} points", path.len());
            self.route.result = Some((outcome, path));
            self.revision += 1;
        }
    }

    fn mark(&mut self, at: V3, kind: MarkKind) {
        let near = self.marks.iter().any(|m| {
            m.kind == kind
                && (0..3)
                    .map(|i| (m.at[i] - at[i]).powi(2))
                    .sum::<f32>()
                    .sqrt()
                    < 1.0
        });
        if !near {
            eprintln!("walk: {kind:?} at {at:?}");
            self.marks.push(Mark { at, kind });
            self.revision += 1;
        }
    }

    /// The collision world for placing objects dropped into the view (stage
    /// 55): built in the background when there is none for this level or it
    /// is stale; the last one built is used meanwhile.
    pub fn world_for(
        &mut self,
        level: &Level,
        catalog: &Catalog,
        parents: &Parents,
        centre: V3,
    ) -> Option<&World> {
        self.poll();
        let fresh = self
            .built
            .as_ref()
            .is_some_and(|b| b.key == Arc::as_ptr(&level.source) as usize);
        if (!fresh || self.stale) && self.job.is_none() {
            self.start_build(level, catalog, parents, centre);
        }
        self.built.as_ref().map(|b| &b.world)
    }

    /// Starts walking at `at` (feet are put on the ground below it, or
    /// above it when there is nothing below).
    pub fn enter(&mut self, at: V3, level: &Level, catalog: &Catalog, parents: &Parents) {
        self.active = true;
        self.enter_at = Some(at);
        let fresh = self
            .built
            .as_ref()
            .is_some_and(|b| b.key == Arc::as_ptr(&level.source) as usize);
        if (!fresh || self.stale) && self.job.is_none() {
            self.start_build(level, catalog, parents, at);
        }
    }

    pub fn exit(&mut self) {
        self.active = false;
        self.enter_at = None;
        self.revision += 1;
    }

    fn place(&mut self, at: V3) {
        let Some(built) = &self.built else { return };
        let world = &built.world;
        let feet = world
            .below([at[0], at[1] + 0.5, at[2]], 2000.0)
            .map(|(toi, _)| [at[0], at[1] + 0.5 - toi, at[2]])
            .or_else(|| {
                world
                    .below([at[0], at[1] + 1000.0, at[2]], 3000.0)
                    .map(|(toi, _)| [at[0], at[1] + 1000.0 - toi, at[2]])
            });
        match feet {
            Some(feet) => {
                // On a kerb or a slope the capsule may touch what is beside
                // the point: lift it by up to a step.
                let lift = [0.05, 0.15, 0.3, self.body.step.max(0.3)]
                    .into_iter()
                    .find(|&h| {
                        world
                            .overlapping(&self.body, [feet[0], feet[1] + h, feet[2]])
                            .is_none()
                    })
                    .unwrap_or(0.05);
                self.player = Player::new([feet[0], feet[1] + lift, feet[2]]);
                self.last_ground = self.player.feet;
                if let Some(owner) = world.overlapping(&self.body, self.player.feet) {
                    let what = match (owner.layer, &built.ground) {
                        (1, Some(g)) => label(g, owner.object),
                        _ => String::new(),
                    };
                    let what = format!("{what} hkx {:08x}", owner.hkx);
                    eprintln!("walk: the start at {:?} overlaps {what}", self.player.feet);
                    self.status = trf("walk.start_inside", &[("what", &what)]);
                }
            }
            None => {
                self.player = Player::new(at);
                self.last_ground = at;
                self.status = tr("walk.no_ground").into();
            }
        }
        self.stall = 0.0;
    }

    /// Input and physics of one frame; moves `camera` to the eyes. Returns
    /// whether the camera moved.
    pub fn update(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        camera: &mut FlyCamera,
        level: &Level,
        catalog: &Catalog,
        parents: &Parents,
    ) -> bool {
        self.ctx = Some(ui.ctx().clone());
        self.poll();
        if !self.active {
            return false;
        }
        let built_for_level = self
            .built
            .as_ref()
            .is_some_and(|b| b.key == Arc::as_ptr(&level.source) as usize);
        let far = self.built.as_ref().and_then(|b| b.area).is_some_and(|a| {
            let d = [
                self.player.feet[0] - a.centre[0],
                self.player.feet[2] - a.centre[2],
            ];
            (d[0] * d[0] + d[1] * d[1]).sqrt() > a.radius / 2.0
        });
        if (self.stale || !built_for_level || far) && self.job.is_none() {
            let centre = if self.enter_at.is_some() || self.built.is_none() {
                self.enter_at.unwrap_or(camera.eye)
            } else {
                self.player.feet
            };
            self.start_build(level, catalog, parents, centre);
        }
        if self.built.is_none() {
            ui.ctx().request_repaint_after(Duration::from_millis(50));
            return false;
        }
        self.drive(ui, response, camera)
    }

    /// Input and physics of one frame on the built world.
    fn drive(&mut self, ui: &egui::Ui, response: &egui::Response, camera: &mut FlyCamera) -> bool {
        if let Some(at) = self.enter_at.take() {
            self.place(at);
        }
        let before = *camera;
        let (dt, keys, run, jump, escape, toggle, up) = ui.input(|i| {
            use egui::Key;
            let k = |key| i.key_down(key) as i32 as f32;
            (
                i.stable_dt.min(0.1),
                [
                    k(Key::W) + k(Key::ArrowUp) - k(Key::S) - k(Key::ArrowDown),
                    k(Key::D) + k(Key::ArrowRight) - k(Key::A) - k(Key::ArrowLeft),
                ],
                i.modifiers.shift,
                i.key_pressed(Key::Space),
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::N),
                k(Key::E) - k(Key::Q),
            )
        });
        let keyboard =
            (response.hovered() || response.has_focus()) && !ui.ctx().egui_wants_keyboard_input();
        if keyboard && escape {
            self.exit();
            return false;
        }
        if keyboard && toggle {
            self.noclip = !self.noclip;
            self.player.vertical = 0.0;
        }
        // Look: drag with any button.
        if response.dragged() {
            let delta = response.drag_delta();
            camera.yaw -= delta.x * 0.005;
            camera.pitch = (camera.pitch - delta.y * 0.005).clamp(-1.5, 1.5);
        }
        let keys = if keyboard { keys } else { [0.0; 2] };
        let speed = if run {
            self.body.run_speed
        } else {
            self.body.walk_speed
        };
        let (sy, cy) = camera.yaw.sin_cos();
        let forward = [-sy, -cy];
        let right = [cy, -sy];
        let mut wish = [
            forward[0] * keys[0] + right[0] * keys[1],
            forward[1] * keys[0] + right[1] * keys[1],
        ];
        let len = (wish[0] * wish[0] + wish[1] * wish[1]).sqrt();
        if len > 0.0 {
            wish = [wish[0] / len * speed, wish[1] / len * speed];
        }
        let built = self.built.clone().expect("built");
        if self.noclip {
            let f = camera.forward();
            let k = if run { 4.0 } else { 1.0 } * self.body.run_speed * dt;
            let up = if keyboard { up } else { 0.0 };
            let r = camera.right();
            for ((feet, f), r) in self.player.feet.iter_mut().zip(f).zip(r) {
                *feet += (f * keys[0] + r * keys[1]) * k;
            }
            self.player.feet[1] += up * k;
            self.player.vertical = 0.0;
        } else {
            let step = self
                .player
                .step(&built.world, &self.body, wish, keyboard && jump, dt);
            // Stuck: pushing on the ground without moving.
            if step.wanted > 1e-4 && self.player.grounded {
                if self.stall < 0.0 || step.made < step.wanted * 0.1 {
                    // Counting up, or cooling down after a mark.
                    self.stall += dt;
                } else {
                    self.stall = 0.0;
                }
                if self.stall > 0.75 {
                    self.mark(self.player.feet, MarkKind::Stuck);
                    self.stall = -2.0;
                }
            } else if self.stall > 0.0 {
                self.stall = 0.0;
            }
            if self.player.grounded {
                self.last_ground = self.player.feet;
            }
            // Fell through: below everything, or long in the air with
            // nothing below.
            let floor = built.world.bounds.map_or(-1e4, |b| b.mins.y) - 20.0;
            let nothing_below = self.player.airborne > 3.0
                && built.world.below(self.player.feet, 10_000.0).is_none();
            if self.player.feet[1] < floor || nothing_below {
                self.mark(self.last_ground, MarkKind::Fell);
                let back = self.last_ground;
                self.player = Player::new([back[0], back[1] + 0.5, back[2]]);
            }
        }
        let feet = self.player.feet;
        camera.eye = [feet[0], feet[1] + self.body.eye, feet[2]];
        ui.ctx().request_repaint();
        *camera != before
    }

    /// The thing straight below the feet: height above it and its label.
    fn under_feet(&self, level: Option<&Level>) -> Option<(f32, String, bool)> {
        let built = self.built.as_ref()?;
        let f = self.player.feet;
        let (toi, owner) = built.world.below([f[0], f[1] + 0.1, f[2]], 1000.0)?;
        let owner = owner?;
        let (text, own) = match owner.layer {
            0 => (
                level.map_or_else(String::new, |l| label(l, owner.object)),
                self.own_set.contains(&owner.object),
            ),
            _ => (
                built
                    .ground
                    .as_ref()
                    .map_or_else(String::new, |g| label(g, owner.object)),
                false,
            ),
        };
        Some((toi - 0.1, format!("{text} (hkx {:08x})", owner.hkx), own))
    }

    /// Text over the scene while walking.
    pub fn hud(&self, level: &Level) -> String {
        self.hud_text(Some(level))
    }

    fn hud_text(&self, level: Option<&Level>) -> String {
        let f = self.player.feet;
        let mut text = trf(
            "walk.hud_position",
            &[("position", &format!("{:.2} {:.2} {:.2}", f[0], f[1], f[2]))],
        );
        match self.under_feet(level) {
            Some((height, what, own)) => {
                text += &trf(
                    "walk.hud_ground",
                    &[("height", &format!("{height:.2}")), ("what", &what)],
                );
                if own {
                    text += tr("walk.hud_own");
                }
            }
            None => text += tr("walk.hud_no_ground"),
        }
        text += "\n";
        text += if self.noclip {
            tr("walk.hud_noclip")
        } else if self.player.grounded {
            tr("walk.hud_grounded")
        } else {
            tr("walk.hud_air")
        };
        if self.job.is_some() || self.built.is_none() {
            text += " · ";
            text += tr("walk.building");
        }
        text += "\n";
        text += tr("walk.hud_keys");
        text
    }

    /// Marks, the route and its result as lines.
    pub fn lines(&self) -> Vec<LineVertex> {
        let mut out = Vec::new();
        let post = |out: &mut Vec<LineVertex>, at: V3, height: f32, color: [u8; 4]| {
            let r = 0.4;
            out.push(LineVertex::new(at, color));
            out.push(LineVertex::new([at[0], at[1] + height, at[2]], color));
            for (dx, dz) in [(r, r), (r, -r)] {
                out.push(LineVertex::new(
                    [at[0] - dx, at[1] + 0.02, at[2] - dz],
                    color,
                ));
                out.push(LineVertex::new(
                    [at[0] + dx, at[1] + 0.02, at[2] + dz],
                    color,
                ));
            }
        };
        for m in &self.marks {
            let color = match m.kind {
                MarkKind::Stuck => [255, 60, 60, 255],
                MarkKind::Fell => [255, 160, 40, 255],
            };
            post(&mut out, m.at, 3.0, color);
        }
        if let Some(a) = self.route.a {
            post(&mut out, a, 2.0, [80, 230, 90, 255]);
        }
        if let Some(b) = self.route.b {
            post(&mut out, b, 2.0, [80, 140, 255, 255]);
        }
        if let Some((_, path)) = &self.route.result {
            for pair in path.windows(2) {
                let lift = |p: V3| [p[0], p[1] + 0.1, p[2]];
                out.push(LineVertex::new(lift(pair[0]), [90, 230, 230, 255]));
                out.push(LineVertex::new(lift(pair[1]), [90, 230, 230, 255]));
            }
        }
        out
    }

    /// Objects of the workspace whose collision is drawn while walking.
    pub fn own(&self) -> &[OwnObject] {
        &self.own
    }

    /// The point to use for A or B: the feet while walking, else the ground
    /// below the camera.
    fn here(&self, camera: &FlyCamera) -> Option<V3> {
        if self.active {
            return Some(self.player.feet);
        }
        let built = self.built.as_ref()?;
        let (toi, _) = built.world.below(camera.eye, 2000.0)?;
        Some([camera.eye[0], camera.eye[1] - toi, camera.eye[2]])
    }

    /// The walk section of the browser panel.
    pub fn panel(
        &mut self,
        ui: &mut egui::Ui,
        camera: &FlyCamera,
        selected: bool,
    ) -> Option<PanelAction> {
        self.ctx = Some(ui.ctx().clone());
        let mut action = None;
        egui::CollapsingHeader::new(tr("walk.header"))
            .id_salt("level-walk")
            .default_open(self.active)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if self.active {
                        if ui.button(tr("walk.stop")).clicked() {
                            self.exit();
                        }
                    } else {
                        if ui
                            .button(tr("walk.start"))
                            .on_hover_text(tr("walk.start_hint"))
                            .clicked()
                        {
                            action = Some(PanelAction::EnterAtCamera);
                        }
                        if ui
                            .add_enabled(selected, egui::Button::new(tr("walk.start_selected")))
                            .on_hover_text(tr("walk.start_selected_hint"))
                            .clicked()
                        {
                            action = Some(PanelAction::EnterAtSelected);
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut self.noclip, tr("walk.noclip"))
                        .on_hover_text(tr("walk.noclip_hint"));
                    if ui
                        .checkbox(&mut self.ground, tr("walk.ground"))
                        .on_hover_text(tr("walk.ground_hint"))
                        .changed()
                    {
                        self.stale = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(tr("walk.radius"));
                    if ui
                        .add(
                            egui::DragValue::new(&mut self.radius)
                                .range(0.0..=5000.0)
                                .speed(10.0)
                                .suffix(" m"),
                        )
                        .on_hover_text(tr("walk.radius_hint"))
                        .changed()
                    {
                        self.stale = true;
                    }
                });
                egui::CollapsingHeader::new(tr("walk.body"))
                    .id_salt("level-walk-body")
                    .show(ui, |ui| {
                        let b = &mut self.body;
                        for (key, value, range) in [
                            ("walk.height", &mut b.height, 0.5..=3.0),
                            ("walk.capsule_radius", &mut b.radius, 0.1..=1.0),
                            ("walk.eye", &mut b.eye, 0.3..=3.0),
                            ("walk.step", &mut b.step, 0.0..=1.5),
                            ("walk.slope", &mut b.max_slope, 10.0..=80.0),
                            ("walk.walk_speed", &mut b.walk_speed, 0.5..=10.0),
                            ("walk.run_speed", &mut b.run_speed, 0.5..=20.0),
                            ("walk.jump", &mut b.jump_speed, 0.0..=15.0),
                        ] {
                            ui.horizontal(|ui| {
                                ui.label(tr(key));
                                ui.add(egui::DragValue::new(value).range(range).speed(0.01));
                            });
                        }
                        b.radius = b.radius.min(b.height / 2.0 - 0.05);
                        if ui.small_button(tr("walk.body_default")).clicked() {
                            *b = BodySize::default();
                        }
                        ui.label(RichText::new(tr("walk.body_note")).weak().small());
                    });
                ui.separator();
                // A to B.
                ui.label(tr("walk.route"));
                ui.horizontal_wrapped(|ui| {
                    let here = self.here(camera);
                    if ui
                        .add_enabled(here.is_some(), egui::Button::new(tr("walk.set_a")))
                        .clicked()
                    {
                        self.route.a = here;
                        self.route.result = None;
                        self.revision += 1;
                    }
                    if ui
                        .add_enabled(here.is_some(), egui::Button::new(tr("walk.set_b")))
                        .clicked()
                    {
                        self.route.b = here;
                        self.route.result = None;
                        self.revision += 1;
                    }
                    let ready = self.route.a.is_some()
                        && self.route.b.is_some()
                        && self.built.is_some()
                        && self.route.job.is_none();
                    if ui
                        .add_enabled(ready, egui::Button::new(tr("walk.check_route")))
                        .on_hover_text(tr("walk.check_route_hint"))
                        .clicked()
                    {
                        self.check_route();
                    }
                });
                if let Some((outcome, path)) = &self.route.result {
                    let walked: f32 = path
                        .windows(2)
                        .map(|p| {
                            (0..3)
                                .map(|i| (p[1][i] - p[0][i]).powi(2))
                                .sum::<f32>()
                                .sqrt()
                        })
                        .sum();
                    let at = |p: &V3| format!("{:.1} {:.1} {:.1}", p[0], p[1], p[2]);
                    let text = match outcome {
                        Outcome::Reached => {
                            trf("walk.route_reached", &[("m", &format!("{walked:.1}"))])
                        }
                        Outcome::Stuck(p) => trf("walk.route_stuck", &[("at", &at(p))]),
                        Outcome::Fell(p) => trf("walk.route_fell", &[("at", &at(p))]),
                        Outcome::TimedOut(p) => trf("walk.route_timeout", &[("at", &at(p))]),
                    };
                    ui.label(text);
                } else if self.route.job.is_some() {
                    ui.spinner();
                }
                ui.separator();
                // Problems found.
                ui.horizontal(|ui| {
                    ui.label(trf("walk.marks", &[("count", &self.marks.len())]));
                    if !self.marks.is_empty() && ui.small_button(tr("walk.clear_marks")).clicked() {
                        self.marks.clear();
                        self.revision += 1;
                    }
                });
                let mut go = None;
                for m in self.marks.iter().take(20) {
                    let kind = match m.kind {
                        MarkKind::Stuck => tr("walk.mark_stuck"),
                        MarkKind::Fell => tr("walk.mark_fell"),
                    };
                    let text = format!("{kind} {:.1} {:.1} {:.1}", m.at[0], m.at[1], m.at[2]);
                    if ui.small_button(text).clicked() {
                        go = Some(m.at);
                    }
                }
                // Objects of the workspace.
                if !self.own.is_empty() {
                    ui.separator();
                    ui.label(trf("walk.own", &[("count", &self.own.len())]));
                    for o in &self.own {
                        let mut text = o.label.clone();
                        if o.new {
                            text += tr("walk.own_new");
                        }
                        let response = ui.small_button(text);
                        let response = if o.collisions == 0 {
                            response.on_hover_text(tr("walk.own_no_collision"))
                        } else {
                            response
                        };
                        if response.clicked() {
                            // Beside its collision, else beside its origin.
                            let bounds = self
                                .built
                                .as_ref()
                                .and_then(|b| b.world.object_bounds(0, o.object));
                            go = Some(match bounds {
                                Some(b) => [
                                    b.maxs.x + self.body.radius + 0.6,
                                    b.mins.y + 1.0,
                                    (b.mins.z + b.maxs.z) / 2.0,
                                ],
                                None => [o.position[0] + 2.0, o.position[1] + 1.0, o.position[2]],
                            });
                        }
                        if o.collisions == 0 {
                            ui.label(
                                RichText::new(tr("walk.own_no_collision"))
                                    .small()
                                    .color(egui::Color32::from_rgb(255, 160, 60)),
                            );
                        }
                    }
                }
                if let Some(at) = go {
                    if self.active && self.built.is_some() {
                        self.place(at);
                    } else {
                        self.route.a = None;
                        self.enter_at = Some(at);
                        self.active = true;
                    }
                }
                if !self.status.is_empty() {
                    ui.label(RichText::new(&self.status).weak().small());
                }
            });
        action
    }

    /// Checks the walk from `a` to `b` once the world is built (for checks
    /// without input).
    pub fn request_route(&mut self, a: V3, b: V3) {
        self.route.a = Some(a);
        self.route.b = Some(b);
        self.route.pending = true;
        self.revision += 1;
    }

    fn check_route(&mut self) {
        let (Some(a), Some(b), Some(built)) = (self.route.a, self.route.b, self.built.clone())
        else {
            return;
        };
        let body = self.body;
        let (tx, rx) = channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(physics::walk_to(&built.world, &body, a, b));
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
        self.route.job = Some(rx);
        self.route.result = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_toolkit::resource::hkx::physics::{Physics, ShapeKind};
    use physics::{Affine, Owner, Placed, physics_parts};

    /// A walk on a floor 40 × 40 m at y = 0 with a wall 2 m high at z = −5.
    fn walk_on_floor() -> Walk {
        let floor = ShapeKind::Box {
            radius: 0.0,
            half_extents: [20.0, 0.5, 20.0, 0.0],
        };
        let wall = ShapeKind::ConvexTranslate {
            radius: 0.0,
            translation: [0.0, 1.5, -5.25, 0.0],
            child: Box::new(
                ShapeKind::Box {
                    radius: 0.0,
                    half_extents: [10.0, 1.0, 0.25, 0.0],
                }
                .into(),
            ),
        };
        let physics = Physics::fixed(vec![(None, floor.into()), (None, wall.into())], Vec::new());
        let world = World::new(&[Placed {
            owner: Owner {
                layer: 0,
                object: (0, 0),
                hkx: 7,
            },
            parts: Arc::new(physics_parts(&physics, &Affine::IDENTITY)),
            pose: rapier3d::prelude::Pose::from_translation(rapier3d::prelude::Vector::new(
                0.0, -0.5, 0.0,
            )),
        }]);
        let mut walk = Walk {
            active: true,
            enter_at: Some([0.0, 3.0, 0.0]),
            ..Walk::default()
        };
        walk.built = Some(Arc::new(Built {
            world,
            ground: None,
            ground_streams: Vec::new(),
            stats: Stats::default(),
            elapsed: Duration::ZERO,
            key: 0,
            area: None,
        }));
        walk
    }

    fn key(key: egui::Key, pressed: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// Runs one frame of the walk without a window; `events` go in first.
    fn frame(
        ctx: &egui::Context,
        walk: &mut Walk,
        camera: &mut FlyCamera,
        events: Vec<egui::Event>,
    ) {
        let mut events = events;
        // Hover the scene so that it takes the keys.
        events.insert(0, egui::Event::PointerMoved(egui::pos2(200.0, 200.0)));
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            predicted_dt: 1.0 / 60.0,
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            let (_, response) =
                ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
            walk.drive(ui, &response, camera);
            walk.panel(ui, camera, false);
        });
        output.textures_delta.clear();
    }

    #[test]
    fn keys_walk_the_capsule_into_a_wall_and_esc_stops() {
        let ctx = egui::Context::default();
        let mut walk = walk_on_floor();
        // Facing −Z, towards the wall.
        let mut camera = FlyCamera {
            yaw: 0.0,
            pitch: 0.0,
            ..FlyCamera::default()
        };
        for _ in 0..30 {
            frame(&ctx, &mut walk, &mut camera, Vec::new());
        }
        assert!(walk.player.grounded, "{:?}", walk.player);
        assert!(walk.player.feet[1].abs() < 0.05, "{:?}", walk.player);
        assert!(
            (camera.eye[1] - walk.body.eye).abs() < 0.06,
            "{:?}",
            camera.eye
        );
        let level_free_hud = walk.hud_text(None);
        assert!(level_free_hud.contains("hkx 00000007"), "{level_free_hud}");

        // W for six seconds: walks to the wall and stops there.
        frame(&ctx, &mut walk, &mut camera, vec![key(egui::Key::W, true)]);
        for _ in 0..360 {
            frame(&ctx, &mut walk, &mut camera, Vec::new());
        }
        let z = walk.player.feet[2];
        assert!(z < -4.4 && z > -5.0 + walk.body.radius - 0.05, "{z}");
        // Pushing into the wall leaves a "stuck" mark.
        assert!(
            walk.marks.iter().any(|m| m.kind == MarkKind::Stuck),
            "{:?}",
            walk.marks
        );
        frame(&ctx, &mut walk, &mut camera, vec![key(egui::Key::W, false)]);

        // N: through walls; W then passes the wall.
        frame(&ctx, &mut walk, &mut camera, vec![key(egui::Key::N, true)]);
        assert!(walk.noclip);
        frame(&ctx, &mut walk, &mut camera, vec![key(egui::Key::W, true)]);
        for _ in 0..120 {
            frame(&ctx, &mut walk, &mut camera, Vec::new());
        }
        assert!(walk.player.feet[2] < -6.0, "{:?}", walk.player);

        // Esc stops walking.
        frame(
            &ctx,
            &mut walk,
            &mut camera,
            vec![key(egui::Key::Escape, true)],
        );
        assert!(!walk.active);
    }

    #[test]
    fn falling_off_the_world_is_marked_and_the_player_put_back() {
        let ctx = egui::Context::default();
        let mut walk = walk_on_floor();
        // Facing +Z, off the floor's edge.
        let mut camera = FlyCamera {
            yaw: std::f32::consts::PI,
            pitch: 0.0,
            ..FlyCamera::default()
        };
        for _ in 0..10 {
            frame(&ctx, &mut walk, &mut camera, Vec::new());
        }
        frame(&ctx, &mut walk, &mut camera, vec![key(egui::Key::W, true)]);
        for _ in 0..60 * 20 {
            frame(&ctx, &mut walk, &mut camera, Vec::new());
            if walk.marks.iter().any(|m| m.kind == MarkKind::Fell) {
                break;
            }
        }
        let fell = walk.marks.iter().find(|m| m.kind == MarkKind::Fell);
        assert!(fell.is_some_and(|m| m.at[2] > 15.0), "{:?}", walk.marks);
        assert!(walk.player.feet[1] > -1.0, "put back: {:?}", walk.player);
        assert!(!walk.lines().is_empty());
    }
}
