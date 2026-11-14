//! The level around a cut-scene in its preview (stage 61,
//! `docs/formats/cutscenes.md` §9): the zones of the city around the camera
//! path and the actors, loaded on worker threads for the whole cut-scene at
//! once (moving the play head or playing never waits for zones), the props
//! of the shot streams on their actors, and the light, sky and fog of the
//! time of day the cut-scene sets.
//!
//! The zones are chosen by [`doctor_toolkit::cutscene::world`]; the meshes
//! are converted by the level view's loader and drawn with its items, so a
//! zone open in a level tab shares its meshes on the GPU.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use doctor_toolkit::cutscene::Cutscene;
use doctor_toolkit::cutscene::world::{
    Choice, PathPoint, TimeOfDay, ZoneBox, ZoneKind, candidate_boxes, choose_zones, mission_time,
    path_points,
};
use doctor_toolkit::game::GameInstance;
use doctor_toolkit::level::{Location, Lookup, Overrides, TYPE_MESH, TYPE_METAMODEL};
use doctor_toolkit::resource::hkx::TYPE_HKX;
use doctor_toolkit::workspace::{ResourceKey, Workspace};

use crate::level_view::loader::{self, Batch, LoadInput, LoadJob, Msg};
use crate::level_view::scene::{self, Filter, Item, ItemKind, LodMode, MeshSphere};
use crate::render::math::{self, Mat4, Vec3};
use crate::render::{Environment, Gpu, MeshData, MeshKey, Scene, View};

/// Largest texture side of the world (as in the level view).
const TEXTURE_LIMIT: u32 = 512;
/// Time per frame spent uploading converted meshes.
const UPLOAD_BUDGET: Duration = Duration::from_millis(12);
/// Seconds between the points of the path the zones are chosen by.
const PATH_STEP: f32 = 0.25;
/// Instances smaller than this angle (radians) are not drawn.
const MIN_ANGLE: f32 = 0.0015;
/// Hour of the game clock before any mission sets one (the time of day
/// manager of `globals_global` starts at 16:00).
pub const DEFAULT_TIME: TimeOfDay = TimeOfDay {
    hour: 16,
    minute: 0,
};

/// World loads running (planning or converting); screenshots wait for them.
static RUNNING: AtomicUsize = AtomicUsize::new(0);

/// Whether a cut-scene world is still loading.
pub fn busy() -> bool {
    RUNNING.load(Ordering::Relaxed) > 0
}

/// Counts a running load while it lives.
struct Running;

