//! Havok collision in the viewers: wireframe lines for the GPU scenes (model
//! and level view overlays) and a standalone wireframe preview of `hkx`
//! resources drawn with the egui painter.

use doctor_toolkit::game::StreamFile;
use doctor_toolkit::import::collision::{collision_references, mesh_references};
use doctor_toolkit::resource::hkx::geometry::{TriangleMesh, physics_mesh};
use doctor_toolkit::resource::hkx::physics::{Physics, Shape, ShapeKind};
use doctor_toolkit::resource::hkx::{Packfile, TYPE_HKX};
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{Color32, Pos2, RichText, Sense, Stroke, Vec2};

use crate::render::{LineVertex, OrbitCamera, math};

/// Colour of collision wireframes.
pub const COLOR: [u8; 4] = [90, 255, 140, 255];

/// At most this many edges are drawn per collision.
const MAX_EDGES: usize = 200_000;

/// A collision as edges, ready to be drawn.
#[derive(Debug, Clone, Default)]
pub struct Wire {
    pub positions: Vec<[f32; 3]>,
    pub edges: Vec<[u32; 2]>,
    pub triangles: usize,
}

impl Wire {
    pub fn new(mesh: &TriangleMesh) -> Self {
        let mut edges = mesh.edges();
        edges.truncate(MAX_EDGES);
        Self {
            positions: mesh.positions.clone(),
            edges,
            triangles: mesh.triangles.len(),
        }
    }

    pub fn bounds(&self) -> math::Aabb {
        let mut b = math::Aabb::EMPTY;
        for &p in &self.positions {
            b.add_point(p);
        }
        b
    }

    /// Line vertices, every point mapped by the row-major `matrix` (row vectors).
    pub fn lines(&self, matrix: Option<&[[f32; 4]; 4]>) -> Vec<LineVertex> {
        let map = |p: [f32; 3]| match matrix {
            None => p,
            Some(m) => {
                [0, 1, 2].map(|j| p[0] * m[0][j] + p[1] * m[1][j] + p[2] * m[2][j] + m[3][j])
            }
        };
        let mapped: Vec<[f32; 3]> = self.positions.iter().map(|&p| map(p)).collect();
        self.edges
            .iter()
            .flat_map(|e| {
                [
                    LineVertex::new(mapped[e[0] as usize], COLOR),
                    LineVertex::new(mapped[e[1] as usize], COLOR),
                ]
            })
            .collect()
    }
}

/// The collision a mesh of `stream` has through a metamodel of the same
/// stream that shows it: `None` when no metamodel there has collision.
pub fn for_mesh(stream: &StreamFile, mesh: usize) -> Option<Result<Wire, String>> {
    let layout = stream.layout();
    let ids = layout.resources.get(mesh)?.header.ids;
    let metamodel_type = doctor_toolkit::hash::sdbm(b"metamodel");
    for (i, r) in layout.resources.iter().enumerate() {
        if r.header.type_id != metamodel_type {
            continue;
        }
        let Ok(data) = stream.resource_data(i) else {
            continue;
        };
        if !mesh_references(&data).is_ok_and(|m| m.contains(&ids)) {
            continue;
        }
        let Ok(refs) = collision_references(&data) else {
            continue;
        };
        // The first collision is the one of the initial state.
        let Some(first) = refs.first() else { continue };
        let Some(h) = layout
            .resources
            .iter()
            .position(|x| x.header.type_id == TYPE_HKX && x.header.ids == *first)
        else {
            return Some(Err(trf(
                "collision.other_stream",
                &[
                    ("collision", &format!("{:08x}", first[3])),
                    ("metamodel", &format!("{:08x}", r.header.ids[3])),
                ],
            )));
        };
        return Some(
            stream
                .resource_data(h)
                .map_err(|e| e.to_string())
                .and_then(|d| Physics::parse(&d).map_err(|e| e.to_string()))
                .map(|p| Wire::new(&physics_mesh(&p))),
        );
    }
    None
}

/// Preview of an `hkx` resource: bodies and shapes, and a wireframe.
pub struct CollisionPreview {
    packfile: Packfile,
    physics: Result<Physics, String>,
    wire: Wire,
    camera: OrbitCamera,
}

