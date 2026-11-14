//! The wgpu side: pipelines, uploaded meshes and textures, offscreen viewports
//! and the id pass used for picking.

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use eframe::egui_wgpu;
use eframe::wgpu;
use eframe::wgpu::util::DeviceExt;

use super::batch::{self, Arena, Batching, DrawCmd, DrawKey, Run, Slot};
use super::camera::View;
use super::math::{self, Aabb, Mat4};
use super::scene::{
    Instance, LineVertex, MaterialData, MeshData, MeshKey, Pick, Scene, TextureData, TextureKey,
    Vertex,
};
use super::timing::GpuTimer;
use crate::profile;

const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const PICK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg32Uint;
/// Winding of front faces in the game data (Direct3D: clockwise).
const FRONT_FACE: wgpu::FrontFace = wgpu::FrontFace::Cw;
/// Meshes kept on the GPU before the least recently drawn ones are dropped.
const DEFAULT_MESH_BUDGET: usize = 48;

const FLAG_TEXTURES: u32 = 1;
const FLAG_LIGHTING: u32 = 2;
const FLAG_VERTEX_COLORS: u32 = 4;
const FLAG_ALPHA_TEST: u32 = 8;
const FLAG_BONE_WEIGHTS: u32 = 16;
const FLAG_SURFACES: u32 = 32;
/// "As in the game" shading (`game_look.rs`).
const FLAG_GAME: u32 = 64;

/// Which textures a surface has (bits of `MaterialUniform::flags.z`).
const HAS_NORMAL: u32 = 1;
const HAS_GLOSS: u32 = 2;
const HAS_DETAIL_NORMAL: u32 = 4;
const HAS_DETAIL_MASK: u32 = 8;
const HAS_DETAIL_GLOSS: u32 = 16;
/// The game's sky dome mesh (`MaterialData::sky`).
const IS_SKY: u32 = 32;
/// Texture bindings of a material after the uniform: base, normal, gloss,
/// detail normal, detail mask, detail gloss (the sampler is binding 2).
const MATERIAL_TEXTURES: [u32; 6] = [1, 3, 4, 5, 6, 7];

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    view_proj: Mat4,
    eye: [f32; 4],
    light: [f32; 4],
    params: [u32; 4],
    line: [f32; 4],
    /// rgb: colour of the light (linear), w: weight of the light from the camera.
    sun: [f32; 4],
    /// rgb: ambient colour multiplying `light.w`.
    ambient: [f32; 4],
    /// rgb: fog colour (linear).
    fog_color: [f32; 4],
    /// x: fog start, y: fog end (metres from the eye), z: fog at the end (0 = none).
    fog: [f32; 4],
    /// The camera's right, up and forward axes in the world (leaf cards
    /// turned to the camera, [`Vertex::BILLBOARD`]).
    camera: [[f32; 4]; 3],
    game: super::game_look::GameUniform,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct MaterialUniform {
    color: [f32; 4],
    /// x: base texture present, y: surface model (0 = plain), z: `HAS_*` bits.
    flags: [u32; 4],
    tints: [[f32; 4]; 3],
    tint: [f32; 4],
    /// rgb: skin tint, w: base texture exponent.
    skin: [f32; 4],
    detail_uv: [f32; 4],
    mask_uv: [f32; 4],
    /// rgb: specular colour, w: exponent.
    spec: [f32; 4],
    /// x: base weight under the detail mask, y: detail normal weight.
    params: [f32; 4],
}

impl MaterialUniform {
    fn new(material: &MaterialData, has: impl Fn(Option<TextureKey>) -> bool) -> Self {
        let mut u = Self {
            color: material.color,
            flags: [has(material.texture) as u32, 0, 0, 0],
            tints: [[1.0; 4]; 3],
            tint: [1.0; 4],
            skin: [1.0; 4],
            detail_uv: [1.0, 1.0, 0.0, 0.0],
            mask_uv: [1.0, 1.0, 0.0, 0.0],
            spec: [0.0, 0.0, 0.0, 16.0],
            params: [0.0; 4],
        };
        if material.sky {
            u.flags[2] |= IS_SKY;
        }
        if let Some(surface) = &material.surface {
            let mut bits = 0;
            for (texture, bit) in [
                (surface.normal, HAS_NORMAL),
                (surface.gloss, HAS_GLOSS),
                (surface.detail_normal, HAS_DETAIL_NORMAL),
                (surface.detail_mask, HAS_DETAIL_MASK),
                (surface.detail_gloss, HAS_DETAIL_GLOSS),
            ] {
                if has(texture) {
                    bits |= bit;
                }
            }
            u.flags[1] = surface.model as u32;
            u.flags[2] |= bits;
            u.flags[3] = surface.blend as u32;
            for (o, t) in u.tints.iter_mut().zip(surface.tints) {
                *o = [t[0], t[1], t[2], 1.0];
            }
            u.tint = surface.tint;
            let k = surface.skin_tint;
            u.skin = [k[0], k[1], k[2], surface.base_gamma];
            u.detail_uv = surface.detail_uv;
            u.mask_uv = surface.detail_mask_uv;
            let c = surface.specular;
            u.spec = [c[0], c[1], c[2], surface.specular_power];
            u.params = [
                surface.detail_mask_strength,
                surface.detail_normal_strength,
                0.0,
                0.0,
            ];
        }
        u
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct InstanceRaw {
    transform: Mat4,
    tint: [f32; 4],
    ids: [u32; 2],
    _pad: [u32; 2],
}

impl InstanceRaw {
    fn new(instance: &Instance, pick_id: u32) -> Self {
        Self {
            transform: instance.transform,
            tint: instance.tint,
            ids: [pick_id, 0],
            _pad: [0; 2],
        }
    }
}

/// Device, queue and adapter the renderer works with.
#[derive(Clone)]
pub struct GpuContext {
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// egui's renderer, to show viewports as images; `None` when headless.
    pub egui: Option<Arc<egui::mutex::RwLock<egui_wgpu::Renderer>>>,
}

impl GpuContext {
    pub fn from_egui(state: &egui_wgpu::RenderState) -> Self {
        Self {
            adapter: state.adapter.clone(),
            device: state.device.clone(),
            queue: state.queue.clone(),
            egui: Some(state.renderer.clone()),
        }
    }

    /// A device without a window, for tests and benchmarks; `None` without an adapter.
    pub fn headless() -> Option<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::from_env()
                .unwrap_or(wgpu::Backends::PRIMARY | wgpu::Backends::GL),
            flags: wgpu::InstanceFlags::from_build_config().with_env(),
            backend_options: wgpu::BackendOptions::from_env_or_default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            display: None,
        });
        let mut adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
        adapters.sort_by_key(adapter_rank);
        // Some adapters are listed but cannot open a device (e.g. GL without a display).
        adapters.into_iter().find_map(|adapter| {
            let limits = if adapter.get_info().backend == wgpu::Backend::Gl {
                wgpu::Limits::downlevel_webgl2_defaults()
            } else {
                wgpu::Limits::default()
            };
            let descriptor = wgpu::DeviceDescriptor {
                label: Some("doctor_toolkit headless"),
                // Batched draws; benchmarks time the scene pass (stage 67).
                required_features: adapter.features() & optional_features(true),
                required_limits: wgpu::Limits {
                    max_texture_dimension_2d: 8192,
                    ..limits
                },
                ..Default::default()
            };
            let (device, queue) = pollster::block_on(adapter.request_device(&descriptor)).ok()?;
            Some(Self {
                adapter,
                device,
                queue,
                egui: None,
            })
        })
    }

    pub fn adapter_summary(&self) -> String {
        let info = self.adapter.get_info();
        format!("{} ({:?}, {:?})", info.name, info.backend, info.device_type)
    }
}

struct GpuTexture {
    view: wgpu::TextureView,
    bytes: usize,
    /// Unique per upload: materials made with this texture name it.
    serial: u64,
}

/// The bind group of a material, shared by every part with the same
/// uniform and textures (stage 67).
struct MaterialBind {
    /// Unique per bind group: parts are sorted and batched by it.
    id: u32,
    bind_group: wgpu::BindGroup,
}

struct GpuPart {
    material: Arc<MaterialBind>,
    first_index: u32,
    index_count: u32,
    /// Drawn in the alpha-blended pass.
    blend: bool,
}

struct GpuMesh {
    data: Arc<MeshData>,
    bounds: Aabb,
    /// Vertices and indices in the shared buffers; `None` when empty.
    slot: Option<Slot>,
    parts: Vec<GpuPart>,
    /// Line-list indices of the triangle edges, built on first use.
    edges: Option<(wgpu::Buffer, u32)>,
    /// Normal lines, built on first use.
    normals: Option<(wgpu::Buffer, u32)>,
    last_used: u64,
}

