//! The level editor: a 3D scene of the placed objects of chosen zones.
//!
//! - **Browser** (left): zones of every level (streams with SimGroups) with
//!   check boxes; the chosen ones load together with their ancestors as scope.
//! - **Scene** (centre): one instance per mesh of every placed object, on the
//!   shared [`crate::render::SceneRenderer`] with frustum culling and LODs by
//!   distance; markers for objects without a mesh, trees and AI points.
//!   Fly camera (right drag look, W A S D Q E, wheel, Alt + drag orbit).
//!   Click selects (GPU id pass, or a CPU ray on software adapters).
//! - **Gizmo**: move along or rotate about world axes; Ctrl snaps.
//! - **Object** (right): the SimGroup object browser and editor
//!   ([`LogicView`]) with the selected object; duplicate, remove, add.
//!
//! Edits are returned as [`LevelAction`]s and recorded in the workspace by
//! the app, which then calls [`LevelView::refresh`].
//!
//! `DOCTOR_TOOLKIT_GUI_LEVEL=<zone>[+<zone>…][;option…]` opens the view and
//! loads the zones once the game is indexed; options: `select=<group>:<object>`,
//! `rotate`, `finest`, `points`, `eye=<x>,<y>,<z>`, `look=<x>,<y>,<z>`,
//! `yaw=<rad>`, `pitch=<rad>`, `act=<edit>` (see [`LevelView::preset_action`]),
//! `nav[=<graph>:<node>]` (show navigation graphs, select a node),
//! `spin` (turn and print frame times), `walk` (walk from the camera),
//! `route=<ax>,<ay>,<az>,<bx>,<by>,<bz>` (check the walk from A to B).
//!
//! **Walk** ([`walk`], stage 37): a first-person capsule with the Havok
//! collision of the loaded zones, gravity, steps; problem marks and A to B.

pub mod camera;
mod gizmo;
mod live;
pub mod loader;
pub mod nav;
pub mod scene;
pub mod walk;

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use doctor_toolkit::game::{GameInstance, ResourceIndex};
use doctor_toolkit::level::edit::ModelChoice;
use doctor_toolkit::level::{
    Catalog, Coverage, Found, Level, LevelAssets, LevelSource, Location, Lookup, Overrides,
    RowMatrix, TYPE_MESH, TYPE_MESH_VOLATILE, TYPE_METAMODEL, TYPE_SIMGROUP,
};
use doctor_toolkit::resource::hkx::TYPE_HKX;
use doctor_toolkit::resource::hkx::collision::CollisionKind;
use doctor_toolkit::resource::logic::TYPE_SIMGROUP as LOGIC_SIMGROUP;
use doctor_toolkit::resource::simgroup_edit::Transform;
use doctor_toolkit::workspace::{ResourceKey, Workspace};
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{Color32, Pos2, Rect, RichText, Sense, Vec2};

use crate::logic_view::{LogicAction, LogicView};
use crate::render::{Gpu, MeshData, RenderStats, Scene, View, Viewport, math};
use camera::FlyCamera;
use loader::{Batch, LoadInput, LoadJob, Msg};
use scene::{Filter, Item, LodMode, MARKER_KEY, ObjRef};

/// Largest texture side uploaded for levels.
const TEXTURE_LIMIT: u32 = 512;
/// Instances smaller than this many pixels are not drawn (when enabled).
const TINY_PIXELS: f32 = 2.0;
/// Time per frame spent uploading converted meshes.
const UPLOAD_BUDGET: Duration = Duration::from_millis(12);

/// An edit the app records in the workspace.
pub enum LevelAction {
    SetTransform {
        group: Location,
        object: usize,
        matrix: RowMatrix,
    },
    Duplicate {
        group: Location,
        object: usize,
    },
    Remove {
        group: Location,
        object: usize,
    },
    Add {
        group: Location,
        template: (Location, usize),
        model: ModelChoice,
        position: [f32; 3],
    },
    /// New data for a SimGroup from the object editor.
    Save {
        group: Location,
        data: Vec<u8>,
        label: String,
    },
    /// Start a wizard (`retexture`, `place`) for a metamodel of the level.
    StartWizard {
        task: &'static str,
        model: Option<u32>,
    },
    /// Show the object in the logic graph of its stream.
    ShowLogic {
        group: Location,
        object: usize,
    },
    /// Start the game near the camera (stage 33): the streams of the
    /// loaded zones and the camera's position.
    PlayHere {
        eye: [f32; 3],
        streams: Vec<usize>,
    },
    /// Move the player of the running game here (stage 35; the Link panel
    /// asks first).
    LiveTeleport {
        position: [f32; 3],
        yaw: f32,
    },
    /// The game draws with this camera (the module's free camera), or with
    /// its own again (`None`).
    LiveCamera(Option<doctor_toolkit::live::proto::CameraView>),
    /// An edited navigation graph; `renumbered` (after a removed node) makes
    /// links of other graphs into it follow.
    Graph {
        graph: Box<doctor_toolkit::level::navgraph::GraphRef>,
        renumbered: Option<Vec<Option<u16>>>,
        label: String,
    },
}

/// Everything that changes the image.
#[derive(Clone, PartialEq)]
struct RenderKey {
    camera: FlyCamera,
    size: [u32; 2],
    generation: u64,
    options: crate::render::SceneOptions,
    gizmo: (gizmo::Mode, Option<usize>),
    collision: Option<(ObjRef, RowMatrix)>,
    nav: u64,
    walk: u64,
}

struct Drag {
    object: ObjRef,
    /// A node of a navigation graph instead of the object.
    node: Option<nav::NodeRef>,
    axis: usize,
    start: RowMatrix,
    start_param: f32,
    current: RowMatrix,
}

#[derive(Default)]
struct Preset {
    select: Option<ObjRef>,
    selected_once: bool,
    spin: Option<Vec<f32>>,
    eye: Option<[f32; 3]>,
    look: Option<[f32; 3]>,
    yaw: Option<f32>,
    pitch: Option<f32>,
    /// Edits to make once the level is loaded, one per frame, on the
    /// selected object (for checks without input).
    actions: VecDeque<String>,
    /// Walk from the camera once the level is loaded.
    walk: bool,
    /// Check the walk from A to B once the level is loaded.
    route: Option<([f32; 3], [f32; 3])>,
}

pub struct LevelView {
    /// Player and camera of the running game (live link, stage 34).
    pub live: Option<crate::app::LiveMarks>,
    ctx: egui::Context,
    gpu: Option<Gpu>,
    input: Option<LoadInput>,
    catalog: Arc<Catalog>,
    zone_filter: String,
    chosen: BTreeSet<usize>,
    job: Option<LoadJob>,
    level: Option<Box<Level>>,
    assets: Option<Arc<LevelAssets>>,
    coverage: Coverage,
    /// Load timings and counts.
    info: String,
    status: String,
    load_started: Option<Instant>,
    pending: VecDeque<Batch>,
    uploaded: HashSet<u64>,
    failed: Vec<(u32, String)>,
    items: Vec<Item>,
    by_object: HashMap<ObjRef, Vec<usize>>,
    classes: Vec<(u32, String, usize)>,
    filter: Filter,
    scene: Scene,
    visible: Vec<usize>,
    camera: FlyCamera,
    viewport: Viewport,
    last_render: Option<RenderKey>,
    generation: u64,
    refill: bool,
    stats: RenderStats,
    last_pick: Option<Pos2>,
    search: String,
    results: Vec<ObjRef>,
    logic: Option<(usize, u64, LogicView)>,
    group_generation: Vec<u64>,
    gizmo: gizmo::Mode,
    hover_axis: Option<usize>,
    drag: Option<Drag>,
    add_model: String,
    add_clone: bool,
    /// Give the clone its own collision built from its meshes.
    add_collision: bool,
    /// A wizard chose the model to add: selecting an object keeps it.
    add_fixed: bool,
    confirm_remove: bool,
    preset: Option<Preset>,
    /// Zones to load once the game is ready (from the environment).
    autoload: Option<Vec<String>>,
    pending_autoload: bool,
    /// An object to select and fly to once the level is loaded.
    goto: Option<(Location, usize)>,
    /// Camera and selection to restore after a reload.
    keep_view: Option<(FlyCamera, Option<ObjRef>)>,
    /// The user moved the camera since the level was loaded.
    camera_moved: bool,
    /// Bounding spheres of the uploaded meshes.
    spheres: HashMap<u64, scene::MeshSphere>,
    /// Skip instances smaller than [`TINY_PIXELS`] on the screen.
    hide_tiny: bool,
    /// Centre of the level, for orbiting.
    center: [f32; 3],
    /// The game files changed since the zones were indexed.
    index_stale: bool,
    /// Draw the Havok collision of the selected object.
    show_collision: bool,
    /// "Play from here" was clicked.
    play_here: bool,
    /// Collision wireframes by `hkx` id (`None`: not readable).
    collisions: HashMap<u32, Option<Arc<crate::collision_view::Wire>>>,
    /// Navigation graphs of the loaded zones.
    nav: nav::NavLayer,
    /// An edit from the navigation panel, returned by the next `ui`.
    nav_action: Option<LevelAction>,
    /// Walking the level (stage 37).
    walk: walk::Walk,
    /// The camera follows the game's (stage 35).
    live_follow: bool,
}

