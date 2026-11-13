//! A miniature synthetic game for tests and examples: `doctor_toolkit.testing`.
//!
//! The files are written with the library's own writers, so they parse like
//! the real game files; nothing is copied from the game.

use std::fs;
use std::path::{Path, PathBuf};

use doctor_toolkit::container::ustream::{
    FileHeader, ResourceHeader, Storage, StreamLayout, StreamWriter,
};
use doctor_toolkit::hash::sdbm;
use doctor_toolkit::io::Endian;
use doctor_toolkit::resource::lh2::StringTable;
use doctor_toolkit::resource::simgroup::{self, Guid};
use doctor_toolkit::resource::tg4::{PARAM_DATA_SIZE, PARAM_FORMAT, ParamValue, Tg4Header};
use doctor_toolkit::toc::{self, Toc};
use doctor_toolkit::workspace::ResourceKey;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::error::ResultExt;

/// Name hash of the sample texture.
const TEXTURE_ID: u32 = 0x0000_0055;
/// Id of the metamodel the sample SimGroup objects show.
const MODEL_ID: u32 = 0x1234_5678;

fn header(tag: &[u8], ids: [u32; 4]) -> ResourceHeader {
    ResourceHeader {
        mem_policy: sdbm(b"default"),
        alignment: 0x10,
        type_id: sdbm(tag),
        ids,
        data_size: 0,
    }
}

fn table(words: &[&str]) -> StringTable {
    StringTable {
        version: 1,
        flags: 0x4000,
        languages: Vec::new(),
        hashes: (1..=words.len() as u32).collect(),
        strings: vec![words.iter().map(|w| w.to_string()).collect()],
    }
}

/// A table of contents listing `streams` (name, data size) without dependencies.
fn toc_bytes(streams: &[(&str, u64)]) -> Vec<u8> {
    let count = streams.len();
    let names_start = 0x10 + count * 20;
    let pool_start = names_start + count * 8;
    let mut out = Vec::new();
    out.extend_from_slice(&toc::MAGIC.to_le_bytes());
    out.extend_from_slice(&10u32.to_le_bytes());
    for value in [count as u16, count as u16, 0, 0] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for (i, (_, size)) in streams.iter().enumerate() {
        out.extend_from_slice(&((names_start + i * 8) as u32).to_le_bytes());
        out.extend_from_slice(&(*size as u32).to_le_bytes());
        out.extend_from_slice(&[0u8; 12]);
    }
    let mut string_offset = pool_start + 1;
    for (i, (name, _)) in streams.iter().enumerate() {
        out.extend_from_slice(&(0x100 + i as u32).to_le_bytes());
        out.extend_from_slice(&(string_offset as u32).to_le_bytes());
        string_offset += name.len() + 1;
    }
    out.push(0);
    for (name, _) in streams {
        out.extend_from_slice(name.replace('/', "\\").as_bytes());
        out.push(0);
    }
    out.resize(toc::PADDING, 0);
    out
}

