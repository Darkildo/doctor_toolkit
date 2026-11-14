//! The level editor: a 3D scene of the placed objects of chosen zones.
//!
//! The tab is the view alone (stage 53); the shell's panels edit the rest:
//!
//! - **Zones** ([`zones`], the shell's Zones panel): zones of every level
//!   (streams with SimGroups) with check boxes — the chosen ones load
//!   together with their ancestors as scope — and what the view shows.
//! - **Scene** (the tab): one instance per mesh of every placed object, on
//!   the shared [`crate::render::SceneRenderer`] with frustum culling and
//!   LODs by distance; markers for objects without a mesh, trees and AI
//!   points. The camera, keys and mouse as in Unreal Engine ([`camera`],
//!   [`interact`], stage 54): the right button held looks and flies with
//!   W A S D Q E, Q W E R choose the gizmo; the camera as before is a
//!   setting. Click selects (GPU id pass, or a CPU ray on software
//!   adapters), Ctrl + click adds or takes away, a frame selects what is
//!   inside; the context menu; a bar over the view ([`viewbar`]) with the
//!   projection, lighting, what shows, snapping and the camera speed.
//! - **Gizmo**: select, move (also in a plane), rotate or scale (also
//!   evenly) along the world's or the object's axes, every selected object
//!   about their centre; Alt + drag moves copies; snapping always or with
//!   Ctrl (the main toolbar of the shell switches them, stage 52).
//! - **Selection, eye and lock**: the selected objects (one is primary: the
//!   gizmo and Details follow it), objects hidden in the view and objects
//!   that clicks in the view do not select ([`ObjectSet`]); the shell's
//!   Outliner and Details edit them through the methods of [`LevelView`].
//!
//! Edits are returned as [`LevelAction`]s and recorded in the workspace by
//! the app, which then calls [`LevelView::refresh`].
//!
//! `DOCTOR_TOOLKIT_GUI_LEVEL=<zone>[+<zone>…][;option…]` opens the view and
//! loads the zones once the game is indexed; options: `select=<group>:<object>`,
//! `near` (select the object with a model nearest to `look`),
//! `rotate`, `finest`, `points`, `eye=<x>,<y>,<z>`, `look=<x>,<y>,<z>`,
//! `yaw=<rad>`, `pitch=<rad>`, `act=<edit>` (see [`LevelView::preset_action`]),
//! `nav[=<graph>:<node>]` (show navigation graphs, select a node),
//! `spin` (turn and print frame times), `walk` (walk from the camera),
//! `route=<ax>,<ay>,<az>,<bx>,<by>,<bz>` (check the walk from A to B).
//!
//! **Walk** ([`walk`], stage 37): a first-person capsule with the Havok
//! collision of the loaded zones, gravity, steps; problem marks and A to B.

pub mod camera;
pub use camera::Projection;
pub mod cine;
pub mod drop;
mod gizmo;
pub use gizmo::{
    ANGLES as SNAP_ANGLES, Mode as GizmoMode, SCALES as SNAP_SCALES, STEPS as SNAP_STEPS, Snap,
    Space as GizmoSpace,
};
mod interact;
#[cfg(test)]
pub use interact::ContextItem;
#[cfg(test)]
pub use viewbar::Lighting;
mod live;
pub use live::{LiveEdit, ObjectAction};
pub mod loader;
mod marks;
pub mod nav;
mod preview;
pub mod scene;
mod viewbar;
pub mod walk;
mod zones;

pub use marks::Mark;
pub use scene::{ObjRef, ObjectSet};

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use doctor_toolkit::game::{GameInstance, ResourceIndex};
use doctor_toolkit::level::edit::ModelChoice;
use doctor_toolkit::level::{
    Catalog, Coverage, Found, Level, LevelAssets, LevelSource, Location, Lookup, Overrides,
    RowMatrix, TYPE_MESH, TYPE_MESH_VOLATILE, TYPE_METAMODEL, TYPE_SIMGROUP,
};
use doctor_toolkit::resource::hkx::TYPE_HKX;
use doctor_toolkit::resource::logic::TYPE_SIMGROUP as LOGIC_SIMGROUP;
use doctor_toolkit::resource::simgroup::{Guid, SimGroup};
use doctor_toolkit::workspace::{ResourceKey, Workspace};
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{Color32, Pos2, Rect, RichText, Sense, Vec2};

use crate::logic_view::LogicView;
use crate::render::{Gpu, MeshData, RenderStats, Scene, View, Viewport, math};
use camera::FlyCamera;
use loader::{Batch, LoadInput, LoadJob, Msg};
use scene::{Filter, Item, LodMode, MARKER_KEY};

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
    /// A copy of the object placed by `matrix` (Alt + drag of the gizmo:
    /// the copy moves, the object stays).
    DuplicateTo {
        group: Location,
        object: usize,
        matrix: RowMatrix,
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
    /// Edit the object in the running game (stage 47; writes are asked
    /// first, the gizmo's once).
    LiveObject(ObjectAction),
    /// An edited navigation graph; `renumbered` (after a removed node) makes
    /// links of other graphs into it follow.
    Graph {
        graph: Box<doctor_toolkit::level::navgraph::GraphRef>,
        renumbered: Option<Vec<Option<u16>>>,
        label: String,
    },
    /// Open the SimGroup resource in a tab of its own.
    OpenResource {
        group: Location,
    },
    /// Show the shell's Zones panel (the view has nothing loaded).
    ShowZones,
    /// Show the shell's Outliner and the selected object in it.
    ShowInOutliner,
    /// An edit or a command of the cut-scene shown over the level
    /// (stage 62).
    Cutscene(cine::CineAction),
    /// Several edits recorded as one operation of the workspace (one undo):
    /// duplicating or removing the objects selected in the Outliner.
    Many {
        actions: Vec<LevelAction>,
        label: String,
    },
}

/// The "Add object" section of Details: a model and how to place it.
#[derive(Debug, Clone, Default)]
pub struct AddObject {
    /// Metamodel id, hexadecimal as typed.
    pub model: String,
    /// Clone the model into the zone.
    pub clone: bool,
    /// Give the clone its own collision built from its meshes.
    pub collision: bool,
    /// A wizard chose the model to add: selecting an object keeps it.
    pub fixed: bool,
    /// Open the section once (the toolbar's "Add an object with a model…").
    pub open: bool,
}

/// A node of the Outliner's tree: a zone (stream), a class in a zone, or
/// one object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Node {
    Zone(usize),
    Class(usize, u32),
    Object(ObjRef),
}

