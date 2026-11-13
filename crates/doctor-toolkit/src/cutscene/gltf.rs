//! Export of a cut-scene to binary glTF: the camera with its path, and every
//! actor of the chosen shots with its character (or skeleton) and clip, all
//! in one animation on the cut-scene's time line.
//!
//! Each actor is written by [`scene_to_glb`] and merged into one document:
//! its nodes go under an actor node that carries the actor's placement and a
//! step scale of 0 outside its shot (glTF has no visibility), and the times of
//! its clip are moved to the moment the shot plays it. The camera is a
//! perspective camera node (−Z forward, Y up, as in the game) with the
//! field of view of the first shot; the per-shot field of view is listed in
//! the node's `extras`.

use serde_json::{Value, json};

use crate::export::gltf::{AssetSource, SceneAnimation, SceneModel, scene_to_glb};
use crate::resource::anim::FRAME_RATE;
use crate::resource::trinity::{Command, euler_quaternion};
use crate::{Error, Result};

use super::cast::Cast;
use super::{Cutscene, Source, actor_state, camera_state};

/// What to export.
#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    /// Shots to export (indices into [`Cutscene::shots`]); empty for all.
    pub shots: Vec<usize>,
    /// Skeletons only, no character meshes.
    pub skeletons_only: bool,
}

/// What the export did.
#[derive(Debug, Clone, Default)]
pub struct ExportReport {
    pub actors: usize,
    pub characters: usize,
    pub animations: usize,
    /// Actors left out or written without a part, with the reason.
    pub problems: Vec<String>,
}

/// A glTF document being merged.
#[derive(Default)]
struct Doc {
    json: Value,
    bin: Vec<u8>,
    /// Channels and samplers of the one animation.
    channels: Vec<Value>,
    samplers: Vec<Value>,
}

fn array<'a>(v: &'a mut Value, key: &str) -> &'a mut Vec<Value> {
    if !v[key].is_array() {
        v[key] = json!([]);
    }
    v[key].as_array_mut().expect("array")
}

fn len(v: &Value, key: &str) -> usize {
    v[key].as_array().map_or(0, Vec::len)
}

/// Adds `by` to the index at `path` inside `v`, if it is there.
fn bump_path(v: &mut Value, path: &[&str], by: usize) {
    let mut at = v;
    for key in path {
        match at.get_mut(*key) {
            Some(next) => at = next,
            None => return,
        }
    }
    bump(at, by);
}

fn bump(v: &mut Value, by: usize) {
    if let Some(n) = v.as_u64() {
        *v = json!(n as usize + by);
    }
}

/// Splits a `.glb` into its JSON document and binary chunk.
fn unpack(glb: &[u8]) -> Result<(Value, Vec<u8>)> {
    let bad = || Error::InvalidArgument("not a binary glTF file".into());
    if glb.len() < 20 || &glb[..4] != b"glTF" {
        return Err(bad());
    }
    let u32_at = |at: usize| -> Result<usize> {
        Ok(
            u32::from_le_bytes(glb.get(at..at + 4).ok_or_else(bad)?.try_into().expect("4"))
                as usize,
        )
    };
    let json_len = u32_at(12)?;
    let json: Value = serde_json::from_slice(glb.get(20..20 + json_len).ok_or_else(bad)?)
        .map_err(|e| Error::InvalidArgument(e.to_string()))?;
    let bin_at = 20 + json_len;
    let bin = if bin_at + 8 <= glb.len() {
        let n = u32_at(bin_at)?;
        glb.get(bin_at + 8..bin_at + 8 + n)
            .ok_or_else(bad)?
            .to_vec()
    } else {
        Vec::new()
    };
    Ok((json, bin))
}

impl Doc {
    fn new() -> Self {
        Self {
            json: json!({
                "asset": { "version": "2.0", "generator": concat!("doctor_toolkit ", env!("CARGO_PKG_VERSION")) },
                "scene": 0,
                "scenes": [{ "nodes": [] }],
                "nodes": [],
            }),
            ..Default::default()
        }
    }

