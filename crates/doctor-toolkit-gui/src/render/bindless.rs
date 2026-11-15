//! The game look on devices with binding arrays (stage 70): every texture
//! in one array, the parameters of every material in one table and, per
//! mesh part, the index of its material, so that one bind group serves the
//! whole frame and neighbouring parts of one buffer go in one indirect
//! multi-draw call whatever their material (`editor-performance.md` §11).
//! Also the resources of the sun's shadow (cascades of a shadow map) and of
//! the lights of lamps (a table of lights and a grid of screen tiles listing
//! the lights that reach them; `docs/analysis/renderer.md` §10.9).
//!
//! The simple look and devices without binding arrays (OpenGL) keep the
//! renderer of stage 69: a bind group per material.

use std::collections::HashMap;
use std::num::{NonZeroU32, NonZeroU64};

use eframe::wgpu;

use super::batch::FreeList;
use super::camera::View;
use super::gpu::MaterialUniform;
use super::math::{self, Mat4, Vec3};
use super::scene::{PointLight, TextureKey};

/// Device features the game look's binding arrays need.
pub const FEATURES: wgpu::Features = wgpu::Features::TEXTURE_BINDING_ARRAY
    .union(wgpu::Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING)
    .union(wgpu::Features::PARTIALLY_BOUND_BINDING_ARRAY);
/// Textures of the array (a city loads about a thousand).
pub const MAX_TEXTURES: u32 = 8192;
/// Side of a cascade of the shadow map, and the cascades.
pub const SHADOW_SIZE: u32 = 2048;
pub const CASCADES: usize = 3;
/// Far ends of the cascades, metres from the eye.
const SPLITS: [f32; CASCADES] = [16.0, 45.0, 100.0];
/// Casters smaller than this (bounding sphere radius, metres) are left out
/// of each cascade: they would cover a few texels of the farther maps.
pub const SMALLEST_CASTER: [f32; CASCADES] = [0.0, 0.4, 1.2];
/// How far behind a cascade (towards the sun) casters still count.
const CASTER_REACH: f32 = 250.0;
/// Lights drawn at most, and per tile of the screen.
pub const MAX_LIGHTS: usize = 1024;
const LIGHTS_PER_TILE: usize = 48;
const TILE: u32 = 32;

/// Whether the device can draw the game look with binding arrays.
pub fn supported(device: &wgpu::Device) -> bool {
    let limits = device.limits();
    device.features().contains(FEATURES)
        && limits.max_binding_array_elements_per_shader_stage >= MAX_TEXTURES + 2
        && limits.max_storage_buffers_per_shader_stage >= 4
        && limits.max_texture_array_layers >= CASCADES as u32
        && limits.max_texture_dimension_2d >= SHADOW_SIZE
}

/// The limits to ask of a device for [`FEATURES`]: `limits` with room for
/// the texture array where the adapter has it.
pub fn limits(adapter: &wgpu::Adapter, limits: wgpu::Limits) -> wgpu::Limits {
    let available = adapter.limits();
    if !adapter.features().contains(FEATURES) {
        return limits;
    }
    wgpu::Limits {
        max_binding_array_elements_per_shader_stage: available
            .max_binding_array_elements_per_shader_stage
            .min(MAX_TEXTURES * 2)
            .max(limits.max_binding_array_elements_per_shader_stage),
        ..limits
    }
}

/// A material of the table: its uniform and the slots of its six textures.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GameMaterial {
    pub material: MaterialUniform,
    pub textures: [[u32; 4]; 2],
}

/// A light as the shader reads it (`Light` in `scene.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct LightRaw {
    position: [f32; 4],
    color: [f32; 4],
    direction: [f32; 4],
    params: [f32; 4],
}

/// The view-projection of a cascade, padded to the uniform offset alignment.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ShadowPassRaw {
    view_proj: Mat4,
    _pad: [f32; 48],
}

const SHADOW_PASS_STRIDE: u64 = std::mem::size_of::<ShadowPassRaw>() as u64;

