//! The drawable items of a level and the scene of one frame: instances of
//! the meshes of every placed object, markers for objects without a mesh,
//! trees and AI points, filtered by class, kind and level of detail.

use std::collections::{HashMap, HashSet};

use doctor_toolkit::level::{Level, LodRange, RowMatrix, compose};

use super::loader;
use crate::render::math::{self, Aabb, Mat4, Vec3};
use crate::render::{Instance, MaterialData, MeshData, MeshKey, Scene, Vertex};

/// An object of the level: group index and object index in the group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjRef {
    pub group: usize,
    pub object: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemKind {
    /// A mesh of the object's model.
    Mesh,
    /// An object with a position but nothing drawable.
    Marker,
    /// A tree of the model (`treemodel`); trees not in the game files are
    /// markers.
    Tree,
    /// A point of a point table (`0xDF14DA75`).
    Point,
}

/// One drawable thing.
#[derive(Debug, Clone)]
pub struct Item {
    pub kind: ItemKind,
    pub mesh: MeshKey,
    /// Placement relative to the object (or the world for points).
    pub local: RowMatrix,
    pub world: Mat4,
    pub object: Option<ObjRef>,
    pub class: u32,
    pub lod: Option<LodRange>,
    pub tint: [f32; 4],
}

pub const MARKER_KEY: MeshKey = 0x4C4D_0000_0000_0001;

/// A small upright box for markers: 1 × 1 × 1 around the origin, base at 0.
pub fn marker_mesh() -> MeshData {
    let corners: Vec<Vec3> = (0..8)
        .map(|i| {
            [
                if i & 1 == 0 { -0.5 } else { 0.5 },
                if i & 2 == 0 { 0.0 } else { 1.0 },
                if i & 4 == 0 { -0.5 } else { 0.5 },
            ]
        })
        .collect();
    let quads: [[u32; 4]; 6] = [
        [2, 6, 7, 3],
        [0, 1, 5, 4],
        [0, 2, 3, 1],
        [4, 5, 7, 6],
        [0, 4, 6, 2],
        [1, 3, 7, 5],
    ];
    let mut mesh = MeshData::default();
    for q in quads {
        let mut vertices: Vec<Vertex> = q
            .iter()
            .map(|&i| Vertex {
                position: corners[i as usize],
                color: [255; 4],
                ..Vertex::default()
            })
            .collect();
        let indices = [0, 1, 2, 0, 2, 3];
        MeshData::compute_normals(&mut vertices, &indices);
        let material = MaterialData {
            texture: None,
            color: [1.0; 4],
            surface: None,
            sky: false,
        };
        if mesh.parts.is_empty() {
            mesh.push_part("marker".into(), vertices, &indices, material);
        } else {
            // One part: append to it.
            let base = mesh.vertices.len() as u32;
            mesh.vertices.extend(vertices);
            mesh.indices.extend(indices.iter().map(|i| i + base));
            mesh.parts[0].index_count += 6;
            for v in &mesh.vertices {
                mesh.parts[0].bounds.add_point(v.position);
            }
        }
    }
    mesh
}