impl LevelView {
    pub fn new(gpu: Option<Gpu>, ctx: &egui::Context) -> Self {
        let mut view = Self {
            live: None,
            ctx: ctx.clone(),
            gpu,
            input: None,
            catalog: Arc::default(),
            zone_filter: String::new(),
            chosen: BTreeSet::new(),
            job: None,
            level: None,
            assets: None,
            coverage: Coverage::default(),
            info: String::new(),
            status: String::new(),
            load_started: None,
            pending: VecDeque::new(),
            uploaded: HashSet::new(),
            failed: Vec::new(),
            items: Vec::new(),
            by_object: HashMap::new(),
            classes: Vec::new(),
            filter: Filter::default(),
            scene: Scene::default(),
            visible: Vec::new(),
            camera: FlyCamera::default(),
            viewport: Viewport::new(),
            last_render: None,
            generation: 0,
            refill: true,
            stats: RenderStats::default(),
            last_pick: None,
            search: String::new(),
            results: Vec::new(),
            logic: None,
            group_generation: Vec::new(),
            gizmo: gizmo::Mode::Move,
            hover_axis: None,
            drag: None,
            add_model: String::new(),
            add_clone: false,
            add_collision: false,
            add_fixed: false,
            confirm_remove: false,
            preset: None,
            autoload: None,
            pending_autoload: false,
            goto: None,
            keep_view: None,
            camera_moved: false,
            spheres: HashMap::new(),
            hide_tiny: true,
            show_collision: false,
            play_here: false,
            collisions: HashMap::new(),
            nav: nav::NavLayer::default(),
            nav_action: None,
            walk: walk::Walk::default(),
            live_follow: false,
            center: [0.0; 3],
            index_stale: false,
        };
        if let Ok(spec) = std::env::var("DOCTOR_TOOLKIT_GUI_LEVEL") {
            view.apply_preset(&spec);
        }
        view
    }

    /// Whether the environment asks for the level view at start.
    pub fn wants_focus(&self) -> bool {
        self.autoload.is_some()
    }

    /// Drops the preset of `DOCTOR_TOOLKIT_GUI_LEVEL`: only the first level
    /// tab takes it.
    pub fn forget_preset(&mut self) {
        self.autoload = None;
        self.preset = None;
    }

    /// Whether a level is loading (screenshots wait for it).
    pub fn busy(&self) -> bool {
        self.job.is_some()
            || !self.pending.is_empty()
            || self.autoload.is_some()
            || self.pending_autoload
            || self.preset.as_ref().is_some_and(|p| !p.actions.is_empty())
            || self.walk.busy()
    }

    fn apply_preset(&mut self, spec: &str) {
        let mut parts = spec.split(';');
        let zones = parts.next().unwrap_or_default();
        self.autoload = Some(zones.split('+').map(str::to_owned).collect());
        let mut preset = Preset::default();
        for item in parts {
            let (name, value) = item.split_once('=').unwrap_or((item, ""));
            let floats: Vec<f32> = value.split(',').filter_map(|v| v.parse().ok()).collect();
            match name {
                "select" => {
                    if let Some((g, o)) = value.split_once(':')
                        && let (Ok(group), Ok(object)) = (g.parse(), o.parse())
                    {
                        preset.select = Some(ObjRef { group, object });
                    }
                }
                "rotate" => self.gizmo = gizmo::Mode::Rotate,
                "finest" => self.filter.lod = LodMode::Finest,
                "points" => self.filter.points = true,
                "collision" => self.show_collision = true,
                "nav" => {
                    self.nav.visible = true;
                    if let Some((g, n)) = value.split_once(':')
                        && let (Ok(graph), Ok(node)) = (g.parse(), n.parse())
                    {
                        self.nav.select(Some(nav::NodeRef { graph, node }));
                    }
                }
                "spin" => preset.spin = Some(Vec::new()),
                "walk" => preset.walk = true,
                "route" if floats.len() == 6 => {
                    preset.route = Some((
                        [floats[0], floats[1], floats[2]],
                        [floats[3], floats[4], floats[5]],
                    ))
                }
                "act" => preset.actions.push_back(value.to_owned()),
                "eye" if floats.len() == 3 => preset.eye = Some([floats[0], floats[1], floats[2]]),
                "look" if floats.len() == 3 => {
                    preset.look = Some([floats[0], floats[1], floats[2]])
                }
                "yaw" => preset.yaw = floats.first().copied(),
                "pitch" => preset.pitch = floats.first().copied(),
                _ => {}
            }
        }
        self.preset = Some(preset);
    }

    /// The game was opened: build the zone list.
    pub fn open_game(&mut self, game: Arc<GameInstance>, index: Arc<ResourceIndex>) {
        self.job = None;
        self.walk.reset();
        self.level = None;
        self.items.clear();
        self.scene.instances.clear();
        self.logic = None;
        self.catalog = Arc::new(Catalog::new(&game, &index));
        let parents = game.stream_parents().unwrap_or_default();
        self.input = Some(LoadInput {
            lookup: Arc::new(Lookup::new(&index)),
            parents: Arc::new(parents),
            game,
            index,
            overrides: Overrides::default(),
            texture_limit: TEXTURE_LIMIT,
        });
        if let Some(zones) = self.autoload.take() {
            self.chosen.clear();
            for name in zones {
                let found: Vec<usize> = self.catalog.under(&name).map(|z| z.stream).collect();
                if found.is_empty() {
                    eprintln!("level: no zone at `{name}`");
                }
                self.chosen.extend(found);
            }
            self.autoload = None;
            self.pending_autoload = true;
        }
    }

    // ------------------------------------------------------------ loading

    fn start_load(&mut self, ctx: &egui::Context, ws: Option<&Workspace>) {
        if self.index_stale {
            self.reindex();
        }
        let Some(input) = &self.input else { return };
        if self.chosen.is_empty() {
            self.status = tr("level.choose_zones").into();
            return;
        }
        let mut input = input.clone();
        input.overrides = overrides(&input.game, ws, &self.chosen_locations(&input));
        self.level = None;
        self.items.clear();
        self.by_object.clear();
        self.scene.instances.clear();
        self.visible.clear();
        self.logic = None;
        self.pending.clear();
        self.uploaded.clear();
        self.spheres.clear();
        self.failed.clear();
        self.filter.selected = None;
        self.filter.hovered = None;
        self.drag = None;
        self.camera_moved = false;
        self.status = trf(
            "level.loading_zones",
            &[("zones", &trn("count.zones", self.chosen.len() as u64, &[]))],
        );
        self.load_started = Some(Instant::now());
        self.job = Some(loader::start(
            input,
            self.chosen.iter().copied().collect(),
            ctx.clone(),
        ));
    }

