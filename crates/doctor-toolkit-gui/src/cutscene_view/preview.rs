//! The 3D preview of a cut-scene at the play head: the actors of the shot as
//! characters posed by their clips (skinned on the CPU), props with the
//! meshes of their shot stream (markers without), the camera path of the shot
//! and the camera itself, and the level around them in the light of the time
//! of day (`world.rs`); seen through the shot camera or from a free orbit
//! camera.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use doctor_toolkit::cutscene::cast::{Animation, Cast};
use doctor_toolkit::cutscene::world::{TimeOfDay, matrix_to_world, time_at, to_world};
use doctor_toolkit::cutscene::{Cutscene, Source, actor_state, camera_state};
use doctor_toolkit::export::gltf::AssetSource;
use doctor_toolkit::game::{GameAssets, GameInstance, ResourceIndex};
use doctor_toolkit::resource::rcb::{Mat4, Skeleton, multiply};
use doctor_toolkit::resource::trinity::Command;
use doctor_toolkit_mods::i18n::tr;
use egui::{Color32, Pos2, Rect, Sense, Vec2};

use super::world::{self, SharedWorld};
use crate::model_view::{LoadOptions, load_with};
use crate::render::{
    Gpu, Instance, LineVertex, MeshData, MeshKey, OrbitCamera, Scene, View, Viewport, math,
};

/// A game directory with its game and resource index, opened once.
type OpenedGame = (PathBuf, Arc<GameInstance>, Arc<ResourceIndex>);

/// The game the previews open: the editor's own once it has indexed it.
static OPENED: Mutex<Option<OpenedGame>> = Mutex::new(None);

/// Hands the editor's game and resource index to the previews, which
/// otherwise open and index the game again (stage 67).
pub fn share_game(game: Arc<GameInstance>, index: Arc<ResourceIndex>) {
    *OPENED.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((game.root().to_path_buf(), game, index));
}

pub(crate) fn open_game(root: &PathBuf) -> Result<(Arc<GameInstance>, Arc<ResourceIndex>), String> {
    let mut cache = OPENED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((r, game, index)) = &*cache
        && r == root
    {
        return Ok((game.clone(), index.clone()));
    }
    let game = Arc::new(GameInstance::open(root).map_err(|e| e.to_string())?);
    let index = Arc::new(game.build_index(|_| {}).0);
    *cache = Some((root.clone(), game.clone(), index.clone()));
    Ok((game, index))
}

/// One part of a character on the GPU.
struct PartModel {
    /// Key of the posed copy (one per actor and part).
    key: MeshKey,
    /// The part in its bind pose.
    bind: Arc<MeshData>,
    skeleton: Option<Skeleton>,
}

/// An actor of a shot ready to draw.
#[derive(Default)]
struct ActorModel {
    parts: Vec<PartModel>,
    problems: Vec<String>,
}

/// Characters, clips and assets, loaded on request; kept across reloads of
/// the view (an edit loads the view again).
pub struct Loaded {
    game: Arc<GameInstance>,
    assets: GameAssets,
    cast: Cast,
    /// By shot resource and actor GUID.
    actors: HashMap<(usize, [u8; 16]), ActorModel>,
    /// By shot resource and clip name.
    clips: HashMap<(usize, String), Result<Arc<Animation>, String>>,
    /// Keeps the posed meshes of this set of characters apart from those
    /// of another view of the same cut-scene (the level view and the
    /// preview pose the same actors at other times).
    pub salt: u64,
}

impl Loaded {
    /// Problems found while loading the actors and clips of shot `s`.
    pub fn problems(&self, cutscene: &Cutscene, s: usize) -> Vec<String> {
        let loaded = self;
        let Some(shot) = cutscene.shots.get(s) else {
            return Vec::new();
        };
        let mut out: Vec<String> = loaded
            .actors
            .iter()
            .filter(|((r, _), _)| *r == shot.resource)
            .flat_map(|(_, a)| a.problems.clone())
            .collect();
        out.extend(
            loaded
                .clips
                .iter()
                .filter(|((r, _), _)| *r == shot.resource)
                .filter_map(|((_, c), a)| a.as_ref().err().map(|e| format!("{c}: {e}"))),
        );
        out.sort();
        out
    }