impl CollisionPreview {
    pub fn new(packfile: Packfile) -> Self {
        let physics = Physics::from_packfile(&packfile).map_err(|e| e.to_string());
        let wire = physics
            .as_ref()
            .map(|p| Wire::new(&physics_mesh(p)))
            .unwrap_or_default();
        let camera = OrbitCamera::framing(&wire.bounds());
        Self {
            packfile,
            physics,
            wire,
            camera,
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        let p = &self.packfile;
        ui.strong(trf(
            "collision.packfile",
            &[("version", &p.contents_version), ("root", &p.root_class)],
        ));
        match &self.physics {
            Err(e) => {
                ui.colored_label(
                    Color32::YELLOW,
                    trf("collision.shapes_not_read", &[("error", e)]),
                );
            }
            Ok(physics) => {
                for body in physics.bodies() {
                    ui.monospace(trf(
                        "collision.body",
                        &[
                            (
                                "name",
                                &body.name.as_deref().unwrap_or(tr("collision.unnamed_body")),
                            ),
                            ("motion", &body.motion_type),
                            ("shape", &describe(&body.shape)),
                        ],
                    ));
                }
                if let Some(materials) = &physics.materials {
                    ui.label(
                        RichText::new(trf(
                            "collision.materials",
                            &[
                                ("count", &materials.len()),
                                (
                                    "list",
                                    &materials
                                        .iter()
                                        .map(|m| {
                                            m.map_or("-".into(), |m| format!("{}", m.words[0]))
                                        })
                                        .collect::<Vec<_>>()
                                        .join(", "),
                                ),
                            ],
                        ))
                        .weak(),
                    );
                }
            }
        }
        ui.label(
            RichText::new(trf(
                "collision.shown",
                &[
                    (
                        "triangles",
                        &trn("count.triangles", self.wire.triangles as u64, &[]),
                    ),
                    (
                        "edges",
                        &trn("count.edges", self.wire.edges.len() as u64, &[]),
                    ),
                ],
            ))
            .weak()
            .small(),
        );
        let width = ui.available_width().max(64.0);
        let size = egui::vec2(width, (width * 0.6).clamp(160.0, 520.0));
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        self.camera.handle_input(ui, &response);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::from_gray(24));
        let view = self.camera.view(rect.aspect_ratio());
        let screen: Vec<Option<Pos2>> = self
            .wire
            .positions
            .iter()
            .map(|&p| {
                view.project(p)
                    .map(|(uv, _)| rect.min + Vec2::new(uv[0], uv[1]) * rect.size())
            })
            .collect();
        let [r, g, b, _] = COLOR;
        let stroke = Stroke::new(1.0, Color32::from_rgb(r, g, b));
        for e in &self.wire.edges {
            if let (Some(a), Some(b)) = (screen[e[0] as usize], screen[e[1] as usize]) {
                painter.line_segment([a, b], stroke);
            }
        }
        let mut counts = std::collections::BTreeMap::<&str, usize>::new();
        for o in &self.packfile.objects {
            *counts.entry(o.class.as_str()).or_default() += 1;
        }
        egui::CollapsingHeader::new(tr("collision.objects"))
            .id_salt("hkx objects")
            .show(ui, |ui| {
                for (class, n) in counts {
                    ui.monospace(format!("{n:5}  {class}"));
                }
            });
    }
}

/// A one-line description of a shape tree.
fn describe(shape: &Shape) -> String {
    match &shape.kind {
        ShapeKind::Sphere { radius } => trf("collision.sphere", &[("radius", radius)]),
        ShapeKind::Box { half_extents, .. } => trf(
            "collision.box",
            &[(
                "size",
                &format!(
                    "{:.2}×{:.2}×{:.2}",
                    2.0 * half_extents[0],
                    2.0 * half_extents[1],
                    2.0 * half_extents[2]
                ),
            )],
        ),
        ShapeKind::Capsule { radius, .. } => trf("collision.capsule", &[("radius", radius)]),
        ShapeKind::Cylinder {
            cylinder_radius, ..
        } => trf("collision.cylinder", &[("radius", cylinder_radius)]),
        ShapeKind::ConvexVertices {
            vertices, planes, ..
        } => trf(
            "collision.convex",
            &[
                (
                    "vertices",
                    &trn("count.vertices", vertices.len() as u64, &[]),
                ),
                ("planes", &trn("count.planes", planes.len() as u64, &[])),
            ],
        ),
        ShapeKind::ConvexTranslate { child, .. } => {
            trf("collision.translated", &[("shape", &describe(child))])
        }
        ShapeKind::ConvexTransform { child, .. } => {
            trf("collision.transformed", &[("shape", &describe(child))])
        }
        ShapeKind::List { children, .. } => trf("collision.list", &[("count", &children.len())]),
        ShapeKind::Mopp { code, child } => trf(
            "collision.mopp",
            &[
                ("bytes", &trn("count.bytes", code.data.len() as u64, &[])),
                ("shape", &describe(child)),
            ],
        ),
        ShapeKind::SimpleMesh { triangles, .. } => trf(
            "collision.simple_mesh",
            &[(
                "triangles",
                &trn("count.triangles", triangles.len() as u64, &[]),
            )],
        ),
        ShapeKind::ExtendedMesh(m) => trf(
            "collision.extended_mesh",
            &[
                ("triangle_parts", &m.triangle_subparts.len()),
                ("shape_parts", &m.shape_subparts.len()),
            ],
        ),
    }
}