impl Running {
    fn new() -> Self {
        RUNNING.fetch_add(1, Ordering::Relaxed);
        Self
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        RUNNING.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Zone boxes by game root and folder stream: a city's boxes take a moment
/// to read, the cut-scenes of a session reuse them.
type BoxCache = HashMap<(PathBuf, String), Arc<Vec<ZoneBox>>>;
static BOXES: Mutex<Option<BoxCache>> = Mutex::new(None);

/// Opens a game folder and its resource index (cached by the caller).
pub type OpenGame =
    fn(&PathBuf) -> Result<(Arc<GameInstance>, Arc<doctor_toolkit::game::ResourceIndex>), String>;

/// What the planning thread found.
struct Plan {
    input: LoadInput,
    boxes: Arc<Vec<ZoneBox>>,
    choices: Vec<Choice>,
    /// Distance from the path to every candidate zone, by stream.
    distances: HashMap<usize, f32>,
    mission_time: Option<TimeOfDay>,
    /// The shot streams: their objects are the actors and props.
    shot_streams: Vec<usize>,
    elapsed: Duration,
}

enum PlanMsg {
    Plan(Box<Plan>),
    Error(String),
}

enum Phase {
    Idle,
    Planning(Receiver<PlanMsg>, Running),
    Loading(LoadJob, Running),
    Ready,
}

/// Workspace changes the world shows before they are applied: data of
/// edited metamodels, meshes, collision and SimGroups.
#[derive(Clone, Default)]
pub struct Edits(Vec<(ResourceKey, Arc<Vec<u8>>)>);

impl Edits {
    pub fn of(ws: Option<&Workspace>) -> Self {
        let Some(ws) = ws else {
            return Self::default();
        };
        let wanted = [
            TYPE_METAMODEL,
            TYPE_MESH,
            doctor_toolkit::level::TYPE_MESH_VOLATILE,
            TYPE_HKX,
            doctor_toolkit::level::TYPE_SIMGROUP,
        ];
        Self(
            ws.changes()
                .filter(|c| wanted.contains(&c.key.type_id))
                .filter_map(|c| Some((c.key.clone(), Arc::new(ws.data(&c.key).ok()??))))
                .collect(),
        )
    }

    /// The overrides of a level, as the level view makes them.
    fn overrides(&self, game: &GameInstance) -> Overrides {
        let mut out = Overrides::default();
        for (key, data) in &self.0 {
            let Some(stream) = game.find_stream(&key.stream) else {
                continue;
            };
            if key.type_id != doctor_toolkit::level::TYPE_SIMGROUP {
                out.by_id
                    .insert((key.type_id, key.ids[3]), (stream, data.clone()));
                continue;
            }
            let Ok(file) = game.open_stream(stream) else {
                continue;
            };
            if let Some(resource) = file
                .layout()
                .resources
                .iter()
                .position(|r| r.header.type_id == key.type_id && r.header.ids == key.ids)
            {
                out.groups
                    .insert(Location { stream, resource }, data.clone());
            }
        }
        out
    }
}

/// The world of one cut-scene, kept across reloads of the view.
pub struct World {
    /// Show the world (`false`: actors only).
    pub enabled: bool,
    phase: Phase,
    edits: Edits,
    /// Every candidate zone of the cut-scene.
    pub boxes: Arc<Vec<ZoneBox>>,
    /// Distance from the path to each candidate, by stream.
    pub distances: HashMap<usize, f32>,
    /// Zones chosen from the path.
    pub auto: BTreeSet<usize>,
    /// Zones to show (the automatic choice unless the user changes it).
    pub chosen: BTreeSet<usize>,
    /// Zones of the loaded world.
    pub loaded: BTreeSet<usize>,
    shot_streams: Vec<usize>,
    input: Option<LoadInput>,
    pub mission_time: Option<TimeOfDay>,
    /// World items (zones), and the mesh items of props by actor GUID.
    items: Vec<Item>,
    props: HashMap<[u8; 16], Vec<(MeshKey, Mat4)>>,
    pending: VecDeque<Batch>,
    /// Converted meshes, uploaded again if the renderer dropped them.
    meshes: HashMap<MeshKey, Arc<MeshData>>,
    spheres: HashMap<MeshKey, MeshSphere>,
    /// Bumped whenever more of the world can be drawn.
    pub generation: u64,
    pub status: String,
    /// Planning, level read and mesh conversion times.
    pub timings: Vec<(&'static str, Duration)>,
    started: Option<Instant>,
    /// Meshes converted and to convert.
    pub progress: Option<(usize, usize)>,
    pub error: Option<String>,
}

pub type SharedWorld = Arc<Mutex<World>>;

impl Default for World {
    fn default() -> Self {
        Self {
            enabled: true,
            phase: Phase::Idle,
            edits: Edits::default(),
            boxes: Arc::default(),
            distances: HashMap::new(),
            auto: BTreeSet::new(),
            chosen: BTreeSet::new(),
            loaded: BTreeSet::new(),
            shot_streams: Vec::new(),
            input: None,
            mission_time: None,
            items: Vec::new(),
            props: HashMap::new(),
            pending: VecDeque::new(),
            meshes: HashMap::new(),
            spheres: HashMap::new(),
            generation: 0,
            status: String::new(),
            timings: Vec::new(),
            started: None,
            progress: None,
            error: None,
        }
    }
}

impl World {
    pub fn new(edits: Edits) -> Self {
        Self {
            edits,
            ..Self::default()
        }
    }

    /// Whether nothing was asked for yet.
    pub fn is_idle(&self) -> bool {
        matches!(self.phase, Phase::Idle)
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.phase, Phase::Planning(..) | Phase::Loading(..))
    }

    pub fn is_ready(&self) -> bool {
        matches!(self.phase, Phase::Ready)
    }

    /// Starts choosing the zones of `cutscene` and loading them, on worker
    /// threads; `open` opens the game and its index (cached by the caller).
    pub fn start(
        &mut self,
        root: PathBuf,
        cutscene: &Cutscene,
        open: OpenGame,
        ctx: &egui::Context,
    ) {
        let points = path_points(cutscene, PATH_STEP);
        let stream = cutscene.stream;
        let edits = self.edits.clone();
        let (tx, rx) = channel();
        let ctx2 = ctx.clone();
        self.started = Some(Instant::now());
        self.timings.clear();
        self.error = None;
        self.status = "choosing zones".into();
        std::thread::spawn(move || {
            let msg = match plan(&root, stream, &points, &edits, open) {
                Ok(p) => PlanMsg::Plan(Box::new(p)),
                Err(e) => PlanMsg::Error(e),
            };
            let _ = tx.send(msg);
            ctx2.request_repaint();
        });
        self.phase = Phase::Planning(rx, Running::new());
    }

    /// Loads the chosen zones again (after the user changed them).
    pub fn reload(&mut self, ctx: &egui::Context) {
        let Some(input) = self.input.clone() else {
            return;
        };
        self.load(input, ctx);
    }

    fn load(&mut self, input: LoadInput, ctx: &egui::Context) {
        let mut streams: Vec<usize> = self.chosen.iter().copied().collect();
        streams.extend(&self.shot_streams);
        self.items.clear();
        self.props.clear();
        self.pending.clear();
        self.progress = None;
        self.loaded = self.chosen.clone();
        self.started = Some(Instant::now());
        self.timings.retain(|(name, _)| *name == "zones");
        self.status = format!("loading {} zones", self.chosen.len());
        self.generation += 1;
        self.phase = Phase::Loading(loader::start(input, streams, ctx.clone()), Running::new());
    }

    /// Takes the results of the worker threads; returns whether the picture
    /// can change.
    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        let mut changed = false;
        match &self.phase {
            Phase::Planning(rx, _) => match rx.try_recv() {
                Ok(PlanMsg::Plan(plan)) => {
                    let plan = *plan;
                    self.boxes = plan.boxes;
                    self.distances = plan.distances;
                    self.auto = plan.choices.iter().map(|c| c.stream).collect();
                    self.chosen = self.auto.clone();
                    self.shot_streams = plan.shot_streams;
                    self.mission_time = plan.mission_time;
                    self.timings.push(("zones", plan.elapsed));
                    self.input = Some(plan.input.clone());
                    self.load(plan.input, ctx);
                    changed = true;
                }
                Ok(PlanMsg::Error(e)) => {
                    self.error = Some(e);
                    self.status.clear();
                    self.phase = Phase::Ready;
                    changed = true;
                }
                Err(_) => {}
            },
            Phase::Loading(job, _) => {
                let messages: Vec<Msg> = job.rx.try_iter().collect();
                let total = job.total.load(Ordering::Relaxed);
                let done = job.done.load(Ordering::Relaxed);
                if total > 0 {
                    self.progress = Some((done, total));
                }
                for msg in messages {
                    match msg {
                        Msg::Level { level, elapsed, .. } => {
                            self.timings.push(("level", elapsed));
                            self.take_level(&level);
                            changed = true;
                        }
                        Msg::Batch(batch) => self.pending.push_back(batch),
                        Msg::Done {
                            meshes, triangles, ..
                        } => {
                            if let Some(s) = self.started {
                                self.timings.push(("meshes", s.elapsed()));
                            }
                            self.status = format!(
                                "{} zones, {} meshes, {}k triangles",
                                self.loaded.len(),
                                meshes,
                                triangles / 1000
                            );
                            self.phase = Phase::Ready;
                            changed = true;
                            break;
                        }
                        Msg::Error(e) => {
                            self.error = Some(e);
                            self.phase = Phase::Ready;
                            changed = true;
                            break;
                        }
                    }
                }
            }
            Phase::Idle | Phase::Ready => {}
        }
        if self.is_loading() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        changed
    }

    /// World items and prop items of a loaded level.
    fn take_level(&mut self, level: &doctor_toolkit::level::Level) {
        let (items, _) = scene::build_items(level);
        let shot_groups: Vec<bool> = level
            .groups
            .iter()
            .map(|g| self.shot_streams.contains(&g.location.stream))
            .collect();
        for item in items {
            let Some(r) = item.object else {
                continue;
            };
            if !shot_groups[r.group] {
                self.items.push(item);
                continue;
            }
            // A prop: its finest meshes, placed on its actor.
            let finest = item.lod.is_none_or(|[near, _]| near <= 0.0);
            if item.kind == ItemKind::Mesh && finest {
                let guid = level.groups[r.group].objects[r.object].guid.0;
                self.props
                    .entry(guid)
                    .or_default()
                    .push((item.mesh, scene::flatten(&item.local)));
            }
        }
        self.items.retain(|i| i.kind == ItemKind::Mesh);
        self.generation += 1;
    }

    /// Uploads converted meshes for a bounded time; returns whether any was.
    pub fn upload(&mut self, gpu: &Gpu) -> bool {
        if self.pending.is_empty() {
            return false;
        }
        let started = Instant::now();
        let mut renderer = gpu.lock();
        let wanted = self.meshes.len() + self.pending.iter().map(|b| b.meshes.len()).sum::<usize>();
        let budget = renderer.mesh_budget().max(wanted + 512);
        renderer.set_mesh_budget(budget);
        while let Some(batch) = self.pending.pop_front() {
            for (key, texture) in &batch.textures {
                renderer.upload_texture(*key, texture);
            }
            for (_, key, mesh) in batch.meshes {
                let bounds = mesh.bounds();
                self.spheres.insert(key, (bounds.center(), bounds.radius()));
                renderer.upload_mesh(key, mesh.clone());
                self.meshes.insert(key, mesh);
            }
            if started.elapsed() > UPLOAD_BUDGET {
                break;
            }
        }
        self.generation += 1;
        true
    }

    /// Adds the world seen through `view` to `scene` (instances only):
    /// levels of detail by distance from the eye, instances outside the
    /// view left out here (the actors are drawn without culling).
    pub fn fill(&self, scene: &mut Scene, view: &View, gpu: &Gpu) {
        if !self.enabled || self.items.is_empty() {
            return;
        }
        let filter = Filter {
            lod: LodMode::Distance,
            markers: false,
            min_angle: MIN_ANGLE,
            ..Filter::default()
        };
        let mut world = Scene::default();
        scene::fill(&mut world, &self.items, view.eye, &filter, &|k| {
            self.spheres.get(&k).copied()
        });
        let planes = frustum_planes(&view.view_projection());
        world.instances.retain(|i| {
            self.spheres.get(&i.mesh).is_none_or(|(centre, radius)| {
                let c = math::transform_point(&i.transform, *centre);
                let r = radius * math::max_scale(&i.transform);
                sphere_visible(&planes, c, r)
            })
        });
        let mut renderer = gpu.lock();
        for instance in &world.instances {
            // Meshes the renderer dropped for another view come back.
            if !renderer.has_mesh(instance.mesh)
                && let Some(mesh) = self.meshes.get(&instance.mesh)
            {
                renderer.upload_mesh(instance.mesh, mesh.clone());
            }
        }
        scene.instances.extend(world.instances);
    }

    /// The meshes of a prop actor (`guid`), placed relative to the actor.
    pub fn props(&self, guid: &[u8; 16]) -> &[(MeshKey, Mat4)] {
        self.props.get(guid).map_or(&[], Vec::as_slice)
    }

    /// Candidate zones for the zone list: chosen ones first, then by
    /// distance from the path.
    pub fn listed(&self) -> Vec<&ZoneBox> {
        let mut out: Vec<&ZoneBox> = self.boxes.iter().collect();
        out.sort_by(|a, b| {
            let key = |z: &ZoneBox| {
                (
                    !self.chosen.contains(&z.stream),
                    self.distances.get(&z.stream).copied().unwrap_or(f32::MAX),
                )
            };
            let (ka, kb) = (key(a), key(b));
            ka.0.cmp(&kb.0).then(ka.1.total_cmp(&kb.1))
        });
        out
    }
}

/// The zones a level view loads for a cut-scene (stage 62): the automatic
/// choice of the preview, or the folder stream alone when nothing is near
/// the path (worker thread).
pub fn choose_streams(
    game: &Arc<GameInstance>,
    index: &doctor_toolkit::game::ResourceIndex,
    cutscene: &Cutscene,
) -> Result<Vec<usize>, String> {
    let folder = game.streams()[cutscene.stream].stream_name();
    let key = (game.root().to_path_buf(), folder.clone());
    let cached = BOXES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .get(&key)
        .cloned();
    let boxes = match cached {
        Some(b) => b,
        None => {
            let b = Arc::new(candidate_boxes(game, index, &folder).map_err(|e| e.to_string())?);
            BOXES
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_or_insert_with(HashMap::new)
                .insert(key, b.clone());
            b
        }
    };
    let points = path_points(cutscene, PATH_STEP);
    let mut streams: Vec<usize> = choose_zones(&boxes, &points)
        .iter()
        .map(|c| c.stream)
        .collect();
    if streams.is_empty() {
        streams.push(cutscene.stream);
    }
    streams.sort_unstable();
    streams.dedup();
    Ok(streams)
}

/// Chooses the zones of a cut-scene (worker thread).
fn plan(
    root: &PathBuf,
    stream: usize,
    points: &[PathPoint],
    edits: &Edits,
    open: OpenGame,
) -> Result<Plan, String> {
    let started = Instant::now();
    let (game, index) = open(root)?;
    let folder = game.streams()[stream].stream_name();
    let key = (root.clone(), folder.clone());
    let cached = BOXES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .get(&key)
        .cloned();
    let boxes = match cached {
        Some(b) => b,
        None => {
            let b = Arc::new(candidate_boxes(&game, &index, &folder).map_err(|e| e.to_string())?);
            BOXES
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_or_insert_with(HashMap::new)
                .insert(key, b.clone());
            b
        }
    };
    let choices = choose_zones(&boxes, points);
    let distances = boxes
        .iter()
        .map(|b| {
            let d = points
                .iter()
                .map(|p| b.distance(p.position))
                .fold(f32::MAX, f32::min);
            (b.stream, if b.kind.always() { 0.0 } else { d })
        })
        .collect();
    let prefix = format!("{folder}/");
    let shot_streams = game
        .streams()
        .iter()
        .enumerate()
        .filter(|(_, s)| s.stream_name().starts_with(&prefix))
        .map(|(i, _)| i)
        .collect();
    let mission_time = mission_time(&game, &folder).ok().flatten();
    let parents = game.stream_parents().unwrap_or_default();
    let input = LoadInput {
        lookup: Arc::new(Lookup::new(&index)),
        parents: Arc::new(parents),
        overrides: edits.overrides(&game),
        game,
        index,
        texture_limit: TEXTURE_LIMIT,
    };
    Ok(Plan {
        input,
        boxes,
        choices,
        distances,
        mission_time,
        shot_streams,
        elapsed: started.elapsed(),
    })
}

/// A short name of a zone kind for the zone list.
pub fn kind_name(kind: ZoneKind) -> &'static str {
    match kind {
        ZoneKind::High => "high",
        ZoneKind::Medium => "medium",
        ZoneKind::Low => "low",
        ZoneKind::Interior => "interior",
        ZoneKind::Sky => "sky",
        ZoneKind::Ocean => "ocean",
        ZoneKind::Mission => "mission",
        ZoneKind::Cutscene => "cut-scene",
        ZoneKind::Other => "other",
    }
}