    /// Actors made so far (for redraws).
    pub fn actor_count(&self) -> usize {
        self.actors.len()
    }

    /// Characters of an opened game: the rigs and parts databases of its
    /// index; actors and clips load per shot when drawn.
    pub fn new(game: Arc<GameInstance>, index: &ResourceIndex) -> Self {
        let mut assets = GameAssets::new(game.clone(), index);
        let rigs = assets.rigs().to_vec();
        let cast = Cast::new(&game, index, rigs);
        Self {
            game,
            assets,
            cast,
            actors: HashMap::new(),
            clips: HashMap::new(),
            salt: 0,
        }
    }
}

/// What survives a reload of the view.
pub type Shared = Arc<Mutex<Option<Result<Loaded, String>>>>;

/// The preview's state.
pub struct Preview {
    root: PathBuf,
    gpu: Option<Gpu>,
    viewport: Viewport,
    pub orbit: OrbitCamera,
    /// See through the camera of the shot.
    pub shot_camera: bool,
    framed: bool,
    pub shared: Shared,
    /// Draw characters (`false`: markers only).
    pub characters: bool,
    /// The level around the cut-scene.
    pub world: SharedWorld,
    /// Time of day chosen by the user; `None`: the cut-scene's.
    pub hour: Option<f32>,
    scene: Scene,
    last: Option<FrameKey>,
    generation: u64,
    /// Seconds of the last frames drawn (for the frame rate while playing).
    pub frame_times: std::collections::VecDeque<f32>,
}

/// What a drawn frame depends on: time, size, camera, loads, world.
type FrameKey = (
    u32,
    [u32; 2],
    bool,
    OrbitCamera,
    u64,
    u64,
    bool,
    Option<u32>,
    bool,
);

/// The time of day at `t`: the cut-scene's command, else the mission's
/// `SetTimeOfDay`, else the game's starting hour.
pub fn time_of_day(cutscene: &Cutscene, t: f32, mission: Option<TimeOfDay>) -> TimeOfDay {
    time_at(cutscene, t)
        .0
        .or(mission)
        .unwrap_or(world::DEFAULT_TIME)
}

fn key_of(parts: impl Hash) -> MeshKey {
    let mut h = DefaultHasher::new();
    "cutscene preview".hash(&mut h);
    parts.hash(&mut h);
    h.finish()
}

/// `p` transformed by a column-major matrix.
fn point(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    math::transform_point(m, p)
}

fn vector(m: &Mat4, v: [f32; 3]) -> [f32; 3] {
    [
        m[0] * v[0] + m[4] * v[1] + m[8] * v[2],
        m[1] * v[0] + m[5] * v[1] + m[9] * v[2],
        m[2] * v[0] + m[6] * v[1] + m[10] * v[2],
    ]
}

/// The bind pose of a part posed by skinning matrices of its skeleton.
fn skin(bind: &MeshData, matrices: &[Mat4]) -> MeshData {
    let mut out = bind.clone();
    for v in &mut out.vertices {
        let mut p = [0.0f32; 3];
        let mut n = [0.0f32; 3];
        let mut total = 0.0f32;
        for k in 0..4 {
            let w = v.weights[k] as f32 / 255.0;
            if w == 0.0 {
                continue;
            }
            let Some(m) = matrices.get(v.joints[k] as usize) else {
                continue;
            };
            let q = point(m, v.position);
            let r = vector(m, v.normal);
            for i in 0..3 {
                p[i] += q[i] * w;
                n[i] += r[i] * w;
            }
            total += w;
        }
        if total > 0.0 {
            v.position = p.map(|x| x / total);
            v.normal = math::normalize_or(n, v.normal);
        }
    }
    // The parts' boxes follow the pose: the level view culls by them.
    for part in &mut out.parts {
        part.bounds = math::Aabb::EMPTY;
    }
    for v in &out.vertices {
        if let Some(part) = out.parts.get_mut(v.part as usize) {
            part.bounds.add_point(v.position);
        }
    }
    out
}

