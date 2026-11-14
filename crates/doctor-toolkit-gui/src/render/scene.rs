//! CPU-side description of what the renderer draws: mesh data uploaded once,
//! and a scene of instances referring to it.

use std::collections::{HashMap, HashSet};

use super::math::{self, Aabb, Mat4, Ray, Vec3};

/// Caller-chosen key of an uploaded mesh, e.g. a hash of stream and resource.
pub type MeshKey = u64;
/// Caller-chosen key of an uploaded texture, e.g. the texture name hash.
pub type TextureKey = u64;

/// One vertex as the GPU sees it (48 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub color: [u8; 4],
    /// Skeleton bone indices.
    pub joints: [u8; 4],
    /// Weights of `joints`, 0..255.
    pub weights: [u8; 4],
    /// Index of the part the vertex belongs to.
    pub part: u32,
}

/// A coloured line vertex.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineVertex {
    pub position: [f32; 3],
    pub color: [u8; 4],
}

impl LineVertex {
    pub fn new(position: Vec3, color: [u8; 4]) -> Self {
        Self { position, color }
    }
}

/// Surface parameters of a part.
#[derive(Debug, Clone, PartialEq)]
pub struct MaterialData {
    pub texture: Option<TextureKey>,
    /// Multiplied with the texture.
    pub color: [f32; 4],
    /// Game-like shading; `None` draws `texture × color`.
    pub surface: Option<Box<Surface>>,
}

impl Default for MaterialData {
    fn default() -> Self {
        Self {
            texture: None,
            color: [0.75, 0.75, 0.75, 1.0],
            surface: None,
        }
    }
}

impl MaterialData {
    /// Every texture the material samples.
    pub fn textures(&self) -> impl Iterator<Item = TextureKey> + '_ {
        let surface = self.surface.as_deref();
        self.texture.into_iter().chain(
            surface
                .into_iter()
                .flat_map(|s| {
                    [
                        s.normal,
                        s.gloss,
                        s.detail_normal,
                        s.detail_mask,
                        s.detail_gloss,
                    ]
                })
                .flatten(),
        )
    }

    /// Whether the part is drawn alpha blended after the opaque parts.
    pub fn blended(&self) -> bool {
        self.surface.as_ref().is_some_and(|s| s.blend)
    }
}

/// How the colour of a surface is formed (`docs/viewers.md` §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SurfaceModel {
    /// The base texture is the colour.
    #[default]
    Basic = 1,
    /// The base texture's channels weigh three tint colours.
    TintMask = 2,
    /// Tint colours combined like screen layers; alpha from the gloss map.
    Hair = 3,
    /// The base colour tinted where the gloss map's red is set.
    Skin = 4,
    /// Eyes: the base colour tinted by the eye colour where the base
    /// texture's alpha (the iris) is set; opaque.
    Eye = 5,
}

/// Game-like shading inputs of a part. Colours are linear.
#[derive(Debug, Clone, PartialEq)]
pub struct Surface {
    pub model: SurfaceModel,
    /// Alpha blended, drawn after opaque parts without depth writes.
    pub blend: bool,
    /// Tangent-space normal map (RGB).
    pub normal: Option<TextureKey>,
    /// R rim, G ambient occlusion (hair: alpha), B specular mask.
    pub gloss: Option<TextureKey>,
    pub detail_normal: Option<TextureKey>,
    /// G: detail normal weight, B: how much of the base texture shows (tint mask).
    pub detail_mask: Option<TextureKey>,
    pub detail_gloss: Option<TextureKey>,
    /// Exponent applied to the base texture (2.2 when it holds gamma-encoded
    /// colour but is not sampled as sRGB).
    pub base_gamma: f32,
    pub tints: [[f32; 3]; 3],
    /// Multiplies the colour; w is the alpha.
    pub tint: [f32; 4],
    pub skin_tint: [f32; 3],
    /// UV of the detail maps: `uv · xy + zw`.
    pub detail_uv: [f32; 4],
    /// UV of the detail mask: `uv · xy + zw`.
    pub detail_mask_uv: [f32; 4],
    pub specular: [f32; 3],
    pub specular_power: f32,
    /// Weight of the base texture where the detail mask's blue is set (tint mask).
    pub detail_mask_strength: f32,
    pub detail_normal_strength: f32,
}