/// Light, sky and fog of a time of day (hours from midnight), and the sky
/// colour behind the world (gamma-encoded RGBA). A rough day: the sun rises
/// at 6 in the east, is highest at noon in the south and sets at 18 in the
/// west; dusk is orange, the night dark blue with dim moonlight.
pub fn environment(hours: f32) -> (Environment, [f32; 4]) {
    let hours = hours.rem_euclid(24.0);
    let angle = (hours - 6.0) / 12.0 * std::f32::consts::PI;
    // Height of the sun: 1 at noon, −1 at midnight.
    let height = angle.sin();
    let day = smoothstep(-0.08, 0.3, height);
    let dusk = (1.0 - (height.abs() / 0.3)).clamp(0.0, 1.0) * smoothstep(-0.25, 0.0, height);
    let mix3 = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] {
        [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ]
    };
    let sun_color = mix3(
        mix3([0.16, 0.2, 0.34], [1.0, 0.95, 0.86], day),
        [1.0, 0.55, 0.3],
        dusk * 0.7,
    );
    let ambient = mix3([0.45, 0.5, 0.75], [1.05, 1.05, 1.1], day);
    let sky = mix3(
        mix3([0.035, 0.05, 0.11], [0.5, 0.68, 0.9], day),
        [0.88, 0.55, 0.38],
        dusk * 0.8,
    );
    // The sun's direction; at night the moon stands opposite it.
    let (dir_height, dir_angle) = if height >= -0.05 {
        (height.max(0.08), angle)
    } else {
        (0.6, angle + std::f32::consts::PI)
    };
    let horizontal = (1.0 - dir_height * dir_height).max(0.0).sqrt();
    let sun = math::normalize_or(
        [
            dir_angle.cos() * horizontal,
            dir_height,
            -dir_angle.sin().abs() * horizontal * 0.6 - 0.2,
        ],
        [0.0, 1.0, 0.0],
    );
    let fog_color = sky.map(srgb_to_linear);
    let environment = Environment {
        sun,
        sun_color,
        ambient,
        camera_light: 0.15 + 0.15 * day,
        fog_color,
        fog_start: 120.0,
        fog_end: 1600.0,
        fog_max: 0.75 - 0.15 * day,
    };
    (environment, [sky[0], sky[1], sky[2], 1.0])
}

