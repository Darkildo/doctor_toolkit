//! A miniature game for the tests of programs built on the library (feature
//! `testing`): a few streams with a texture, a material, a mesh that uses
//! them, a SimGroup, a parts database, a rig and string tables; on demand a
//! level with a world grid and a zone that places a prop with collision
//! ([`write_town`]).

use std::fs;
use std::path::Path;

use crate::container::ustream::{FileHeader, ResourceHeader, Storage, StreamLayout, StreamWriter};
use crate::hash::{crc32, sdbm};
use crate::io::Endian;
use crate::resource::lh2::StringTable;
use crate::resource::mesh_build::{BuildOptions, PartGeometry, build};
use crate::resource::partsdb::PartsDb;
use crate::resource::simgroup::Hash32;
use crate::resource::tg4::{PARAM_DATA_SIZE, PARAM_FORMAT, ParamValue, Tg4Header};
use crate::toc::{self, Toc};
use crate::{Error, Result};

/// Stream of the level: the resources below, in this order.
pub const LEVEL: &str = "gf2/level";
/// A text stream with one string table.
pub const TEXT: &str = "gf2/text/abcd";
pub const TEX_HEADER: usize = 0;
pub const TEX_DATA: usize = 1;
pub const MATERIAL: usize = 2;
pub const MESH: usize = 3;
pub const SIMGROUP: usize = 4;
pub const STRINGS: usize = 5;
pub const PARTSDB: usize = 6;
/// A rig with a skeleton of two bones, `m_root` and `l_wrist`.
pub const RIG: usize = 7;
/// Name hash of the texture (`wall`, 4×4 DXT1).
pub const TEXTURE_HASH: u32 = 0x55;
/// Name of the material of the mesh.
pub const MATERIAL_NAME: &str = "brick_wall";
/// Id of the mesh (`ids[3]`).
pub const MESH_ID: u32 = 0x3333;
/// GUID of the object of the SimGroup.
pub const OBJECT_GUID: &str = "01020304050607084348524953543432";

fn header(tag: &[u8], ids: [u32; 4]) -> ResourceHeader {
    ResourceHeader {
        mem_policy: sdbm(b"default"),
        alignment: 0x10,
        type_id: sdbm(tag),
        ids,
        data_size: 0,
    }
}

/// A string table with one language and two strings (hashes 1 and 2).
pub fn sample_table(words: [&str; 2]) -> StringTable {
    StringTable {
        version: 1,
        flags: 0x4000,
        languages: Vec::new(),
        hashes: vec![1, 2],
        strings: vec![words.iter().map(|w| w.to_string()).collect()],
    }
}

/// A table of contents listing `streams` (name, data size) without dependencies.
pub fn toc_bytes(streams: &[(&str, u64)]) -> Vec<u8> {
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
        out.extend_from_slice(&[0u8; 4]);
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
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

/// A material named `name` with one colour texture and one colour parameter.
pub fn material(name: &str, texture: u32) -> Vec<u8> {
    let mut d = vec![0u8; 0x60];
    d[0..4].copy_from_slice(&0x0AA0_DEEDu32.to_le_bytes());
    d[4..8].copy_from_slice(&19u32.to_le_bytes());
    let hash = crc32(name.to_ascii_lowercase().as_bytes());
    d[8..12].copy_from_slice(&hash.to_le_bytes());
    d[12..16].copy_from_slice(&hash.to_le_bytes());
    let mut strings = format!("{name}\0").into_bytes();
    let sampler = strings.len() as u16;
    strings.extend_from_slice(b"g_BaseSampler\0textures/wall.tga\0");
    let source = sampler + 14;
    let colour = strings.len() as u16;
    strings.extend_from_slice(b"g_Color\0\0");
    let empty = strings.len() as u16 - 1;
    d[0x46..0x48].copy_from_slice(&2u16.to_le_bytes());
    let (table, data_at) = (0x60usize, 0x80usize);
    let strings_at = data_at + 32;
    for (i, v) in [table, 24, data_at, 32, strings_at, strings.len()]
        .iter()
        .enumerate()
    {
        d[0x48 + 2 * i..0x4A + 2 * i].copy_from_slice(&(*v as u16).to_le_bytes());
    }
    d.resize(strings_at + strings.len(), 0);
    let entry = |hash: u32, kind: u16, data: u16, name: u16, source: u16| {
        let mut e = hash.to_le_bytes().to_vec();
        for v in [kind, data, name, source] {
            e.extend_from_slice(&v.to_le_bytes());
        }
        e
    };
    d[table..table + 12].copy_from_slice(&entry(1, 5, 0, sampler, source));
    d[table + 12..table + 24].copy_from_slice(&entry(2, 3, 16, colour, empty));
    d[data_at..data_at + 4].copy_from_slice(&texture.to_le_bytes());
    for (k, v) in [0.5f32, 0.25, 1.0, 1.0].iter().enumerate() {
        let at = data_at + 16 + 4 * k;
        d[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    d[strings_at..].copy_from_slice(&strings);
    d
}

/// A mesh of one quad with material `material`: the mesh and its volatile data.
pub fn quad_mesh(name: &str, material: &str) -> Result<(Vec<u8>, Vec<u8>)> {
    let part = PartGeometry {
        name: name.into(),
        material_hash: crc32(material.to_ascii_lowercase().as_bytes()),
        positions: vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
        ],
        uv0: Some(vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]]),
        triangles: vec![[0, 1, 2], [2, 1, 3]],
        ..Default::default()
    };
    let options = BuildOptions {
        name: name.into(),
        ..Default::default()
    };
    let (image, _) = build(&[part], &options)?;
    image.encode()
}

/// The texture header of `wall`: 4×4 DXT1, one mip level.
pub fn texture_header() -> Tg4Header {
    let mut tex = Tg4Header {
        version: 3,
        budget_hash: 0,
        package: "pkg".into(),
        name_hash: TEXTURE_HASH,
        name: "wall".into(),
        data_size: 8,
        width: 4,
        height: 4,
        padding: 0,
        surface_count: 1,
        params: Vec::new(),
    };
    tex.set_param(PARAM_FORMAT, ParamValue::Str("DXT1c".into()));
    tex.set_param(PARAM_DATA_SIZE, ParamValue::Int(8));
    tex
}

/// A parts database `db` whose `head` part shows the mesh of the level.
pub fn parts_db() -> PartsDb {
    let mut db = crate::resource::partsdb::sample_db();
    db.parts[0].models[0] = Hash32(MESH_ID);
    db
}

