//! Fallback model preview without a GPU renderer: a small software rasteriser with a depth buffer, textures
//! sampled per pixel and an orbit camera (drag to rotate, scroll to zoom).

use std::collections::HashMap;

use doctor_toolkit::export::gltf::AssetSource;
use doctor_toolkit::game::StreamFile;
use doctor_toolkit::resource::material::TextureRole;
use doctor_toolkit::resource::mesh::{self, Model, TYPE_MESH};
use doctor_toolkit_mods::i18n::{tr, trf, trn};

const SIZE: usize = 480;

/// A decoded texture for sampling.
struct Image {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

struct Triangle {
    positions: [[f32; 3]; 3],
    uvs: [[f32; 2]; 3],
    texture: Option<usize>,
}

pub struct ModelPreview {
    name: String,
    error: Option<String>,
    triangles: Vec<Triangle>,
    images: Vec<Image>,
    center: [f32; 3],
    radius: f32,
    yaw: f32,
    pitch: f32,
    zoom: f32,
    textured: bool,
    handle: Option<egui::TextureHandle>,
    /// Parts, triangles and textured materials.
    stats: [usize; 3],
}

impl ModelPreview {
    /// Loads mesh resource `index`, or returns `None` if it is not a mesh.
    pub fn load(stream: &StreamFile, index: usize, assets: &mut dyn AssetSource) -> Option<Self> {
        if stream.layout().resources[index].header.type_id != TYPE_MESH {
            return None;
        }
        let mut preview = Self {
            name: String::new(),
            error: None,
            triangles: Vec::new(),
            images: Vec::new(),
            center: [0.0; 3],
            radius: 1.0,
            yaw: 0.6,
            pitch: 0.35,
            zoom: 1.0,
            textured: true,
            handle: None,
            stats: [0; 3],
        };
        if let Err(error) = mesh::load(stream, index).and_then(|m| preview.fill(&m, assets)) {
            preview.error = Some(error.to_string());
        }
        Some(preview)
    }

    /// A preview of a model that is not in a stream (an import).
    pub fn from_model(model: &Model, assets: &mut dyn AssetSource) -> Self {
        let mut preview = Self {
            name: String::new(),
            error: None,
            triangles: Vec::new(),
            images: Vec::new(),
            center: [0.0; 3],
            radius: 1.0,
            yaw: 0.6,
            pitch: 0.35,
            zoom: 1.0,
            textured: true,
            handle: None,
            stats: [0; 3],
        };
        if let Err(error) = preview.fill(model, assets) {
            preview.error = Some(error.to_string());
        }
        preview
    }

