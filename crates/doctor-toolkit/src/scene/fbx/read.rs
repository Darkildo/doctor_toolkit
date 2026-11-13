//! FBX 7.x document → [`Scene`].
//!
//! - Units and axes come from `GlobalSettings` (`UnitScaleFactor` in
//!   centimetres per unit, `UpAxis`/`FrontAxis`/`CoordAxis` with signs) and
//!   are converted to Y up, metres.
//! - Node transforms follow the FBX formula
//!   `T · Roff · Rp · Rpre · R · Rpost⁻¹ · Rp⁻¹ · Soff · Sp · S · Sp⁻¹`,
//!   with property templates from `Definitions`; geometric transforms apply
//!   to the geometry only.
//! - Geometry: polygons are triangulated; layer elements (normals, UVs,
//!   colours, materials) may be mapped by polygon vertex, control point,
//!   polygon or all the same, directly or through an index. One part per
//!   model and material.
//! - Skins: clusters give joint weights per control point (the four largest
//!   are kept), `TransformLink` the joint's bind pose and
//!   `TransformLink · Transform` the mesh's.
//! - Joints: `LimbNode` models and every model a cluster links to.
//! - Animation: every stack is one clip (its first layer), sampled at the
//!   file's frame rate between `LocalStart` and `LocalStop`.

use std::collections::{HashMap, HashSet};

use super::node::{Document, Node, Prop};
use crate::resource::mesh_build::PartGeometry;
use crate::resource::rcb::Mat4;
use crate::scene::math::{self, EulerOrder, IDENTITY};
use crate::scene::{
    Corner, CornerBuilder, JointTrack, Scene, SceneClip, SceneImage, SceneJoint, SceneMaterial,
    ScenePart, SideFiles, top_four,
};
use crate::{Error, Result};

/// Most node transforms evaluated for one clip (frames × nodes).
const MAX_SAMPLES: usize = 50_000_000;

/// FBX time units per second.
pub const KTIME: f64 = 46_186_158_000.0;

fn bad(message: impl Into<String>) -> Error {
    Error::InvalidArgument(format!("FBX: {}", message.into()))
}

/// A `Properties70` block: name → values (after type, label and flags).
#[derive(Default, Clone)]
struct Props<'a> {
    own: HashMap<String, &'a [Prop]>,
    template: Option<&'a HashMap<String, &'a [Prop]>>,
}

fn props70(node: &Node) -> HashMap<String, &[Prop]> {
    let mut out = HashMap::new();
    for block in ["Properties70", "Properties60"] {
        if let Some(p) = node.child(block) {
            for entry in &p.children {
                if let Some(name) = entry.props.first().and_then(Prop::as_str) {
                    let values = entry.props.get(4..).unwrap_or(&[]);
                    out.insert(name.into_owned(), values);
                }
            }
        }
    }
    out
}

impl<'a> Props<'a> {
    fn get(&self, name: &str) -> Option<&'a [Prop]> {
        self.own
            .get(name)
            .copied()
            .or_else(|| self.template.and_then(|t| t.get(name).copied()))
    }

    fn vec3(&self, name: &str, default: [f64; 3]) -> [f64; 3] {
        match self.get(name) {
            Some(v) if v.len() >= 3 => {
                let mut out = default;
                for (k, o) in out.iter_mut().enumerate() {
                    if let Some(x) = v[k].as_f64() {
                        *o = x;
                    }
                }
                out
            }
            _ => default,
        }
    }

    fn f64(&self, name: &str, default: f64) -> f64 {
        self.get(name)
            .and_then(|v| v.first())
            .and_then(Prop::as_f64)
            .unwrap_or(default)
    }

    fn i64(&self, name: &str, default: i64) -> i64 {
        self.get(name)
            .and_then(|v| v.first())
            .and_then(Prop::as_i64)
            .unwrap_or(default)
    }
}

struct Object<'a> {
    id: i64,
    name: String,
    /// Node name: `Model`, `Geometry`, `Material`…
    class: String,
    /// Third property: `Mesh`, `LimbNode`, `Skin`, `Cluster`…
    kind: String,
    node: &'a Node,
}

/// `Name\0\x01Class` (binary) or `Class::Name` (ASCII) → `Name`.
fn object_name(raw: &str) -> String {
    if let Some((name, _)) = raw.split_once("\0\u{1}") {
        return name.to_owned();
    }
    if let Some((_, name)) = raw.split_once("::") {
        return name.to_owned();
    }
    raw.to_owned()
}

struct Graph<'a> {
    objects: HashMap<i64, Object<'a>>,
    /// parent → [(child, property)]
    children: HashMap<i64, Vec<(i64, Option<String>)>>,
    /// child → [(parent, property)]
    parents: HashMap<i64, Vec<(i64, Option<String>)>>,
    templates: HashMap<String, HashMap<String, &'a [Prop]>>,
}

impl<'a> Graph<'a> {
    fn new(doc: &'a Document) -> Result<Self> {
        let objects_node = doc
            .get("Objects")
            .ok_or_else(|| bad("no Objects section"))?;
        let mut objects = HashMap::new();
        for node in &objects_node.children {
            let Some(id) = node.props.first().and_then(Prop::as_i64) else {
                continue;
            };
            let name = node
                .props
                .get(1)
                .and_then(Prop::as_str)
                .map(|s| object_name(&s))
                .unwrap_or_default();
            let kind = node
                .props
                .get(2)
                .and_then(Prop::as_str)
                .map(|s| s.into_owned())
                .unwrap_or_default();
            objects.insert(
                id,
                Object {
                    id,
                    name,
                    class: node.name.clone(),
                    kind,
                    node,
                },
            );
        }
        let mut children: HashMap<i64, Vec<(i64, Option<String>)>> = HashMap::new();
        let mut parents: HashMap<i64, Vec<(i64, Option<String>)>> = HashMap::new();
        if let Some(connections) = doc.get("Connections") {
            for c in connections.children_named("C") {
                let (Some(child), Some(parent)) = (
                    c.props.get(1).and_then(Prop::as_i64),
                    c.props.get(2).and_then(Prop::as_i64),
                ) else {
                    continue;
                };
                let property = c
                    .props
                    .get(3)
                    .and_then(Prop::as_str)
                    .map(|s| s.into_owned());
                children
                    .entry(parent)
                    .or_default()
                    .push((child, property.clone()));
                parents.entry(child).or_default().push((parent, property));
            }
        }
        let mut templates = HashMap::new();
        if let Some(definitions) = doc.get("Definitions") {
            for object_type in definitions.children_named("ObjectType") {
                let Some(class) = object_type.props.first().and_then(Prop::as_str) else {
                    continue;
                };
                if let Some(template) = object_type.child("PropertyTemplate") {
                    templates.insert(class.into_owned(), props70(template));
                }
            }
        }
        Ok(Self {
            objects,
            children,
            parents,
            templates,
        })
    }

