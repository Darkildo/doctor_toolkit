//! A small picture of a model of the loaded level, for the Model group of
//! Details (stage 53): its finest meshes, already on the GPU for the view,
//! drawn into a viewport of their own.

use egui::{Color32, Pos2, Rect, Sense, Vec2};

use super::{LevelView, loader, scene};
use crate::render::{Instance, OrbitCamera, Scene, Viewport, math};

/// The viewport of the picture and what it shows.
#[derive(Default)]
pub struct ModelPreview {
    viewport: Option<Viewport>,
    /// Model, size in pixels and meshes uploaded when it was drawn.
    key: Option<(u32, [u32; 2], usize)>,
}

impl LevelView {
    /// Draws model `id` into a box of `size`; false (and nothing drawn)
    /// when it cannot: no GPU, the model is not in the level or none of its
    /// meshes is loaded yet.
    pub fn model_preview(&mut self, ui: &mut egui::Ui, id: u32, size: Vec2) -> bool {
        let (Some(gpu), Some(level)) = (self.gpu.clone(), &self.level) else {
            return false;
        };
        let Some(model) = level.models.get(&id) else {
            return false;
        };
        let mut renderer = gpu.lock();
        let mut scene = Scene {
            options: self.scene.options,
            ..Scene::default()
        };
        let mut bounds = math::Aabb::EMPTY;
        for piece in &model.desc.pieces {
            if piece.lod.is_some_and(|[near, _]| near > 0.0) {
                continue;
            }
            let Some(slot) = level.mesh_slot(piece.mesh) else {
                continue;
            };
            let mesh = &level.meshes[slot];
            if mesh.found.is_none() {
                continue;
            }
            let key = loader::mesh_key(mesh.id, mesh.found);
            let Some(data) = renderer.mesh_data(key) else {
                continue;
            };
            let world = scene::flatten(&piece.matrix);
            bounds.union(&data.bounds().transformed(&world));
            scene.instances.push(Instance::new(key, world));
        }
        for tree in &model.desc.trees {
            let Some(slot) = level.tree_slot(tree.resource) else {
                continue;
            };
            let found = level.trees[slot].found;
            if found.is_none() {
                continue;
            }
            let key = loader::tree_key(tree.resource, found);
            let Some(data) = renderer.mesh_data(key) else {
                continue;
            };
            let world = scene::flatten(&tree.matrix);
            bounds.union(&data.bounds().transformed(&world));
            scene.instances.push(Instance::new(key, world));
        }
        if scene.instances.is_empty() || bounds.is_empty() {
            return false;
        }
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        let ppp = ui.ctx().pixels_per_point();
        let pixels = [
            (rect.width() * ppp).round().max(1.0) as u32,
            (rect.height() * ppp).round().max(1.0) as u32,
        ];
        let viewport = self
            .model_preview
            .viewport
            .get_or_insert_with(Viewport::new);
        let key = (id, pixels, self.uploaded.len());
        if self.model_preview.key != Some(key) {
            let view = OrbitCamera::framing(&bounds).view(rect.aspect_ratio());
            renderer.render(viewport, pixels, &scene, &view);
            self.model_preview.key = Some(key);
        }
        drop(renderer);
        if let Some(texture) = viewport.texture_id() {
            ui.painter().image(
                texture,
                rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        true
    }
}