/// A binary glTF with one triangle moved by `offset` along X, with a
/// material named `material`. `skinned` binds it to the two bones of the
/// rig of the miniature game (`m_root`, `l_wrist`).
pub fn triangle_glb(offset: f32, material: &str, skinned: bool) -> Vec<u8> {
    use serde_json::json;
    let mut bin: Vec<u8> = Vec::new();
    let mut views = Vec::new();
    let mut push = |bin: &mut Vec<u8>, bytes: &[u8]| {
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        views.push(json!({"buffer": 0, "byteOffset": bin.len(), "byteLength": bytes.len()}));
        bin.extend_from_slice(bytes);
        views.len() - 1
    };
    let floats = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
    let positions = [offset, 0.0, 0.0, offset + 1.0, 0.0, 0.0, offset, 1.0, 0.0];
    let p = push(&mut bin, &floats(&positions));
    let i = push(&mut bin, &[0, 0, 1, 0, 2, 0]);
    let mut accessors = vec![
        json!({"bufferView": p, "componentType": 5126, "count": 3, "type": "VEC3",
               "min": [offset, 0.0, 0.0], "max": [offset + 1.0, 1.0, 0.0]}),
        json!({"bufferView": i, "componentType": 5123, "count": 3, "type": "SCALAR"}),
    ];
    let mut attributes = json!({"POSITION": 0});
    let mut nodes = vec![json!({"name": "triangle", "mesh": 0})];
    let mut doc = json!({});
    if skinned {
        let j = push(&mut bin, &[0, 1, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0]);
        let w = push(
            &mut bin,
            &floats(&[0.5, 0.5, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]),
        );
        let identity = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0f32,
        ];
        let mut ibm = identity.to_vec();
        ibm.extend_from_slice(&identity);
        let m = push(&mut bin, &floats(&ibm));
        accessors.push(json!({"bufferView": j, "componentType": 5121, "count": 3, "type": "VEC4"}));
        accessors.push(json!({"bufferView": w, "componentType": 5126, "count": 3, "type": "VEC4"}));
        accessors.push(json!({"bufferView": m, "componentType": 5126, "count": 2, "type": "MAT4"}));
        attributes = json!({"POSITION": 0, "JOINTS_0": 2, "WEIGHTS_0": 3});
        nodes[0]["skin"] = json!(0);
        nodes.push(json!({"name": "m_root", "children": [2]}));
        nodes.push(json!({"name": "l_wrist", "translation": [0.0, 1.0, 0.0]}));
        doc["skins"] = json!([{"joints": [1, 2], "inverseBindMatrices": 4}]);
    }
    let roots: Vec<usize> = if skinned { vec![0, 1] } else { vec![0] };
    doc["asset"] = json!({"version": "2.0"});
    doc["scene"] = json!(0);
    doc["scenes"] = json!([{"nodes": roots}]);
    doc["nodes"] = json!(nodes);
    doc["materials"] = json!([{"name": material}]);
    doc["meshes"] = json!([{"name": "triangle", "primitives": [
        {"attributes": attributes, "indices": 1, "material": 0}
    ]}]);
    doc["accessors"] = json!(accessors);
    doc["bufferViews"] = json!(views);
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    doc["buffers"] = json!([{"byteLength": bin.len()}]);
    let mut text = serde_json::to_vec(&doc).unwrap_or_default();
    while !text.len().is_multiple_of(4) {
        text.push(b' ');
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&((12 + 8 + text.len() + 8 + bin.len()) as u32).to_le_bytes());
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&text);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

/// A binary glTF with a box `size` (metres; standing on y = 0, centred on X
/// and Z) of eight corners and twelve triangles, with a material named
/// `material`: a wall or a crate to import with collision.
pub fn box_glb(size: [f32; 3], material: &str) -> Vec<u8> {
    use serde_json::json;
    let [x, y, z] = [size[0] / 2.0, size[1], size[2] / 2.0];
    let corners: [[f32; 3]; 8] = [
        [-x, 0.0, -z],
        [x, 0.0, -z],
        [x, 0.0, z],
        [-x, 0.0, z],
        [-x, y, -z],
        [x, y, -z],
        [x, y, z],
        [-x, y, z],
    ];
    let triangles: [[u16; 3]; 12] = [
        [0, 1, 2],
        [0, 2, 3],
        [4, 6, 5],
        [4, 7, 6],
        [0, 4, 5],
        [0, 5, 1],
        [1, 5, 6],
        [1, 6, 2],
        [2, 6, 7],
        [2, 7, 3],
        [3, 7, 4],
        [3, 4, 0],
    ];
    let mut bin: Vec<u8> = corners
        .iter()
        .flatten()
        .flat_map(|f| f.to_le_bytes())
        .collect();
    let positions = bin.len();
    bin.extend(triangles.iter().flatten().flat_map(|i| i.to_le_bytes()));
    let indices = bin.len() - positions;
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let doc = json!({
        "asset": {"version": "2.0"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"name": "box", "mesh": 0}],
        "materials": [{"name": material}],
        "meshes": [{"name": "box", "primitives": [
            {"attributes": {"POSITION": 0}, "indices": 1, "material": 0}
        ]}],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 8, "type": "VEC3",
             "min": [-x, 0.0, -z], "max": [x, y, z]},
            {"bufferView": 1, "componentType": 5123, "count": 36, "type": "SCALAR"},
        ],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": positions},
            {"buffer": 0, "byteOffset": positions, "byteLength": indices},
        ],
        "buffers": [{"byteLength": bin.len()}],
    });
    let mut text = serde_json::to_vec(&doc).unwrap_or_default();
    while !text.len().is_multiple_of(4) {
        text.push(b' ');
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&((12 + 8 + text.len() + 8 + bin.len()) as u32).to_le_bytes());
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&text);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