    fn fill(&mut self, model: &Model, assets: &mut dyn AssetSource) -> doctor_toolkit::Result<()> {
        self.name = model.name.clone();
        let mut texture_slots: HashMap<u32, Option<usize>> = HashMap::new();
        for record in &model.records {
            let vertices = model.vertices(record)?;
            let triangles = model.triangles(record)?;
            let texture = *texture_slots
                .entry(record.material_hash)
                .or_insert_with(|| {
                    let hash = assets
                        .material(record.material_hash)?
                        .texture(TextureRole::Diffuse)?;
                    let (width, height, rgba) = assets.texture(hash)?.rgba(0).ok()?;
                    self.images.push(Image {
                        width,
                        height,
                        rgba,
                    });
                    Some(self.images.len() - 1)
                });
            for t in triangles {
                let p = t.map(|i| vertices.positions[i as usize]);
                let uv = match &vertices.uv0 {
                    Some(uv) => t.map(|i| uv[i as usize]),
                    None => [[0.0; 2]; 3],
                };
                self.triangles.push(Triangle {
                    positions: p,
                    uvs: uv,
                    texture: if vertices.uv0.is_some() {
                        texture
                    } else {
                        None
                    },
                });
            }
        }
        self.center = [0, 1, 2].map(|i| (model.aabb_min[i] + model.aabb_max[i]) / 2.0);
        self.radius = (0..3)
            .map(|i| (model.aabb_max[i] - model.aabb_min[i]).powi(2))
            .sum::<f32>()
            .sqrt()
            .max(1e-3)
            / 2.0;
        self.stats = [model.records.len(), self.triangles.len(), self.images.len()];
        Ok(())
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        if let Some(error) = &self.error {
            ui.colored_label(
                egui::Color32::RED,
                trf("model.cannot_load", &[("error", error)]),
            );
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.strong(&self.name);
            let [parts, triangles, materials] = self.stats.map(|n| n as u64);
            let stats = trf(
                "model.summary",
                &[
                    ("parts", &trn("count.parts", parts, &[])),
                    ("triangles", &trn("count.triangles", triangles, &[])),
                    (
                        "materials",
                        &trn("model.textured_materials", materials, &[]),
                    ),
                ],
            );
            ui.label(egui::RichText::new(stats).weak());
            if ui
                .checkbox(&mut self.textured, tr("model.textures"))
                .changed()
            {
                self.handle = None;
            }
        });
        if self.handle.is_none() {
            let image = self.render();
            self.handle = Some(ui.ctx().load_texture(
                "model-preview",
                image,
                egui::TextureOptions::LINEAR,
            ));
        }
        let Some(handle) = &self.handle else { return };
        let side = ui.available_width().clamp(160.0, SIZE as f32);
        let response = ui.add(
            egui::Image::from_texture(handle)
                .fit_to_exact_size(egui::vec2(side, side))
                .sense(egui::Sense::drag()),
        );
        let mut changed = false;
        if response.dragged() {
            let delta = response.drag_delta();
            self.yaw += delta.x * 0.01;
            self.pitch = (self.pitch + delta.y * 0.01).clamp(-1.5, 1.5);
            changed = true;
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.zoom = (self.zoom * (1.0 + scroll * 0.002)).clamp(0.2, 20.0);
                changed = true;
                ui.ctx()
                    .input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
            }
        }
        if changed {
            self.handle = None;
            ui.ctx().request_repaint();
        }
        ui.label(egui::RichText::new(tr("model.software_mouse_hint")).weak());
    }

    fn render(&self) -> egui::ColorImage {
        let mut color = vec![egui::Color32::from_gray(38); SIZE * SIZE];
        let mut depth = vec![f32::NEG_INFINITY; SIZE * SIZE];
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let scale = SIZE as f32 * 0.45 * self.zoom / self.radius;
        let half = SIZE as f32 / 2.0;
        let project = |p: [f32; 3]| -> [f32; 3] {
            let (x, y, z) = (
                p[0] - self.center[0],
                p[1] - self.center[1],
                p[2] - self.center[2],
            );
            let (x, z) = (x * cy + z * sy, -x * sy + z * cy);
            let (y, z) = (y * cp - z * sp, y * sp + z * cp);
            [half + x * scale, half - y * scale, z]
        };
        let light = normalize([0.35, 0.8, 0.5]);
        for tri in &self.triangles {
            let v = tri.positions.map(project);
            let e1 = [v[1][0] - v[0][0], v[1][1] - v[0][1]];
            let e2 = [v[2][0] - v[0][0], v[2][1] - v[0][1]];
            let area = e1[0] * e2[1] - e1[1] * e2[0];
            if area.abs() < 1e-6 {
                continue;
            }
            // World-space face normal for lighting; both sides are lit.
            let p = tri.positions;
            let n = normalize(cross(sub(p[1], p[0]), sub(p[2], p[0])));
            let shade = (n[0] * light[0] + n[1] * light[1] + n[2] * light[2])
                .abs()
                .max(0.25);
            let texture = if self.textured {
                tri.texture.map(|i| &self.images[i])
            } else {
                None
            };

            let min_x = v
                .iter()
                .map(|q| q[0])
                .fold(f32::INFINITY, f32::min)
                .floor()
                .max(0.0) as usize;
            let max_x = v
                .iter()
                .map(|q| q[0])
                .fold(f32::NEG_INFINITY, f32::max)
                .ceil()
                .min(SIZE as f32 - 1.0);
            let min_y = v
                .iter()
                .map(|q| q[1])
                .fold(f32::INFINITY, f32::min)
                .floor()
                .max(0.0) as usize;
            let max_y = v
                .iter()
                .map(|q| q[1])
                .fold(f32::NEG_INFINITY, f32::max)
                .ceil()
                .min(SIZE as f32 - 1.0);
            if max_x < 0.0 || max_y < 0.0 {
                continue;
            }
            for y in min_y..=max_y as usize {
                for x in min_x..=max_x as usize {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let w1 = ((px - v[0][0]) * e2[1] - (py - v[0][1]) * e2[0]) / area;
                    let w2 = (e1[0] * (py - v[0][1]) - e1[1] * (px - v[0][0])) / area;
                    let w0 = 1.0 - w1 - w2;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let z = w0 * v[0][2] + w1 * v[1][2] + w2 * v[2][2];
                    let slot = y * SIZE + x;
                    if z <= depth[slot] {
                        continue;
                    }
                    let base = match texture {
                        Some(image) => {
                            let u = w0 * tri.uvs[0][0] + w1 * tri.uvs[1][0] + w2 * tri.uvs[2][0];
                            let t = w0 * tri.uvs[0][1] + w1 * tri.uvs[1][1] + w2 * tri.uvs[2][1];
                            let tx =
                                ((u.rem_euclid(1.0)) * image.width as f32) as usize % image.width;
                            let ty =
                                ((t.rem_euclid(1.0)) * image.height as f32) as usize % image.height;
                            let i = (ty * image.width + tx) * 4;
                            if image.rgba[i + 3] < 128 {
                                continue; // alpha-tested foliage, fences and decals
                            }
                            [image.rgba[i], image.rgba[i + 1], image.rgba[i + 2]]
                        }
                        None => [190, 190, 190],
                    };
                    depth[slot] = z;
                    let c = base.map(|b| (b as f32 * shade) as u8);
                    color[slot] = egui::Color32::from_rgb(c[0], c[1], c[2]);
                }
            }
        }
        egui::ColorImage::new([SIZE, SIZE], color)
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-12 {
        v.map(|x| x / len)
    } else {
        [0.0, 1.0, 0.0]
    }
}
