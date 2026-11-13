//! Wavefront OBJ with an MTL material library.
//!
//! OBJ holds static meshes only: positions, texture coordinates, normals,
//! per-vertex colours (the common `v x y z r g b` extension), objects
//! (`o`), groups (`g`) and materials (`usemtl`). It has **no skeleton, bone
//! weights or animation**: a skinned scene is written in its bind pose, and
//! an OBJ imports as an unskinned model.
//!
//! Reading: each run of faces with one object/group and one material is a
//! part; polygons are triangulated; negative indices are supported. Axes
//! and units are taken as they are (Y up, metres; Blender's default OBJ
//! export is Y up). The MTL gives names, `Kd`, `d`/`Tr`, and the textures
//! `map_Kd`, `map_Bump`/`bump`/`norm` and `map_Ks`.
//!
//! Writing: one `o` per object, one `g` per part, the MTL next to the OBJ
//! and textures as PNG/JPEG files in `textures/`. V is flipped (OBJ has the
//! texture origin at the bottom left).

use std::collections::HashMap;
use std::fmt::Write as _;

use super::{
    Corner, CornerBuilder, ImageFiles, Scene, SceneMaterial, ScenePart, SideFiles, WrittenFile,
};
use crate::{Error, Result};

/// Largest number of elements of any kind accepted (vertices, faces…).
const MAX_ELEMENTS: usize = 50_000_000;

fn bad(line: usize, message: impl std::fmt::Display) -> Error {
    Error::InvalidArgument(format!("OBJ line {line}: {message}"))
}

fn floats<'a>(fields: impl Iterator<Item = &'a str>) -> Vec<f32> {
    fields.map_while(|f| f.parse::<f32>().ok()).collect()
}

struct Group {
    object: String,
    name: String,
    material: Option<usize>,
    builder: CornerBuilder,
}

