//! [`Scene`] → binary FBX 7.4 (the version most tools read).
//!
//! Y up, one unit = one metre (`UnitScaleFactor` 100). One mesh model and
//! geometry per part; the skeleton as `LimbNode` models with node
//! attributes, a skin with one cluster per joint and a bind pose; materials
//! (Phong) with textures embedded in `Video` nodes; one animation stack per
//! clip with linear Euler XYZ curves. Texture V is flipped (FBX has the
//! origin at the bottom left).

use std::collections::HashMap;

use super::node::{CREATION_TIME, FILE_ID, Node, Prop, write_binary};
use super::read::KTIME;
use crate::Result;
use crate::scene::math::{self, IDENTITY};
use crate::scene::{ImageFiles, Scene, SceneImage};

pub const VERSION: u32 = 7400;

fn s(text: &str) -> Prop {
    Prop::str(text)
}

/// `Name\0\x01Class`.
fn named(name: &str, class: &str) -> Prop {
    let mut bytes = name.replace('\0', "_").into_bytes();
    bytes.extend_from_slice(b"\0\x01");
    bytes.extend_from_slice(class.as_bytes());
    Prop::String(bytes)
}

fn leaf(name: &str, value: Prop) -> Node {
    Node::new(name, vec![value])
}

/// One `P` entry of a `Properties70` block.
fn p(name: &str, kind: &str, label: &str, flags: &str, values: Vec<Prop>) -> Node {
    let mut props = vec![s(name), s(kind), s(label), s(flags)];
    props.extend(values);
    Node::new("P", props)
}

fn p_int(name: &str, value: i32) -> Node {
    p(name, "int", "Integer", "", vec![Prop::I32(value)])
}

fn p_enum(name: &str, value: i32) -> Node {
    p(name, "enum", "", "", vec![Prop::I32(value)])
}

fn p_double(name: &str, value: f64) -> Node {
    p(name, "double", "Number", "", vec![Prop::F64(value)])
}

fn p_vec(name: &str, kind: &str, label: &str, flags: &str, v: [f64; 3]) -> Node {
    p(
        name,
        kind,
        label,
        flags,
        v.iter().map(|&x| Prop::F64(x)).collect(),
    )
}

fn p_time(name: &str, value: i64) -> Node {
    p(name, "KTime", "Time", "", vec![Prop::I64(value)])
}

fn properties(entries: Vec<Node>) -> Node {
    Node::new("Properties70", vec![]).with(entries)
}

fn matrix(m: &[f32; 16]) -> Prop {
    Prop::F64s(m.iter().map(|&x| x as f64).collect())
}

struct Writer {
    next_id: i64,
    objects: Vec<Node>,
    connections: Vec<Node>,
    counts: HashMap<&'static str, usize>,
}

impl Writer {
    fn id(&mut self) -> i64 {
        self.next_id += 1;
        self.next_id
    }

    fn object(&mut self, class: &'static str, node: Node) {
        *self.counts.entry(class).or_insert(0) += 1;
        self.objects.push(node);
    }

    fn connect(&mut self, child: i64, parent: i64) {
        self.connections.push(Node::new(
            "C",
            vec![s("OO"), Prop::I64(child), Prop::I64(parent)],
        ));
    }

    fn connect_property(&mut self, child: i64, parent: i64, property: &str) {
        self.connections.push(Node::new(
            "C",
            vec![s("OP"), Prop::I64(child), Prop::I64(parent), s(property)],
        ));
    }
}

/// Euler XYZ angles (degrees) of a local transform, its translation and scale.
fn trs(m: &[f32; 16]) -> ([f64; 3], [f64; 3], [f64; 3]) {
    let (t, r, sc) = math::decompose(m);
    let e = math::quat_euler_xyz(r);
    (
        t.map(|x| x as f64),
        e.map(|x| x as f64),
        sc.map(|x| x as f64),
    )
}

