//! Characters assembled from parts databases and animation clips, as glTF,
//! FBX or USD.

use std::sync::Arc;

use doctor_toolkit::character::{Character, MeshLocator, parse_selection};
use doctor_toolkit::export::gltf::{SceneAnimation, SceneModel};
use doctor_toolkit::game::{GameAssets, GameInstance};
use doctor_toolkit::resource::anim::{AnimBank, FRAME_RATE, TYPE_BNK, bone_set, find_skeleton};
use doctor_toolkit::resource::partsdb::PartsDb;
use doctor_toolkit::scene::{self, ModelFormat};
use pyo3::prelude::*;
use pyo3::types::PyBytes;

use crate::error::{ResultExt, invalid};
use crate::game::Game;
use crate::util;

const TYPE_PARTSDB: u32 = doctor_toolkit::hash::sdbm(b"partsdb");

fn find_database(game: &GameInstance, stream: &str, name: &str) -> PyResult<(usize, PartsDb)> {
    let s = game
        .find_stream(stream)
        .ok_or_else(|| invalid(format!("stream `{stream}` not found")))?;
    let file = game.open_stream(s).py()?;
    for (i, r) in file.layout().resources.iter().enumerate() {
        if r.header.type_id == TYPE_PARTSDB {
            let db = PartsDb::parse(&file.resource_data(i).py()?).py()?;
            if db.name.eq_ignore_ascii_case(name) {
                return Ok((s, db));
            }
        }
    }
    Err(invalid(format!("no parts database `{name}` in {stream}")))
}

fn load_character(
    game: &Arc<GameInstance>,
    index: &doctor_toolkit::game::ResourceIndex,
    stream: &str,
    database: &str,
    set: &str,
    choices: &[String],
    lod: usize,
) -> PyResult<Character> {
    let (s, db) = find_database(game, stream, database)?;
    let set = db
        .set_index(set)
        .ok_or_else(|| invalid(format!("no set `{set}` in `{database}`")))?;
    let selection = parse_selection(&db, set, choices).py()?;
    let meshes = MeshLocator::new(index);
    Character::load(game, &meshes, s, &db, set, &selection, lod).py()
}

/// A model format name (`glb`, `fbx`, `usd`, `usda`, `usdc`, `usdz`).
pub(crate) fn single_file_format(format: &str) -> PyResult<ModelFormat> {
    let f: ModelFormat = format.parse().py()?;
    if f == ModelFormat::Obj {
        return Err(invalid(
            "OBJ is several files; use Model.export(path) or the CLI `models --format obj`",
        ));
    }
    Ok(f)
}

/// A character of a parts database as one model file (binary glTF by
/// default; `format`: `glb`, `fbx`, `usdz`, or `usd`/`usda`/`usdc` without
/// textures): the parts of `set` with the chosen variants (`choices`:
/// `"slot=variant"`, `"preset=item"`, `"tag=..."`), material swaps and body
/// morphs applied.
#[pyfunction]
#[pyo3(signature = (game, stream, database, set, choices = None, lod = 0, format = "glb"))]
#[allow(clippy::too_many_arguments)]
fn character_glb<'py>(
    py: Python<'py>,
    game: PyRef<'py, Game>,
    stream: &str,
    database: &str,
    set: &str,
    choices: Option<Vec<String>>,
    lod: usize,
    format: &str,
) -> PyResult<Bound<'py, PyBytes>> {
    let game = game.game.clone();
    let choices = choices.unwrap_or_default();
    let format = single_file_format(format)?;
    let glb = py.detach(|| -> PyResult<Vec<u8>> {
        let (index, _) = game.build_index(|_| {});
        let character = load_character(&game, &index, stream, database, set, &choices, lod)?;
        let mut assets = GameAssets::new(game.clone(), &index);
        let built = character.to_scene(&mut assets).py()?;
        scene::to_bytes(&built, format).py()
    })?;
    Ok(util::bytes(py, &glb))
}