    fn align(&mut self) {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
    }

    /// Appends floats as a new accessor; returns its index.
    fn floats(&mut self, values: &[f32], kind: &str, with_bounds: bool) -> usize {
        self.align();
        let offset = self.bin.len();
        for v in values {
            self.bin.extend_from_slice(&v.to_le_bytes());
        }
        let components = match kind {
            "SCALAR" => 1,
            "VEC3" => 3,
            _ => 4,
        };
        let view = len(&self.json, "bufferViews");
        array(&mut self.json, "bufferViews").push(json!({
            "buffer": 0, "byteOffset": offset, "byteLength": values.len() * 4
        }));
        let mut accessor = json!({
            "bufferView": view, "componentType": 5126, "count": values.len() / components, "type": kind
        });
        if with_bounds {
            let min = values.iter().copied().fold(f32::INFINITY, f32::min);
            let max = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            accessor["min"] = json!([min]);
            accessor["max"] = json!([max]);
        }
        let index = len(&self.json, "accessors");
        array(&mut self.json, "accessors").push(accessor);
        index
    }

    fn node(&mut self, node: Value) -> usize {
        let index = len(&self.json, "nodes");
        array(&mut self.json, "nodes").push(node);
        index
    }

    fn channel(&mut self, node: usize, path: &str, times: usize, values: usize, step: bool) {
        self.samplers.push(json!({
            "input": times, "output": values,
            "interpolation": if step { "STEP" } else { "LINEAR" }
        }));
        self.channels.push(json!({
            "sampler": self.samplers.len() - 1, "target": { "node": node, "path": path }
        }));
    }

