//! [`Layer`] ↔ [`Scene`]: UsdGeom meshes, UsdShade preview materials,
//! UsdSkel skeletons, skinning and animations.
//!
//! Written layers look like this (names made valid USD identifiers; the
//! original names are kept in `custom string doctor:name` attributes and
//! `doctor:jointNames`):
//!
//! ```text
//! #usda 1.0 (defaultPrim = "root", metersPerUnit = 1, upAxis = "Y", timeCodesPerSecond = 30)
//! def SkelRoot "root"
//!     def Skeleton "Skeleton"   joints, bindTransforms, restTransforms, skel:animationSource
//!         def SkelAnimation "<clip>"   joints, translations/rotations/scales.timeSamples
//!     def Mesh "<part>"   points, faceVertexCounts/Indices, normals, primvars:st,
//!                         primvars:displayColor, primvars:skel:jointIndices/jointWeights
//!                         (elementSize 4), skel:skeleton, material:binding
//!     def Scope "Materials"
//!         def Material "<name>"   UsdPreviewSurface + UsdUVTexture + UsdPrimvarReader_float2
//! ```

use std::collections::HashMap;

use super::{Attribute, Layer, Prim, Property, Specifier, Value};
use crate::resource::rcb::Mat4;
use crate::scene::math::{self, EulerOrder, IDENTITY};
use crate::scene::{
    Corner, CornerBuilder, ImageFiles, JointTrack, Scene, SceneClip, SceneImage, SceneJoint,
    SceneMaterial, ScenePart, SideFiles, WrittenFile, top_four,
};
use crate::{Error, Result};

// ------------------------------------------------------------------ names

/// A valid prim name (`[A-Za-z_][A-Za-z0-9_]*`).
fn identifier(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() || s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    s
}

/// Unique identifiers for names, in order.
fn unique_identifiers<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut used: HashMap<String, usize> = HashMap::new();
    names
        .map(|n| {
            let base = identifier(n);
            let count = used.entry(base.clone()).or_insert(0);
            *count += 1;
            if *count == 1 {
                base
            } else {
                format!("{base}_{}", *count - 1)
            }
        })
        .collect()
}

fn custom_string(value: &str) -> Attribute {
    Attribute {
        custom: true,
        ..Attribute::new("string", Value::String(value.to_owned()))
    }
}

fn array(values: Vec<f64>, width: usize) -> Value {
    Value::Array { values, width }
}

fn f(values: impl IntoIterator<Item = f32>) -> Vec<f64> {
    values.into_iter().map(|x| x as f64).collect()
}

fn with_meta(mut a: Attribute, meta: &[(&str, Value)]) -> Attribute {
    for (k, v) in meta {
        a.metadata.push(((*k).to_owned(), v.clone()));
    }
    a
}

fn token(v: &str) -> Value {
    Value::Token(v.to_owned())
}

// ----------------------------------------------------------------- writer

