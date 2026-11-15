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
    /// Index of the part the vertex belongs to, with [`Vertex::BILLBOARD`]
    /// for a corner of a leaf card that faces the camera.
    pub part: u32,
}

impl Vertex {
    /// Bit of [`Vertex::part`]: the vertex is a corner of a card turned to the
    /// camera (the rotating leaves of trees, `docs/formats/treemodel.md`).
    /// `position` is the card centre, `normal` the corner's offset in camera
    /// space (x right, y up, z forward) scaled by the object's height scale,
    /// and `weights` the card's normal in camera space as `n · ½ + ½`.
    pub const BILLBOARD: u32 = 0x8000_0000;

    pub fn is_billboard(&self) -> bool {
        self.part & Self::BILLBOARD != 0
    }
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
    /// The game's sky dome: hidden when shading as in the game, which draws
    /// its own sky.
    pub sky: bool,
}

impl Default for MaterialData {
    fn default() -> Self {
        Self {
            texture: None,
            color: [0.75, 0.75, 0.75, 1.0],
            surface: None,
            sky: false,
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
                        s.detail,
                        s.exponent_map,
                        s.mask,
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
    /// World surfaces, shaded only as in the game (simple shading draws
    /// their base texture): the models of
    /// [`doctor_toolkit::resource::world_shading::WorldModel`].
    WorldBasic = 6,
    Building = 7,
    Detail = 8,
    Panorama = 9,
    Overlay = 10,
    Window = 11,
    Glass = 12,
    Water = 13,
}

impl SurfaceModel {
    /// A world surface (`docs/analysis/renderer.md` §10.7).
    pub fn is_world(self) -> bool {
        self as u32 >= SurfaceModel::WorldBasic as u32
    }
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
    /// World surfaces: the second colour layer (windows: the front map,
    /// panoramas: the map whose blue masks the tint), at `detail_uv`.
    pub detail: Option<TextureKey>,
    /// World surfaces: R weighs the base layer over the detail layer, at
    /// `detail_mask_uv`.
    pub mask: Option<TextureKey>,
    /// World surfaces: blue is the sharpness of the sun's highlight.
    pub exponent_map: Option<TextureKey>,
    /// World surfaces: tint of the base layer where its alpha is set
    /// (water: its colour); w: alpha. Linear.
    pub base_tint: [f32; 4],
    /// World surfaces: tint of the detail layer; w: its alpha.
    pub detail_tint: [f32; 4],
    pub world: WorldParams,
}

/// Parameters of world surfaces (`docs/analysis/renderer.md` §10.7).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldParams {
    /// Multiplies the mask.
    pub mask_scale: f32,
    /// How far the normal map tilts the normal.
    pub normal_scale: f32,
    /// Sharpness `s` of the highlight (`pow(n·h, s³·255)`), or the factor
    /// on the exponent map's blue.
    pub exponent: f32,
    /// How much the overlay darkens (`Overlay`).
    pub overlay: f32,
    /// Reflection of the sky: facing the camera and the extra at grazing angles.
    pub reflection: [f32; 2],
    /// The highlight is masked by the gloss map's red.
    pub specular_masked: bool,
    /// The highlight is multiplied by the light.
    pub specular_lit: bool,
    /// The reflection is masked by the gloss map's green.
    pub reflection_masked: bool,
    /// Cut where the base texture's alpha is below one half.
    pub alpha_test: bool,
    /// Light given off by day and at night (rgb times w; stage 70):
    /// panoramas, lit windows, lamps and signs.
    pub glow: [[f32; 4]; 2],
}

impl Default for WorldParams {
    fn default() -> Self {
        Self {
            mask_scale: 1.0,
            normal_scale: 1.0,
            exponent: 0.0,
            overlay: 0.0,
            reflection: [0.0; 2],
            specular_masked: false,
            specular_lit: false,
            reflection_masked: false,
            alpha_test: false,
            glow: [[0.0; 4]; 2],
        }
    }
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
            detail: None,
            mask: None,
            exponent_map: None,
            base_tint: [1.0; 4],
            detail_tint: [1.0; 4],
            world: WorldParams::default(),
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
    /// Draw-order layer ([`MeshData::layer_coplanar_parts`]); `None`: the
    /// part's index (the parts keep their order).
    pub layer: Option<u32>,
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
            v.part = part | (v.part & Vertex::BILLBOARD);
            if v.is_billboard() {
                // The card turns to the camera: any direction up to its corner.
                let r = math::length(v.normal);
                bounds.add_point(math::sub(v.position, [r; 3]));
                bounds.add_point(math::add(v.position, [r; 3]));
            } else {
                bounds.add_point(v.position);
            }
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
            layer: None,
        });
    }

    /// Gives every part a draw-order layer: 0, or one above the highest
    /// layer of the earlier parts it shares a plane with (a sign on a wall,
    /// a patch on the ground — surfaces in one plane take the colour of the
    /// part drawn last). The game look groups the parts of one layer of all
    /// meshes by material (`batch::Order::Layers`).
    pub fn layer_coplanar_parts(&mut self) {
        use std::collections::HashSet;
        // Planes of a part's triangles, quantised (normal to 1/1000, offset
        // to 1 cm), the normal turned to one side.
        let planes = |part: &PartData| -> HashSet<[i32; 4]> {
            let range = part.first_index as usize..(part.first_index + part.index_count) as usize;
            let mut out = HashSet::new();
            for t in self.indices[range].chunks_exact(3) {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| self.vertices[i as usize].position);
                let n = math::cross(math::sub(b, a), math::sub(c, a));
                let len = math::length(n);
                if len < 1e-12 {
                    continue;
                }
                let mut n = math::scale(n, 1.0 / len);
                let flip = n.iter().find(|x| x.abs() > 1e-4).is_some_and(|x| *x < 0.0);
                if flip {
                    n = math::scale(n, -1.0);
                }
                let d = math::dot(n, a);
                out.insert([
                    (n[0] * 1000.0).round() as i32,
                    (n[1] * 1000.0).round() as i32,
                    (n[2] * 1000.0).round() as i32,
                    (d * 100.0).round() as i32,
                ]);
            }
            out
        };
        let sets: Vec<HashSet<[i32; 4]>> = self.parts.iter().map(planes).collect();
        let mut layers: Vec<u32> = Vec::with_capacity(self.parts.len());
        for i in 0..self.parts.len() {
            let layer = (0..i)
                .filter(|&j| {
                    let (a, b) = (&self.parts[i].bounds, &self.parts[j].bounds);
                    (0..3).all(|k| a.min[k] <= b.max[k] + 0.01 && b.min[k] <= a.max[k] + 0.01)
                        && !sets[i].is_disjoint(&sets[j])
                })
                .map(|j| layers[j] + 1)
                .max()
                .unwrap_or(0);
            layers.push(layer);
        }
        for (part, layer) in self.parts.iter_mut().zip(layers) {
            part.layer = Some(layer);
        }
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
    /// Shade as the game does (its light, sky and fog); `None`: the simple
    /// shading above.
    pub game: Option<super::GameLook>,
    /// In the game look: the sun casts shadows (stage 70).
    pub shadows: bool,
    /// In the game look: the lights of lamps ([`Scene::lights`]) shine.
    pub lamps: bool,
}