/// A 4×4 DXT1 texture of one colour (RGB565).
fn texture(name: &str, hash: u32, color: u16) -> (Vec<u8>, Vec<u8>) {
    let mut header = Tg4Header {
        version: 3,
        budget_hash: 0,
        package: "sample_pkg".into(),
        name_hash: hash,
        name: name.into(),
        data_size: 8,
        width: 4,
        height: 4,
        padding: 0,
        surface_count: 1,
        params: Vec::new(),
    };
    header.set_param(PARAM_FORMAT, ParamValue::Str("DXT1c".into()));
    header.set_param(PARAM_DATA_SIZE, ParamValue::Int(8));
    let mut data = Vec::new();
    data.extend_from_slice(&color.to_le_bytes());
    data.extend_from_slice(&color.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    (header.to_bytes().expect("valid header"), data)
}

/// A SimGroup with two `Entity` objects, each with a name (`CMD_SetName`),
/// a transform (`CSystemCommands.CMD_LoadMatrix`) and a model
/// (`Entity.CMD_SetModel`, a reference to resource `MODEL_ID`).
fn simgroup_bytes() -> Vec<u8> {
    const HEADER_SIZE: usize = 0x30;
    const CLASS_NAME: u32 = 0xC8F2_456F;
    let objects = [
        ("don_vito", [1.0f32, 2.0, 3.0]),
        ("barrel_01", [-4.0, 0.5, 10.0]),
    ];
    let count = objects.len();

    let mut aux = Vec::new();
    let mut guid_at = Vec::new();
    let mut matrix_at = Vec::new();
    for (i, _) in objects.iter().enumerate() {
        guid_at.push(aux.len());
        let mut guid = [0u8; 16];
        guid[0] = i as u8 + 1;
        guid[8..].copy_from_slice(b"SAMPLE01");
        aux.extend_from_slice(&guid);
    }
    let model_at = aux.len();
    aux.extend_from_slice(&Guid::from_resource_id(MODEL_ID).0);
    for (_, position) in &objects {
        matrix_at.push(aux.len());
        let mut m = [[0f32; 4]; 4];
        for (k, row) in m.iter_mut().enumerate() {
            row[k] = 1.0;
        }
        m[3][..3].copy_from_slice(position);
        for v in m.iter().flatten() {
            aux.extend_from_slice(&v.to_le_bytes());
        }
    }
    let mut strings = vec![0u8];
    let mut name_at = Vec::new();
    for (name, _) in &objects {
        name_at.push(strings.len());
        strings.extend_from_slice(name.as_bytes());
        strings.push(0);
    }
    while !strings.len().is_multiple_of(4) {
        strings.push(0);
    }

    let aux_start = HEADER_SIZE + 4 * count;
    let str_start = aux_start + aux.len();
    let mut out = vec![0u8; 8];
    for v in [
        simgroup::MAGIC,
        1,
        0xDEAD_BEEF,
        1,
        count as u32,
        0,
        aux.len() as u32,
        strings.len() as u32,
        HEADER_SIZE as u32,
        0,
    ] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&vec![0u8; 4 * count]);
    out.extend_from_slice(&aux);
    out.extend_from_slice(&strings);
    let put_rel = |out: &mut Vec<u8>, target: usize| {
        let at = out.len();
        out.extend_from_slice(&((target as i64 - at as i64) as i32).to_le_bytes());
    };
    // One packet per (attribute count, stored attribute) with a 2-byte mask.
    let packet = |out: &mut Vec<u8>, class: u32, attrs: u16, default_mask: u8, target: usize| {
        let at = out.len();
        out.extend_from_slice(&class.to_le_bytes());
        out.extend_from_slice(&attrs.to_le_bytes());
        out.push(default_mask);
        out.push(0);
        put_rel(out, target);
        at
    };
    let mut packets = Vec::new();
    for i in 0..count {
        let name = packet(&mut out, CLASS_NAME, 1, 0, str_start + name_at[i]);
        let system = packet(
            &mut out,
            simgroup::CLASS_SYSTEM_COMMANDS,
            2,
            0b01,
            aux_start + matrix_at[i],
        );
        let entity = packet(
            &mut out,
            simgroup::CLASS_ENTITY,
            2,
            0b01,
            aux_start + model_at,
        );
        packets.push([name, system, entity]);
    }
    for (i, packets) in packets.iter().enumerate() {
        let object = out.len();
        let slot = HEADER_SIZE + 4 * i;
        out[slot..slot + 4].copy_from_slice(&(object as u32).to_le_bytes());
        out.extend_from_slice(&[0u8; 0x1C]);
        out.extend_from_slice(&[3, 1, 0, packets.len() as u8]);
        out.extend_from_slice(&0u32.to_le_bytes());
        put_rel(&mut out, aux_start + guid_at[i]);
        out.extend_from_slice(&simgroup::CLASS_ENTITY.to_le_bytes());
        for &p in packets {
            put_rel(&mut out, p);
        }
    }
    out
}