/// Converts a scene to a layer; textures become files under `prefix`.
pub fn from_scene(scene: &Scene, prefix: &str) -> Result<(Layer, Vec<WrittenFile>)> {
    let images = ImageFiles::new(scene, prefix);
    let mut root = Prim::new(
        "root",
        if scene.joints.is_empty() {
            "Xform"
        } else {
            "SkelRoot"
        },
    );
    let rate = scene.clips.first().map_or(30.0, |c| {
        if c.frame_rate > 0.0 {
            c.frame_rate
        } else {
            30.0
        }
    });
    let longest = scene.clips.iter().map(SceneClip::frames).max().unwrap_or(0);
    let mut layer = Layer {
        metadata: vec![
            (
                "documentation".into(),
                Value::String(concat!("doctor_toolkit ", env!("CARGO_PKG_VERSION")).into()),
            ),
            ("defaultPrim".into(), token("root")),
            ("metersPerUnit".into(), Value::Float(1.0)),
            ("upAxis".into(), token("Y")),
        ],
        prims: Vec::new(),
    };
    if longest > 0 {
        layer.metadata.extend([
            ("startTimeCode".into(), Value::Float(0.0)),
            ("endTimeCode".into(), Value::Float((longest - 1) as f64)),
            ("timeCodesPerSecond".into(), Value::Float(rate as f64)),
            ("framesPerSecond".into(), Value::Float(rate as f64)),
        ]);
    }

    // Skeleton.
    let skeleton_path = "/root/Skeleton";
    let mut joint_paths: Vec<String> = Vec::new();
    if !scene.joints.is_empty() {
        let mut names_used: HashMap<(Option<usize>, String), usize> = HashMap::new();
        for joint in &scene.joints {
            let mut name = identifier(&joint.name);
            let count = names_used.entry((joint.parent, name.clone())).or_insert(0);
            *count += 1;
            if *count > 1 {
                name = format!("{name}_{}", *count - 1);
            }
            joint_paths.push(match joint.parent {
                Some(p) => format!("{}/{name}", joint_paths[p]),
                None => name,
            });
        }
        let mut skeleton = Prim::new("Skeleton", "Skeleton");
        skeleton.metadata.push((
            "apiSchemas".into(),
            Value::Strings(vec!["SkelBindingAPI".into()]),
        ));
        let uniform = |a: Attribute| Attribute { uniform: true, ..a };
        skeleton.set(
            "joints",
            uniform(Attribute::new(
                "token[]",
                Value::Strings(joint_paths.clone()),
            )),
        );
        let binds: Vec<f64> = scene
            .joints
            .iter()
            .flat_map(|j| f(math::invert(&j.inverse_bind)))
            .collect();
        skeleton.set(
            "bindTransforms",
            uniform(Attribute::new("matrix4d[]", array(binds, 16))),
        );
        let rests: Vec<f64> = scene.joints.iter().flat_map(|j| f(j.local)).collect();
        skeleton.set(
            "restTransforms",
            uniform(Attribute::new("matrix4d[]", array(rests, 16))),
        );
        let renamed = scene
            .joints
            .iter()
            .zip(&joint_paths)
            .any(|(j, p)| p.rsplit('/').next() != Some(j.name.as_str()));
        if renamed {
            skeleton.set(
                "doctor:jointNames",
                Attribute {
                    custom: true,
                    uniform: true,
                    ..Attribute::new(
                        "token[]",
                        Value::Strings(scene.joints.iter().map(|j| j.name.clone()).collect()),
                    )
                },
            );
        }
        let clip_names = unique_identifiers(scene.clips.iter().map(|c| c.name.as_str()));
        for (clip, prim_name) in scene.clips.iter().zip(&clip_names) {
            let mut animation = Prim::new(prim_name, "SkelAnimation");
            if *prim_name != clip.name {
                animation.set("doctor:name", custom_string(&clip.name));
            }
            let tracks: Vec<&JointTrack> = clip
                .tracks
                .iter()
                .filter(|t| t.joint < scene.joints.len())
                .collect();
            animation.set(
                "joints",
                uniform(Attribute::new(
                    "token[]",
                    Value::Strings(
                        tracks
                            .iter()
                            .map(|t| joint_paths[t.joint].clone())
                            .collect(),
                    ),
                )),
            );
            let frames = clip.frames();
            let rest: Vec<([f32; 3], [f32; 4], [f32; 3])> = tracks
                .iter()
                .map(|t| math::decompose(&scene.joints[t.joint].local))
                .collect();
            let mut translations = Attribute {
                type_name: "float3[]".into(),
                ..Default::default()
            };
            let mut rotations = Attribute {
                type_name: "quatf[]".into(),
                ..Default::default()
            };
            let with_scales = tracks.iter().any(|t| !t.scales.is_empty());
            let mut scales = Attribute {
                type_name: "half3[]".into(),
                ..Default::default()
            };
            for frame in 0..frames {
                let at = frame as f32;
                let mut t = Vec::new();
                let mut r = Vec::new();
                let mut s = Vec::new();
                for (track, rest) in tracks.iter().zip(&rest) {
                    t.extend(f(
                        math::sample_vec3(&track.translations, at).unwrap_or(rest.0)
                    ));
                    r.extend(f(math::sample_quat(&track.rotations, at).unwrap_or(rest.1)));
                    s.extend(f(math::sample_vec3(&track.scales, at).unwrap_or([1.0; 3])));
                }
                translations.samples.push((frame as f64, array(t, 3)));
                rotations.samples.push((frame as f64, array(r, 4)));
                scales.samples.push((frame as f64, array(s, 3)));
            }
            animation.set("translations", translations);
            animation.set("rotations", rotations);
            // UsdSkel needs all three arrays (translations, rotations, scales).
            if !with_scales {
                scales.samples.clear();
                scales.default = Some(array(vec![1.0; 3 * tracks.len()], 3));
            }
            animation.set("scales", scales);
            skeleton.children.push(animation);
        }
        if let Some(first) = clip_names.first() {
            skeleton.relate(
                "skel:animationSource",
                vec![format!("{skeleton_path}/{first}")],
            );
        }
        root.children.push(skeleton);
    }

    // Materials.
    let material_names = unique_identifiers(scene.materials.iter().map(|m| m.name.as_str()));
    let mut materials_scope = Prim::new("Materials", "Scope");
    for (material, name) in scene.materials.iter().zip(&material_names) {
        materials_scope
            .children
            .push(material_prim(material, name, &images));
    }

    // Meshes.
    let part_names = unique_identifiers(scene.parts.iter().map(|p| p.name.as_str()));
    for (part, name) in scene.parts.iter().zip(&part_names) {
        let g = &part.geometry;
        if g.triangles.is_empty() {
            continue;
        }
        let mut mesh = Prim::new(name, "Mesh");
        let mut schemas = vec!["MaterialBindingAPI".to_owned()];
        let skinned = g.skin.is_some() && !scene.joints.is_empty();
        if skinned {
            schemas.push("SkelBindingAPI".into());
        }
        mesh.metadata
            .push(("apiSchemas".into(), Value::Strings(schemas)));
        mesh.set("doctor:name", custom_string(&part.name));
        if part.object != part.name {
            mesh.set("doctor:object", custom_string(&part.object));
        }
        let (lo, hi) = g.positions.iter().fold(
            ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]),
            |(lo, hi), p| {
                (
                    std::array::from_fn(|i| lo[i].min(p[i])),
                    std::array::from_fn(|i| hi[i].max(p[i])),
                )
            },
        );
        mesh.set(
            "extent",
            Attribute::new("float3[]", array(f(lo.into_iter().chain(hi)), 3)),
        );
        mesh.set(
            "faceVertexCounts",
            Attribute::new("int[]", array(vec![3.0; g.triangles.len()], 1)),
        );
        mesh.set(
            "faceVertexIndices",
            Attribute::new(
                "int[]",
                array(g.triangles.iter().flatten().map(|&i| i as f64).collect(), 1),
            ),
        );
        if let Some(n) = &g.normals {
            mesh.set(
                "normals",
                with_meta(
                    Attribute::new("normal3f[]", array(f(n.iter().flatten().copied()), 3)),
                    &[("interpolation", token("vertex"))],
                ),
            );
        }
        mesh.set(
            "points",
            Attribute::new(
                "point3f[]",
                array(f(g.positions.iter().flatten().copied()), 3),
            ),
        );
        for (uv, primvar) in [(&g.uv0, "primvars:st"), (&g.uv1, "primvars:st1")] {
            if let Some(uv) = uv {
                mesh.set(
                    primvar,
                    with_meta(
                        Attribute::new(
                            "texCoord2f[]",
                            array(f(uv.iter().flat_map(|t| [t[0], 1.0 - t[1]])), 2),
                        ),
                        &[("interpolation", token("vertex"))],
                    ),
                );
            }
        }
        if let Some(colors) = &g.colors {
            let c = |v: u8| (v as f64 / 255.0 * 1e6).round() / 1e6;
            mesh.set(
                "primvars:displayColor",
                with_meta(
                    Attribute::new(
                        "color3f[]",
                        array(
                            colors
                                .iter()
                                .flat_map(|x| [c(x[0]), c(x[1]), c(x[2])])
                                .collect(),
                            3,
                        ),
                    ),
                    &[("interpolation", token("vertex"))],
                ),
            );
            if colors.iter().any(|x| x[3] != 255) {
                mesh.set(
                    "primvars:displayOpacity",
                    with_meta(
                        Attribute::new(
                            "float[]",
                            array(colors.iter().map(|x| c(x[3])).collect(), 1),
                        ),
                        &[("interpolation", token("vertex"))],
                    ),
                );
            }
        }
        if skinned {
            let skin = g.skin.as_ref().expect("skinned");
            let joints: Vec<f64> = skin.iter().flat_map(|(j, _)| j.map(|x| x as f64)).collect();
            let weights: Vec<f64> = skin.iter().flat_map(|(_, w)| f(*w)).collect();
            let meta = [
                ("elementSize", Value::Int(4)),
                ("interpolation", token("vertex")),
            ];
            mesh.set(
                "primvars:skel:geomBindTransform",
                Attribute::new("matrix4d", Value::Tuple(f(IDENTITY))),
            );
            mesh.set(
                "primvars:skel:jointIndices",
                with_meta(Attribute::new("int[]", array(joints, 1)), &meta),
            );
            mesh.set(
                "primvars:skel:jointWeights",
                with_meta(Attribute::new("float[]", array(weights, 1)), &meta),
            );
            mesh.relate("skel:skeleton", vec![skeleton_path.into()]);
        }
        if let Some(m) = part.material {
            mesh.relate(
                "material:binding",
                vec![format!("/root/Materials/{}", material_names[m])],
            );
        }
        mesh.set(
            "subdivisionScheme",
            Attribute {
                uniform: true,
                ..Attribute::new("token", token("none"))
            },
        );
        root.children.push(mesh);
    }
    if !materials_scope.children.is_empty() {
        root.children.push(materials_scope);
    }
    layer.prims.push(root);
    Ok((layer, images.files))
}