    fn props(&self, object: &Object<'a>) -> Props<'a> {
        Props {
            own: props70(object.node),
            template: None,
        }
    }

    fn props_with_template<'s>(&'s self, object: &Object<'a>) -> Props<'s>
    where
        'a: 's,
    {
        Props {
            own: props70(object.node),
            template: self.templates.get(&object.class),
        }
    }

    fn children_of(&self, id: i64, class: &str) -> Vec<(&Object<'a>, Option<&str>)> {
        self.children
            .get(&id)
            .into_iter()
            .flatten()
            .filter_map(|(c, p)| {
                let o = self.objects.get(c)?;
                (o.class == class).then_some((o, p.as_deref()))
            })
            .collect()
    }
}

/// File space → scene space: an orthogonal axis change and a unit scale.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Conversion {
    pub axes: Mat4,
    pub scale: f32,
}

impl Conversion {
    fn from_settings(props: &Props<'_>) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let axis = |which: &str, sign: &str, default: (i64, i64)| -> [f32; 3] {
            let a = props.i64(which, default.0).clamp(0, 2) as usize;
            let s = if props.i64(sign, default.1) < 0 {
                -1.0
            } else {
                1.0
            };
            let mut v = [0.0; 3];
            v[a] = s;
            v
        };
        let up = axis("UpAxis", "UpAxisSign", (1, 1));
        let front = axis("FrontAxis", "FrontAxisSign", (2, 1));
        let right = axis("CoordAxis", "CoordAxisSign", (0, 1));
        // Columns: where our X, Y, Z are in the file; the conversion is its inverse.
        let b: Mat4 = [
            right[0], right[1], right[2], 0.0, up[0], up[1], up[2], 0.0, front[0], front[1],
            front[2], 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        let det = math::determinant3(&b);
        let axes = if det.abs() < 0.5 {
            warnings.push("inconsistent axis system; axes are kept".into());
            IDENTITY
        } else {
            math::invert(&b)
        };
        if det < 0.0 {
            warnings.push("left-handed axis system: the model is mirrored".into());
        }
        let unit = props.f64("UnitScaleFactor", 1.0);
        let scale = if unit.is_finite() && unit > 0.0 {
            (unit / 100.0) as f32
        } else {
            0.01
        };
        (Self { axes, scale }, warnings)
    }

    /// A world matrix in file space → scene space (rotation axes changed,
    /// translations scaled).
    pub fn matrix(&self, m: &Mat4) -> Mat4 {
        let mut scaled = *m;
        for v in &mut scaled[12..15] {
            *v *= self.scale;
        }
        math::multiply(&self.axes, &scaled)
    }

    /// The transform applied to geometry in file world space.
    pub fn points(&self) -> Mat4 {
        math::multiply(&self.axes, &math::scaling([self.scale; 3]))
    }
}

/// Local transform of a model from its properties (optionally animated
/// translation, Euler rotation in degrees and scale).
fn node_local(props: &Props<'_>, anim: Option<&[[f64; 3]; 3]>) -> Mat4 {
    let v = |name: &str, d: [f64; 3]| props.vec3(name, d).map(|x| x as f32);
    let (t, r, s) = match anim {
        Some(a) => (
            a[0].map(|x| x as f32),
            a[1].map(|x| x as f32),
            a[2].map(|x| x as f32),
        ),
        None => (
            v("Lcl Translation", [0.0; 3]),
            v("Lcl Rotation", [0.0; 3]),
            v("Lcl Scaling", [1.0; 3]),
        ),
    };
    let active = props.i64("RotationActive", 0) != 0;
    let order = if active {
        EulerOrder::fbx(props.i64("RotationOrder", 0))
    } else {
        EulerOrder::XYZ
    };
    let quat = |deg: [f32; 3], o: EulerOrder| math::quat_matrix(math::euler_quat(deg, o));
    let pre = if active {
        quat(v("PreRotation", [0.0; 3]), EulerOrder::XYZ)
    } else {
        IDENTITY
    };
    let post = if active {
        math::invert(&quat(v("PostRotation", [0.0; 3]), EulerOrder::XYZ))
    } else {
        IDENTITY
    };
    let rp = v("RotationPivot", [0.0; 3]);
    let sp = v("ScalingPivot", [0.0; 3]);
    let chain = [
        math::translation(t),
        math::translation(v("RotationOffset", [0.0; 3])),
        math::translation(rp),
        pre,
        quat(r, order),
        post,
        math::translation(rp.map(|x| -x)),
        math::translation(v("ScalingOffset", [0.0; 3])),
        math::translation(sp),
        math::scaling(s),
        math::translation(sp.map(|x| -x)),
    ];
    chain
        .iter()
        .fold(IDENTITY, |acc, m| math::multiply(&acc, m))
}

fn geometric(props: &Props<'_>) -> Mat4 {
    let v = |name: &str, d: [f64; 3]| props.vec3(name, d).map(|x| x as f32);
    let t = math::translation(v("GeometricTranslation", [0.0; 3]));
    let r = math::quat_matrix(math::euler_quat(
        v("GeometricRotation", [0.0; 3]),
        EulerOrder::XYZ,
    ));
    let s = math::scaling(v("GeometricScaling", [1.0; 3]));
    math::multiply(&t, &math::multiply(&r, &s))
}