impl Default for Surface {
    fn default() -> Self {
        Self {
            model: SurfaceModel::Basic,
            blend: false,
            normal: None,
            gloss: None,
            detail_normal: None,
            detail_mask: None,
            detail_gloss: None,
            base_gamma: 1.0,
            tints: [[1.0; 3]; 3],
            tint: [1.0; 4],
            skin_tint: [1.0; 3],
            detail_uv: [1.0, 1.0, 0.0, 0.0],
            detail_mask_uv: [1.0, 1.0, 0.0, 0.0],
            specular: [0.1; 3],
            specular_power: 16.0,
            detail_mask_strength: 0.0,
            detail_normal_strength: 0.0,
        }
    }
}

/// A range of triangles drawn with one material.
#[derive(Debug, Clone, PartialEq)]
pub struct PartData {
    pub name: String,
    pub first_index: u32,
    pub index_count: u32,
    pub material: MaterialData,
    pub bounds: Aabb,
}

/// Geometry of one mesh: one vertex and one index buffer shared by its parts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshData {
    pub vertices: Vec<Vertex>,
    /// Triangle list indices into `vertices`.
    pub indices: Vec<u32>,
    pub parts: Vec<PartData>,
}

impl MeshData {
    /// Appends a part; `indices` are relative to `vertices`.
    pub fn push_part(
        &mut self,
        name: String,
        mut vertices: Vec<Vertex>,
        indices: &[u32],
        material: MaterialData,
    ) {
        let part = self.parts.len() as u32;
        let base = self.vertices.len() as u32;
        let mut bounds = Aabb::EMPTY;
        for v in &mut vertices {
            v.part = part;
            bounds.add_point(v.position);
        }
        let first_index = self.indices.len() as u32;
        self.indices.extend(indices.iter().map(|i| i + base));
        self.vertices.extend(vertices);
        self.parts.push(PartData {
            name,
            first_index,
            index_count: indices.len() as u32,
            material,
            bounds,
        });
    }

    pub fn bounds(&self) -> Aabb {
        let mut bounds = Aabb::EMPTY;
        for part in &self.parts {
            bounds.union(&part.bounds);
        }
        bounds
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Unique edges of every triangle, as line-list indices.
    pub fn edges(&self) -> Vec<u32> {
        let mut seen = HashSet::with_capacity(self.indices.len());
        let mut out = Vec::with_capacity(self.indices.len() * 2);
        for t in self.indices.chunks_exact(3) {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                if seen.insert((a.min(b), a.max(b))) {
                    out.extend([a, b]);
                }
            }
        }
        out
    }

    /// One line per vertex along its normal.
    pub fn normal_lines(&self, length: f32) -> Vec<LineVertex> {
        const BASE: [u8; 4] = [40, 120, 255, 255];
        const TIP: [u8; 4] = [140, 220, 255, 255];
        self.vertices
            .iter()
            .flat_map(|v| {
                [
                    LineVertex::new(v.position, BASE),
                    LineVertex::new(math::add(v.position, math::scale(v.normal, length)), TIP),
                ]
            })
            .collect()
    }

    /// Fills in normals averaged over the adjacent triangles, weighted by area.
    pub fn compute_normals(vertices: &mut [Vertex], indices: &[u32]) {
        let mut sums = vec![[0f32; 3]; vertices.len()];
        for t in indices.chunks_exact(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| vertices[i as usize].position);
            let n = math::cross(math::sub(b, a), math::sub(c, a));
            for &i in t {
                sums[i as usize] = math::add(sums[i as usize], n);
            }
        }
        for (v, n) in vertices.iter_mut().zip(sums) {
            v.normal = math::normalize_or(n, [0.0, 1.0, 0.0]);
        }
    }

    /// The nearest part hit by a ray in mesh space, `(part, distance)`;
    /// parts for which `skip` is true are ignored.
    pub fn ray_pick(&self, ray: &Ray, skip: impl Fn(u32) -> bool) -> Option<(u32, f32)> {
        let mut best: Option<(u32, f32)> = None;
        for (index, part) in self.parts.iter().enumerate() {
            if skip(index as u32) {
                continue;
            }
            let Some(entry) = ray.aabb(&part.bounds) else {
                continue;
            };
            if best.is_some_and(|(_, t)| entry > t) {
                continue;
            }
            let range = part.first_index as usize..(part.first_index + part.index_count) as usize;
            for t in self.indices[range].chunks_exact(3) {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| self.vertices[i as usize].position);
                if let Some(d) = ray.triangle(a, b, c)
                    && best.is_none_or(|(_, t)| d < t)
                {
                    best = Some((index as u32, d));
                }
            }
        }
        best
    }
}