fn material_prim(material: &SceneMaterial, name: &str, images: &ImageFiles) -> Prim {
    let base = format!("/root/Materials/{name}");
    let mut prim = Prim::new(name, "Material");
    if material.name != name {
        prim.set("doctor:name", custom_string(&material.name));
    }
    let mut surface_out = Attribute {
        type_name: "token".into(),
        ..Default::default()
    };
    surface_out
        .connections
        .push(format!("{base}/Surface.outputs:surface"));
    prim.set("outputs:surface", surface_out);
    let mut surface = Prim::new("Surface", "Shader");
    let uniform_token = |v: &str| Attribute {
        uniform: true,
        ..Attribute::new("token", token(v))
    };
    surface.set("info:id", uniform_token("UsdPreviewSurface"));
    let file = |image: &Option<SceneImage>| image.as_ref().and_then(|i| images.name(i));
    let connected = |input: &str, kind: &str, target: &str| {
        let mut a = Attribute {
            type_name: kind.into(),
            ..Default::default()
        };
        a.connections.push(target.to_owned());
        let _ = input;
        a
    };
    let fac = material.base_color_factor;
    match file(&material.base_color) {
        Some(_) => surface.set(
            "inputs:diffuseColor",
            connected(
                "diffuseColor",
                "color3f",
                &format!("{base}/BaseColor.outputs:rgb"),
            ),
        ),
        None => surface.set(
            "inputs:diffuseColor",
            Attribute::new("color3f", Value::Tuple(f(fac[..3].iter().copied()))),
        ),
    }
    if material.alpha && file(&material.base_color).is_some() {
        surface.set(
            "inputs:opacity",
            connected("opacity", "float", &format!("{base}/BaseColor.outputs:a")),
        );
        surface.set(
            "inputs:opacityThreshold",
            Attribute::new("float", Value::Float(0.5)),
        );
    } else {
        surface.set(
            "inputs:opacity",
            Attribute::new("float", Value::Float(fac[3] as f64)),
        );
    }
    if file(&material.normal).is_some() {
        surface.set(
            "inputs:normal",
            connected("normal", "normal3f", &format!("{base}/Normal.outputs:rgb")),
        );
    }
    if file(&material.specular).is_some() {
        surface.set(
            "inputs:specularColor",
            connected(
                "specularColor",
                "color3f",
                &format!("{base}/Specular.outputs:rgb"),
            ),
        );
        surface.set(
            "inputs:useSpecularWorkflow",
            Attribute::new("int", Value::Int(1)),
        );
    }
    surface.set(
        "inputs:roughness",
        Attribute::new("float", Value::Float(1.0)),
    );
    surface.set(
        "inputs:metallic",
        Attribute::new("float", Value::Float(0.0)),
    );
    surface.set(
        "outputs:surface",
        Attribute {
            type_name: "token".into(),
            ..Default::default()
        },
    );
    prim.children.push(surface);
    let textures: Vec<(&str, &Option<SceneImage>, &str)> = vec![
        ("BaseColor", &material.base_color, "sRGB"),
        ("Normal", &material.normal, "raw"),
        ("Specular", &material.specular, "sRGB"),
    ];
    let mut any = false;
    for (shader_name, image, space) in textures {
        let Some(path) = file(image) else { continue };
        any = true;
        let mut shader = Prim::new(shader_name, "Shader");
        shader.set("info:id", uniform_token("UsdUVTexture"));
        shader.set(
            "inputs:file",
            Attribute::new("asset", Value::Asset(format!("./{path}"))),
        );
        shader.set(
            "inputs:st",
            connected("st", "float2", &format!("{base}/UV.outputs:result")),
        );
        shader.set(
            "inputs:sourceColorSpace",
            Attribute::new("token", token(space)),
        );
        shader.set("inputs:wrapS", Attribute::new("token", token("repeat")));
        shader.set("inputs:wrapT", Attribute::new("token", token("repeat")));
        if shader_name == "Normal" {
            shader.set(
                "inputs:scale",
                Attribute::new("float4", Value::Tuple(vec![2.0, 2.0, 2.0, 1.0])),
            );
            shader.set(
                "inputs:bias",
                Attribute::new("float4", Value::Tuple(vec![-1.0, -1.0, -1.0, 0.0])),
            );
        }
        shader.set(
            "outputs:rgb",
            Attribute {
                type_name: "float3".into(),
                ..Default::default()
            },
        );
        if shader_name == "BaseColor" {
            shader.set(
                "outputs:a",
                Attribute {
                    type_name: "float".into(),
                    ..Default::default()
                },
            );
        }
        prim.children.push(shader);
    }
    if any {
        let mut uv = Prim::new("UV", "Shader");
        uv.set("info:id", uniform_token("UsdPrimvarReader_float2"));
        uv.set(
            "inputs:varname",
            Attribute::new("string", Value::String("st".into())),
        );
        uv.set(
            "outputs:result",
            Attribute {
                type_name: "float2".into(),
                ..Default::default()
            },
        );
        prim.children.push(uv);
    }
    prim
}

// ----------------------------------------------------------------- reader

fn bad(message: impl Into<String>) -> Error {
    super::bad(message)
}

/// Numbers of an attribute's value (default, else first sample).
fn numbers(prim: &Prim, name: &str) -> Option<Vec<f64>> {
    prim.attribute(name)?
        .value()?
        .numbers()
        .map(<[f64]>::to_vec)
}

fn matrices(values: &[f64]) -> Vec<Mat4> {
    values
        .chunks_exact(16)
        .map(|c| std::array::from_fn(|i| c[i] as f32))
        .collect()
}

fn mat(values: &[f64]) -> Option<Mat4> {
    (values.len() >= 16).then(|| std::array::from_fn(|i| values[i] as f32))
}

fn interpolation(a: &Attribute, default: &str) -> String {
    a.meta("interpolation")
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_owned()
}