fn scale_matrix(x: f32, y: f32, z: f32) -> RowMatrix {
    [
        [x, 0.0, 0.0, 0.0],
        [0.0, y, 0.0, 0.0],
        [0.0, 0.0, z, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// A colour of a class, for markers.
fn class_tint(class: u32) -> [f32; 4] {
    let h = class.wrapping_mul(0x9E37_79B9);
    let c = |shift: u32| 0.35 + ((h >> shift) & 0xFF) as f32 / 255.0 * 0.6;
    [c(0), c(8), c(16), 1.0]
}

pub fn flatten(m: &RowMatrix) -> Mat4 {
    *m.as_flattened().first_chunk::<16>().expect("16 values")
}

/// A light of a placed object (stage 70, `docs/analysis/renderer.md` §10.9).
#[derive(Debug, Clone)]
pub struct WorldLight {
    pub light: doctor_toolkit::level::Light,
    pub position: Vec3,
    /// Spots: the axis they shine along.
    pub axis: Vec3,
}

/// The lights of the level's objects that light the streets: lights of
/// interiors (`insideOutside` 1), of faces in cut-scenes and of finer levels
/// of detail than the nearest are left out.
pub fn build_lights(level: &Level) -> Vec<WorldLight> {
    use doctor_toolkit::level::LightKind;
    let mut out = Vec::new();
    for group in &level.groups {
        for object in &group.objects {
            let Some(transform) = object.transform else {
                continue;
            };
            let Some(model) = object.model.and_then(|id| level.models.get(&id)) else {
                continue;
            };
            for light in &model.desc.lights {
                if light.inside_outside == 1
                    || light.kind == LightKind::Other
                    || light.radius <= 0.0
                    || light.lod.is_some_and(|l| l[0] > 0.0)
                {
                    continue;
                }
                let m = compose(&light.matrix, &transform);
                out.push(WorldLight {
                    light: light.clone(),
                    position: [m[3][0], m[3][1], m[3][2]],
                    axis: math::normalize_or([m[2][0], m[2][1], m[2][2]], [0.0, -1.0, 0.0]),
                });
            }
        }
    }
    out
}

/// The renderer's lights at night share `night`: colours to linear light,
/// times the strength of the time of day; lights that are off are left out.
pub fn scene_lights(lights: &[WorldLight], night: f32) -> Vec<crate::render::PointLight> {
    use doctor_toolkit::level::LightKind;
    lights
        .iter()
        .filter_map(|w| {
            let l = &w.light;
            let strength = l.strength(night);
            if strength <= 1e-3 {
                return None;
            }
            let spot = matches!(l.kind, LightKind::Spot | LightKind::Area).then(|| {
                let cone = l.cone.clamp(1.0, 89.0).to_radians().cos();
                let full = l
                    .full_cone
                    .clamp(0.0, l.cone.clamp(1.0, 89.0))
                    .to_radians()
                    .cos();
                (w.axis, cone, full.max(cone + 1e-3))
            });
            Some(crate::render::PointLight {
                position: w.position,
                radius: l.radius,
                full_radius: l.full_radius,
                color: l.color.map(|c| c.powf(2.2) * strength),
                spot,
                wrap: l.wrap,
            })
        })
        .collect()
}

/// Every drawable item of the level; meshes not found are left out.
pub fn build_items(level: &Level) -> (Vec<Item>, HashMap<ObjRef, Vec<usize>>) {
    let mut items = Vec::new();
    let mut by_object: HashMap<ObjRef, Vec<usize>> = HashMap::new();
    for (g, group) in level.groups.iter().enumerate() {
        for object in &group.objects {
            let Some(transform) = object.transform else {
                continue;
            };
            let r = ObjRef {
                group: g,
                object: object.index,
            };
            let mut push = |item: Item, items: &mut Vec<Item>| {
                by_object.entry(r).or_default().push(items.len());
                items.push(item);
            };
            let model = object.model.and_then(|id| level.models.get(&id));
            let mut drawn = false;
            if let Some(model) = model {
                for piece in &model.desc.pieces {
                    let Some(slot) = level.mesh_slot(piece.mesh) else {
                        continue;
                    };
                    let mesh = &level.meshes[slot];
                    if mesh.found.is_none() {
                        continue;
                    }
                    drawn = true;
                    push(
                        Item {
                            kind: ItemKind::Mesh,
                            mesh: loader::mesh_key(mesh.id, mesh.found),
                            local: piece.matrix,
                            world: flatten(&compose(&piece.matrix, &transform)),
                            object: Some(r),
                            class: object.class,
                            lod: piece.lod,
                            tint: [1.0; 4],
                        },
                        &mut items,
                    );
                }
                for tree in &model.desc.trees {
                    let found = level
                        .tree_slot(tree.resource)
                        .and_then(|slot| level.trees[slot].found);
                    let item = match found {
                        Some(found) => Item {
                            kind: ItemKind::Tree,
                            mesh: loader::tree_key(tree.resource, Some(found)),
                            local: tree.matrix,
                            world: flatten(&compose(&tree.matrix, &transform)),
                            object: Some(r),
                            class: object.class,
                            lod: None,
                            tint: [1.0; 4],
                        },
                        // A tree that is not in the game files: a green post.
                        None => {
                            let local = compose(&scale_matrix(1.0, 6.0, 1.0), &tree.matrix);
                            Item {
                                kind: ItemKind::Marker,
                                mesh: MARKER_KEY,
                                local,
                                world: flatten(&compose(&local, &transform)),
                                object: Some(r),
                                class: object.class,
                                lod: None,
                                tint: [0.3, 0.75, 0.3, 1.0],
                            }
                        }
                    };
                    drawn |= found.is_some();
                    push(item, &mut items);
                }
            }
            if !drawn {
                let local = scale_matrix(0.6, 0.6, 0.6);
                push(
                    Item {
                        kind: ItemKind::Marker,
                        mesh: MARKER_KEY,
                        local,
                        world: flatten(&compose(&local, &transform)),
                        object: Some(r),
                        class: object.class,
                        lod: None,
                        tint: class_tint(object.class),
                    },
                    &mut items,
                );
            }
        }
    }
    for table in &level.points {
        for p in &table.points {
            let local = compose(&scale_matrix(0.3, 1.3, 0.3), &p.matrix());
            items.push(Item {
                kind: ItemKind::Point,
                mesh: MARKER_KEY,
                world: flatten(&local),
                local,
                object: None,
                class: 0,
                lod: None,
                tint: [0.3, 0.85, 0.95, 1.0],
            });
        }
    }
    (items, by_object)
}

/// Recomputes the world matrices of an object's items for a new transform.
pub fn move_items(items: &mut [Item], indices: &[usize], transform: &RowMatrix) {
    for &i in indices {
        items[i].world = flatten(&compose(&items[i].local, transform));
    }
}

/// How levels of detail are chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LodMode {
    /// By the distance of the camera to the object, as the game does.
    Distance,
    /// Always the most detailed level.
    Finest,
}

/// Objects of a level picked by zone, by class in a zone or one by one: the
/// objects hidden by the Outliner's eye, or locked by its lock (stage 53).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectSet {
    /// Every object of these SimGroups (by index in the level).
    pub groups: HashSet<usize>,
    /// Objects of a class in a SimGroup.
    pub classes: HashSet<(usize, u32)>,
    pub objects: HashSet<ObjRef>,
}

impl ObjectSet {
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty() && self.classes.is_empty() && self.objects.is_empty()
    }

    /// Whether object `r` of class `class` is in the set, by any of the three.
    pub fn contains(&self, r: ObjRef, class: u32) -> bool {
        !self.is_empty()
            && (self.groups.contains(&r.group)
                || self.classes.contains(&(r.group, class))
                || self.objects.contains(&r))
    }

    pub fn clear(&mut self) {
        self.groups.clear();
        self.classes.clear();
        self.objects.clear();
    }
}