/// A storage buffer grown by doubling, with a copy of its contents.
struct Table<T: bytemuck::Pod> {
    data: Vec<T>,
    buffer: wgpu::Buffer,
    label: &'static str,
    /// Written since the last upload: `[first, end)`.
    dirty: Option<(usize, usize)>,
    /// The buffer was made anew: bind groups naming it are stale.
    regrown: bool,
}

impl<T: bytemuck::Pod> Table<T> {
    fn new(device: &wgpu::Device, label: &'static str, capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
            buffer: storage(device, label, (capacity * std::mem::size_of::<T>()) as u64),
            label,
            dirty: None,
            regrown: false,
        }
    }

    fn set(&mut self, index: usize, value: T) {
        if index >= self.data.len() {
            self.data.resize(index + 1, T::zeroed());
        }
        self.data[index] = value;
        let (a, b) = self.dirty.unwrap_or((index, index + 1));
        self.dirty = Some((a.min(index), b.max(index + 1)));
    }

    /// Sends what changed; returns whether the buffer was made anew.
    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        let size = std::mem::size_of::<T>();
        let needed = (self.data.len().max(1) * size) as u64;
        if needed > self.buffer.size() {
            let capacity = needed.next_power_of_two().max(4096);
            self.buffer = storage(device, self.label, capacity);
            self.dirty = Some((0, self.data.len()));
            self.regrown = true;
        }
        if let Some((a, b)) = self.dirty.take()
            && b > a
        {
            queue.write_buffer(
                &self.buffer,
                (a * size) as u64,
                bytemuck::cast_slice(&self.data[a..b]),
            );
        }
        std::mem::take(&mut self.regrown)
    }
}