/// Writes the miniature game into `root` (which must exist): `godfather2.exe`,
/// [`LEVEL`] with the resources of the constants above, [`TEXT`] and a table
/// of contents for both.
pub fn write_game(root: &Path) -> Result<()> {
    let io = |path: &Path, e| Error::io(path, e);
    let mut level = StreamWriter::new(&FileHeader::GF2_PC, Endian::Little);
    let tg4h = texture_header().to_bytes()?;
    level.push_resource(
        &header(b"tg4h", [0, sdbm(b"tg4h"), 7, TEXTURE_HASH]),
        &tg4h,
        Storage::Raw,
    )?;
    level.push_resource(
        &header(b"tg4d", [0, sdbm(b"tg4d"), 7, TEXTURE_HASH]),
        &[0u8; 8],
        Storage::RefPack,
    )?;
    let hash = crc32(MATERIAL_NAME.as_bytes());
    level.push_resource(
        &header(b"material", [hash; 4]),
        &material(MATERIAL_NAME, TEXTURE_HASH),
        Storage::Raw,
    )?;
    let (mesh, volatile) = quad_mesh("wall_quad", MATERIAL_NAME)?;
    level.push_resource(&header(b"mesh", [MESH_ID; 4]), &mesh, Storage::RefPack)?;
    level.push_resource(
        &header(b"simgroup", [4; 4]),
        &crate::resource::simgroup::sample_group(),
        Storage::RefPack,
    )?;
    level.push_resource(
        &header(b"lh2", [9; 4]),
        &sample_table(["Hello", "World"]).to_bytes()?,
        Storage::Raw,
    )?;
    level.push_resource(
        &header(b"partsdb", [6; 4]),
        &parts_db().encode()?,
        Storage::Raw,
    )?;
    level.push_resource(
        &header(b"rcb", [8; 4]),
        &crate::resource::rcb::sample_rig(),
        Storage::Raw,
    )?;
    if !volatile.is_empty() {
        level.push_resource(
            &header(b"meshvolatile", [!MESH_ID; 4]),
            &volatile,
            Storage::Raw,
        )?;
    }
    let level = level.finish();

    let mut text = StreamWriter::new(&FileHeader::GF2_PC, Endian::Little);
    text.push_resource(
        &header(b"lh2", [2; 4]),
        &sample_table(["Yes", "No"]).to_bytes()?,
        Storage::Raw,
    )?;
    let text = text.finish();

    let size = |data: &[u8]| StreamLayout::parse(data).map(|l| l.total_data_size());
    fs::create_dir_all(root.join("gf2/text")).map_err(|e| io(root, e))?;
    let write = |rel: &str, data: &[u8]| {
        let path = root.join(rel);
        fs::write(&path, data).map_err(|e| io(&path, e))
    };
    write("godfather2.exe", b"MZ fake")?;
    write("gf2/level.str", &level)?;
    write("gf2/text/abcd.str", &text)?;
    let table = Toc::parse(&toc_bytes(&[(LEVEL, size(&level)?), (TEXT, size(&text)?)]))?;
    write("gf2/level.toc", &table.to_bytes()?)?;
    Ok(())
}

/// Gives the miniature game of [`write_game`] its list of languages: an
/// `lhr` resource in [`TEXT`] that names that stream as the table of `en`,
/// so that [`crate::text::language_tables`] finds one table. (Call it
/// before [`write_extra_streams`], which lists the streams again.)
pub fn write_languages(root: &Path) -> Result<()> {
    let mut text = StreamWriter::new(&FileHeader::GF2_PC, Endian::Little);
    text.push_resource(
        &header(b"lh2", [2; 4]),
        &sample_table(["Yes", "No"]).to_bytes()?,
        Storage::Raw,
    )?;
    let languages = format!("1\r\nen\r\n{}\r\n0\r\n", TEXT.replace('/', "\\"));
    text.push_resource(&header(b"lhr", [3; 4]), languages.as_bytes(), Storage::Raw)?;
    let text = text.finish();
    let path = root.join(format!("{TEXT}.str"));
    fs::write(&path, &text).map_err(|e| Error::io(&path, e))?;
    let size = |rel: &str| -> Result<u64> {
        let path = root.join(format!("{rel}.str"));
        let data = fs::read(&path).map_err(|e| Error::io(&path, e))?;
        Ok(StreamLayout::parse(&data)?.total_data_size())
    };
    let table = Toc::parse(&toc_bytes(&[(LEVEL, size(LEVEL)?), (TEXT, size(TEXT)?)]))?;
    let path = root.join("gf2/level.toc");
    fs::write(&path, table.to_bytes()?).map_err(|e| Error::io(&path, e))?;
    Ok(())
}

/// Stream with a navigation graph, added by [`write_extra_streams`].
pub const NAV: &str = "gf2/nav";

/// The stream that holds the game's `LuaAudioScript` objects (the template
/// of scripts made from graphs), added by [`write_extra_streams`].
pub const GLOBALS: &str = "gf2/globals_global";

/// GUID of the navigation graph of [`NAV`].
pub const GRAPH_GUID: [u32; 4] = [0x6e61_7601, 0x6772_6170, 0x6831_3233, 0x3435_3637];

/// Adds two streams to the miniature game of [`write_game`]: [`NAV`] with
/// one navigation graph of three nodes in a row along X (0, 10 and 20 m),
/// linked both ways, and [`GLOBALS`] with a SimGroup of one
/// `LuaAudioScript` object; all four streams go into the table of contents.
pub fn write_extra_streams(root: &Path) -> Result<()> {
    use crate::resource::graph::{Graph, graph_flags};
    use crate::resource::simgroup::Guid;
    let mut bytes = [0u8; 16];
    for (i, part) in GRAPH_GUID.iter().enumerate() {
        bytes[i * 4..i * 4 + 4].copy_from_slice(&part.to_le_bytes());
    }
    let mut graph = Graph::new(Guid(bytes), graph_flags::GROUND_PATHING);
    for x in [0.0, 10.0, 20.0] {
        graph.add_node([x, 0.0, 0.0], 0.5)?;
    }
    for (a, b) in [(0, 1), (1, 0), (1, 2), (2, 1)] {
        graph.add_link(a, b, 0)?;
    }
    graph.rebuild_rtree();
    let mut nav = StreamWriter::new(&FileHeader::GF2_PC, Endian::Little);
    nav.push_resource(
        &header(b"graph", GRAPH_GUID),
        &graph.to_bytes()?,
        Storage::Raw,
    )?;
    let nav = nav.finish();
    let path = root.join(format!("{NAV}.str"));
    fs::write(&path, &nav).map_err(|e| Error::io(&path, e))?;
    let mut scripts = crate::resource::simgroup::SimGroup::empty();
    scripts.push_object(
        crate::script::deploy::CLASS_LUA_AUDIO_SCRIPT,
        Guid(*b"LUASCRIPTTEMPL01"),
        &[(crate::script::deploy::CLASS_LUA_AUDIO_SCRIPT, 4)],
    )?;
    let mut globals = StreamWriter::new(&FileHeader::GF2_PC, Endian::Little);
    globals.push_resource(
        &header(b"simgroup", [0x61; 4]),
        &scripts.encode(),
        Storage::RefPack,
    )?;
    let globals = globals.finish();
    let path = root.join(format!("{GLOBALS}.str"));
    fs::write(&path, &globals).map_err(|e| Error::io(&path, e))?;
    let size = |rel: &str| -> Result<u64> {
        let path = root.join(format!("{rel}.str"));
        let data = fs::read(&path).map_err(|e| Error::io(&path, e))?;
        Ok(StreamLayout::parse(&data)?.total_data_size())
    };
    let table = Toc::parse(&toc_bytes(&[
        (LEVEL, size(LEVEL)?),
        (TEXT, size(TEXT)?),
        (NAV, size(NAV)?),
        (GLOBALS, size(GLOBALS)?),
    ]))?;
    let path = root.join("gf2/level.toc");
    fs::write(&path, table.to_bytes()?).map_err(|e| Error::io(&path, e))?;
    Ok(())
}

