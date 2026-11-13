//! Round trips of a synthetic scene through every format.

use super::*;
use crate::scene::math::{axis_quat, compose};

/// A skinned quad on two joints, a static triangle with a textured
/// material, and a three-frame clip.
pub(crate) fn sample() -> Scene {
    let png = crate::resource::pixels::encode_png(2, 2, &[10, 20, 30, 255].repeat(4)).unwrap();
    let normal_png = crate::resource::pixels::encode_png(1, 1, &[128, 128, 255, 255]).unwrap();
    let root_local = compose([0.0, 0.5, 0.0], axis_quat(0, 0.3), [1.0; 3]);
    let child_local = compose([0.0, 1.0, 0.25], axis_quat(2, -0.7), [1.0; 3]);
    let root_world = root_local;
    let child_world = math::multiply(&root_world, &child_local);
    let joints = vec![
        SceneJoint {
            name: "m_root".into(),
            parent: None,
            local: root_local,
            inverse_bind: math::invert(&root_world),
        },
        SceneJoint {
            name: "m_pelvis".into(),
            parent: Some(0),
            local: child_local,
            inverse_bind: math::invert(&child_world),
        },
    ];
    let skinned = PartGeometry {
        name: "body".into(),
        material_hash: 0,
        positions: vec![[0., 0., 0.], [1., 0., 0.], [0., 1.5, 0.], [1., 1.5, 0.25]],
        normals: Some(vec![[0., 0., 1.]; 4]),
        tangents: None,
        uv0: Some(vec![[0., 0.], [1., 0.], [0., 1.], [1., 1.]]),
        uv1: None,
        colors: Some(vec![
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255; 4],
        ]),
        skin: Some(vec![
            ([0, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]),
            ([0, 1, 0, 0], [0.75, 0.25, 0.0, 0.0]),
            ([1, 0, 0, 0], [0.5, 0.5, 0.0, 0.0]),
            ([1, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]),
        ]),
        triangles: vec![[0, 1, 2], [2, 1, 3]],
    };
    let prop = PartGeometry {
        name: "prop".into(),
        material_hash: 0,
        positions: vec![[2., 0., 0.], [3., 0., 0.], [2., 0., -1.]],
        normals: Some(vec![[0., 1., 0.]; 3]),
        tangents: None,
        uv0: Some(vec![[0.0, 0.0], [0.5, 0.0], [0.0, 0.25]]),
        uv1: None,
        colors: None,
        skin: None,
        triangles: vec![[0, 1, 2]],
    };
    let frames = 3;
    let clip = SceneClip {
        name: "wave".into(),
        frame_rate: 30.0,
        tracks: vec![
            JointTrack {
                joint: 0,
                rotations: (0..frames)
                    .map(|f| axis_quat(0, 0.3 + 0.1 * f as f32))
                    .collect(),
                translations: (0..frames)
                    .map(|f| [0.0, 0.5 + 0.05 * f as f32, 0.0])
                    .collect(),
                scales: Vec::new(),
            },
            JointTrack {
                joint: 1,
                rotations: (0..frames)
                    .map(|f| axis_quat(2, -0.7 + 0.2 * f as f32))
                    .collect(),
                translations: vec![[0.0, 1.0, 0.25]; frames],
                scales: Vec::new(),
            },
        ],
    };
    Scene {
        name: "body".into(),
        parts: vec![
            ScenePart {
                name: "body".into(),
                object: "body".into(),
                material: Some(0),
                geometry: skinned,
            },
            ScenePart {
                name: "prop".into(),
                object: "prop".into(),
                material: Some(1),
                geometry: prop,
            },
        ],
        materials: vec![
            SceneMaterial {
                name: "skin_mat".into(),
                base_color: Some(SceneImage::png("skin_d", png)),
                normal: Some(SceneImage::png("skin_n", normal_png)),
                ..Default::default()
            },
            SceneMaterial {
                name: "prop_mat".into(),
                base_color_factor: [0.5, 0.25, 1.0, 1.0],
                ..Default::default()
            },
        ],
        joints,
        clips: vec![clip],
        warnings: Vec::new(),
    }
}

type TriCorner = ([f32; 3], [f32; 3], [f32; 2], Vec<(String, f32)>);