fn storage(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(16).next_multiple_of(16),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// The shared resources of the game look on devices with binding arrays.
pub struct Bindless {
    /// Group 0: globals, shadow map and its sampler, lights, light grid.
    pub globals_layout: wgpu::BindGroupLayout,
    /// Group 1: material table, part table, sampler, texture array.
    pub material_layout: wgpu::BindGroupLayout,
    /// Group 2 of the shadow pass: the cascade's view-projection.
    pub shadow_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    views: Vec<Option<wgpu::TextureView>>,
    free_textures: Vec<u32>,
    slot_of: HashMap<TextureKey, u32>,
    materials: Table<GameMaterial>,
    free_materials: Vec<u32>,
    parts: Table<u32>,
    free_parts: FreeList,
    material_group: Option<wgpu::BindGroup>,
    textures_changed: bool,
    lights: Table<LightRaw>,
    grid: Table<u32>,
    globals_group: Option<wgpu::BindGroup>,
    shadow_texture: wgpu::Texture,
    /// One view per cascade, to draw into.
    pub shadow_layers: Vec<wgpu::TextureView>,
    shadow_view: wgpu::TextureView,
    shadow_sampler: wgpu::Sampler,
    shadow_passes: wgpu::Buffer,
    pub shadow_group: wgpu::BindGroup,
}

/// Slots of the white texture and the flat normal.
pub const WHITE: u32 = 0;
pub const FLAT_NORMAL: u32 = 1;

impl Bindless {
    pub fn new(
        device: &wgpu::Device,
        sampler: wgpu::Sampler,
        white: wgpu::TextureView,
        flat_normal: wgpu::TextureView,
        globals_size: u64,
    ) -> Self {
        let storage_entry = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let both = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("game globals layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: both,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(globals_size),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: fragment,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: fragment,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                storage_entry(3, fragment),
                storage_entry(4, fragment),
            ],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("game material layout"),
            entries: &[
                storage_entry(0, fragment),
                storage_entry(1, both),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: fragment,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: fragment,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: NonZeroU32::new(MAX_TEXTURES),
                },
            ],
        });
        let shadow_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow pass layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: NonZeroU64::new(64),
                },
                count: None,
            }],
        });
        let shadow_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow map"),
            size: wgpu::Extent3d {
                width: SHADOW_SIZE,
                height: SHADOW_SIZE,
                depth_or_array_layers: CASCADES as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let shadow_layers = (0..CASCADES as u32)
            .map(|layer| {
                shadow_texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("shadow cascade"),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let shadow_view = shadow_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("shadow map"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let shadow_passes = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadow passes"),
            size: SHADOW_PASS_STRIDE * CASCADES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow pass"),
            layout: &shadow_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &shadow_passes,
                    offset: 0,
                    size: NonZeroU64::new(64),
                }),
            }],
        });
        Self {
            globals_layout,
            material_layout,
            shadow_layout,
            sampler,
            views: vec![Some(white), Some(flat_normal)],
            free_textures: Vec::new(),
            slot_of: HashMap::new(),
            materials: Table::new(device, "material table", 1024),
            free_materials: Vec::new(),
            parts: Table::new(device, "part table", 16 * 1024),
            free_parts: FreeList::new(0),
            material_group: None,
            textures_changed: true,
            lights: Table::new(device, "lights", 64),
            grid: Table::new(device, "light grid", 4096),
            globals_group: None,
            shadow_texture,
            shadow_layers,
            shadow_view,
            shadow_sampler,
            shadow_passes,
            shadow_group,
        }
    }

    /// The slot of an uploaded texture.
    pub fn texture_slot(&self, key: TextureKey) -> Option<u32> {
        self.slot_of.get(&key).copied()
    }

    /// Puts a texture in the array (in its old slot when the key is uploaded
    /// again); `None` when the array is full.
    pub fn add_texture(&mut self, key: TextureKey, view: wgpu::TextureView) -> Option<u32> {
        let slot = match self.slot_of.get(&key) {
            Some(&slot) => slot,
            None => {
                let slot = match self.free_textures.pop() {
                    Some(slot) => slot,
                    None if (self.views.len() as u32) < MAX_TEXTURES => {
                        self.views.push(None);
                        self.views.len() as u32 - 1
                    }
                    None => return None,
                };
                self.slot_of.insert(key, slot);
                slot
            }
        };
        self.views[slot as usize] = Some(view);
        self.textures_changed = true;
        Some(slot)
    }

    pub fn remove_texture(&mut self, key: TextureKey) {
        if let Some(slot) = self.slot_of.remove(&key) {
            self.views[slot as usize] = None;
            self.free_textures.push(slot);
            self.textures_changed = true;
        }
    }

    /// A slot of the material table holding `material`.
    pub fn add_material(&mut self, material: GameMaterial) -> u32 {
        let slot = self
            .free_materials
            .pop()
            .unwrap_or(self.materials.data.len() as u32);
        self.materials.set(slot as usize, material);
        slot
    }

    pub fn free_material(&mut self, slot: u32) {
        self.free_materials.push(slot);
    }

    /// Room in the part table for the material slots of a mesh's parts; the
    /// offset the mesh's instances carry.
    pub fn add_parts(&mut self, slots: &[u32]) -> u32 {
        let len = slots.len().max(1) as u32;
        let start = match self.free_parts.alloc(len) {
            Some(start) => start,
            None => {
                let capacity = (self.free_parts.capacity() + len)
                    .next_power_of_two()
                    .max(4096);
                self.free_parts.grow(capacity);
                self.free_parts.alloc(len).expect("grown to fit")
            }
        };
        for (i, &slot) in slots.iter().enumerate() {
            self.parts.set(start as usize + i, slot);
        }
        start
    }

    pub fn free_parts(&mut self, start: u32, len: usize) {
        self.free_parts.free(start, len.max(1) as u32);
    }

    /// Sends the tables and makes the bind group of group 1 anew if needed.
    pub fn material_group(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> &wgpu::BindGroup {
        let regrown = self.materials.upload(device, queue) | self.parts.upload(device, queue);
        if regrown || self.textures_changed || self.material_group.is_none() {
            self.textures_changed = false;
            let white = self.views[WHITE as usize].clone().expect("white");
            let views: Vec<&wgpu::TextureView> = self
                .views
                .iter()
                .map(|v| v.as_ref().unwrap_or(&white))
                .collect();
            self.material_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("game materials"),
                layout: &self.material_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.materials.buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.parts.buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureViewArray(&views),
                    },
                ],
            }));
        }
        self.material_group.as_ref().expect("made above")
    }

    /// Sends the lights and their grid and the cascades' matrices; makes the
    /// bind group of group 0 anew if needed.
    pub fn frame_group(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        globals: &wgpu::Buffer,
        frame: &Frame,
    ) -> &wgpu::BindGroup {
        self.lights.data.clear();
        self.lights.data.extend_from_slice(&frame.lights);
        if self.lights.data.is_empty() {
            self.lights.data.push(LightRaw::default());
        }
        self.lights.dirty = Some((0, self.lights.data.len()));
        self.grid.data.clear();
        self.grid.data.extend_from_slice(&frame.grid);
        if self.grid.data.is_empty() {
            self.grid.data.extend([0, 0]);
        }
        self.grid.dirty = Some((0, self.grid.data.len()));
        let regrown = self.lights.upload(device, queue) | self.grid.upload(device, queue);
        let passes: Vec<ShadowPassRaw> = frame
            .cascades
            .iter()
            .map(|m| ShadowPassRaw {
                view_proj: *m,
                _pad: [0.0; 48],
            })
            .collect();
        queue.write_buffer(&self.shadow_passes, 0, bytemuck::cast_slice(&passes));
        if regrown || self.globals_group.is_none() {
            self.globals_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("game globals"),
                layout: &self.globals_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: globals.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&self.shadow_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.shadow_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self.lights.buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: self.grid.buffer.as_entire_binding(),
                    },
                ],
            }));
        }
        self.globals_group.as_ref().expect("made above")
    }

    /// The bind groups of the frame: material tables and texture array,
    /// globals with the lights and the shadow map.
    pub fn groups(&self) -> (Option<&wgpu::BindGroup>, Option<&wgpu::BindGroup>) {
        (self.material_group.as_ref(), self.globals_group.as_ref())
    }

    /// Dynamic offset of cascade `c` in the shadow pass buffer.
    pub fn cascade_offset(c: usize) -> u32 {
        (c as u64 * SHADOW_PASS_STRIDE) as u32
    }

    /// Bytes of the shadow map (for the memory figures).
    #[allow(dead_code)]
    pub fn shadow_bytes(&self) -> u64 {
        let s = self.shadow_texture.size();
        s.width as u64 * s.height as u64 * s.depth_or_array_layers as u64 * 4
    }
}