fn stream(resources: &[(ResourceHeader, Vec<u8>, Storage)]) -> doctor_toolkit::Result<Vec<u8>> {
    let mut writer = StreamWriter::new(&FileHeader::GF2_PC, Endian::Little);
    for (header, data, storage) in resources {
        writer.push_resource(header, data, *storage)?;
    }
    Ok(writer.finish())
}

fn write(root: &Path, rel: &str, data: &[u8]) -> doctor_toolkit::Result<()> {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| doctor_toolkit::Error::io(parent, e))?;
    }
    fs::write(&path, data).map_err(|e| doctor_toolkit::Error::io(&path, e))
}

fn build(root: &Path) -> doctor_toolkit::Result<()> {
    let (tex_header, tex_data) = texture("wall", TEXTURE_ID, 0xF800);
    let level = stream(&[
        (
            header(b"tg4h", [0, sdbm(b"tg4h"), 7, TEXTURE_ID]),
            tex_header,
            Storage::Raw,
        ),
        (
            header(b"tg4d", [0, sdbm(b"tg4d"), 7, TEXTURE_ID]),
            tex_data,
            Storage::RefPack,
        ),
        (
            header(b"lh2", [9; 4]),
            table(&["Hello", "World", "Don Corleone"]).to_bytes()?,
            Storage::Raw,
        ),
        (
            header(b"simgroup", [0x42; 4]),
            simgroup_bytes(),
            Storage::RefPack,
        ),
        (
            header(b"vfx", [0x30; 4]),
            b"fire_big\0".to_vec(),
            Storage::Raw,
        ),
    ])?;
    let text = stream(&[(
        header(b"lh2", [2; 4]),
        table(&["Yes", "No"]).to_bytes()?,
        Storage::Raw,
    )])?;
    let size = |data: &[u8]| StreamLayout::parse(data).map(|l| l.total_data_size());
    let toc = Toc::parse(&toc_bytes(&[
        ("gf2/level", size(&level)?),
        ("gf2/text/abcd", size(&text)?),
    ]))?;
    write(root, "godfather2.exe", b"MZ sample executable")?;
    write(root, "gf2/level.str", &level)?;
    write(root, "gf2/text/abcd.str", &text)?;
    write(root, "gf2/level.toc", &toc.to_bytes()?)
}

/// Writes a miniature game into `root` (which must not hold one yet):
/// `gf2/level.str` with a 4×4 red DXT1 texture `wall`, a string table, a
/// SimGroup with two objects and an unparsed `vfx` resource;
/// `gf2/text/abcd.str` with a string table; `gf2/level.toc` listing both; a
/// fake `godfather2.exe`. Returns a dict with the keys of the resources.
#[pyfunction]
fn build_sample_game<'py>(py: Python<'py>, root: PathBuf) -> PyResult<Bound<'py, PyDict>> {
    if root.join("gf2/level.str").exists() {
        return Err(crate::error::invalid(format!(
            "{} already holds a game",
            root.display()
        )));
    }
    py.detach(|| build(&root)).py()?;
    let keys = PyDict::new(py);
    let key = |stream: &str, tag: &[u8], ids: [u32; 4]| {
        ResourceKey::new(stream, &header(tag, ids)).to_string()
    };
    let tex = |tag: &[u8]| key("gf2/level", tag, [0, sdbm(tag), 7, TEXTURE_ID]);
    keys.set_item("texture_header", tex(b"tg4h"))?;
    keys.set_item("texture_data", tex(b"tg4d"))?;
    keys.set_item("strings", key("gf2/level", b"lh2", [9; 4]))?;
    keys.set_item("simgroup", key("gf2/level", b"simgroup", [0x42; 4]))?;
    keys.set_item("vfx", key("gf2/level", b"vfx", [0x30; 4]))?;
    keys.set_item("text", key("gf2/text/abcd", b"lh2", [2; 4]))?;
    keys.set_item("model_id", MODEL_ID)?;
    Ok(keys)
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(build_sample_game, m)?)?;
    Ok(())
}