/// Offscreen targets of one on-screen view.
pub struct Viewport {
    size: [u32; 2],
    color: Option<wgpu::Texture>,
    color_view: Option<wgpu::TextureView>,
    msaa_view: Option<wgpu::TextureView>,
    depth_view: Option<wgpu::TextureView>,
    pick: Option<PickTargets>,
    texture_id: Option<egui::TextureId>,
    egui: Option<Arc<egui::mutex::RwLock<egui_wgpu::Renderer>>>,
}

struct PickTargets {
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    readback: wgpu::Buffer,
}

impl Default for Viewport {
    fn default() -> Self {
        Self::new()
    }
}

impl Viewport {
    pub fn new() -> Self {
        Self {
            size: [0, 0],
            color: None,
            color_view: None,
            msaa_view: None,
            depth_view: None,
            pick: None,
            texture_id: None,
            egui: None,
        }
    }

    /// The image egui shows; `None` before the first render.
    pub fn texture_id(&self) -> Option<egui::TextureId> {
        self.texture_id
    }

    /// Recreates the targets for a new size; returns whether they changed.
    fn ensure(&mut self, renderer: &SceneRenderer, size: [u32; 2]) -> bool {
        let size = [size[0].clamp(1, 8192), size[1].clamp(1, 8192)];
        if size == self.size && self.color.is_some() {
            return false;
        }
        let device = &renderer.context.device;
        let extent = wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        };
        let target = |label, format, samples, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let color = target(
            "viewport color",
            COLOR_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
        );
        let color_view = color.create_view(&Default::default());
        let samples = renderer.sample_count;
        self.msaa_view = (samples > 1).then(|| {
            target(
                "viewport msaa",
                COLOR_FORMAT,
                samples,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
            .create_view(&Default::default())
        });
        self.depth_view = Some(
            target(
                "viewport depth",
                DEPTH_FORMAT,
                samples,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
            .create_view(&Default::default()),
        );
        self.pick = None;
        if let Some(egui) = &renderer.context.egui {
            let mut egui_renderer = egui.write();
            match self.texture_id {
                Some(id) => egui_renderer.update_egui_texture_from_wgpu_texture(
                    device,
                    &color_view,
                    wgpu::FilterMode::Linear,
                    id,
                ),
                None => {
                    self.texture_id = Some(egui_renderer.register_native_texture(
                        device,
                        &color_view,
                        wgpu::FilterMode::Linear,
                    ));
                }
            }
            self.egui = Some(egui.clone());
        }
        self.color = Some(color);
        self.color_view = Some(color_view);
        self.size = size;
        true
    }

    fn pick_targets(&mut self, device: &wgpu::Device) -> &PickTargets {
        let size = self.size;
        self.pick.get_or_insert_with(|| {
            let extent = wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            };
            let texture = |label, format, usage| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
            };
            let color = texture(
                "pick ids",
                PICK_FORMAT,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            );
            let depth = texture(
                "pick depth",
                DEPTH_FORMAT,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            );
            PickTargets {
                color_view: color.create_view(&Default::default()),
                color,
                depth_view: depth.create_view(&Default::default()),
                readback: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("pick readback"),
                    size: wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as u64,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
            }
        })
    }
}

impl Drop for Viewport {
    fn drop(&mut self) {
        if let (Some(egui), Some(id)) = (&self.egui, self.texture_id) {
            egui.write().free_texture(&id);
        }
    }
}

/// Counters of the last rendered frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RenderStats {
    pub instances: usize,
    pub culled: usize,
    /// Draw calls sent to the GPU (an indirect multi-draw counts once).
    pub draw_calls: usize,
    /// Parts drawn: one mesh part for all its instances.
    pub parts: usize,
    pub triangles: usize,
    /// CPU time spent recording and submitting the frame.
    pub cpu_time: Duration,
    /// GPU time of the scene pass of a recent frame, where the device can
    /// time passes (profiler, benchmark).
    pub gpu_time: Option<Duration>,
}

/// The parts of a frame in drawing order, cut into runs, and the bind
/// groups of their materials.
struct Commands {
    cmds: Vec<DrawCmd>,
    runs: Vec<Run>,
    binds: HashMap<u32, Arc<MaterialBind>>,
}

/// One mesh with the slice of the instance buffer holding its instances.
struct DrawGroup {
    mesh: MeshKey,
    first: u32,
    count: u32,
}

/// Draws scenes of instanced meshes into viewports.
///
/// Meshes and textures are uploaded once under caller-chosen keys and kept
/// until the mesh budget is exceeded; a scene then refers to them by key.
pub struct SceneRenderer {
    context: GpuContext,
    sample_count: u32,
    globals: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    material_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    white: wgpu::TextureView,
    /// Tangent-space "straight up" normal for surfaces without a normal map.
    flat_normal: wgpu::TextureView,
    /// Without and with back-face culling.
    mesh_pipelines: [wgpu::RenderPipeline; 2],
    /// Alpha-blended parts, without depth writes.
    blend_pipelines: [wgpu::RenderPipeline; 2],
    pick_pipelines: [wgpu::RenderPipeline; 2],
    wire_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    overlay_pipeline: wgpu::RenderPipeline,
    /// The game's sky dome behind the scene (a full-screen triangle).
    sky_pipeline: wgpu::RenderPipeline,
    meshes: HashMap<MeshKey, GpuMesh>,
    textures: HashMap<TextureKey, GpuTexture>,
    /// Shared vertex and index buffers of the meshes.
    arena: Arena,
    /// Bind groups of materials by uniform and texture serials.
    materials: HashMap<Vec<u8>, Weak<MaterialBind>>,
    next_material: u32,
    next_texture: u64,
    batching: Batching,
    /// Arguments of the indirect draws of a frame.
    indirect: Option<(wgpu::Buffer, u64)>,
    instances: Option<(wgpu::Buffer, u64)>,
    identity: wgpu::Buffer,
    lines: Option<(wgpu::Buffer, u64)>,
    frame: u64,
    mesh_budget: usize,
    timer: Option<GpuTimer>,
    pub stats: RenderStats,
}