/// What a frame shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Filter {
    pub lod: LodMode,
    pub meshes: bool,
    pub markers: bool,
    pub trees: bool,
    pub points: bool,
    pub hidden_classes: HashSet<u32>,
    /// Objects hidden in the view (the Outliner's eye).
    pub hidden: ObjectSet,
    /// The primary selected object: the gizmo and Details follow it.
    pub selected: Option<ObjRef>,
    /// Every selected object, the primary one too (all highlighted).
    pub selection: HashSet<ObjRef>,
    pub hovered: Option<ObjRef>,
    /// Instances smaller than this angle (radians) on the screen are skipped.
    pub min_angle: f32,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            lod: LodMode::Distance,
            meshes: true,
            markers: true,
            trees: true,
            points: false,
            hidden_classes: HashSet::new(),
            hidden: ObjectSet::default(),
            selected: None,
            selection: HashSet::new(),
            hovered: None,
            min_angle: 0.0,
        }
    }
}

const SELECTED_TINT: [f32; 4] = [1.5, 1.25, 0.45, 1.0];
const HOVERED_TINT: [f32; 4] = [1.25, 1.25, 1.4, 1.0];

/// Centre and radius of a mesh in its own space.
pub type MeshSphere = (Vec3, f32);

/// Fills `scene.instances` with the items to draw; returns the item index
/// of every instance (for picking). `sphere` gives the bounds of uploaded
/// meshes, for skipping instances smaller than [`Filter::min_angle`].
pub fn fill(
    scene: &mut Scene,
    items: &[Item],
    eye: Vec3,
    filter: &Filter,
    sphere: &dyn Fn(MeshKey) -> Option<MeshSphere>,
) -> Vec<usize> {
    scene.instances.clear();
    let mut visible = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let selected = item
            .object
            .is_some_and(|r| filter.selected == Some(r) || filter.selection.contains(&r));
        let shown = match item.kind {
            ItemKind::Mesh => filter.meshes,
            // A selected object stays visible with the markers hidden.
            ItemKind::Marker => filter.markers || selected,
            ItemKind::Tree => filter.trees,
            ItemKind::Point => filter.points,
        };
        if !shown || filter.hidden_classes.contains(&item.class) {
            continue;
        }
        if let Some(r) = item.object
            && filter.hidden.contains(r, item.class)
        {
            continue;
        }
        if let Some([near, far]) = item.lod {
            match filter.lod {
                LodMode::Distance => {
                    let at = [item.world[12], item.world[13], item.world[14]];
                    let d = math::length(math::sub(at, eye));
                    if d < near || d >= far {
                        continue;
                    }
                }
                LodMode::Finest => {
                    if near > 0.0 {
                        continue;
                    }
                }
            }
        }
        if filter.min_angle > 0.0
            && !selected
            && let Some((centre, radius)) = sphere(item.mesh)
        {
            let at = math::transform_point(&item.world, centre);
            let distance = math::length(math::sub(at, eye));
            let size = radius * math::max_scale(&item.world);
            if distance > size && size < distance * filter.min_angle {
                continue;
            }
        }
        let mut tint = item.tint;
        let highlight = if selected {
            Some(SELECTED_TINT)
        } else if item.object.is_some() && item.object == filter.hovered {
            Some(HOVERED_TINT)
        } else {
            None
        };
        if let Some(h) = highlight {
            for (t, h) in tint.iter_mut().zip(h) {
                *t *= h;
            }
        }
        scene.instances.push(Instance {
            mesh: item.mesh,
            transform: item.world,
            tint,
        });
        visible.push(i);
    }
    visible
}