fn frame_rate(props: &Props<'_>) -> f32 {
    match props.i64("TimeMode", 0) {
        1 => 120.0,
        2 => 100.0,
        3 => 60.0,
        4 => 50.0,
        5 => 48.0,
        6 | 7 => 30.0,
        8 | 9 => 29.97,
        10 => 25.0,
        11 => 24.0,
        12 => 1000.0,
        13 => 23.976,
        14 => {
            let custom = props.f64("CustomFrameRate", 30.0) as f32;
            if custom.is_finite() && custom > 0.0 && custom <= 1000.0 {
                custom
            } else {
                30.0
            }
        }
        15 => 96.0,
        16 => 72.0,
        17 => 59.94,
        18 => 119.88,
        _ => 30.0,
    }
}

/// A layer element of a geometry: values and how they map to corners.
struct Layer {
    values: Vec<f64>,
    width: usize,
    index: Option<Vec<i64>>,
    mapping: Mapping,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mapping {
    PolygonVertex,
    ControlPoint,
    Polygon,
    AllSame,
}

impl Layer {
    fn read(node: &Node, values: &str, index: &str, width: usize) -> Option<Self> {
        let mapping = match node.string("MappingInformationType")?.as_str() {
            "ByPolygonVertex" => Mapping::PolygonVertex,
            "ByVertice" | "ByVertex" | "ByControlPoint" => Mapping::ControlPoint,
            "ByPolygon" => Mapping::Polygon,
            "AllSame" => Mapping::AllSame,
            _ => return None,
        };
        let reference = node.string("ReferenceInformationType").unwrap_or_default();
        let indexed = matches!(reference.as_str(), "IndexToDirect" | "Index");
        Some(Self {
            values: node.array_f64(values).unwrap_or_default(),
            width,
            index: if indexed { node.array_i64(index) } else { None },
            mapping,
        })
    }