impl SceneRenderer {
    pub fn new(context: GpuContext) -> Self {
        let device = &context.device;
        let supports = |format: wgpu::TextureFormat, samples| {
            context
                .adapter
                .get_texture_format_features(format)
                .flags
                .sample_count_supported(samples)
        };
        let sample_count = if supports(COLOR_FORMAT, 4) && supports(DEPTH_FORMAT, 4) {
            4
        } else {
            1
        };
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("scene.wgsl").into()),
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(std::mem::size_of::<Globals>() as u64),
                },
                count: None,
            }],
        });
        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(
                            std::mem::size_of::<MaterialUniform>() as u64
                        ),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                texture_entry(3),
                texture_entry(4),
                texture_entry(5),
                texture_entry(6),
                texture_entry(7),
            ],
        });
        let mesh_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh layout"),
            bind_group_layouts: &[Some(&globals_layout), Some(&material_layout)],
            immediate_size: 0,
        });
        let line_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("line layout"),
            bind_group_layouts: &[Some(&globals_layout)],
            immediate_size: 0,
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Unorm8x4,
                4 => Uint8x4, 5 => Unorm8x4, 6 => Uint32
            ],
        };
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<InstanceRaw>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![
                8 => Float32x4, 9 => Float32x4, 10 => Float32x4, 11 => Float32x4,
                12 => Float32x4, 13 => Uint32x2
            ],
        };
        let line_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Unorm8x4],
        };
        let depth = |write: bool, compare| {
            Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(write),
                depth_compare: Some(compare),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            })
        };
        let pipeline = |label: &str,
                        layout: &wgpu::PipelineLayout,
                        vs: &str,
                        fs: &str,
                        buffers: &[Option<wgpu::VertexBufferLayout<'_>>],
                        topology,
                        depth_stencil,
                        format,
                        samples,
                        blend: Option<wgpu::BlendState>,
                        cull_mode: Option<wgpu::Face>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    buffers,
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology,
                    front_face: FRONT_FACE,
                    cull_mode,
                    ..Default::default()
                },
                depth_stencil,
                multisample: wgpu::MultisampleState {
                    count: samples,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        use wgpu::CompareFunction::{Always, LessEqual};
        use wgpu::PrimitiveTopology::{LineList, TriangleList};
        let mesh_buffers = [Some(vertex_layout.clone()), Some(instance_layout.clone())];
        let line_buffers = [Some(line_vertex_layout), Some(instance_layout)];
        let alpha = Some(wgpu::BlendState::ALPHA_BLENDING);
        let mesh_pipelines = [None, Some(wgpu::Face::Back)].map(|cull| {
            pipeline(
                "mesh",
                &mesh_layout,
                "vs_mesh",
                "fs_mesh",
                &mesh_buffers,
                TriangleList,
                depth(true, LessEqual),
                COLOR_FORMAT,
                sample_count,
                None,
                cull,
            )
        });
        let blend_pipelines = [None, Some(wgpu::Face::Back)].map(|cull| {
            pipeline(
                "blended mesh",
                &mesh_layout,
                "vs_mesh",
                "fs_mesh",
                &mesh_buffers,
                TriangleList,
                depth(false, LessEqual),
                COLOR_FORMAT,
                sample_count,
                alpha,
                cull,
            )
        });
        let pick_pipelines = [None, Some(wgpu::Face::Back)].map(|cull| {
            pipeline(
                "pick",
                &mesh_layout,
                "vs_mesh",
                "fs_pick",
                &mesh_buffers,
                TriangleList,
                depth(true, LessEqual),
                PICK_FORMAT,
                1,
                None,
                cull,
            )
        });
        let wire_pipeline = pipeline(
            "wireframe",
            &line_layout,
            "vs_wire",
            "fs_wire",
            &mesh_buffers,
            LineList,
            depth(false, LessEqual),
            COLOR_FORMAT,
            sample_count,
            alpha,
            None,
        );
        let line_pipeline = pipeline(
            "lines",
            &line_layout,
            "vs_line",
            "fs_line",
            &line_buffers,
            LineList,
            depth(false, LessEqual),
            COLOR_FORMAT,
            sample_count,
            alpha,
            None,
        );
        let overlay_pipeline = pipeline(
            "overlay lines",
            &line_layout,
            "vs_line",
            "fs_line",
            &line_buffers,
            LineList,
            depth(false, Always),
            COLOR_FORMAT,
            sample_count,
            alpha,
            None,
        );
        let sky_pipeline = pipeline(
            "sky",
            &line_layout,
            "vs_sky",
            "fs_sky",
            &[],
            TriangleList,
            depth(false, Always),
            COLOR_FORMAT,
            sample_count,
            None,
            None,
        );

        let anisotropic = context
            .adapter
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::ANISOTROPIC_FILTERING);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("diffuse sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: if anisotropic { 8 } else { 1 },
            ..Default::default()
        });
        let pixel = |label: &str, rgba: [u8; 4]| {
            device
                .create_texture_with_data(
                    &context.queue,
                    &wgpu::TextureDescriptor {
                        label: Some(label),
                        size: wgpu::Extent3d {
                            width: 1,
                            height: 1,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    },
                    wgpu::util::TextureDataOrder::LayerMajor,
                    &rgba,
                )
                .create_view(&Default::default())
        };
        let flat_normal = pixel("flat normal", [128, 128, 255, 255]);
        let white = pixel("white", [255; 4]);
        let identity = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("identity instance"),
            contents: bytemuck::bytes_of(&InstanceRaw::new(&Instance::new(0, math::IDENTITY), 0)),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let timer = GpuTimer::new(&context.device, &context.queue);
        let batching = Batching::for_device(&context.device, &context.adapter);
        let arena = Arena::new(
            std::mem::size_of::<Vertex>(),
            context.device.limits().max_buffer_size,
            context
                .adapter
                .get_downlevel_capabilities()
                .flags
                .contains(wgpu::DownlevelFlags::BASE_VERTEX),
        );
        Self {
            context,
            sample_count,
            globals,
            globals_bind_group,
            material_layout,
            sampler,
            white,
            flat_normal,
            mesh_pipelines,
            blend_pipelines,
            pick_pipelines,
            wire_pipeline,
            line_pipeline,
            overlay_pipeline,
            sky_pipeline,
            meshes: HashMap::new(),
            textures: HashMap::new(),
            arena,
            materials: HashMap::new(),
            next_material: 0,
            next_texture: 0,
            batching,
            indirect: None,
            instances: None,
            identity,
            lines: None,
            frame: 0,
            mesh_budget: DEFAULT_MESH_BUDGET,
            timer,
            stats: RenderStats::default(),
        }
    }

    /// Whether the adapter is a software rasteriser (lavapipe, llvmpipe):
    /// there the id pass is slow and ray picking on the CPU is preferable.
    pub fn is_software(&self) -> bool {
        self.context.adapter.get_info().device_type == wgpu::DeviceType::Cpu
    }

    /// Samples per pixel of the colour target (1 or 4).
    pub fn sample_count(&self) -> u32 {
        self.sample_count
    }

    /// The device the renderer draws with.
    pub fn context(&self) -> &GpuContext {
        &self.context
    }

    /// Keeps up to `budget` meshes on the GPU (at least the ones drawn last).
    pub fn set_mesh_budget(&mut self, budget: usize) {
        self.mesh_budget = budget.max(1);
    }

    /// Meshes kept on the GPU (see [`Self::set_mesh_budget`]).
    /// Meshes on the GPU now.
    pub fn mesh_count(&self) -> usize {
        self.meshes.len()
    }

    pub fn mesh_budget(&self) -> usize {
        self.mesh_budget
    }

    pub fn has_texture(&self, key: TextureKey) -> bool {
        self.textures.contains_key(&key)
    }

    pub fn texture_bytes(&self) -> usize {
        self.textures.values().map(|t| t.bytes).sum()
    }

    /// Uploads a texture with its mip chain; levels that do not fit are dropped.
    pub fn upload_texture(&mut self, key: TextureKey, data: &TextureData) {
        let width = data.width.clamp(1, 8192);
        let height = data.height.clamp(1, 8192);
        let max_levels = 32 - width.max(height).leading_zeros();
        let levels: Vec<&Vec<u8>> = data
            .mips
            .iter()
            .enumerate()
            .take_while(|(i, mip)| {
                let (w, h) = ((width >> i).max(1), (height >> i).max(1));
                (*i as u32) < max_levels && mip.len() == (w * h * 4) as usize
            })
            .map(|(_, mip)| mip)
            .collect();
        if levels.is_empty() {
            return;
        }
        let format = if data.srgb {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };
        let texture = self
            .context
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some(&data.name),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: levels.len() as u32,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
        let mut bytes = 0;
        for (level, pixels) in levels.iter().enumerate() {
            let (w, h) = ((width >> level).max(1), (height >> level).max(1));
            self.context.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * w),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
            bytes += pixels.len();
        }
        self.next_texture += 1;
        self.textures.insert(
            key,
            GpuTexture {
                view: texture.create_view(&Default::default()),
                bytes,
                serial: self.next_texture,
            },
        );
    }

    /// How parts are sent to the GPU (see [`Batching`]).
    pub fn batching(&self) -> Batching {
        self.batching
    }

    /// Chooses how parts are sent to the GPU (indirect calls only where
    /// the device has them).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_batching(&mut self, mut batching: Batching) {
        batching.indirect &=
            Batching::device_can_draw_indirect(&self.context.device, &self.context.adapter);
        self.batching = batching;
    }

    /// Bytes of GPU memory of the shared mesh buffers.
    pub fn mesh_bytes(&self) -> u64 {
        self.arena.bytes()
    }

    /// Bind groups of materials in use.
    #[cfg(test)]
    fn material_count(&self) -> usize {
        self.materials
            .values()
            .filter(|m| m.strong_count() > 0)
            .count()
    }

    /// Where a mesh lives in the shared buffers.
    #[cfg(test)]
    fn mesh_slot(&self, key: MeshKey) -> Option<Slot> {
        self.meshes.get(&key)?.slot
    }

    /// Whether a mesh is uploaded; marks it as used.
    pub fn has_mesh(&mut self, key: MeshKey) -> bool {
        let frame = self.frame;
        self.meshes
            .get_mut(&key)
            .map(|m| m.last_used = frame)
            .is_some()
    }

    /// The CPU copy of an uploaded mesh.
    pub fn mesh_data(&self, key: MeshKey) -> Option<&Arc<MeshData>> {
        self.meshes.get(&key).map(|m| &m.data)
    }

    /// Uploads a mesh. Textures its parts name must be uploaded first; parts
    /// whose texture is missing are drawn untextured.
    pub fn upload_mesh(&mut self, key: MeshKey, data: Arc<MeshData>) {
        let vertex_count = data.vertices.len() as u32;
        let index_count = data.indices.len() as u32;
        if self.update_in_place(key, &data) {
            return;
        }
        if let Some(old) = self.meshes.remove(&key)
            && let Some(slot) = old.slot
        {
            self.arena.free(slot);
        }
        let device = self.context.device.clone();
        let slot = if vertex_count == 0 || index_count == 0 {
            None
        } else {
            let slot = self.arena.alloc(&device, vertex_count, index_count);
            if let Some(slot) = slot {
                self.arena.write(
                    &self.context.queue,
                    slot,
                    bytemuck::cast_slice(&data.vertices),
                    bytemuck::cast_slice(&data.indices),
                );
            }
            slot
        };
        let parts = data
            .parts
            .iter()
            .map(|part| GpuPart {
                material: self.material(&part.material, &part.name),
                first_index: part.first_index,
                index_count: part.index_count,
                blend: part.material.blended(),
            })
            .collect();
        let bounds = data.bounds();
        self.meshes.insert(
            key,
            GpuMesh {
                data,
                bounds,
                slot,
                parts,
                edges: None,
                normals: None,
                last_used: self.frame,
            },
        );
        self.evict();
    }

    /// A new version of a mesh with the same shape (vertex and index
    /// counts, parts and materials), e.g. a posed character of a playing
    /// cut-scene: written over the old one in place.
    fn update_in_place(&mut self, key: MeshKey, data: &Arc<MeshData>) -> bool {
        let Some(slot) = self.meshes.get(&key).and_then(|m| m.slot) else {
            return false;
        };
        if slot.vertex_count as usize != data.vertices.len()
            || slot.index_count as usize != data.indices.len()
        {
            return false;
        }
        let materials: Vec<Arc<MaterialBind>> = data
            .parts
            .iter()
            .map(|part| self.material(&part.material, &part.name))
            .collect();
        let mesh = &self.meshes[&key];
        let same =
            mesh.parts.len() == data.parts.len()
                && mesh.parts.iter().zip(&data.parts).zip(&materials).all(
                    |((old, new), material)| {
                        old.first_index == new.first_index
                            && old.index_count == new.index_count
                            && old.blend == new.material.blended()
                            && Arc::ptr_eq(&old.material, material)
                    },
                );
        if !same {
            return false;
        }
        self.arena.write(
            &self.context.queue,
            slot,
            bytemuck::cast_slice(&data.vertices),
            bytemuck::cast_slice(&data.indices),
        );
        let frame = self.frame;
        let mesh = self.meshes.get_mut(&key).expect("checked above");
        mesh.bounds = data.bounds();
        mesh.data = data.clone();
        mesh.edges = None;
        mesh.normals = None;
        mesh.last_used = frame;
        true
    }

    /// The bind group of a material: one per uniform and set of textures.
    fn material(&mut self, material: &MaterialData, label: &str) -> Arc<MaterialBind> {
        let view = |key: Option<TextureKey>| key.and_then(|k| self.textures.get(&k));
        let uniform = MaterialUniform::new(material, |k| view(k).is_some());
        let surface = material.surface.as_deref();
        let keys = [
            material.texture,
            surface.and_then(|s| s.normal),
            surface.and_then(|s| s.gloss),
            surface.and_then(|s| s.detail_normal),
            surface.and_then(|s| s.detail_mask),
            surface.and_then(|s| s.detail_gloss),
        ];
        let mut cache_key = bytemuck::bytes_of(&uniform).to_vec();
        for &k in &keys {
            let serial = view(k).map_or(0, |t| t.serial);
            cache_key.extend_from_slice(&serial.to_le_bytes());
        }
        if let Some(bind) = self.materials.get(&cache_key).and_then(Weak::upgrade) {
            return bind;
        }
        let device = &self.context.device;
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("material"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let views: Vec<&wgpu::TextureView> = keys
            .iter()
            .enumerate()
            .map(|(i, &k)| match view(k) {
                Some(t) => &t.view,
                // Normal maps (bindings 3 and 5) default to "up".
                None if i == 1 || i == 3 => &self.flat_normal,
                None => &self.white,
            })
            .collect();
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&self.sampler),
            },
        ];
        for (binding, view) in MATERIAL_TEXTURES.iter().zip(&views) {
            entries.push(wgpu::BindGroupEntry {
                binding: *binding,
                resource: wgpu::BindingResource::TextureView(view),
            });
        }
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.material_layout,
            entries: &entries,
        });
        self.next_material += 1;
        let bind = Arc::new(MaterialBind {
            id: self.next_material,
            bind_group,
        });
        // Forget the materials no mesh uses any more now and then.
        if self.materials.len() >= 64 && self.materials.len().is_power_of_two() {
            self.materials.retain(|_, m| m.strong_count() > 0);
        }
        self.materials.insert(cache_key, Arc::downgrade(&bind));
        bind
    }

    /// Drops the least recently drawn meshes over the budget and textures no
    /// remaining mesh uses.
    fn evict(&mut self) {
        if self.meshes.len() <= self.mesh_budget {
            return;
        }
        let mut order: Vec<(u64, MeshKey)> =
            self.meshes.iter().map(|(k, m)| (m.last_used, *k)).collect();
        order.sort();
        let excess = self.meshes.len() - self.mesh_budget;
        for (_, key) in order.into_iter().take(excess) {
            if let Some(slot) = self.meshes.remove(&key).and_then(|m| m.slot) {
                self.arena.free(slot);
            }
        }
        self.materials.retain(|_, m| m.strong_count() > 0);
        let used: std::collections::HashSet<TextureKey> = self
            .meshes
            .values()
            .flat_map(|m| m.data.parts.iter().flat_map(|p| p.material.textures()))
            .collect();
        self.textures.retain(|key, _| used.contains(key));
    }

    /// Writes the per-frame uniforms and the instance and line buffers;
    /// returns the draw groups after frustum culling.
    fn prepare(&mut self, scene: &Scene, view: &View, aspect: f32) -> Vec<DrawGroup> {
        self.frame += 1;
        let view_proj = view.view_projection();
        let options = &scene.options;
        let mut flags = 0;
        for (on, flag) in [
            (options.textures, FLAG_TEXTURES),
            (options.lighting, FLAG_LIGHTING),
            (options.vertex_colors, FLAG_VERTEX_COLORS),
            (options.alpha_test, FLAG_ALPHA_TEST),
            (options.bone_weights.is_some(), FLAG_BONE_WEIGHTS),
            (options.surfaces, FLAG_SURFACES),
            (options.game.is_some() && options.lighting, FLAG_GAME),
        ] {
            if on {
                flags |= flag;
            }
        }
        let (highlight_id, highlight_part) = scene
            .highlight
            .map_or((0, Pick::ALL_PARTS), |p| (p.instance + 1, p.part));
        // Light from above, slightly behind the camera's left shoulder.
        let back = math::normalize_or(math::sub(view.eye, [0.0; 3]), [0.0, 0.0, 1.0]);
        let mut light = math::normalize_or(
            math::add([0.3, 1.0, 0.2], math::scale(back, 0.4)),
            [0.0, 1.0, 0.0],
        );
        let (mut sun, mut ambient, mut fog_color, mut fog) =
            ([1.0; 4], [1.0; 4], [0.0; 4], [0.0, 1.0, 0.0, 0.0]);
        if let Some(env) = &options.environment {
            light = math::normalize_or(env.sun, [0.0, 1.0, 0.0]);
            sun = [
                env.sun_color[0],
                env.sun_color[1],
                env.sun_color[2],
                env.camera_light,
            ];
            ambient = [env.ambient[0], env.ambient[1], env.ambient[2], 1.0];
            fog_color = [env.fog_color[0], env.fog_color[1], env.fog_color[2], 1.0];
            fog = [
                env.fog_start,
                env.fog_end.max(env.fog_start + 1.0),
                env.fog_max,
                0.0,
            ];
        }
        if let Some(game) = options.game.as_ref().filter(|_| options.lighting) {
            light = math::normalize_or(game.sun, [0.0, 1.0, 0.0]);
        }
        let globals = Globals {
            view_proj,
            eye: [view.eye[0], view.eye[1], view.eye[2], 1.0],
            light: [light[0], light[1], light[2], 0.32],
            params: [
                flags,
                options.bone_weights.unwrap_or(0) as u32,
                highlight_id,
                highlight_part,
            ],
            line: [0.0004, aspect, 0.0, 0.0],
            sun,
            ambient,
            fog_color,
            fog,
            camera: camera_axes(&view.view),
            game: super::game_look::GameUniform::new(
                options.game.as_ref().filter(|_| options.lighting),
                &view_proj,
            ),
        };
        self.context
            .queue
            .write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));

        let planes = frustum_planes(&view_proj);
        // Visible instances by mesh key, each mesh's in the scene's order.
        let mut visible: Vec<(MeshKey, u32)> = Vec::with_capacity(scene.instances.len());
        let mut culled = 0;
        let frame = self.frame;
        for (index, instance) in scene.instances.iter().enumerate() {
            let Some(mesh) = self.meshes.get_mut(&instance.mesh) else {
                continue;
            };
            mesh.last_used = frame;
            if options.frustum_culling {
                let centre = math::transform_point(&instance.transform, mesh.bounds.center());
                let radius = mesh.bounds.radius() * math::max_scale(&instance.transform);
                if !sphere_visible(&planes, centre, radius) {
                    culled += 1;
                    continue;
                }
            }
            visible.push((instance.mesh, index as u32));
        }
        visible.sort_unstable();
        let mut raw = Vec::with_capacity(visible.len());
        let mut draws: Vec<DrawGroup> = Vec::new();
        for &(key, index) in &visible {
            match draws.last_mut() {
                Some(group) if group.mesh == key => group.count += 1,
                _ => draws.push(DrawGroup {
                    mesh: key,
                    first: raw.len() as u32,
                    count: 1,
                }),
            }
            raw.push(InstanceRaw::new(
                &scene.instances[index as usize],
                index + 1,
            ));
        }
        if !raw.is_empty() {
            let bytes: &[u8] = bytemuck::cast_slice(&raw);
            let buffer = grow_buffer(
                &self.context.device,
                &mut self.instances,
                bytes.len() as u64,
                "instances",
                wgpu::BufferUsages::VERTEX,
            );
            self.context.queue.write_buffer(buffer, 0, bytes);
        }
        if !scene.lines.is_empty() || !scene.overlay_lines.is_empty() {
            let mut bytes = Vec::with_capacity(
                (scene.lines.len() + scene.overlay_lines.len()) * std::mem::size_of::<LineVertex>(),
            );
            bytes.extend_from_slice(bytemuck::cast_slice(&scene.lines));
            bytes.extend_from_slice(bytemuck::cast_slice(&scene.overlay_lines));
            let buffer = grow_buffer(
                &self.context.device,
                &mut self.lines,
                bytes.len() as u64,
                "lines",
                wgpu::BufferUsages::VERTEX,
            );
            self.context.queue.write_buffer(buffer, 0, &bytes);
        }
        self.stats = RenderStats {
            instances: raw.len(),
            culled,
            ..RenderStats::default()
        };
        draws
    }

    /// Builds the wireframe and normal buffers the options need.
    fn build_overlays(&mut self, scene: &Scene, draws: &[DrawGroup]) {
        let device = &self.context.device;
        for draw in draws {
            let Some(mesh) = self.meshes.get_mut(&draw.mesh) else {
                continue;
            };
            if scene.options.wireframe && mesh.edges.is_none() {
                let edges = mesh.data.edges();
                let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("mesh edges"),
                    contents: if edges.is_empty() {
                        &[0; 4]
                    } else {
                        bytemuck::cast_slice(&edges)
                    },
                    usage: wgpu::BufferUsages::INDEX,
                });
                mesh.edges = Some((buffer, edges.len() as u32));
            }
            if scene.options.normals && mesh.normals.is_none() {
                let length = mesh.bounds.radius().max(1e-3) * 0.015;
                let lines = mesh.data.normal_lines(length);
                let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("mesh normals"),
                    contents: if lines.is_empty() {
                        &[0; std::mem::size_of::<LineVertex>()]
                    } else {
                        bytemuck::cast_slice(&lines)
                    },
                    usage: wgpu::BufferUsages::VERTEX,
                });
                mesh.normals = Some((buffer, lines.len() as u32));
            }
        }
    }

    /// The parts of every group to draw, as the unbatched renderer drew
    /// them: by mesh, then by part. `blended`: `Some(false)` only opaque
    /// parts, `Some(true)` only alpha-blended parts, `None` every part (as
    /// opaque ones, for the id pass).
    fn draw_commands(
        &self,
        scene: &Scene,
        draws: &[DrawGroup],
        blended: Option<bool>,
        out: &mut Vec<DrawCmd>,
        binds: &mut HashMap<u32, Arc<MaterialBind>>,
    ) {
        for draw in draws {
            let Some(mesh) = self.meshes.get(&draw.mesh) else {
                continue;
            };
            let Some(slot) = mesh.slot else { continue };
            for (index, part) in mesh.parts.iter().enumerate() {
                if part.index_count == 0
                    || blended.is_some_and(|b| b != part.blend)
                    || (!scene.hidden_parts.is_empty()
                        && scene.hidden_parts.contains(&(draw.mesh, index as u32)))
                {
                    continue;
                }
                binds
                    .entry(part.material.id)
                    .or_insert_with(|| part.material.clone());
                out.push(DrawCmd {
                    key: DrawKey {
                        blend: blended == Some(true),
                        page: slot.page,
                        material: part.material.id,
                    },
                    args: wgpu::util::DrawIndexedIndirectArgs {
                        index_count: part.index_count,
                        instance_count: draw.count,
                        first_index: slot.first_index + part.first_index,
                        base_vertex: slot.first_vertex as i32,
                        first_instance: draw.first,
                    },
                });
            }
        }
    }

    /// The commands of a pass, sorted and cut into runs, with the indirect
    /// arguments written for the GPU.
    fn prepare_commands(
        &mut self,
        scene: &Scene,
        draws: &[DrawGroup],
        passes: &[Option<bool>],
    ) -> Commands {
        let mut cmds = Vec::new();
        let mut binds = HashMap::new();
        for &blended in passes {
            self.draw_commands(scene, draws, blended, &mut cmds, &mut binds);
        }
        let runs = batch::batches(&mut cmds, self.batching.sort);
        if self.batching.indirect && runs.iter().any(|r| r.count > 1) {
            let args: Vec<wgpu::util::DrawIndexedIndirectArgs> =
                cmds.iter().map(|c| c.args).collect();
            let bytes: &[u8] = bytemuck::cast_slice(&args);
            let buffer = grow_buffer(
                &self.context.device,
                &mut self.indirect,
                bytes.len() as u64,
                "indirect draws",
                wgpu::BufferUsages::INDIRECT,
            );
            self.context.queue.write_buffer(buffer, 0, bytes);
        }
        Commands { cmds, runs, binds }
    }

    /// Draws runs of commands; `pipelines` gives the pipeline of the opaque
    /// and of the blended runs. Returns (draw calls, parts, triangles).
    fn draw_runs(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        commands: &Commands,
        pipelines: [&wgpu::RenderPipeline; 2],
    ) -> (usize, usize, usize) {
        let Commands { cmds, runs, binds } = commands;
        let Some((instances, _)) = &self.instances else {
            return (0, 0, 0);
        };
        if runs.is_empty() {
            return (0, 0, 0);
        }
        pass.set_vertex_buffer(1, instances.slice(..));
        let stride = std::mem::size_of::<wgpu::util::DrawIndexedIndirectArgs>() as u64;
        let (mut calls, mut triangles) = (0, 0);
        let mut bound: (Option<bool>, Option<u32>, Option<u32>) = (None, None, None);
        for run in runs.iter() {
            if bound.0 != Some(run.key.blend) {
                pass.set_pipeline(pipelines[run.key.blend as usize]);
                bound.0 = Some(run.key.blend);
            }
            if bound.1 != Some(run.key.page) {
                let Some(page) = self.arena.page(run.key.page) else {
                    continue;
                };
                pass.set_vertex_buffer(0, page.vertices.slice(..));
                pass.set_index_buffer(page.indices.slice(..), wgpu::IndexFormat::Uint32);
                bound.1 = Some(run.key.page);
            }
            if bound.2 != Some(run.key.material) {
                let Some(bind) = binds.get(&run.key.material) else {
                    continue;
                };
                pass.set_bind_group(1, &bind.bind_group, &[]);
                bound.2 = Some(run.key.material);
            }
            let list = &cmds[run.first as usize..(run.first + run.count) as usize];
            for c in list {
                triangles += c.args.index_count as usize / 3 * c.args.instance_count as usize;
            }
            match &self.indirect {
                Some((buffer, _)) if self.batching.indirect && run.count > 1 => {
                    pass.multi_draw_indexed_indirect(buffer, run.first as u64 * stride, run.count);
                    calls += 1;
                }
                _ if self.batching.indirect => {
                    for c in list {
                        let a = c.args;
                        pass.draw_indexed(
                            a.first_index..a.first_index + a.index_count,
                            a.base_vertex,
                            a.first_instance..a.first_instance + a.instance_count,
                        );
                    }
                    calls += list.len();
                }
                _ => {
                    // Without indirect draws (OpenGL among others) the
                    // instances of a part are bound as a slice, as before.
                    let size = std::mem::size_of::<InstanceRaw>() as u64;
                    for c in list {
                        let a = c.args;
                        let first = a.first_instance as u64 * size;
                        pass.set_vertex_buffer(
                            1,
                            instances.slice(first..first + a.instance_count as u64 * size),
                        );
                        pass.draw_indexed(
                            a.first_index..a.first_index + a.index_count,
                            a.base_vertex,
                            0..a.instance_count,
                        );
                    }
                    calls += list.len();
                }
            }
        }
        (calls, cmds.len(), triangles)
    }

    /// Renders a scene into a viewport of `size` pixels.
    pub fn render(&mut self, viewport: &mut Viewport, size: [u32; 2], scene: &Scene, view: &View) {
        let started = Instant::now();
        viewport.ensure(self, size);
        let aspect = viewport.size[0] as f32 / viewport.size[1] as f32;
        let draws = {
            let _profile = profile::scope("render: instances");
            self.prepare(scene, view, aspect)
        };
        self.build_overlays(scene, &draws);
        let commands = {
            let _profile = profile::scope("render: commands");
            self.prepare_commands(scene, &draws, &[Some(false), Some(true)])
        };
        let prepared = started.elapsed();
        profile::record("render: prepare", prepared);
        let timestamps = self.timer.as_mut().and_then(|t| t.begin());
        let device = &self.context.device;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("scene"),
        });
        let (Some(color_view), Some(depth_view)) = (&viewport.color_view, &viewport.depth_view)
        else {
            return;
        };
        let bg = scene.options.background;
        let (draw_calls, parts, triangles) = {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: viewport.msaa_view.as_ref().unwrap_or(color_view),
                    resolve_target: viewport.msaa_view.as_ref().map(|_| color_view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: bg[0] as f64,
                            g: bg[1] as f64,
                            b: bg[2] as f64,
                            a: bg[3] as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                timestamp_writes: timestamps,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bind_group, &[]);
            if scene.options.lighting && scene.options.game.is_some_and(|g| g.sky.is_some()) {
                pass.set_pipeline(&self.sky_pipeline);
                pass.draw(0..3, 0..1);
            }
            let cull = scene.options.backface_culling as usize;
            let (mut calls, parts, triangles) = self.draw_runs(
                &mut pass,
                &commands,
                [&self.mesh_pipelines[cull], &self.blend_pipelines[cull]],
            );
            calls += self.draw_overlays(&mut pass, scene, &draws);
            let _profile = profile::scope("render: end of the pass");
            drop(pass);
            (calls, parts, triangles)
        };
        if let Some(timer) = &self.timer {
            timer.resolve(&mut encoder);
        }
        let submit = profile::scope("render: finish and submit");
        self.context.queue.submit([encoder.finish()]);
        drop(submit);
        if let Some(timer) = &mut self.timer {
            timer.submitted();
        }
        self.stats.draw_calls = draw_calls;
        self.stats.parts = parts;
        self.stats.triangles = triangles;
        self.stats.cpu_time = started.elapsed();
        self.stats.gpu_time = self.timer.as_ref().and_then(|t| t.last);
        profile::record("render: record and submit", self.stats.cpu_time - prepared);
        if let Some(gpu) = self.stats.gpu_time {
            profile::record("render: GPU scene pass", gpu);
        }
        profile::value("render: draw calls", draw_calls as f64);
        profile::value("render: instances", self.stats.instances as f64);
    }

    /// Wireframe, normals and scene lines; returns the number of draw calls.
    fn draw_overlays(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        scene: &Scene,
        draws: &[DrawGroup],
    ) -> usize {
        let mut calls = 0;
        let stride = std::mem::size_of::<InstanceRaw>() as u64;
        if let Some((instances, _)) = &self.instances
            && (scene.options.wireframe || scene.options.normals)
        {
            for draw in draws {
                let Some(mesh) = self.meshes.get(&draw.mesh) else {
                    continue;
                };
                let slice = instances
                    .slice(draw.first as u64 * stride..(draw.first + draw.count) as u64 * stride);
                if scene.options.wireframe
                    && let Some((edges, count)) = &mesh.edges
                    && *count > 0
                    && let Some(slot) = mesh.slot
                    && let Some(page) = self.arena.page(slot.page)
                {
                    pass.set_pipeline(&self.wire_pipeline);
                    pass.set_vertex_buffer(0, page.vertices.slice(..));
                    pass.set_vertex_buffer(1, slice);
                    pass.set_index_buffer(edges.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..*count, slot.first_vertex as i32, 0..draw.count);
                    calls += 1;
                }
                if scene.options.normals
                    && let Some((normals, count)) = &mesh.normals
                    && *count > 0
                {
                    pass.set_pipeline(&self.line_pipeline);
                    pass.set_vertex_buffer(0, normals.slice(..));
                    pass.set_vertex_buffer(1, slice);
                    pass.draw(0..*count, 0..draw.count);
                    calls += 1;
                }
            }
        }
        if let Some((lines, _)) = &self.lines {
            let stride = std::mem::size_of::<LineVertex>() as u64;
            let depth_tested = scene.lines.len() as u64;
            let overlay = scene.overlay_lines.len() as u64;
            pass.set_vertex_buffer(1, self.identity.slice(..));
            if depth_tested > 0 {
                pass.set_pipeline(&self.line_pipeline);
                pass.set_vertex_buffer(0, lines.slice(..depth_tested * stride));
                pass.draw(0..depth_tested as u32, 0..1);
                calls += 1;
            }
            if overlay > 0 {
                pass.set_pipeline(&self.overlay_pipeline);
                pass.set_vertex_buffer(
                    0,
                    lines.slice(depth_tested * stride..(depth_tested + overlay) * stride),
                );
                pass.draw(0..overlay as u32, 0..1);
                calls += 1;
            }
        }
        calls
    }

    /// The instance and part under pixel `pixel` of the last rendered view,
    /// from an id pass restricted to that pixel. Waits for the GPU.
    pub fn pick(
        &mut self,
        viewport: &mut Viewport,
        scene: &Scene,
        view: &View,
        pixel: [u32; 2],
    ) -> Option<Pick> {
        if viewport.color.is_none() || pixel[0] >= viewport.size[0] || pixel[1] >= viewport.size[1]
        {
            return None;
        }
        let aspect = viewport.size[0] as f32 / viewport.size[1] as f32;
        let stats = self.stats;
        let draws = self.prepare(scene, view, aspect);
        let commands = self.prepare_commands(scene, &draws, &[None]);
        self.stats = stats;
        let device = self.context.device.clone();
        let targets = viewport.pick_targets(&device);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("pick"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pick"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.color_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            pass.set_scissor_rect(pixel[0], pixel[1], 1, 1);
            pass.set_bind_group(0, &self.globals_bind_group, &[]);
            let pick = &self.pick_pipelines[scene.options.backface_culling as usize];
            self.draw_runs(&mut pass, &commands, [pick, pick]);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &targets.color,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: pixel[0],
                    y: pixel[1],
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &targets.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT),
                    rows_per_image: Some(1),
                },
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        self.context.queue.submit([encoder.finish()]);
        let slice = targets.readback.slice(..8);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let mapped = rx.recv().ok()?.ok();
        let ids = mapped.and_then(|()| {
            let bytes = slice.get_mapped_range().ok()?;
            Some([
                u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
                u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            ])
        });
        targets.readback.unmap();
        let [id, part] = ids?;
        (id != 0).then(|| Pick {
            instance: id - 1,
            part,
        })
    }

    /// Reads back the colour target of a viewport as RGBA rows; waits for the GPU.
    pub fn read_pixels(&self, viewport: &Viewport) -> Option<(u32, u32, Vec<u8>)> {
        let texture = viewport.color.as_ref()?;
        let [width, height] = viewport.size;
        let row = (4 * width).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let device = &self.context.device;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: row as u64 * height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(height),
                },
            },
            texture.size(),
        );
        self.context.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().ok()?.ok()?;
        let data = buffer.slice(..).get_mapped_range().ok()?;
        let mut out = Vec::with_capacity((4 * width * height) as usize);
        for y in 0..height as usize {
            let start = y * row as usize;
            out.extend_from_slice(&data[start..start + 4 * width as usize]);
        }
        Some((width, height, out))
    }

    /// Blocks until the GPU finished the submitted work.
    pub fn wait(&self) {
        let _ = self
            .context
            .device
            .poll(wgpu::PollType::wait_indefinitely());
    }
}