    /// SimGroups of the chosen zones (for the workspace overrides).
    fn chosen_locations(&self, input: &LoadInput) -> Vec<Location> {
        self.chosen
            .iter()
            .flat_map(|&s| {
                input
                    .lookup
                    .in_stream(s, TYPE_SIMGROUP)
                    .map(move |r| Location {
                        stream: s,
                        resource: r,
                    })
            })
            .collect()
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else { return };
        let messages: Vec<Msg> = job.rx.try_iter().collect();
        let mut finished = false;
        for msg in messages {
            match msg {
                Msg::Level {
                    level,
                    assets,
                    elapsed,
                } => {
                    self.coverage = level.coverage();
                    self.info = trf(
                        "level.read_info",
                        &[
                            ("ms", &elapsed.as_millis()),
                            (
                                "objects",
                                &trn("count.objects", self.coverage.objects as u64, &[]),
                            ),
                            (
                                "models",
                                &trn("count.models", self.coverage.models as u64, &[]),
                            ),
                            (
                                "meshes",
                                &trn("count.meshes", self.coverage.meshes as u64, &[]),
                            ),
                            (
                                "materials",
                                &trn("count.materials", assets.material_count() as u64, &[]),
                            ),
                        ],
                    );
                    self.group_generation = vec![0; level.groups.len()];
                    self.level = Some(level);
                    self.walk.level_changed();
                    self.assets = Some(assets);
                    self.rebuild_items();
                    match self.keep_view.take() {
                        Some((camera, selected)) => {
                            self.camera = camera;
                            let exists = selected.filter(|r| {
                                self.level.as_ref().is_some_and(|l| {
                                    l.groups
                                        .get(r.group)
                                        .is_some_and(|g| r.object < g.objects.len())
                                })
                            });
                            self.select(exists);
                        }
                        None => self.frame_all(),
                    }
                    if let Some(gpu) = &self.gpu {
                        let mut renderer = gpu.lock();
                        let budget = self.level.as_ref().map_or(0, |l| l.meshes.len());
                        renderer.set_mesh_budget(budget + 256);
                        let marker = scene::marker_mesh();
                        let b = marker.bounds();
                        self.spheres.insert(MARKER_KEY, (b.center(), b.radius()));
                        renderer.upload_mesh(MARKER_KEY, Arc::new(marker));
                    }
                    self.apply_preset_selection();
                    if let Some((group, object)) = self.goto.take() {
                        self.show_object(group, object);
                    }
                }
                Msg::Batch(batch) => self.pending.push_back(batch),
                Msg::Done {
                    elapsed,
                    meshes,
                    textures,
                    triangles,
                } => {
                    let total = self.load_started.map_or(elapsed, |s| s.elapsed());
                    self.info += &trf(
                        "level.converted_info",
                        &[
                            ("meshes", &trn("count.meshes", meshes as u64, &[])),
                            ("ktriangles", &(triangles / 1000)),
                            ("textures", &trn("count.textures", textures as u64, &[])),
                            ("seconds", &format!("{:.1}", total.as_secs_f64())),
                        ],
                    );
                    finished = true;
                    if !self.camera_moved
                        && self.keep_view.is_none()
                        && self.preset_camera().is_none()
                    {
                        self.frame_all();
                    }
                }
                Msg::Error(error) => {
                    self.status = trf("level.cannot_load", &[("error", &error)]);
                    finished = true;
                }
            }
        }
        if finished {
            self.job = None;
            self.status.clear();
        } else {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    /// Uploads converted meshes for a bounded time per frame.
    fn upload(&mut self) {
        let Some(gpu) = &self.gpu else {
            self.pending.clear();
            return;
        };
        if self.pending.is_empty() {
            return;
        }
        let started = Instant::now();
        let mut renderer = gpu.lock();
        while let Some(batch) = self.pending.pop_front() {
            for (key, texture) in &batch.textures {
                renderer.upload_texture(*key, texture);
            }
            for (_, key, mesh) in batch.meshes {
                let bounds = mesh.bounds();
                self.spheres.insert(key, (bounds.center(), bounds.radius()));
                renderer.upload_mesh(key, mesh);
                self.uploaded.insert(key);
            }
            self.refill = true;
            self.failed.extend(batch.failed);
            self.generation += 1;
            if started.elapsed() > UPLOAD_BUDGET {
                break;
            }
        }
    }

    fn rebuild_items(&mut self) {
        let Some(level) = &self.level else { return };
        let (items, by_object) = scene::build_items(level);
        let mut classes: HashMap<u32, (String, usize)> = HashMap::new();
        for g in &level.groups {
            for o in g.objects.iter().filter(|o| o.transform.is_some()) {
                let entry = classes.entry(o.class).or_insert_with(|| {
                    (
                        o.class_name
                            .map_or_else(|| format!("{:08x}", o.class), str::to_owned),
                        0,
                    )
                });
                entry.1 += 1;
            }
        }
        let mut classes: Vec<(u32, String, usize)> =
            classes.into_iter().map(|(k, (n, c))| (k, n, c)).collect();
        classes.sort_by(|a, b| b.2.cmp(&a.2).then(a.1.cmp(&b.1)));
        self.classes = classes;
        self.items = items;
        self.by_object = by_object;
        self.refill = true;
    }

    fn frame_all(&mut self) {
        let bounds = match &self.gpu {
            Some(gpu) => {
                let renderer = gpu.lock();
                scene::bounds(&self.items, &|key| {
                    renderer.mesh_data(key).map(|m| m.bounds())
                })
            }
            None => scene::bounds(&self.items, &|_| None),
        };
        self.camera.frame(&bounds);
        self.center = bounds.center();
        self.refill = true;
    }

    /// Whether the environment preset places the camera.
    fn preset_camera(&self) -> Option<()> {
        let p = self.preset.as_ref()?;
        (p.eye.is_some() || p.selected_once).then_some(())
    }

    fn apply_preset_selection(&mut self) {
        let Some(preset) = &mut self.preset else {
            return;
        };
        if let Some(eye) = preset.eye {
            self.camera.eye = eye;
        }
        if let Some(yaw) = preset.yaw {
            self.camera.yaw = yaw;
        }
        if let Some(pitch) = preset.pitch {
            self.camera.pitch = pitch;
        }
        if let Some(target) = preset.look {
            self.camera.turn_to(target);
        }
        let walk = std::mem::take(&mut preset.walk);
        if let Some((a, b)) = preset.route.take() {
            self.walk.request_route(a, b);
        }
        let select = preset.select.take();
        preset.selected_once |= select.is_some();
        let has_eye = preset.eye.is_some();
        if let Some(r) = select {
            self.select(Some(r));
            if !has_eye {
                self.focus();
            }
        }
        if walk {
            self.enter_walk(self.camera.eye);
        }
        self.refill = true;
    }

    /// Starts walking at `at` (the ground below it).
    fn enter_walk(&mut self, at: [f32; 3]) {
        let (Some(level), Some(input)) = (&self.level, &self.input) else {
            return;
        };
        self.walk.enter(at, level, &self.catalog, &input.parents);
        self.drag = None;
        self.hover_axis = None;
    }

    /// The next edit of the environment preset, once the level is ready:
    /// `move:<dx>,<dy>,<dz>`, `turn:<degrees>` (about Y), `duplicate`,
    /// `remove`, `add:<metamodel id>[:clone]`.
    fn preset_action(&mut self) -> Option<LevelAction> {
        if self.job.is_some() || !self.pending.is_empty() || self.level.is_none() {
            return None;
        }
        let r = self.filter.selected?;
        let text = self.preset.as_mut()?.actions.pop_front()?;
        let level = self.level.as_ref()?;
        let group = level.groups.get(r.group)?.location;
        let transform = level.groups[r.group].objects.get(r.object)?.transform;
        let (name, value) = text.split_once(':').unwrap_or((&text, ""));
        let floats: Vec<f32> = value.split(',').filter_map(|v| v.parse().ok()).collect();
        let action = match name {
            "move" if floats.len() == 3 => {
                let mut m = transform?;
                for i in 0..3 {
                    m[3][i] += floats[i];
                }
                LevelAction::SetTransform {
                    group,
                    object: r.object,
                    matrix: m,
                }
            }
            "turn" if floats.len() == 1 => LevelAction::SetTransform {
                group,
                object: r.object,
                matrix: gizmo::apply(gizmo::Mode::Rotate, &transform?, 1, floats[0].to_radians()),
            },
            "duplicate" => LevelAction::Duplicate {
                group,
                object: r.object,
            },
            "remove" => LevelAction::Remove {
                group,
                object: r.object,
            },
            "add" => {
                let (id, clone) = value.split_once(':').unwrap_or((value, ""));
                let id = u32::from_str_radix(id, 16).ok()?;
                let found = level.source.find(TYPE_METAMODEL, id)?;
                let model = match (clone, found) {
                    ("clone", Found::Game(at, _)) => ModelChoice::Clone(at),
                    _ => ModelChoice::Existing(id),
                };
                let m = transform?;
                LevelAction::Add {
                    group,
                    template: (group, r.object),
                    model,
                    position: [m[3][0] + 3.0, m[3][1], m[3][2]],
                }
            }
            _ => {
                eprintln!("level: unknown action `{text}`");
                return None;
            }
        };
        eprintln!("level: action `{text}` on {}:{}", r.group, r.object);
        Some(action)
    }

    // ---------------------------------------------------------- after edits

    /// Reads the edited SimGroups (and cloned models) from the workspace and
    /// updates the scene. `select` selects an object afterwards.
    pub fn refresh(&mut self, ws: Option<&Workspace>, select: Option<(Location, usize)>) {
        let ctx = self.ctx.clone();
        if let Some(input) = &self.input {
            self.nav.reload(&input.game, ws);
        }
        let (Some(level), Some(input)) = (&mut self.level, &self.input) else {
            return;
        };
        let locations: Vec<Location> = level.groups.iter().map(|g| g.location).collect();
        let overrides = overrides(&input.game, ws, &locations);
        let source = Arc::new(LevelSource::new(
            input.game.clone(),
            input.lookup.clone(),
            overrides.clone(),
            level.source.scope.clone(),
        ));
        level.source = source.clone();
        let mut changed = false;
        for (g, location) in locations.iter().enumerate() {
            let data = match source.resource_data(*location) {
                Ok(data) => data,
                Err(error) => {
                    self.status = trf("level.cannot_read_group", &[("error", &error)]);
                    continue;
                }
            };
            if data != level.groups[g].group.encode() {
                if let Err(error) = level.set_group(g, &data) {
                    self.status = trf("level.cannot_read_group", &[("error", &error)]);
                    continue;
                }
                self.group_generation[g] += 1;
                changed = true;
            }
        }
        if !changed && select.is_none() {
            return;
        }
        // Meshes of new models.
        let new: Vec<(u32, Option<Found>)> = level
            .meshes
            .iter()
            .filter(|m| m.found.is_some())
            .filter(|m| !self.uploaded.contains(&loader::mesh_key(m.id, m.found)))
            .map(|m| (m.id, m.found))
            .collect();
        self.coverage = level.coverage();
        if !new.is_empty()
            && self.job.is_none()
            && let Some(assets) = &self.assets
        {
            self.job = Some(loader::more(
                source,
                assets.clone(),
                new,
                TEXTURE_LIMIT,
                ctx.clone(),
            ));
        }
        self.rebuild_items();
        self.walk.level_changed();
        self.drag = None;
        if let Some((location, object)) = select
            && let Some(group) = self
                .level
                .as_ref()
                .and_then(|l| l.groups.iter().position(|g| g.location == location))
        {
            self.select(Some(ObjRef { group, object }));
        } else if let Some(r) = self.filter.selected {
            // Keep the selection if the object is still there.
            let exists = self
                .level
                .as_ref()
                .is_some_and(|l| r.object < l.groups[r.group].objects.len());
            self.select(exists.then_some(r));
        }
        self.generation += 1;
    }

    /// Selects object `object` of the SimGroup at `group` and flies to it,
    /// loading its zone first when the level does not have it.
    pub fn show_object(&mut self, group: Location, object: usize) {
        if let Some(level) = &self.level
            && let Some(g) = level.groups.iter().position(|x| x.location == group)
            && object < level.groups[g].objects.len()
        {
            self.select(Some(ObjRef { group: g, object }));
            self.focus();
            return;
        }
        self.chosen = BTreeSet::from([group.stream]);
        self.goto = Some((group, object));
        self.pending_autoload = true;
    }

    /// The selected object: its SimGroup and its number there.
    pub fn selected_location(&self) -> Option<(Location, usize)> {
        let r = self.filter.selected?;
        let level = self.level.as_ref()?;
        Some((level.groups.get(r.group)?.location, r.object))
    }

    /// GUIDs of every object of the loaded level.
    pub fn guids(&self) -> HashSet<doctor_toolkit::resource::simgroup::Guid> {
        self.level.as_ref().map(|l| l.guids()).unwrap_or_default()
    }

    /// Closes the stream files the level reads, before the game files are replaced.
    pub fn release_files(&mut self) {
        self.job = None;
        if let Some(level) = &self.level {
            level.source.close_streams();
        }
    }

    /// The game files changed (apply, revert, restore, mods): index them
    /// again and reload the loaded zones, keeping the camera.
    pub fn game_files_changed(&mut self, ws: Option<&Workspace>) {
        if self.level.is_none() && self.job.is_none() {
            // Nothing loaded: index again when zones are loaded next.
            self.index_stale = true;
            return;
        }
        self.reindex();
        self.walk.forget_cache();
        let loaded = self.level.as_ref().map(|l| l.streams.clone());
        let camera = self.camera;
        let selected = self.filter.selected;
        if let Some(streams) = loaded {
            self.chosen = streams.into_iter().collect();
            let ctx = self.ctx.clone();
            self.start_load(&ctx, ws);
            self.keep_view = Some((camera, selected));
        }
    }

    /// Indexes the game files again (their layout changed).
    fn reindex(&mut self) {
        let Some(input) = &self.input else { return };
        let game = input.game.clone();
        let (index, _) = game.build_index(|_| {});
        let loaded = self.level.as_ref().map(|l| l.streams.clone());
        let chosen = std::mem::take(&mut self.chosen);
        self.open_game(game, Arc::new(index));
        self.chosen = chosen;
        if let Some(streams) = loaded {
            self.chosen = streams.into_iter().collect();
        }
        self.index_stale = false;
    }

    /// Zones (catalog paths or streams) to load when the game is opened.
    pub fn load_on_open(&mut self, zones: Vec<String>) {
        self.autoload = Some(zones);
    }

    /// Fills in the "Add object" panel for a wizard: the metamodel `model`,
    /// cloned into the zone (with its own collision) or used as it is.
    pub fn prefill_add(&mut self, model: u32, clone: bool, collision: bool) {
        self.add_model = format!("{model:08x}");
        self.add_clone = clone;
        self.add_collision = clone && collision;
        self.add_fixed = true;
    }

    /// Clears the selection (after the selected object was removed).
    pub fn clear_selection(&mut self) {
        self.filter.selected = None;
        self.logic = None;
        self.refill = true;
    }

    // ---------------------------------------------------------- selection

    fn select(&mut self, r: Option<ObjRef>) {
        if r.is_some() {
            self.nav.select(None);
        }
        if self.filter.selected != r {
            self.confirm_remove = false;
            if let Some(r) = r
                && let Some(level) = &self.level
            {
                let object = &level.groups[r.group].objects[r.object];
                if !self.add_fixed {
                    self.add_model = object
                        .model
                        .map_or_else(String::new, |m| format!("{m:08x}"));
                }
            }
        }
        self.filter.selected = r;
        self.refill = true;
        if let Some(r) = r {
            self.sync_logic(r);
        }
    }

    /// The object editor shows the selected object's SimGroup.
    fn sync_logic(&mut self, r: ObjRef) {
        let Some(level) = &self.level else { return };
        let generation = self.group_generation.get(r.group).copied().unwrap_or(0);
        let current = self.logic.as_ref().is_some_and(|(g, generation_shown, _)| {
            *g == r.group && *generation_shown == generation
        });
        if !current {
            let group = &level.groups[r.group];
            let data = group.group.encode();
            let original = level
                .source
                .stream(group.location.stream)
                .ok()
                .and_then(|s| s.resource_data(group.location.resource).ok())
                .filter(|o| *o != data);
            let view = LogicView::from_data(LOGIC_SIMGROUP, Ok(data), original);
            self.logic = Some((r.group, generation, view));
        }
        if let Some((_, _, view)) = &mut self.logic
            && view.selected_object() != Some(r.object)
        {
            view.select_object(r.object);
        }
    }

    fn object_transform(&self, r: ObjRef) -> Option<RowMatrix> {
        if let Some(drag) = &self.drag
            && drag.object == r
        {
            return Some(drag.current);
        }
        self.level
            .as_ref()?
            .groups
            .get(r.group)?
            .objects
            .get(r.object)?
            .transform
    }

    /// Collision wireframe lines of object `r` placed by `world`.
    fn collision_lines(&mut self, r: ObjRef, world: &RowMatrix) -> Vec<crate::render::LineVertex> {
        let Some(level) = &self.level else {
            return Vec::new();
        };
        let Some(model) = level
            .groups
            .get(r.group)
            .and_then(|g| g.objects.get(r.object))
            .and_then(|o| o.model)
            .and_then(|id| level.models.get(&id))
        else {
            return Vec::new();
        };
        let mut lines = Vec::new();
        for marker in &model.desc.collisions {
            let wire = self
                .collisions
                .entry(marker.resource)
                .or_insert_with(|| {
                    level
                        .source
                        .collision(marker.resource)
                        .ok()
                        .map(|mesh| Arc::new(crate::collision_view::Wire::new(&mesh)))
                })
                .clone();
            if let Some(wire) = wire {
                let placed = doctor_toolkit::level::compose(&marker.matrix, world);
                lines.extend(wire.lines(Some(&placed)));
            }
        }
        lines
    }

    /// Flies the camera to the selected object.
    fn focus(&mut self) {
        let Some(r) = self.filter.selected else {
            return;
        };
        let Some(m) = self.object_transform(r) else {
            return;
        };
        let mut bounds = math::Aabb::EMPTY;
        if let (Some(indices), Some(gpu)) = (self.by_object.get(&r), &self.gpu) {
            let renderer = gpu.lock();
            for &i in indices {
                let item = &self.items[i];
                if let Some(mesh) = renderer.mesh_data(item.mesh) {
                    bounds.union(&mesh.bounds().transformed(&item.world));
                }
            }
        }
        let (center, radius) = if bounds.is_empty() {
            ([m[3][0], m[3][1], m[3][2]], 2.0)
        } else {
            (bounds.center(), bounds.radius().clamp(1.0, 400.0))
        };
        self.camera.look_at(center, radius * 2.5 + 3.0);
        self.refill = true;
    }

    // ---------------------------------------------------------------- ui

    /// The whole view: browser on the left, details on the right, scene in
    /// the middle. Returns an edit to record.
    /// Follow the camera of the running game (stage 35).
    pub fn follow_game(&mut self, on: bool) {
        self.live_follow = on;
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, ws: Option<&Workspace>) -> Option<LevelAction> {
        let ctx = ui.ctx().clone();
        if std::mem::take(&mut self.pending_autoload) {
            self.start_load(&ctx, ws);
        }
        self.poll(&ctx);
        self.upload();
        if let (Some(level), Some(input)) = (&self.level, &self.input) {
            let mut streams: Vec<usize> = level.groups.iter().map(|g| g.location.stream).collect();
            streams.sort_unstable();
            streams.dedup();
            self.nav.ensure(&input.game, ws, &streams);
        }
        let mut action = self.preset_action();
        if let Some(a) = self.nav_action.take() {
            action = Some(a);
        }
        if std::mem::take(&mut self.play_here)
            && let Some(level) = &self.level
        {
            action = Some(LevelAction::PlayHere {
                eye: self.camera.eye,
                streams: level.streams.clone(),
            });
        }
        egui::Panel::left("level-browser")
            .resizable(true)
            .default_size(260.0)
            .max_size(420.0)
            .show(ui, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                let load = egui::ScrollArea::vertical()
                    .id_salt("level-browser-scroll")
                    .auto_shrink(false)
                    .show(ui, |ui| self.browser(ui))
                    .inner;
                if load {
                    self.start_load(&ctx, ws);
                }
            });
        if self.level.is_some() {
            egui::Panel::right("level-details")
                .resizable(true)
                .default_size(380.0)
                .max_size(640.0)
                .show(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    let details = egui::ScrollArea::vertical()
                        .id_salt("level-details-scroll")
                        .auto_shrink(false)
                        .show(ui, |ui| self.details(ui))
                        .inner;
                    if let Some(a) = details {
                        action = Some(a);
                    }
                });
        }
        egui::CentralPanel::default().show(ui, |ui| {
            if self.gpu.is_none() {
                ui.label(RichText::new(tr("level.needs_gpu")).weak());
                return;
            }
            if let Some(a) = self.scene_ui(ui) {
                action = Some(a);
            }
        });
        action
    }