/// Everything that changes the image.
#[derive(Clone, PartialEq)]
struct RenderKey {
    camera: FlyCamera,
    size: [u32; 2],
    generation: u64,
    options: crate::render::SceneOptions,
    gizmo: (gizmo::Mode, gizmo::Space, Option<gizmo::Handle>),
    grid: bool,
    collision: Option<(ObjRef, RowMatrix)>,
    nav: u64,
    walk: u64,
    /// The cut-scene over the level ([`cine::Layer::revision`]).
    cine: u64,
}

/// A drag of the gizmo: every selected object (that is not locked) moves,
/// turns or scales about the centre of the selection.
struct Drag {
    /// The objects and their transforms when the drag started (none for a
    /// node of a navigation graph).
    objects: Vec<(ObjRef, RowMatrix)>,
    /// Their transforms now.
    current: Vec<RowMatrix>,
    /// A node of a navigation graph instead of objects.
    node: Option<nav::NodeRef>,
    mode: gizmo::Mode,
    handle: gizmo::Handle,
    /// The gizmo's axes when the drag started.
    axes: [math::Vec3; 3],
    /// The gizmo's size when the drag started (scale works in its units).
    size: f32,
    /// The centre of the selection when the drag started.
    pivot: math::Vec3,
    /// Where the drag started ([`gizmo::grab`]).
    start: [f32; 2],
    change: Option<gizmo::Change>,
    /// Alt: copies move, the objects stay where they are.
    copy: bool,
    /// A camera or actor of the cut-scene instead: where it started and
    /// where it is now.
    cine: Option<(cine::Item, RowMatrix, RowMatrix)>,
}

impl Drag {
    /// Where the gizmo is now.
    fn origin(&self) -> math::Vec3 {
        match self.change {
            Some(gizmo::Change::Move(by)) => math::add(self.pivot, by),
            _ => self.pivot,
        }
    }

    /// The transform of a dragged object now (not for copies: the object
    /// itself stays).
    fn transform_of(&self, r: ObjRef) -> Option<RowMatrix> {
        if self.copy {
            return None;
        }
        let i = self.objects.iter().position(|(o, _)| *o == r)?;
        self.current.get(i).copied()
    }
}

#[derive(Default)]
struct Preset {
    select: Option<ObjRef>,
    /// Select the object with a model nearest to `look` (for screenshots).
    near: bool,
    /// Select this many objects with a model nearest to `look`.
    nearby: usize,
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
    /// Hour of the light, sky and fog when shading as in the game (stage 66).
    pub hours: f32,
    generation: u64,
    refill: bool,
    stats: RenderStats,
    last_pick: Option<Pos2>,
    group_generation: Vec<u64>,
    gizmo: gizmo::Mode,
    space: gizmo::Space,
    snap: gizmo::Snap,
    hover_handle: Option<gizmo::Handle>,
    drag: Option<Drag>,
    /// A frame drawn with the left button to select what is inside.
    marquee: Option<interact::Marquee>,
    /// What the context menu of the view is for.
    context: Option<interact::Context>,
    /// End was pressed: the selection goes to the ground once the
    /// collision is built.
    drop_pending: bool,
    /// The camera as before stage 54 (the setting "Camera: as before").
    classic: bool,
    /// The bar over the view: projection, lighting, what shows, steps.
    bar: viewbar::Bar,
    /// Where the view was drawn last.
    view_rect: Rect,
    /// A short message in the view (Ctrl+A over the limit, End).
    note: Option<(String, Instant)>,
    /// Height of the grid without a selection: low among the objects.
    ground_y: f32,
    /// The "Add object" section of Details.
    pub add: AddObject,
    /// Changes whenever the selection does (Details follows the selection
    /// made last, in a level or in a list of resources).
    selection_serial: u64,
    /// Objects a click in the view does not select (the Outliner's lock).
    locked: ObjectSet,
    /// Changes whenever the objects of the level do (load, edits).
    revision: u64,
    /// Objects the workspace added or changed.
    marks: HashMap<ObjRef, Mark>,
    /// The game's version of edited SimGroups, by group (`None`: unreadable).
    originals: HashMap<usize, Option<Arc<SimGroup>>>,
    /// A transform shown while Details drags a value, not recorded yet.
    preview: Vec<(ObjRef, RowMatrix)>,
    /// The small picture of a model in Details.
    model_preview: preview::ModelPreview,
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
    /// Markers chosen by the user ("Markers" on the bar, Show); `None`
    /// follows the shading: hidden as in the game, shown otherwise.
    markers_choice: Option<bool>,
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
    /// Edits for the next frames (one is returned per frame).
    queue: VecDeque<LevelAction>,
    /// Walking the level (stage 37).
    walk: walk::Walk,
    /// The camera follows the game's (stage 35).
    live_follow: bool,
    /// Editing the running game (stage 47), set by the app.
    pub live_edit: LiveEdit,
    /// The gizmo moves the object in the running game too.
    live_gizmo: bool,
    /// Moves of the gizmo for the game, returned by the next `ui`s.
    live_pending: Vec<LevelAction>,
    /// The object whose state in the game was asked for last.
    live_asked: Option<[u8; 16]>,
    /// The cut-scene shown over the level (stage 62), set by the app.
    pub cine: Option<cine::Layer>,
    /// Instances of the level in the scene; the cut-scene's follow them.
    base_instances: usize,
    /// What the cut-scene added to the scene, and for which revision.
    cine_cache: Option<(u64, CineDrawn)>,
}