/// The folder stream of the cut-scene added by [`write_cutscene`].
pub const CUTSCENE: &str = "gf2/igc99/igc99_folderstream";

/// GUID of the actor of [`sample_shot`].
pub const CUTSCENE_ACTOR: [u8; 16] = *b"\x01\x02\x03\x04\x05\x06\x07\x08DOCTORTK";

/// A shot of two seconds: an actor created at x = 1 and walking from x = 0
/// to x = 4 between 1 and 2 s, a camera moving from x = 0 to x = 10 between
/// 0 and 2 s with a focal length of 50 mm.
pub fn sample_shot() -> crate::resource::trinity::SeqMaster {
    use crate::resource::trinity::{
        Command, CurveKey, FixedText, Guid, PositionKey, RotationKey, SeqBlock, SeqMaster,
    };
    let actor = Guid(CUTSCENE_ACTOR);
    let identity = [0.0, 0.0, 0.0, 1.0];
    SeqMaster {
        path: FixedText::new("x:\\assets\\igcs\\igc99\\source\\igc99_s01.xml"),
        blocks: vec![SeqBlock {
            version: 4,
            hash: 0,
            transform: [-0.0, -0.0, -0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            name: FixedText::new("trSeq_igc99_s01"),
            duration: 2.0,
            commands: vec![
                Command::ActorCreate {
                    actor,
                    name: FixedText::new("igc99_hero"),
                    flags: [0; 3],
                    position: [1.0, 0.0, 0.0],
                    reserved: 0,
                },
                Command::CameraCreate {
                    camera: 0,
                    flags: 0,
                    aspect: 16.0 / 9.0,
                },
                Command::CameraCurves {
                    camera: 0,
                    curves: std::array::from_fn(|i| match i {
                        0 => vec![CurveKey::flat(0.0, 0.0), CurveKey::flat(2.0, 10.0)],
                        1 => vec![CurveKey::flat(0.0, 1.7)],
                        2 => vec![CurveKey::flat(0.0, 8.0)],
                        _ => vec![CurveKey::flat(0.0, 0.0)],
                    }),
                },
                Command::CameraFov {
                    camera: 0,
                    keys: vec![CurveKey::flat(0.0, 50.0)],
                },
                Command::Wait { seconds: 1.0 },
                Command::ActorTransform {
                    actor,
                    positions: vec![
                        PositionKey {
                            time: 0.0,
                            position: [0.0, 0.0, 0.0],
                        },
                        PositionKey {
                            time: 1.0,
                            position: [4.0, 0.0, 0.0],
                        },
                    ],
                    rotations: vec![
                        RotationKey {
                            time: 0.0,
                            rotation: identity,
                        },
                        RotationKey {
                            time: 1.0,
                            rotation: identity,
                        },
                    ],
                },
                Command::Wait { seconds: 1.0 },
            ],
        }],
    }
}

/// Adds the folder stream [`CUTSCENE`] to the miniature game of
/// [`write_game`]: the shot of [`sample_shot`] and the SimGroup of the
/// level stream (so that a level of it loads); the table of contents lists
/// every stream of the game written so far.
pub fn write_cutscene(root: &Path) -> Result<()> {
    use crate::resource::trinity::{SeqMaster, TYPE_TRINITY_SEQ_MASTER};
    let shot = sample_shot();
    let id = SeqMaster::id_for_path(&shot.path.text);
    let mut stream = StreamWriter::new(&FileHeader::GF2_PC, Endian::Little);
    let mut h = header(b"trinity_seq_master", [id; 4]);
    h.type_id = TYPE_TRINITY_SEQ_MASTER;
    stream.push_resource(&h, &shot.to_bytes()?, Storage::Raw)?;
    stream.push_resource(
        &header(b"simgroup", [0x99; 4]),
        &crate::resource::simgroup::sample_group(),
        Storage::RefPack,
    )?;
    let data = stream.finish();
    let path = root.join(format!("{CUTSCENE}.str"));
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    fs::write(&path, &data).map_err(|e| Error::io(&path, e))?;
    let mut streams = Vec::new();
    for rel in [LEVEL, TEXT, NAV, GLOBALS, CUTSCENE, SOUND] {
        let path = root.join(format!("{rel}.str"));
        if let Ok(data) = fs::read(&path) {
            streams.push((rel, StreamLayout::parse(&data)?.total_data_size()));
        }
    }
    let table = Toc::parse(&toc_bytes(&streams))?;
    let path = root.join("gf2/level.toc");
    fs::write(&path, table.to_bytes()?).map_err(|e| Error::io(&path, e))?;
    Ok(())
}

// [`SOUND`] is also the stream [`write_sound`] writes (in a game folder of
// its own): a speech bank (`smb`), the speech event table (`get`), the
// audio configuration (`auc`) and a mixer snapshot (`mix`), in this order.
/// The speech file of the archive of [`write_sound`]: half a second at
/// 32 kHz with subtitles (`EN`: "Hello there.").
pub const SPEECH_ENTRY: &str = "audiostreams\\d_one\\d_one.exa.snu";
/// The streamed sound without subtitles of the archive of [`write_sound`].
pub const MUSIC_ENTRY: &str = "audiostreams\\music\\theme.exa.snu";
/// The speech event with a record in the table of [`write_sound`].
pub const SPEECH_EVENT: &str = "F2F_HELLO";
/// The mixer snapshot of [`write_sound`]: one channel, `DIALOG` at 0.5.
pub const MIX_NAME: &str = "occlusion.mix";

/// A tone of `seconds` at `rate`, mono.
pub fn tone(seconds: f64, rate: u32) -> crate::resource::video::input::Pcm {
    let n = (seconds * f64::from(rate)) as usize;
    crate::resource::video::input::Pcm {
        channels: 1,
        sample_rate: rate,
        samples: (0..n)
            .map(|i| ((i as f64 * 0.3).sin() * 12_000.0) as i16)
            .collect(),
    }
}

/// Adds sound to the miniature game of [`write_game`]: the archive
/// `GODFATH0.viv` with [`SPEECH_ENTRY`] and [`MUSIC_ENTRY`], and the stream
/// [`SOUND`] with a speech bank of one entry (the speech file), an event
/// table with [`SPEECH_EVENT`], an audio configuration with the categories
/// `DIALOG` and `CHATTER` and the mix [`MIX_NAME`]; the table of contents
/// lists every stream of the game written so far.
pub fn write_sound(root: &Path) -> Result<()> {
    use crate::resource::auc::{AudioConfig, BankPool, Category, Duck};
    use crate::resource::audio_encode::{SampleSpec, build_snu};
    use crate::resource::get::{SpeechEvent, SpeechEventTable};
    use crate::resource::mix::{Mix, MixChannel};
    use crate::resource::smb::{self, SpeechBank, SpeechBankEntry};
    use crate::resource::speech;

    let spec = |rate| SampleSpec {
        channels: 1,
        sample_rate: rate,
        kind: 1,
        loop_start: None,
    };
    let speech_file = build_snu(&tone(0.5, 32_000), &spec(32_000), None)?;
    let table = speech::new_table("d_one", &[("EN".to_owned(), "Hello there.".to_owned())])?;
    let speech_file = speech::attach_subtitles(&speech_file, &table)?;
    let music = build_snu(&tone(1.0, 44_100), &spec(44_100), None)?;
    let archive = crate::container::big::build_archive(&[
        (SPEECH_ENTRY.to_owned(), speech_file),
        (MUSIC_ENTRY.to_owned(), music),
    ])?;
    let path = root.join("GODFATH0.viv");
    fs::write(&path, archive).map_err(|e| Error::io(&path, e))?;

    let bank = SpeechBank {
        version: smb::VERSION,
        entries: vec![SpeechBankEntry {
            tag: smb::ENTRY_TAG,
            flags: 0x49,
            id: [0x0900_0001; 4],
            reserved: 0,
            settings: [3.0, 45.0, 0.0, 0.0, 0.056, 1.219],
            level: 80,
            category: crate::hash::string_id("CHATTER"),
            category2: crate::hash::string_id("DIALOG"),
            duck: 0,
            file: "d_one.exa".into(),
            extra: 0,
            name_hash: smb::name_hash("d_one.exa"),
        }],
    };
    let mut events = SpeechEventTable {
        version: 3,
        events: Vec::new(),
    };
    events.insert(SpeechEvent::new(SPEECH_EVENT));
    let category = |name: &str, parent: &str| Category {
        name: name.into(),
        second: String::new(),
        parent: parent.into(),
        value: 0,
    };
    let config = AudioConfig {
        version: crate::resource::auc::VERSION,
        categories: vec![category("DIALOG", "DIALOG"), category("CHATTER", "DIALOG")],
        pools: vec![BankPool {
            name: "audio_bank_00".into(),
            size: 1536,
            kind: 1,
        }],
        ducks: vec![Duck {
            levels: [95, 50],
            filler: *b"rsion='1",
            name: "WEAPON_DUCK".into(),
            mix: "combine_weapon_duck.mix".into(),
        }],
        chains: 0,
        tail: Vec::new(),
    };
    let mix = Mix {
        stamp: 0x7C90_EE18,
        name: MIX_NAME.into(),
        version: crate::resource::mix::VERSION,
        flags: 0,
        channels: vec![MixChannel {
            category: crate::hash::string_id("DIALOG"),
            levels: [-1.0, -1.0, 1.0, 0.5, 0.0, -1.0, 0.25, 0.25],
            frequency: 6000,
        }],
    };
    let mut stream = StreamWriter::new(&FileHeader::GF2_PC, Endian::Little);
    for (tag, id, data) in [
        (&b"smb"[..], 0x51, bank.to_bytes()?),
        (b"get", 0x52, events.to_bytes()?),
        (b"auc", 0x53, config.to_bytes()?),
        (b"mix", 0x54, mix.to_bytes()?),
    ] {
        stream.push_resource(&header(tag, [id; 4]), &data, Storage::Raw)?;
    }
    let path = root.join(format!("{SOUND}.str"));
    fs::write(&path, stream.finish()).map_err(|e| Error::io(&path, e))?;
    let mut streams = Vec::new();
    for rel in [LEVEL, TEXT, NAV, GLOBALS, CUTSCENE, SOUND] {
        let path = root.join(format!("{rel}.str"));
        if let Ok(data) = fs::read(&path) {
            streams.push((rel, StreamLayout::parse(&data)?.total_data_size()));
        }
    }
    let table = Toc::parse(&toc_bytes(&streams))?;
    let path = root.join("gf2/level.toc");
    fs::write(&path, table.to_bytes()?).map_err(|e| Error::io(&path, e))?;
    Ok(())
}

/// Rewrites the table of contents to list every stream of the miniature
/// game written so far.
fn rewrite_toc(root: &Path) -> Result<()> {
    let mut streams = Vec::new();
    for rel in [LEVEL, TEXT, NAV, GLOBALS, CUTSCENE, SOUND] {
        let path = root.join(format!("{rel}.str"));
        if let Ok(data) = fs::read(&path) {
            streams.push((rel, StreamLayout::parse(&data)?.total_data_size()));
        }
    }
    let table = Toc::parse(&toc_bytes(&streams))?;
    let path = root.join("gf2/level.toc");
    fs::write(&path, table.to_bytes()?).map_err(|e| Error::io(&path, e))
}

/// Adds to [`GLOBALS`] (after [`write_extra_streams`]) the SimGroup the
/// curated game parameters name (`gf2/globals_global:simgroup:00000000`)
/// with the four pistols of `weapons.pistol.damage` as `Gun` objects:
/// damage 25, 26, 27 and 28 (attribute 6), clip size 8 (attribute 7).
pub fn write_tunable_globals(root: &Path) -> Result<()> {
    use crate::resource::simgroup::{Guid, SimGroup, Value};
    let path = root.join(format!("{GLOBALS}.str"));
    let old = fs::read(&path).map_err(|e| Error::io(&path, e))?;
    let layout = StreamLayout::parse(&old)?;
    let mut stream = StreamWriter::new(&FileHeader::GF2_PC, Endian::Little);
    for (index, resource) in layout.resources.iter().enumerate() {
        stream.push_resource(
            &resource.header,
            &layout.resource_data(&old, index)?,
            Storage::RefPack,
        )?;
    }
    let pistols = crate::tunables::by_id("weapons.pistol.damage")?;
    let gun = pistols.class;
    let mut group = SimGroup::empty();
    for (n, text) in pistols.objects.iter().enumerate() {
        let mut guid = [0u8; 16];
        for (i, byte) in guid.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16)
                .map_err(|_| Error::InvalidArgument(format!("bad GUID {text}")))?;
        }
        let o = group.push_object(gun, Guid(guid), &[(gun, 100)])?;
        group.set_class_attr(o, gun, 6, Some(Value::Float(25.0 + n as f32)))?;
        group.set_class_attr(o, gun, 7, Some(Value::Int(8)))?;
    }
    stream.push_resource(
        &header(b"simgroup", [0; 4]),
        &group.encode(),
        Storage::RefPack,
    )?;
    fs::write(&path, stream.finish()).map_err(|e| Error::io(&path, e))?;
    rewrite_toc(root)
}