    /// The value of corner `corner` (of polygon `polygon`, control point `point`).
    fn get(&self, corner: usize, polygon: usize, point: usize) -> Option<&[f64]> {
        let slot = match self.mapping {
            Mapping::PolygonVertex => corner,
            Mapping::ControlPoint => point,
            Mapping::Polygon => polygon,
            Mapping::AllSame => 0,
        };
        let i = match &self.index {
            Some(index) => usize::try_from(*index.get(slot)?).ok()?,
            None => slot,
        };
        self.values.get(i * self.width..(i + 1) * self.width)
    }
}

/// Reads a document.
pub fn read(doc: &Document, files: &SideFiles) -> Result<Scene> {
    if doc.version != 0 && doc.version < 7000 {
        return Err(bad(format!(
            "version {} (FBX 6 and older) is not supported; save the file as FBX 7",
            doc.version
        )));
    }
    let graph = Graph::new(doc)?;
    let mut scene = Scene::default();
    let settings = doc
        .get("GlobalSettings")
        .map(|g| Props {
            own: props70(g),
            template: None,
        })
        .unwrap_or_default();
    let (conversion, mut warnings) = Conversion::from_settings(&settings);
    scene.warnings.append(&mut warnings);
    let rate = frame_rate(&settings);

    // Models: parents, rest locals and worlds.
    let mut model_ids: Vec<i64> = graph
        .objects
        .values()
        .filter(|o| o.class == "Model")
        .map(|o| o.id)
        .collect();
    model_ids.sort_unstable();
    let parent: HashMap<i64, i64> = model_ids
        .iter()
        .filter_map(|&id| {
            graph
                .parents
                .get(&id)?
                .iter()
                .find(|(p, prop)| {
                    prop.is_none() && graph.objects.get(p).is_some_and(|o| o.class == "Model")
                })
                .map(|(p, _)| (id, *p))
        })
        .collect();
    let model_props: HashMap<i64, Props<'_>> = model_ids
        .iter()
        .map(|&id| (id, graph.props_with_template(&graph.objects[&id])))
        .collect();
    let rest_local: HashMap<i64, Mat4> = model_ids
        .iter()
        .map(|&id| (id, node_local(&model_props[&id], None)))
        .collect();
    let worlds = |locals: &HashMap<i64, Mat4>| -> HashMap<i64, Mat4> {
        let mut out: HashMap<i64, Mat4> = HashMap::new();
        for &id in &model_ids {
            let mut chain = vec![id];
            let mut p = parent.get(&id).copied();
            while let Some(pid) = p {
                if chain.len() > 1024 || chain.contains(&pid) {
                    break;
                }
                chain.push(pid);
                p = parent.get(&pid).copied();
            }
            let mut w = IDENTITY;
            for m in chain.iter().rev() {
                w = math::multiply(&w, &locals[m]);
            }
            out.insert(id, w);
        }
        out
    };
    let rest_world = worlds(&rest_local);

    // Joints: limb nodes and models that clusters link to.
    let mut joint_set: HashSet<i64> = model_ids
        .iter()
        .copied()
        .filter(|id| {
            let o = &graph.objects[id];
            matches!(o.kind.as_str(), "LimbNode" | "Limb" | "Root")
                || graph
                    .children_of(*id, "NodeAttribute")
                    .iter()
                    .any(|(a, _)| matches!(a.kind.as_str(), "LimbNode" | "Limb" | "Root"))
        })
        .collect();
    let clusters: Vec<&Object<'_>> = graph
        .objects
        .values()
        .filter(|o| o.class == "Deformer" && o.kind == "Cluster")
        .collect();
    let cluster_link = |cluster: i64| -> Option<i64> {
        graph
            .parents
            .get(&cluster)?
            .iter()
            .chain(graph.children.get(&cluster).into_iter().flatten())
            .find(|(m, _)| graph.objects.get(m).is_some_and(|o| o.class == "Model"))
            .map(|(m, _)| *m)
    };
    for c in &clusters {
        if let Some(m) = cluster_link(c.id) {
            joint_set.insert(m);
        }
    }
    // Hierarchy order (parents first), siblings by id.
    let mut kids: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut roots = Vec::new();
    for &id in &model_ids {
        match parent.get(&id) {
            Some(p) => kids.entry(*p).or_default().push(id),
            None => roots.push(id),
        }
    }
    let mut order = Vec::new();
    let mut stack: Vec<i64> = roots.into_iter().rev().collect();
    let mut visited = HashSet::new();
    while let Some(id) = stack.pop() {
        if !visited.insert(id) {
            continue;
        }
        order.push(id);
        if let Some(k) = kids.get(&id) {
            stack.extend(k.iter().rev());
        }
    }
    // Bind matrices from clusters and bind poses.
    let mut bind_world: HashMap<i64, Mat4> = HashMap::new();
    for pose in graph
        .objects
        .values()
        .filter(|o| o.class == "Pose" && o.kind == "BindPose")
    {
        for pn in pose.node.children_named("PoseNode") {
            if let (Some(id), Some(m)) = (
                pn.value("Node").and_then(Prop::as_i64),
                pn.array_f64("Matrix"),
            ) && m.len() == 16
            {
                bind_world
                    .entry(id)
                    .or_insert_with(|| std::array::from_fn(|i| m[i] as f32));
            }
        }
    }
    for c in &clusters {
        if let (Some(m), Some(link)) = (cluster_link(c.id), c.node.array_f64("TransformLink"))
            && link.len() == 16
        {
            bind_world.insert(m, std::array::from_fn(|i| link[i] as f32));
        }
    }
    let mut joint_index: HashMap<i64, usize> = HashMap::new();
    let converted_rest: HashMap<i64, Mat4> = rest_world
        .iter()
        .map(|(id, w)| (*id, conversion.matrix(w)))
        .collect();
    // Joints in object order (the order a writer created them), each after
    // the joints above it.
    let mut joint_order: Vec<i64> = Vec::new();
    let mut placed: HashSet<i64> = HashSet::new();
    for &id in &model_ids {
        if !joint_set.contains(&id) || placed.contains(&id) {
            continue;
        }
        let mut chain = vec![id];
        let mut p = parent.get(&id).copied();
        while let Some(pid) = p {
            if chain.len() > 1024 || chain.contains(&pid) || placed.contains(&pid) {
                break;
            }
            if joint_set.contains(&pid) {
                chain.push(pid);
            }
            p = parent.get(&pid).copied();
        }
        for &j in chain.iter().rev() {
            if placed.insert(j) {
                joint_order.push(j);
            }
        }
    }
    for &id in &joint_order {
        let mut p = parent.get(&id).copied();
        let mut guard = 0;
        while let Some(pid) = p {
            if joint_index.contains_key(&pid) || guard > 1024 {
                break;
            }
            p = parent.get(&pid).copied();
            guard += 1;
        }
        let parent_joint = p.and_then(|pid| joint_index.get(&pid).copied());
        let world = converted_rest[&id];
        let local = match p.filter(|_| parent_joint.is_some()) {
            Some(pid) => math::multiply(&math::invert(&converted_rest[&pid]), &world),
            None => world,
        };
        let bind = match bind_world.get(&id) {
            Some(b) => conversion.matrix(b),
            None => {
                scene.joints_without_bind.push(scene.joints.len());
                world
            }
        };
        scene.joints.push(SceneJoint {
            name: graph.objects[&id].name.clone(),
            parent: parent_joint,
            local,
            inverse_bind: math::invert(&bind),
        });
        joint_index.insert(id, scene.joints.len() - 1);
    }

    // Materials and textures.
    let mut material_index: HashMap<i64, usize> = HashMap::new();
    let mut image_cache: HashMap<i64, Option<SceneImage>> = HashMap::new();
    let mut texture_image =
        |texture: &Object<'_>, warnings: &mut Vec<String>| -> Option<SceneImage> {
            if let Some(found) = image_cache.get(&texture.id) {
                return found.clone();
            }
            let mut image = None;
            let videos = graph.children_of(texture.id, "Video");
            for (video, _) in &videos {
                if let Some(Prop::Bytes(content)) = video.node.value("Content")
                    && content.len() > 8
                {
                    let name = video
                        .node
                        .string("RelativeFilename")
                        .or_else(|| video.node.string("FileName"))
                        .or_else(|| video.node.string("Filename"))
                        .unwrap_or_else(|| texture.name.clone());
                    let stem = name
                        .replace('\\', "/")
                        .rsplit('/')
                        .next()
                        .unwrap_or("")
                        .rsplit_once('.')
                        .map_or_else(|| texture.name.clone(), |(s, _)| s.to_owned());
                    let mime = if content.starts_with(&[0xFF, 0xD8]) {
                        "image/jpeg"
                    } else {
                        "image/png"
                    };
                    image = Some(SceneImage {
                        name: stem,
                        mime_type: mime.into(),
                        bytes: content.clone(),
                    });
                    break;
                }
            }
            if image.is_none() {
                let names = [
                    texture.node.string("RelativeFilename"),
                    texture.node.string("FileName"),
                ];
                let mut last_error = None;
                for name in names.into_iter().flatten().filter(|n| !n.is_empty()) {
                    match files.image(&name) {
                        Ok(i) => {
                            image = Some(i);
                            break;
                        }
                        Err(e) => last_error = Some(e),
                    }
                }
                if image.is_none()
                    && let Some(e) = last_error
                {
                    warnings.push(format!("texture `{}`: {e}", texture.name));
                }
            }
            image_cache.insert(texture.id, image.clone());
            image
        };
    let mut material_of = |id: i64, scene: &mut Scene| -> usize {
        if let Some(&m) = material_index.get(&id) {
            return m;
        }
        let object = &graph.objects[&id];
        let props = graph.props(object);
        let mut material = SceneMaterial {
            name: object.name.clone(),
            ..Default::default()
        };
        let color = props.vec3("DiffuseColor", props.vec3("Diffuse", [1.0; 3]));
        let factor = props.f64("DiffuseFactor", 1.0);
        let opacity = match props.get("Opacity") {
            Some(_) => props.f64("Opacity", 1.0),
            None => 1.0 - props.f64("TransparencyFactor", 0.0).clamp(0.0, 1.0),
        };
        material.base_color_factor = [
            (color[0] * factor) as f32,
            (color[1] * factor) as f32,
            (color[2] * factor) as f32,
            opacity as f32,
        ];
        material.alpha = opacity < 1.0;
        for (texture, property) in graph.children_of(id, "Texture") {
            let property = property.unwrap_or("").to_ascii_lowercase();
            let slot = if property.contains("normal") || property.contains("bump") {
                1
            } else if property.contains("specular") || property.contains("reflection") {
                2
            } else if property.contains("diffuse")
                || property.contains("basecolor")
                || property.contains("base_color")
                || property.is_empty()
            {
                0
            } else if property.contains("transparen") || property.contains("opacity") {
                material.alpha = true;
                continue;
            } else {
                continue;
            };
            let image = texture_image(texture, &mut scene.warnings);
            let target = match slot {
                0 => &mut material.base_color,
                1 => &mut material.normal,
                _ => &mut material.specular,
            };
            if target.is_none() {
                *target = image;
            }
        }
        scene.materials.push(material);
        material_index.insert(id, scene.materials.len() - 1);
        scene.materials.len() - 1
    };