/// What a frame of the game look needs besides its draws.
#[derive(Default)]
pub struct Frame {
    lights: Vec<LightRaw>,
    grid: Vec<u32>,
    /// World to each cascade's map.
    pub cascades: Vec<Mat4>,
    /// Far ends of the cascades and the depth bias.
    pub splits: [f32; 4],
    /// Tile size, tiles across and down, lights per tile.
    pub tiles: [f32; 4],
}

impl Frame {
    /// The sun's cascades for `view` (`sun`: towards the sun) and the lights
    /// of the screen's tiles.
    pub fn new(view: &View, size: [u32; 2], sun: Option<Vec3>, lights: &[PointLight]) -> Self {
        let mut frame = Self::default();
        if let Some(sun) = sun {
            frame.cascades = cascades(view, sun);
            frame.splits = [SPLITS[0], SPLITS[1], SPLITS[2], 0.0015];
        }
        frame.light_grid(view, size, lights);
        frame
    }

    pub fn has_lights(&self) -> bool {
        !self.lights.is_empty()
    }

    fn light_grid(&mut self, view: &View, size: [u32; 2], lights: &[PointLight]) {
        let across = size[0].div_ceil(TILE).max(1);
        let down = size[1].div_ceil(TILE).max(1);
        self.tiles = [
            TILE as f32,
            across as f32,
            down as f32,
            LIGHTS_PER_TILE as f32,
        ];
        if lights.is_empty() {
            return;
        }
        let view_proj = view.view_projection();
        let planes = frustum_planes(&view_proj);
        // The visible lights, nearest first.
        let mut visible: Vec<(f32, &PointLight)> = lights
            .iter()
            .filter(|l| l.radius > 0.0 && l.color.iter().any(|c| *c > 1e-3))
            .filter(|l| sphere_visible(&planes, l.position, l.radius))
            .map(|l| (math::length(math::sub(l.position, view.eye)), l))
            .collect();
        visible.sort_by(|a, b| a.0.total_cmp(&b.0));
        visible.truncate(MAX_LIGHTS);
        let tiles = (across * down) as usize;
        let mut lists: Vec<Vec<u32>> = vec![Vec::new(); tiles];
        let proj = &view.projection;
        for (index, &(distance, l)) in visible.iter().enumerate() {
            let raw = light_raw(l);
            self.lights.push(raw);
            // The light's square on the screen; all of it when the eye is near.
            let (x0, y0, x1, y1) = if distance < l.radius + 0.5 {
                (0, 0, across - 1, down - 1)
            } else {
                let c = math::mul_vec4(
                    &view.view,
                    [l.position[0], l.position[1], l.position[2], 1.0],
                );
                let depth = -c[2];
                if depth - l.radius <= 0.05 {
                    (0, 0, across - 1, down - 1)
                } else {
                    let near = depth - l.radius;
                    let sx = proj[0] / near * l.radius;
                    let sy = proj[5] / near * l.radius;
                    let cx = proj[0] * c[0] / depth;
                    let cy = proj[5] * c[1] / depth;
                    let px = |ndc: f32| ((ndc * 0.5 + 0.5) * size[0] as f32 / TILE as f32).floor();
                    let py = |ndc: f32| ((0.5 - ndc * 0.5) * size[1] as f32 / TILE as f32).floor();
                    let clamp_x = |v: f32| v.clamp(0.0, across as f32 - 1.0) as u32;
                    let clamp_y = |v: f32| v.clamp(0.0, down as f32 - 1.0) as u32;
                    (
                        clamp_x(px(cx - sx)),
                        clamp_y(py(cy + sy)),
                        clamp_x(px(cx + sx)),
                        clamp_y(py(cy - sy)),
                    )
                }
            };
            for ty in y0..=y1 {
                for tx in x0..=x1 {
                    let list = &mut lists[(ty * across + tx) as usize];
                    if list.len() < LIGHTS_PER_TILE {
                        list.push(index as u32);
                    }
                }
            }
        }
        // Headers (first, count) per tile, then the lists.
        self.grid = vec![0; 2 * tiles];
        for (t, list) in lists.iter().enumerate() {
            self.grid[2 * t] = self.grid.len() as u32;
            self.grid[2 * t + 1] = list.len() as u32;
            self.grid.extend_from_slice(list);
        }
    }
}