    /// Zone list; returns whether to load the chosen zones.
    fn browser(&mut self, ui: &mut egui::Ui) -> bool {
        let mut load = false;
        ui.horizontal(|ui| {
            let busy = self.job.is_some();
            let text = trf(
                "level.load_zones",
                &[("zones", &trn("count.zones", self.chosen.len() as u64, &[]))],
            );
            if ui
                .add_enabled(!self.chosen.is_empty() && !busy, egui::Button::new(text))
                .on_hover_text(tr("level.load_zones_hint"))
                .clicked()
            {
                load = true;
            }
            if ui.button(tr("level.clear")).clicked() {
                self.chosen.clear();
            }
        });
        if self.level.is_some()
            && ui
                .button(tr("gametab.play_here"))
                .on_hover_text(tr("gametab.play_here_hint"))
                .clicked()
        {
            self.play_here = true;
        }
        if let Some(job) = &self.job {
            let total = job.total.load(Ordering::Relaxed).max(1);
            let done = job.done.load(Ordering::Relaxed);
            ui.add(egui::ProgressBar::new(done as f32 / total as f32).text(trf(
                "level.meshes_progress",
                &[("done", &done), ("total", &total)],
            )));
        }
        if !self.status.is_empty() {
            ui.label(RichText::new(&self.status).weak());
        }
        ui.add(
            egui::TextEdit::singleline(&mut self.zone_filter)
                .hint_text(tr("level.filter_zones"))
                .desired_width(ui.available_width()),
        );
        ui.separator();
        let filter = self.zone_filter.to_ascii_lowercase();
        let catalog = self.catalog.clone();
        egui::ScrollArea::vertical()
            .id_salt("level-zones")
            .max_height(ui.available_height() * if self.level.is_some() { 0.4 } else { 1.0 })
            .show(ui, |ui| {
                for level in catalog.levels() {
                    let zones: Vec<_> = catalog
                        .under(level)
                        .filter(|z| filter.is_empty() || z.name.contains(&filter))
                        .collect();
                    if zones.is_empty() {
                        continue;
                    }
                    let chosen = zones
                        .iter()
                        .filter(|z| self.chosen.contains(&z.stream))
                        .count();
                    let header = match chosen {
                        0 => format!("{level} ({})", zones.len()),
                        n => trf(
                            "level.zones_header_on",
                            &[("level", &level), ("count", &zones.len()), ("on", &n)],
                        ),
                    };
                    egui::CollapsingHeader::new(header)
                        .id_salt(("level", level))
                        .default_open(!filter.is_empty())
                        .show(ui, |ui| {
                            let mut dirs: Vec<&str> = zones.iter().map(|z| z.dir()).collect();
                            dirs.dedup();
                            for dir in dirs {
                                let in_dir: Vec<_> =
                                    zones.iter().filter(|z| z.dir() == dir).collect();
                                let all = in_dir.iter().all(|z| self.chosen.contains(&z.stream));
                                let mut check = all;
                                let label = format!("{dir}/ ({})", in_dir.len());
                                ui.horizontal(|ui| {
                                    if ui.checkbox(&mut check, "").changed() {
                                        for z in &in_dir {
                                            if check {
                                                self.chosen.insert(z.stream);
                                            } else {
                                                self.chosen.remove(&z.stream);
                                            }
                                        }
                                    }
                                    egui::CollapsingHeader::new(label)
                                        .id_salt(("dir", dir))
                                        .show(ui, |ui| {
                                            for z in &in_dir {
                                                let mut on = self.chosen.contains(&z.stream);
                                                let name =
                                                    z.name.rsplit('/').next().unwrap_or(&z.name);
                                                let text = trf(
                                                    "level.zone_entry",
                                                    &[
                                                        ("name", &name),
                                                        (
                                                            "meshes",
                                                            &trn(
                                                                "count.meshes",
                                                                z.meshes as u64,
                                                                &[],
                                                            ),
                                                        ),
                                                        (
                                                            "mb",
                                                            &format!("{:.1}", z.bytes as f64 / 1e6),
                                                        ),
                                                    ],
                                                );
                                                if ui.checkbox(&mut on, text).changed() {
                                                    if on {
                                                        self.chosen.insert(z.stream);
                                                    } else {
                                                        self.chosen.remove(&z.stream);
                                                    }
                                                }
                                            }
                                        });
                                });
                            }
                        });
                }
            });
        if self.level.is_some() {
            ui.separator();
            self.filters(ui);
            ui.separator();
            let selected = self.filter.selected.and_then(|r| self.object_transform(r));
            match self.walk.panel(ui, &self.camera, selected.is_some()) {
                Some(walk::PanelAction::EnterAtCamera) => self.enter_walk(self.camera.eye),
                Some(walk::PanelAction::EnterAtSelected) => {
                    if let Some(m) = selected {
                        // Beside the object, on what is below.
                        self.enter_walk([m[3][0] + 1.5, m[3][1] + 2.0, m[3][2]]);
                    }
                }
                None => {}
            }
        }
        load
    }