/// The stream of sound, animation and interface data added by
/// [`write_sound_stream`]; its resources are in the order of the constants
/// below.
pub const SOUND: &str = "gf2/sound";
/// The chatter set of the one speaker: event `F2F_HELLO` with one line.
pub const SOUND_CHV: usize = 0;
/// The speech bank with the entry of that line (`d_one.exa`).
pub const SOUND_SMB: usize = 1;
/// The speech event table with the record of `F2F_HELLO`.
pub const SOUND_GET: usize = 2;
/// The audio configuration: categories DIALOG and CHATTER.
pub const SOUND_AUC: usize = 3;
/// The mixer snapshot `occlusion.mix` with channels CHATTER and DIALOG.
pub const SOUND_MIX: usize = 4;
/// A sound bank of two emitters, `a.emx` (two variations) and `b.emx`.
pub const SOUND_SBK: usize = 5;
/// An animation bank of two clips (hashes 0x10 and 0x20; the first has a
/// `footstep` event) and the entry `idle`: a sequence of a random choice of
/// the two.
pub const SOUND_BNK: usize = 6;
/// An interface file `hud` with a movie that names `$menu_start`, and constants.
pub const SOUND_APX: usize = 7;

/// Adds the stream [`SOUND`] to the miniature game (after [`write_game`]).
pub fn write_sound_stream(root: &Path) -> Result<()> {
    use crate::hash::string_id;
    use crate::resource::apx::{AptFile, Block};
    use crate::resource::chv::ChatterSet;
    use crate::resource::get::{SpeechEvent, SpeechEventTable};
    use crate::resource::mix::{Mix, MixChannel};
    use crate::resource::smb::{ENTRY_TAG, SpeechBank, SpeechBankEntry, VERSION, name_hash};
    let sound = 0x0900_0001;
    let mut set = ChatterSet {
        version: 3,
        events: Vec::new(),
    };
    set.add_line("F2F_HELLO", sound, string_id("one"), 1.0)?;
    let bank = SpeechBank {
        version: VERSION,
        entries: vec![SpeechBankEntry {
            tag: ENTRY_TAG,
            flags: 0x49,
            id: [sound; 4],
            reserved: 0,
            settings: [3.0, 45.0, 0.0, 0.0, 0.056, 1.219],
            level: 80,
            category: string_id("CHATTER"),
            category2: 0x3504_8678,
            duck: 0,
            file: "d_one.exa".into(),
            extra: 0,
            name_hash: name_hash("d_one.exa"),
        }],
    };
    let mut events = SpeechEventTable {
        version: 3,
        events: Vec::new(),
    };
    events.insert(SpeechEvent::new("F2F_HELLO"));
    let channel = |name: &str, volume: f32| MixChannel {
        category: string_id(name),
        levels: [-1.0, -1.0, 1.0, volume, 0.0, -1.0, 2.0, 2.0],
        frequency: 6000,
    };
    let mix = Mix {
        stamp: 0x7C90_EE18,
        name: "occlusion.mix".into(),
        version: crate::resource::mix::VERSION,
        flags: 0,
        channels: vec![channel("CHATTER", 0.6), channel("DIALOG", 0.6)],
    };
    let block = |tag: &[u8; 4], data: &[u8]| Block {
        tag: *tag,
        data: data.to_vec(),
    };
    let mut apx = AptFile {
        blocks: vec![
            block(b"titl", b"hud\0"),
            block(b"apti", b"Apt Data:\x1a\0$menu_start\0\0\0\0\0"),
            block(b"cons", b"Apt constant file\x1a\0"),
        ],
    };
    apx.align_blocks();
    let mut animations = crate::resource::anim_graph::sample_bank();
    animations.set_events(
        0,
        &[crate::resource::anim::ClipEvent {
            frame: 1.0,
            name: "footstep".into(),
            signature: 0x0300_0000,
            args: vec![
                crate::resource::anim::EventArg::Text("step.emx".into()),
                crate::resource::anim::EventArg::Int(3),
            ],
        }],
    )?;
    let mut stream = StreamWriter::new(&FileHeader::GF2_PC, Endian::Little);
    for (tag, id, data) in [
        (&b"chv"[..], 0x18C7_062Eu32, set.to_bytes()?),
        (b"smb", 0x51, bank.to_bytes()?),
        (b"get", 0x52, events.to_bytes()?),
        (b"auc", 0x53, crate::resource::auc::sample().to_bytes()?),
        (b"mix", 0x54, mix.to_bytes()?),
        (
            b"sbk",
            0x55,
            crate::resource::sbk::samples::bank().to_bytes(),
        ),
        (b"bnk", 0x56, animations.encode()?),
        (b"apx", 0x57, apx.to_bytes()?),
    ] {
        stream.push_resource(&header(tag, [id; 4]), &data, Storage::Raw)?;
    }
    let path = root.join(format!("{SOUND}.str"));
    fs::write(&path, stream.finish()).map_err(|e| Error::io(&path, e))?;
    rewrite_toc(root)
}