/// Preference order of adapters, lowest first: discrete over integrated over
/// other GPUs over software rasterisers; Vulkan, Metal and DirectX 12 over OpenGL.
/// A filterable 2D texture binding of the material layout.
fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

/// Device features the renderer uses where the adapter has them: indirect
/// draws with a first instance (batches, stage 67) and, for `timing`,
/// timestamps of passes (the profiler).
pub fn optional_features(timing: bool) -> wgpu::Features {
    let mut features = wgpu::Features::INDIRECT_FIRST_INSTANCE;
    if timing {
        features |= wgpu::Features::TIMESTAMP_QUERY;
    }
    features
}

pub fn adapter_rank(adapter: &wgpu::Adapter) -> (u8, bool) {
    let info = adapter.get_info();
    let kind = match info.device_type {
        wgpu::DeviceType::DiscreteGpu => 0,
        wgpu::DeviceType::IntegratedGpu => 1,
        wgpu::DeviceType::VirtualGpu | wgpu::DeviceType::Other => 2,
        wgpu::DeviceType::Cpu => 3,
    };
    (kind, info.backend == wgpu::Backend::Gl)
}

fn grow_buffer<'a>(
    device: &wgpu::Device,
    slot: &'a mut Option<(wgpu::Buffer, u64)>,
    size: u64,
    label: &str,
    usage: wgpu::BufferUsages,
) -> &'a wgpu::Buffer {
    if slot.as_ref().is_none_or(|(_, capacity)| *capacity < size) {
        let capacity = size.next_power_of_two().max(4096);
        *slot = Some((
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: capacity,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            capacity,
        ));
    }
    &slot.as_ref().unwrap().0
}

