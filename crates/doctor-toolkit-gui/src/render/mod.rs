//! A reusable scene renderer on wgpu.
//!
//! The model viewer uses it for one mesh; the level view is meant to use it for
//! thousands of placed objects. The pieces:
//!
//! - [`MeshData`] / [`TextureData`] — CPU data, uploaded once with
//!   [`SceneRenderer::upload_mesh`] / [`SceneRenderer::upload_texture`] under
//!   caller-chosen keys and cached (least recently drawn meshes are evicted).
//! - [`Scene`] — instances (mesh key + transform + tint), part visibility,
//!   highlight, extra lines and render switches. Instances of one mesh are
//!   drawn with one instanced draw call per part, after frustum culling.
//! - [`Viewport`] — offscreen colour (with 4× MSAA where supported) and depth
//!   targets, registered with egui as a native texture and shown as an image.
//!   The renderer draws into it from the UI thread only when something changed.
//! - [`SceneRenderer::pick`] — an id pass restricted to one pixel answers
//!   which instance and part are under the cursor; [`Scene::ray_pick`] does the
//!   same on the CPU from the retained mesh data.
//! - [`OrbitCamera`] / [`View`] — camera input, projection and picking rays.
//!
//! Offscreen rendering rather than drawing inside egui's own render pass keeps
//! depth, MSAA and the id pass independent of the window surface and lets an
//! unchanged view cost nothing per frame.

mod batch;
mod camera;
mod game_look;
mod gpu;
pub mod math;
mod scene;
mod timing;
pub mod tree;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

#[allow(unused_imports)]
pub use batch::Batching;
pub use camera::{OrbitCamera, View};
pub use game_look::GameLook;
pub use gpu::{GpuContext, RenderStats, SceneRenderer, Viewport, adapter_rank, optional_features};
// The whole scene API is public for the level view, even parts the model viewer does not use.
#[allow(unused_imports)]
pub use scene::{
    Environment, Instance, LineVertex, MaterialData, MeshData, MeshKey, Pick, Scene, SceneOptions,
    Surface, SurfaceModel, TextureData, TextureKey, Vertex,
};

static GAME_LOOK: AtomicBool = AtomicBool::new(true);

/// Whether the views shade "as in the game" (Preferences and the level
/// view's lighting menu; `Settings::view`).
pub fn game_look() -> bool {
    GAME_LOOK.load(Ordering::Relaxed)
}

pub fn set_game_look(on: bool) {
    GAME_LOOK.store(on, Ordering::Relaxed);
}

/// The renderer shared by every view of the application.
#[derive(Clone)]
pub struct Gpu {
    renderer: Arc<Mutex<SceneRenderer>>,
    summary: Arc<str>,
}

impl Gpu {
    /// Uses eframe's wgpu device; `None` when eframe runs on another backend.
    pub fn from_creation_context(cc: &eframe::CreationContext<'_>) -> Option<Self> {
        let state = cc.wgpu_render_state.as_ref()?;
        Some(Self::new(GpuContext::from_egui(state)))
    }

    pub fn new(context: GpuContext) -> Self {
        let summary = context.adapter_summary().into();
        Self {
            renderer: Arc::new(Mutex::new(SceneRenderer::new(context))),
            summary,
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, SceneRenderer> {
        self.renderer.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The device of the renderer, for another renderer on it (the previews
    /// of the Content Browser draw on a worker thread).
    pub fn context(&self) -> GpuContext {
        self.lock().context().clone()
    }

    /// Adapter name and backend.
    pub fn summary(&self) -> &str {
        &self.summary
    }
}