/// Reads an OBJ file; `files` resolves the material library and textures.
pub fn read(bytes: &[u8], files: &SideFiles, name: &str) -> Result<Scene> {
    let text = String::from_utf8_lossy(bytes);
    let mut scene = Scene {
        name: String::new(),
        ..Default::default()
    };
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut colors: Vec<Option<[u8; 4]>> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut material_index: HashMap<String, usize> = HashMap::new();
    let mut groups: Vec<Group> = Vec::new();
    let mut current: Option<usize> = None;
    let mut object = String::new();
    let mut group = String::new();
    let mut material: Option<usize> = None;
    let mut skipped = 0usize;
    let mut has_colors = false;
    let mut faces = 0usize;
    let mut lines = text.lines().enumerate().peekable();
    while let Some((number, raw)) = lines.next() {
        let number = number + 1;
        // Line continuations.
        let mut line = raw.trim_end().to_owned();
        while line.ends_with('\\') {
            line.pop();
            match lines.next() {
                Some((_, next)) => line.push_str(next.trim_end()),
                None => break,
            }
        }
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let (keyword, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        let mut fields = rest.split_whitespace();
        match keyword {
            "v" => {
                let v = floats(&mut fields);
                if v.len() < 3 {
                    return Err(bad(number, "vertex needs three coordinates"));
                }
                if positions.len() >= MAX_ELEMENTS {
                    return Err(bad(number, "too many vertices"));
                }
                positions.push([v[0], v[1], v[2]]);
                // `v x y z r g b` (0..1) or `v x y z w`.
                if v.len() >= 6 {
                    has_colors = true;
                    let c = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
                    colors.push(Some([c(v[3]), c(v[4]), c(v[5]), 255]));
                } else {
                    colors.push(None);
                }
            }
            "vt" => {
                let v = floats(&mut fields);
                if v.is_empty() {
                    return Err(bad(number, "texture coordinate needs a value"));
                }
                if uvs.len() >= MAX_ELEMENTS {
                    return Err(bad(number, "too many texture coordinates"));
                }
                uvs.push([v[0], 1.0 - v.get(1).copied().unwrap_or(0.0)]);
            }
            "vn" => {
                let v = floats(&mut fields);
                if v.len() < 3 {
                    return Err(bad(number, "normal needs three coordinates"));
                }
                if normals.len() >= MAX_ELEMENTS {
                    return Err(bad(number, "too many normals"));
                }
                normals.push(super::math::normalize3([v[0], v[1], v[2]]));
            }
            "f" | "fo" => {
                let mut corners = Vec::new();
                for field in fields {
                    let mut refs = field.split('/');
                    let index = |s: Option<&str>, len: usize| -> Result<Option<usize>> {
                        match s.filter(|s| !s.is_empty()) {
                            None => Ok(None),
                            Some(s) => {
                                let i: i64 = s
                                    .parse()
                                    .map_err(|_| bad(number, format!("bad index `{s}`")))?;
                                let resolved = if i < 0 { len as i64 + i } else { i - 1 };
                                if resolved < 0 || resolved as usize >= len {
                                    return Err(bad(number, format!("index {i} out of range")));
                                }
                                Ok(Some(resolved as usize))
                            }
                        }
                    };
                    let p = index(refs.next(), positions.len())?
                        .ok_or_else(|| bad(number, "face corner without a vertex"))?;
                    let t = index(refs.next(), uvs.len())?;
                    let n = index(refs.next(), normals.len())?;
                    corners.push(Corner {
                        point: p as u32,
                        position: positions[p],
                        normal: n.map(|n| normals[n]),
                        uv0: t.map(|t| uvs[t]),
                        color: if has_colors {
                            Some(colors[p].unwrap_or([255; 4]))
                        } else {
                            None
                        },
                        ..Default::default()
                    });
                }
                if corners.len() < 3 {
                    skipped += 1;
                    continue;
                }
                faces += 1;
                if faces > MAX_ELEMENTS {
                    return Err(bad(number, "too many faces"));
                }
                let g = match current {
                    Some(g) => g,
                    None => {
                        let label = if !group.is_empty() {
                            group.clone()
                        } else if !object.is_empty() {
                            object.clone()
                        } else {
                            name.to_owned()
                        };
                        let obj = if object.is_empty() {
                            label.clone()
                        } else {
                            object.clone()
                        };
                        groups.push(Group {
                            object: obj,
                            name: label.clone(),
                            material,
                            builder: CornerBuilder::new(&label),
                        });
                        current = Some(groups.len() - 1);
                        groups.len() - 1
                    }
                };
                groups[g].builder.polygon(&corners);
            }
            "l" | "p" => skipped += 1,
            "o" => {
                object = rest.to_owned();
                group.clear();
                current = None;
            }
            "g" => {
                group = rest.to_owned();
                current = None;
            }
            "usemtl" => {
                let m = match material_index.get(rest) {
                    Some(&m) => m,
                    None => {
                        scene.materials.push(SceneMaterial {
                            name: rest.to_owned(),
                            ..Default::default()
                        });
                        material_index.insert(rest.to_owned(), scene.materials.len() - 1);
                        scene.materials.len() - 1
                    }
                };
                if material != Some(m) {
                    material = Some(m);
                    current = None;
                }
            }
            "mtllib" => {
                // File names may contain spaces; try the whole rest first.
                let candidates: Vec<String> = std::iter::once(rest.to_owned())
                    .chain(rest.split_whitespace().map(str::to_owned))
                    .collect();
                let mut loaded = false;
                for file in candidates {
                    if let Ok(bytes) = files.read(&file) {
                        read_mtl(&bytes, files, &mut scene, &mut material_index);
                        loaded = true;
                        break;
                    }
                }
                if !loaded {
                    scene
                        .warnings
                        .push(format!("material library `{rest}` not found"));
                }
            }
            _ => {}
        }
    }
    if skipped > 0 {
        scene.warnings.push(format!(
            "{skipped} point, line or degenerate element(s) skipped"
        ));
    }
    // Several groups of one name and material (faces interleaved) stay
    // separate parts; give them unique names.
    let mut seen: HashMap<String, usize> = HashMap::new();
    for g in groups {
        if g.builder.geometry.triangles.is_empty() {
            continue;
        }
        let count = seen.entry(g.name.clone()).or_insert(0);
        *count += 1;
        let part_name = if *count == 1 {
            g.name.clone()
        } else {
            format!("{}.{}", g.name, *count - 1)
        };
        let mut geometry = g.builder.finish();
        geometry.name = part_name.clone();
        if scene.name.is_empty() {
            scene.name = g.object.clone();
        }
        scene.parts.push(ScenePart {
            name: part_name,
            object: g.object,
            material: g.material,
            geometry,
        });
    }
    // Parts named after the material when one object has several materials.
    if !scene.parts.is_empty() {
        scene
            .warnings
            .push("OBJ has no skeleton or bone weights: the model is imported unskinned".into());
    }
    Ok(scene)
}

/// Splits texture options (`-bm 1 -o 0 0 0 file.png`) from the file name.
fn map_file(rest: &str) -> String {
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    let mut i = 0;
    while i < tokens.len() && tokens[i].starts_with('-') && tokens[i].len() > 1 {
        let option = tokens[i];
        i += 1;
        let max = match option {
            "-o" | "-s" | "-t" => 3,
            "-mm" => 2,
            _ => 1,
        };
        let mut taken = 0;
        while taken < max
            && i < tokens.len()
            && (tokens[i].parse::<f32>().is_ok() || matches!(tokens[i], "on" | "off") || max == 1)
        {
            i += 1;
            taken += 1;
            if max == 1 {
                break;
            }
        }
    }
    tokens[i.min(tokens.len())..].join(" ")
}

fn read_mtl(
    bytes: &[u8],
    files: &SideFiles,
    scene: &mut Scene,
    index: &mut HashMap<String, usize>,
) {
    let text = String::from_utf8_lossy(bytes);
    let mut current: Option<usize> = None;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let (keyword, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        if keyword == "newmtl" {
            current = Some(match index.get(rest) {
                Some(&m) => m,
                None => {
                    scene.materials.push(SceneMaterial {
                        name: rest.to_owned(),
                        ..Default::default()
                    });
                    index.insert(rest.to_owned(), scene.materials.len() - 1);
                    scene.materials.len() - 1
                }
            });
            continue;
        }
        let Some(m) = current else { continue };
        let image = |rest: &str, warnings: &mut Vec<String>| {
            let file = map_file(rest);
            match files.image(&file) {
                Ok(image) => Some(image),
                Err(e) => {
                    warnings.push(format!("texture `{file}`: {e}"));
                    None
                }
            }
        };
        match keyword.to_ascii_lowercase().as_str() {
            "kd" => {
                let v = floats(rest.split_whitespace());
                if v.len() >= 3 {
                    let f = &mut scene.materials[m].base_color_factor;
                    f[..3].copy_from_slice(&v[..3]);
                }
            }
            "d" => {
                if let Some(&d) = floats(rest.split_whitespace()).first() {
                    scene.materials[m].base_color_factor[3] = d;
                    scene.materials[m].alpha |= d < 1.0;
                }
            }
            "tr" => {
                if let Some(&t) = floats(rest.split_whitespace()).first() {
                    scene.materials[m].base_color_factor[3] = 1.0 - t;
                    scene.materials[m].alpha |= t > 0.0;
                }
            }
            "map_kd" => {
                let img = image(rest, &mut scene.warnings);
                scene.materials[m].base_color = img;
            }
            "map_bump" | "bump" | "norm" | "map_kn" => {
                let img = image(rest, &mut scene.warnings);
                scene.materials[m].normal = img;
            }
            "map_ks" => {
                let img = image(rest, &mut scene.warnings);
                scene.materials[m].specular = img;
            }
            "map_d" => scene.materials[m].alpha = true,
            _ => {}
        }
    }
}

fn number(out: &mut String, v: f32) {
    if v == v.trunc() && v.abs() < 1e7 {
        let _ = write!(out, "{}", v as i64);
    } else {
        let _ = write!(out, "{v}");
    }
}

fn name_of(name: &str) -> String {
    // Names end at the line; keep them on one line without `#`.
    let s: String = name
        .chars()
        .map(|c| if c.is_control() || c == '#' { '_' } else { c })
        .collect();
    let s = s.trim().to_owned();
    if s.is_empty() { "unnamed".into() } else { s }
}

/// Writes a scene as `stem.obj`, `stem.mtl` and the textures under
/// `textures` (a relative folder, `textures/` by default).
pub fn write(scene: &Scene, stem: &str, textures: &str) -> Result<Vec<WrittenFile>> {
    let images = ImageFiles::new(scene, textures);
    let mut obj = String::new();
    let _ = writeln!(
        obj,
        "# {}",
        concat!("doctor_toolkit ", env!("CARGO_PKG_VERSION"))
    );
    if !scene.joints.is_empty() {
        let _ = writeln!(
            obj,
            "# OBJ keeps no skeleton: {} joints and the bone weights are not written",
            scene.joints.len()
        );
    }
    let _ = writeln!(obj, "mtllib {stem}.mtl");
    let material_names: Vec<String> = unique_names(scene.materials.iter().map(|m| m.name.as_str()));
    let (mut base_v, mut base_t, mut base_n) = (1usize, 1usize, 1usize);
    let mut last_object: Option<&str> = None;
    for part in &scene.parts {
        let g = &part.geometry;
        if g.triangles.is_empty() {
            continue;
        }
        if last_object != Some(part.object.as_str()) {
            let _ = writeln!(obj, "o {}", name_of(&part.object));
            last_object = Some(&part.object);
        }
        let _ = writeln!(obj, "g {}", name_of(&part.name));
        for (i, p) in g.positions.iter().enumerate() {
            obj.push('v');
            for &x in p {
                obj.push(' ');
                number(&mut obj, x);
            }
            if let Some(colors) = &g.colors {
                for &c in &colors[i][..3] {
                    obj.push(' ');
                    number(&mut obj, (c as f32 / 255.0 * 10000.0).round() / 10000.0);
                }
            }
            obj.push('\n');
        }
        if let Some(uv) = &g.uv0 {
            for t in uv {
                obj.push_str("vt ");
                number(&mut obj, t[0]);
                obj.push(' ');
                number(&mut obj, 1.0 - t[1]);
                obj.push('\n');
            }
        }
        if let Some(normals) = &g.normals {
            for n in normals {
                obj.push_str("vn");
                for &x in n {
                    obj.push(' ');
                    number(&mut obj, x);
                }
                obj.push('\n');
            }
        }
        match part.material {
            Some(m) => {
                let _ = writeln!(obj, "usemtl {}", material_names[m]);
            }
            None => {
                let _ = writeln!(obj, "usemtl default");
            }
        }
        obj.push_str("s off\n");
        for t in &g.triangles {
            obj.push('f');
            for &i in t {
                let i = i as usize;
                let _ = match (g.uv0.is_some(), g.normals.is_some()) {
                    (true, true) => write!(obj, " {}/{}/{}", base_v + i, base_t + i, base_n + i),
                    (true, false) => write!(obj, " {}/{}", base_v + i, base_t + i),
                    (false, true) => write!(obj, " {}//{}", base_v + i, base_n + i),
                    (false, false) => write!(obj, " {}", base_v + i),
                };
            }
            obj.push('\n');
        }
        base_v += g.positions.len();
        if let Some(uv) = &g.uv0 {
            base_t += uv.len();
        }
        if let Some(n) = &g.normals {
            base_n += n.len();
        }
    }
    let mut mtl = String::new();
    let _ = writeln!(
        mtl,
        "# {}",
        concat!("doctor_toolkit ", env!("CARGO_PKG_VERSION"))
    );
    let uses_default = scene.parts.iter().any(|p| p.material.is_none());
    if uses_default {
        mtl.push_str("\nnewmtl default\nKd 0.8 0.8 0.8\nd 1\nillum 1\n");
    }
    for (material, name) in scene.materials.iter().zip(&material_names) {
        let _ = writeln!(mtl, "\nnewmtl {name}");
        let f = material.base_color_factor;
        let _ = writeln!(mtl, "Kd {} {} {}", f[0], f[1], f[2]);
        let _ = writeln!(
            mtl,
            "Ks 0 0 0\nd {}\nillum {}",
            f[3],
            if material.specular.is_some() { 2 } else { 1 }
        );
        for (keyword, image) in [
            ("map_Kd", &material.base_color),
            ("map_Bump", &material.normal),
            ("map_Ks", &material.specular),
        ] {
            if let Some(file) = image.as_ref().and_then(|i| images.name(i)) {
                let _ = writeln!(mtl, "{keyword} {file}");
            }
        }
        if material.alpha
            && material.base_color.is_some()
            && let Some(file) = material.base_color.as_ref().and_then(|i| images.name(i))
        {
            let _ = writeln!(mtl, "map_d {file}");
        }
    }
    let mut out = vec![
        WrittenFile {
            name: format!("{stem}.obj"),
            bytes: obj.into_bytes(),
        },
        WrittenFile {
            name: format!("{stem}.mtl"),
            bytes: mtl.into_bytes(),
        },
    ];
    out.extend(images.files);
    Ok(out)
}

/// Names made unique (and usable on one line) in order.
pub(crate) fn unique_names<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut used: HashMap<String, usize> = HashMap::new();
    names
        .map(|n| {
            let base = name_of(n).replace(char::is_whitespace, "_");
            let count = used.entry(base.clone()).or_insert(0);
            *count += 1;
            if *count == 1 {
                base
            } else {
                format!("{base}.{}", *count - 1)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_groups_materials_and_polygons() {
        let obj = b"# test\nmtllib missing.mtl\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0 \nvt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\nvn 0 0 1\no box\ng front\nusemtl red\nf 1/1/1 2/2/1 3/3/1 4/4/1\nusemtl blue\nf -4/-4/-1 -2/-2/-1 -1/-1/-1\nl 1 2\n";
        let scene = read(obj, &SideFiles::new(None), "file").unwrap();
        assert_eq!(scene.parts.len(), 2);
        assert_eq!(scene.parts[0].name, "front");
        assert_eq!(scene.parts[0].object, "box");
        assert_eq!(scene.parts[0].geometry.triangles.len(), 2);
        assert_eq!(scene.parts[0].geometry.positions.len(), 4);
        // V is flipped to the top-left origin.
        assert_eq!(scene.parts[0].geometry.uv0.as_ref().unwrap()[0], [0.0, 1.0]);
        assert_eq!(scene.parts[1].geometry.triangles.len(), 1);
        assert_eq!(
            scene.materials[scene.parts[1].material.unwrap()].name,
            "blue"
        );
        assert!(scene.warnings.iter().any(|w| w.contains("missing.mtl")));
        assert!(scene.warnings.iter().any(|w| w.contains("no skeleton")));
        assert!(read(b"f 1 2 3\n", &SideFiles::new(None), "x").is_err());
    }

    #[test]
    fn texture_options_are_skipped() {
        assert_eq!(map_file("-bm 0.5 -o 1 2 3 my file.png"), "my file.png");
        assert_eq!(map_file("tex.png"), "tex.png");
        assert_eq!(map_file("-clamp on a.png"), "a.png");
    }
}