/// The six clip planes `(normal, distance)` of a view-projection matrix with depth 0..1.
/// Right, up and forward of a view matrix (column-major, camera looking
/// down −z) in world space.
fn camera_axes(view: &Mat4) -> [[f32; 4]; 3] {
    let row = |r: usize| [view[r], view[4 + r], view[8 + r], 0.0];
    let back = row(2);
    [row(0), row(1), [-back[0], -back[1], -back[2], 0.0]]
}

fn frustum_planes(m: &Mat4) -> [[f32; 4]; 6] {
    let row = |r: usize| [m[r], m[4 + r], m[8 + r], m[12 + r]];
    let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
    let add = |a: [f32; 4], b: [f32; 4]| [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]];
    let sub = |a: [f32; 4], b: [f32; 4]| [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]];
    [
        add(r3, r0),
        sub(r3, r0),
        add(r3, r1),
        sub(r3, r1),
        r2,
        sub(r3, r2),
    ]
}

fn sphere_visible(planes: &[[f32; 4]; 6], centre: [f32; 3], radius: f32) -> bool {
    planes.iter().all(|p| {
        let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
        p[0] * centre[0] + p[1] * centre[1] + p[2] * centre[2] + p[3] >= -radius * len
    })
}

#[cfg(test)]
mod tests {
    use super::super::camera::OrbitCamera;
    use super::super::scene::tests::cube;
    use super::*;
    use crate::render::Batching;