fn light_raw(l: &PointLight) -> LightRaw {
    let full = l.full_radius.clamp(0.0, l.radius * 0.999);
    let (axis, cos_cone, cos_full) = match l.spot {
        Some((axis, cone, full)) => (axis, cone, full),
        None => ([0.0, -1.0, 0.0], -1.0, -2.0),
    };
    LightRaw {
        position: [l.position[0], l.position[1], l.position[2], l.radius],
        color: [l.color[0], l.color[1], l.color[2], full],
        direction: [axis[0], axis[1], axis[2], cos_full],
        params: [
            cos_cone,
            l.wrap,
            1.0 / (l.radius - full).max(1e-3),
            1.0 / (cos_full - cos_cone).max(1e-3),
        ],
    }
}

/// World to the shadow map of each cascade: an orthographic box around the
/// sphere of the view's slice, looking along the sun, moved in whole texels
/// so that the shadows do not crawl as the camera turns.
fn cascades(view: &View, sun: Vec3) -> Vec<Mat4> {
    let to_sun = math::normalize_or(sun, [0.0, 1.0, 0.0]);
    // Camera axes from the view matrix (column-major, looking down −z).
    let v = &view.view;
    let right = [v[0], v[4], v[8]];
    let up = [v[1], v[5], v[9]];
    let forward = [-v[2], -v[6], -v[10]];
    let (sx, sy) = (1.0 / view.projection[0], 1.0 / view.projection[5]);
    let mut near = 0.1;
    let mut out = Vec::with_capacity(CASCADES);
    for far in SPLITS {
        // The slice's corners and their bounding sphere.
        let mut corners = Vec::with_capacity(8);
        for d in [near, far] {
            for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                let p = math::add(
                    view.eye,
                    math::add(
                        math::scale(forward, d),
                        math::add(math::scale(right, a * sx * d), math::scale(up, b * sy * d)),
                    ),
                );
                corners.push(p);
            }
        }
        let centre = math::scale(
            corners.iter().fold([0.0; 3], |acc, &p| math::add(acc, p)),
            1.0 / 8.0,
        );
        let radius = corners
            .iter()
            .map(|&p| math::length(math::sub(p, centre)))
            .fold(0.0f32, f32::max);
        // Whole metres: the box keeps its size as the camera turns.
        let radius = radius.ceil();
        let world_up = if to_sun[1].abs() > 0.99 {
            [0.0, 0.0, 1.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        let eye = math::add(centre, math::scale(to_sun, CASTER_REACH));
        let light_view = math::look_at(eye, centre, world_up);
        let depth = CASTER_REACH + radius;
        let mut proj = orthographic(radius, depth);
        // Move the box in whole texels of the map.
        let shadow = math::mul(&proj, &light_view);
        let origin = math::mul_vec4(&shadow, [0.0, 0.0, 0.0, 1.0]);
        let texels = SHADOW_SIZE as f32 / 2.0;
        let snap = |c: f32| (c * texels).round() / texels - c;
        proj[12] += snap(origin[0]);
        proj[13] += snap(origin[1]);
        out.push(math::mul(&proj, &light_view));
        near = far;
    }
    out
}

/// Orthographic projection of `±half` across and `0..depth` ahead (depth
/// 0..1), column-major.
fn orthographic(half: f32, depth: f32) -> Mat4 {
    let mut m = [0.0; 16];
    m[0] = 1.0 / half;
    m[5] = 1.0 / half;
    m[10] = -1.0 / depth;
    m[15] = 1.0;
    m
}

/// The six clip planes `(normal, distance)` of a view-projection with depth 0..1.
pub fn frustum_planes(m: &Mat4) -> [[f32; 4]; 6] {
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

pub fn sphere_visible(planes: &[[f32; 4]; 6], centre: Vec3, radius: f32) -> bool {
    planes.iter().all(|p| {
        let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
        p[0] * centre[0] + p[1] * centre[1] + p[2] * centre[2] + p[3] >= -radius * len
    })
}

/// Shadow casters of a cascade: everything in its box or between the box
/// and the sun (the near plane of the box is that far back already).
pub fn caster_planes(cascade: &Mat4) -> [[f32; 4]; 6] {
    frustum_planes(cascade)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::OrbitCamera;

    #[test]
    fn cascades_hold_the_view_and_look_down_the_sun() {
        let camera = OrbitCamera::default();
        let view = camera.view(16.0 / 9.0);
        let sun = math::normalize_or([0.3, 1.0, 0.2], [0.0, 1.0, 0.0]);
        let c = cascades(&view, sun);
        assert_eq!(c.len(), CASCADES);
        // A point a few metres ahead of the eye lies in the first cascade.
        let v = &view.view;
        let forward = [-v[2], -v[6], -v[10]];
        let p = math::add(view.eye, math::scale(forward, 5.0));
        let q = math::mul_vec4(&c[0], [p[0], p[1], p[2], 1.0]);
        assert!(
            q[0].abs() <= 1.0 && q[1].abs() <= 1.0 && (0.0..=1.0).contains(&q[2]),
            "{q:?}"
        );
        // Nearer the sun is nearer in the map.
        let higher = math::add(p, math::scale(sun, 10.0));
        let h = math::mul_vec4(&c[0], [higher[0], higher[1], higher[2], 1.0]);
        assert!(h[2] < q[2]);
    }

    #[test]
    fn lights_land_in_the_tiles_they_reach() {
        let camera = OrbitCamera::default();
        let view = camera.view(1.0);
        let light = PointLight {
            position: camera.target,
            radius: 0.5,
            full_radius: 0.1,
            color: [1.0; 3],
            spot: None,
            wrap: 0.0,
        };
        let frame = Frame::new(&view, [256, 256], None, &[light]);
        assert_eq!(frame.lights.len(), 1);
        // The tile at the centre lists it, a corner does not.
        let across = frame.tiles[1] as usize;
        let at = |x: usize, y: usize| frame.grid[2 * (y * across + x) + 1];
        assert_eq!(at(4, 4), 1);
        assert_eq!(at(0, 0), 0);
        assert!(frame.cascades.is_empty());
    }
}