    /// Merges a document under node `parent`; the times of its animations
    /// are moved by `offset` seconds and their channels join the animation.
    fn merge(&mut self, glb: &[u8], parent: usize, offset: f32) -> Result<()> {
        let (mut other, mut bin) = unpack(glb)?;
        self.align();
        let bin_base = self.bin.len();
        let base = |key: &str| len(&self.json, key);
        let (nodes, meshes, accessors, views) = (
            base("nodes"),
            base("meshes"),
            base("accessors"),
            base("bufferViews"),
        );
        let (materials, textures, images, skins, samplers) = (
            base("materials"),
            base("textures"),
            base("images"),
            base("skins"),
            base("samplers"),
        );
        // Move the clip times before the buffers are copied.
        let mut moved = std::collections::HashSet::new();
        for animation in other["animations"].as_array().cloned().unwrap_or_default() {
            for sampler in animation["samplers"]
                .as_array()
                .cloned()
                .unwrap_or_default()
            {
                let Some(input) = sampler["input"].as_u64().map(|i| i as usize) else {
                    continue;
                };
                if !moved.insert(input) {
                    continue;
                }
                let view_index = other["accessors"][input]["bufferView"]
                    .as_u64()
                    .unwrap_or(0);
                let view_offset = other["bufferViews"][view_index as usize]["byteOffset"]
                    .as_u64()
                    .unwrap_or(0) as usize;
                let accessor = &mut other["accessors"][input];
                let start = view_offset + accessor["byteOffset"].as_u64().unwrap_or(0) as usize;
                let count = accessor["count"].as_u64().unwrap_or(0) as usize;
                for k in 0..count {
                    let at = start + 4 * k;
                    if let Some(b) = bin.get_mut(at..at + 4) {
                        let v = f32::from_le_bytes(b.try_into().expect("4")) + offset;
                        b.copy_from_slice(&v.to_le_bytes());
                    }
                }
                for key in ["min", "max"] {
                    if let Some(v) = accessor[key][0].as_f64() {
                        accessor[key] = json!([v as f32 + offset]);
                    }
                }
            }
        }
        self.bin.extend_from_slice(&bin);
        for v in other["bufferViews"].as_array_mut().into_iter().flatten() {
            v["buffer"] = json!(0);
            v["byteOffset"] = json!(v["byteOffset"].as_u64().unwrap_or(0) as usize + bin_base);
        }
        for a in other["accessors"].as_array_mut().into_iter().flatten() {
            bump_path(a, &["bufferView"], views);
        }
        for m in other["meshes"].as_array_mut().into_iter().flatten() {
            for p in m["primitives"].as_array_mut().into_iter().flatten() {
                for (_, a) in p["attributes"].as_object_mut().into_iter().flatten() {
                    bump(a, accessors);
                }
                bump_path(p, &["indices"], accessors);
                bump_path(p, &["material"], materials);
            }
        }
        for m in other["materials"].as_array_mut().into_iter().flatten() {
            for key in ["normalTexture", "occlusionTexture", "emissiveTexture"] {
                bump_path(m, &[key, "index"], textures);
            }
            for key in ["baseColorTexture", "metallicRoughnessTexture"] {
                bump_path(m, &["pbrMetallicRoughness", key, "index"], textures);
            }
        }
        for t in other["textures"].as_array_mut().into_iter().flatten() {
            bump_path(t, &["source"], images);
            bump_path(t, &["sampler"], samplers);
        }
        for i in other["images"].as_array_mut().into_iter().flatten() {
            bump_path(i, &["bufferView"], views);
        }
        for s in other["skins"].as_array_mut().into_iter().flatten() {
            bump_path(s, &["inverseBindMatrices"], accessors);
            bump_path(s, &["skeleton"], nodes);
            for j in s["joints"].as_array_mut().into_iter().flatten() {
                bump(j, nodes);
            }
        }
        for n in other["nodes"].as_array_mut().into_iter().flatten() {
            bump_path(n, &["mesh"], meshes);
            bump_path(n, &["skin"], skins);
            for c in n["children"].as_array_mut().into_iter().flatten() {
                bump(c, nodes);
            }
        }
        for animation in other["animations"].as_array().cloned().unwrap_or_default() {
            let first = self.samplers.len();
            for mut s in animation["samplers"]
                .as_array()
                .cloned()
                .unwrap_or_default()
            {
                bump_path(&mut s, &["input"], accessors);
                bump_path(&mut s, &["output"], accessors);
                self.samplers.push(s);
            }
            for mut c in animation["channels"]
                .as_array()
                .cloned()
                .unwrap_or_default()
            {
                bump_path(&mut c, &["sampler"], first);
                bump_path(&mut c, &["target", "node"], nodes);
                self.channels.push(c);
            }
        }
        let roots: Vec<usize> = other["scenes"][0]["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|n| n.as_u64())
            .map(|n| n as usize + nodes)
            .collect();
        for key in [
            "nodes",
            "meshes",
            "accessors",
            "bufferViews",
            "materials",
            "textures",
            "images",
            "skins",
            "samplers",
        ] {
            let items = other[key].as_array().cloned().unwrap_or_default();
            array(&mut self.json, key).extend(items);
        }
        array(&mut self.json["nodes"][parent], "children")
            .extend(roots.into_iter().map(|n| json!(n)));
        Ok(())
    }

    fn finish(mut self) -> Result<Vec<u8>> {
        self.align();
        self.json["buffers"] = json!([{ "byteLength": self.bin.len() }]);
        if !self.channels.is_empty() {
            self.json["animations"] = json!([{
                "name": "cutscene", "samplers": self.samplers, "channels": self.channels
            }]);
        }
        for key in [
            "materials",
            "textures",
            "images",
            "skins",
            "samplers",
            "meshes",
            "accessors",
            "bufferViews",
        ] {
            if self.json[key].as_array().is_some_and(Vec::is_empty) {
                self.json.as_object_mut().expect("object").remove(key);
            }
        }
        let mut json =
            serde_json::to_vec(&self.json).map_err(|e| Error::InvalidArgument(e.to_string()))?;
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let total = 12 + 8 + json.len() + 8 + self.bin.len();
        let mut glb = Vec::with_capacity(total);
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&(total as u32).to_le_bytes());
        glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"JSON");
        glb.extend_from_slice(&json);
        glb.extend_from_slice(&(self.bin.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"BIN\0");
        glb.extend_from_slice(&self.bin);
        Ok(glb)
    }
}