    #[test]
    fn frustum_culls_spheres_behind_the_camera() {
        let camera = OrbitCamera::default();
        let view = camera.view(1.0);
        let planes = frustum_planes(&view.view_projection());
        assert!(sphere_visible(&planes, camera.target, 0.1));
        let behind = math::add(camera.eye(), math::sub(camera.eye(), camera.target));
        assert!(!sphere_visible(&planes, behind, 0.1));
        assert!(sphere_visible(&planes, behind, 100.0));
    }

    /// Renders a cube on the GPU when an adapter exists (lavapipe counts) and
    /// checks the image and both picking paths.
    #[test]
    fn game_look_draws_the_sky_and_lights_like_the_game() {
        use doctor_toolkit::time_of_day::{Fog, Sky};
        let Some(context) = GpuContext::headless() else {
            eprintln!("no wgpu adapter; GPU test skipped");
            return;
        };
        let mut renderer = SceneRenderer::new(context);
        renderer.upload_mesh(7, Arc::new(cube()));
        let mut scene = Scene::single(7);
        scene.options.background = [0.0, 0.0, 0.0, 1.0];
        let mut look = super::super::GameLook::studio();
        look.sky = Some(Sky {
            low: [1.0, 0.0, 0.0],
            high: [0.0, 0.0, 1.0],
            haze: 0.2,
            ..Sky::default()
        });
        scene.options.game = Some(look);
        let mut camera = OrbitCamera::framing(&cube().bounds());
        camera.yaw = 0.3;
        camera.pitch = -0.6; // from below: sky above the cube
        let size = [64, 48];
        let view = camera.view(size[0] as f32 / size[1] as f32);
        let mut viewport = Viewport::new();
        renderer.render(&mut viewport, size, &scene, &view);
        let (w, _, pixels) = renderer.read_pixels(&viewport).unwrap();
        let at = |x: usize, y: usize| &pixels[(y * w as usize + x) * 4..][..3];
        // The top of the view looks up into the blue above the haze.
        let top = at(32, 0);
        assert!(top[2] > 200 && top[0] < 60, "sky above: {top:?}");
        let centre = at(32, 24);
        assert!(centre != top, "the cube covers the sky");

        // Fog: everything past its end takes the far colour.
        look.sky = None;
        look.fog = Some(Fog {
            color: [0.0, 1.0, 0.0],
            far_color: [0.0, 1.0, 0.0],
            start: 0.0,
            end: 0.01,
            near_amount: 1.0,
            far_amount: 1.0,
        });
        scene.options.game = Some(look);
        renderer.render(&mut viewport, size, &scene, &view);
        let (_, _, pixels) = renderer.read_pixels(&viewport).unwrap();
        let c = &pixels[(24 * w as usize + 32) * 4..][..3];
        assert!(c[1] > 240 && c[0] < 10, "fogged cube: {c:?}");
    }