/// A light of a lamp in the world (game look, `docs/analysis/renderer.md`
/// §10.9). Colours are linear and include the strength.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointLight {
    pub position: Vec3,
    /// No light beyond it.
    pub radius: f32,
    /// Full light within it.
    pub full_radius: f32,
    pub color: [f32; 3],
    /// A spot: its axis and the cosines of its cone and of its full light.
    pub spot: Option<(Vec3, f32, f32)>,
    /// Added to `n·l` before clamping.
    pub wrap: f32,
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
            game: None,
            shadows: true,
            lamps: true,
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
    /// Lights of lamps (game look, [`SceneOptions::lamps`]).
    pub lights: Vec<PointLight>,
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
    fn parts_in_one_plane_keep_their_order() {
        // The cube (top, sides), a patch on its top, a box beside it.
        let mut mesh = cube();
        let quad = |y: f32, x: f32| -> Vec<Vertex> {
            [
                [-0.2, y, -0.2],
                [0.2, y, -0.2],
                [0.2, y, 0.2],
                [-0.2, y, 0.2],
            ]
            .map(|p| Vertex {
                position: [p[0] + x, p[1], p[2]],
                ..Vertex::default()
            })
            .to_vec()
        };
        let indices = [0, 1, 2, 0, 2, 3];
        mesh.push_part(
            "patch".into(),
            quad(0.5, 0.0),
            &indices,
            MaterialData::default(),
        );
        mesh.push_part(
            "away".into(),
            quad(0.5, 5.0),
            &indices,
            MaterialData::default(),
        );
        mesh.layer_coplanar_parts();
        let layers: Vec<Option<u32>> = mesh.parts.iter().map(|p| p.layer).collect();
        // The patch lies on the top: above it; the far quad shares the plane
        // but not the place; the sides touch the top only at its edges.
        assert_eq!(layers, [Some(0), Some(0), Some(1), Some(0)]);
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