    // Meshes.
    let points = conversion.points();
    for &id in &order {
        let model = &graph.objects[&id];
        let Some((geometry, _)) = graph
            .children_of(id, "Geometry")
            .into_iter()
            .find(|(g, _)| g.kind == "Mesh" || g.kind.is_empty())
        else {
            continue;
        };
        if scene.name.is_empty() {
            scene.name = model.name.clone();
        }
        let materials: Vec<i64> = graph
            .children_of(id, "Material")
            .into_iter()
            .map(|(m, _)| m.id)
            .collect();
        let control_points = geometry
            .node
            .child("Vertices")
            .and_then(|v| v.props.first())
            .and_then(Prop::f64s)
            .map_or(0, |v| v.len() / 3);
        // Skin: clusters of the geometry's skin deformers.
        let mut weights: Vec<Vec<(u16, f32)>> = Vec::new();
        let mut mesh_bind: Option<Mat4> = None;
        for (skin, _) in graph.children_of(geometry.id, "Deformer") {
            if skin.kind != "Skin" {
                continue;
            }
            for (cluster, _) in graph.children_of(skin.id, "Deformer") {
                let Some(joint) = cluster_link(cluster.id).and_then(|m| joint_index.get(&m)) else {
                    continue;
                };
                let indexes = cluster.node.array_i64("Indexes").unwrap_or_default();
                let values = cluster.node.array_f64("Weights").unwrap_or_default();
                for (&i, &w) in indexes.iter().zip(&values) {
                    let Ok(i) = usize::try_from(i) else { continue };
                    if i >= control_points {
                        continue;
                    }
                    if weights.len() <= i {
                        weights.resize(i + 1, Vec::new());
                    }
                    weights[i].push((*joint as u16, w as f32));
                }
                if mesh_bind.is_none()
                    && let (Some(t), Some(l)) = (
                        cluster.node.array_f64("Transform"),
                        cluster.node.array_f64("TransformLink"),
                    )
                    && t.len() == 16
                    && l.len() == 16
                {
                    let t: Mat4 = std::array::from_fn(|i| t[i] as f32);
                    let l: Mat4 = std::array::from_fn(|i| l[i] as f32);
                    mesh_bind = Some(snap_identity(math::multiply(&l, &t)));
                }
            }
        }
        let skinned = !weights.is_empty();
        let props = &model_props[&id];
        let world = if skinned {
            mesh_bind.unwrap_or(rest_world[&id])
        } else {
            math::multiply(&rest_world[&id], &geometric(props))
        };
        let parts = read_geometry(
            geometry.node,
            &model.name,
            skinned.then_some(&weights[..]),
            &mut scene.warnings,
        )?;
        let several = parts.len() > 1;
        for (slot, mut g) in parts {
            math::transform_geometry(&math::multiply(&points, &world), &mut g);
            let material = materials.get(slot).map(|&m| material_of(m, &mut scene));
            let name = if several {
                format!("{}.{slot}", model.name)
            } else {
                model.name.clone()
            };
            g.name = name.clone();
            scene.parts.push(ScenePart {
                name,
                object: model.name.clone(),
                material,
                geometry: g,
            });
        }
    }

    // Animation stacks.
    let mut stacks: Vec<&Object<'_>> = graph
        .objects
        .values()
        .filter(|o| o.class == "AnimationStack")
        .collect();
    stacks.sort_by_key(|o| o.id);
    for stack in stacks {
        if let Some(clip) = read_stack(
            &graph,
            stack,
            rate,
            &model_ids,
            &model_props,
            &rest_local,
            &worlds,
            &joint_index,
            &order,
            &parent,
            &conversion,
            &mut scene.warnings,
        ) {
            scene.clips.push(clip);
        }
    }
    Ok(scene)
}