/// Planes `(n, d)` of a view frustum (inside: `n·p + d ≥ 0`), from a
/// column-major view-projection with depth 0..1.
fn frustum_planes(m: &Mat4) -> [[f32; 4]; 6] {
    let row = |i: usize| [m[i], m[4 + i], m[8 + i], m[12 + i]];
    let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
    let add = |a: [f32; 4], b: [f32; 4]| std::array::from_fn(|k| a[k] + b[k]);
    let sub = |a: [f32; 4], b: [f32; 4]| std::array::from_fn(|k| a[k] - b[k]);
    [
        add(r3, r0),
        sub(r3, r0),
        add(r3, r1),
        sub(r3, r1),
        r2,
        sub(r3, r2),
    ]
}

fn sphere_visible(planes: &[[f32; 4]; 6], c: Vec3, r: f32) -> bool {
    planes.iter().all(|p| {
        let n = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt().max(1e-12);
        p[0] * c[0] + p[1] * c[1] + p[2] * c[2] + p[3] >= -r * n
    })
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_is_bright_and_night_is_dark() {
        let (noon, noon_sky) = environment(12.0);
        let (night, night_sky) = environment(0.0);
        let (dusk, dusk_sky) = environment(17.8);
        let sum = |c: [f32; 3]| c[0] + c[1] + c[2];
        assert!(sum(noon.sun_color) > 2.5);
        assert!(sum(night.sun_color) < 1.0);
        assert!(noon.sun[1] > 0.9, "noon sun overhead: {:?}", noon.sun);
        assert!(night.sun[1] > 0.0, "the moon is above the horizon");
        assert!(noon_sky[2] > noon_sky[0], "blue day sky");
        assert!(night_sky[0] + night_sky[1] + night_sky[2] < 0.3);
        assert!(dusk_sky[0] > dusk_sky[2], "orange dusk: {dusk_sky:?}");
        assert!(dusk.sun_color[0] > dusk.sun_color[2]);
        // The day wraps.
        assert_eq!(environment(-12.0).1, environment(12.0).1);
    }

    fn slow_game(
        _: &PathBuf,
    ) -> Result<(Arc<GameInstance>, Arc<doctor_toolkit::game::ResourceIndex>), String> {
        std::thread::sleep(Duration::from_millis(400));
        Err("no game here".into())
    }

    #[test]
    fn loading_the_world_does_not_hold_up_a_frame() {
        let cutscene = Cutscene {
            stream: 0,
            stream_name: "gf2/cuba/x".into(),
            name: "test".into(),
            sequence: None,
            shots: Vec::new(),
            audio: Vec::new(),
        };
        let ctx = egui::Context::default();
        let mut world = World::default();
        assert!(world.is_idle());
        let started = Instant::now();
        world.start(PathBuf::from("/nowhere"), &cutscene, slow_game, &ctx);
        // Starting and polling return at once while the worker reads.
        assert!(world.is_loading());
        assert!(!world.poll(&ctx));
        assert!(started.elapsed() < Duration::from_millis(200));
        // The worker's answer arrives later.
        let deadline = Instant::now() + Duration::from_secs(10);
        while world.is_loading() && Instant::now() < deadline {
            world.poll(&ctx);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(world.error.as_deref(), Some("no game here"));
        assert!(world.is_ready());
    }

    #[test]
    fn the_frustum_keeps_what_the_camera_sees() {
        let view = math::mul(
            &math::perspective(1.0, 1.0, 0.1, 100.0),
            &math::look_at([0.0; 3], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
        );
        let planes = frustum_planes(&view);
        assert!(sphere_visible(&planes, [0.0, 0.0, -10.0], 1.0));
        assert!(!sphere_visible(&planes, [0.0, 0.0, 10.0], 1.0), "behind");
        assert!(!sphere_visible(&planes, [50.0, 0.0, -10.0], 1.0), "aside");
        assert!(
            sphere_visible(&planes, [7.0, 0.0, -10.0], 3.0),
            "on the edge"
        );
        assert!(
            !sphere_visible(&planes, [0.0, 0.0, -200.0], 1.0),
            "past far"
        );
    }

    #[test]
    fn running_loads_are_counted_until_dropped() {
        let before = RUNNING.load(Ordering::Relaxed);
        let a = Running::new();
        assert!(busy());
        drop(a);
        assert_eq!(RUNNING.load(Ordering::Relaxed), before);
    }
}