/// Root stream of the level added by [`write_town`]: its world grid (`wgi`,
/// resource 0) has one cell, of [`YARD`].
pub const TOWN: &str = "gf2/town";
/// A zone of [`TOWN`], added by [`write_town`]: a crate (resources
/// [`YARD_MESH`], [`YARD_HKX`], [`YARD_MODEL`]) placed by the one object of
/// its SimGroup ([`YARD_SIMGROUP`]).
pub const YARD: &str = "gf2/town/yard";
pub const YARD_MESH: usize = 0;
pub const YARD_HKX: usize = 1;
pub const YARD_MODEL: usize = 2;
pub const YARD_SIMGROUP: usize = 3;
/// Id of the metamodel of the crate.
pub const CRATE_MODEL_ID: u32 = 0x7001_0001;
/// Id of the mesh of the crate.
pub const CRATE_MESH_ID: u32 = 0x7001_0002;
/// Where the crate stands.
pub const CRATE_POSITION: [f32; 3] = [4.0, 0.0, 6.0];

/// A metamodel with id `id` that shows mesh `mesh` and, with `hkx`, has
/// collision in the `hkx` resource of that id: two resource references, one
/// state and one `ModelPart` whose `Model` property is the mesh.
pub fn prop_metamodel(id: u32, mesh: u32, hkx: Option<u32>) -> Vec<u8> {
    const SIZE: usize = 0x240;
    const T0: u32 = 0x200;
    let mut d = vec![0u8; SIZE];
    let put = |d: &mut Vec<u8>, at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
    let put16 =
        |d: &mut Vec<u8>, at: usize, v: u16| d[at..at + 2].copy_from_slice(&v.to_le_bytes());
    put(&mut d, 0, crate::resource::metamodel::MAGIC);
    put(&mut d, 4, 14);
    put(&mut d, 8, SIZE as u32);
    for k in 0..4 {
        put(&mut d, 0x10 + 4 * k, id);
    }
    // Tables: resources, states, variables, properties, conditions, parts.
    let resources = if hkx.is_some() { 2 } else { 1 };
    for (i, (at, n)) in [
        (T0, resources),
        (0x88, 1),
        (0xB8, 1),
        (0xC4, 2),
        (0xF4, 1),
        (0x100, 1),
    ]
    .into_iter()
    .enumerate()
    {
        put(&mut d, 0x34 + 4 * i, at);
        put16(&mut d, 0x58 + 2 * i, n);
    }
    let strings = b"\0State\0S0\0health\0Model\0ModelPart\0CMD_LoadMatrix\0CSystemCommands\0P0\0";
    d[0x150..0x150 + strings.len()].copy_from_slice(strings);
    let s = |name: &str| {
        let pos = strings
            .windows(name.len() + 2)
            .position(|w| {
                w[0] == 0 && &w[1..=name.len()] == name.as_bytes() && w[name.len() + 1] == 0
            })
            .expect("a string of the table");
        0x151 + pos as u32
    };
    // Resource references: the GUID is the id four times, as in the game.
    let reference = |d: &mut Vec<u8>, at: usize, type_id: u32, id: u32| {
        put(d, at, 0x1234);
        put(d, at + 4, id);
        put(d, at + 8, type_id);
        for k in 0..4 {
            put(d, at + 0xC + 4 * k, id);
        }
    };
    reference(&mut d, T0 as usize, sdbm(b"mesh"), mesh);
    if let Some(hkx) = hkx {
        reference(
            &mut d,
            T0 as usize + 0x20,
            crate::resource::hkx::TYPE_HKX,
            hkx,
        );
    }
    // The state: property list at 0x130, part list at 0x134, conditions at 0x138.
    put(&mut d, 0x88, 0xFE60_4109);
    put(&mut d, 0x8C, s("State"));
    put(&mut d, 0x90, s("S0"));
    d[0x94..0xA4].copy_from_slice(b"\x01\x02\x03\x04\x05\x06\x07\x08MMOPTIMI");
    put(&mut d, 0xA4, 0x130);
    put16(&mut d, 0xA8, 1);
    put16(&mut d, 0xAA, 1);
    put(&mut d, 0xAC, 0x134);
    put(&mut d, 0xB0, 0x138);
    put16(&mut d, 0xB4, 1);
    put16(&mut d, 0xB6, 0);
    put(&mut d, 0xB8, s("health"));
    put(&mut d, 0xC0, 4);
    // Properties: a matrix (identity, at 0x1C0) and the mesh of the part.
    put(&mut d, 0xC4, 0x1C0);
    put(&mut d, 0xC8, s("CMD_LoadMatrix"));
    put(&mut d, 0xCC, s("CSystemCommands"));
    put(&mut d, 0xD0, 1);
    put(&mut d, 0xD8, 0x20);
    put(&mut d, 0xDC, T0);
    put(&mut d, 0xE0, s("Model"));
    put(&mut d, 0xE4, s("ModelPart"));
    put(&mut d, 0xE8, 1);
    put(&mut d, 0xF0, 0x10);
    put(&mut d, 0xF4, 50f32.to_bits());
    put(&mut d, 0xF8, 0xB8);
    put(&mut d, 0xFC, 3);
    put(&mut d, 0x104, s("ModelPart"));
    put(&mut d, 0x108, s("P0"));
    put(&mut d, 0x11C, 0x13C);
    put16(&mut d, 0x120, 1);
    put16(&mut d, 0x122, 2);
    put(&mut d, 0x130, 0xC4);
    put(&mut d, 0x134, 0x100);
    put(&mut d, 0x138, 0xF4);
    put(&mut d, 0x13C, 0xDC);
    for k in 0..4 {
        put(&mut d, 0x1C0 + 20 * k, 1f32.to_bits());
    }
    d
}