    #[test]
    fn renders_and_picks_on_the_gpu() {
        let Some(context) = GpuContext::headless() else {
            eprintln!("no wgpu adapter; GPU test skipped");
            return;
        };
        eprintln!("adapter: {}", context.adapter_summary());
        let mut renderer = SceneRenderer::new(context);
        let mesh = Arc::new(cube());
        renderer.upload_mesh(7, mesh.clone());
        let mut scene = Scene::single(7);
        scene
            .instances
            .push(Instance::new(7, math::translation([3.0, 0.0, 0.0])));
        scene.options.background = [0.0, 0.0, 0.0, 1.0];
        let mut bounds = mesh.bounds();
        bounds.union(&bounds.transformed(&math::translation([3.0, 0.0, 0.0])));
        let mut camera = OrbitCamera::framing(&bounds);
        camera.yaw = 0.0;
        camera.pitch = 0.6;
        let size = [64, 48];
        let view = camera.view(size[0] as f32 / size[1] as f32);
        let mut viewport = Viewport::new();
        renderer.render(&mut viewport, size, &scene, &view);
        assert_eq!(renderer.stats.instances, 2);
        assert_eq!(renderer.stats.triangles, 24);
        let (w, h, pixels) = renderer.read_pixels(&viewport).unwrap();
        assert_eq!((w, h), (64, 48));
        let lit = pixels.chunks_exact(4).filter(|p| p[0] > 20).count();
        assert!(lit > 100, "{lit} lit pixels");

        // The centre of the left cube's top face, projected to pixels.
        let (uv, _) = view.project([0.0, 0.5, 0.0]).unwrap();
        let pixel = [
            (uv[0] * size[0] as f32) as u32,
            (uv[1] * size[1] as f32) as u32,
        ];
        let gpu = renderer.pick(&mut viewport, &scene, &view, pixel);
        assert_eq!(
            gpu,
            Some(Pick {
                instance: 0,
                part: 0
            })
        );
        let meshes = HashMap::from([(7, mesh.as_ref())]);
        let ray = view.ray([
            (pixel[0] as f32 + 0.5) / size[0] as f32,
            (pixel[1] as f32 + 0.5) / size[1] as f32,
        ]);
        assert_eq!(scene.ray_pick(&ray, &meshes), gpu);
        assert_eq!(renderer.pick(&mut viewport, &scene, &view, [0, 0]), None);

        // Hidden parts are skipped, wireframe and lines draw.
        scene.hidden_parts.insert((7, 0));
        scene.options.wireframe = true;
        scene.options.normals = true;
        scene.overlay_lines = vec![
            LineVertex::new([0.0; 3], [255; 4]),
            LineVertex::new([3.0, 0.0, 0.0], [255; 4]),
        ];
        renderer.render(&mut viewport, size, &scene, &view);
        assert_eq!(renderer.stats.triangles, 20);
        // Sides, wireframe, normals and the overlay line.
        assert_eq!(renderer.stats.draw_calls, 4);
        assert_ne!(
            renderer.pick(&mut viewport, &scene, &view, pixel),
            Some(Pick {
                instance: 0,
                part: 0
            })
        );
    }