fn model_node(id: i64, name: &str, kind: &str, local: &[f32; 16]) -> Node {
    let (t, r, sc) = trs(local);
    let mut entries = vec![p_enum("InheritType", 1), p_int("DefaultAttributeIndex", 0)];
    entries.push(p_vec("Lcl Translation", "Lcl Translation", "", "A", t));
    entries.push(p_vec("Lcl Rotation", "Lcl Rotation", "", "A", r));
    entries.push(p_vec("Lcl Scaling", "Lcl Scaling", "", "A", sc));
    Node::new("Model", vec![Prop::I64(id), named(name, "Model"), s(kind)]).with(vec![
        leaf("Version", Prop::I32(232)),
        properties(entries),
        leaf("MultiLayer", Prop::I32(0)),
        leaf("MultiTake", Prop::I32(0)),
        leaf("Shading", Prop::Bool(true)),
        leaf("Culling", s("CullingOff")),
    ])
}

/// Writes a scene as binary FBX 7.4.
pub fn write(scene: &Scene) -> Result<Vec<u8>> {
    let mut w = Writer {
        next_id: 1_000_000,
        objects: Vec::new(),
        connections: Vec::new(),
        counts: HashMap::new(),
    };
    let images = ImageFiles::new(scene, "textures/");

    // Skeleton.
    let mut joint_ids = Vec::with_capacity(scene.joints.len());
    for joint in &scene.joints {
        let id = w.id();
        let attribute = w.id();
        joint_ids.push(id);
        w.object(
            "NodeAttribute",
            Node::new(
                "NodeAttribute",
                vec![
                    Prop::I64(attribute),
                    named(&joint.name, "NodeAttribute"),
                    s("LimbNode"),
                ],
            )
            .with(vec![
                properties(vec![p_double("Size", 0.05)]),
                leaf("TypeFlags", s("Skeleton")),
            ]),
        );
        w.object(
            "Model",
            model_node(id, &joint.name, "LimbNode", &joint.local),
        );
        w.connect(attribute, id);
    }
    for (i, joint) in scene.joints.iter().enumerate() {
        let parent = joint.parent.map_or(0, |p| joint_ids[p]);
        w.connect(joint_ids[i], parent);
    }
    let joint_worlds: Vec<[f32; 16]> = scene
        .joints
        .iter()
        .map(|j| math::invert(&j.inverse_bind))
        .collect();

    // Materials and textures.
    let mut video_ids: HashMap<String, i64> = HashMap::new();
    let mut texture_ids: HashMap<(String, &'static str), i64> = HashMap::new();
    let mut material_ids = Vec::with_capacity(scene.materials.len());
    let material_names =
        crate::scene::obj::unique_names(scene.materials.iter().map(|m| m.name.as_str()));
    for (material, name) in scene.materials.iter().zip(&material_names) {
        let id = w.id();
        material_ids.push(id);
        let f = material.base_color_factor;
        w.object(
            "Material",
            Node::new(
                "Material",
                vec![Prop::I64(id), named(name, "Material"), s("")],
            )
            .with(vec![
                leaf("Version", Prop::I32(102)),
                leaf("ShadingModel", s("phong")),
                leaf("MultiLayer", Prop::I32(0)),
                properties(vec![
                    p("ShadingModel", "KString", "", "", vec![s("phong")]),
                    p_vec(
                        "DiffuseColor",
                        "Color",
                        "",
                        "A",
                        [f[0] as f64, f[1] as f64, f[2] as f64],
                    ),
                    p("DiffuseFactor", "Number", "", "A", vec![Prop::F64(1.0)]),
                    p(
                        "Opacity",
                        "double",
                        "Number",
                        "",
                        vec![Prop::F64(f[3] as f64)],
                    ),
                    p("SpecularFactor", "Number", "", "A", vec![Prop::F64(0.0)]),
                ]),
            ]),
        );
        for (role, property, image) in [
            ("diffuse", "DiffuseColor", &material.base_color),
            ("normal", "NormalMap", &material.normal),
            ("specular", "SpecularColor", &material.specular),
        ] {
            let Some(image) = image else { continue };
            let Some(file) = images.name(image).map(str::to_owned) else {
                continue;
            };
            let texture =
                texture_node(&mut w, &mut video_ids, &mut texture_ids, image, &file, role);
            w.connect_property(texture, id, property);
        }
    }

    // Meshes.
    let mut mesh_ids = Vec::new();
    for part in &scene.parts {
        let g = &part.geometry;
        if g.triangles.is_empty() {
            continue;
        }
        let model = w.id();
        let geometry = w.id();
        mesh_ids.push(model);
        w.object("Model", model_node(model, &part.name, "Mesh", &IDENTITY));
        w.connect(model, 0);
        let vertices: Vec<f64> = g.positions.iter().flatten().map(|&x| x as f64).collect();
        let polygons: Vec<i32> = g
            .triangles
            .iter()
            .flat_map(|t| [t[0] as i32, t[1] as i32, !(t[2] as i32)])
            .collect();
        let corners: Vec<i32> = g.triangles.iter().flatten().map(|&i| i as i32).collect();
        let mut children = vec![
            properties(vec![]),
            leaf("GeometryVersion", Prop::I32(124)),
            leaf("Vertices", Prop::F64s(vertices)),
            leaf("PolygonVertexIndex", Prop::I32s(polygons)),
        ];
        let mut layer0 = vec![];
        let mut layer1 = vec![];
        let element = |kind: &str, index: i32| {
            Node::new("LayerElement", vec![]).with(vec![
                leaf("Type", s(kind)),
                leaf("TypedIndex", Prop::I32(index)),
            ])
        };
        if let Some(normals) = &g.normals {
            children.push(
                Node::new("LayerElementNormal", vec![Prop::I32(0)]).with(vec![
                    leaf("Version", Prop::I32(102)),
                    leaf("Name", s("")),
                    leaf("MappingInformationType", s("ByVertice")),
                    leaf("ReferenceInformationType", s("Direct")),
                    leaf(
                        "Normals",
                        Prop::F64s(normals.iter().flatten().map(|&x| x as f64).collect()),
                    ),
                ]),
            );
            layer0.push(element("LayerElementNormal", 0));
        }
        for (k, uv) in [&g.uv0, &g.uv1].into_iter().enumerate() {
            let Some(uv) = uv else { continue };
            children.push(
                Node::new("LayerElementUV", vec![Prop::I32(k as i32)]).with(vec![
                    leaf("Version", Prop::I32(101)),
                    leaf("Name", s(if k == 0 { "UVMap" } else { "UVMap.001" })),
                    leaf("MappingInformationType", s("ByPolygonVertex")),
                    leaf("ReferenceInformationType", s("IndexToDirect")),
                    leaf(
                        "UV",
                        Prop::F64s(
                            uv.iter()
                                .flat_map(|t| [t[0] as f64, 1.0 - t[1] as f64])
                                .collect(),
                        ),
                    ),
                    leaf("UVIndex", Prop::I32s(corners.clone())),
                ]),
            );
            if k == 0 {
                layer0.push(element("LayerElementUV", 0));
            } else {
                layer1.push(element("LayerElementUV", 1));
            }
        }
        if let Some(colors) = &g.colors {
            children.push(
                Node::new("LayerElementColor", vec![Prop::I32(0)]).with(vec![
                    leaf("Version", Prop::I32(101)),
                    leaf("Name", s("Col")),
                    leaf("MappingInformationType", s("ByPolygonVertex")),
                    leaf("ReferenceInformationType", s("IndexToDirect")),
                    leaf(
                        "Colors",
                        Prop::F64s(
                            colors
                                .iter()
                                .flatten()
                                .map(|&c| (c as f64 / 255.0 * 1e6).round() / 1e6)
                                .collect(),
                        ),
                    ),
                    leaf("ColorIndex", Prop::I32s(corners.clone())),
                ]),
            );
            layer0.push(element("LayerElementColor", 0));
        }
        children.push(
            Node::new("LayerElementMaterial", vec![Prop::I32(0)]).with(vec![
                leaf("Version", Prop::I32(101)),
                leaf("Name", s("")),
                leaf("MappingInformationType", s("AllSame")),
                leaf("ReferenceInformationType", s("IndexToDirect")),
                leaf("Materials", Prop::I32s(vec![0])),
            ]),
        );
        layer0.push(element("LayerElementMaterial", 0));
        let mut layer = vec![leaf("Version", Prop::I32(100))];
        layer.extend(layer0);
        children.push(Node::new("Layer", vec![Prop::I32(0)]).with(layer));
        if !layer1.is_empty() {
            let mut layer = vec![leaf("Version", Prop::I32(100))];
            layer.extend(layer1);
            children.push(Node::new("Layer", vec![Prop::I32(1)]).with(layer));
        }
        w.object(
            "Geometry",
            Node::new(
                "Geometry",
                vec![
                    Prop::I64(geometry),
                    named(&part.name, "Geometry"),
                    s("Mesh"),
                ],
            )
            .with(children),
        );
        w.connect(geometry, model);
        if let Some(m) = part.material.and_then(|m| material_ids.get(m)) {
            w.connect(*m, model);
        }
        // Skin.
        if let Some(skin) = g.skin.as_ref().filter(|_| !scene.joints.is_empty()) {
            let mut per_joint: Vec<(Vec<i32>, Vec<f64>)> =
                vec![Default::default(); scene.joints.len()];
            for (v, (joints, weights)) in skin.iter().enumerate() {
                for k in 0..4 {
                    let j = joints[k] as usize;
                    if weights[k] > 0.0 && j < per_joint.len() {
                        per_joint[j].0.push(v as i32);
                        per_joint[j].1.push(weights[k] as f64);
                    }
                }
            }
            let skin_id = w.id();
            w.object(
                "Deformer",
                Node::new(
                    "Deformer",
                    vec![Prop::I64(skin_id), named(&part.name, "Deformer"), s("Skin")],
                )
                .with(vec![
                    leaf("Version", Prop::I32(101)),
                    leaf("Link_DeformAcuracy", Prop::F64(50.0)),
                    leaf("SkinningType", s("Linear")),
                ]),
            );
            w.connect(skin_id, geometry);
            for (j, (indexes, weights)) in per_joint.into_iter().enumerate() {
                if indexes.is_empty() {
                    continue;
                }
                let cluster = w.id();
                let link = joint_worlds[j];
                // Geometry → joint space at bind time (the mesh is at the origin).
                let transform = scene.joints[j].inverse_bind;
                w.object(
                    "Deformer",
                    Node::new(
                        "Deformer",
                        vec![
                            Prop::I64(cluster),
                            named(&scene.joints[j].name, "SubDeformer"),
                            s("Cluster"),
                        ],
                    )
                    .with(vec![
                        leaf("Version", Prop::I32(100)),
                        Node::new("UserData", vec![s(""), s("")]),
                        leaf("Indexes", Prop::I32s(indexes)),
                        leaf("Weights", Prop::F64s(weights)),
                        leaf("Transform", matrix(&transform)),
                        leaf("TransformLink", matrix(&link)),
                    ]),
                );
                w.connect(cluster, skin_id);
                w.connect(joint_ids[j], cluster);
            }
        }
    }

    // Bind pose.
    if !scene.joints.is_empty() {
        let pose = w.id();
        let mut nodes = vec![
            leaf("Type", s("BindPose")),
            leaf("Version", Prop::I32(100)),
            leaf(
                "NbPoseNodes",
                Prop::I32((mesh_ids.len() + scene.joints.len()) as i32),
            ),
        ];
        for &m in &mesh_ids {
            nodes.push(Node::new("PoseNode", vec![]).with(vec![
                leaf("Node", Prop::I64(m)),
                leaf("Matrix", matrix(&IDENTITY)),
            ]));
        }
        for (j, world) in joint_worlds.iter().enumerate() {
            nodes.push(Node::new("PoseNode", vec![]).with(vec![
                leaf("Node", Prop::I64(joint_ids[j])),
                leaf("Matrix", matrix(world)),
            ]));
        }
        w.object(
            "Pose",
            Node::new(
                "Pose",
                vec![Prop::I64(pose), named(&scene.name, "Pose"), s("BindPose")],
            )
            .with(nodes),
        );
    }

    // Animation.
    let rate = scene.clips.first().map_or(30.0, |c| {
        if c.frame_rate > 0.0 {
            c.frame_rate
        } else {
            30.0
        }
    });
    let mut longest = 0i64;
    let mut takes = vec![leaf(
        "Current",
        s(scene.clips.first().map_or("", |c| c.name.as_str())),
    )];
    for clip in &scene.clips {
        let frames = clip.frames();
        if frames == 0 {
            continue;
        }
        let clip_rate = if clip.frame_rate > 0.0 {
            clip.frame_rate
        } else {
            30.0
        } as f64;
        let stop = ((frames - 1) as f64 / clip_rate * KTIME).round() as i64;
        longest = longest.max(stop);
        let stack = w.id();
        let layer = w.id();
        w.object(
            "AnimationStack",
            Node::new(
                "AnimationStack",
                vec![Prop::I64(stack), named(&clip.name, "AnimStack"), s("")],
            )
            .with(vec![properties(vec![
                p_time("LocalStart", 0),
                p_time("LocalStop", stop),
                p_time("ReferenceStart", 0),
                p_time("ReferenceStop", stop),
            ])]),
        );
        w.object(
            "AnimationLayer",
            Node::new(
                "AnimationLayer",
                vec![Prop::I64(layer), named("BaseLayer", "AnimLayer"), s("")],
            ),
        );
        w.connect(layer, stack);
        takes.push(Node::new("Take", vec![s(&clip.name)]).with(vec![
            leaf(
                "FileName",
                s(&format!("{}.tak", crate::scene::file_name(&clip.name))),
            ),
            Node::new("LocalTime", vec![Prop::I64(0), Prop::I64(stop)]),
            Node::new("ReferenceTime", vec![Prop::I64(0), Prop::I64(stop)]),
        ]));
        for track in &clip.tracks {
            let Some(&model) = joint_ids.get(track.joint) else {
                continue;
            };
            let times = |n: usize| -> Vec<i64> {
                (0..n)
                    .map(|f| (f as f64 / clip_rate * KTIME).round() as i64)
                    .collect()
            };
            let mut channels: Vec<(&str, &str, Vec<[f64; 3]>)> = Vec::new();
            if !track.translations.is_empty() {
                channels.push((
                    "T",
                    "Lcl Translation",
                    track
                        .translations
                        .iter()
                        .map(|t| t.map(|x| x as f64))
                        .collect(),
                ));
            }
            if !track.rotations.is_empty() {
                channels.push(("R", "Lcl Rotation", unwrapped_euler(&track.rotations)));
            }
            if !track.scales.is_empty() {
                channels.push((
                    "S",
                    "Lcl Scaling",
                    track.scales.iter().map(|t| t.map(|x| x as f64)).collect(),
                ));
            }
            for (short, property, values) in channels {
                let cnode = w.id();
                let first = values[0];
                w.object(
                    "AnimationCurveNode",
                    Node::new(
                        "AnimationCurveNode",
                        vec![Prop::I64(cnode), named(short, "AnimCurveNode"), s("")],
                    )
                    .with(vec![properties(vec![
                        p("d|X", "Number", "", "A", vec![Prop::F64(first[0])]),
                        p("d|Y", "Number", "", "A", vec![Prop::F64(first[1])]),
                        p("d|Z", "Number", "", "A", vec![Prop::F64(first[2])]),
                    ])]),
                );
                w.connect(cnode, layer);
                w.connect_property(cnode, model, property);
                let key_times = times(values.len());
                for (axis, channel) in ["d|X", "d|Y", "d|Z"].iter().enumerate() {
                    let curve = w.id();
                    let v: Vec<f32> = values.iter().map(|x| x[axis] as f32).collect();
                    w.object(
                        "AnimationCurve",
                        Node::new(
                            "AnimationCurve",
                            vec![Prop::I64(curve), named("", "AnimCurve"), s("")],
                        )
                        .with(vec![
                            leaf("Default", Prop::F64(first[axis])),
                            leaf("KeyVer", Prop::I32(4008)),
                            leaf("KeyTime", Prop::I64s(key_times.clone())),
                            leaf("KeyValueFloat", Prop::F32s(v)),
                            leaf("KeyAttrFlags", Prop::I32s(vec![4])),
                            leaf("KeyAttrDataFloat", Prop::F32s(vec![0.0; 4])),
                            leaf("KeyAttrRefCount", Prop::I32s(vec![values.len() as i32])),
                        ]),
                    );
                    w.connect_property(curve, cnode, channel);
                }
            }
        }
    }

    // Header, settings, definitions.
    let creator = concat!("doctor_toolkit ", env!("CARGO_PKG_VERSION"));
    let header = Node::new("FBXHeaderExtension", vec![]).with(vec![
        leaf("FBXHeaderVersion", Prop::I32(1003)),
        leaf("FBXVersion", Prop::I32(VERSION as i32)),
        leaf("EncryptionType", Prop::I32(0)),
        Node::new("CreationTimeStamp", vec![]).with(vec![
            leaf("Version", Prop::I32(1000)),
            leaf("Year", Prop::I32(1970)),
            leaf("Month", Prop::I32(1)),
            leaf("Day", Prop::I32(1)),
            leaf("Hour", Prop::I32(10)),
            leaf("Minute", Prop::I32(0)),
            leaf("Second", Prop::I32(0)),
            leaf("Millisecond", Prop::I32(0)),
        ]),
        leaf("Creator", s(creator)),
    ]);
    let (time_mode, custom) = if (rate - 30.0).abs() < 1e-3 {
        (6, 30.0)
    } else if (rate - 24.0).abs() < 1e-3 {
        (11, 24.0)
    } else if (rate - 60.0).abs() < 1e-3 {
        (3, 60.0)
    } else {
        (14, rate as f64)
    };
    let settings = Node::new("GlobalSettings", vec![]).with(vec![
        leaf("Version", Prop::I32(1000)),
        properties(vec![
            p_int("UpAxis", 1),
            p_int("UpAxisSign", 1),
            p_int("FrontAxis", 2),
            p_int("FrontAxisSign", 1),
            p_int("CoordAxis", 0),
            p_int("CoordAxisSign", 1),
            p_int("OriginalUpAxis", 1),
            p_int("OriginalUpAxisSign", 1),
            p_double("UnitScaleFactor", 100.0),
            p_double("OriginalUnitScaleFactor", 100.0),
            p_vec("AmbientColor", "ColorRGB", "Color", "", [0.0; 3]),
            p(
                "DefaultCamera",
                "KString",
                "",
                "",
                vec![s("Producer Perspective")],
            ),
            p_enum("TimeMode", time_mode),
            p_enum("TimeProtocol", 2),
            p_enum("SnapOnFrameMode", 0),
            p_time("TimeSpanStart", 0),
            p_time("TimeSpanStop", longest.max(KTIME as i64)),
            p_double("CustomFrameRate", custom),
            p("TimeMarker", "Compound", "", "", vec![]),
            p_int("CurrentTimeMarker", -1),
        ]),
    ]);
    let document_id = w.id();
    let documents = Node::new("Documents", vec![]).with(vec![
        leaf("Count", Prop::I32(1)),
        Node::new(
            "Document",
            vec![Prop::I64(document_id), s("Scene"), s("Scene")],
        )
        .with(vec![
            properties(vec![
                p("SourceObject", "object", "", "", vec![]),
                p(
                    "ActiveAnimStackName",
                    "KString",
                    "",
                    "",
                    vec![s(scene.clips.first().map_or("", |c| c.name.as_str()))],
                ),
            ]),
            leaf("RootNode", Prop::I64(0)),
        ]),
    ]);
    let mut types: Vec<(&str, usize)> = w.counts.iter().map(|(k, v)| (*k, *v)).collect();
    types.sort();
    let total: usize = types.iter().map(|t| t.1).sum::<usize>() + 1;
    let mut definitions = vec![
        leaf("Version", Prop::I32(100)),
        leaf("Count", Prop::I32(total as i32)),
        Node::new("ObjectType", vec![s("GlobalSettings")]).with(vec![leaf("Count", Prop::I32(1))]),
    ];
    for (kind, count) in types {
        definitions.push(
            Node::new("ObjectType", vec![s(kind)])
                .with(vec![leaf("Count", Prop::I32(count as i32))]),
        );
    }
    let nodes = vec![
        header,
        leaf("FileId", Prop::Bytes(FILE_ID.to_vec())),
        leaf("CreationTime", s(CREATION_TIME)),
        leaf("Creator", s(creator)),
        settings,
        documents,
        Node::new("References", vec![]),
        Node::new("Definitions", vec![]).with(definitions),
        Node::new("Objects", vec![]).with(w.objects),
        Node::new("Connections", vec![]).with(w.connections),
        Node::new("Takes", vec![]).with(takes),
    ];
    Ok(write_binary(&nodes, VERSION))
}

fn texture_node(
    w: &mut Writer,
    videos: &mut HashMap<String, i64>,
    textures: &mut HashMap<(String, &'static str), i64>,
    image: &SceneImage,
    file: &str,
    role: &'static str,
) -> i64 {
    if let Some(&id) = textures.get(&(file.to_owned(), role)) {
        return id;
    }
    let video = match videos.get(file) {
        Some(&v) => v,
        None => {
            let v = w.id();
            w.object(
                "Video",
                Node::new(
                    "Video",
                    vec![Prop::I64(v), named(&image.name, "Video"), s("Clip")],
                )
                .with(vec![
                    leaf("Type", s("Clip")),
                    properties(vec![p("Path", "KString", "XRefUrl", "", vec![s(file)])]),
                    leaf("UseMipMap", Prop::I32(0)),
                    leaf("Filename", s(file)),
                    leaf("RelativeFilename", s(file)),
                    leaf("Content", Prop::Bytes(image.bytes.clone())),
                ]),
            );
            videos.insert(file.to_owned(), v);
            v
        }
    };
    let id = w.id();
    w.object(
        "Texture",
        Node::new(
            "Texture",
            vec![Prop::I64(id), named(&image.name, "Texture"), s("")],
        )
        .with(vec![
            leaf("Type", s("TextureVideoClip")),
            leaf("Version", Prop::I32(202)),
            leaf("TextureName", named(&image.name, "Texture")),
            properties(vec![p("UseMaterial", "bool", "", "", vec![Prop::I32(1)])]),
            leaf("Media", named(&image.name, "Video")),
            leaf("FileName", s(file)),
            leaf("RelativeFilename", s(file)),
            Node::new("ModelUVTranslation", vec![Prop::F64(0.0), Prop::F64(0.0)]),
            Node::new("ModelUVScaling", vec![Prop::F64(1.0), Prop::F64(1.0)]),
            leaf("Texture_Alpha_Source", s("None")),
            Node::new(
                "Cropping",
                vec![Prop::I32(0), Prop::I32(0), Prop::I32(0), Prop::I32(0)],
            ),
        ]),
    );
    w.connect(video, id);
    textures.insert((file.to_owned(), role), id);
    id
}

/// Euler XYZ angles of quaternions, each channel kept within 180° of the
/// previous frame.
fn unwrapped_euler(rotations: &[[f32; 4]]) -> Vec<[f64; 3]> {
    let mut out: Vec<[f64; 3]> = Vec::with_capacity(rotations.len());
    for q in rotations {
        let e = math::quat_euler_xyz(*q).map(|x| x as f64);
        // The same rotation as (x + 180, 180 - y, z + 180).
        let alt = [e[0] + 180.0, 180.0 - e[1], e[2] + 180.0];
        let next = match out.last() {
            None => e,
            Some(prev) => {
                let near = |v: [f64; 3]| -> [f64; 3] {
                    std::array::from_fn(|i| v[i] - 360.0 * ((v[i] - prev[i]) / 360.0).round())
                };
                let (a, b) = (near(e), near(alt));
                let dist = |v: [f64; 3]| (0..3).map(|i| (v[i] - prev[i]).abs()).sum::<f64>();
                if dist(b) < dist(a) { b } else { a }
            }
        };
        out.push(next);
    }
    out
}