    fn filters(&mut self, ui: &mut egui::Ui) {
        let before = self.filter.clone();
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.filter.meshes, tr("level.models"));
            ui.checkbox(&mut self.filter.markers, tr("level.markers"))
                .on_hover_text(tr("level.markers_hint"));
            ui.checkbox(&mut self.filter.trees, tr("level.trees"))
                .on_hover_text(tr("level.trees_hint"));
            ui.checkbox(&mut self.filter.points, tr("level.ai_points"))
                .on_hover_text(tr("level.ai_points_hint"));
        });
        ui.horizontal(|ui| {
            ui.label("LOD");
            ui.selectable_value(
                &mut self.filter.lod,
                LodMode::Distance,
                tr("level.lod_distance"),
            );
            ui.selectable_value(
                &mut self.filter.lod,
                LodMode::Finest,
                tr("level.lod_finest"),
            );
            if ui
                .checkbox(&mut self.hide_tiny, tr("level.skip_tiny"))
                .on_hover_text(tr("level.skip_tiny_hint"))
                .changed()
            {
                self.refill = true;
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.scene.options.textures, tr("level.textures"));
            ui.checkbox(&mut self.scene.options.lighting, tr("level.lighting"));
            ui.checkbox(&mut self.scene.options.backface_culling, tr("level.cull"));
            ui.checkbox(&mut self.scene.options.wireframe, tr("level.wireframe"));
            ui.checkbox(&mut self.show_collision, tr("level.collision"))
                .on_hover_text(tr("level.collision_hint"));
        });
        ui.horizontal(|ui| {
            ui.label(tr("level.search"));
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text(tr("level.search_hint"))
                        .desired_width(ui.available_width()),
                )
                .changed()
            {
                self.run_search();
            }
        });
        let mut clicked = None;
        if !self.results.is_empty() {
            egui::ScrollArea::vertical()
                .id_salt("level-search")
                .max_height(160.0)
                .show(ui, |ui| {
                    let Some(level) = &self.level else { return };
                    for &r in &self.results {
                        let o = &level.groups[r.group].objects[r.object];
                        let text = format!(
                            "{}:{} {} {}",
                            r.group,
                            r.object,
                            o.class_name.unwrap_or("?"),
                            o.name
                        );
                        if ui
                            .selectable_label(self.filter.selected == Some(r), text)
                            .clicked()
                        {
                            clicked = Some(r);
                        }
                    }
                });
        }
        if let Some(r) = clicked {
            self.select(Some(r));
            self.focus();
        }
        egui::CollapsingHeader::new(trf("level.classes", &[("count", &self.classes.len())]))
            .id_salt("level-classes")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.small_button(tr("level.show_all")).clicked() {
                        self.filter.hidden_classes.clear();
                    }
                    if ui.small_button(tr("level.hide_all")).clicked() {
                        self.filter.hidden_classes = self.classes.iter().map(|c| c.0).collect();
                    }
                });
                egui::ScrollArea::vertical()
                    .id_salt("level-class-list")
                    .max_height(260.0)
                    .show(ui, |ui| {
                        for (class, name, count) in &self.classes {
                            let mut on = !self.filter.hidden_classes.contains(class);
                            if ui.checkbox(&mut on, format!("{name} ({count})")).changed() {
                                if on {
                                    self.filter.hidden_classes.remove(class);
                                } else {
                                    self.filter.hidden_classes.insert(*class);
                                }
                            }
                        }
                    });
            });
        if self.filter != before {
            self.refill = true;
        }
        egui::CollapsingHeader::new(tr("navgraph.title"))
            .id_salt("level-navgraph")
            .default_open(self.nav.visible)
            .show(ui, |ui| {
                let object_at = self
                    .filter
                    .selected
                    .and_then(|r| self.object_transform(r))
                    .map(|m| [m[3][0], m[3][1], m[3][2]]);
                let target = math::add(self.camera.eye, math::scale(self.camera.forward(), 10.0));
                match self.nav.panel(ui, object_at, target) {
                    Some(Ok(a)) => self.nav_action = Some(a),
                    Some(Err(e)) => self.status = e,
                    None => {}
                }
                if self.nav.selected.is_some() && self.filter.selected.is_some() {
                    self.filter.selected = None;
                    self.logic = None;
                    self.refill = true;
                }
            });
    }

    fn run_search(&mut self) {
        self.results.clear();
        let needle = self.search.trim().to_lowercase();
        let Some(level) = &self.level else { return };
        if needle.is_empty() {
            return;
        }
        for (g, group) in level.groups.iter().enumerate() {
            for o in &group.objects {
                let class = o.class_name.unwrap_or_default().to_lowercase();
                if o.name.to_lowercase().contains(&needle)
                    || class.contains(&needle)
                    || o.guid.to_string().contains(&needle)
                {
                    self.results.push(ObjRef {
                        group: g,
                        object: o.index,
                    });
                    if self.results.len() >= 500 {
                        return;
                    }
                }
            }
        }
    }

    /// The right panel: the selected object, edit buttons, the object editor.
    fn details(&mut self, ui: &mut egui::Ui) -> Option<LevelAction> {
        let c = self.coverage;
        ui.label(
            RichText::new(trf(
                "level.coverage",
                &[
                    ("zones", &trn("count.zones", c.streams as u64, &[])),
                    ("scope", &c.scope_streams),
                    ("objects", &trn("count.objects", c.objects as u64, &[])),
                    ("placed", &c.placed),
                    ("with_model", &c.with_model),
                    ("drawn", &c.rendered),
                    (
                        "instances",
                        &trn("count.instances", c.instances as u64, &[]),
                    ),
                    ("meshes", &trn("count.meshes", c.meshes as u64, &[])),
                    ("missing", &c.meshes_missing),
                ],
            ))
            .small(),
        );
        if !self.info.is_empty() {
            ui.label(RichText::new(&self.info).weak().small());
        }
        if !self.failed.is_empty() {
            ui.label(
                RichText::new(trf(
                    "level.meshes_failed",
                    &[
                        (
                            "meshes",
                            &trn("count.meshes", self.failed.len() as u64, &[]),
                        ),
                        ("first", &format!("{:08x}", self.failed[0].0)),
                        ("error", &self.failed[0].1),
                    ],
                ))
                .color(Color32::from_rgb(230, 120, 80))
                .small(),
            );
        }
        ui.separator();
        let r = self.filter.selected?;
        let (location, object, has_transform, model) = {
            let level = self.level.as_ref()?;
            let group = level.groups.get(r.group)?;
            let o = group.objects.get(r.object)?;
            (group.location, o.clone(), o.transform.is_some(), o.model)
        };
        let input = self.input.as_ref()?;
        let (game, parents) = (input.game.clone(), input.parents.clone());
        let stream_name = game.streams()[location.stream].stream_name();
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!(
                "{} {}",
                object.class_name.unwrap_or("?"),
                object.name
            ));
            ui.label(
                RichText::new(trf(
                    "level.object_place",
                    &[
                        ("stream", &stream_name),
                        ("resource", &location.resource),
                        ("object", &r.object),
                    ],
                ))
                .weak(),
            );
        });
        let mut action = None;
        ui.horizontal(|ui| {
            ui.label(tr("level.gizmo"));
            ui.selectable_value(&mut self.gizmo, gizmo::Mode::Move, tr("level.gizmo_move"));
            ui.selectable_value(
                &mut self.gizmo,
                gizmo::Mode::Rotate,
                tr("level.gizmo_rotate"),
            );
            if ui.button(tr("level.fly_to")).clicked() {
                self.focus();
            }
            if ui
                .button(tr("logic.open_from_level"))
                .on_hover_text(tr("logic.open_from_level_hint"))
                .clicked()
            {
                action = Some(LevelAction::ShowLogic {
                    group: location,
                    object: r.object,
                });
            }
        });
        ui.horizontal(|ui| {
            if ui
                .button(tr("level.duplicate"))
                .on_hover_text(tr("level.duplicate_hint"))
                .clicked()
            {
                action = Some(LevelAction::Duplicate {
                    group: location,
                    object: r.object,
                });
            }
            let text = if self.confirm_remove {
                tr("level.remove_confirm")
            } else {
                tr("level.remove")
            };
            if ui.button(text).clicked() {
                if self.confirm_remove {
                    action = Some(LevelAction::Remove {
                        group: location,
                        object: r.object,
                    });
                    self.confirm_remove = false;
                } else {
                    self.confirm_remove = true;
                }
            }
        });
        if let Some(m) = model
            && ui
                .button(tr("level.retexture"))
                .on_hover_text(tr("level.retexture_hint"))
                .clicked()
        {
            action = Some(LevelAction::StartWizard {
                task: "retexture",
                model: Some(m),
            });
        }
        egui::CollapsingHeader::new(tr("level.add_object"))
            .id_salt("level-add")
            .default_open(self.add_fixed)
            .show(ui, |ui| {
                ui.label(RichText::new(tr("level.add_object_hint")).weak().small());
                ui.horizontal(|ui| {
                    ui.label(tr("level.metamodel_id"));
                    ui.add(egui::TextEdit::singleline(&mut self.add_model).desired_width(90.0));
                    ui.checkbox(&mut self.add_clone, tr("level.clone"))
                        .on_hover_text(tr("level.clone_hint"));
                    ui.add_enabled(
                        self.add_clone,
                        egui::Checkbox::new(&mut self.add_collision, tr("level.own_collision")),
                    )
                    .on_hover_text(tr("level.own_collision_hint"));
                });
                let id = u32::from_str_radix(self.add_model.trim(), 16).ok();
                let level = self.level.as_ref();
                let found = id.and_then(|id| level?.source.find(TYPE_METAMODEL, id));
                let ancestors = doctor_toolkit::game::ancestors(&parents, &stream_name);
                let reachable = found.is_some_and(|f| {
                    let s = game.streams()[f.stream()].stream_name();
                    f.stream() == location.stream || ancestors.contains(&s)
                });
                let note = match (id, found) {
                    (None, _) => tr("level.note_type_id").to_owned(),
                    (Some(_), None) => tr("level.note_not_found").to_owned(),
                    (Some(_), Some(f)) if self.add_clone => match f {
                        Found::Game(..) => tr("level.note_clone_here").to_owned(),
                        Found::Override(..) => tr("level.note_clone_game_only").to_owned(),
                    },
                    (Some(_), Some(_)) if reachable => tr("level.note_reachable").to_owned(),
                    (Some(_), Some(f)) => trf(
                        "level.note_unreachable",
                        &[("stream", &game.streams()[f.stream()].stream_name())],
                    ),
                };
                ui.label(RichText::new(note).small());
                if !self.add_fixed
                    && ui
                        .small_button(tr("level.place_wizard"))
                        .on_hover_text(tr("level.place_wizard_hint"))
                        .clicked()
                {
                    action = Some(LevelAction::StartWizard {
                        task: "place",
                        model: id,
                    });
                }
                let can_add = matches!(
                    (found, self.add_clone),
                    (Some(Found::Game(..)), true) | (Some(_), false)
                );
                if ui
                    .add_enabled(
                        can_add && has_transform,
                        egui::Button::new(tr("level.add_in_front")),
                    )
                    .clicked()
                    && let (Some(id), Some(f)) = (id, found)
                {
                    let model = match (self.add_clone, f) {
                        (true, Found::Game(at, _)) if self.add_collision => {
                            ModelChoice::CloneWithCollision(at, CollisionKind::Mesh, None)
                        }
                        (true, Found::Game(at, _)) => ModelChoice::Clone(at),
                        _ => ModelChoice::Existing(id),
                    };
                    let p = math::add(self.camera.eye, math::scale(self.camera.forward(), 10.0));
                    action = Some(LevelAction::Add {
                        group: location,
                        template: (location, r.object),
                        model,
                        position: p,
                    });
                }
            });
        if model.is_some() {
            let level = self.level.as_ref()?;
            let m = &level.models[&model?];
            ui.label(
                RichText::new(trf(
                    "level.model_info",
                    &[
                        ("model", &format!("{:08x}", model?)),
                        (
                            "meshes",
                            &trn("count.meshes", m.desc.pieces.len() as u64, &[]),
                        ),
                        ("trees", &trn("count.trees", m.desc.trees.len() as u64, &[])),
                        (
                            "nested",
                            &trn(
                                "count.nested_metamodels",
                                m.desc.metamodels.saturating_sub(1) as u64,
                                &[],
                            ),
                        ),
                        (
                            "where",
                            &match m.found {
                                None => tr("level.model_not_found").to_owned(),
                                Some(f) => trf(
                                    "level.model_in",
                                    &[("stream", &game.streams()[f.stream()].stream_name())],
                                ),
                            },
                        ),
                    ],
                ))
                .small(),
            );
        }
        ui.separator();
        let mut reselect = None;
        if let Some((group, _, view)) = &mut self.logic {
            let mut status = String::new();
            match view.show(ui, &mut status) {
                Some(LogicAction::Save { data, label }) => {
                    action = Some(LevelAction::Save {
                        group: location,
                        data,
                        label,
                    });
                }
                Some(LogicAction::OpenGraph { object }) => {
                    action = Some(LevelAction::ShowLogic {
                        group: location,
                        object: object.unwrap_or(r.object),
                    });
                }
                None => {}
            }
            if !status.is_empty() {
                self.status = status;
            }
            // A click in the object list selects in the scene.
            if let Some(o) = view.selected_object()
                && o != r.object
            {
                reselect = Some(ObjRef {
                    group: *group,
                    object: o,
                });
            }
        }
        if reselect.is_some() {
            self.select(reselect);
        }
        action
    }

    // ------------------------------------------------------------- scene

    fn spin(&mut self, ui: &egui::Ui) {
        let Some(intervals) = self.preset.as_mut().and_then(|p| p.spin.as_mut()) else {
            return;
        };
        if self.job.is_some() || !self.pending.is_empty() || self.level.is_none() {
            ui.ctx().request_repaint();
            return;
        }
        // Orbit the point the camera looks at, as far as the level centre.
        let distance = math::length(math::sub(self.center, self.camera.eye)).max(20.0);
        let pivot = math::add(
            self.camera.eye,
            math::scale(self.camera.forward(), distance),
        );
        self.camera.yaw += 0.01;
        self.camera.look_at(pivot, distance);
        self.refill = true;
        intervals.push(ui.input(|i| i.unstable_dt));
        if intervals.len() == 300 {
            let mut sorted = intervals.clone();
            sorted.sort_by(f32::total_cmp);
            eprintln!(
                "level view: {} frames, interval median {:.2} ms, p99 {:.2} ms; last render {:.2} ms CPU, {} instances drawn, {} culled, {} draw calls",
                sorted.len(),
                sorted[sorted.len() / 2] * 1000.0,
                sorted[sorted.len() * 99 / 100] * 1000.0,
                self.stats.cpu_time.as_secs_f64() * 1000.0,
                self.stats.instances,
                self.stats.culled,
                self.stats.draw_calls,
            );
            intervals.clear();
        }
        ui.ctx().request_repaint();
    }

    fn scene_ui(&mut self, ui: &mut egui::Ui) -> Option<LevelAction> {
        if self.level.is_none() {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new(if self.job.is_some() {
                        tr("level.loading")
                    } else {
                        tr("level.empty")
                    })
                    .weak(),
                );
            });
            return None;
        }
        self.spin(ui);
        // Game → editor: the camera follows the game's.
        match self.live.and_then(|m| m.game_camera) {
            Some(c) if self.live_follow => {
                if live::fly_from_game(&c, &mut self.camera) {
                    self.refill = true;
                    self.camera_moved = true;
                }
            }
            None if self.live.is_none() => self.live_follow = false,
            _ => {}
        }
        let size = ui.available_size().max(Vec2::splat(64.0));
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let ppp = ui.ctx().pixels_per_point();
        let pixels = [
            (rect.width() * ppp).round().max(1.0) as u32,
            (rect.height() * ppp).round().max(1.0) as u32,
        ];
        let mut action = None;
        if response.hovered() && !ui.ctx().egui_wants_keyboard_input() {
            let (f, one, two) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::F),
                    i.key_pressed(egui::Key::Num1),
                    i.key_pressed(egui::Key::Num2),
                )
            });
            if f {
                if self.filter.selected.is_some() {
                    self.focus();
                } else {
                    self.frame_all();
                }
            }
            if one {
                self.gizmo = gizmo::Mode::Move;
            }
            if two {
                self.gizmo = gizmo::Mode::Rotate;
            }
        }

        // Gizmo hover and drag.
        let view = self.camera.view(rect.aspect_ratio());
        let to_screen = |p: [f32; 3]| {
            view.project(p).map(|(uv, _)| {
                [
                    rect.min.x + uv[0] * rect.width(),
                    rect.min.y + uv[1] * rect.height(),
                ]
            })
        };
        let selected = self.filter.selected;
        let node = self
            .nav
            .selected
            .filter(|_| selected.is_none() && self.nav.visible);
        let gizmo_mode = if node.is_some() {
            gizmo::Mode::Move
        } else {
            self.gizmo
        };
        let gizmo_at = selected
            .and_then(|r| self.object_transform(r))
            .map(|m| [m[3][0], m[3][1], m[3][2]])
            .or_else(|| node.and_then(|r| self.nav.position(r)));
        let pointer = response
            .hover_pos()
            .or(ui.input(|i| i.pointer.interact_pos()));
        if self.walk.active {
            self.hover_axis = None;
        } else if let (Some(origin), None) = (gizmo_at, &self.drag) {
            let size = gizmo::size(&view, origin);
            self.hover_axis = pointer
                .filter(|_| response.hovered())
                .and_then(|p| gizmo::hit(gizmo_mode, origin, size, [p.x, p.y], &to_screen));
        }
        let ray_at = |p: Pos2| {
            let uv = (p - rect.min) / rect.size();
            view.ray([uv.x, uv.y])
        };
        let alt = ui.input(|i| i.modifiers.alt);
        if response.drag_started_by(egui::PointerButton::Primary)
            && !alt
            && let (Some(axis), Some(r), Some(origin), Some(p)) =
                (self.hover_axis, selected, gizmo_at, pointer)
            && let Some(start) = self.object_transform(r)
            && let Some(param) = gizmo::parameter(self.gizmo, origin, axis, &ray_at(p))
        {
            self.drag = Some(Drag {
                object: r,
                node: None,
                axis,
                start,
                start_param: param,
                current: start,
            });
        } else if response.drag_started_by(egui::PointerButton::Primary)
            && !alt
            && let (Some(axis), Some(n), Some(origin), Some(p)) =
                (self.hover_axis, node, gizmo_at, pointer)
            && let Some(param) = gizmo::parameter(gizmo_mode, origin, axis, &ray_at(p))
        {
            let mut start = doctor_toolkit::level::IDENTITY;
            start[3] = [origin[0], origin[1], origin[2], 1.0];
            self.drag = Some(Drag {
                object: ObjRef {
                    group: usize::MAX,
                    object: usize::MAX,
                },
                node: Some(n),
                axis,
                start,
                start_param: param,
                current: start,
            });
        }
        let mut moved = false;
        if let Some(drag) = &mut self.drag {
            let origin = [drag.start[3][0], drag.start[3][1], drag.start[3][2]];
            let mode = if drag.node.is_some() {
                gizmo::Mode::Move
            } else {
                self.gizmo
            };
            if let Some(p) = pointer
                && let Some(param) = gizmo::parameter(mode, origin, drag.axis, &ray_at(p))
            {
                let mut delta = param - drag.start_param;
                if mode == gizmo::Mode::Rotate {
                    // Shortest turn between the two angles.
                    delta = (delta + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
                        - std::f32::consts::PI;
                }
                if ui.input(|i| i.modifiers.ctrl) {
                    delta = gizmo::snap(mode, delta);
                }
                let m = gizmo::apply(mode, &drag.start, drag.axis, delta);
                if m != drag.current {
                    drag.current = m;
                    moved = true;
                }
            }
            if response.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
                let drag = self.drag.take().expect("dragging");
                if let Some(n) = drag.node {
                    if drag.current != drag.start {
                        let to = [drag.current[3][0], drag.current[3][1], drag.current[3][2]];
                        match self.nav.moved(n, to) {
                            Ok(a) => action = Some(a),
                            Err(e) => self.status = e,
                        }
                    } else {
                        self.nav.preview = None;
                    }
                } else if drag.current != drag.start
                    && let Some(level) = &self.level
                {
                    action = Some(LevelAction::SetTransform {
                        group: level.groups[drag.object.group].location,
                        object: drag.object.object,
                        matrix: drag.current,
                    });
                }
            }
        } else if let (Some(level), Some(input)) = (&self.level, &self.input)
            && self.walk.update(
                ui,
                &response,
                &mut self.camera,
                level,
                &self.catalog,
                &input.parents,
            )
        {
            self.refill = true;
            self.camera_moved = true;
        } else if !self.walk.active {
            let pivot = gizmo_at.unwrap_or_else(|| {
                math::add(self.camera.eye, math::scale(self.camera.forward(), 20.0))
            });
            if self.camera.handle_input(ui, &response, pivot) {
                self.refill = true;
                self.camera_moved = true;
            }
        }
        if moved
            && let Some(drag) = &self.drag
            && let Some(n) = drag.node
        {
            let m = drag.current;
            self.nav.preview = Some((n, [m[3][0], m[3][1], m[3][2]]));
            self.nav.generation += 1;
        } else if moved
            && let Some(drag) = &self.drag
            && let Some(indices) = self.by_object.get(&drag.object)
        {
            scene::move_items(&mut self.items, indices, &drag.current);
            self.refill = true;
        }

        let gpu = self.gpu.clone()?;
        let view = self.camera.view(rect.aspect_ratio());
        // Hover picking on GPUs; software adapters pick on click only.
        let software = gpu.lock().is_software();
        let mut hovered = self.filter.hovered;
        match response.hover_pos() {
            Some(pos) if !software && self.drag.is_none() && !response.dragged() => {
                if self.last_pick != Some(pos) {
                    self.last_pick = Some(pos);
                    hovered = self.pick(&gpu, &view, rect, pos, ppp);
                }
            }
            _ => {
                self.last_pick = None;
                if self.drag.is_none() {
                    hovered = None;
                }
            }
        }
        if hovered != self.filter.hovered {
            self.filter.hovered = hovered;
            self.refill = true;
        }
        let node_click = if response.clicked() && self.hover_axis.is_none() {
            response
                .interact_pointer_pos()
                .and_then(|p| self.nav.pick(&to_screen, [p.x, p.y], nav::PICK_RADIUS))
        } else {
            None
        };
        if let Some(n) = node_click {
            if ui.input(|i| i.modifiers.shift) && self.nav.selected.is_some() {
                match self.nav.link_to(n) {
                    Some(Ok(a)) => action = Some(a),
                    Some(Err(e)) => self.status = e,
                    None => {}
                }
            } else {
                self.select(None);
                self.nav.select(Some(n));
            }
        } else if response.clicked() && self.hover_axis.is_none() {
            self.nav.select(None);
            let picked = match response.interact_pointer_pos() {
                Some(pos) if software => self.pick(&gpu, &view, rect, pos, ppp),
                Some(_) => self.filter.hovered,
                None => None,
            };
            self.select(picked);
        }
        if response.double_clicked() {
            self.focus();
        }

        if self.refill {
            self.refill = false;
            self.filter.min_angle = if self.hide_tiny {
                TINY_PIXELS * self.camera.fov_y / pixels[1].max(1) as f32
            } else {
                0.0
            };
            let spheres = &self.spheres;
            self.visible = scene::fill(
                &mut self.scene,
                &self.items,
                self.camera.eye,
                &self.filter,
                &|key| spheres.get(&key).copied(),
            );
            self.generation += 1;
        }
        // Gizmo lines.
        self.scene.overlay_lines.clear();
        if let Some(r) = self.filter.selected
            && let Some(m) = self.object_transform(r)
        {
            let origin = [m[3][0], m[3][1], m[3][2]];
            let axis = self.drag.as_ref().map(|d| d.axis).or(self.hover_axis);
            self.scene.overlay_lines =
                gizmo::lines(self.gizmo, origin, gizmo::size(&view, origin), axis);
        } else if let Some(origin) = gizmo_at.filter(|_| node.is_some()) {
            let axis = self.drag.as_ref().map(|d| d.axis).or(self.hover_axis);
            self.scene.overlay_lines =
                gizmo::lines(gizmo::Mode::Move, origin, gizmo::size(&view, origin), axis);
        }
        self.scene.overlay_lines.extend(self.nav.lines());
        let collision = self
            .filter
            .selected
            .filter(|_| self.show_collision)
            .and_then(|r| Some((r, self.object_transform(r)?)));
        if let Some((r, m)) = collision {
            let lines = self.collision_lines(r, &m);
            self.scene.overlay_lines.extend(lines);
        }
        // Walking: problem marks, the route, and the collision of the
        // objects of the workspace.
        self.scene.overlay_lines.extend(self.walk.lines());
        if self.walk.active {
            let own: Vec<ObjRef> = self
                .walk
                .own()
                .iter()
                .map(|o| ObjRef {
                    group: o.object.0,
                    object: o.object.1,
                })
                .collect();
            for r in own {
                if let Some(m) = self.object_transform(r) {
                    let lines = self.collision_lines(r, &m);
                    self.scene.overlay_lines.extend(lines);
                }
            }
        }
        let key = RenderKey {
            camera: self.camera,
            size: pixels,
            generation: self.generation,
            options: self.scene.options,
            gizmo: (self.gizmo, self.hover_axis),
            collision,
            nav: self.nav.generation,
            walk: self.walk.revision,
        };
        if self.last_render.as_ref() != Some(&key) {
            let mut renderer = gpu.lock();
            renderer.render(&mut self.viewport, pixels, &self.scene, &view);
            self.stats = renderer.stats;
            self.last_render = Some(key);
        }
        if let Some(id) = self.viewport.texture_id() {
            ui.painter().image(
                id,
                rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        self.overlay_text(ui, rect);
        if let Some(marks) = self.live {
            live_marks(ui, rect, &to_screen, &marks);
            if let Some(a) = live::bar(ui, rect, &marks, &mut self.live_follow, &self.camera) {
                action = Some(a);
            }
        }
        action
    }

    fn pick(&mut self, gpu: &Gpu, view: &View, rect: Rect, pos: Pos2, ppp: f32) -> Option<ObjRef> {
        let mut renderer = gpu.lock();
        let pick = if renderer.is_software() {
            let uv = (pos - rect.min) / rect.size();
            let ray = view.ray([uv.x, uv.y]);
            let datas: HashMap<u64, Arc<MeshData>> = self
                .scene
                .instances
                .iter()
                .filter_map(|i| Some((i.mesh, renderer.mesh_data(i.mesh)?.clone())))
                .collect();
            let meshes: HashMap<u64, &MeshData> = datas.iter().map(|(k, v)| (*k, &**v)).collect();
            self.scene.ray_pick(&ray, &meshes)
        } else {
            let local = (pos - rect.min) * ppp;
            let pixel = [local.x.max(0.0) as u32, local.y.max(0.0) as u32];
            renderer.pick(&mut self.viewport, &self.scene, view, pixel)
        }?;
        let item = self.visible.get(pick.instance as usize)?;
        self.items.get(*item)?.object
    }

    fn overlay_text(&self, ui: &egui::Ui, rect: Rect) {
        let mut text = trf(
            "level.overlay_stats",
            &[
                ("drawn", &self.stats.instances),
                ("culled", &self.stats.culled),
                ("calls", &self.stats.draw_calls),
                (
                    "ms",
                    &format!("{:.2}", self.stats.cpu_time.as_secs_f64() * 1000.0),
                ),
            ],
        );
        if let Some(level) = &self.level {
            for r in [self.filter.hovered, self.filter.selected]
                .into_iter()
                .flatten()
            {
                let o = &level.groups[r.group].objects[r.object];
                let mut line = format!("\n{} {}", o.class_name.unwrap_or("?"), o.name);
                if let Some(m) = self.object_transform(r) {
                    let t = Transform::from_matrix(&m);
                    line += &trf(
                        "level.overlay_at",
                        &[
                            (
                                "position",
                                &format!(
                                    "{:.2} {:.2} {:.2}",
                                    t.position[0], t.position[1], t.position[2]
                                ),
                            ),
                            ("yaw", &format!("{:.1}", t.rotation[1])),
                        ],
                    );
                }
                text += &line;
            }
        }
        text += "\n";
        match &self.level {
            Some(level) if self.walk.active => text += &self.walk.hud(level),
            _ => text += tr("level.overlay_keys"),
        }
        let painter = ui.painter_at(rect);
        let galley = painter.layout_no_wrap(text, egui::FontId::monospace(11.0), Color32::WHITE);
        let at = rect.left_top() + Vec2::splat(6.0);
        painter.rect_filled(
            Rect::from_min_size(at, galley.size()).expand(4.0),
            3.0,
            Color32::from_black_alpha(150),
        );
        painter.galley(at, galley, Color32::WHITE);
    }
}