/// Adds a level to the miniature game of [`write_game`]: [`TOWN`] with a
/// world grid and its zone [`YARD`] with a crate that has a mesh, Havok
/// collision and a metamodel, placed by an object at [`CRATE_POSITION`].
/// Every stream of the miniature game goes into the table of contents.
pub fn write_town(root: &Path) -> Result<()> {
    use crate::resource::hkx::collision::{self, CollisionKind, CollisionMesh};
    use crate::resource::simgroup::{CLASS_ENTITY, CLASS_SYSTEM_COMMANDS, Guid, SimGroup};
    use crate::resource::wgi::{self, NewCell, WorldGrid};
    let io = |path: &Path, e| Error::io(path, e);
    let yard_guid = *b"\x21\x22\x23\x24\x25\x26\x27\x28DOCTORTK";
    let mut grid = WorldGrid {
        streams: Vec::new(),
        cells: Vec::new(),
        pool: Vec::new(),
    };
    grid.add_cell(&NewCell {
        guid: yard_guid,
        box_min: [0.0, -1.0, 0.0],
        box_max: [20.0, 10.0, 20.0],
        kind: wgi::KIND_WORLD,
        flags: 0,
        lod_mask: wgi::lod::HIGH,
        name: None,
        needs: None,
    })?;
    let mut town = StreamWriter::new(&FileHeader::GF2_PC, Endian::Little);
    town.push_resource(
        &ResourceHeader {
            type_id: wgi::TYPE_WGI,
            ..header(b"wgi", [0x77; 4])
        },
        &grid.to_bytes()?,
        Storage::Raw,
    )?;
    let town = town.finish();

    let (mesh, volatile) = quad_mesh("crate", MATERIAL_NAME)?;
    let mut shape = CollisionMesh::default();
    shape.append(
        &[
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [0.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
        ],
        &[[0, 1, 2], [2, 1, 3], [4, 6, 5], [5, 6, 7]],
    );
    let (hkx, _) = collision::build(
        &shape,
        CollisionKind::Box,
        collision::DEFAULT_MATERIAL,
        "crate",
    )?;
    let hkx_id = 0x7001_0003;
    let mut group = SimGroup::empty();
    let object = group.push_object(
        CLASS_ENTITY,
        Guid(*b"\x31\x32\x33\x34\x35\x36\x37\x38DOCTORTK"),
        &[(CLASS_SYSTEM_COMMANDS, 4), (CLASS_ENTITY, 4)],
    )?;
    let mut matrix = [[0.0f32; 4]; 4];
    for (k, row) in matrix.iter_mut().enumerate() {
        row[k] = 1.0;
    }
    matrix[3][..3].copy_from_slice(&CRATE_POSITION);
    group.set_transform(object, matrix)?;
    group.set_model(object, Some(CRATE_MODEL_ID))?;
    let mut yard = StreamWriter::new(&FileHeader::GF2_PC, Endian::Little);
    yard.push_resource(
        &header(b"mesh", [CRATE_MESH_ID; 4]),
        &mesh,
        Storage::RefPack,
    )?;
    yard.push_resource(&header(b"hkx", [hkx_id; 4]), &hkx, Storage::RefPack)?;
    yard.push_resource(
        &header(b"metamodel", [CRATE_MODEL_ID; 4]),
        &prop_metamodel(CRATE_MODEL_ID, CRATE_MESH_ID, Some(hkx_id)),
        Storage::RefPack,
    )?;
    yard.push_resource(
        &header(b"simgroup", [0x72; 4]),
        &group.encode(),
        Storage::RefPack,
    )?;
    if !volatile.is_empty() {
        yard.push_resource(
            &header(b"meshvolatile", [!CRATE_MESH_ID; 4]),
            &volatile,
            Storage::Raw,
        )?;
    }
    let yard = yard.finish();

    fs::create_dir_all(root.join(TOWN)).map_err(|e| io(root, e))?;
    for (name, data) in [(TOWN, &town), (YARD, &yard)] {
        let path = root.join(format!("{name}.str"));
        fs::write(&path, data).map_err(|e| io(&path, e))?;
    }
    let mut listed = Vec::new();
    for name in [LEVEL, TEXT, NAV, GLOBALS, CUTSCENE, TOWN, YARD] {
        let path = root.join(format!("{name}.str"));
        if let Ok(data) = fs::read(&path) {
            listed.push((name, StreamLayout::parse(&data)?.total_data_size()));
        }
    }
    let table = Toc::parse(&toc_bytes(&listed))?;
    let path = root.join("gf2/level.toc");
    fs::write(&path, table.to_bytes()?).map_err(|e| io(&path, e))
}

/// The sound track of the cut-scene added by [`write_cutscene_sound`].
pub const CUTSCENE_SOUND: &str = "audiostreams\\igc99\\aud_igc99.exa.snu";

/// A streamed sound of three seconds whose subtitle table has two lines
/// (at 0.5 s for 0.4 s and at 1 s for 0.8 s) and whose sound data is `sound`.
pub fn cutscene_sound(sound: &[u8]) -> Vec<u8> {
    use crate::resource::tob::{Tob, TobEntry, TobGroup};
    let table = Tob {
        version: 1,
        movie_hash: sdbm(b"igc99"),
        groups: vec![TobGroup {
            name_hash: sdbm(b"subtitles"),
            name: Some("subtitles"),
            entries: vec![
                TobEntry {
                    string: 0x1111,
                    start_ms: 500,
                    duration_ms: 400,
                },
                TobEntry {
                    string: 0x2222,
                    start_ms: 1000,
                    duration_ms: 800,
                },
            ],
        }],
    }
    .to_bytes();
    let mut snu = vec![3u8, 4, 0, 2];
    snu.extend_from_slice(&3000u32.to_le_bytes());
    let blocks = (0x20 + table.len()).div_ceil(16) * 16;
    snu.extend_from_slice(&(blocks as u32).to_le_bytes());
    snu.resize(0x20, 0);
    snu.extend_from_slice(&table);
    snu.resize(blocks, 0);
    snu.extend_from_slice(sound);
    snu
}

/// Adds the archive `GODFATH1.viv` with the sound track of the cut-scene of
/// [`write_cutscene`]: [`cutscene_sound`] with the sound data `sound`.
pub fn write_cutscene_sound_with(root: &Path, sound: &[u8]) -> Result<()> {
    let archive = crate::container::big::build_archive(&[(
        CUTSCENE_SOUND.to_owned(),
        cutscene_sound(sound),
    )])?;
    let path = root.join("GODFATH1.viv");
    fs::write(&path, archive).map_err(|e| Error::io(&path, e))?;
    Ok(())
}

/// [`write_cutscene_sound_with`] the sound data `SOUND DATA`.
pub fn write_cutscene_sound(root: &Path) -> Result<()> {
    write_cutscene_sound_with(root, b"SOUND DATA")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{GameAssets, GameInstance};

    #[test]
    fn the_mini_game_opens_and_its_model_finds_its_texture() {
        let dir = tempfile::tempdir().unwrap();
        write_game(dir.path()).unwrap();
        let game = std::sync::Arc::new(GameInstance::open(dir.path()).unwrap());
        let level = game.find_stream(LEVEL).unwrap();
        let (index, errors) = game.build_index(|_| {});
        assert!(errors.is_empty(), "{errors:?}");
        let mut assets = GameAssets::new(game.clone(), &index);
        let locator = crate::character::MeshLocator::new(&index);
        let meshes = crate::retexture::meshes_of(&game, &locator, (level, MESH)).unwrap();
        assert_eq!(meshes, [(level, MESH)]);
        let textures =
            crate::retexture::textures_of(&game, &index, &mut assets, &meshes, &Default::default())
                .unwrap();
        assert_eq!(textures.len(), 1);
        let t = &textures[0];
        assert_eq!(
            (t.hash, t.name.as_str(), t.size),
            (TEXTURE_HASH, "wall", (4, 4))
        );
        assert_eq!(t.role, crate::resource::material::TextureRole::Diffuse);
        assert_eq!(t.materials, [MATERIAL_NAME]);
        assert_eq!(t.copies, [(level, TEX_HEADER)]);
        assert_eq!(t.pairs(&game).unwrap()[0].1.data, TEX_DATA);
        let file = game.open_stream(level).unwrap();
        let db = PartsDb::parse(&file.resource_data(PARTSDB).unwrap()).unwrap();
        assert_eq!(db.parts[0].models[0], Hash32(MESH_ID));
    }

    #[test]
    fn the_triangles_import_over_the_mesh() {
        use crate::import::{gltf, model};
        let dir = tempfile::tempdir().unwrap();
        write_game(dir.path()).unwrap();
        let game = std::sync::Arc::new(GameInstance::open(dir.path()).unwrap());
        let level = game.find_stream(LEVEL).unwrap();
        let index = game.build_index(|_| {}).0;
        let mut assets = GameAssets::new(game.clone(), &index);
        let ws = crate::workspace::Workspace::for_game(&game, None).unwrap();
        for skinned in [false, true] {
            let glb = triangle_glb(2.0, "new_cloth", skinned);
            let parsed = gltf::read(&glb, None).unwrap();
            let options = model::ImportOptions {
                new_name: Some("outfit".into()),
                ..Default::default()
            };
            let plan = model::plan(
                &game,
                &ws,
                &index,
                &mut assets,
                level,
                MESH,
                &parsed,
                &options,
            )
            .unwrap();
            assert_eq!(plan.summary.skeleton.is_some(), skinned);
            assert_eq!(plan.mesh_ids().len(), 1);
            assert_ne!(plan.mesh_ids()[0], MESH_ID);
            assert_eq!(plan.new_materials().len(), 1, "{:?}", plan.summary);
        }
    }
}