/// Clips of the animation bank `bank` (resource index of a `bnk` in
/// `stream`) as animations (binary glTF by default; `format` as for
/// [`character_glb`]), on the rig skeleton or, with `database` and `set`, on
/// that character.
#[pyfunction]
#[pyo3(signature = (game, stream, bank, clips = None, database = None, set = None, database_stream = "gf2/globalfolder/characters/allcharacters", format = "glb"))]
#[allow(clippy::too_many_arguments)]
fn animation_glb<'py>(
    py: Python<'py>,
    game: PyRef<'py, Game>,
    stream: &str,
    bank: usize,
    clips: Option<Vec<String>>,
    database: Option<&str>,
    set: Option<&str>,
    database_stream: &str,
    format: &str,
) -> PyResult<Bound<'py, PyBytes>> {
    let game = game.game.clone();
    let wanted = clips.unwrap_or_default();
    let format = single_file_format(format)?;
    let glb = py.detach(|| -> PyResult<Vec<u8>> {
        let s = game
            .find_stream(stream)
            .ok_or_else(|| invalid(format!("stream `{stream}` not found")))?;
        let file = game.open_stream(s).py()?;
        let header = file
            .layout()
            .resources
            .get(bank)
            .ok_or_else(|| invalid(format!("no resource {bank} in {stream}")))?
            .header;
        if header.type_id != TYPE_BNK {
            return Err(invalid(format!("resource {bank} is not an animation bank")));
        }
        let data = file.resource_data(bank).py()?;
        let parsed = AnimBank::parse(&data).py()?;
        let (index, _) = game.build_index(|_| {});
        let mut assets = GameAssets::new(game.clone(), &index);
        let rigs = assets.rigs().to_vec();
        let skeleton = find_skeleton(&rigs, parsed.skeleton)
            .ok_or_else(|| invalid("the bank's skeleton is in no rig"))?;
        let mut animations = Vec::new();
        for clip in &parsed.clips {
            let selected = wanted.is_empty()
                || wanted.iter().any(|w| {
                    clip.name
                        .as_deref()
                        .is_some_and(|n| n.eq_ignore_ascii_case(w))
                        || u32::from_str_radix(w.trim_start_matches("0x"), 16) == Ok(clip.hash)
                });
            let Some(bones) = bone_set(&rigs, clip.bone_set).filter(|_| selected) else {
                continue;
            };
            if let Ok(tracks) = clip.decode(&data, bones) {
                animations.push(SceneAnimation {
                    name: clip.display_name(),
                    skeleton,
                    tracks,
                    frame_rate: FRAME_RATE,
                });
            }
        }
        if animations.is_empty() {
            return Err(invalid("no clip decoded"));
        }
        let character = match (database, set) {
            (Some(db), Some(set)) => Some(load_character(
                &game,
                &index,
                database_stream,
                db,
                set,
                &[],
                0,
            )?),
            _ => None,
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
        let skeleton = models.is_empty().then_some(skeleton);
        let built = scene::game::build(&models, skeleton, &animations, &mut assets).py()?;
        scene::to_bytes(&built, format).py()
    })?;
    Ok(util::bytes(py, &glb))
}

/// The clips of an animation bank (`bnk` data) as a dict.
#[pyfunction]
fn animation_bank<'py>(py: Python<'py>, data: &[u8]) -> PyResult<Bound<'py, PyAny>> {
    let bank = AnimBank::parse(data).py()?;
    util::to_python(py, &bank)
}

/// What a model file (glTF, FBX, OBJ, USD) holds as the importer reads it:
/// parts, materials, joints, clips and warnings, in the game's space
/// (Y up, metres).
#[pyfunction]
fn read_model<'py>(py: Python<'py>, path: std::path::PathBuf) -> PyResult<Bound<'py, PyAny>> {
    let s = py.detach(|| scene::read_file(&path)).py()?;
    let summary = serde_json::json!({
        "name": s.name,
        "parts": s.parts.iter().map(|p| serde_json::json!({
            "name": p.name,
            "object": p.object,
            "material": p.material.map(|m| s.materials[m].name.clone()),
            "vertices": p.geometry.positions.len(),
            "triangles": p.geometry.triangles.len(),
            "skinned": p.geometry.skin.is_some(),
        })).collect::<Vec<_>>(),
        "materials": s.materials.iter().map(|m| serde_json::json!({
            "name": m.name,
            "base_color": m.base_color.as_ref().map(|i| i.name.clone()),
            "normal": m.normal.as_ref().map(|i| i.name.clone()),
            "specular": m.specular.as_ref().map(|i| i.name.clone()),
            "factor": m.base_color_factor,
        })).collect::<Vec<_>>(),
        "joints": s.joints.iter().map(|j| serde_json::json!({
            "name": j.name,
            "parent": j.parent,
        })).collect::<Vec<_>>(),
        "clips": s.clips.iter().map(|c| serde_json::json!({
            "name": c.name,
            "frames": c.frames(),
            "frame_rate": c.frame_rate,
            "tracks": c.tracks.len(),
        })).collect::<Vec<_>>(),
        "warnings": s.warnings,
    });
    util::to_python(py, &summary)
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(read_model, m)?)?;
    m.add_function(wrap_pyfunction!(character_glb, m)?)?;
    m.add_function(wrap_pyfunction!(animation_glb, m)?)?;
    m.add_function(wrap_pyfunction!(animation_bank, m)?)?;
    Ok(())
}