    /// A mesh of `n` cubes side by side, one part each, the parts cycling
    /// through `materials`.
    fn cubes(n: usize, materials: &[MaterialData]) -> MeshData {
        cubes_at(n, materials, 0.0)
    }

    /// [`cubes`] raised by `dy`.
    fn cubes_at(n: usize, materials: &[MaterialData], dy: f32) -> MeshData {
        let one = cube();
        let mut mesh = MeshData::default();
        for i in 0..n {
            let vertices: Vec<Vertex> = one
                .vertices
                .iter()
                .map(|v| Vertex {
                    position: math::add(v.position, [i as f32 * 1.5, dy, 0.0]),
                    ..*v
                })
                .collect();
            mesh.push_part(
                format!("cube {i}"),
                vertices,
                &one.indices,
                materials[i % materials.len()].clone(),
            );
        }
        mesh
    }

    /// Many meshes and instances, textured, untextured and blended parts:
    /// the scene of the batching tests.
    fn batching_scene(renderer: &mut SceneRenderer) -> (Scene, View) {
        let checker: Vec<u8> = (0..16 * 16)
            .flat_map(|i| {
                if (i / 16 + i % 16) % 2 == 0 {
                    [230, 80, 40, 255]
                } else {
                    [40, 90, 220, 255]
                }
            })
            .collect();
        renderer.upload_texture(
            100,
            &TextureData {
                name: "checker".into(),
                width: 16,
                height: 16,
                srgb: true,
                mips: vec![checker],
            },
        );
        let plain = |c: [f32; 4]| MaterialData {
            texture: None,
            color: c,
            surface: None,
            sky: false,
        };
        let materials = [
            plain([0.9, 0.2, 0.2, 1.0]),
            MaterialData {
                texture: Some(100),
                color: [1.0; 4],
                surface: None,
                sky: false,
            },
            plain([0.2, 0.9, 0.3, 1.0]),
            MaterialData {
                texture: None,
                color: [0.3, 0.4, 1.0, 0.5],
                surface: Some(Box::new(super::super::scene::Surface {
                    blend: true,
                    ..Default::default()
                })),
                sky: false,
            },
        ];
        let mut scene = Scene::default();
        for m in 0..12u64 {
            renderer.upload_mesh(
                m + 1,
                Arc::new(cubes(1 + m as usize % 4, &materials[m as usize % 3..])),
            );
            for i in 0..3 {
                let at = [
                    i as f32 * 7.0 - 7.0,
                    m as f32 * 0.9 - 5.0,
                    -(m as f32) * 1.3,
                ];
                scene.instances.push(Instance::new(
                    m + 1,
                    math::mul(&math::translation(at), &math::rotation_y(m as f32 * 0.3)),
                ));
            }
        }
        scene.options.background = [0.0, 0.0, 0.0, 1.0];
        let mut bounds = Aabb::EMPTY;
        for instance in &scene.instances {
            let mesh = renderer.mesh_data(instance.mesh).unwrap();
            bounds.union(&mesh.bounds().transformed(&instance.transform));
        }
        let mut camera = OrbitCamera::framing(&bounds);
        camera.yaw = 0.4;
        camera.pitch = 0.3;
        (scene, camera.view(4.0 / 3.0))
    }

    /// Batched draws (shared buffers, shared materials, indirect runs, even
    /// sorted by material) give the picture of part-by-part drawing.
    #[test]
    fn batched_draws_render_the_same_picture() {
        let Some(context) = GpuContext::headless() else {
            eprintln!("no wgpu adapter; GPU test skipped");
            return;
        };
        let mut renderer = SceneRenderer::new(context);
        renderer.set_mesh_budget(64);
        let (scene, view) = batching_scene(&mut renderer);
        let size = [160, 120];
        let mut pictures = Vec::new();
        for (indirect, sort) in [(false, false), (true, false), (true, true), (false, true)] {
            renderer.set_batching(Batching { indirect, sort });
            let mut viewport = Viewport::new();
            renderer.render(&mut viewport, size, &scene, &view);
            let stats = renderer.stats;
            let (_, _, pixels) = renderer.read_pixels(&viewport).unwrap();
            // Indirect calls only where the device has them.
            let indirect = renderer.batching().indirect;
            pictures.push((indirect, sort, stats, pixels));
        }
        let (_, _, first, reference) = &pictures[0];
        assert_eq!(first.parts, first.draw_calls, "one call per part");
        assert!(first.parts > 20, "{} parts", first.parts);
        let lit = reference
            .chunks_exact(4)
            .filter(|p| p[..3] != [0, 0, 0])
            .count();
        assert!(lit > 1000, "{lit} lit pixels");
        for (indirect, sort, stats, pixels) in &pictures[1..] {
            assert_eq!(stats.parts, first.parts);
            assert_eq!(stats.triangles, first.triangles);
            let differ = pixels
                .chunks_exact(4)
                .zip(reference.chunks_exact(4))
                .filter(|(a, b)| a != b)
                .count();
            assert_eq!(differ, 0, "indirect {indirect}, sorted {sort}");
            if *sort && *indirect {
                assert!(stats.draw_calls < first.draw_calls / 2, "{stats:?}");
            }
        }
        // The id pass agrees in every mode, on lit pixels.
        let targets: Vec<[u32; 2]> = reference
            .chunks_exact(4)
            .enumerate()
            .filter(|(_, p)| p[..3] != [0, 0, 0])
            .step_by(97)
            .take(12)
            .map(|(i, _)| [i as u32 % size[0], i as u32 / size[0]])
            .collect();
        let mut viewport = Viewport::new();
        renderer.set_batching(Batching {
            indirect: false,
            sort: false,
        });
        renderer.render(&mut viewport, size, &scene, &view);
        let picks: Vec<_> = targets
            .iter()
            .map(|&pixel| renderer.pick(&mut viewport, &scene, &view, pixel))
            .collect();
        assert!(
            picks.iter().filter(|p| p.is_some()).count() > 3,
            "{picks:?}"
        );
        renderer.set_batching(Batching {
            indirect: true,
            sort: false,
        });
        let again: Vec<_> = targets
            .iter()
            .map(|&pixel| renderer.pick(&mut viewport, &scene, &view, pixel))
            .collect();
        assert_eq!(picks, again);
    }

    /// Meshes share the arena and the bind groups of equal materials; a
    /// mesh of the same shape is written in place; evicted meshes give
    /// their room back.
    #[test]
    fn meshes_share_buffers_and_materials() {
        let Some(context) = GpuContext::headless() else {
            eprintln!("no wgpu adapter; GPU test skipped");
            return;
        };
        let mut renderer = SceneRenderer::new(context);
        renderer.set_mesh_budget(8);
        let red = MaterialData {
            texture: None,
            color: [1.0, 0.0, 0.0, 1.0],
            surface: None,
            sky: false,
        };
        for key in 1..=4 {
            renderer.upload_mesh(key, Arc::new(cubes(2, std::slice::from_ref(&red))));
        }
        assert_eq!(renderer.material_count(), 1);
        assert_eq!(
            renderer.mesh_slot(1).unwrap().page,
            renderer.mesh_slot(4).unwrap().page
        );
        let bytes = renderer.mesh_bytes();
        // The same shape again (a posed copy): same room.
        let slot = renderer.mesh_slot(2);
        let moved = cubes_at(2, std::slice::from_ref(&red), 1.0);
        renderer.upload_mesh(2, Arc::new(moved));
        assert_eq!(renderer.mesh_slot(2), slot);
        assert_eq!(renderer.mesh_data(2).unwrap().bounds().min[1], 0.5);
        // Another shape: new room, the old one is free again.
        renderer.upload_mesh(2, Arc::new(cubes(3, std::slice::from_ref(&red))));
        assert_ne!(renderer.mesh_slot(2), slot);
        // Over the budget the least recently drawn go, with their room.
        for key in 10..30 {
            renderer.upload_mesh(key, Arc::new(cubes(1, std::slice::from_ref(&red))));
        }
        assert_eq!(renderer.mesh_count(), 8);
        assert_eq!(renderer.mesh_bytes(), bytes, "no new page");
        let green = MaterialData {
            color: [0.0, 1.0, 0.0, 1.0],
            ..red.clone()
        };
        renderer.upload_mesh(40, Arc::new(cubes(2, &[red, green])));
        assert_eq!(renderer.material_count(), 2);
    }
}