/// RGBA8 pixels of a texture with its mip chain.
#[derive(Debug, Clone, PartialEq)]
pub struct TextureData {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Colour data is gamma-encoded.
    pub srgb: bool,
    /// Level 0 first; each level halves the size (at least 1).
    pub mips: Vec<Vec<u8>>,
}

/// One placed mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Instance {
    pub mesh: MeshKey,
    pub transform: Mat4,
    /// Multiplied with the surface colour.
    pub tint: [f32; 4],
}

impl Instance {
    pub fn new(mesh: MeshKey, transform: Mat4) -> Self {
        Self {
            mesh,
            transform,
            tint: [1.0; 4],
        }
    }
}

/// What a pixel shows: the instance index in [`Scene::instances`] and the part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pick {
    pub instance: u32,
    pub part: u32,
}

impl Pick {
    /// Highlights every part of an instance.
    pub const ALL_PARTS: u32 = u32::MAX;
}

/// Render switches.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneOptions {
    pub textures: bool,
    pub lighting: bool,
    pub vertex_colors: bool,
    pub alpha_test: bool,
    /// Hide faces turned away from the camera (look into closed interiors).
    pub backface_culling: bool,
    pub wireframe: bool,
    pub normals: bool,
    /// Colour by the skinning weight of this skeleton bone.
    pub bone_weights: Option<u8>,
    /// Gamma-encoded RGBA.
    pub background: [f32; 4],
    /// Skip instances outside the view frustum.
    pub frustum_culling: bool,
    /// Game-like shading of parts that have a [`Surface`]; off draws the base texture.
    pub surfaces: bool,
    /// Sun, ambient light and fog of an outdoor scene; `None`: white light
    /// from above the camera.
    pub environment: Option<Environment>,
}

/// Light and air of an outdoor scene (the cut-scene preview's time of day).
/// Colours are linear.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Environment {
    /// Direction towards the sun (or the moon).
    pub sun: Vec3,
    pub sun_color: [f32; 3],
    /// Multiplies the ambient term.
    pub ambient: [f32; 3],
    /// Weight of the soft light from the camera (1 in the viewers).
    pub camera_light: f32,
    pub fog_color: [f32; 3],
    /// Metres from the eye where the fog starts and is strongest.
    pub fog_start: f32,
    pub fog_end: f32,
    /// How much the fog hides at `fog_end` (0 = no fog).
    pub fog_max: f32,
}

impl Default for SceneOptions {
    fn default() -> Self {
        Self {
            textures: true,
            lighting: true,
            vertex_colors: false,
            alpha_test: true,
            backface_culling: false,
            wireframe: false,
            normals: false,
            bone_weights: None,
            background: [0.15, 0.155, 0.17, 1.0],
            frustum_culling: true,
            surfaces: true,
            environment: None,
        }
    }
}

/// Everything drawn in one view.
#[derive(Debug, Clone, Default)]
pub struct Scene {
    pub instances: Vec<Instance>,
    /// World-space lines tested against depth.
    pub lines: Vec<LineVertex>,
    /// World-space lines drawn over everything (gizmos, skeletons).
    pub overlay_lines: Vec<LineVertex>,
    /// Parts not drawn: `(mesh, part)`.
    pub hidden_parts: HashSet<(MeshKey, u32)>,
    pub highlight: Option<Pick>,
    pub options: SceneOptions,
}

impl Scene {
    /// A scene with one mesh at the origin.
    pub fn single(mesh: MeshKey) -> Self {
        Self {
            instances: vec![Instance::new(mesh, math::IDENTITY)],
            ..Self::default()
        }
    }