/// Sample times from `start` to `end` at the clip frame rate.
fn sample_times(start: f32, end: f32) -> Vec<f32> {
    let n = ((end - start) * FRAME_RATE).ceil().max(0.0) as usize;
    (0..=n)
        .map(|i| (start + i as f32 / FRAME_RATE).min(end))
        .collect()
}

/// Writes the cut-scene (or some of its shots) as one `.glb`.
pub fn export(
    cutscene: &Cutscene,
    source: &Source<'_>,
    cast: &mut Cast,
    assets: &mut dyn AssetSource,
    options: &ExportOptions,
) -> Result<(Vec<u8>, ExportReport)> {
    let shots: Vec<usize> = if options.shots.is_empty() {
        (0..cutscene.shots.len()).collect()
    } else {
        options.shots.clone()
    };
    if shots.iter().any(|&s| s >= cutscene.shots.len()) {
        return Err(Error::InvalidArgument(format!(
            "the cut-scene has {} shots",
            cutscene.shots.len()
        )));
    }
    // Times are moved so that the first exported shot starts at 0.
    let origin = shots
        .iter()
        .map(|&s| cutscene.shots[s].start)
        .fold(f32::INFINITY, f32::min);
    let end = shots
        .iter()
        .map(|&s| cutscene.shots[s].end())
        .fold(0.0, f32::max)
        - origin;
    let mut doc = Doc::new();
    let mut report = ExportReport::default();
    let root = doc.node(json!({ "name": cutscene.name }));
    array(&mut doc.json["scenes"][0], "nodes").push(json!(root));

    // The camera: sampled at the frame rate over the exported shots.
    let mut times = Vec::new();
    let mut translations = Vec::new();
    let mut rotations = Vec::new();
    let mut fovs = Vec::new();
    for &s in &shots {
        let shot = &cutscene.shots[s];
        let local = sample_times(0.0, shot.duration());
        for &t in &local {
            if let Some(c) = camera_state(shot, t) {
                let at = shot.start - origin + t;
                if times.last().is_some_and(|&l: &f32| at <= l) {
                    continue;
                }
                times.push(at);
                translations.extend_from_slice(&c.values[..3]);
                rotations.extend_from_slice(&euler_quaternion(
                    c.values[3],
                    c.values[4],
                    c.values[5],
                ));
                if fovs.last().is_none_or(|&(_, f): &(usize, f32)| f != c.fov) {
                    fovs.push((s, c.fov));
                }
            }
        }
    }
    let first = camera_state(&cutscene.shots[shots[0]], 0.0);
    let (fov, aspect) = first.map_or((40.0, 16.0 / 9.0), |c| (c.vertical_fov(), c.aspect));
    array(&mut doc.json, "cameras").push(json!({
        "type": "perspective",
        "perspective": { "yfov": fov.to_radians(), "aspectRatio": aspect, "znear": 0.05, "zfar": 2000.0 }
    }));
    let camera = doc.node(json!({
        "name": "camera", "camera": 0,
        "extras": { "horizontal_fov_degrees": fovs.iter().map(|(s, f)| json!({ "shot": s, "fov": f })).collect::<Vec<_>>() }
    }));
    array(&mut doc.json["nodes"][root], "children").push(json!(camera));
    if !times.is_empty() {
        let input = doc.floats(&times, "SCALAR", true);
        let t = doc.floats(&translations, "VEC3", false);
        let r = doc.floats(&rotations, "VEC4", false);
        doc.channel(camera, "translation", input, t, false);
        doc.channel(camera, "rotation", input, r, false);
    }

    for &s in &shots {
        let shot = &cutscene.shots[s];
        let block = shot.block();
        let command_times = block.times();
        for actor in &shot.actors {
            report.actors += 1;
            let label = format!("{} ({})", actor.name, shot.seq.source_name());
            // Clip, its start in the shot.
            let anim = block
                .commands
                .iter()
                .zip(&command_times)
                .find_map(|(c, &at)| match c {
                    Command::ActorAnim { actor: a, clip, .. } if *a == actor.guid => {
                        Some((clip.text.clone(), at))
                    }
                    _ => None,
                });
            let animation = match &anim {
                Some((clip, _)) => match cast.animation(source, shot, clip) {
                    Ok(a) => Some(a),
                    Err(e) => {
                        report.problems.push(format!("{label}: {e}"));
                        None
                    }
                },
                None => None,
            };
            let character = if options.skeletons_only {
                None
            } else {
                match cast.character(source.game, actor) {
                    Some(Ok(c)) => Some(c),
                    Some(Err(e)) => {
                        report.problems.push(format!("{label}: {e}"));
                        None
                    }
                    None => None,
                }
            };
            let models: Vec<SceneModel<'_>> = character
                .as_ref()
                .map(|c| {
                    c.parts
                        .iter()
                        .map(|p| SceneModel {
                            model: &p.model,
                            name: p.name.clone(),
                            materials: p.materials.clone(),
                            morphs: c.morphs.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let animations: Vec<SceneAnimation<'_>> = animation
                .iter()
                .map(|a| SceneAnimation {
                    name: a.name.clone(),
                    skeleton: &a.skeleton,
                    tracks: a.tracks.clone(),
                    frame_rate: FRAME_RATE,
                })
                .collect();
            if models.is_empty() && animations.is_empty() {
                report.problems.push(format!("{label}: nothing to show"));
                continue;
            }
            let skeleton = models
                .is_empty()
                .then(|| animation.as_ref().map(|a| &a.skeleton))
                .flatten();
            let glb = match scene_to_glb(&models, skeleton, &animations, assets) {
                Ok(g) => g,
                Err(e) => {
                    report.problems.push(format!("{label}: {e}"));
                    continue;
                }
            };
            // The actor node: placement and visibility over the shot.
            let start = shot.start - origin;
            let node = doc.node(json!({ "name": label }));
            array(&mut doc.json["nodes"][root], "children").push(json!(node));
            let local = sample_times(0.0, shot.duration());
            let mut tt = Vec::new();
            let (mut pos, mut rot) = (Vec::new(), Vec::new());
            for &t in &local {
                if let Some(state) = actor_state(shot, &actor.guid, t) {
                    tt.push(start + t);
                    pos.extend_from_slice(&state.position);
                    rot.extend_from_slice(&state.rotation);
                }
            }
            if !tt.is_empty() {
                let input = doc.floats(&tt, "SCALAR", true);
                let p = doc.floats(&pos, "VEC3", false);
                let r = doc.floats(&rot, "VEC4", false);
                doc.channel(node, "translation", input, p, false);
                doc.channel(node, "rotation", input, r, false);
            }
            let shot_end = start + shot.duration();
            let (vis_times, vis): (Vec<f32>, Vec<f32>) = if start > 0.0 {
                (
                    vec![0.0, start, shot_end],
                    vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0],
                )
            } else {
                (vec![0.0, shot_end], vec![1.0, 1.0, 1.0, 0.0, 0.0, 0.0])
            };
            let keep = if shot_end >= end {
                vis_times.len() - 1
            } else {
                vis_times.len()
            };
            let input = doc.floats(&vis_times[..keep], "SCALAR", true);
            let output = doc.floats(&vis[..keep * 3], "VEC3", false);
            doc.channel(node, "scale", input, output, true);
            let offset = start + anim.as_ref().map_or(0.0, |a| a.1);
            doc.merge(&glb, node, offset)?;
            if character.is_some() {
                report.characters += 1;
            }
            report.animations += animations.len();
        }
    }
    Ok((doc.finish()?, report))
}