/// Triangles of a part as corner tuples, rotated to start at the smallest
/// position, sorted; skin by joint name.
fn corners(scene: &Scene, part: &ScenePart) -> Vec<[TriCorner; 3]> {
    let g = &part.geometry;
    let mut out: Vec<[TriCorner; 3]> = g
        .triangles
        .iter()
        .map(|t| {
            let c = t.map(|i| {
                let i = i as usize;
                let mut skin: Vec<(String, f32)> = g
                    .skin
                    .as_ref()
                    .map(|s| {
                        (0..4)
                            .filter(|&k| s[i].1[k] > 0.0)
                            .map(|k| (scene.joints[s[i].0[k] as usize].name.clone(), s[i].1[k]))
                            .collect()
                    })
                    .unwrap_or_default();
                skin.sort_by(|a, b| a.0.cmp(&b.0));
                (
                    g.positions[i],
                    g.normals.as_ref().map_or([0.0; 3], |n| n[i]),
                    g.uv0.as_ref().map_or([0.0; 2], |u| u[i]),
                    skin,
                )
            });
            let r = (0..3)
                .min_by_key(|&k| c[k].0.map(|x| (x * 1e4).round() as i64))
                .unwrap();
            [c[r].clone(), c[(r + 1) % 3].clone(), c[(r + 2) % 3].clone()]
        })
        .collect();
    out.sort_by(|a, b| {
        let key = |t: &[TriCorner; 3]| {
            [t[0].0, t[1].0, t[2].0].map(|p| p.map(|x| (x * 1e4).round() as i64))
        };
        key(a).partial_cmp(&key(b)).unwrap()
    });
    out
}

/// What survives a format.
#[derive(Clone, Copy)]
pub(crate) struct Keeps {
    pub skin: bool,
    pub clips: bool,
    /// Joint frames (orientations) are kept; otherwise only joint
    /// positions are compared (other programs may reorient bones).
    pub frames: bool,
}

const EXACT: Keeps = Keeps {
    skin: true,
    clips: true,
    frames: true,
};

/// World positions of the joints at frame `f` of a clip (rest pose for
/// joints without a track).
fn joint_positions(scene: &Scene, clip: Option<&SceneClip>, f: f32) -> Vec<[f32; 3]> {
    let mut locals: Vec<crate::resource::rcb::Mat4> =
        scene.joints.iter().map(|j| j.local).collect();
    if let Some(clip) = clip {
        for t in &clip.tracks {
            let (t0, r0, _) = math::decompose(&locals[t.joint]);
            let tr = math::sample_vec3(&t.translations, f).unwrap_or(t0);
            let r = math::sample_quat(&t.rotations, f).unwrap_or(r0);
            let sc = math::sample_vec3(&t.scales, f).unwrap_or([1.0; 3]);
            locals[t.joint] = math::compose(tr, r, sc);
        }
    }
    let mut worlds: Vec<crate::resource::rcb::Mat4> = Vec::new();
    for (i, j) in scene.joints.iter().enumerate() {
        let w = match j.parent {
            Some(p) => math::multiply(&worlds[p], &locals[i]),
            None => locals[i],
        };
        worlds.push(w);
    }
    worlds.iter().map(math::translation_of).collect()
}