    /// Ray picking on the CPU against the given mesh data, ignoring hidden
    /// parts (but not alpha-tested texels); the GPU id pass
    /// ([`super::SceneRenderer::pick`]) is the faster path for large scenes.
    pub fn ray_pick(&self, ray: &Ray, meshes: &HashMap<MeshKey, &MeshData>) -> Option<Pick> {
        let mut best: Option<(Pick, f32)> = None;
        for (index, instance) in self.instances.iter().enumerate() {
            let Some(mesh) = meshes.get(&instance.mesh) else {
                continue;
            };
            let Some(inverse) = math::inverse(&instance.transform) else {
                continue;
            };
            let origin = math::transform_point(&inverse, ray.origin);
            let far = math::transform_point(&inverse, ray.at(1.0));
            let local = Ray {
                origin,
                dir: math::normalize_or(math::sub(far, origin), ray.dir),
            };
            let hidden = |part| self.hidden_parts.contains(&(instance.mesh, part));
            if let Some((part, t)) = mesh.ray_pick(&local, hidden) {
                let world = math::transform_point(&instance.transform, local.at(t));
                let distance = math::length(math::sub(world, ray.origin));
                if best.is_none_or(|(_, d)| distance < d) {
                    best = Some((
                        Pick {
                            instance: index as u32,
                            part,
                        },
                        distance,
                    ));
                }
            }
        }
        best.map(|(pick, _)| pick)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A unit cube centred on the origin as two parts (top and the rest).
    pub fn cube() -> MeshData {
        let corners: Vec<Vec3> = (0..8)
            .map(|i| {
                [
                    if i & 1 == 0 { -0.5 } else { 0.5 },
                    if i & 2 == 0 { -0.5 } else { 0.5 },
                    if i & 4 == 0 { -0.5 } else { 0.5 },
                ]
            })
            .collect();
        let quads: [[u32; 4]; 6] = [
            [2, 6, 7, 3], // top (+Y)
            [0, 1, 5, 4], // bottom
            [0, 2, 3, 1], // -Z
            [4, 5, 7, 6], // +Z
            [0, 4, 6, 2], // -X
            [1, 3, 7, 5], // +X
        ];
        let mut mesh = MeshData::default();
        for (name, faces) in [("top", &quads[..1]), ("sides", &quads[1..])] {
            let mut vertices: Vec<Vertex> = corners
                .iter()
                .map(|&position| Vertex {
                    position,
                    ..Vertex::default()
                })
                .collect();
            let indices: Vec<u32> = faces
                .iter()
                .flat_map(|q| [q[0], q[1], q[2], q[0], q[2], q[3]])
                .collect();
            MeshData::compute_normals(&mut vertices, &indices);
            mesh.push_part(name.to_owned(), vertices, &indices, MaterialData::default());
        }
        mesh
    }

    #[test]
    fn parts_share_buffers() {
        let mesh = cube();
        assert_eq!(mesh.parts.len(), 2);
        assert_eq!(mesh.vertices.len(), 16);
        assert_eq!(mesh.parts[1].first_index, 6);
        assert_eq!(mesh.triangle_count(), 12);
        assert!(mesh.indices[6..].iter().all(|&i| i >= 8));
        assert!(mesh.vertices[8..].iter().all(|v| v.part == 1));
        assert_eq!(mesh.bounds().min, [-0.5; 3]);
        // The top part: 4 edges and a diagonal. The sides: all 12 cube edges and
        // 5 diagonals, as the parts do not share vertices.
        assert_eq!(mesh.edges().len() / 2, 5 + 12 + 5);
        assert_eq!(mesh.normal_lines(0.1).len(), 32);
    }

    #[test]
    fn ray_pick_finds_the_nearest_part() {
        let mesh = cube();
        let mut scene = Scene::single(1);
        scene
            .instances
            .push(Instance::new(1, math::translation([0.0, 0.0, -3.0])));
        let meshes = HashMap::from([(1, &mesh)]);
        let down = Ray {
            origin: [0.1, 5.0, 0.1],
            dir: [0.0, -1.0, 0.0],
        };
        assert_eq!(
            scene.ray_pick(&down, &meshes),
            Some(Pick {
                instance: 0,
                part: 0
            })
        );
        let along = Ray {
            origin: [0.1, 0.1, -10.0],
            dir: [0.0, 0.0, 1.0],
        };
        assert_eq!(
            scene.ray_pick(&along, &meshes),
            Some(Pick {
                instance: 1,
                part: 1
            })
        );
        scene.hidden_parts.insert((1, 0));
        assert_eq!(
            scene.ray_pick(&down, &meshes),
            Some(Pick {
                instance: 0,
                part: 1
            }),
            "the ray passes the hidden top and hits the bottom"
        );
        let miss = Ray {
            origin: [5.0, 5.0, 5.0],
            dir: [0.0, 1.0, 0.0],
        };
        assert_eq!(scene.ray_pick(&miss, &meshes), None);
    }
}