/// The level extent: world boxes of the meshes whose bounds `mesh` knows,
/// or the item origins when none is known yet.
pub fn bounds(items: &[Item], mesh: &dyn Fn(MeshKey) -> Option<Aabb>) -> Aabb {
    let mut b = Aabb::EMPTY;
    for item in items.iter().filter(|i| i.kind == ItemKind::Mesh) {
        if let Some(local) = mesh(item.mesh) {
            b.union(&local.transformed(&item.world));
        }
    }
    if b.is_empty() {
        for item in items {
            b.add_point([item.world[12], item.world[13], item.world[14]]);
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: ItemKind, x: f32, lod: Option<LodRange>, object: usize) -> Item {
        let mut m = doctor_toolkit::level::IDENTITY;
        m[3][0] = x;
        Item {
            kind,
            mesh: 1,
            local: doctor_toolkit::level::IDENTITY,
            world: flatten(&m),
            object: Some(ObjRef { group: 0, object }),
            class: 7,
            lod,
            tint: [1.0; 4],
        }
    }

    #[test]
    fn hidden_objects_are_left_out_and_every_selected_one_is_lit() {
        let items = vec![
            item(ItemKind::Marker, 0.0, None, 0),
            item(ItemKind::Marker, 1.0, None, 1),
            item(ItemKind::Marker, 2.0, None, 2),
        ];
        let mut scene = Scene::default();
        let mut filter = Filter::default();
        let r = |object| ObjRef { group: 0, object };
        filter.hidden.objects.insert(r(1));
        assert_eq!(
            fill(&mut scene, &items, [0.0; 3], &filter, &|_| None),
            [0, 2]
        );
        filter.hidden.clear();
        filter.hidden.classes.insert((0, 7));
        assert!(fill(&mut scene, &items, [0.0; 3], &filter, &|_| None).is_empty());
        filter.hidden.clear();
        filter.hidden.groups.insert(1);
        assert_eq!(
            fill(&mut scene, &items, [0.0; 3], &filter, &|_| None).len(),
            3
        );
        filter.selected = Some(r(0));
        filter.selection = [r(0), r(2)].into();
        fill(&mut scene, &items, [0.0; 3], &filter, &|_| None);
        assert_eq!(scene.instances[0].tint, SELECTED_TINT);
        assert_ne!(scene.instances[1].tint, SELECTED_TINT);
        assert_eq!(scene.instances[2].tint, SELECTED_TINT);
    }

    #[test]
    fn filters_by_kind_class_and_distance() {
        let items = vec![
            item(ItemKind::Mesh, 0.0, Some([0.0, 25.0]), 0),
            item(ItemKind::Mesh, 0.0, Some([25.0, 1e4]), 0),
            item(ItemKind::Marker, 100.0, None, 1),
            item(ItemKind::Point, 5.0, None, 2),
        ];
        let mut scene = Scene::default();
        let mut filter = Filter::default();
        // Near: the fine LOD and the marker; points are off by default.
        let visible = fill(&mut scene, &items, [0.0, 0.0, 10.0], &filter, &|_| None);
        assert_eq!(visible, vec![0, 2]);
        // Far: the coarse LOD.
        assert_eq!(
            fill(&mut scene, &items, [0.0, 0.0, 50.0], &filter, &|_| None),
            vec![1, 2]
        );
        filter.lod = LodMode::Finest;
        filter.points = true;
        assert_eq!(
            fill(&mut scene, &items, [0.0, 0.0, 50.0], &filter, &|_| None),
            vec![0, 2, 3]
        );
        filter.hidden_classes.insert(7);
        assert!(fill(&mut scene, &items, [0.0; 3], &filter, &|_| None).is_empty());
        filter.hidden_classes.clear();
        filter.selected = Some(ObjRef {
            group: 0,
            object: 1,
        });
        fill(&mut scene, &items, [0.0; 3], &filter, &|_| None);
        assert_eq!(scene.instances[1].tint, SELECTED_TINT);
        // Tiny instances far away are skipped, the selected one stays.
        filter.min_angle = 0.01;
        let small = |_| Some(([0.0; 3], 0.5));
        let visible = fill(&mut scene, &items, [0.0, 0.0, 200.0], &filter, &small);
        assert_eq!(visible, vec![2]);
        filter.selected = None;
        assert!(fill(&mut scene, &items, [0.0, 0.0, 200.0], &filter, &small).is_empty());
        assert_eq!(
            fill(&mut scene, &items, [0.0, 0.0, 20.0], &filter, &small).len(),
            2
        );
        assert_eq!(bounds(&items, &|_| None).max[0], 100.0, "origins");
        let unit = Aabb {
            min: [-1.0; 3],
            max: [1.0; 3],
        };
        assert_eq!(bounds(&items, &|_| Some(unit)).max[0], 1.0, "meshes only");
    }

    #[test]
    fn marker_mesh_is_one_closed_part() {
        let m = marker_mesh();
        assert_eq!(m.parts.len(), 1);
        assert_eq!(m.triangle_count(), 12);
        assert_eq!(m.bounds().min, [-0.5, 0.0, -0.5]);
        assert_eq!(m.bounds().max, [0.5, 1.0, 0.5]);
    }
}