/// Compares two scenes: parts (geometry within `eps`), materials, joints,
/// clips. Returns the first difference.
pub(crate) fn compare(
    a: &Scene,
    b: &Scene,
    eps: f32,
    keeps: Keeps,
) -> std::result::Result<(), String> {
    let close = |x: &[f32], y: &[f32], tol: f32| {
        x.len() == y.len() && x.iter().zip(y).all(|(p, q)| (p - q).abs() <= tol)
    };
    let parts_a: Vec<&ScenePart> = a
        .parts
        .iter()
        .filter(|p| !p.geometry.triangles.is_empty())
        .collect();
    if parts_a.len() != b.parts.len() {
        return Err(format!("{} parts, {} back", parts_a.len(), b.parts.len()));
    }
    // Parts by name (other programs add `.001` suffixes), else in order.
    let base = |n: &str| -> String {
        match n.rsplit_once('.') {
            Some((stem, num)) if num.len() == 3 && num.bytes().all(|b| b.is_ascii_digit()) => {
                stem.to_owned()
            }
            _ => n.to_owned(),
        }
    };
    let by_name = parts_a
        .iter()
        .all(|p| b.parts.iter().filter(|q| base(&q.name) == p.name).count() == 1);
    for (k, pa) in parts_a.iter().enumerate() {
        let pb = if by_name {
            b.parts.iter().find(|q| base(&q.name) == pa.name).unwrap()
        } else {
            &b.parts[k]
        };
        let ca = corners(a, pa);
        let cb = corners(b, pb);
        if ca.len() != cb.len() {
            return Err(format!(
                "part {}: {} triangles, {} back",
                pa.name,
                ca.len(),
                cb.len()
            ));
        }
        for (ta, tb) in ca.iter().zip(&cb) {
            for (x, y) in ta.iter().zip(tb) {
                let scale = 1.0 + x.0.iter().fold(0f32, |m, v| m.max(v.abs()));
                if !close(&x.0, &y.0, eps * scale) {
                    return Err(format!("part {}: position {:?} vs {:?}", pa.name, x.0, y.0));
                }
                if pa.geometry.normals.is_some() && !close(&x.1, &y.1, 1e-3) {
                    return Err(format!("part {}: normal {:?} vs {:?}", pa.name, x.1, y.1));
                }
                if pa.geometry.uv0.is_some()
                    && !close(&x.2, &y.2, 1e-5 * (1.0 + x.2[0].abs().max(x.2[1].abs())))
                {
                    return Err(format!("part {}: uv {:?} vs {:?}", pa.name, x.2, y.2));
                }
                if keeps.skin && pa.geometry.skin.is_some() {
                    let names =
                        |s: &Vec<(String, f32)>| s.iter().map(|p| p.0.clone()).collect::<Vec<_>>();
                    let weights =
                        |s: &Vec<(String, f32)>| s.iter().map(|p| p.1).collect::<Vec<_>>();
                    if names(&x.3) != names(&y.3) || !close(&weights(&x.3), &weights(&y.3), 1e-4) {
                        return Err(format!("part {}: skin {:?} vs {:?}", pa.name, x.3, y.3));
                    }
                }
            }
        }
        let ma = pa.material.map(|m| &a.materials[m]);
        let mb = pb.material.map(|m| &b.materials[m]);
        match (ma, mb) {
            (Some(ma), Some(mb)) => {
                if ma.name != mb.name {
                    return Err(format!("material {} vs {}", ma.name, mb.name));
                }
                for (x, y, role) in [
                    (&ma.base_color, &mb.base_color, "base"),
                    (&ma.normal, &mb.normal, "normal"),
                ] {
                    // Other programs may encode the image again: compare pixels.
                    let pixels = |i: &Option<SceneImage>| i.as_ref().map(|i| i.rgba().ok());
                    if !keeps.frames && y.is_none() {
                        // Programs that rebuild materials may not export every image.
                        eprintln!("material {}: {role} image missing", ma.name);
                        continue;
                    }
                    if x.as_ref().map(|i| &i.bytes) != y.as_ref().map(|i| &i.bytes)
                        && pixels(x) != pixels(y)
                    {
                        return Err(format!("material {}: {role} image differs", ma.name));
                    }
                }
            }
            (None, _) => {}
            (Some(m), None) => return Err(format!("material {} lost", m.name)),
        }
    }
    if !keeps.skin {
        return Ok(());
    }
    if a.joints.len() != b.joints.len() {
        return Err(format!(
            "{} joints, {} back",
            a.joints.len(),
            b.joints.len()
        ));
    }
    let rest_a = joint_positions(a, None, 0.0);
    let rest_b = joint_positions(b, None, 0.0);
    for (ia, ja) in a.joints.iter().enumerate() {
        let ib = b
            .joints
            .iter()
            .position(|j| j.name == ja.name)
            .ok_or_else(|| format!("joint {} lost", ja.name))?;
        let jb = &b.joints[ib];
        let parent = |s: &Scene, j: &SceneJoint| j.parent.map(|p| s.joints[p].name.clone());
        if parent(a, ja) != parent(b, jb) {
            return Err(format!("joint {}: parent differs", ja.name));
        }
        let bind = |j: &SceneJoint| math::translation_of(&math::invert(&j.inverse_bind));
        if !close(&bind(ja), &bind(jb), 1e-4) {
            return Err(format!(
                "joint {}: bind position {:?} vs {:?}",
                ja.name,
                bind(ja),
                bind(jb)
            ));
        }
        if !close(&rest_a[ia], &rest_b[ib], 1e-4) {
            return Err(format!(
                "joint {}: rest position {:?} vs {:?}",
                ja.name, rest_a[ia], rest_b[ib]
            ));
        }
        if keeps.frames {
            if !close(&ja.inverse_bind, &jb.inverse_bind, 1e-4) {
                return Err(format!(
                    "joint {}: inverse bind {:?} vs {:?}",
                    ja.name, ja.inverse_bind, jb.inverse_bind
                ));
            }
            if !close(&ja.local, &jb.local, 1e-4) {
                return Err(format!(
                    "joint {}: local {:?} vs {:?}",
                    ja.name, ja.local, jb.local
                ));
            }
        }
    }
    if !keeps.clips {
        return Ok(());
    }
    if a.clips.len() != b.clips.len() {
        return Err(format!("{} clips, {} back", a.clips.len(), b.clips.len()));
    }
    for (ca, cb) in a.clips.iter().zip(&b.clips) {
        if (keeps.frames && cb.name != ca.name) || ca.frames() != cb.frames() {
            return Err(format!(
                "clip {} ({} frames) vs {} ({} frames)",
                ca.name,
                ca.frames(),
                cb.name,
                cb.frames()
            ));
        }
        for f in 0..ca.frames() {
            let pa = joint_positions(a, Some(ca), f as f32);
            let pb = joint_positions(b, Some(cb), f as f32);
            for (ia, ja) in a.joints.iter().enumerate() {
                let ib = b.joints.iter().position(|j| j.name == ja.name).unwrap();
                if !close(&pa[ia], &pb[ib], 1e-4) {
                    return Err(format!(
                        "clip {}: {} frame {f}: position {:?} vs {:?}",
                        ca.name, ja.name, pa[ia], pb[ib]
                    ));
                }
            }
        }
        if !keeps.frames {
            continue;
        }
        for ta in &ca.tracks {
            let name = &a.joints[ta.joint].name;
            let tb = cb
                .tracks
                .iter()
                .find(|t| b.joints[t.joint].name == *name)
                .ok_or_else(|| format!("clip {}: track {name} lost", ca.name))?;
            for f in 0..ca.frames() {
                let qa = math::sample_quat(&ta.rotations, f as f32).unwrap();
                let qb = math::sample_quat(&tb.rotations, f as f32).unwrap();
                let dot: f32 = (0..4).map(|i| qa[i] * qb[i]).sum();
                if dot.abs() < 0.9999 {
                    return Err(format!(
                        "clip {}: {name} frame {f}: rotation {qa:?} vs {qb:?}",
                        ca.name
                    ));
                }
            }
        }
    }
    Ok(())
}

