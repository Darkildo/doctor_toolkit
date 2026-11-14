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

pub(super) fn open_game(root: &PathBuf) -> Result<(Arc<GameInstance>, Arc<ResourceIndex>), String> {
    static OPENED: Mutex<Option<OpenedGame>> = Mutex::new(None);
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
        let result = open_game(&self.root).map(|(game, index)| {
            let mut assets = GameAssets::new(game.clone(), &index);
            let rigs = assets.rigs().to_vec();
            let cast = Cast::new(&game, &index, rigs);
            Loaded {
                game,
                assets,
                cast,
                actors: HashMap::new(),
                clips: HashMap::new(),
            }
        });
        *self.shared.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
        self.generation += 1;
    }

    /// Problems found while loading the actors and clips of a shot.
    pub fn problems(&self, cutscene: &Cutscene, s: usize) -> Vec<String> {
        let guard = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        let Some(Ok(loaded)) = &*guard else {
            return Vec::new();
        };
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

    /// Characters and clips of the actors of shot `s`.
    fn ensure_shot(&mut self, loaded: &mut Loaded, cutscene: &Cutscene, s: usize) {
        let Some(gpu) = self.gpu.clone() else { return };
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
        if !self.characters {
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
                        let skeleton = loaded.assets.skeleton(&part.model.bones);
                        let mut renderer = gpu.lock();
                        match load_with(
                            &part.model,
                            bind_key,
                            &mut loaded.assets,
                            &mut renderer,
                            &options,
                        ) {
                            Ok(m) => model.parts.push(PartModel {
                                key: key_of(("posed", shot.resource, a, p)),
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
            self.ensure_shot(l, cutscene, s);
        }
        let loaded = loaded.as_deref();
        let shot = &cutscene.shots[s];
        let local = t - shot.start;
        let mut bounds = math::Aabb::EMPTY;
        for (a, actor) in shot.actors.iter().enumerate() {
            let Some(state) = actor_state(shot, &actor.guid, local) else {
                continue;
            };
            let world = matrix_to_world(&anchor, &state.matrix());
            let position = [world[12], world[13], world[14]];
            bounds.add_point(position);
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
                    let offset = math::translation([
                        -start[r][12] * s,
                        -start[r][13] * s,
                        -start[r][14] * s,
                    ]);
                    math::mul(&math::mul(&world, &offset), &math::uniform_scale(s))
                }
                _ => world,
            };
            let mut drawn = false;
            let _ = a;
            if let (Some(model), Some(gpu)) = (model, &self.gpu) {
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
                    gpu.lock().upload_mesh(part.key, mesh);
                    self.scene.instances.push(Instance::new(part.key, world));
                    drawn = true;
                }
            }
            // Props: the meshes of their object in the shot stream.
            if !drawn && world_shown && actor.assembly.is_none() {
                // Moved by the clip's root bone when the clip is loaded.
                let placed = match (&pose, root) {
                    (Some((_, globals)), Some(r)) => math::mul(&world, &globals[r]),
                    _ => actor_world,
                };
                for (mesh, local) in world_state.props(&actor.guid.0) {
                    self.scene
                        .instances
                        .push(Instance::new(*mesh, math::mul(&placed, local)));
                    drawn = true;
                }
            }
            // Props and actors without a character: their skeleton or a cross.
            if !drawn {
                let colour = [255, 200, 60, 255];
                if let Some((anim, globals)) = pose.as_ref().filter(|_| actor.assembly.is_some()) {
                    for (i, bone) in anim.skeleton.bones.iter().enumerate() {
                        if let Some(p) = bone.parent {
                            let a = point(&world, math::transform_point(&globals[i], [0.0; 3]));
                            let b = point(&world, math::transform_point(&globals[p], [0.0; 3]));
                            self.scene.overlay_lines.push(LineVertex::new(a, colour));
                            self.scene.overlay_lines.push(LineVertex::new(b, colour));
                        }
                    }
                } else {
                    let c = position;
                    for axis in 0..3 {
                        let mut a = c;
                        let mut b = c;
                        a[axis] -= 0.15;
                        b[axis] += 0.15;
                        self.scene.overlay_lines.push(LineVertex::new(a, colour));
                        self.scene.overlay_lines.push(LineVertex::new(b, colour));
                    }
                }
            }
        }
        // The camera path of the shot and, seen from outside, the camera.
        let path_colour = [80, 200, 255, 255];
        let steps = (shot.duration() * 10.0).ceil().max(1.0) as usize;
        let mut previous = None;
        for i in 0..=steps {
            let at = shot.duration() * i as f32 / steps as f32;
            if let Some(c) = camera_state(shot, at) {
                let p = to_world(&anchor, [c.values[0], c.values[1], c.values[2]]);
                bounds.add_point(p);
                if let Some(q) = previous {
                    self.scene.lines.push(LineVertex::new(q, path_colour));
                    self.scene.lines.push(LineVertex::new(p, path_colour));
                }
                previous = Some(p);
            }
        }
        if !self.shot_camera
            && let Some(c) = camera_state(shot, local)
        {
            let m = matrix_to_world(&anchor, &c.matrix());
            let eye = point(&m, [0.0; 3]);
            let h = (c.vertical_fov().to_radians() / 2.0).tan();
            let w = h * c.aspect;
            let corners = [[-w, -h, -1.0], [w, -h, -1.0], [w, h, -1.0], [-w, h, -1.0]]
                .map(|v| point(&m, v.map(|x| x * 0.8)));
            let colour = [255, 90, 90, 255];
            for i in 0..4 {
                for (a, b) in [(eye, corners[i]), (corners[i], corners[(i + 1) % 4])] {
                    self.scene.overlay_lines.push(LineVertex::new(a, colour));
                    self.scene.overlay_lines.push(LineVertex::new(b, colour));
                }
            }
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
        // The world: start it, take what the workers finished, upload meshes.
        let (world_shown, world_generation, mission) = {
            let mut world = self.world.lock().unwrap_or_else(|e| e.into_inner());
            let ctx = ui.ctx().clone();
            if world.enabled && world.is_idle() {
                world.start(self.root.clone(), cutscene, open_game, &ctx);
            }
            world.poll(&ctx);
            world.upload(&gpu);
            (world.enabled, world.generation, world.mission_time)
        };
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
        );
        if self.last != Some(key) {
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
                            rect.aspect_ratio(),
                            near,
                            far,
                        ),
                        eye: [m[12], m[13], m[14]],
                    }
                }
                _ => {
                    let mut view = self.orbit.view(rect.aspect_ratio());
                    if world_shown {
                        view.projection = math::perspective(
                            self.orbit.fov_y,
                            rect.aspect_ratio().max(1e-3),
                            near,
                            far,
                        );
                    }
                    view
                }
            };
            self.build(cutscene, t, &view);
            let defaults = crate::render::SceneOptions::default();
            if world_shown {
                let (environment, sky) = world::environment(hours);
                self.scene.options.environment = Some(environment);
                self.scene.options.background = sky;
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
}
