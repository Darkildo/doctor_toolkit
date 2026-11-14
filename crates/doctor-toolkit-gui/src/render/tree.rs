//! Tree models (`treemodel`, `docs/formats/treemodel.md`) as renderer
//! meshes: one part per tree part; trunk strips and leaf quads become
//! triangles, the corners of rotating leaves become [`Vertex::BILLBOARD`]
//! vertices that the vertex shader turns to the camera.

use doctor_toolkit::resource::material::{Material, ParamValue, TextureRole};
use doctor_toolkit::resource::treemodel::{TreeModel, Vertices, leaf_corner};

use super::{MaterialData, MeshData, Vertex};

/// What a tree part's material gives the renderer: the colour texture
/// (`sampColor`) and the tint it is multiplied with (`g_BaseTintColor`,
/// linear).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TreeMaterial {
    pub texture: Option<u32>,
    pub tint: [f32; 4],
}

impl TreeMaterial {
    pub fn of(material: &Material) -> Self {
        let texture = material
            .params
            .iter()
            .find_map(|p| match p.value {
                ParamValue::Texture(hash) if p.name == "sampColor" => Some(hash),
                _ => None,
            })
            .or_else(|| material.texture(TextureRole::Diffuse));
        let tint = material
            .floats("g_BaseTintColor")
            .filter(|v| v.len() >= 3)
            .map_or([1.0; 4], |v| [v[0], v[1], v[2], 1.0]);
        Self { texture, tint }
    }
}

/// The renderer mesh of a tree; `material` gives the material of a part
/// from its material hash.
pub fn tree_mesh(tree: &TreeModel, mut material: impl FnMut(u32) -> MaterialData) -> MeshData {
    let unorm = |x: f32| ((x * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8;
    let mut data = MeshData::default();
    for part in &tree.parts {
        let mut vertices: Vec<Vertex> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        for mesh in &part.meshes {
            let base = vertices.len() as u32;
            match &mesh.vertices {
                Vertices::Mesh(source) => vertices.extend(source.iter().map(|v| Vertex {
                    position: v.position,
                    normal: v.normal(),
                    uv: v.uv,
                    color: [255; 4],
                    ..Vertex::default()
                })),
                Vertices::Leaves(source) => vertices.extend(source.iter().map(|v| {
                    let (offset, normal) = leaf_corner(v.corner);
                    Vertex {
                        position: v.position,
                        normal: offset.map(|x| x * v.size),
                        uv: v.uv,
                        color: [255; 4],
                        weights: [unorm(normal[0]), unorm(normal[1]), unorm(normal[2]), 255],
                        part: Vertex::BILLBOARD,
                        ..Vertex::default()
                    }
                })),
            }
            indices.extend(
                mesh.triangles(part.kind)
                    .into_iter()
                    .flatten()
                    .map(|i| i + base),
            );
        }
        data.push_part(
            tree.part_name(part).to_owned(),
            vertices,
            &indices,
            material(part.material_hash),
        );
    }
    data
}

/// The material of a tree part for a texture key (or none) and the tint.
pub fn tree_part_material(texture: Option<u64>, tint: [f32; 4]) -> MaterialData {
    MaterialData {
        texture,
        color: if texture.is_some() {
            tint
        } else {
            [0.3, 0.55, 0.25, 1.0]
        },
        surface: None,
        sky: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_toolkit::resource::treemodel::{
        LeafVertex, PartKind, TreeMesh, TreePart, TreeVertex,
    };

    #[test]
    fn rotating_leaves_become_billboards_and_strips_triangles() {
        let trunk = TreePart {
            kind: PartKind::Trunk,
            material_hash: 1,
            name_offset: 0,
            meshes: vec![TreeMesh {
                quads: 0,
                vertices: Vertices::Mesh(
                    (0..4)
                        .map(|i| TreeVertex {
                            position: [i as f32, 0.0, 0.0],
                            uv: [0.0; 2],
                            normal: [127, 255, 127, 255],
                        })
                        .collect(),
                ),
                indices: vec![0, 1, 2, 3],
                padding: Vec::new(),
            }],
        };
        let leaves = TreePart {
            kind: PartKind::RotatingLeaves,
            material_hash: 2,
            name_offset: 0,
            meshes: vec![TreeMesh {
                quads: 1,
                vertices: Vertices::Leaves(
                    (0..4)
                        .map(|c| LeafVertex {
                            position: [0.0, 5.0, 0.0],
                            uv: [0.0; 2],
                            size: 2.0,
                            corner: c as f32,
                            shade: 0.0,
                        })
                        .collect(),
                ),
                indices: Vec::new(),
                padding: Vec::new(),
            }],
        };
        let tree = TreeModel {
            path_hash: 0,
            reserved: [0; 2],
            sphere: [0.0; 4],
            height: 6.0,
            parts: vec![trunk, leaves],
            strings: b"x\0".to_vec(),
            tail: Vec::new(),
        };
        let mut asked = Vec::new();
        let mesh = tree_mesh(&tree, |hash| {
            asked.push(hash);
            MaterialData::default()
        });
        assert_eq!(asked, [1, 2]);
        assert_eq!(mesh.parts.len(), 2);
        assert_eq!(mesh.parts[0].index_count, 6, "a strip of 4: two triangles");
        assert_eq!(mesh.parts[1].index_count, 6, "a card: two triangles");
        let leaf = &mesh.vertices[4];
        assert!(leaf.is_billboard() && leaf.part & !Vertex::BILLBOARD == 1);
        assert!((crate::render::math::length(leaf.normal) - 2.0 * 2f32.sqrt()).abs() < 1e-4);
        // The card's box holds every way it can turn.
        assert!(mesh.parts[1].bounds.max[1] >= 5.0 + 2.0);
        assert!(!mesh.vertices[0].is_billboard());
    }
}