fn round_trip(format: ModelFormat) -> Scene {
    let scene = sample();
    let files = write(&scene, format, "model").unwrap();
    let memory: HashMap<String, Vec<u8>> = files
        .iter()
        .map(|f| (f.name.clone(), f.bytes.clone()))
        .collect();
    let side = SideFiles::in_memory(memory);
    let bytes = &files[0].bytes;
    let back = match format {
        ModelFormat::Glb | ModelFormat::Gltf => crate::import::gltf::read(bytes, None).unwrap(),
        ModelFormat::Fbx => fbx::read(bytes, &side).unwrap(),
        ModelFormat::Obj => obj::read(bytes, &side, "model").unwrap(),
        ModelFormat::Usdz => usd::read_usdz(bytes).unwrap(),
        _ => usd::read(bytes, &side).unwrap(),
    };
    back.validate().unwrap();
    back
}

#[test]
fn gltf_round_trip() {
    let back = round_trip(ModelFormat::Glb);
    compare(&sample(), &back, 0.0, EXACT).unwrap();
}

#[test]
fn fbx_round_trip() {
    let back = round_trip(ModelFormat::Fbx);
    compare(&sample(), &back, 1e-6, EXACT).unwrap();
    assert_eq!(back.materials[1].base_color_factor, [0.5, 0.25, 1.0, 1.0]);
}