/// Local transform of a prim from its `xformOp`s.
fn local_transform(prim: &Prim) -> Mat4 {
    let Some(order) = prim
        .attribute("xformOpOrder")
        .and_then(Attribute::value)
        .and_then(Value::strings)
    else {
        return IDENTITY;
    };
    let mut m = IDENTITY;
    for op in order {
        let (inverse, name) = match op.strip_prefix("!invert!") {
            Some(n) => (true, n),
            None => (false, op.as_str()),
        };
        if name == "!resetXformStack!" {
            m = IDENTITY;
            continue;
        }
        let Some(v) = numbers(prim, name) else {
            continue;
        };
        let kind = name
            .strip_prefix("xformOp:")
            .unwrap_or(name)
            .split(':')
            .next()
            .unwrap_or("");
        let v3 =
            |d: f32| -> [f32; 3] { [0, 1, 2].map(|i| v.get(i).copied().map_or(d, |x| x as f32)) };
        let single = v.first().copied().unwrap_or(0.0) as f32;
        let op_matrix = match kind {
            "translate" => math::translation(v3(0.0)),
            "scale" => math::scaling(if v.len() == 1 { [single; 3] } else { v3(1.0) }),
            "rotateX" => math::quat_matrix(math::axis_quat(0, single.to_radians())),
            "rotateY" => math::quat_matrix(math::axis_quat(1, single.to_radians())),
            "rotateZ" => math::quat_matrix(math::axis_quat(2, single.to_radians())),
            "orient" if v.len() >= 4 => math::quat_matrix(math::quat_normalize([
                v[0] as f32,
                v[1] as f32,
                v[2] as f32,
                v[3] as f32,
            ])),
            "transform" => mat(&v).unwrap_or(IDENTITY),
            k if k.starts_with("rotate") && k.len() == 9 => {
                let axes: Vec<usize> = k[6..]
                    .bytes()
                    .map(|b| (b.to_ascii_uppercase() - b'X') as usize)
                    .collect();
                if axes.len() == 3 && axes.iter().all(|&a| a < 3) {
                    math::quat_matrix(math::euler_quat(
                        v3(0.0),
                        EulerOrder([axes[0], axes[1], axes[2]]),
                    ))
                } else {
                    IDENTITY
                }
            }
            _ => IDENTITY,
        };
        let op_matrix = if inverse {
            math::invert(&op_matrix)
        } else {
            op_matrix
        };
        m = math::multiply(&m, &op_matrix);
    }
    m
}

/// Stage-level conversion: axes and units.
struct Conversion {
    axes: Mat4,
    scale: f32,
}

impl Conversion {
    fn matrix(&self, m: &Mat4) -> Mat4 {
        let mut scaled = *m;
        for v in &mut scaled[12..15] {
            *v *= self.scale;
        }
        math::multiply(&self.axes, &scaled)
    }

    fn points(&self) -> Mat4 {
        math::multiply(&self.axes, &math::scaling([self.scale; 3]))
    }
}

struct Skeleton {
    path: String,
    world: Mat4,
    /// Joint tokens (paths) and the scene joint of each.
    joints: Vec<String>,
    scene_joint: Vec<usize>,
    rest: Vec<Mat4>,
    parents: Vec<Option<usize>>,
    animation: Option<String>,
}

/// A mesh found by [`walk`] with what it inherits.
struct MeshUse<'a> {
    prim: &'a Prim,
    path: String,
    world: Mat4,
    inherited: Inherited,
}

/// Bindings inherited down the prim tree.
#[derive(Clone, Default)]
struct Inherited {
    skeleton: Option<String>,
    material: Option<String>,
    /// The nearest `Xform` (or typeless prim) name.
    object: String,
}

#[derive(Default)]
struct Walk<'a> {
    meshes: Vec<MeshUse<'a>>,
    skeletons: Vec<(&'a Prim, String, Mat4)>,
    animations: HashMap<String, &'a Prim>,
    materials: HashMap<String, &'a Prim>,
    prims: HashMap<String, &'a Prim>,
}

fn walk<'a>(
    prim: &'a Prim,
    path: String,
    parent_world: Mat4,
    inherited: &Inherited,
    out: &mut Walk<'a>,
    depth: usize,
) {
    if depth > 256 || prim.specifier != Specifier::Def {
        return;
    }
    if prim.meta("active") == Some(&Value::Bool(false)) {
        return;
    }
    out.prims.insert(path.clone(), prim);
    let world = math::multiply(&parent_world, &local_transform(prim));
    let inherited = Inherited {
        skeleton: prim
            .relationship("skel:skeleton")
            .and_then(|t| t.first().cloned())
            .or_else(|| inherited.skeleton.clone()),
        material: prim
            .relationship("material:binding")
            .and_then(|t| t.first().cloned())
            .or_else(|| inherited.material.clone()),
        object: match prim.type_name.as_str() {
            "Xform" | "" => prim.name.clone(),
            _ => inherited.object.clone(),
        },
    };
    match prim.type_name.as_str() {
        "Mesh" => out.meshes.push(MeshUse {
            prim,
            path: path.clone(),
            world,
            inherited: inherited.clone(),
        }),
        "Skeleton" => out.skeletons.push((prim, path.clone(), world)),
        "SkelAnimation" => {
            out.animations.insert(path.clone(), prim);
        }
        "Material" => {
            out.materials.insert(path.clone(), prim);
        }
        _ => {}
    }
    for child in &prim.children {
        walk(
            child,
            format!("{path}/{}", child.name),
            world,
            &inherited,
            out,
            depth + 1,
        );
    }
}

/// Resolves a relative connection or target path against `base`.
fn absolute(path: &str, base: &str) -> String {
    if path.starts_with('/') {
        return path.to_owned();
    }
    let mut parts: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    for c in path.split('/') {
        match c {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            c => parts.push(c),
        }
    }
    format!("/{}", parts.join("/"))
}

fn original_name(prim: &Prim) -> String {
    for key in [
        "doctor:name",
        "userProperties:blender:data_name",
        "userProperties:blender:object_name",
    ] {
        if let Some(s) = prim
            .attribute(key)
            .and_then(Attribute::value)
            .and_then(Value::as_str)
        {
            return s.to_owned();
        }
    }
    prim.name.clone()
}

