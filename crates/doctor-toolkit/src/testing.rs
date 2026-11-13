//! A miniature game for the tests of programs built on the library (feature
//! `testing`): a few streams with a texture, a material, a mesh that uses
//! them, a SimGroup, a parts database, a rig and string tables.

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