#[test]
fn obj_round_trip() {
    let back = round_trip(ModelFormat::Obj);
    // OBJ keeps no skeleton: the skinned part comes back in bind pose.
    compare(
        &sample(),
        &back,
        0.0,
        Keeps {
            skin: false,
            clips: false,
            frames: false,
        },
    )
    .unwrap();
    assert!(back.joints.is_empty());
    assert_eq!(
        back.parts[0].geometry.colors.as_ref().unwrap()[1],
        [0, 255, 0, 255]
    );
}

#[test]
fn usda_round_trip() {
    let back = round_trip(ModelFormat::Usda);
    compare(&sample(), &back, 0.0, EXACT).unwrap();
}

#[test]
fn usdc_round_trip() {
    let back = round_trip(ModelFormat::Usdc);
    compare(&sample(), &back, 0.0, EXACT).unwrap();
}

#[test]
fn usdz_round_trip() {
    let back = round_trip(ModelFormat::Usdz);
    compare(&sample(), &back, 0.0, EXACT).unwrap();
}

#[test]
fn formats_by_name_and_extension() {
    assert_eq!("usdz".parse::<ModelFormat>().unwrap(), ModelFormat::Usdz);
    assert_eq!(".FBX".parse::<ModelFormat>().unwrap(), ModelFormat::Fbx);
    assert!("dae".parse::<ModelFormat>().is_err());
    assert_eq!(
        ModelFormat::from_path(Path::new("a/b.Obj")),
        Some(ModelFormat::Obj)
    );
}

#[test]
fn side_files_stay_in_the_folder() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("t.png"), b"\x89PNGxx").unwrap();
    let files = SideFiles::new(Some(dir.path()));
    assert!(files.read("t.png").is_ok());
    // Outside paths are tried by file name only.
    assert!(files.read("../../etc/t.png").is_ok());
    assert!(files.read("C:\\textures\\t.png").is_ok());
    assert!(files.read("/etc/passwd").is_err());
    assert!(files.read("../x/missing.png").is_err());
}

#[test]
fn corner_builder_shares_equal_corners() {
    let mut b = CornerBuilder::new("p");
    let c = |p: u32, uv: [f32; 2]| Corner {
        point: p,
        position: [p as f32, 0.0, (p % 2) as f32],
        uv0: Some(uv),
        ..Default::default()
    };
    b.polygon(&[
        c(0, [0.0, 0.0]),
        c(1, [1.0, 0.0]),
        c(2, [1.0, 1.0]),
        c(3, [0.0, 1.0]),
    ]);
    b.polygon(&[c(0, [0.0, 0.0]), c(2, [0.5, 0.5]), c(4, [0.0, 0.5])]);
    let g = b.finish();
    assert_eq!(g.triangles.len(), 3);
    // Point 2 with another UV is a new vertex; point 0 is shared.
    assert_eq!(g.positions.len(), 6);
}

#[test]
fn four_largest_weights_are_kept() {
    let mut w = vec![(3, 0.1), (1, 0.4), (2, 0.2), (1, 0.1), (5, 0.15), (6, 0.05)];
    let (skin, dropped) = top_four(&mut w);
    let (j, w) = skin.unwrap();
    assert!(dropped);
    assert_eq!(j, [1, 2, 5, 3]);
    assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-6);
    assert_eq!(top_four(&mut vec![(1, 0.0)]).0, None);
}

/// Writes the sample scene in every format into `$DOCTOR_SCENE_SAMPLES`
/// (for checks with other programs).
#[test]
#[ignore]
fn write_samples() {
    let Ok(dir) = std::env::var("DOCTOR_SCENE_SAMPLES") else {
        return;
    };
    for format in ModelFormat::WRITABLE {
        let path = Path::new(&dir).join(format!("sample.{}", format.extension()));
        write_file(&sample(), &path, None).unwrap();
    }
}

