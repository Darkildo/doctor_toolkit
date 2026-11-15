//! Colour grading of the game look (stage 70, `docs/analysis/renderer.md`
//! §10.9): the game draws the finished frame through a blend of 32³ colour
//! tables (`lutcolor_*` volume textures) chosen by the time of day. The game
//! look draws its scene into a frame of its own; this pass grades it into the
//! viewport.

use std::collections::HashMap;
use std::num::NonZeroU64;

use eframe::wgpu;
use eframe::wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    weights: [f32; 4],
}

pub struct Post {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    params: wgpu::Buffer,
    /// Uploaded colour tables by key.
    tables: HashMap<u32, wgpu::TextureView>,
    /// A table that changes nothing, for a missing table.
    identity: wgpu::TextureView,
}

fn volume(device: &wgpu::Device, queue: &wgpu::Queue, size: u32, rgba: &[u8]) -> wgpu::TextureView {
    device
        .create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("colour table"),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: size,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            rgba,
        )
        .create_view(&Default::default())
}

impl Post {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("colour grading"),
            source: wgpu::ShaderSource::Wgsl(include_str!("post.wgsl").into()),
        });
        let texture = |binding, view_dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("colour grading layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(16),
                    },
                    count: None,
                },
                texture(1, wgpu::TextureViewDimension::D2),
                texture(2, wgpu::TextureViewDimension::D3),
                texture(3, wgpu::TextureViewDimension::D3),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("colour grading"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("colour grading"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_post"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_post"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("colour table sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("colour grading"),
            contents: bytemuck::bytes_of(&Params { weights: [0.0; 4] }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        // 2³ corners: the identity (trilinear).
        let identity: Vec<u8> = (0..8u8)
            .flat_map(|i| {
                let c = |bit: u8| if i & bit != 0 { 255 } else { 0 };
                [c(1), c(2), c(4), 255]
            })
            .collect();
        let identity = volume(device, queue, 2, &identity);
        Self {
            pipeline,
            layout,
            sampler,
            params,
            tables: HashMap::new(),
            identity,
        }
    }

    /// The view of a colour table, uploaded on first use from the registered
    /// tables ([`super::register_tables`]).
    fn table(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: u32,
    ) -> Option<&wgpu::TextureView> {
        if let std::collections::hash_map::Entry::Vacant(entry) = self.tables.entry(key) {
            let table = super::game_look::table(key)?;
            let size = table.size as u32;
            if table.rgba.len() != (size * size * size * 4) as usize {
                return None;
            }
            entry.insert(volume(device, queue, size, &table.rgba));
        }
        self.tables.get(&key)
    }

    /// Grades `source` (the finished frame, gamma-encoded) into `target`.
    pub fn run(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView,
        target: &wgpu::TextureView,
        grading: &[(u32, f32)],
    ) {
        let mut views = [self.identity.clone(), self.identity.clone()];
        let mut weights = [0.0f32; 2];
        let mut total = 0.0;
        for (i, &(key, weight)) in grading.iter().take(2).enumerate() {
            if let Some(view) = self.table(device, queue, key) {
                views[i] = view.clone();
                weights[i] = weight;
                total += weight;
            }
        }
        // The game normalises the weights; its strength is the largest weight
        // of a table (1 in the cities).
        let strength = if total > 1e-4 { 1.0 } else { 0.0 };
        let norm = if total > 1e-4 { 1.0 / total } else { 0.0 };
        queue.write_buffer(
            &self.params,
            0,
            bytemuck::bytes_of(&Params {
                weights: [strength, weights[0] * norm, weights[1] * norm, 0.0],
            }),
        );
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("colour grading"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&views[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("colour grading"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            timestamp_writes: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
}