#[allow(clippy::too_many_arguments)]
fn read_stack(
    graph: &Graph<'_>,
    stack: &Object<'_>,
    rate: f32,
    model_ids: &[i64],
    model_props: &HashMap<i64, Props<'_>>,
    rest_local: &HashMap<i64, Mat4>,
    worlds: &Worlds<'_>,
    joint_index: &HashMap<i64, usize>,
    order: &[i64],
    parent: &HashMap<i64, i64>,
    conversion: &Conversion,
    warnings: &mut Vec<String>,
) -> Option<SceneClip> {
    let layers = graph.children_of(stack.id, "AnimationLayer");
    if layers.len() > 1 {
        warnings.push(format!(
            "animation `{}`: {} layers; only the first is read",
            stack.name,
            layers.len()
        ));
    }
    let mut layers: Vec<_> = layers.into_iter().map(|(l, _)| l).collect();
    layers.sort_by_key(|l| l.id);
    let layer = layers.first()?;
    // model → [T, R, S] → [x, y, z] keys (times in seconds, values).
    type Channel = Option<(Vec<f64>, Vec<f64>)>;
    let mut animated: HashMap<i64, [[Channel; 3]; 3]> = HashMap::new();
    let mut start = f64::INFINITY;
    let mut stop = f64::NEG_INFINITY;
    for (cnode, _) in graph.children_of(layer.id, "AnimationCurveNode") {
        let Some((model, property)) = graph
            .parents
            .get(&cnode.id)
            .into_iter()
            .flatten()
            .find(|(m, p)| p.is_some() && graph.objects.get(m).is_some_and(|o| o.class == "Model"))
        else {
            continue;
        };
        let slot = match property.as_deref() {
            Some("Lcl Translation") => 0,
            Some("Lcl Rotation") => 1,
            Some("Lcl Scaling") => 2,
            _ => continue,
        };
        let defaults = props70(cnode.node);
        let entry = animated.entry(*model).or_default();
        for (curve, prop) in graph.children_of(cnode.id, "AnimationCurve") {
            let axis = match prop.map(|p| p.to_ascii_uppercase()) {
                Some(p) if p.ends_with('X') => 0,
                Some(p) if p.ends_with('Y') => 1,
                Some(p) if p.ends_with('Z') => 2,
                _ => continue,
            };
            let times: Vec<f64> = curve
                .node
                .array_i64("KeyTime")
                .unwrap_or_default()
                .into_iter()
                .map(|t| t as f64 / KTIME)
                .collect();
            let values = curve.node.array_f64("KeyValueFloat").unwrap_or_default();
            if times.is_empty() || values.len() < times.len() {
                continue;
            }
            start = start.min(times[0]);
            stop = stop.max(*times.last().unwrap_or(&0.0));
            entry[slot][axis] = Some((times, values));
        }
        // Constant channels from the curve node defaults (`d|X`…).
        for (axis, key) in ["d|X", "d|Y", "d|Z"].iter().enumerate() {
            if entry[slot][axis].is_none()
                && let Some(v) = defaults
                    .get(*key)
                    .and_then(|v| v.first())
                    .and_then(Prop::as_f64)
            {
                entry[slot][axis] = Some((vec![0.0], vec![v]));
            }
        }
    }
    if animated.is_empty() {
        return None;
    }
    let props = graph.props(stack);
    let local_start = props
        .get("LocalStart")
        .and_then(|v| v.first())
        .and_then(Prop::as_i64);
    let local_stop = props
        .get("LocalStop")
        .and_then(|v| v.first())
        .and_then(Prop::as_i64);
    if let (Some(a), Some(b)) = (local_start, local_stop)
        && b > a
    {
        start = a as f64 / KTIME;
        stop = b as f64 / KTIME;
    }
    if !start.is_finite() || !stop.is_finite() || stop < start {
        start = 0.0;
        stop = 0.0;
    }
    let frames = (((stop - start) * rate as f64).round() as usize + 1).min(100_000);
    if frames.saturating_mul(model_ids.len()) > MAX_SAMPLES {
        warnings.push(format!("animation `{}` is too long; skipped", stack.name));
        return None;
    }
    let sample = |keys: &(Vec<f64>, Vec<f64>), t: f64| -> f64 {
        let (times, values) = keys;
        let n = times.len();
        if n == 1 || t <= times[0] {
            return values[0];
        }
        if t >= times[n - 1] {
            return values[n - 1];
        }
        let i = times.partition_point(|&x| x <= t).max(1) - 1;
        let (t0, t1) = (times[i], times[i + 1]);
        if t1 <= t0 {
            return values[i];
        }
        values[i] + (values[i + 1] - values[i]) * (t - t0) / (t1 - t0)
    };
    let mut tracks: HashMap<usize, JointTrack> = HashMap::new();
    let animated_joints: HashSet<usize> = animated
        .keys()
        .filter_map(|m| joint_index.get(m).copied())
        .collect();
    for f in 0..frames {
        let t = start + f as f64 / rate as f64;
        let mut locals = rest_local.clone();
        for (&model, channels) in &animated {
            let props = &model_props[&model];
            let rest = [
                props.vec3("Lcl Translation", [0.0; 3]),
                props.vec3("Lcl Rotation", [0.0; 3]),
                props.vec3("Lcl Scaling", [1.0; 3]),
            ];
            let mut values = rest;
            for slot in 0..3 {
                for axis in 0..3 {
                    if let Some(keys) = &channels[slot][axis] {
                        values[slot][axis] = sample(keys, t);
                    }
                }
            }
            locals.insert(model, node_local(props, Some(&values)));
        }
        let _ = model_ids;
        let world = worlds(&locals);
        let converted: HashMap<i64, Mat4> = order
            .iter()
            .filter(|id| joint_index.contains_key(id))
            .map(|&id| (id, conversion.matrix(&world[&id])))
            .collect();
        for (&id, &joint) in joint_index {
            // Only joints whose transform changes: animated ones or below one.
            let mut moved = animated_joints.contains(&joint);
            let mut p = parent.get(&id).copied();
            while !moved && let Some(pid) = p {
                if joint_index.contains_key(&pid) {
                    break;
                }
                moved |= animated.contains_key(&pid);
                p = parent.get(&pid).copied();
            }
            if !moved {
                continue;
            }
            let mut pj = parent.get(&id).copied();
            while let Some(pid) = pj {
                if joint_index.contains_key(&pid) {
                    break;
                }
                pj = parent.get(&pid).copied();
            }
            let local = match pj {
                // The parent joint is the node's parent: its local transform
                // as it is (translation in metres), without the precision
                // lost through world matrices far from the origin.
                Some(pid) if parent.get(&id) == Some(&pid) => {
                    let mut m = locals[&id];
                    for v in &mut m[12..15] {
                        *v *= conversion.scale;
                    }
                    m
                }
                Some(pid) => math::multiply(&math::invert(&converted[&pid]), &converted[&id]),
                None => converted[&id],
            };
            let (tr, r, s) = math::decompose(&local);
            let track = tracks.entry(joint).or_insert_with(|| JointTrack {
                joint,
                ..Default::default()
            });
            let r = match track.rotations.last() {
                Some(prev) if (0..4).map(|i| prev[i] * r[i]).sum::<f32>() < 0.0 => r.map(|x| -x),
                _ => r,
            };
            track.rotations.push(r);
            track.translations.push(tr);
            track.scales.push(s);
        }
    }
    let mut tracks: Vec<JointTrack> = tracks.into_values().collect();
    for t in &mut tracks {
        if t.scales
            .iter()
            .all(|s| s.iter().all(|x| (x - 1.0).abs() < 1e-5))
        {
            t.scales.clear();
        }
    }
    tracks.sort_by_key(|t| t.joint);
    if tracks.is_empty() {
        return None;
    }
    Some(SceneClip {
        name: stack.name.clone(),
        frame_rate: rate,
        tracks,
    })
}

type Worlds<'a> = dyn Fn(&HashMap<i64, Mat4>) -> HashMap<i64, Mat4> + 'a;

/// A matrix within rounding of the identity is the identity (a bind pose
/// written as `TransformLink · TransformLink⁻¹`).
fn snap_identity(m: Mat4) -> Mat4 {
    if m.iter().zip(&IDENTITY).all(|(a, b)| (a - b).abs() < 1e-5) {
        IDENTITY
    } else {
        m
    }
}