/// Converts a layer to a scene.
pub fn to_scene(layer: &Layer, files: &SideFiles) -> Result<Scene> {
    let mut scene = Scene::default();
    let up = layer
        .meta("upAxis")
        .and_then(Value::as_str)
        .unwrap_or("Y")
        .to_owned();
    let axes = if up == "Z" {
        // (x, y, z) Z up → (x, z, -y) Y up.
        math::quat_matrix(math::axis_quat(0, -std::f32::consts::FRAC_PI_2))
            .map(|v| (v * 1e6).round() / 1e6)
    } else {
        IDENTITY
    };
    let meters = layer
        .meta("metersPerUnit")
        .and_then(Value::as_f64)
        .filter(|m| m.is_finite() && *m > 0.0)
        .unwrap_or(0.01) as f32;
    let conversion = Conversion {
        axes,
        scale: meters,
    };
    let rate = layer
        .meta("timeCodesPerSecond")
        .and_then(Value::as_f64)
        .or_else(|| layer.meta("framesPerSecond").and_then(Value::as_f64))
        .filter(|r| r.is_finite() && *r > 0.0 && *r <= 1000.0)
        .unwrap_or(24.0) as f32;

    let mut w = Walk::default();
    for prim in &layer.prims {
        let start = Inherited {
            object: prim.name.clone(),
            ..Default::default()
        };
        walk(prim, format!("/{}", prim.name), IDENTITY, &start, &mut w, 0);
    }
    if layer.prims.iter().any(|p| has_composition(p, 0)) {
        scene
            .warnings
            .push("references, payloads and variants are not followed".into());
    }

    // Skeletons and joints.
    let mut skeletons: Vec<Skeleton> = Vec::new();
    let mut joint_by_name: HashMap<String, usize> = HashMap::new();
    for (prim, path, world) in &w.skeletons {
        let joints: Vec<String> = prim
            .attribute("joints")
            .and_then(Attribute::value)
            .and_then(Value::strings)
            .map(<[String]>::to_vec)
            .unwrap_or_default();
        if joints.is_empty() {
            continue;
        }
        let names: Vec<String> = match prim
            .attribute("doctor:jointNames")
            .and_then(Attribute::value)
            .and_then(Value::strings)
        {
            Some(n) if n.len() == joints.len() => n.to_vec(),
            _ => joints
                .iter()
                .map(|j| j.rsplit('/').next().unwrap_or(j).to_owned())
                .collect(),
        };
        let parents: Vec<Option<usize>> = joints
            .iter()
            .map(|j| {
                let (parent, _) = j.rsplit_once('/')?;
                joints.iter().position(|p| p == parent)
            })
            .collect();
        let binds = numbers(prim, "bindTransforms").map(|v| matrices(&v));
        let rests = numbers(prim, "restTransforms").map(|v| matrices(&v));
        let binds = binds.filter(|b| b.len() == joints.len());
        // Rest locals: given, else from the bind pose.
        let rest: Vec<Mat4> = match rests.filter(|r| r.len() == joints.len()) {
            Some(r) => r,
            None => match &binds {
                Some(b) => (0..joints.len())
                    .map(|j| match parents[j] {
                        Some(p) => math::multiply(&math::invert(&b[p]), &b[j]),
                        None => b[j],
                    })
                    .collect(),
                None => vec![IDENTITY; joints.len()],
            },
        };
        // Worlds (file space) of the rest pose, in joint order (parents
        // may come later in malformed files: iterate until settled).
        let mut rest_world: Vec<Option<Mat4>> = vec![None; joints.len()];
        for _ in 0..joints.len() {
            let mut changed = false;
            for j in 0..joints.len() {
                if rest_world[j].is_some() {
                    continue;
                }
                let w = match parents[j] {
                    Some(p) => rest_world[p].map(|pw| math::multiply(&pw, &rest[j])),
                    None => Some(math::multiply(world, &rest[j])),
                };
                if w.is_some() {
                    rest_world[j] = w;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let rest_world: Vec<Mat4> = rest_world
            .into_iter()
            .map(|w| w.unwrap_or(*world))
            .collect();
        let converted: Vec<Mat4> = rest_world.iter().map(|m| conversion.matrix(m)).collect();
        let mut scene_joint = Vec::with_capacity(joints.len());
        for j in 0..joints.len() {
            if let Some(&existing) = joint_by_name.get(&names[j]) {
                scene_joint.push(existing);
                continue;
            }
            let parent = parents[j].and_then(|p| scene_joint.get(p).copied());
            let local = match parents[j] {
                Some(p) => math::multiply(&math::invert(&converted[p]), &converted[j]),
                None => converted[j],
            };
            let bind_world = match &binds {
                Some(b) => conversion.matrix(&math::multiply(world, &b[j])),
                None => converted[j],
            };
            scene.joints.push(SceneJoint {
                name: names[j].clone(),
                parent,
                local,
                inverse_bind: math::invert(&bind_world),
            });
            let index = scene.joints.len() - 1;
            joint_by_name.insert(names[j].clone(), index);
            scene_joint.push(index);
        }
        // Parents must come first: joints out of order are not supported.
        let animation = prim
            .relationship("skel:animationSource")
            .and_then(|t| t.first())
            .map(|t| absolute(t, path));
        skeletons.push(Skeleton {
            path: path.clone(),
            world: *world,
            joints,
            scene_joint,
            rest,
            parents,
            animation,
        });
    }
    if scene.validate_joint_order().is_err() {
        return Err(bad("skeleton joints are not ordered parents first"));
    }

    // Materials.
    let mut material_index: HashMap<String, Option<usize>> = HashMap::new();
    let mut material_of = |path: &str, scene: &mut Scene| -> Option<usize> {
        if let Some(found) = material_index.get(path) {
            return *found;
        }
        let found = w.materials.get(path).map(|prim| {
            let material = read_material(prim, path, &w.prims, files, &mut scene.warnings);
            scene.materials.push(material);
            scene.materials.len() - 1
        });
        material_index.insert(path.to_owned(), found);
        found
    };

    // Meshes.
    let points_conversion = conversion.points();
    for mesh in &w.meshes {
        let (prim, path, world) = (mesh.prim, &mesh.path, &mesh.world);
        let (skeleton_path, material_path, object) = (
            &mesh.inherited.skeleton,
            &mesh.inherited.material,
            &mesh.inherited.object,
        );
        let skeleton = skeleton_path
            .as_ref()
            .map(|p| absolute(p, path))
            .and_then(|p| skeletons.iter().find(|s| s.path == p));
        let mut parts = read_mesh(prim, path, skeleton, &mut scene.warnings)?;
        let name = original_name(prim);
        let object = prim
            .attribute("doctor:object")
            .and_then(Attribute::value)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| {
                if object.is_empty() || prim.name == *object {
                    name.clone()
                } else {
                    object.clone()
                }
            });
        let several = parts.len() > 1;
        for (k, (subset_material, mut g)) in parts.drain(..).enumerate() {
            let space = match (skeleton, g.skin.is_some()) {
                (Some(s), true) => {
                    let bind = numbers(prim, "primvars:skel:geomBindTransform")
                        .and_then(|v| mat(&v))
                        .unwrap_or(IDENTITY);
                    math::multiply(&s.world, &bind)
                }
                _ => *world,
            };
            let transform = math::multiply(&points_conversion, &space);
            let snapped = if transform
                .iter()
                .zip(&IDENTITY)
                .all(|(a, b)| (a - b).abs() < 1e-7)
            {
                IDENTITY
            } else {
                transform
            };
            math::transform_geometry(&snapped, &mut g);
            let binding = subset_material
                .map(|m| absolute(&m, path))
                .or_else(|| material_path.as_ref().map(|m| absolute(m, path)));
            let material = binding.and_then(|m| material_of(&m, &mut scene));
            let part_name = if several {
                format!("{name}.{k}")
            } else {
                name.clone()
            };
            g.name = part_name.clone();
            if scene.name.is_empty() {
                scene.name = object.clone();
            }
            scene.parts.push(ScenePart {
                name: part_name,
                object: object.clone(),
                material,
                geometry: g,
            });
        }
    }

    // Animations: the source of each skeleton, else every animation whose
    // joints a skeleton has.
    let start = layer.meta("startTimeCode").and_then(Value::as_f64);
    let end = layer.meta("endTimeCode").and_then(Value::as_f64);
    let mut used = Vec::new();
    for skeleton in &skeletons {
        let mut sources: Vec<String> = skeleton.animation.iter().cloned().collect();
        for (path, prim) in &w.animations {
            if sources.contains(path) || used.contains(path) {
                continue;
            }
            let joints = prim
                .attribute("joints")
                .and_then(Attribute::value)
                .and_then(Value::strings)
                .unwrap_or(&[]);
            if !joints.is_empty() && joints.iter().all(|j| skeleton.joints.contains(j)) {
                sources.push(path.clone());
            }
        }
        sources.sort();
        if let Some(bound) = &skeleton.animation
            && let Some(i) = sources.iter().position(|s| s == bound)
        {
            let s = sources.remove(i);
            sources.insert(0, s);
        }
        for source in sources {
            if used.contains(&source) {
                continue;
            }
            let Some(prim) = w.animations.get(&source) else {
                continue;
            };
            used.push(source.clone());
            if let Some(clip) = read_animation(prim, skeleton, &conversion, rate, start, end) {
                scene.clips.push(clip);
            }
        }
    }
    Ok(scene)
}

impl Scene {
    fn validate_joint_order(&self) -> std::result::Result<(), ()> {
        if self
            .joints
            .iter()
            .enumerate()
            .all(|(i, j)| j.parent.is_none_or(|p| p < i))
        {
            Ok(())
        } else {
            Err(())
        }
    }
}

fn has_composition(prim: &Prim, depth: usize) -> bool {
    depth < 256
        && (prim.metadata.iter().any(|(k, _)| {
            matches!(
                k.as_str(),
                "references" | "payload" | "inherits" | "specializes" | "variants"
            )
        }) || prim.children.iter().any(|c| has_composition(c, depth + 1)))
}

/// The value a shader input has, following one connection to an output.
enum Input {
    Value(Vec<f64>),
    /// The connected shader.
    Connected(String),
}

fn input(shader: &Prim, shader_path: &str, name: &str) -> Option<Input> {
    let a = shader.attribute(&format!("inputs:{name}"))?;
    if let Some(target) = a.connections.first() {
        let target = absolute(target, shader_path);
        let (prim, _output) = target.split_once('.')?;
        return Some(Input::Connected(prim.to_owned()));
    }
    a.value()?.numbers().map(|v| Input::Value(v.to_vec()))
}

fn read_material(
    prim: &Prim,
    path: &str,
    prims: &HashMap<String, &Prim>,
    files: &SideFiles,
    warnings: &mut Vec<String>,
) -> SceneMaterial {
    let mut material = SceneMaterial {
        name: original_name(prim),
        ..Default::default()
    };
    // The surface shader: outputs:surface (any render context) connection.
    let surface = prim
        .properties
        .iter()
        .find_map(|(name, p)| match p {
            Property::Attribute(a) if name.ends_with("surface") && name.starts_with("outputs:") => {
                a.connections.first().cloned()
            }
            _ => None,
        })
        .map(|t| absolute(&t, path))
        .and_then(|t| t.split_once('.').map(|(p, _)| p.to_owned()))
        .or_else(|| {
            prim.children
                .iter()
                .find(|c| {
                    c.attribute("info:id")
                        .and_then(Attribute::value)
                        .and_then(Value::as_str)
                        == Some("UsdPreviewSurface")
                })
                .map(|c| format!("{path}/{}", c.name))
        });
    let Some(surface_path) = surface else {
        return material;
    };
    let Some(shader) = prims.get(&surface_path) else {
        return material;
    };
    let texture =
        |input_name: &str, warnings: &mut Vec<String>| -> (Option<SceneImage>, Option<Vec<f64>>) {
            match input(shader, &surface_path, input_name) {
                Some(Input::Value(v)) => (None, Some(v)),
                Some(Input::Connected(target)) => {
                    let Some(tex) = prims.get(&target) else {
                        return (None, None);
                    };
                    let file = tex
                        .attribute("inputs:file")
                        .and_then(Attribute::value)
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    match file {
                        Some(f) if !f.is_empty() => match files.image(&f) {
                            Ok(image) => (Some(image), None),
                            Err(e) => {
                                warnings.push(format!("texture `{f}`: {e}"));
                                (None, None)
                            }
                        },
                        _ => (None, None),
                    }
                }
                None => (None, None),
            }
        };
    let (image, value) = texture("diffuseColor", warnings);
    material.base_color = image;
    if let Some(v) = value
        && v.len() >= 3
    {
        material.base_color_factor[..3].copy_from_slice(&[v[0] as f32, v[1] as f32, v[2] as f32]);
    }
    material.normal = texture("normal", warnings).0;
    material.specular = texture("specularColor", warnings).0;
    match input(shader, &surface_path, "opacity") {
        Some(Input::Value(v)) => {
            let a = v.first().copied().unwrap_or(1.0) as f32;
            material.base_color_factor[3] = a;
            material.alpha = a < 1.0;
        }
        Some(Input::Connected(_)) => material.alpha = true,
        None => {}
    }
    material
}

type MeshPart = (Option<String>, crate::resource::mesh_build::PartGeometry);

/// Reads a mesh into one geometry per material subset (with the subset's
/// binding), in the mesh's local space (bind space for skinned meshes).
fn read_mesh(
    prim: &Prim,
    path: &str,
    skeleton: Option<&Skeleton>,
    warnings: &mut Vec<String>,
) -> Result<Vec<MeshPart>> {
    let name = original_name(prim);
    let points: Vec<[f32; 3]> = numbers(prim, "points")
        .unwrap_or_default()
        .chunks_exact(3)
        .map(|c| [c[0] as f32, c[1] as f32, c[2] as f32])
        .collect();
    let counts: Vec<usize> = numbers(prim, "faceVertexCounts")
        .unwrap_or_default()
        .into_iter()
        .map(|c| c.max(0.0) as usize)
        .collect();
    let indices: Vec<usize> = numbers(prim, "faceVertexIndices")
        .unwrap_or_default()
        .into_iter()
        .map(|i| if i < 0.0 { usize::MAX } else { i as usize })
        .collect();
    if counts.iter().sum::<usize>() != indices.len() {
        return Err(bad(format!(
            "mesh `{path}`: face counts and indices differ"
        )));
    }
    if indices.iter().any(|&i| i >= points.len()) {
        return Err(bad(format!("mesh `{path}`: index out of range")));
    }
    let left_handed = prim
        .attribute("orientation")
        .and_then(Attribute::value)
        .and_then(Value::as_str)
        == Some("leftHanded");
    // Primvar lookup: values, indices, interpolation, width.
    struct Primvar {
        values: Vec<f64>,
        indices: Option<Vec<usize>>,
        interpolation: String,
        width: usize,
    }
    let primvar = |name: &str, default: &str| -> Option<Primvar> {
        let a = prim.attribute(name)?;
        let value = a.value()?;
        let (values, width) = match value {
            Value::Array { values, width } => (values.clone(), *width),
            _ => return None,
        };
        let indices = prim
            .attribute(&format!("{name}:indices"))
            .and_then(Attribute::value)
            .and_then(Value::numbers)
            .map(|v| v.iter().map(|&i| i.max(0.0) as usize).collect());
        Some(Primvar {
            values,
            indices,
            interpolation: interpolation(a, default),
            width: width.max(1),
        })
    };
    let get = |p: &Primvar, face: usize, corner: usize, point: usize| -> Option<Vec<f64>> {
        let slot = match p.interpolation.as_str() {
            "constant" => 0,
            "uniform" => face,
            "faceVarying" => corner,
            _ => point,
        };
        let i = match &p.indices {
            Some(ix) => *ix.get(slot)?,
            None => slot,
        };
        p.values
            .get(i * p.width..(i + 1) * p.width)
            .map(<[f64]>::to_vec)
    };
    let normals = primvar("primvars:normals", "vertex").or_else(|| primvar("normals", "vertex"));
    // Texture coordinates: `st` first, then other 2D primvars.
    let mut uv_names: Vec<String> = prim
        .properties
        .iter()
        .filter_map(|(n, p)| match p {
            Property::Attribute(a)
                if n.starts_with("primvars:")
                    && !n.ends_with(":indices")
                    && a.type_name.starts_with("texCoord2")
                    || n == "primvars:st" =>
            {
                Some(n.clone())
            }
            _ => None,
        })
        .collect();
    uv_names.sort_by_key(|n| (n != "primvars:st", n.clone()));
    uv_names.dedup();
    let uv0 = uv_names.first().and_then(|n| primvar(n, "vertex"));
    let uv1 = uv_names.get(1).and_then(|n| primvar(n, "vertex"));
    let colors = primvar("primvars:displayColor", "constant");
    let opacity = primvar("primvars:displayOpacity", "constant");
    // Skin.
    let mut skin_at: Option<Box<dyn Fn(usize) -> Option<crate::scene::VertexSkin>>> = None;
    let mut dropped = false;
    if let Some(s) = skeleton {
        let ji = prim.attribute("primvars:skel:jointIndices");
        let jw = prim.attribute("primvars:skel:jointWeights");
        if let (Some(ji), Some(jw)) = (ji, jw) {
            let size = ji
                .meta("elementSize")
                .and_then(Value::as_f64)
                .unwrap_or(1.0)
                .clamp(1.0, 64.0) as usize;
            let constant = interpolation(ji, "vertex") == "constant";
            let joints = ji.value().and_then(Value::numbers).unwrap_or(&[]).to_vec();
            let weights = jw.value().and_then(Value::numbers).unwrap_or(&[]).to_vec();
            // Mesh-local joint order.
            let local: Option<Vec<usize>> = prim
                .attribute("skel:joints")
                .and_then(Attribute::value)
                .and_then(Value::strings)
                .map(|names| {
                    names
                        .iter()
                        .map(|n| s.joints.iter().position(|j| j == n).unwrap_or(usize::MAX))
                        .collect()
                });
            let map: Vec<usize> = s.scene_joint.clone();
            let mut per_point: Vec<Option<crate::scene::VertexSkin>> = Vec::new();
            let n = if constant { 1 } else { points.len() };
            for p in 0..n {
                let mut influences = Vec::with_capacity(size);
                for k in 0..size {
                    let (Some(&j), Some(&wt)) =
                        (joints.get(p * size + k), weights.get(p * size + k))
                    else {
                        continue;
                    };
                    if j < 0.0 {
                        continue;
                    }
                    let skeleton_joint = match &local {
                        Some(l) => l.get(j as usize).copied().unwrap_or(usize::MAX),
                        None => j as usize,
                    };
                    if let Some(&scene_joint) = map.get(skeleton_joint) {
                        influences.push((scene_joint as u16, wt as f32));
                    }
                }
                let (skin, d) = top_four(&mut influences);
                dropped |= d;
                per_point.push(skin);
            }
            skin_at = Some(Box::new(move |p| {
                let p = if constant { 0 } else { p };
                Some(
                    per_point
                        .get(p)
                        .copied()
                        .flatten()
                        .unwrap_or(([0; 4], [1.0, 0.0, 0.0, 0.0])),
                )
            }));
        }
    }
    if dropped {
        warnings.push(format!(
            "mesh `{name}`: more than 4 weights per vertex; the 4 largest are used"
        ));
    }
    // Faces of each material subset.
    let mut face_group: Vec<usize> = vec![0; counts.len()];
    let mut groups: Vec<Option<String>> = vec![None];
    for child in &prim.children {
        if child.type_name != "GeomSubset" {
            continue;
        }
        let family = child
            .attribute("familyName")
            .and_then(Attribute::value)
            .and_then(Value::as_str);
        if family.is_some_and(|f| f != "materialBind") {
            continue;
        }
        let Some(binding) = child
            .relationship("material:binding")
            .and_then(|t| t.first())
        else {
            continue;
        };
        groups.push(Some(absolute(binding, &format!("{path}/{}", child.name))));
        let g = groups.len() - 1;
        for i in numbers(child, "indices").unwrap_or_default() {
            if let Some(slot) = face_group.get_mut(i.max(0.0) as usize) {
                *slot = g;
            }
        }
    }
    let mut builders: Vec<CornerBuilder> = (0..groups.len())
        .map(|_| CornerBuilder::new(&name))
        .collect();
    let mut corner = 0usize;
    let mut polygon: Vec<Corner> = Vec::new();
    for (face, &count) in counts.iter().enumerate() {
        polygon.clear();
        for k in 0..count {
            let c = corner + k;
            let point = indices[c];
            let n = normals
                .as_ref()
                .and_then(|p| get(p, face, c, point))
                .filter(|v| v.len() >= 3)
                .map(|v| math::normalize3([v[0] as f32, v[1] as f32, v[2] as f32]));
            let uv = |p: &Option<Primvar>| {
                p.as_ref()
                    .and_then(|p| get(p, face, c, point))
                    .filter(|v| v.len() >= 2)
                    .map(|v| [v[0] as f32, 1.0 - v[1] as f32])
            };
            let color = colors
                .as_ref()
                .and_then(|p| get(p, face, c, point))
                .map(|v| {
                    let a = opacity
                        .as_ref()
                        .and_then(|p| get(p, face, c, point))
                        .and_then(|o| o.first().copied())
                        .unwrap_or(1.0);
                    let b = |x: f64| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
                    [
                        b(v.first().copied().unwrap_or(1.0)),
                        b(v.get(1).copied().unwrap_or(1.0)),
                        b(v.get(2).copied().unwrap_or(1.0)),
                        b(a),
                    ]
                });
            polygon.push(Corner {
                point: point as u32,
                position: points[point],
                normal: n,
                uv0: uv(&uv0),
                uv1: uv(&uv1),
                color,
                skin: skin_at.as_ref().and_then(|f| f(point)),
                tangent: None,
            });
        }
        if left_handed {
            polygon.reverse();
        }
        builders[face_group[face]].polygon(&polygon);
        corner += count;
    }
    Ok(groups
        .into_iter()
        .zip(builders)
        .map(|(m, b)| (m, b.finish()))
        .filter(|(_, g)| !g.triangles.is_empty())
        .collect())
}

/// Times of an attribute's samples (or nothing).
fn sample_times(a: Option<&Attribute>) -> Vec<f64> {
    a.map(|a| a.samples.iter().map(|s| s.0).collect())
        .unwrap_or_default()
}

/// The value of an animated array attribute at `time` (linear between
/// samples; quaternions spherically).
fn sample_array(a: &Attribute, time: f64, quat: bool) -> Option<Vec<f64>> {
    if a.samples.is_empty() {
        return a.value()?.numbers().map(<[f64]>::to_vec);
    }
    let s = &a.samples;
    let i = s.partition_point(|x| x.0 <= time);
    let at = |k: usize| s[k].1.numbers().map(<[f64]>::to_vec);
    if i == 0 {
        return at(0);
    }
    if i >= s.len() {
        return at(s.len() - 1);
    }
    let (t0, t1) = (s[i - 1].0, s[i].0);
    let (a0, a1) = (at(i - 1)?, at(i)?);
    if t1 <= t0 || a0.len() != a1.len() {
        return Some(a0);
    }
    let t = ((time - t0) / (t1 - t0)) as f32;
    if quat {
        return Some(
            a0.chunks_exact(4)
                .zip(a1.chunks_exact(4))
                .flat_map(|(x, y)| {
                    let q = math::slerp(
                        [x[0] as f32, x[1] as f32, x[2] as f32, x[3] as f32],
                        [y[0] as f32, y[1] as f32, y[2] as f32, y[3] as f32],
                        t,
                    );
                    q.map(|v| v as f64)
                })
                .collect(),
        );
    }
    Some(
        a0.iter()
            .zip(&a1)
            .map(|(x, y)| x + (y - x) * t as f64)
            .collect(),
    )
}

fn read_animation(
    prim: &Prim,
    skeleton: &Skeleton,
    conversion: &Conversion,
    rate: f32,
    start: Option<f64>,
    end: Option<f64>,
) -> Option<SceneClip> {
    let joints = prim
        .attribute("joints")
        .and_then(Attribute::value)
        .and_then(Value::strings)?
        .to_vec();
    let skel_index: Vec<Option<usize>> = joints
        .iter()
        .map(|j| skeleton.joints.iter().position(|s| s == j))
        .collect();
    let translations = prim.attribute("translations");
    let rotations = prim.attribute("rotations");
    let scales = prim.attribute("scales");
    let mut times: Vec<f64> = [translations, rotations, scales]
        .into_iter()
        .flat_map(sample_times)
        .collect();
    times.sort_by(f64::total_cmp);
    let (first, last) = match (times.first(), times.last()) {
        (Some(&a), Some(&b)) => (start.unwrap_or(a).min(a), end.unwrap_or(b).max(b)),
        _ => (0.0, 0.0),
    };
    let frames = ((last - first).round().max(0.0) as usize + 1).min(100_000);
    // At most 50 million joint transforms per clip.
    if frames.saturating_mul(skeleton.joints.len()) > 50_000_000 {
        return None;
    }
    let n = skeleton.joints.len();
    let mut tracks: Vec<JointTrack> = Vec::new();
    let animated: Vec<usize> = skel_index.iter().flatten().copied().collect();
    for frame in 0..frames {
        let time = first + frame as f64;
        let t = translations.and_then(|a| sample_array(a, time, false));
        let r = rotations.and_then(|a| sample_array(a, time, true));
        let s = scales.and_then(|a| sample_array(a, time, false));
        let mut locals = skeleton.rest.clone();
        for (k, j) in skel_index.iter().enumerate() {
            let Some(j) = *j else { continue };
            let (rt, rr, rs) = math::decompose(&skeleton.rest[j]);
            let pick3 = |v: &Option<Vec<f64>>, d: [f32; 3]| -> [f32; 3] {
                v.as_ref()
                    .and_then(|v| v.get(3 * k..3 * k + 3))
                    .map_or(d, |x| [x[0] as f32, x[1] as f32, x[2] as f32])
            };
            let q = r
                .as_ref()
                .and_then(|v| v.get(4 * k..4 * k + 4))
                .map_or(rr, |x| {
                    math::quat_normalize([x[0] as f32, x[1] as f32, x[2] as f32, x[3] as f32])
                });
            locals[j] = math::compose(pick3(&t, rt), q, pick3(&s, rs));
        }
        // Worlds, converted, then locals in scene space.
        let mut worlds: Vec<Mat4> = Vec::with_capacity(n);
        for (j, local) in locals.iter().enumerate() {
            let w = match skeleton.parents[j].filter(|&p| p < j) {
                Some(p) => math::multiply(&worlds[p], local),
                None => math::multiply(&skeleton.world, local),
            };
            worlds.push(w);
        }
        let converted: Vec<Mat4> = worlds.iter().map(|m| conversion.matrix(m)).collect();
        for &j in &animated {
            let local = match skeleton.parents[j].filter(|&p| p < j) {
                // Local as it is (translation in metres): no precision lost
                // through world matrices far from the origin.
                Some(_) => {
                    let mut m = locals[j];
                    for v in &mut m[12..15] {
                        *v *= conversion.scale;
                    }
                    m
                }
                None => converted[j],
            };
            let (tr, rot, sc) = math::decompose(&local);
            let joint = skeleton.scene_joint[j];
            let track = match tracks.iter_mut().position(|t| t.joint == joint) {
                Some(i) => &mut tracks[i],
                None => {
                    tracks.push(JointTrack {
                        joint,
                        ..Default::default()
                    });
                    tracks.last_mut().expect("pushed")
                }
            };
            let rot = match track.rotations.last() {
                Some(prev) if (0..4).map(|i| prev[i] * rot[i]).sum::<f32>() < 0.0 => {
                    rot.map(|x| -x)
                }
                _ => rot,
            };
            track.rotations.push(rot);
            track.translations.push(tr);
            track.scales.push(sc);
        }
    }
    for t in &mut tracks {
        if t.scales
            .iter()
            .all(|s| s.iter().all(|x| (x - 1.0).abs() < 1e-4))
        {
            t.scales.clear();
        }
    }
    tracks.sort_by_key(|t| t.joint);
    if tracks.is_empty() {
        return None;
    }
    Some(SceneClip {
        name: original_name(prim),
        frame_rate: rate,
        tracks,
    })
}