impl Preview {
    pub fn new(root: PathBuf, gpu: Option<Gpu>) -> Self {
        Self {
            root,
            gpu,
            viewport: Viewport::new(),
            orbit: OrbitCamera::default(),
            shot_camera: true,
            framed: false,
            shared: Shared::default(),
            characters: true,
            world: SharedWorld::default(),
            hour: None,
            scene: Scene::default(),
            last: None,
            generation: 0,
            frame_times: Default::default(),
        }
    }

    pub fn has_gpu(&self) -> bool {
        self.gpu.is_some()
    }

    /// Whether characters were asked for.
    pub fn is_loaded(&self) -> bool {
        matches!(
            &*self.shared.lock().unwrap_or_else(|e| e.into_inner()),
            Some(Ok(_))
        )
    }

    pub fn load_error(&self) -> Option<String> {
        match &*self.shared.lock().unwrap_or_else(|e| e.into_inner()) {
            Some(Err(e)) => Some(e.clone()),
            _ => None,
        }
    }

    /// Opens the game index, rigs and parts databases (once per game).
    pub fn load(&mut self) {
        let _profile = crate::profile::scope("cut-scene: open the game and its rigs");
        let result = open_game(&self.root).map(|(game, index)| Loaded::new(game, &index));
        *self.shared.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
        self.generation += 1;
    }

    /// Problems found while loading the actors and clips of a shot.
    pub fn problems(&self, cutscene: &Cutscene, s: usize) -> Vec<String> {
        let guard = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        let Some(Ok(loaded)) = &*guard else {
            return Vec::new();
        };
        loaded.problems(cutscene, s)
    }

    /// Builds the scene for time `t` (seconds from the start of the
    /// cut-scene) seen through `view` (culling and levels of detail of the
    /// world).
    fn build(&mut self, cutscene: &Cutscene, t: f32, view: &View) {
        self.scene.instances.clear();
        self.scene.lines.clear();
        self.scene.overlay_lines.clear();
        self.scene.options.frustum_culling = false;
        let Some(s) = cutscene.shot_at(t) else { return };
        let anchor = cutscene.anchor(s);
        let world_guard = self.world.clone();
        let world_state = world_guard.lock().unwrap_or_else(|e| e.into_inner());
        let world_shown = world_state.enabled;
        if let Some(gpu) = &self.gpu {
            world_state.fill(&mut self.scene, view, gpu);
        }
        let shared = self.shared.clone();
        let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
        let mut loaded = match &mut *guard {
            Some(Ok(l)) => Some(l),
            _ => None,
        };
        if let Some(l) = loaded.as_deref_mut() {
            let _profile = crate::profile::scope("cut-scene: characters and clips of a shot");
            ensure_shot(l, self.gpu.as_ref(), self.characters, cutscene, s);
        }
        let loaded = loaded.as_deref();
        let props = world_shown.then_some(&*world_state);
        let mut bounds = draw_actors(
            &mut self.scene,
            self.gpu.as_ref(),
            loaded,
            props,
            cutscene,
            t,
            &ActorOptions::default(),
        );
        let shot = &cutscene.shots[s];
        let local = t - shot.start;
        // The camera path of the shot and, seen from outside, the camera.
        let path = camera_path(cutscene, s, 10.0);
        for p in &path {
            bounds.add_point(*p);
        }
        push_path(&mut self.scene.lines, &path, PATH_COLOUR);
        if !self.shot_camera
            && let Some(c) = camera_state(shot, local)
        {
            let m = matrix_to_world(&anchor, &c.matrix());
            self.scene.overlay_lines.extend(frustum_lines(
                &m,
                c.vertical_fov(),
                c.aspect,
                0.8,
                CAMERA_COLOUR,
            ));
        }
        if !self.framed && !bounds.is_empty() {
            self.orbit = OrbitCamera::framing(&bounds);
            self.framed = true;
        }
    }