/// Reads `$DOCTOR_SCENE_COMPARE` (a file another program wrote from the
/// sample scene) and compares it with the sample: geometry, skin, joint
/// positions and animated joint positions.
#[test]
#[ignore]
fn compare_with_sample() {
    let Ok(path) = std::env::var("DOCTOR_SCENE_COMPARE") else {
        return;
    };
    let back = read_file(Path::new(&path)).unwrap();
    eprintln!(
        "{path}: {} parts, {} materials, {} joints, {} clips, warnings {:?}",
        back.parts.len(),
        back.materials.len(),
        back.joints.len(),
        back.clips.len(),
        back.warnings
    );
    for m in &back.materials {
        let size = |i: &Option<SceneImage>| {
            i.as_ref().map(|i| {
                (
                    i.name.clone(),
                    i.rgba().map(|r| (r.0, r.1, r.2[..4].to_vec())).ok(),
                )
            })
        };
        eprintln!(
            "  material {} base {:?} normal {:?} factor {:?}",
            m.name,
            size(&m.base_color),
            size(&m.normal),
            m.base_color_factor
        );
    }
    for p in &back.parts {
        eprintln!(
            "  part {} ({}) {} vertices {} triangles {:?}",
            p.name,
            p.object,
            p.geometry.positions.len(),
            p.geometry.triangles.len(),
            &p.geometry.positions[..p.geometry.positions.len().min(4)]
        );
    }
    for c in &back.clips {
        eprintln!(
            "  clip {} {} frames at {} fps, {} tracks",
            c.name,
            c.frames(),
            c.frame_rate,
            c.tracks.len()
        );
    }
    let skin = std::env::var("DOCTOR_SCENE_SKIN").map_or(true, |v| v != "0");
    let keeps = Keeps {
        skin,
        clips: skin,
        frames: false,
    };
    compare(&sample(), &back, 1e-5, keeps).unwrap();
}

/// Prints the node tree of `$DOCTOR_FBX_DUMP` (debugging).
#[test]
#[ignore]
fn dump_fbx() {
    let Ok(path) = std::env::var("DOCTOR_FBX_DUMP") else {
        return;
    };
    let doc = fbx::node::parse(&std::fs::read(path).unwrap()).unwrap();
    fn show(n: &fbx::node::Node, depth: usize) {
        let props: Vec<String> = n
            .props
            .iter()
            .map(|p| {
                let s = match p {
                    fbx::node::Prop::String(s) => format!("{:?}", String::from_utf8_lossy(s)),
                    p => format!("{p:?}"),
                };
                if s.len() > 80 {
                    format!("{}…", &s[..80])
                } else {
                    s
                }
            })
            .collect();
        eprintln!("{}{}: {}", "  ".repeat(depth), n.name, props.join(", "));
        for c in &n.children {
            show(c, depth + 1);
        }
    }
    eprintln!("version {}", doc.version);
    for n in &doc.nodes {
        show(n, 0);
    }
}