/// What the cut-scene adds to the view's scene.
#[derive(Default)]
struct CineDrawn {
    instances: Vec<crate::render::Instance>,
    lines: Vec<crate::render::LineVertex>,
    overlay: Vec<crate::render::LineVertex>,
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
            hours: DEFAULT_HOURS,
            generation: 0,
            refill: true,
            stats: RenderStats::default(),
            last_pick: None,
            group_generation: Vec::new(),
            gizmo: gizmo::Mode::Move,
            space: gizmo::Space::World,
            snap: gizmo::Snap::default(),
            hover_handle: None,
            drag: None,
            marquee: None,
            context: None,
            drop_pending: false,
            classic: false,
            bar: viewbar::Bar::default(),
            view_rect: Rect::NOTHING,
            note: None,
            ground_y: 0.0,
            add: AddObject::default(),
            selection_serial: 0,
            locked: ObjectSet::default(),
            revision: 0,
            marks: HashMap::new(),
            originals: HashMap::new(),
            preview: Vec::new(),
            model_preview: preview::ModelPreview::default(),
            preset: None,
            autoload: None,
            pending_autoload: false,
            goto: None,
            keep_view: None,
            camera_moved: false,
            spheres: HashMap::new(),
            hide_tiny: true,
            markers_choice: None,
            show_collision: false,
            play_here: false,
            collisions: HashMap::new(),
            nav: nav::NavLayer::default(),
            nav_action: None,
            queue: VecDeque::new(),
            walk: walk::Walk::default(),
            live_follow: false,
            live_edit: LiveEdit::default(),
            live_gizmo: false,
            live_pending: Vec::new(),
            live_asked: None,
            cine: None,
            base_instances: 0,
            cine_cache: None,
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
            || self.cine.as_ref().is_some_and(|c| !c.status.is_empty())
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
                "near" => preset.near = true,
                "nearby" => preset.nearby = value.parse().unwrap_or(0),
                "grid" => self.bar.grid = true,
                "stats" => self.bar.stats = true,
                "top" => self.camera.projection = camera::Projection::Top,

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
                "fov" => {
                    if let Some(deg) = floats.first() {
                        self.camera.fov_y = deg.to_radians();
                    }
                }
                "time" => {
                    if let Some(h) = parse_hours(value) {
                        self.hours = h;
                    }
                }
                "shading" => crate::render::set_game_look(value != "simple"),
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
        self.clear_selection();
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
        self.pending.clear();
        self.uploaded.clear();
        self.spheres.clear();
        self.failed.clear();
        self.clear_selection();
        self.filter.hidden.clear();
        self.locked.clear();
        self.marks.clear();
        self.originals.clear();
        self.filter.hovered = None;
        self.drag = None;
        self.preview.clear();
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
                        // Only raised: a cut-scene's characters (stage 62) and
                        // other views share the renderer.
                        let budget = (budget + 256).max(renderer.mesh_budget());
                        renderer.set_mesh_budget(budget);
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
        let _profile = crate::profile::scope("level: upload");
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
        // Without a selection the grid lies low among the objects (a quarter
        // of them are lower).
        let mut heights: Vec<f32> = items.iter().map(|i| i.world[13]).collect();
        if !heights.is_empty() {
            let low = heights.len() / 4;
            heights.select_nth_unstable_by(low, f32::total_cmp);
            self.ground_y = heights[low];
        }
        self.items = items;
        self.by_object = by_object;
        self.refill = true;
        self.revision += 1;
        self.update_marks();
    }

    /// The "new" and "edited" marks of the SimGroups the workspace changed.
    fn update_marks(&mut self) {
        let Some(level) = &self.level else { return };
        self.marks.clear();
        for (g, group) in level.groups.iter().enumerate() {
            if !level.source.overrides.groups.contains_key(&group.location) {
                continue;
            }
            let original = self
                .originals
                .entry(g)
                .or_insert_with(|| {
                    let stream = level.source.stream(group.location.stream).ok()?;
                    let data = stream.resource_data(group.location.resource).ok()?;
                    SimGroup::parse(&data).ok().map(Arc::new)
                })
                .clone();
            let Some(original) = original else { continue };
            for (object, mark) in marks::group_marks(&original, &group.group)
                .into_iter()
                .enumerate()
            {
                if let Some(mark) = mark {
                    self.marks.insert(ObjRef { group: g, object }, mark);
                }
            }
        }
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
        // The objects with a model nearest to `look`: one (`near`) or
        // `nearby=N` of them.
        let count =
            std::mem::take(&mut preset.nearby).max(usize::from(std::mem::take(&mut preset.near)));
        let mut nearest: Vec<(f32, ObjRef)> = Vec::new();
        if let (Some(at), Some(level), true) = (preset.look, self.level.as_ref(), count > 0) {
            for (g, group) in level.groups.iter().enumerate() {
                for o in &group.objects {
                    let (Some(m), Some(_)) = (o.transform, o.model) else {
                        continue;
                    };
                    let d = math::length(math::sub([m[3][0], m[3][1], m[3][2]], at));
                    nearest.push((
                        d,
                        ObjRef {
                            group: g,
                            object: o.index,
                        },
                    ));
                }
            }
            nearest.sort_by(|a, b| a.0.total_cmp(&b.0));
            nearest.truncate(count);
        }
        let near = nearest.first().map(|(_, r)| *r);
        let select = preset.select.take().or(near);
        preset.selected_once |= select.is_some();
        let has_eye = preset.eye.is_some();
        if let Some(r) = select {
            self.set_selection(Some(r), nearest.iter().map(|(_, r)| *r).collect());
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
        self.hover_handle = None;
        // Walking looks through the eyes: a perspective view.
        self.camera
            .set_projection(camera::Projection::Perspective, 20.0);
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
                matrix: gizmo::apply(
                    gizmo::Mode::Rotate,
                    &transform?,
                    &gizmo::WORLD,
                    1,
                    floats[0].to_radians(),
                ),
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
                let before: Vec<Guid> = level.groups[g].objects.iter().map(|o| o.guid).collect();
                if let Err(error) = level.set_group(g, &data) {
                    self.status = trf("level.cannot_read_group", &[("error", &error)]);
                    continue;
                }
                // Objects keep their selection, eye and lock by GUID (a
                // removed object moves the ones after it up).
                let after: HashMap<Guid, usize> = level.groups[g]
                    .objects
                    .iter()
                    .map(|o| (o.guid, o.index))
                    .collect();
                let moved = |r: ObjRef| -> Option<ObjRef> {
                    if r.group != g {
                        return Some(r);
                    }
                    let guid = before.get(r.object)?;
                    Some(ObjRef {
                        group: g,
                        object: *after.get(guid)?,
                    })
                };
                let remap = |set: &mut HashSet<ObjRef>| {
                    *set = set.iter().filter_map(|&r| moved(r)).collect();
                };
                remap(&mut self.filter.hidden.objects);
                remap(&mut self.locked.objects);
                remap(&mut self.filter.selection);
                self.filter.selected = self.filter.selected.and_then(moved);
                self.group_generation[g] += 1;
                changed = true;
            }
        }
        if !changed && select.is_none() {
            return;
        }
        // Meshes of new models.
        let new: Vec<loader::Part> = loader::parts(level)
            .into_iter()
            .filter(|p| !self.uploaded.contains(&p.key()))
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
        } else {
            // Keep the selected objects that are still there.
            let exists = |r: &ObjRef| {
                self.level.as_ref().is_some_and(|l| {
                    l.groups
                        .get(r.group)
                        .is_some_and(|g| r.object < g.objects.len())
                })
            };
            let set: BTreeSet<ObjRef> = self
                .filter
                .selection
                .iter()
                .copied()
                .filter(exists)
                .collect();
            let primary = self.filter.selected.filter(exists);
            self.set_selection(primary, set);
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
        // The same zones load again: the objects keep their numbers.
        let hidden = self.filter.hidden.clone();
        let locked = self.locked.clone();
        if let Some(streams) = loaded {
            self.chosen = streams.into_iter().collect();
            let ctx = self.ctx.clone();
            self.start_load(&ctx, ws);
            self.keep_view = Some((camera, selected));
            self.filter.hidden = hidden;
            self.locked = locked;
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

    /// Loads these zones (streams) instead of the chosen ones: the zones of
    /// a cut-scene (stage 62).
    pub fn load_streams(&mut self, streams: impl IntoIterator<Item = usize>) {
        self.chosen = streams.into_iter().collect();
        self.pending_autoload = true;
        self.ctx.request_repaint();
    }

    /// Fills in the "Add object" panel for a wizard: the metamodel `model`,
    /// cloned into the zone (with its own collision) or used as it is.
    pub fn prefill_add(&mut self, model: u32, clone: bool, collision: bool) {
        self.add.model = format!("{model:08x}");
        self.add.clone = clone;
        self.add.collision = clone && collision;
        self.add.fixed = true;
    }

    /// Selects the objects at these SimGroups and numbers (the copies an
    /// edit made); the last one is the primary.
    pub fn select_locations(&mut self, objects: &[(Location, usize)]) {
        let set: BTreeSet<ObjRef> = objects
            .iter()
            .filter_map(|&(group, object)| self.find(group, object))
            .collect();
        let primary = objects
            .last()
            .and_then(|&(group, object)| self.find(group, object));
        self.set_selection(primary, set);
    }

    /// GUID of the selected object.
    fn selected_guid(&self) -> Option<[u8; 16]> {
        let r = self.filter.selected?;
        let level = self.level.as_ref()?;
        Some(level.groups.get(r.group)?.objects.get(r.object)?.guid.0)
    }

    /// Selects the object with this GUID when the loaded zones have it (the
    /// selection of the running game, stage 47).
    pub fn select_guid(&mut self, guid: &[u8; 16]) -> bool {
        let Some(level) = &self.level else {
            return false;
        };
        let found = level.groups.iter().enumerate().find_map(|(g, group)| {
            group
                .objects
                .iter()
                .position(|o| &o.guid.0 == guid)
                .map(|object| ObjRef { group: g, object })
        });
        match found {
            Some(r) => {
                self.select(Some(r));
                self.focus();
                true
            }
            None => false,
        }
    }

    /// The section "In the running game" of the Details of object `r`
    /// (stage 47): its state in the game and the edits there, while the
    /// live link edits the world.
    pub fn live_section(&mut self, ui: &mut egui::Ui, r: ObjRef) -> Option<LevelAction> {
        let o = self.object(r)?.clone();
        live::object_section(
            ui,
            o.guid.0,
            o.transform.as_ref().map(live::flat),
            &mut self.live_gizmo,
            &self.live_edit,
        )
        .map(LevelAction::LiveObject)
    }

    /// Clears the selection (after the selected objects were removed).
    pub fn clear_selection(&mut self) {
        self.set_selection(None, BTreeSet::new());
    }

    // ---------------------------------------------------------- selection

    /// Selects one object (or nothing).
    fn select(&mut self, r: Option<ObjRef>) {
        self.set_selection(r, r.into_iter().collect());
    }

    /// Selects `set`, with `primary` (in the set, or added to it) the one the
    /// gizmo and Details follow.
    pub fn set_selection(&mut self, primary: Option<ObjRef>, mut set: BTreeSet<ObjRef>) {
        let primary = primary.or_else(|| set.iter().next_back().copied());
        set.extend(primary);
        if primary.is_some() {
            self.nav.select(None);
            if let Some(layer) = &mut self.cine
                && layer.selected.take().is_some()
            {
                self.selection_serial += 1;
            }
        }
        let set: HashSet<ObjRef> = set.into_iter().collect();
        if self.filter.selected == primary && self.filter.selection == set {
            return;
        }
        if self.filter.selected != primary
            && let Some(r) = primary
            && let Some(object) = self.object(r)
            && !self.add.fixed
        {
            self.add.model = object
                .model
                .map_or_else(String::new, |m| format!("{m:08x}"));
        }
        if self.preview.iter().any(|(r, _)| !set.contains(r)) {
            self.set_previews(Vec::new());
        }
        self.filter.selected = primary;
        self.filter.selection = set;
        self.selection_serial += 1;
        self.refill = true;
    }

    /// The primary selected object.
    pub fn primary(&self) -> Option<ObjRef> {
        self.filter.selected
    }

    /// Every selected object, in order.
    pub fn selection(&self) -> BTreeSet<ObjRef> {
        self.filter.selection.iter().copied().collect()
    }

    pub fn is_selected(&self, r: ObjRef) -> bool {
        self.filter.selection.contains(&r)
    }

    /// Changes whenever the selection does.
    pub fn selection_serial(&self) -> u64 {
        self.selection_serial
    }

    /// Changes whenever the objects of the level do.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Changes whenever SimGroup `group` does.
    pub fn group_revision(&self, group: usize) -> u64 {
        self.group_generation.get(group).copied().unwrap_or(0)
    }

    /// The loaded level.
    pub fn level(&self) -> Option<&Level> {
        self.level.as_deref()
    }

    /// The light, sky and fog of the level's city at [`LevelView::hours`],
    /// when shading as in the game.
    fn game_look(&self) -> Option<crate::render::GameLook> {
        let input = self.input.as_ref()?;
        let level = self.level.as_ref()?;
        let stream = level
            .streams
            .iter()
            .map(|&s| input.game.streams()[s].stream_name())
            .find(|n| doctor_toolkit::time_of_day::city_of(n).is_some())?;
        crate::game_light::look(&input.game, &stream, self.hours)
    }

    /// The game and the index the view was opened with.
    pub fn game(&self) -> Option<(&Arc<GameInstance>, &Arc<ResourceIndex>)> {
        self.input.as_ref().map(|i| (&i.game, &i.index))
    }

    /// The parents of every stream (for what a zone can reach).
    pub fn stream_parents(&self) -> Option<&std::collections::HashMap<String, Vec<String>>> {
        self.input.as_ref().map(|i| &*i.parents)
    }

    /// One object of the level.
    pub fn object(&self, r: ObjRef) -> Option<&doctor_toolkit::level::LevelObject> {
        self.level
            .as_ref()?
            .groups
            .get(r.group)?
            .objects
            .get(r.object)
    }

    /// The SimGroup and number of an object (as edits name it).
    pub fn location_of(&self, r: ObjRef) -> Option<(Location, usize)> {
        let group = self.level.as_ref()?.groups.get(r.group)?;
        (r.object < group.objects.len()).then_some((group.location, r.object))
    }

    /// The object at a SimGroup and number.
    pub fn find(&self, group: Location, object: usize) -> Option<ObjRef> {
        let level = self.level.as_ref()?;
        let g = level.groups.iter().position(|x| x.location == group)?;
        (object < level.groups[g].objects.len()).then_some(ObjRef { group: g, object })
    }

    /// Whether the workspace added or changed an object.
    pub fn mark(&self, r: ObjRef) -> Option<Mark> {
        self.marks.get(&r).copied()
    }

    /// The SimGroup editor of a group (its attributes, with the game's
    /// version to compare when the workspace changed it).
    pub fn logic_view(&self, group: usize) -> Option<LogicView> {
        let level = self.level.as_ref()?;
        let g = level.groups.get(group)?;
        let data = g.group.encode();
        let original = self
            .originals
            .get(&group)
            .cloned()
            .flatten()
            .map(|o| o.encode())
            .filter(|o| *o != data);
        Some(LogicView::from_data(LOGIC_SIMGROUP, Ok(data), original))
    }

    // ----------------------------------------------------- eye and lock

    /// SimGroups (by index) of a zone.
    fn zone_groups(&self, stream: usize) -> Vec<usize> {
        self.level.as_ref().map_or_else(Vec::new, |l| {
            (0..l.groups.len())
                .filter(|&g| l.groups[g].location.stream == stream)
                .collect()
        })
    }

    fn node_in(&self, set: &ObjectSet, node: Node) -> bool {
        match node {
            Node::Zone(stream) => {
                let groups = self.zone_groups(stream);
                !groups.is_empty() && groups.iter().all(|g| set.groups.contains(g))
            }
            Node::Class(stream, class) => {
                let groups = self.zone_groups(stream);
                !groups.is_empty() && groups.iter().all(|&g| set.classes.contains(&(g, class)))
            }
            Node::Object(r) => set.objects.contains(&r),
        }
    }

    fn set_node(set: &mut ObjectSet, groups: Vec<usize>, node: Node, on: bool) {
        match node {
            Node::Zone(_) => {
                for g in groups {
                    if on {
                        set.groups.insert(g);
                    } else {
                        set.groups.remove(&g);
                    }
                }
            }
            Node::Class(_, class) => {
                for g in groups {
                    if on {
                        set.classes.insert((g, class));
                    } else {
                        set.classes.remove(&(g, class));
                    }
                }
            }
            Node::Object(r) => {
                if on {
                    set.objects.insert(r);
                } else {
                    set.objects.remove(&r);
                }
            }
        }
    }

    fn groups_of(&self, node: Node) -> Vec<usize> {
        match node {
            Node::Zone(stream) | Node::Class(stream, _) => self.zone_groups(stream),
            Node::Object(r) => vec![r.group],
        }
    }

    /// Whether a node itself is hidden in the view (its eye is closed).
    pub fn is_hidden(&self, node: Node) -> bool {
        self.node_in(&self.filter.hidden, node)
    }

    /// Whether an object is hidden by its own eye, its class's or its zone's.
    pub fn is_object_hidden(&self, r: ObjRef) -> bool {
        self.object(r)
            .is_some_and(|o| self.filter.hidden.contains(r, o.class))
    }

    pub fn set_hidden(&mut self, node: Node, hidden: bool) {
        let groups = self.groups_of(node);
        Self::set_node(&mut self.filter.hidden, groups, node, hidden);
        self.refill = true;
        self.generation += 1;
    }

    /// Shows every object again.
    pub fn show_all(&mut self) {
        self.filter.hidden.clear();
        self.refill = true;
    }

    /// Whether a node itself is locked (clicks in the view pass it by).
    pub fn is_locked(&self, node: Node) -> bool {
        self.node_in(&self.locked, node)
    }

    /// Whether an object is locked by its own lock, its class's or its zone's.
    pub fn is_object_locked(&self, r: ObjRef) -> bool {
        self.object(r)
            .is_some_and(|o| self.locked.contains(r, o.class))
    }

    pub fn set_locked(&mut self, node: Node, locked: bool) {
        let groups = self.groups_of(node);
        Self::set_node(&mut self.locked, groups, node, locked);
        self.generation += 1;
    }

    // ------------------------------------------------- details' transform

    /// Shows objects placed anew until they are recorded (the Transform
    /// of Details; empty: back to their own transforms).
    pub fn set_previews(&mut self, preview: Vec<(ObjRef, RowMatrix)>) {
        let old = std::mem::replace(&mut self.preview, preview);
        let changed: Vec<ObjRef> = old
            .iter()
            .chain(self.preview.iter())
            .map(|(r, _)| *r)
            .collect();
        for r in changed {
            if let (Some(m), Some(indices)) = (self.object_transform(r), self.by_object.get(&r)) {
                scene::move_items(&mut self.items, indices, &m);
            }
        }
        self.refill = true;
    }

    /// Whether the view shows the Havok collision of the selected object.
    pub fn shows_collision(&self) -> bool {
        self.show_collision
    }

    pub fn set_show_collision(&mut self, on: bool) {
        self.show_collision = on;
    }

    /// Ten metres in front of the camera: where "Add in front" places.
    pub fn in_front(&self) -> [f32; 3] {
        math::add(self.camera.eye, math::scale(self.camera.forward(), 10.0))
    }

    /// Flies the camera to object `r` (and selects it when it is not).
    pub fn focus_on(&mut self, r: ObjRef) {
        if !self.is_selected(r) {
            self.select(Some(r));
        }
        let primary = self.filter.selected;
        self.filter.selected = Some(r);
        self.focus();
        self.filter.selected = primary;
    }

    /// Duplicates (or removes) every selected object: one edit, or one
    /// operation of several (removed from the last, so the numbers of the
    /// ones still to remove stay right).
    pub fn edit_selection(&self, remove: bool) -> Option<LevelAction> {
        let mut objects: Vec<ObjRef> = self.selection().into_iter().collect();
        if remove {
            objects.reverse();
        }
        let mut actions: Vec<LevelAction> = objects
            .into_iter()
            .filter_map(|r| self.location_of(r))
            .map(|(group, object)| {
                if remove {
                    LevelAction::Remove { group, object }
                } else {
                    LevelAction::Duplicate { group, object }
                }
            })
            .collect();
        match actions.len() {
            0 => None,
            1 => actions.pop(),
            n => Some(LevelAction::Many {
                actions,
                label: if remove {
                    trn("outliner.removed_label", n as u64, &[])
                } else {
                    trn("outliner.duplicated_label", n as u64, &[])
                },
            }),
        }
    }

    /// Flies the camera to the selected objects, or to everything.
    pub fn focus_selection(&mut self) {
        if self.filter.selected.is_some() {
            self.focus();
        } else if let Some(m) = self.cine.as_ref().and_then(|l| l.matrix(l.selected?)) {
            self.camera.look_at([m[12], m[13] + 0.9, m[14]], 4.0);
            self.camera_moved = true;
            self.refill = true;
        } else {
            self.frame_all();
        }
    }

    /// The world transform of an object as the view shows it (while a drag
    /// or Details changes it, the changed one).
    pub fn object_transform(&self, r: ObjRef) -> Option<RowMatrix> {
        if let Some(m) = self.drag.as_ref().and_then(|d| d.transform_of(r)) {
            return Some(m);
        }
        if let Some((_, m)) = self.preview.iter().find(|(p, _)| *p == r) {
            return Some(*m);
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

    /// Flies the camera to the primary selected object, or to every selected
    /// object when the primary one is among them.
    fn focus(&mut self) {
        let Some(primary) = self.filter.selected else {
            return;
        };
        let mut objects: Vec<ObjRef> = vec![primary];
        if self.filter.selection.contains(&primary) {
            objects.extend(self.filter.selection.iter().filter(|&&r| r != primary));
        }
        let mut bounds = math::Aabb::EMPTY;
        let mut origins = math::Aabb::EMPTY;
        let renderer = self.gpu.as_ref().map(|gpu| gpu.lock());
        for r in objects {
            let Some(m) = self.object_transform(r) else {
                continue;
            };
            origins.add_point([m[3][0], m[3][1], m[3][2]]);
            if let (Some(indices), Some(renderer)) = (self.by_object.get(&r), &renderer) {
                for &i in indices {
                    let item = &self.items[i];
                    if let Some(mesh) = renderer.mesh_data(item.mesh) {
                        bounds.union(&mesh.bounds().transformed(&item.world));
                    }
                }
            }
        }
        drop(renderer);
        if origins.is_empty() {
            return;
        }
        let (center, radius) = if bounds.is_empty() {
            (origins.center(), (origins.radius() + 2.0).clamp(2.0, 400.0))
        } else {
            (bounds.center(), bounds.radius().clamp(1.0, 400.0))
        };
        self.camera.look_at(center, radius * 2.5 + 3.0);
        self.refill = true;
    }

    // ---------------------------------------------------------------- ui

    /// Follow the camera of the running game (stage 35).
    pub fn follow_game(&mut self, on: bool) {
        self.live_follow = on;
    }

    // ------------------------------------------------- the shell's toolbar

    pub fn gizmo_mode(&self) -> GizmoMode {
        self.gizmo
    }

    pub fn set_gizmo_mode(&mut self, mode: GizmoMode) {
        self.gizmo = mode;
        self.hover_handle = None;
    }

    pub fn gizmo_space(&self) -> GizmoSpace {
        self.space
    }

    pub fn set_gizmo_space(&mut self, space: GizmoSpace) {
        self.space = space;
    }

    pub fn snap(&self) -> Snap {
        self.snap
    }

    pub fn set_snap(&mut self, snap: Snap) {
        self.snap = snap;
    }

    /// Whether zones are loaded.
    pub fn loaded(&self) -> bool {
        self.level.is_some()
    }

    /// Whether an object is selected.
    pub fn has_selection(&self) -> bool {
        self.filter.selected.is_some()
    }

    /// Duplicates the selected objects (the edit comes from the next `ui`).
    pub fn request_duplicate(&mut self) {
        if let Some(a) = self.edit_selection(false) {
            self.queue.push_back(a);
        }
    }

    /// Opens the "Add object" section of Details.
    pub fn request_add(&mut self) {
        self.add.open = true;
    }

    /// Starts the game at the camera, as "Play from here".
    pub fn request_play_here(&mut self) {
        self.play_here = true;
    }

    /// Loading, if a load is going on: meshes converted and in all.
    pub fn progress(&self) -> Option<(usize, usize)> {
        let job = self.job.as_ref()?;
        Some((
            job.done.load(std::sync::atomic::Ordering::Relaxed),
            job.total.load(std::sync::atomic::Ordering::Relaxed),
        ))
    }

    /// The view: the scene of the loaded zones. Returns an edit to record.
    pub fn ui(&mut self, ui: &mut egui::Ui, ws: Option<&Workspace>) -> Option<LevelAction> {
        let ctx = ui.ctx().clone();
        if std::mem::take(&mut self.pending_autoload) {
            self.start_load(&ctx, ws);
        }
        self.poll(&ctx);
        self.upload();
        if self.level.is_some() && self.job.is_none() && self.pending.is_empty() {
            crate::profile::mark("level");
        }
        if let (Some(level), Some(input)) = (&self.level, &self.input) {
            let mut streams: Vec<usize> = level.groups.iter().map(|g| g.location.stream).collect();
            streams.sort_unstable();
            streams.dedup();
            self.nav.ensure(&input.game, ws, &streams);
        }
        if let Some(a) = self.preset_action() {
            self.queue.push_back(a);
        }
        if let Some(a) = self.nav_action.take() {
            self.queue.push_back(a);
        }
        if !self.live_pending.is_empty() {
            self.queue.extend(self.live_pending.drain(..));
        } else if self.queue.is_empty()
            && self.live_edit.reads
            && let Some(g) = self.selected_guid()
            && self.live_asked != Some(g)
        {
            // The selection in the running game: its state there.
            self.live_asked = Some(g);
            self.queue
                .push_back(LevelAction::LiveObject(ObjectAction::Get(g)));
        }
        if std::mem::take(&mut self.play_here)
            && let Some(level) = &self.level
        {
            self.queue.push_back(LevelAction::PlayHere {
                eye: self.camera.eye,
                streams: level.streams.clone(),
            });
        }
        if let Some(a) = self.drop_to_ground() {
            self.queue.push_back(a);
        }
        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(a) = self.scene_ui(ui) {
                self.queue.push_back(a);
            }
        });
        let action = self.queue.pop_front();
        if !self.queue.is_empty() {
            ctx.request_repaint();
        }
        action
    }

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
            let mut zones = false;
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() / 2.0 - 30.0).max(8.0));
                if self.job.is_some() {
                    ui.label(RichText::new(tr("level.loading")).weak());
                } else {
                    ui.label(RichText::new(tr("level.empty")).weak());
                    zones = ui
                        .button(tr("level.choose_zones_button"))
                        .on_hover_text(tr("level.choose_zones_hint"))
                        .clicked();
                }
            });
            return zones.then_some(LevelAction::ShowZones);
        }
        self.spin(ui);
        // Game → editor: the camera follows the game's.
        match self.live.and_then(|m| m.game_camera) {
            Some(c) if self.live_follow => {
                self.camera
                    .set_projection(camera::Projection::Perspective, 20.0);
                if live::fly_from_game(&c, &mut self.camera) {
                    self.refill = true;
                    self.camera_moved = true;
                }
            }
            None if self.live.is_none() => self.live_follow = false,
            _ => {}
        }
        self.cine_camera();
        let size = ui.available_size().max(Vec2::splat(64.0));
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        self.view_rect = rect;
        let ppp = ui.ctx().pixels_per_point();
        let pixels = [
            (rect.width() * ppp).round().max(1.0) as u32,
            (rect.height() * ppp).round().max(1.0) as u32,
        ];
        let mut action = self.view_keys(ui, &response);

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
        let ray_at = |p: Pos2| {
            let uv = (p - rect.min) / rect.size();
            view.ray([uv.x, uv.y])
        };
        let target = self.gizmo_target();
        let pointer = response
            .hover_pos()
            .or(ui.input(|i| i.pointer.interact_pos()));
        if self.walk.active || self.marquee.is_some() {
            self.hover_handle = None;
        } else if let (Some(t), None) = (&target, &self.drag) {
            let size = gizmo::size(&view, t.origin);
            self.hover_handle = pointer
                .filter(|_| response.hovered())
                .and_then(|p| gizmo::hit(t.mode, t.origin, &t.axes, size, [p.x, p.y], &to_screen));
        }
        let (alt, ctrl, shift) =
            ui.input(|i| (i.modifiers.alt, i.modifiers.command, i.modifiers.shift));
        if response.drag_started_by(egui::PointerButton::Primary) && !self.walk.active {
            let press = ui.input(|i| i.pointer.press_origin()).or(pointer);
            let grabbed = match (self.hover_handle, &target, pointer) {
                (Some(handle), Some(t), Some(p)) => {
                    gizmo::grab(t.mode, handle, t.origin, &t.axes, &ray_at(p), [p.x, p.y])
                        .map(|start| (handle, start))
                }
                _ => None,
            };
            if let (Some((handle, start)), Some(t)) = (grabbed, &target) {
                self.drag = Some(Drag {
                    current: t.objects.iter().map(|(_, m)| *m).collect(),
                    objects: t.objects.clone(),
                    node: t.node,
                    mode: t.mode,
                    handle,
                    axes: t.axes,
                    size: gizmo::size(&view, t.origin),
                    pivot: t.origin,
                    start,
                    change: None,
                    copy: alt && t.node.is_none() && t.cine.is_none(),
                    cine: t.cine.map(|(item, m)| (item, m, m)),
                });
                self.refill = true;
            } else if !alt
                && (self.gizmo == gizmo::Mode::Select
                    || ctrl
                    || shift
                    || self.camera.projection.is_ortho())
                && let Some(p) = press
            {
                self.marquee = Some(interact::Marquee::new(p, ctrl, shift));
            }
        }
        let released = response.drag_stopped() || !ui.input(|i| i.pointer.primary_down());
        let mut moved = false;
        if let Some(drag) = &mut self.drag {
            // Ctrl snaps, or drags freely while snapping is on.
            let snap = (ui.input(|i| i.modifiers.ctrl) != self.snap.on).then_some(&self.snap);
            if let Some(p) = pointer
                && let Some(now) = gizmo::grab(
                    drag.mode,
                    drag.handle,
                    drag.pivot,
                    &drag.axes,
                    &ray_at(p),
                    [p.x, p.y],
                )
            {
                let change = gizmo::change(
                    drag.mode,
                    drag.handle,
                    &drag.axes,
                    drag.start,
                    now,
                    drag.size,
                    snap,
                );
                if change != drag.change {
                    drag.change = change;
                    drag.current = drag
                        .objects
                        .iter()
                        .map(|(_, m)| change.map_or(*m, |c| gizmo::apply_change(&c, m, drag.pivot)))
                        .collect();
                    if let Some((item, start, current)) = &mut drag.cine {
                        *current =
                            change.map_or(*start, |c| gizmo::apply_change(&c, start, drag.pivot));
                        if let Some(layer) = &mut self.cine {
                            layer.dragged = Some((*item, scene::flatten(current)));
                        }
                    }
                    moved = true;
                }
            }
            if released {
                let drag = self.drag.take().expect("dragging");
                self.refill = true;
                // The gizmo moves the objects in the running game too
                // (stage 47; not the copies of Alt + drag).
                if self.live_gizmo && self.live_edit.writes && !drag.copy {
                    let moves: Vec<LevelAction> = drag
                        .objects
                        .iter()
                        .zip(&drag.current)
                        .filter(|((_, start), m)| start != *m)
                        .filter_map(|((r, _), m)| {
                            let o = self.object(*r)?;
                            Some(LevelAction::LiveObject(ObjectAction::Move(
                                o.guid.0,
                                live::flat(m),
                                true,
                            )))
                        })
                        .collect();
                    self.live_pending.extend(moves);
                }
                if let Some(a) = self.finish_drag(drag) {
                    action = Some(a);
                }
            }
        } else if let Some(m) = &mut self.marquee {
            if let Some(p) = pointer {
                m.now = p;
            }
            if released {
                let m = self.marquee.take().expect("drawing");
                self.select_in(&m, &view, rect);
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
            let pivot = target.as_ref().map_or_else(
                || math::add(self.camera.eye, math::scale(self.camera.forward(), 20.0)),
                |t| t.origin,
            );
            let controls = camera::Controls {
                ue: !self.classic,
                left_moves: self.gizmo != gizmo::Mode::Select,
            };
            if self.camera.handle_input(ui, &response, pivot, controls) {
                self.refill = true;
                self.camera_moved = true;
                // Moving the camera leaves the shot's.
                if let Some(layer) = &mut self.cine {
                    layer.through_shot = false;
                }
            }
        }
        if moved && let Some(drag) = &self.drag {
            if let Some(n) = drag.node {
                self.nav.preview = Some((n, drag.origin()));
                self.nav.generation += 1;
            } else if !drag.copy {
                for ((r, _), m) in drag.objects.iter().zip(&drag.current) {
                    if let Some(indices) = self.by_object.get(r) {
                        scene::move_items(&mut self.items, indices, m);
                    }
                }
            }
            self.refill = true;
        }

        let view = self.camera.view(rect.aspect_ratio());
        // Hover picking on GPUs; software adapters pick on click only.
        let software = self.gpu.as_ref().is_none_or(|gpu| gpu.lock().is_software());
        let mut hovered = self.filter.hovered;
        match response.hover_pos() {
            Some(pos)
                if !software
                    && self.drag.is_none()
                    && self.marquee.is_none()
                    && !response.dragged() =>
            {
                if self.last_pick != Some(pos) {
                    self.last_pick = Some(pos);
                    hovered = self.pick_at(&view, rect, pos, ppp);
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
        let picked_at = |this: &mut Self, pos: Option<Pos2>| match pos {
            Some(pos) if software => this.pick_at(&view, rect, pos, ppp),
            Some(_) => this.filter.hovered,
            None => None,
        };
        let node_click = if response.clicked() && self.hover_handle.is_none() {
            response
                .interact_pointer_pos()
                .and_then(|p| self.nav.pick(&to_screen, [p.x, p.y], nav::PICK_RADIUS))
        } else {
            None
        };
        if let Some(n) = node_click {
            if shift && self.nav.selected.is_some() {
                match self.nav.link_to(n) {
                    Some(Ok(a)) => action = Some(a),
                    Some(Err(e)) => self.status = e,
                    None => {}
                }
            } else {
                self.select(None);
                self.nav.select(Some(n));
            }
        } else if response.clicked()
            && self.hover_handle.is_none()
            && let Some(item) = response
                .interact_pointer_pos()
                .and_then(|p| self.cine.as_ref()?.pick(&view, rect, p))
        {
            // A camera or an actor of the cut-scene.
            self.nav.select(None);
            self.cine_select(Some(item));
        } else if response.clicked() && self.hover_handle.is_none() {
            self.nav.select(None);
            if let Some(layer) = &mut self.cine
                && layer.selected.take().is_some()
            {
                self.selection_serial += 1;
            }
            let picked = picked_at(self, response.interact_pointer_pos());
            if ctrl {
                // Ctrl + click: add to the selection or take away from it.
                if let Some(r) = picked {
                    let mut set = self.selection();
                    let primary = if set.remove(&r) {
                        self.filter.selected.filter(|&p| p != r)
                    } else {
                        set.insert(r);
                        Some(r)
                    };
                    self.set_selection(primary, set);
                }
            } else if shift {
                // Shift + click: add to the selection.
                if let Some(r) = picked {
                    let mut set = self.selection();
                    set.insert(r);
                    self.set_selection(Some(r), set);
                }
            } else {
                self.select(picked);
            }
        }
        if response.double_clicked() {
            self.focus();
        }
        // The right button without a drag: the context menu, for the object
        // under the pointer.
        if response.secondary_clicked() {
            let picked = picked_at(self, response.interact_pointer_pos());
            self.open_context(picked);
        }
        let mut menu = None;
        response.context_menu(|ui| menu = self.context_ui(ui));
        if let Some(a) = menu {
            action = Some(a);
        }

        let markers = self.markers_shown();
        if self.filter.markers != markers {
            self.filter.markers = markers;
            self.refill = true;
        }
        if self.refill {
            let _profile = crate::profile::scope("level: fill");
            self.refill = false;
            let eye = self.camera.lod_eye();
            self.filter.min_angle = if self.hide_tiny {
                TINY_PIXELS * self.camera.fov_y / pixels[1].max(1) as f32
            } else {
                0.0
            };
            let spheres = &self.spheres;
            self.visible = scene::fill(&mut self.scene, &self.items, eye, &self.filter, &|key| {
                spheres.get(&key).copied()
            });
            self.add_ghosts();
            self.base_instances = self.scene.instances.len();
            self.generation += 1;
        }
        // Gizmo lines.
        self.scene.overlay_lines.clear();
        if let Some(t) = self.gizmo_target() {
            let handle = self.drag.as_ref().map(|d| d.handle).or(self.hover_handle);
            self.scene.overlay_lines = gizmo::lines(
                t.mode,
                t.origin,
                &t.axes,
                gizmo::size(&view, t.origin),
                handle,
            );
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
        self.scene.lines = if self.bar.grid {
            self.grid_lines()
        } else {
            Vec::new()
        };
        let cine_revision = self.cine_scene();
        self.scene.options.game = self.game_look();
        match self.gpu.clone() {
            Some(gpu) => {
                let key = RenderKey {
                    camera: self.camera,
                    size: pixels,
                    generation: self.generation,
                    options: self.scene.options,
                    gizmo: (self.gizmo, self.space, self.hover_handle),
                    grid: self.bar.grid,
                    collision,
                    nav: self.nav.generation,
                    walk: self.walk.revision,
                    cine: cine_revision,
                };
                if self.last_render.as_ref() != Some(&key) {
                    let _profile = crate::profile::scope("level: render");
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
            }
            None => {
                // No 3D without a GPU: the view still selects and edits.
                ui.painter()
                    .rect_filled(rect, 0.0, Color32::from_rgb(38, 40, 44));
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    tr("level.needs_gpu"),
                    egui::FontId::proportional(14.0),
                    Color32::GRAY,
                );
            }
        }
        self.hud(ui, rect);
        if let Some(a) = self.drop_ui(ui, &response, rect, &view) {
            action = Some(a);
        }
        self.bar_ui(ui, rect);
        if let Some(a) = self.cine.as_mut().and_then(|l| l.bar_ui(ui, rect)) {
            action = Some(LevelAction::Cutscene(self.fill_cine_action(a)));
        }
        if let Some(marks) = self.live {
            live_marks(ui, rect, &to_screen, &marks);
            if let Some(a) = live::bar(ui, rect, &marks, &mut self.live_follow, &self.camera) {
                action = Some(a);
            }
        }
        action
    }

    /// The object at a point of the view (GPU id pass, or a ray on the CPU).
    fn pick_at(&mut self, view: &View, rect: Rect, pos: Pos2, ppp: f32) -> Option<ObjRef> {
        let gpu = self.gpu.clone()?;
        self.pick(&gpu, view, rect, pos, ppp)
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
        let item = self.items.get(*item)?;
        // Locked objects (the Outliner's lock) are not picked.
        item.object
            .filter(|&r| !self.locked.contains(r, item.class))
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
/// The game's own hour at the start of a level (`TimeOfDayManager`: 16:00).
pub const DEFAULT_HOURS: f32 = 16.0;

/// `HH:MM` or hours as a number.
pub fn parse_hours(value: &str) -> Option<f32> {
    match value.split_once(':') {
        Some((h, m)) => Some(h.trim().parse::<f32>().ok()? + m.trim().parse::<f32>().ok()? / 60.0),
        None => value.trim().parse().ok(),
    }
    .filter(|h: &f32| h.is_finite())
}

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