    /// Draws the preview for time `t` in a rectangle of `size`.
    pub fn show(&mut self, ui: &mut egui::Ui, cutscene: &Cutscene, t: f32, size: Vec2) {
        let Some(gpu) = self.gpu.clone() else {
            ui.label(tr("cutscene.no_gpu"));
            return;
        };
        let camera = cutscene.camera_at(t);
        // Keep the shot camera's aspect ratio.
        let aspect = camera.map_or(16.0 / 9.0, |c| c.aspect.clamp(0.5, 3.0));
        let mut size = size;
        if size.x / size.y > aspect {
            size.x = size.y * aspect;
        } else {
            size.y = size.x / aspect;
        }
        let (rect, response) =
            ui.allocate_exact_size(size.max(Vec2::splat(32.0)), Sense::click_and_drag());
        if !self.shot_camera {
            self.orbit.handle_input(ui, &response);
        } else if response.dragged() {
            // Dragging leaves the shot camera for the free one.
            self.shot_camera = false;
        }
        let ppp = ui.ctx().pixels_per_point();
        let pixels = [
            (rect.width() * ppp).round().max(1.0) as u32,
            (rect.height() * ppp).round().max(1.0) as u32,
        ];
        let ctx = ui.ctx().clone();
        self.draw(&gpu, &ctx, cutscene, t, pixels);
        if let Some(id) = self.viewport.texture_id() {
            ui.painter().image(
                id,
                rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        // Subtitles at the play head.
        let ms = (t * 1000.0) as u32;
        let lines: Vec<String> = cutscene
            .audio
            .iter()
            .flat_map(|a| &a.subtitles)
            .filter(|s| ms >= s.start_ms && ms < s.start_ms + s.duration_ms)
            .map(|s| {
                s.text
                    .clone()
                    .unwrap_or_else(|| format!("[{:08x}]", s.string))
            })
            .collect();
        if !lines.is_empty() {
            let text = lines.join("\n");
            let font = egui::FontId::proportional(13.0);
            let galley = ui
                .painter()
                .layout(text, font, Color32::WHITE, rect.width() * 0.85);
            let pos = Pos2::new(
                rect.center().x - galley.size().x / 2.0,
                rect.max.y - galley.size().y - 8.0,
            );
            ui.painter().rect_filled(
                Rect::from_min_size(pos, galley.size()).expand(4.0),
                3.0,
                Color32::from_black_alpha(160),
            );
            ui.painter().galley(pos, galley, Color32::WHITE);
        }
    }

    /// Forces the next frame to be drawn again (after a reload).
    pub fn invalidate(&mut self) {
        self.last = None;
    }

    /// Takes the world's loads forward and uploads its meshes; returns its
    /// state: shown, generation, the mission's hour.
    fn step_world(
        &mut self,
        gpu: &Gpu,
        ctx: &egui::Context,
        cutscene: &Cutscene,
    ) -> (bool, u64, Option<TimeOfDay>) {
        let mut world = self.world.lock().unwrap_or_else(|e| e.into_inner());
        if world.enabled && world.is_idle() {
            world.start(self.root.clone(), cutscene, open_game, ctx);
        }
        world.poll(ctx);
        world.upload(gpu);
        (world.enabled, world.generation, world.mission_time)
    }

    /// Draws the picture of time `t` into the viewport (`pixels` big)
    /// unless it is drawn already.
    fn draw(
        &mut self,
        gpu: &Gpu,
        ctx: &egui::Context,
        cutscene: &Cutscene,
        t: f32,
        pixels: [u32; 2],
    ) {
        let camera = cutscene.camera_at(t);
        let aspect = pixels[0] as f32 / pixels[1].max(1) as f32;
        // The world: start it, take what the workers finished, upload meshes.
        let (world_shown, world_generation, mission) = self.step_world(gpu, ctx, cutscene);
        let hours = self
            .hour
            .unwrap_or_else(|| time_of_day(cutscene, t, mission).hours());
        let key = (
            t.to_bits(),
            pixels,
            self.shot_camera,
            self.orbit,
            self.generation,
            world_generation,
            world_shown,
            world_shown.then_some(hours.to_bits()),
            crate::render::game_look(),
        );
        if self.last == Some(key) {
            return;
        }
        let started = std::time::Instant::now();
        // Far enough for the city around: the world reaches kilometres.
        let far = if world_shown { 6000.0 } else { 2000.0 };
        let near = if world_shown { 0.1 } else { 0.05 };
        let view = match (self.shot_camera, camera) {
            (true, Some(c)) => {
                let m = matrix_to_world(&cutscene.anchor(c.shot), &c.matrix());
                View {
                    view: math::inverse(&m).unwrap_or(math::IDENTITY),
                    projection: math::perspective(
                        c.vertical_fov().to_radians().clamp(0.05, 3.0),
                        aspect,
                        near,
                        far,
                    ),
                    eye: [m[12], m[13], m[14]],
                }
            }
            _ => {
                let mut view = self.orbit.view(aspect);
                if world_shown {
                    view.projection =
                        math::perspective(self.orbit.fov_y, aspect.max(1e-3), near, far);
                }
                view
            }
        };
        self.build(cutscene, t, &view);
        let defaults = crate::render::SceneOptions::default();
        let game_look = if world_shown {
            let game = self
                .shared
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .and_then(|r| r.as_ref().ok())
                .map(|l| l.game.clone());
            game.and_then(|g| crate::game_light::look(&g, &cutscene.stream_name, hours))
        } else {
            crate::game_light::studio()
        };
        self.scene.options.game = game_look;
        if world_shown {
            let (environment, sky) = world::environment(hours);
            self.scene.options.environment = Some(environment);
            self.scene.options.background = game_look.and_then(|g| g.background()).unwrap_or(sky);
        } else {
            self.scene.options.environment = None;
            self.scene.options.background = defaults.background;
        }
        gpu.lock()
            .render(&mut self.viewport, pixels, &self.scene, &view);
        self.last = Some(key);
        self.frame_times.push_back(started.elapsed().as_secs_f32());
        if self.frame_times.len() > 60 {
            self.frame_times.pop_front();
        }
    }

    /// Loads the world without a window (video export): returns once every
    /// zone's meshes are on the GPU, or `false` after `limit`.
    pub fn load_world(
        &mut self,
        ctx: &egui::Context,
        cutscene: &Cutscene,
        limit: std::time::Duration,
    ) -> bool {
        let Some(gpu) = self.gpu.clone() else {
            return false;
        };
        let started = std::time::Instant::now();
        loop {
            self.step_world(&gpu, ctx, cutscene);
            let mut world = self.world.lock().unwrap_or_else(|e| e.into_inner());
            if !world.enabled {
                return true;
            }
            if world.is_ready() && !world.upload(&gpu) {
                return world.error.is_none();
            }
            drop(world);
            if started.elapsed() > limit {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    }

    /// The picture of time `t` through the shot camera, `pixels` big, as
    /// RGBA rows (video export).
    pub fn render_pixels(
        &mut self,
        ctx: &egui::Context,
        cutscene: &Cutscene,
        t: f32,
        pixels: [u32; 2],
    ) -> Option<Vec<u8>> {
        let gpu = self.gpu.clone()?;
        self.shot_camera = true;
        self.draw(&gpu, ctx, cutscene, t, pixels);
        let (w, h, rgba) = gpu.lock().read_pixels(&self.viewport)?;
        (w == pixels[0] && h == pixels[1]).then_some(rgba)
    }
}

/// Colour of the camera path of the shot.
pub(crate) const PATH_COLOUR: [u8; 4] = [80, 200, 255, 255];
/// Colour of the camera seen from outside.
pub(crate) const CAMERA_COLOUR: [u8; 4] = [255, 90, 90, 255];
/// Colour of actors without a character (skeletons, crosses).
const MARKER_COLOUR: [u8; 4] = [255, 200, 60, 255];

/// Characters and clips of the actors of shot `s`.
pub(crate) fn ensure_shot(
    loaded: &mut Loaded,
    gpu: Option<&Gpu>,
    characters: bool,
    cutscene: &Cutscene,
    s: usize,
) {
    let Some(gpu) = gpu.cloned() else { return };
    let shot = &cutscene.shots[s];
    let source = Source::new(&loaded.game, None);
    for command in &shot.block().commands {
        if let Command::ActorAnim { clip, .. } = command {
            let key = (shot.resource, clip.text.clone());
            if !loaded.clips.contains_key(&key) {
                let anim = loaded
                    .cast
                    .animation(&source, shot, &clip.text)
                    .map(Arc::new)
                    .map_err(|e| e.to_string());
                loaded.clips.insert(key, anim);
            }
        }
    }
    if !characters {
        return;
    }
    for (a, actor) in shot.actors.iter().enumerate() {
        if loaded.actors.contains_key(&(shot.resource, actor.guid.0)) {
            continue;
        }
        let mut model = ActorModel::default();
        match loaded.cast.character(&loaded.game, actor) {
            Some(Ok(character)) => {
                for (p, part) in character.parts.iter().enumerate() {
                    let options = LoadOptions {
                        swaps: part.materials.clone(),
                        morphs: character.morphs.clone(),
                    };
                    let mut swaps: Vec<(u32, u32)> =
                        options.swaps.iter().map(|(a, b)| (*a, *b)).collect();
                    swaps.sort_unstable();
                    let bind_key = key_of(("bind", part.location, swaps));
                    // The actor's own rig: its proportions are the ones the
                    // clip moves the bones to.
                    let skeleton = loaded.assets.rig_skeleton(actor.rig, &part.model.bones);
                    let mut renderer = gpu.lock();
                    match load_with(
                        &part.model,
                        bind_key,
                        &mut loaded.assets,
                        &mut renderer,
                        &options,
                    ) {
                        Ok(m) => model.parts.push(PartModel {
                            key: key_of(("posed", loaded.salt, shot.resource, a, p)),
                            bind: m.mesh,
                            skeleton,
                        }),
                        Err(e) => model.problems.push(format!("{}: {e}", part.name)),
                    }
                }
            }
            Some(Err(e)) => model.problems.push(format!("{}: {e}", actor.name)),
            None => {}
        }
        loaded.actors.insert((shot.resource, actor.guid.0), model);
    }
    let count: usize = loaded.actors.values().map(|a| a.parts.len()).sum();
    // The budget is shared with the world and other views: only raise it.
    let mut renderer = gpu.lock();
    let budget = renderer.mesh_budget().max(64.max(count * 3));
    renderer.set_mesh_budget(budget);
}

/// How [`draw_actors`] draws.
#[derive(Default)]
pub(crate) struct ActorOptions {
    /// An actor placed by this matrix (anchor space to world, column-major)
    /// instead of its keys: the gizmo drags it.
    pub moved: Option<([u8; 16], [f32; 16])>,
    /// The selected actor: tinted.
    pub selected: Option<[u8; 16]>,
}

/// The tint of a selected actor.
const SELECTED_TINT: [f32; 4] = [1.6, 1.25, 0.55, 1.0];

/// Draws the actors of the shot playing at `t` into `scene`: characters
/// posed by their clips (when `loaded` has them), props with the meshes of
/// `props`, skeletons or crosses otherwise. Returns the box of their places.
pub(crate) fn draw_actors(
    scene: &mut Scene,
    gpu: Option<&Gpu>,
    loaded: Option<&Loaded>,
    props: Option<&world::World>,
    cutscene: &Cutscene,
    t: f32,
    options: &ActorOptions,
) -> math::Aabb {
    let mut bounds = math::Aabb::EMPTY;
    let Some(s) = cutscene.shot_at(t) else {
        return bounds;
    };
    let anchor = cutscene.anchor(s);
    let shot = &cutscene.shots[s];
    let local = t - shot.start;
    for actor in &shot.actors {
        let Some(state) = actor_state(shot, &actor.guid, local) else {
            continue;
        };
        let world = match options.moved {
            Some((g, m)) if g == actor.guid.0 => m,
            _ => matrix_to_world(&anchor, &state.matrix()),
        };
        let position = [world[12], world[13], world[14]];
        bounds.add_point(position);
        let tint = if options.selected == Some(actor.guid.0) {
            SELECTED_TINT
        } else {
            [1.0; 4]
        };
        let model = loaded.and_then(|l| l.actors.get(&(shot.resource, actor.guid.0)));
        let pose = state.clip.as_ref().and_then(|(clip, at)| {
            let anim = loaded?
                .clips
                .get(&(shot.resource, clip.clone()))?
                .as_ref()
                .ok()?;
            Some((anim, anim.pose(*at)))
        });
        // The actor's position (0x206) is where the clip's root bone
        // (the trajectory on the ground) is when the clip starts: the
        // root's own translation only moves it from there. People are
        // drawn, and moved, the rig's base scale larger: their rigs and
        // models are 1.0 high, the scale 1.88 (`docs/formats/cutscenes.md`
        // §9.2).
        let root = pose
            .as_ref()
            .map(|(anim, _)| anim.skeleton.bones.iter().position(|b| b.parent.is_none()))
            .unwrap_or_default();
        let actor_world = world;
        let world = match (&pose, root) {
            (Some((anim, _)), Some(r)) => {
                let start = anim.pose(0.0);
                let s = anim.scale;
                let offset =
                    math::translation([-start[r][12] * s, -start[r][13] * s, -start[r][14] * s]);
                math::mul(&math::mul(&world, &offset), &math::uniform_scale(s))
            }
            _ => world,
        };
        let mut drawn = false;
        if let (Some(model), Some(gpu)) = (model, gpu) {
            for part in &model.parts {
                let mesh = match (&pose, &part.skeleton) {
                    (Some((anim, globals)), Some(skeleton)) => {
                        // Model bone → animated bone by name hash.
                        let matrices: Vec<Mat4> = skeleton
                            .bones
                            .iter()
                            .map(|b| {
                                anim.skeleton
                                    .bones
                                    .iter()
                                    .position(|x| x.hash == b.hash)
                                    .map_or(math::IDENTITY, |i| {
                                        multiply(&globals[i], &b.inverse_bind)
                                    })
                            })
                            .collect();
                        Arc::new(skin(&part.bind, &matrices))
                    }
                    _ => part.bind.clone(),
                };
                let mut renderer = gpu.lock();
                // A posed copy never pushes out a mesh drawn this frame: when
                // the budget is full, ties of the least recently drawn
                // meshes would drop the copies just made.
                if !renderer.has_mesh(part.key) {
                    let room = renderer.mesh_count() + 16;
                    if room > renderer.mesh_budget() {
                        renderer.set_mesh_budget(room);
                    }
                }
                renderer.upload_mesh(part.key, mesh);
                drop(renderer);
                let mut instance = Instance::new(part.key, world);
                instance.tint = tint;
                scene.instances.push(instance);
                drawn = true;
            }
        }
        // Props: the meshes of their object in the shot stream.
        if !drawn
            && let Some(props) = props
            && actor.assembly.is_none()
        {
            // Moved by the clip's root bone when the clip is loaded.
            let placed = match (&pose, root) {
                (Some((_, globals)), Some(r)) => math::mul(&world, &globals[r]),
                _ => actor_world,
            };
            for (mesh, local) in props.props(&actor.guid.0) {
                let mut instance = Instance::new(*mesh, math::mul(&placed, local));
                instance.tint = tint;
                scene.instances.push(instance);
                drawn = true;
            }
        }
        // Props and actors without a character: their skeleton or a cross.
        if !drawn {
            let colour = if options.selected == Some(actor.guid.0) {
                [255, 170, 40, 255]
            } else {
                MARKER_COLOUR
            };
            if let Some((anim, globals)) = pose.as_ref().filter(|_| actor.assembly.is_some()) {
                for (i, bone) in anim.skeleton.bones.iter().enumerate() {
                    if let Some(p) = bone.parent {
                        let a = point(&world, math::transform_point(&globals[i], [0.0; 3]));
                        let b = point(&world, math::transform_point(&globals[p], [0.0; 3]));
                        scene.overlay_lines.push(LineVertex::new(a, colour));
                        scene.overlay_lines.push(LineVertex::new(b, colour));
                    }
                }
            } else {
                let c = position;
                for axis in 0..3 {
                    let mut a = c;
                    let mut b = c;
                    a[axis] -= 0.15;
                    b[axis] += 0.15;
                    scene.overlay_lines.push(LineVertex::new(a, colour));
                    scene.overlay_lines.push(LineVertex::new(b, colour));
                }
            }
        }
    }
    bounds
}

/// The camera path of shot `s` in the world, `per_second` points a second.
pub(crate) fn camera_path(cutscene: &Cutscene, s: usize, per_second: f32) -> Vec<[f32; 3]> {
    let shot = &cutscene.shots[s];
    let anchor = cutscene.anchor(s);
    let steps = (shot.duration() * per_second).ceil().max(1.0) as usize;
    (0..=steps)
        .filter_map(|i| {
            let at = shot.duration() * i as f32 / steps as f32;
            let c = camera_state(shot, at)?;
            Some(to_world(&anchor, [c.values[0], c.values[1], c.values[2]]))
        })
        .collect()
}

/// A polyline as line segments.
pub(crate) fn push_path(lines: &mut Vec<LineVertex>, path: &[[f32; 3]], colour: [u8; 4]) {
    for pair in path.windows(2) {
        lines.push(LineVertex::new(pair[0], colour));
        lines.push(LineVertex::new(pair[1], colour));
    }
}

/// A camera as a pyramid: from the eye to the corners of its picture
/// `depth` metres ahead (camera to world `m`, column-major).
pub(crate) fn frustum_lines(
    m: &[f32; 16],
    vertical_fov: f32,
    aspect: f32,
    depth: f32,
    colour: [u8; 4],
) -> Vec<LineVertex> {
    let eye = point(m, [0.0; 3]);
    let h = (vertical_fov.to_radians() / 2.0).tan();
    let w = h * aspect;
    let corners = [[-w, -h, -1.0], [w, -h, -1.0], [w, h, -1.0], [-w, h, -1.0]]
        .map(|v| point(m, v.map(|x| x * depth)));
    let mut out = Vec::with_capacity(20);
    for i in 0..4 {
        for (a, b) in [(eye, corners[i]), (corners[i], corners[(i + 1) % 4])] {
            out.push(LineVertex::new(a, colour));
            out.push(LineVertex::new(b, colour));
        }
    }
    // The top of the picture: a small triangle over its upper edge.
    let up = [
        point(m, [-w * 0.4 * depth, h * depth, -depth]),
        point(m, [0.0, h * 1.4 * depth, -depth]),
        point(m, [w * 0.4 * depth, h * depth, -depth]),
    ];
    for (a, b) in [(up[0], up[1]), (up[1], up[2])] {
        out.push(LineVertex::new(a, colour));
        out.push(LineVertex::new(b, colour));
    }
    out
}