/// Geometry and skeleton of a scene independent of triangulation and
/// vertex splitting: unique positions (with their skin), area per
/// material, joints and animated joint positions.
fn loose_compare(a: &Scene, b: &Scene, skin: bool) -> std::result::Result<(), String> {
    use std::collections::BTreeMap;
    type Key = [i64; 3];
    let key = |p: [f32; 3]| p.map(|x| (x * 1e3).round() as i64);
    // Several parts may have vertices at one position: a set of skins each.
    type Skins = Vec<Vec<(String, i64)>>;
    let points = |s: &Scene| -> BTreeMap<Key, Skins> {
        let mut out = BTreeMap::new();
        for part in &s.parts {
            let g = &part.geometry;
            for (i, p) in g.positions.iter().enumerate() {
                let mut influences: Vec<(String, i64)> = g
                    .skin
                    .as_ref()
                    .map(|sk| {
                        (0..4)
                            .filter(|&k| sk[i].1[k] > 1e-3)
                            .map(|k| {
                                (
                                    format!(
                                        "{:08x}",
                                        crate::import::model::joint_hash(
                                            &s.joints[sk[i].0[k] as usize].name
                                        )
                                    ),
                                    (sk[i].1[k] * 100.0).round() as i64,
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                influences.sort();
                let list: &mut Skins = out.entry(key(*p)).or_default();
                if !list.contains(&influences) {
                    list.push(influences);
                    list.sort();
                }
            }
        }
        out
    };
    let area = |s: &Scene| -> BTreeMap<String, f32> {
        let mut out = BTreeMap::new();
        for part in &s.parts {
            let g = &part.geometry;
            let name = part
                .material
                .map_or(String::new(), |m| s.materials[m].name.clone());
            let total: f32 = g
                .triangles
                .iter()
                .map(|t| {
                    let [a, b, c] = t.map(|i| g.positions[i as usize]);
                    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                    let n = [
                        u[1] * v[2] - u[2] * v[1],
                        u[2] * v[0] - u[0] * v[2],
                        u[0] * v[1] - u[1] * v[0],
                    ];
                    (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() / 2.0
                })
                .sum();
            *out.entry(name).or_insert(0.0) += total;
        }
        out
    };
    let (pa, mut pb) = (points(a), points(b));
    // Points on a rounding boundary: take the neighbour key.
    for k in pa.keys() {
        if pb.contains_key(k) {
            continue;
        }
        let near = (0..27).find_map(|n| {
            let d = [n % 3, n / 3 % 3, n / 9].map(|x| x as i64 - 1);
            let c = [k[0] + d[0], k[1] + d[1], k[2] + d[2]];
            (!pa.contains_key(&c))
                .then_some(c)
                .filter(|c| pb.contains_key(c))
        });
        if let Some(c) = near {
            let v = pb.remove(&c).unwrap();
            pb.insert(*k, v);
        }
    }
    if pa.keys().ne(pb.keys()) {
        let only_a: Vec<_> = pa.keys().filter(|k| !pb.contains_key(*k)).take(3).collect();
        let only_b: Vec<_> = pb.keys().filter(|k| !pa.contains_key(*k)).take(3).collect();
        return Err(format!(
            "positions differ ({} vs {}): only in reference {only_a:?}, only here {only_b:?}",
            pa.len(),
            pb.len()
        ));
    }
    if skin {
        for (k, wa) in &pa {
            let wb = &pb[k];
            let same = |x: &Vec<(String, i64)>, y: &Vec<(String, i64)>| {
                // Weights below 1 % may be dropped or kept.
                let big = |v: &Vec<(String, i64)>| -> Vec<(String, i64)> {
                    v.iter().filter(|w| w.1 > 1).cloned().collect()
                };
                let (x, y) = (big(x), big(y));
                x.len() == y.len()
                    && x.iter()
                        .zip(&y)
                        .all(|(p, q)| p.0 == q.0 && (p.1 - q.1).abs() <= 1)
            };
            let close = wa.iter().all(|x| wb.iter().any(|y| same(x, y)))
                && wb.iter().all(|y| wa.iter().any(|x| same(x, y)));
            if !close {
                return Err(format!("skin at {k:?}: {wa:?} vs {wb:?}"));
            }
        }
    }
    let (aa, ab) = (area(a), area(b));
    for (name, x) in &aa {
        let y = ab.get(name).copied().unwrap_or(0.0);
        if (x - y).abs() > 1e-3 * x.max(1.0) {
            return Err(format!("area of material `{name}`: {x} vs {y} ({ab:?})"));
        }
    }
    if !skin {
        return Ok(());
    }
    let hash = |n: &str| crate::import::model::joint_hash(n);
    let names = |s: &Scene| {
        s.joints
            .iter()
            .map(|j| hash(&j.name))
            .collect::<std::collections::BTreeSet<_>>()
    };
    if names(a) != names(b) {
        return Err(format!("joints {:?} vs {:?}", names(a), names(b)));
    }
    let close3 = |x: [f32; 3], y: [f32; 3]| (0..3).all(|i| (x[i] - y[i]).abs() < 1e-3);
    for ja in &a.joints {
        let jb = b
            .joints
            .iter()
            .find(|j| hash(&j.name) == hash(&ja.name))
            .unwrap();
        let bind = |j: &SceneJoint| math::translation_of(&math::invert(&j.inverse_bind));
        // Without meshes the bind pose is whatever the program keeps as rest.
        if !b.parts.is_empty() && !close3(bind(ja), bind(jb)) {
            return Err(format!(
                "joint {}: bind {:?} vs {:?}",
                ja.name,
                bind(ja),
                bind(jb)
            ));
        }
    }
    // Programs may add a clip of the rest pose to files without animation.
    if a.clips.len() != b.clips.len() && !a.clips.is_empty() {
        return Err(format!("{} clips vs {}", a.clips.len(), b.clips.len()));
    }
    for (ca, cb) in a.clips.iter().zip(&b.clips) {
        let frames = ca.frames().min(cb.frames());
        if ca.frames().abs_diff(cb.frames()) > 1 {
            return Err(format!(
                "clip {}: {} frames vs {}",
                ca.name,
                ca.frames(),
                cb.frames()
            ));
        }
        for f in 0..frames {
            let xa = joint_positions(a, Some(ca), f as f32);
            let xb = joint_positions(b, Some(cb), f as f32);
            for (ia, ja) in a.joints.iter().enumerate() {
                let ib = b
                    .joints
                    .iter()
                    .position(|j| hash(&j.name) == hash(&ja.name))
                    .unwrap();
                if !close3(xa[ia], xb[ib]) {
                    return Err(format!(
                        "clip {} frame {f}: joint {} at {:?} vs {:?}",
                        ca.name, ja.name, xa[ia], xb[ib]
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Compares `$DOCTOR_SCENE_COMPARE` with `$DOCTOR_SCENE_REFERENCE` (two
/// exports of one scene by another program) with [`loose_compare`].
#[test]
#[ignore]
fn compare_files() {
    let (Ok(reference), Ok(path)) = (
        std::env::var("DOCTOR_SCENE_REFERENCE"),
        std::env::var("DOCTOR_SCENE_COMPARE"),
    ) else {
        return;
    };
    let a = read_file(Path::new(&reference)).unwrap();
    let b = read_file(Path::new(&path)).unwrap();
    let skin = std::env::var("DOCTOR_SCENE_SKIN").map_or(true, |v| v != "0");
    eprintln!(
        "{path}: {} parts, {} materials {:?}, {} joints, clips {:?}, warnings {:?}",
        b.parts.len(),
        b.materials.len(),
        b.materials
            .iter()
            .map(|m| (m.name.clone(), m.base_color.is_some()))
            .collect::<Vec<_>>(),
        b.joints.len(),
        b.clips
            .iter()
            .map(|c| (c.name.clone(), c.frames(), c.frame_rate))
            .collect::<Vec<_>>(),
        b.warnings
    );
    loose_compare(&a, &b, skin).unwrap();
}

/// Reads the clips of `$DOCTOR_SCENE_COMPARE` onto the skeleton of
/// `$DOCTOR_SCENE_REFERENCE` (a clip exported from the game) with
/// [`anim::clips_to_game`] and compares the bone tracks with the
/// reference's own.
#[test]
#[ignore]
fn compare_game_clips() {
    use crate::resource::rcb::{Bone, Skeleton};
    let (Ok(reference), Ok(path)) = (
        std::env::var("DOCTOR_SCENE_REFERENCE"),
        std::env::var("DOCTOR_SCENE_COMPARE"),
    ) else {
        return;
    };
    let a = read_file(Path::new(&reference)).unwrap();
    let b = read_file(Path::new(&path)).unwrap();
    let skeleton = Skeleton {
        name_hash: 0,
        name: "reference".into(),
        bones: a
            .joints
            .iter()
            .map(|j| Bone {
                hash: crate::import::model::joint_hash(&j.name),
                name: None,
                parent: j.parent,
                bind_index: -1,
                bind_type: 1,
                inverse_bind: j.inverse_bind,
            })
            .collect(),
    };
    let options = anim::ClipOptions::default();
    let ta = anim::clips_to_game(&a, &skeleton, &options).unwrap();
    let tb = anim::clips_to_game(&b, &skeleton, &options).unwrap();
    let (ga, gb) = (&ta[0], &tb[0]);
    eprintln!(
        "{path}: {} frames, rest {:?}, unmapped {:?}",
        gb.frames, gb.rest, gb.unmapped
    );
    assert_eq!(ga.frames, gb.frames);
    let mut worst_dot = 1f32;
    let mut worst_t = 0f32;
    for (x, y) in ga.tracks.iter().zip(&gb.tracks) {
        for f in 0..ga.frames {
            let (q, p) = (x.rotations[f], y.rotations[f]);
            worst_dot = worst_dot.min((0..4).map(|i| q[i] * p[i]).sum::<f32>().abs());
            let d = (0..3)
                .map(|i| (x.translations[f][i] - y.translations[f][i]).abs())
                .fold(0f32, f32::max);
            worst_t = worst_t.max(d);
        }
    }
    eprintln!("worst rotation dot {worst_dot}, worst translation {worst_t}");
    assert!(worst_dot > 0.9999 && worst_t < 1e-3);
}