/// Reads one mesh geometry: one geometry per material slot, in the
/// geometry's space. `weights` are per control point (scene joints).
fn read_geometry(
    node: &Node,
    name: &str,
    weights: Option<&[Vec<(u16, f32)>]>,
    warnings: &mut Vec<String>,
) -> Result<Vec<(usize, PartGeometry)>> {
    let vertices = node.array_f64("Vertices").unwrap_or_default();
    let indices = node.array_i64("PolygonVertexIndex").unwrap_or_default();
    let points: Vec<[f32; 3]> = vertices
        .chunks_exact(3)
        .map(|c| [c[0] as f32, c[1] as f32, c[2] as f32])
        .collect();
    let layer = |element: &str, values: &str, index: &str, width: usize| {
        node.child(element)
            .and_then(|e| Layer::read(e, values, index, width))
    };
    let normals = layer("LayerElementNormal", "Normals", "NormalsIndex", 3);
    let uv_layers: Vec<Layer> = node
        .children_named("LayerElementUV")
        .filter_map(|e| Layer::read(e, "UV", "UVIndex", 2))
        .collect();
    let colors = layer("LayerElementColor", "Colors", "ColorIndex", 4);
    let materials = node.child("LayerElementMaterial").map(|e| {
        let mapping = e.string("MappingInformationType").unwrap_or_default();
        let values = e.array_i64("Materials").unwrap_or_default();
        (mapping == "AllSame", values)
    });
    let mut builders: HashMap<usize, CornerBuilder> = HashMap::new();
    let mut slots: Vec<usize> = Vec::new();
    let mut skin_cache: HashMap<usize, Option<crate::scene::VertexSkin>> = HashMap::new();
    let mut dropped = false;
    let mut unweighted = 0usize;
    let mut polygon = 0usize;
    let mut first_corner = 0usize;
    let mut corners: Vec<Corner> = Vec::new();
    for (k, &raw) in indices.iter().enumerate() {
        let last = raw < 0;
        let point = if last { !raw } else { raw };
        let point = usize::try_from(point)
            .ok()
            .filter(|&p| p < points.len())
            .ok_or_else(|| bad(format!("mesh `{name}`: vertex index {point} out of range")))?;
        let skin = match weights {
            Some(w) => *skin_cache.entry(point).or_insert_with(|| {
                let mut influences = w.get(point).cloned().unwrap_or_default();
                let (skin, d) = top_four(&mut influences);
                dropped |= d;
                if skin.is_none() {
                    unweighted += 1;
                }
                skin
            }),
            None => None,
        };
        let corner = k;
        corners.push(Corner {
            point: point as u32,
            position: points[point],
            normal: normals
                .as_ref()
                .and_then(|l| l.get(corner, polygon, point))
                .map(|v| math::normalize3([v[0] as f32, v[1] as f32, v[2] as f32])),
            uv0: uv_layers
                .first()
                .and_then(|l| l.get(corner, polygon, point))
                .map(|v| [v[0] as f32, 1.0 - v[1] as f32]),
            uv1: uv_layers
                .get(1)
                .and_then(|l| l.get(corner, polygon, point))
                .map(|v| [v[0] as f32, 1.0 - v[1] as f32]),
            color: colors
                .as_ref()
                .and_then(|l| l.get(corner, polygon, point))
                .map(|v| {
                    v.iter()
                        .map(|&c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)
                        .collect::<Vec<u8>>()
                })
                .map(|c| [c[0], c[1], c[2], c[3]]),
            skin: skin.or(weights.map(|_| ([0; 4], [1.0, 0.0, 0.0, 0.0]))),
            tangent: None,
        });
        if last {
            let slot = match &materials {
                Some((true, v)) => v.first().copied().unwrap_or(0),
                Some((false, v)) => v.get(polygon).copied().unwrap_or(0),
                None => 0,
            };
            let slot = usize::try_from(slot).unwrap_or(0);
            let builder = builders.entry(slot).or_insert_with(|| {
                slots.push(slot);
                CornerBuilder::new(name)
            });
            builder.polygon(&corners);
            corners.clear();
            polygon += 1;
            first_corner = k + 1;
        }
    }
    let _ = first_corner;
    if !corners.is_empty() {
        warnings.push(format!("mesh `{name}`: last polygon is not closed"));
    }
    if dropped {
        warnings.push(format!(
            "mesh `{name}`: more than 4 weights per vertex; the 4 largest are used"
        ));
    }
    if unweighted > 0 {
        warnings.push(format!(
            "mesh `{name}`: {unweighted} vertices without bone weights are bound to the first joint"
        ));
    }
    slots.sort_unstable();
    Ok(slots
        .into_iter()
        .filter_map(|s| {
            let g = builders.remove(&s)?.finish();
            (!g.triangles.is_empty()).then_some((s, g))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(text: &str) -> Prop {
        Prop::str(text)
    }

    fn p(name: &str, values: Vec<Prop>) -> Node {
        let mut props = vec![s(name), s(""), s(""), s("")];
        props.extend(values);
        Node::new("P", props)
    }

    fn v3(name: &str, v: [f64; 3]) -> Node {
        p(name, v.iter().map(|&x| Prop::F64(x)).collect())
    }

    fn props(entries: Vec<Node>) -> Node {
        Node::new("Properties70", vec![]).with(entries)
    }

    /// A Maya-style rig in centimetres: pre-rotations active through the
    /// property template, a skinned quad (polygon of four), an animation.
    fn maya_style() -> Document {
        let model = |id: i64, name: &str, kind: &str, entries: Vec<Node>| {
            Node::new(
                "Model",
                vec![Prop::I64(id), s(&format!("Model::{name}")), s(kind)],
            )
            .with(vec![props(entries)])
        };
        let c = |kind: &str, child: i64, parent: i64, prop: Option<&str>| {
            let mut v = vec![s(kind), Prop::I64(child), Prop::I64(parent)];
            if let Some(p) = prop {
                v.push(s(p));
            }
            Node::new("C", v)
        };
        let identity: Vec<f64> = IDENTITY.iter().map(|&x| x as f64).collect();
        let mut link = identity.clone();
        link[13] = 100.0; // the root joint at y = 1 m
        let mut inverse = identity.clone();
        inverse[13] = -100.0;
        let curve = |id: i64, values: Vec<f32>| {
            Node::new(
                "AnimationCurve",
                vec![Prop::I64(id), s("AnimCurve::"), s("")],
            )
            .with(vec![
                Node::new("KeyTime", vec![Prop::I64s(vec![0, KTIME as i64])]),
                Node::new("KeyValueFloat", vec![Prop::F32s(values)]),
            ])
        };
        let objects = Node::new("Objects", vec![]).with(vec![
            model(
                1,
                "root",
                "LimbNode",
                vec![
                    v3("Lcl Translation", [0.0, 100.0, 0.0]),
                    v3("PreRotation", [0.0, 0.0, 90.0]),
                ],
            ),
            model(
                2,
                "child",
                "LimbNode",
                vec![v3("Lcl Translation", [100.0, 0.0, 0.0])],
            ),
            model(3, "quad", "Mesh", vec![]),
            Node::new(
                "Geometry",
                vec![Prop::I64(4), s("Geometry::quad"), s("Mesh")],
            )
            .with(vec![
                Node::new(
                    "Vertices",
                    vec![Prop::F64s(vec![
                        0., 0., 0., 100., 0., 0., 100., 100., 0., 0., 100., 0.,
                    ])],
                ),
                Node::new("PolygonVertexIndex", vec![Prop::I32s(vec![0, 1, 2, -4])]),
                Node::new("LayerElementUV", vec![Prop::I32(0)]).with(vec![
                    Node::new("MappingInformationType", vec![s("ByPolygonVertex")]),
                    Node::new("ReferenceInformationType", vec![s("IndexToDirect")]),
                    Node::new("UV", vec![Prop::F64s(vec![0., 0., 1., 1.])]),
                    Node::new("UVIndex", vec![Prop::I32s(vec![0, 0, 1, 1])]),
                ]),
            ]),
            Node::new(
                "Deformer",
                vec![Prop::I64(5), s("Deformer::skin"), s("Skin")],
            ),
            Node::new(
                "Deformer",
                vec![Prop::I64(6), s("SubDeformer::c"), s("Cluster")],
            )
            .with(vec![
                Node::new("Indexes", vec![Prop::I32s(vec![0, 1, 2, 3])]),
                Node::new("Weights", vec![Prop::F64s(vec![1.0, 1.0, 0.5, 1.0])]),
                Node::new("Transform", vec![Prop::F64s(inverse)]),
                Node::new("TransformLink", vec![Prop::F64s(link)]),
            ]),
            Node::new(
                "AnimationStack",
                vec![Prop::I64(10), s("AnimStack::turn"), s("")],
            ),
            Node::new(
                "AnimationLayer",
                vec![Prop::I64(11), s("AnimLayer::base"), s("")],
            ),
            Node::new(
                "AnimationCurveNode",
                vec![Prop::I64(12), s("AnimCurveNode::R"), s("")],
            ),
            curve(13, vec![0.0, 0.0]),
            curve(14, vec![0.0, 0.0]),
            curve(15, vec![0.0, 90.0]),
        ]);
        let connections = Node::new("Connections", vec![]).with(vec![
            c("OO", 1, 0, None),
            c("OO", 2, 1, None),
            c("OO", 3, 0, None),
            c("OO", 4, 3, None),
            c("OO", 5, 4, None),
            c("OO", 6, 5, None),
            c("OO", 1, 6, None),
            c("OO", 11, 10, None),
            c("OO", 12, 11, None),
            c("OP", 12, 1, Some("Lcl Rotation")),
            c("OP", 13, 12, Some("d|X")),
            c("OP", 14, 12, Some("d|Y")),
            c("OP", 15, 12, Some("d|Z")),
        ]);
        let definitions = Node::new("Definitions", vec![]).with(vec![
            Node::new("ObjectType", vec![s("Model")]).with(vec![
                Node::new("PropertyTemplate", vec![s("FbxNode")])
                    .with(vec![props(vec![p("RotationActive", vec![Prop::I32(1)])])]),
            ]),
        ]);
        let settings = Node::new("GlobalSettings", vec![]).with(vec![props(vec![
            p("UpAxis", vec![Prop::I32(1)]),
            p("UnitScaleFactor", vec![Prop::F64(1.0)]),
            p("TimeMode", vec![Prop::I32(6)]),
        ])]);
        Document {
            version: 7500,
            binary: false,
            nodes: vec![settings, definitions, objects, connections],
        }
    }

    #[test]
    fn maya_style_rigs_use_pre_rotations_templates_and_units() {
        let scene = read(&maya_style(), &SideFiles::new(None)).unwrap();
        assert_eq!(scene.joints.len(), 2);
        let worlds = scene.rest_globals();
        let close = |a: [f32; 3], b: [f32; 3]| (0..3).all(|i| (a[i] - b[i]).abs() < 1e-5);
        // Root at 1 m, child 1 m along the root's x turned 90° about z.
        assert!(close(math::translation_of(&worlds[0]), [0.0, 1.0, 0.0]));
        assert!(
            close(math::translation_of(&worlds[1]), [0.0, 2.0, 0.0]),
            "{worlds:?}"
        );
        // The quad: two triangles in metres, skinned to the root.
        let part = &scene.parts[0];
        assert_eq!(part.geometry.triangles.len(), 2);
        assert!(
            part.geometry
                .positions
                .iter()
                .any(|p| close(*p, [1.0, 1.0, 0.0]))
        );
        let skin = part.geometry.skin.as_ref().unwrap();
        assert!(skin.iter().all(|(j, w)| j[0] == 0 && w[0] == 1.0));
        assert_eq!(part.geometry.uv0.as_ref().unwrap()[0], [0.0, 1.0]);
        // Bind pose from the cluster.
        assert!(close(
            math::translation_of(&math::invert(&scene.joints[0].inverse_bind)),
            [0.0, 1.0, 0.0]
        ));
        // One second at 30 frames per second: the root turns 90° more.
        let clip = &scene.clips[0];
        assert_eq!((clip.name.as_str(), clip.frames()), ("turn", 31));
        let last = *clip.tracks[0].rotations.last().unwrap();
        let expected = math::axis_quat(2, std::f32::consts::PI);
        let dot: f32 = (0..4).map(|i| last[i] * expected[i]).sum();
        assert!(dot.abs() > 0.9999, "{last:?}");
    }
}