/// The player (green) and the camera (yellow) of the running game: a dot
/// and a line one metre (camera: three) along where they face.
fn live_marks(
    ui: &egui::Ui,
    rect: Rect,
    to_screen: &dyn Fn([f32; 3]) -> Option<[f32; 2]>,
    marks: &crate::app::LiveMarks,
) {
    let painter = ui.painter_at(rect);
    let mark = |at: [f32; 3], dir: [f32; 3], length: f32, color: Color32, label: &str| {
        let Some(p) = to_screen(at) else { return };
        let p = Pos2::new(p[0], p[1]);
        let tip = [
            at[0] + dir[0] * length,
            at[1] + dir[1] * length,
            at[2] + dir[2] * length,
        ];
        if let Some(t) = to_screen(tip) {
            painter.line_segment([p, Pos2::new(t[0], t[1])], egui::Stroke::new(2.0, color));
        }
        painter.circle(p, 5.0, color, egui::Stroke::new(1.5, Color32::BLACK));
        painter.text(
            p + Vec2::new(8.0, -8.0),
            egui::Align2::LEFT_BOTTOM,
            label,
            egui::FontId::proportional(12.0),
            color,
        );
    };
    if let Some((at, dir)) = marks.player {
        mark(
            at,
            dir,
            1.0,
            Color32::from_rgb(0x50, 0xe0, 0x60),
            tr("live.mark_player"),
        );
    }
    if let Some((at, dir)) = marks.camera {
        mark(
            at,
            dir,
            3.0,
            Color32::from_rgb(0xf0, 0xd0, 0x40),
            tr("live.mark_camera"),
        );
    }
}

/// Edited SimGroups of `groups` and edited or added models and meshes of the
/// workspace, keyed as the level looks them up.
pub fn overrides(game: &GameInstance, ws: Option<&Workspace>, groups: &[Location]) -> Overrides {
    let mut out = Overrides::default();
    let Some(ws) = ws else { return out };
    for change in ws.changes() {
        let t = change.key.type_id;
        if ![TYPE_METAMODEL, TYPE_MESH, TYPE_MESH_VOLATILE, TYPE_HKX].contains(&t) {
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
    let mut streams: HashMap<usize, Option<Arc<doctor_toolkit::game::StreamFile>>> = HashMap::new();
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
