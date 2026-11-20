//! Tests against an installed copy of The Godfather II (PC).
//!
//! Set `EARS_GF2_DIR` to the game directory to run them; without it they pass
//! without doing anything. Run them in release mode, the corpus is ~9 GB:
//!
//! ```sh
//! EARS_GF2_DIR="/path/to/The Godfather II" cargo test --release --test corpus
//! ```

use std::path::PathBuf;
use std::sync::Mutex;

use rayon::prelude::*;

use doctor_toolkit::container::ustream::{self, Replacement, Storage, StreamLayout};
use doctor_toolkit::game::{GameInstance, StreamFile};
use doctor_toolkit::resource::names::type_tags;
use doctor_toolkit::toc::Toc;

#[path = "corpus/copies.rs"]
mod copies;
#[path = "corpus/crew.rs"]
mod crew;
#[path = "corpus/delta.rs"]
mod delta;
#[path = "corpus/lua_api.rs"]
mod lua_api;
#[path = "corpus/mod_deltas.rs"]
mod mod_deltas;
#[path = "corpus/project.rs"]
mod project;
#[path = "corpus/rebase.rs"]
mod rebase;
#[path = "corpus/resources_in_memory.rs"]
mod resources_in_memory;
#[path = "corpus/story.rs"]
mod story;
#[path = "corpus/types.rs"]
mod types;
#[path = "corpus/whole_game.rs"]
mod whole_game;

fn game_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("EARS_GF2_DIR")?);
    assert!(dir.is_dir(), "EARS_GF2_DIR does not point to a directory");
    Some(dir)
}

/// Collects failures from worker threads and reports them all at once.
#[derive(Default)]
struct Failures(Mutex<Vec<String>>);

impl Failures {
    fn push(&self, message: String) {
        self.0.lock().unwrap().push(message);
    }

    fn assert_empty(self) {
        let failures = self.0.into_inner().unwrap();
        if let Some(path) = std::env::var_os("DOCTOR_FAILURES_FILE") {
            let _ = std::fs::write(path, failures.join("\n"));
        }
        let shown: Vec<_> = failures.iter().take(20).collect();
        assert!(
            failures.is_empty(),
            "{} failure(s):\n{shown:#?}",
            failures.len()
        );
    }
}

#[test]
fn every_stream_parses_and_rewrites_identically() {
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    assert_eq!(game.streams().len(), 2300);
    let failures = Failures::default();
    game.streams().par_iter().for_each(|file| {
        let stream = match StreamFile::open(&file.path) {
            Ok(stream) => stream,
            Err(error) => return failures.push(format!("{}: {error}", file.rel_path)),
        };
        let (data, layout) = (stream.data(), stream.layout());
        if ustream::rebuild(data, layout, &[]).ok().as_deref() != Some(data) {
            failures.push(format!("{}: rebuild differs", file.rel_path));
        }
        // Laying out every block from scratch must reproduce the game's page layout.
        // globals_global.str was rewritten by the old toolkit with its own layout;
        // its original is checked in `original_streams_relayout_identically`.
        if file.rel_path != "gf2/globals_global.str"
            && ustream::relayout(data, layout).ok().as_deref() != Some(data)
        {
            failures.push(format!("{}: relayout differs", file.rel_path));
        }
    });
    failures.assert_empty();
}

#[test]
fn every_resource_decompresses_to_its_declared_size() {
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let failures = Failures::default();
    game.streams().par_iter().for_each(|file| {
        let stream = StreamFile::open(&file.path).unwrap();
        for index in 0..stream.layout().resources.len() {
            match stream
                .layout()
                .resource_data_with_excess(stream.data(), index)
            {
                Ok((data, excess)) => {
                    let declared = stream.layout().resources[index].header.data_size as usize;
                    assert_eq!(data.len(), declared);
                    // One resource in globals_global.str carries 16 surplus bytes,
                    // appended by the "Improved Effects" patch of the old toolkit.
                    if excess > 0 && !(file.rel_path == "gf2/globals_global.str" && excess == 13) {
                        failures.push(format!(
                            "{} #{index}: {excess} surplus byte(s)",
                            file.rel_path
                        ));
                    }
                }
                Err(error) => failures.push(format!("{} #{index}: {error}", file.rel_path)),
            }
        }
    });
    failures.assert_empty();
}

#[test]
fn every_toc_parses_rewrites_identically_and_matches_streams() {
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    assert_eq!(game.tocs().len(), 25);
    // These streams were modified by patches of the old toolkit, which left
    // the sizes in the tables of contents unchanged.
    let patched = [
        "gf2\\globals_global",
        "gf2\\miami\\local",
        "gf2\\queens\\local",
    ];
    for (toc_index, file) in game.tocs().iter().enumerate() {
        let bytes = std::fs::read(&file.path).unwrap();
        let table = Toc::parse(&bytes).unwrap();
        assert_eq!(table.to_bytes().unwrap(), bytes, "{}", file.rel_path);
        assert!(table.is_sorted_by_hash(), "{}", file.rel_path);
        assert_eq!(game.load_toc(toc_index).unwrap(), table);
        for index in 0..table.files.len() {
            let name = table.file_name(index).name_lossy();
            let stream_index = game
                .find_stream(&name)
                .unwrap_or_else(|| panic!("{}: `{name}` has no stream", file.rel_path));
            let actual = game
                .open_stream(stream_index)
                .unwrap()
                .layout()
                .total_data_size();
            let listed = table.files[index].data_size as u64;
            if !patched.contains(&name.as_str()) {
                assert_eq!(listed, actual, "{}: `{name}`", file.rel_path);
            }
        }
    }
}

#[test]
fn tables_of_contents_match_original_streams() {
    let Some(dir) = game_dir() else { return };
    // The old toolkit keeps the originals of the files it patched.
    let backups = dir.join("_toolkit/backups");
    if !backups.is_dir() {
        return;
    }
    let game = GameInstance::open(&dir).unwrap();
    for (name, toc_name) in [
        ("gf2/globals_global.str", "gf2/globals_global.toc"),
        ("gf2/miami/local.str", "gf2/miami.toc"),
        ("gf2/queens/local.str", "gf2/queens.toc"),
    ] {
        let stream = StreamFile::open(backups.join(name)).unwrap();
        let toc_index = game
            .tocs()
            .iter()
            .position(|t| t.rel_path == toc_name)
            .unwrap();
        let table = game.load_toc(toc_index).unwrap();
        let entry = table.find_file(name.trim_end_matches(".str")).unwrap();
        assert_eq!(
            table.files[entry].data_size as u64,
            stream.layout().total_data_size(),
            "{name}"
        );
    }
}

#[test]
fn original_streams_relayout_identically() {
    let Some(dir) = game_dir() else { return };
    let backups = dir.join("_toolkit/backups");
    for name in [
        "gf2/globals_global.str",
        "gf2/frontend.str",
        "gf2/miami/local.str",
        "gf2/queens/local.str",
    ] {
        let path = backups.join(name);
        if !path.is_file() {
            continue;
        }
        let stream = StreamFile::open(&path).unwrap();
        let relaid = ustream::relayout(stream.data(), stream.layout()).unwrap();
        assert!(relaid == stream.data(), "{name}: relayout differs");
    }
}

#[test]
fn every_type_id_has_a_name() {
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let (index, errors) = game.build_index(|_| {});
    assert!(errors.is_empty());
    assert_eq!(index.entries.len(), 212_544);
    let mut unknown: Vec<String> = index
        .type_summary()
        .iter()
        .filter(|t| type_tags().get(t.type_id).is_none())
        .map(|t| format!("{:08x}", t.type_id))
        .collect();
    unknown.sort();
    // The last two, `treemodel` and `covernodetable`, were named in stage 40.
    assert!(unknown.is_empty(), "{unknown:?}");
}

#[test]
fn replacing_a_resource_in_a_real_stream_keeps_the_others() {
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let file = &game.streams()[game.find_stream("gf2/frontend").unwrap()];
    let stream = StreamFile::open(&file.path).unwrap();
    let (data, layout) = (stream.data(), stream.layout());
    // Replace a resource in the middle with compressed and with raw data.
    let target = layout.resources.len() / 2;
    let original: Vec<Vec<u8>> = (0..layout.resources.len())
        .map(|i| stream.resource_data(i).unwrap())
        .collect();
    for storage in [Storage::Raw, Storage::RefPack] {
        let mut new_data = original[target].clone();
        new_data.extend_from_slice(b"doctor_toolkit");
        let rebuilt = ustream::rebuild(
            data,
            layout,
            &[Replacement {
                index: target,
                data: &new_data,
                storage,
            }],
        )
        .unwrap();
        let new_layout = StreamLayout::parse(&rebuilt).unwrap();
        assert_eq!(new_layout.resources.len(), layout.resources.len());
        for (i, expected) in original.iter().enumerate() {
            let actual = new_layout.resource_data(&rebuilt, i).unwrap();
            if i == target {
                assert_eq!(actual, new_data);
            } else {
                assert_eq!(&actual, expected, "resource {i}");
            }
        }
    }
}

#[test]
fn every_texture_parses_pairs_and_converts() {
    use doctor_toolkit::resource::{dds, texture, tg4::Tg4Header};
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let failures = Failures::default();
    let textures = std::sync::atomic::AtomicUsize::new(0);
    game.streams().par_iter().for_each(|file| {
        let stream = StreamFile::open(&file.path).unwrap();
        let layout = stream.layout();
        let pairs = texture::pairs(layout);
        let headers = layout
            .resources
            .iter()
            .filter(|r| r.header.type_id == texture::TYPE_TG4H)
            .count();
        if pairs.len() != headers {
            failures.push(format!(
                "{}: {} of {headers} headers paired",
                file.rel_path,
                pairs.len()
            ));
        }
        for pair in pairs {
            let raw = stream.resource_data(pair.header).unwrap();
            let header = match Tg4Header::parse(&raw) {
                Ok(header) => header,
                Err(error) => {
                    return failures.push(format!("{} #{}: {error}", file.rel_path, pair.header));
                }
            };
            let at = format!("{} #{} {}", file.rel_path, pair.header, header.name);
            if header.to_bytes().ok().as_deref() != Some(&raw[..]) {
                failures.push(format!("{at}: header rewrite differs"));
            }
            let tex = match texture::Texture::load(stream.data(), layout, pair) {
                Ok(tex) => tex,
                Err(error) => return failures.push(format!("{at}: {error}")),
            };
            if header.expected_data_size().ok() != Some(tex.data.len()) {
                failures.push(format!("{at}: data size does not match the mip chain"));
            }
            match tex
                .to_dds()
                .and_then(|file| dds::read(&file).map(|img| img.data == tex.data))
            {
                Ok(true) => {}
                Ok(false) => failures.push(format!("{at}: DDS data differs")),
                Err(error) => failures.push(format!("{at}: DDS: {error}")),
            }
            if let Err(error) = tex.rgba(0) {
                failures.push(format!("{at}: decode: {error}"));
            }
            textures.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    });
    failures.assert_empty();
    assert_eq!(textures.into_inner(), 28_246);
}

#[test]
fn every_string_table_parses_and_rewrites_identically() {
    use doctor_toolkit::hash::sdbm;
    use doctor_toolkit::resource::lh2::StringTable;
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let lh2 = sdbm(b"lh2");
    let mut tables = 0;
    let mut strings = 0;
    for file in game.streams() {
        let stream = StreamFile::open(&file.path).unwrap();
        for (index, resource) in stream.layout().resources.iter().enumerate() {
            if resource.header.type_id != lh2 {
                continue;
            }
            let data = stream.resource_data(index).unwrap();
            let table = StringTable::parse(&data)
                .unwrap_or_else(|e| panic!("{} #{index}: {e}", file.rel_path));
            assert_eq!(
                table.to_bytes().unwrap(),
                data,
                "{} #{index}",
                file.rel_path
            );
            assert!(table.hashes.is_sorted());
            tables += 1;
            strings += table.strings.iter().map(Vec::len).sum::<usize>();
        }
    }
    assert_eq!(tables, 21);
    assert!(strings > 30_000, "{strings} strings");
}

#[test]
fn every_apx_parses_rewrites_identically_and_yields_textures() {
    use doctor_toolkit::hash::sdbm;
    use doctor_toolkit::resource::apx::{AptFile, TAG_APT, TAG_TEXTURE_HEADER};
    use doctor_toolkit::resource::tg4;
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let apx = sdbm(b"apx");
    // The old toolkit rewrote apx resources of globals_global.str with its own
    // alignment; its original is checked instead.
    let mut paths: Vec<(String, PathBuf)> = game
        .streams()
        .iter()
        .filter(|f| f.rel_path != "gf2/globals_global.str")
        .map(|f| (f.rel_path.clone(), f.path.clone()))
        .collect();
    let original = dir.join("_toolkit/backups/gf2/globals_global.str");
    if original.is_file() {
        paths.push(("backup gf2/globals_global.str".into(), original));
    }
    let (mut files, mut textures) = (0, 0);
    for (name, path) in paths {
        let stream = StreamFile::open(&path).unwrap();
        for (index, resource) in stream.layout().resources.iter().enumerate() {
            if resource.header.type_id != apx {
                continue;
            }
            let at = format!("{name} #{index}");
            let data = stream.resource_data(index).unwrap();
            let parsed = AptFile::parse(&data).unwrap_or_else(|e| panic!("{at}: {e}"));
            assert!(parsed.to_bytes().unwrap() == data, "{at}: rewrite differs");
            assert!(
                parsed
                    .block(TAG_APT)
                    .unwrap()
                    .data
                    .starts_with(b"Apt Data:"),
                "{at}"
            );
            for block in parsed.blocks.iter().filter(|b| b.tag == TAG_TEXTURE_HEADER) {
                let set = tg4::parse_set(&block.data).unwrap();
                assert_eq!(
                    tg4::set_to_bytes(&set).unwrap(),
                    block.data,
                    "{at}: tx4h rewrite differs"
                );
            }
            for texture in parsed.textures().unwrap_or_else(|e| panic!("{at}: {e}")) {
                assert_eq!(
                    texture.header.expected_data_size().unwrap(),
                    texture.data.len(),
                    "{at}"
                );
                texture.rgba(0).unwrap();
                textures += 1;
            }
            files += 1;
        }
    }
    assert!(files >= 90, "{files} apx files");
    assert!(textures >= 400, "{textures} embedded textures");
    eprintln!("apx files: {files}, embedded textures: {textures}");
}

#[test]
fn every_mesh_decodes_within_its_bounds() {
    use doctor_toolkit::resource::mesh::{self, TYPE_MESH};
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let failures = Failures::default();
    let models = std::sync::atomic::AtomicUsize::new(0);
    let records = std::sync::atomic::AtomicUsize::new(0);
    // Tangents of CPU-skinned vertices by their two w bits.
    let tangent_w: [std::sync::atomic::AtomicUsize; 4] = Default::default();
    // Records whose primitive count (0x3E) wrapped past 65 535.
    let wrapped = std::sync::atomic::AtomicUsize::new(0);
    game.streams().par_iter().for_each(|file| {
        let stream = StreamFile::open(&file.path).unwrap();
        for (index, resource) in stream.layout().resources.iter().enumerate() {
            if resource.header.type_id != TYPE_MESH {
                continue;
            }
            let at = format!("{} #{index}", file.rel_path);
            let model = match mesh::load(&stream, index) {
                Ok(model) => model,
                Err(error) => return failures.push(format!("{at}: {error}")),
            };
            let slack = 1e-3 * (1.0 + model.bounding_sphere[3]);
            // The primitive count of every record is what the mesh builder writes.
            let data = stream.resource_data(index).unwrap();
            let records_at = u32::from_le_bytes(data[0x50..0x54].try_into().unwrap()) as usize;
            for (k, record) in model.records.iter().enumerate() {
                let at3e = records_at + 0xB0 * k + 0x3E;
                let stored = u16::from_le_bytes(data[at3e..at3e + 2].try_into().unwrap());
                let expected = doctor_toolkit::resource::mesh_build::primitive_count(
                    record.primitive,
                    record.index_count as usize,
                );
                if stored != expected {
                    failures.push(format!(
                        "{at} {}: primitive count {stored}, expected {expected}",
                        record.name
                    ));
                }
                if record.index_count > 0xFFFF && record.primitive == mesh::Primitive::TriangleStrip
                {
                    wrapped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
            for record in &model.records {
                let check = || -> Result<(), String> {
                    let v = model.vertices(record).map_err(|e| e.to_string())?;
                    model.triangles(record).map_err(|e| e.to_string())?;
                    for p in &v.positions {
                        for i in 0..3 {
                            if !(p[i] >= model.aabb_min[i] - slack
                                && p[i] <= model.aabb_max[i] + slack)
                            {
                                return Err(format!("position {p:?} outside the model bounds"));
                            }
                        }
                    }
                    if let Some(normals) = &v.normals
                        && normals.iter().any(|n| {
                            ((n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() - 1.0).abs() > 1e-3
                        })
                    {
                        return Err("normal is not unit length".into());
                    }
                    Ok(())
                };
                if let Err(error) = check() {
                    failures.push(format!("{at} {}: {error}", record.name));
                }
                match model.skinned_tangent_w(record) {
                    Ok(ws) => {
                        for w in ws {
                            tangent_w[w as usize]
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    Err(error) => failures.push(format!("{at} {}: {error}", record.name)),
                }
                records.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            models.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    });
    failures.assert_empty();
    assert_eq!(models.into_inner(), 17_622);
    assert_eq!(records.into_inner(), 157_229);
    assert_eq!(wrapped.into_inner(), 1, "strips longer than 65 535 indices");
    // The bitangent sign of a skinned tangent is 0 or 2 in w, never 1 or 3:
    // the mesh builder writes the same.
    let tangent_w = tangent_w.map(|c| c.into_inner());
    eprintln!("skinned tangents by w: {tangent_w:?}");
    assert!(tangent_w[0] > 0 && tangent_w[2] > 0, "{tangent_w:?}");
    assert_eq!((tangent_w[1], tangent_w[3]), (0, 0), "{tangent_w:?}");
}

#[test]
fn every_mesh_reencodes_byte_for_byte() {
    use doctor_toolkit::resource::mesh::{TYPE_MESH, volatile_for};
    use doctor_toolkit::resource::mesh_image::MeshImage;
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let failures = Failures::default();
    let models = std::sync::atomic::AtomicUsize::new(0);
    game.streams().par_iter().for_each(|file| {
        let stream = StreamFile::open(&file.path).unwrap();
        for (index, resource) in stream.layout().resources.iter().enumerate() {
            if resource.header.type_id != TYPE_MESH {
                continue;
            }
            let at = format!("{} #{index}", file.rel_path);
            let mesh = stream.resource_data(index).unwrap();
            let volatile = volatile_for(stream.layout(), index)
                .map(|v| stream.resource_data(v).unwrap())
                .unwrap_or_default();
            if let Ok(image) = MeshImage::parse(&mesh, &volatile) {
                // The vertex offset of every record equals its first vertex.
                for r in image
                    .records
                    .iter()
                    .filter(|r| r.u16_at(0x3A) != r.u16_at(0x40))
                {
                    failures.push(format!("{at}: record {} vertex offset", r.name));
                }
            }
            match MeshImage::parse(&mesh, &volatile).and_then(|m| m.encode()) {
                Ok((m, v)) if m == mesh && v == volatile => {}
                Ok(_) => failures.push(format!("{at}: re-encoded mesh differs")),
                Err(error) => failures.push(format!("{at}: {error}")),
            }
            models.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    });
    failures.assert_empty();
    assert_eq!(models.into_inner(), 17_622);
}

/// Every model exported to glTF (geometry and skin) and imported back with
/// the original as template gives the same triangles: positions exact,
/// normals, UVs and skin weights within the precision of their encodings,
/// skin joints identical.
#[test]
fn exported_models_import_back_with_the_same_geometry() {
    use doctor_toolkit::export::gltf::{self as export, AssetSource};
    use doctor_toolkit::import::{gltf as import, model};
    use doctor_toolkit::resource::mesh::{Model, TYPE_MESH, volatile_for};
    use doctor_toolkit::resource::mesh_build::{self, BuildOptions};
    use doctor_toolkit::resource::mesh_image::MeshImage;
    use doctor_toolkit::resource::rcb::{Rig, Skeleton, TYPE_RCB};

    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let rigs: Vec<Rig> = game
        .streams()
        .par_iter()
        .flat_map_iter(|file| {
            let stream = StreamFile::open(&file.path).unwrap();
            let layout = stream.layout();
            (0..layout.resources.len())
                .filter(|&i| layout.resources[i].header.type_id == TYPE_RCB)
                .map(|i| Rig::parse(&stream.resource_data(i).unwrap()).unwrap())
                .collect::<Vec<_>>()
        })
        .collect();
    struct Skeletons<'a>(&'a [Rig]);
    impl AssetSource for Skeletons<'_> {
        fn material(&mut self, _: u32) -> Option<doctor_toolkit::resource::material::Material> {
            None
        }
        fn texture(&mut self, _: u32) -> Option<doctor_toolkit::resource::texture::Texture> {
            None
        }
        fn skeleton(&mut self, bones: &[(u32, u32)]) -> Option<Skeleton> {
            self.0.iter().find_map(|r| r.skeleton_for(bones)).cloned()
        }
    }

    /// Triangles as position triples, rotated to start at the smallest
    /// position, with the per-corner attributes, sorted.
    type Corner = ([f32; 3], [f32; 3], [f32; 2], [u16; 4], [f32; 4]);
    fn corners(model: &Model, bones: usize, names: &[usize]) -> Vec<[Corner; 3]> {
        let mut out = Vec::new();
        for &r in names {
            let record = &model.records[r];
            let v = model.vertices(record).unwrap();
            let skin = export::vertex_skin(&record.palette, record.rigid_bone, &v, bones);
            for t in model.triangles(record).unwrap() {
                let c = t.map(|i| {
                    let i = i as usize;
                    let (mut j, mut w) = if bones > 0 {
                        skin[i]
                    } else {
                        ([0; 4], [0.0; 4])
                    };
                    // Order influences by bone so that both sides compare.
                    let mut pairs: Vec<(u16, f32)> = (0..4)
                        .filter(|&k| w[k] > 0.0)
                        .map(|k| (j[k], w[k]))
                        .collect();
                    pairs.sort_by_key(|p| p.0);
                    (j, w) = ([0; 4], [0.0; 4]);
                    for (k, p) in pairs.iter().enumerate() {
                        j[k] = p.0;
                        w[k] = p.1;
                    }
                    (
                        v.positions[i],
                        v.normals.as_ref().map_or([0.0; 3], |n| n[i]),
                        v.uv0.as_ref().map_or([0.0; 2], |u| u[i]),
                        j,
                        w,
                    )
                });
                let r = (0..3)
                    .min_by(|&a, &b| c[a].0.partial_cmp(&c[b].0).unwrap())
                    .unwrap();
                out.push([c[r], c[(r + 1) % 3], c[(r + 2) % 3]]);
            }
        }
        out.sort_by(|a, b| {
            let key = |t: &[Corner; 3]| [t[0].0, t[1].0, t[2].0];
            key(a).partial_cmp(&key(b)).unwrap()
        });
        out
    }

    let failures = Failures::default();
    let models = std::sync::atomic::AtomicUsize::new(0);
    let triangles = std::sync::atomic::AtomicUsize::new(0);
    game.streams().par_iter().for_each(|file| {
        let stream = StreamFile::open(&file.path).unwrap();
        let mut source = Skeletons(&rigs);
        for (index, resource) in stream.layout().resources.iter().enumerate() {
            if resource.header.type_id != TYPE_MESH {
                continue;
            }
            let at = format!("{} #{index}", file.rel_path);
            let mesh = stream.resource_data(index).unwrap();
            let volatile = volatile_for(stream.layout(), index)
                .map(|v| stream.resource_data(v).unwrap())
                .unwrap_or_default();
            let original = Model::parse(&mesh, &volatile).unwrap();
            let template = MeshImage::parse(&mesh, &volatile).unwrap();
            let skeleton = if original.bones.is_empty() {
                None
            } else {
                source.skeleton(&original.bones)
            };
            let mut check = || -> Result<usize, String> {
                let Ok(glb) = export::model_to_glb(&original, &mut source) else {
                    return Ok(0); // no triangles
                };
                let gltf = import::read(&glb, None).map_err(|e| e.to_string())?;
                // Parts follow the non-empty records.
                let kept: Vec<usize> = (0..original.records.len())
                    .filter(|&r| !original.triangles(&original.records[r]).unwrap().is_empty())
                    .collect();
                if gltf.parts.len() != kept.len() {
                    return Err(format!(
                        "{} parts for {} records",
                        gltf.parts.len(),
                        kept.len()
                    ));
                }
                let mut parts: Vec<_> = gltf.parts.iter().map(|p| p.geometry.clone()).collect();
                for (part, &r) in parts.iter_mut().zip(&kept) {
                    part.material_hash = original.records[r].material_hash;
                    part.name = format!("part{r}");
                }
                if let Some(s) = &skeleton {
                    model::bind_to_skeleton(&gltf, s, &mut parts).map_err(|e| e.to_string())?;
                }
                let (built, _) = mesh_build::build(
                    &parts,
                    &BuildOptions {
                        name: original.name.clone(),
                        skeleton: skeleton
                            .as_ref()
                            .map(|s| s.bones.iter().map(|b| b.hash).collect()),
                        template: Some(&template),
                        ..Default::default()
                    },
                )
                .map_err(|e| e.to_string())?;
                if let Some(r) = built
                    .records
                    .iter()
                    .find(|r| r.u16_at(0x3A) != r.u16_at(0x40))
                {
                    return Err(format!("built record {}: vertex offset", r.name));
                }
                let (m, v) = built.encode().map_err(|e| e.to_string())?;
                let back = Model::parse(&m, &v).map_err(|e| e.to_string())?;
                let bones = skeleton.as_ref().map_or(0, |s| s.bones.len());
                let mut count = 0;
                for &r in &kept {
                    let name = format!("part{r}");
                    let records: Vec<usize> = (0..back.records.len())
                        .filter(|&b| back.records[b].name == name)
                        .collect();
                    let a = corners(&original, bones, &[r]);
                    let b = corners(&back, bones, &records);
                    if a.len() != b.len() {
                        return Err(format!(
                            "record {r}: {} triangles, {} back",
                            a.len(),
                            b.len()
                        ));
                    }
                    for (ta, tb) in a.iter().zip(&b) {
                        for (ca, cb) in ta.iter().zip(tb) {
                            let close = |x: &[f32], y: &[f32], tol: f32| {
                                x.iter().zip(y).all(|(p, q)| (p - q).abs() <= tol)
                            };
                            if ca.0 != cb.0 {
                                return Err(format!(
                                    "record {r}: position {:?} vs {:?}",
                                    ca.0, cb.0
                                ));
                            }
                            if !close(&ca.1, &cb.1, 0.02) {
                                return Err(format!("record {r}: normal {:?} vs {:?}", ca.1, cb.1));
                            }
                            if !close(
                                &ca.2,
                                &cb.2,
                                1e-3 * (1.0 + ca.2[0].abs().max(ca.2[1].abs())),
                            ) {
                                return Err(format!("record {r}: uv {:?} vs {:?}", ca.2, cb.2));
                            }
                            if ca.3 != cb.3 || !close(&ca.4, &cb.4, 1e-3) {
                                return Err(format!(
                                    "record {r}: skin {:?} {:?} vs {:?} {:?}",
                                    ca.3, ca.4, cb.3, cb.4
                                ));
                            }
                        }
                    }
                    count += a.len();
                }
                Ok(count)
            };
            match check() {
                Ok(n) => {
                    triangles.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
                }
                Err(error) => failures.push(format!("{at} {}: {error}", original.name)),
            }
            models.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    });
    eprintln!(
        "{} models, {} triangles imported back",
        models.load(std::sync::atomic::Ordering::Relaxed),
        triangles.load(std::sync::atomic::Ordering::Relaxed)
    );
    failures.assert_empty();
    assert_eq!(models.into_inner(), 17_622);
}

/// Model import through the workspace on copies of a level stream: an
/// exported model imported over itself reuses its materials and keeps the
/// geometry; a recoloured model becomes a new model set (cloned metamodel,
/// meshes, a new material and texture) that resolves; a second new set is
/// planned over the applied first; undo and unapply give the original files
/// back byte for byte.
#[test]
fn model_import_records_replacements_and_new_sets_on_copies() {
    use doctor_toolkit::create::{self, TYPE_METAMODEL};
    use doctor_toolkit::export::gltf as export;
    use doctor_toolkit::game::GameAssets;
    use doctor_toolkit::import::{gltf as import, model};
    use doctor_toolkit::resource::mesh::{self, TYPE_MESH};
    use doctor_toolkit::resource::metamodel::MetaModel;
    use doctor_toolkit::workspace::{ApplyOptions, Workspace};

    let Some(dir) = game_dir() else { return };
    let real = GameInstance::open(&dir).unwrap();
    let source_name = "mp_rooftops/mp_rooftops".to_owned();
    let mut files = vec![
        real.streams()[real.find_stream(&source_name).unwrap()]
            .rel_path
            .clone(),
    ];
    for ancestor in real.stream_ancestors(&source_name).unwrap() {
        if let Some(i) = real.find_stream(&ancestor) {
            files.push(real.streams()[i].rel_path.clone());
        }
    }
    for toc in real.tocs() {
        files.push(toc.rel_path.clone());
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for rel in &files {
        std::fs::create_dir_all(root.join(rel).parent().unwrap()).unwrap();
        std::fs::copy(dir.join(rel), root.join(rel)).unwrap();
    }
    let snapshot = || -> Vec<Vec<u8>> {
        files
            .iter()
            .map(|rel| std::fs::read(root.join(rel)).unwrap())
            .collect()
    };
    let originals = snapshot();
    let game = std::sync::Arc::new(GameInstance::open(root).unwrap());
    let s = game.find_stream(&source_name).unwrap();
    let (index, _) = game.build_index(|_| {});
    let mut assets = GameAssets::new(game.clone(), &index);
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let baseline: std::collections::BTreeSet<_> = create::check_references(&game, s)
        .unwrap()
        .into_iter()
        .map(|u| u.target)
        .collect();

    // A metamodel with meshes in the stream.
    let stream = game.open_stream(s).unwrap();
    let layout = stream.layout();
    let (meta, mesh_index) = (0..layout.resources.len())
        .filter(|&i| layout.resources[i].header.type_id == TYPE_METAMODEL)
        .find_map(|i| {
            let m = MetaModel::parse(&stream.resource_data(i).unwrap()).ok()?;
            let r = m.resources.iter().find(|r| r.type_id.0 == TYPE_MESH)?;
            let ids = r.ids.map(|h| h.0);
            let mesh = layout
                .resources
                .iter()
                .position(|x| x.header.type_id == TYPE_MESH && x.header.ids == ids)?;
            Some((i, mesh))
        })
        .unwrap();
    let (meta_before, meta_ids) = (meta, layout.resources[meta].header.ids);
    let original = mesh::load(&stream, mesh_index).unwrap();
    let glb = export::model_to_glb(&original, &mut assets).unwrap();
    drop(stream);
    let gltf = import::read(&glb, None).unwrap();

    // 1. Over itself: materials reused, same triangles.
    let plan = model::plan(
        &game,
        &ws,
        &index,
        &mut assets,
        s,
        mesh_index,
        &gltf,
        &model::ImportOptions::default(),
    )
    .unwrap();
    assert_eq!(plan.summary.materials_new, 0, "{:?}", plan.summary);
    assert!(plan.summary.added.is_empty(), "{:?}", plan.summary.added);
    let back = plan.preview().unwrap();
    let total = |m: &mesh::Model| -> usize {
        m.records
            .iter()
            .map(|r| m.triangles(r).unwrap().len())
            .sum()
    };
    assert_eq!(total(&back), total(&original));
    plan.commit(&mut ws, &game).unwrap().unwrap();

    // 2. Recoloured, as a new model set pointing nowhere yet.
    let mut recoloured = gltf.clone();
    let red: Vec<u8> = (0..64 * 64).flat_map(|_| [200, 20, 20, 255]).collect();
    let png = doctor_toolkit::resource::pixels::encode_png(64, 64, &red).unwrap();
    let material = recoloured
        .parts
        .first()
        .and_then(|p| p.material)
        .expect("the export has materials");
    recoloured.materials[material].base_color = Some(import::GltfImage {
        name: "red".into(),
        mime_type: "image/png".into(),
        bytes: png,
    });
    let plan = model::plan(
        &game,
        &ws,
        &index,
        &mut assets,
        s,
        meta,
        &recoloured,
        &model::ImportOptions {
            new_name: Some("doctor_corpus".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let summary = plan.summary.clone();
    assert_eq!(summary.materials_new, 1, "{summary:?}");
    assert_eq!(summary.textures_imported, 1, "{summary:?}");
    assert!(
        summary.added.iter().any(|a| a.contains(":metamodel:")),
        "{summary:?}"
    );
    assert!(
        summary.added.iter().any(|a| a.contains(":mesh:")),
        "{summary:?}"
    );
    plan.commit(&mut ws, &game).unwrap().unwrap();
    assert_eq!(ws.operations().len(), 2);

    ws.apply(&game, ApplyOptions::default()).unwrap();
    let after: std::collections::BTreeSet<_> = create::check_references(&game, s)
        .unwrap()
        .into_iter()
        .map(|u| u.target)
        .collect();
    assert!(
        after.is_subset(&baseline),
        "new unresolved: {:?}",
        after.difference(&baseline)
    );
    // The applied stream holds the new set and the meshes read back.
    let stream = game.open_stream(s).unwrap();
    let mut models = 0;
    for (i, r) in stream.layout().resources.iter().enumerate() {
        if r.header.type_id == TYPE_MESH {
            let m = mesh::load(&stream, i).unwrap();
            for record in &m.records {
                m.vertices(record).unwrap();
                m.triangles(record).unwrap();
            }
            models += usize::from(m.name == "doctor_corpus");
        }
    }
    assert!(models >= 1);
    drop(stream);

    // 3. Another new set of the same model, now that the game file holds
    // the applied clones of the first right after their sources: the new
    // resources go before a resource of the original stream (planning used
    // to stop at "… is not a resource of the original …").
    let meta = {
        let stream = game.open_stream(s).unwrap();
        stream
            .layout()
            .resources
            .iter()
            .position(|r| r.header.ids == meta_ids)
            .unwrap()
    };
    assert_ne!(meta, meta_before, "the clones of the first set come first");
    let plan = model::plan(
        &game,
        &ws,
        &index,
        &mut assets,
        s,
        meta,
        &gltf,
        &model::ImportOptions {
            new_name: Some("doctor_corpus2".into()),
            ..Default::default()
        },
    )
    .unwrap();
    plan.commit(&mut ws, &game).unwrap().unwrap();
    ws.apply(&game, ApplyOptions::default()).unwrap();
    let stream = game.open_stream(s).unwrap();
    let names: Vec<String> = (0..stream.layout().resources.len())
        .filter(|&i| stream.layout().resources[i].header.type_id == TYPE_MESH)
        .map(|i| mesh::load(&stream, i).unwrap().name)
        .collect();
    assert!(names.iter().any(|n| n == "doctor_corpus2"), "{names:?}");
    drop(stream);

    // Undo all three, unapply: the original files again.
    ws.undo().unwrap().unwrap();
    ws.undo().unwrap().unwrap();
    ws.undo().unwrap().unwrap();
    ws.apply(&game, ApplyOptions::default()).unwrap();
    ws.unapply(&game, false).unwrap();
    assert!(
        snapshot() == originals,
        "files differ after undo and unapply"
    );
}

/// A model imported in place with a new texture, and imported again (stage
/// of a modder's work: look, change the file, import once more): the clone
/// of the material and its texture go before every mesh of the metamodel;
/// the workspace gives them to a viewer before `apply`; the second import
/// keeps them under their ids with the new pixels instead of adding one more
/// clone, applied or not; undo gives the original files back.
#[test]
fn model_imported_again_keeps_its_material_and_textures() {
    use doctor_toolkit::create::{self, TYPE_MATERIAL, TYPE_METAMODEL};
    use doctor_toolkit::export::gltf::{self as export, AssetSource};
    use doctor_toolkit::game::GameAssets;
    use doctor_toolkit::import::{gltf as import, model};
    use doctor_toolkit::resource::material::TextureRole;
    use doctor_toolkit::resource::mesh::{self, TYPE_MESH};
    use doctor_toolkit::resource::metamodel::MetaModel;
    use doctor_toolkit::workspace::{ApplyOptions, EditedAssets, Workspace};

    let Some(dir) = game_dir() else { return };
    let real = GameInstance::open(&dir).unwrap();
    let source_name = "mp_rooftops/mp_rooftops".to_owned();
    let mut files = vec![
        real.streams()[real.find_stream(&source_name).unwrap()]
            .rel_path
            .clone(),
    ];
    for ancestor in real.stream_ancestors(&source_name).unwrap() {
        if let Some(i) = real.find_stream(&ancestor) {
            files.push(real.streams()[i].rel_path.clone());
        }
    }
    for toc in real.tocs() {
        files.push(toc.rel_path.clone());
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for rel in &files {
        std::fs::create_dir_all(root.join(rel).parent().unwrap()).unwrap();
        std::fs::copy(dir.join(rel), root.join(rel)).unwrap();
    }
    let snapshot = || -> Vec<Vec<u8>> {
        files
            .iter()
            .map(|rel| std::fs::read(root.join(rel)).unwrap())
            .collect()
    };
    let originals = snapshot();
    let game = std::sync::Arc::new(GameInstance::open(root).unwrap());
    let s = game.find_stream(&source_name).unwrap();
    let (index, _) = game.build_index(|_| {});
    let mut assets = GameAssets::new(game.clone(), &index);
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let baseline: std::collections::BTreeSet<_> = create::check_references(&game, s)
        .unwrap()
        .into_iter()
        .map(|u| u.target)
        .collect();

    // A metamodel with meshes in the stream; its ids and theirs stay, the
    // indices do not.
    let stream = game.open_stream(s).unwrap();
    let layout = stream.layout();
    let count = |type_id: u32, stream: &StreamFile| {
        stream
            .layout()
            .resources
            .iter()
            .filter(|r| r.header.type_id == type_id)
            .count()
    };
    let (meta, mesh_ids) = (0..layout.resources.len())
        .filter(|&i| layout.resources[i].header.type_id == TYPE_METAMODEL)
        .find_map(|i| {
            let m = MetaModel::parse(&stream.resource_data(i).unwrap()).ok()?;
            let ids: Vec<[u32; 4]> = m
                .resources
                .iter()
                .filter(|r| r.type_id.0 == TYPE_MESH)
                .map(|r| r.ids.map(|h| h.0))
                .filter(|ids| {
                    layout
                        .resources
                        .iter()
                        .any(|x| x.header.type_id == TYPE_MESH && x.header.ids == *ids)
                })
                .collect();
            (!ids.is_empty()).then_some((i, ids))
        })
        .unwrap();
    let meta_ids = layout.resources[meta].header.ids;
    let position = |stream: &StreamFile, type_id: u32, ids: [u32; 4]| {
        stream
            .layout()
            .resources
            .iter()
            .position(|r| r.header.type_id == type_id && r.header.ids == ids)
            .unwrap()
    };
    let first_mesh = position(&stream, TYPE_MESH, mesh_ids[0]);
    let original = mesh::load(&stream, first_mesh).unwrap();
    let materials_before = count(TYPE_MATERIAL, &stream);
    let resources_before = layout.resources.len();
    let glb = export::model_to_glb(&original, &mut assets).unwrap();
    drop(stream);
    let gltf = import::read(&glb, None).unwrap();
    let coloured = |rgb: [u8; 3]| {
        let mut model = gltf.clone();
        let pixels: Vec<u8> = (0..64 * 64)
            .flat_map(|_| [rgb[0], rgb[1], rgb[2], 255])
            .collect();
        let material = model
            .parts
            .first()
            .and_then(|p| p.material)
            .expect("the export has materials");
        model.materials[material].base_color = Some(import::GltfImage {
            name: "colour".into(),
            mime_type: "image/png".into(),
            bytes: doctor_toolkit::resource::pixels::encode_png(64, 64, &pixels).unwrap(),
        });
        model
    };
    let options = model::ImportOptions::default();

    // 1. Red, recorded only.
    let plan = model::plan(
        &game,
        &ws,
        &index,
        &mut assets,
        s,
        meta,
        &coloured([200, 20, 20]),
        &options,
    )
    .unwrap();
    assert_eq!(plan.summary.materials_new, 1, "{:?}", plan.summary);
    let old: Vec<u32> = original.records.iter().map(|r| r.material_hash).collect();
    let clone = plan
        .preview()
        .unwrap()
        .records
        .iter()
        .map(|r| r.material_hash)
        .find(|h| !old.contains(h))
        .expect("a record with the new material");
    plan.commit(&mut ws, &game).unwrap().unwrap();
    // The game files do not have the clone; the workspace gives it.
    assert!(AssetSource::material(&mut assets, clone).is_none());
    let colour = |ws: &Workspace, assets: &mut GameAssets| -> [u8; 3] {
        let mut edited = EditedAssets::new(ws, assets);
        let material = edited
            .material(clone)
            .expect("the material of the workspace");
        let texture = material.texture(TextureRole::Diffuse).unwrap();
        assert_ne!(edited.texture_version(texture), 0);
        let (w, h, rgba) = edited.texture(texture).unwrap().rgba(0).unwrap();
        assert_eq!((w, h), (64, 64));
        [rgba[0], rgba[1], rgba[2]]
    };
    let [r, _, b] = colour(&ws, &mut assets);
    assert!(r > 150 && b < 80, "red expected, got {r} .. {b}");

    // 2. Blue over it, still not applied: the same material and texture.
    let plan = model::plan(
        &game,
        &ws,
        &index,
        &mut assets,
        s,
        meta,
        &coloured([20, 20, 200]),
        &options,
    )
    .unwrap();
    let summary = plan.summary.clone();
    assert_eq!(summary.materials_new, 0, "{summary:?}");
    assert_eq!(summary.materials_updated, 1, "{summary:?}");
    assert!(summary.added.is_empty(), "{summary:?}");
    assert!(
        plan.preview()
            .unwrap()
            .records
            .iter()
            .any(|r| r.material_hash == clone),
        "the records keep the material"
    );
    // A preview of the plan draws with the kept material and the new pixels.
    {
        let mut preview = plan.assets(&mut assets);
        let material = preview.material(clone).expect("the material in the plan");
        let texture = material.texture(TextureRole::Diffuse).unwrap();
        assert_ne!(preview.texture_version(texture), 0);
        let (_, _, rgba) = preview.texture(texture).unwrap().rgba(0).unwrap();
        assert!(
            rgba[2] > 150 && rgba[0] < 80,
            "blue expected in the preview"
        );
    }
    plan.commit(&mut ws, &game).unwrap().unwrap();
    let [r, _, b] = colour(&ws, &mut assets);
    assert!(b > 150 && r < 80, "blue expected, got {r} .. {b}");

    // Applied: one more material, before every mesh of the metamodel.
    ws.apply(&game, ApplyOptions::default()).unwrap();
    let stream = game.open_stream(s).unwrap();
    assert_eq!(count(TYPE_MATERIAL, &stream), materials_before + 1);
    assert_eq!(stream.layout().resources.len(), resources_before + 3);
    let material_at = stream
        .layout()
        .resources
        .iter()
        .enumerate()
        .position(|(i, r)| {
            r.header.type_id == TYPE_MATERIAL
                && stream
                    .resource_data(i)
                    .is_ok_and(|d| d.get(8..12) == Some(&clone.to_le_bytes()[..]))
        })
        .expect("the clone in the stream");
    for ids in &mesh_ids {
        assert!(
            material_at < position(&stream, TYPE_MESH, *ids),
            "the material loads before mesh {ids:08x?}"
        );
    }
    let meta = position(&stream, TYPE_METAMODEL, meta_ids);
    drop(stream);
    let after: std::collections::BTreeSet<_> = create::check_references(&game, s)
        .unwrap()
        .into_iter()
        .map(|u| u.target)
        .collect();
    assert!(
        after.is_subset(&baseline),
        "new unresolved: {:?}",
        after.difference(&baseline)
    );

    // 3. Green over the applied one: again nothing new.
    let plan = model::plan(
        &game,
        &ws,
        &index,
        &mut assets,
        s,
        meta,
        &coloured([20, 200, 20]),
        &options,
    )
    .unwrap();
    assert_eq!(plan.summary.materials_updated, 1, "{:?}", plan.summary);
    assert!(plan.summary.added.is_empty(), "{:?}", plan.summary);
    plan.commit(&mut ws, &game).unwrap().unwrap();
    ws.apply(&game, ApplyOptions::default()).unwrap();
    let stream = game.open_stream(s).unwrap();
    assert_eq!(count(TYPE_MATERIAL, &stream), materials_before + 1);
    assert_eq!(stream.layout().resources.len(), resources_before + 3);
    drop(stream);

    ws.undo().unwrap().unwrap();
    ws.undo().unwrap().unwrap();
    ws.undo().unwrap().unwrap();
    ws.apply(&game, ApplyOptions::default()).unwrap();
    ws.unapply(&game, false).unwrap();
    assert!(
        snapshot() == originals,
        "files differ after undo and unapply"
    );
}

#[test]
fn every_material_parses_names_its_shaders_and_decodes_its_parameters() {
    use doctor_toolkit::hash::{crc32, sdbm};
    use doctor_toolkit::resource::material::Material;
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let material = sdbm(b"material");
    let failures = Failures::default();
    let count = std::sync::atomic::AtomicUsize::new(0);
    game.streams().par_iter().for_each(|file| {
        let stream = StreamFile::open(&file.path).unwrap();
        for (index, resource) in stream.layout().resources.iter().enumerate() {
            if resource.header.type_id != material {
                continue;
            }
            match Material::parse(&stream.resource_data(index).unwrap()) {
                Ok(m) if m.name_hash == crc32(m.name.to_ascii_lowercase().as_bytes()) => {
                    use doctor_toolkit::resource::material::ParamValue;
                    use doctor_toolkit::resource::shading::{Shading, shader};
                    // Every parameter has a known type and every shader a name.
                    if let Some(p) = m
                        .params
                        .iter()
                        .find(|p| matches!(p.value, ParamValue::Unknown(_)))
                    {
                        failures.push(format!(
                            "{} #{index}: `{}` has parameter `{}` of an unknown type",
                            file.rel_path, m.name, p.name
                        ));
                    }
                    for (hash, _, _) in m.shaders {
                        if hash != 0 && shader(hash).is_none() {
                            failures.push(format!(
                                "{} #{index}: `{}` uses unknown shader {hash:08x}",
                                file.rel_path, m.name
                            ));
                        }
                    }
                    let shading = Shading::from_material(&m);
                    if shading.shader.is_none() && m.shaders[1].0 != 0 {
                        failures.push(format!("{} #{index}: no lit shader", file.rel_path));
                    }
                }
                Ok(m) => failures.push(format!(
                    "{} #{index}: hash of `{}` differs",
                    file.rel_path, m.name
                )),
                Err(error) => failures.push(format!("{} #{index}: {error}", file.rel_path)),
            }
            count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    });
    failures.assert_empty();
    assert_eq!(count.into_inner(), 77_643);
}

/// Workspace keys (stream, type id, the four `SHDR` ids) must identify every
/// resource of the game: no two resources of a stream share a key, and the
/// text form parses back to the same key.
#[test]
fn resource_keys_are_unique_and_round_trip() {
    use doctor_toolkit::workspace::ResourceKey;
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let names: std::collections::HashSet<_> =
        game.streams().iter().map(|s| s.stream_name()).collect();
    assert_eq!(names.len(), game.streams().len(), "stream names are unique");
    let failures = Failures::default();
    let total = std::sync::atomic::AtomicUsize::new(0);
    game.streams().par_iter().for_each(|file| {
        let stream = StreamFile::open(&file.path).unwrap();
        let name = file.stream_name();
        let mut seen = std::collections::HashMap::new();
        for (index, resource) in stream.layout().resources.iter().enumerate() {
            let key = ResourceKey::new(&name, &resource.header);
            match key.to_string().parse::<ResourceKey>() {
                Ok(parsed) if parsed == key => {}
                _ => failures.push(format!("{key}: text form does not round-trip")),
            }
            if let Some(first) = seen.insert(key.clone(), index) {
                failures.push(format!("{key}: resources {first} and {index}"));
            }
            if key.find_in(stream.layout()) != Some(index) {
                failures.push(format!("{key}: lookup does not find resource {index}"));
            }
        }
        total.fetch_add(seen.len(), std::sync::atomic::Ordering::Relaxed);
    });
    failures.assert_empty();
    assert_eq!(total.into_inner(), 212_544);
}

/// A workspace on a copy of real streams: import a texture and edit strings,
/// apply, undo, apply again and unapply. The game directory is only read.
#[test]
fn workspace_round_trip_on_copies_of_real_streams() {
    use doctor_toolkit::resource::{lh2::StringTable, texture};
    use doctor_toolkit::workspace::{ApplyOptions, ChangeState, StreamAction, Workspace};

    let Some(dir) = game_dir() else { return };
    const FILES: [&str; 4] = [
        "gf2/frontend.str",
        "gf2/frontend.toc",
        "gf2/frontend_global.toc",
        "gf2/text/dd7d7050.str",
    ];
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let originals: Vec<Vec<u8>> = FILES
        .iter()
        .map(|rel| std::fs::read(dir.join(rel)).unwrap())
        .collect();
    for (rel, data) in FILES.iter().zip(&originals) {
        std::fs::create_dir_all(root.join(rel).parent().unwrap()).unwrap();
        std::fs::write(root.join(rel), data).unwrap();
    }
    let read_all = || -> Vec<Vec<u8>> {
        FILES
            .iter()
            .map(|rel| std::fs::read(root.join(rel)).unwrap())
            .collect()
    };
    let game = GameInstance::open(root).unwrap();
    let frontend = game.find_stream("gf2/frontend").unwrap();
    let text = game.find_stream("gf2/text/dd7d7050").unwrap();
    let mut ws = Workspace::for_game(&game, None).unwrap();

    // A texture: its own image with the red channel inverted.
    let stream = game.open_stream(frontend).unwrap();
    let before: Vec<Vec<u8>> = (0..stream.layout().resources.len())
        .map(|i| stream.resource_data(i).unwrap())
        .collect();
    let pair = texture::pairs(stream.layout())
        .into_iter()
        .find(|p| {
            texture::Texture::load(stream.data(), stream.layout(), *p)
                .is_ok_and(|t| t.header.layer_count() == 1 && t.header.width >= 64)
        })
        .unwrap();
    let original = texture::Texture::load(stream.data(), stream.layout(), pair).unwrap();
    drop(stream);
    let (w, h, mut rgba) = original.rgba(0).unwrap();
    for px in rgba.chunks_mut(4) {
        px[0] = 255 - px[0];
    }
    let png = doctor_toolkit::resource::pixels::encode_png(w, h, &rgba).unwrap();
    let imported = ws
        .import_texture(&game, frontend, pair, &png, true)
        .unwrap();

    // A string of the main text table.
    let stream = game.open_stream(text).unwrap();
    let lh2 = doctor_toolkit::hash::sdbm(b"lh2");
    let index = stream
        .layout()
        .resources
        .iter()
        .position(|r| r.header.type_id == lh2)
        .unwrap();
    let mut table = StringTable::parse(&stream.resource_data(index).unwrap()).unwrap();
    drop(stream);
    table.strings[0][0] = "doctor_toolkit workspace test".into();
    assert!(ws.set_string_table(&game, text, index, &table).unwrap());
    assert_eq!(read_all(), originals, "recording does not touch the game");
    // The texture header is only replaced when it changes; this image keeps its shape.
    let changes = ws.len();
    assert_eq!(changes, 2);

    let report = ws.apply(&game, ApplyOptions::default()).unwrap();
    assert_eq!(report.streams.len(), 2);
    let check_tocs = || {
        for (toc_index, toc_file) in game.tocs().iter().enumerate() {
            let table = game.load_toc(toc_index).unwrap();
            for file in game.streams() {
                if let Some(i) = table.find_file(&file.stream_name()) {
                    let data = std::fs::read(&file.path).unwrap();
                    let size = StreamLayout::parse(&data).unwrap().total_data_size();
                    assert_eq!(
                        table.files[i].data_size as u64, size,
                        "{} in {}",
                        file.rel_path, toc_file.rel_path
                    );
                }
            }
        }
    };
    check_tocs();
    let stream = game.open_stream(frontend).unwrap();
    for (i, expected) in before.iter().enumerate() {
        if i != pair.header && i != pair.data {
            assert_eq!(&stream.resource_data(i).unwrap(), expected, "resource {i}");
        }
    }
    let loaded = texture::Texture::load(stream.data(), stream.layout(), pair).unwrap();
    assert_eq!(loaded.data, imported.data);
    drop(stream);
    let stream = game.open_stream(text).unwrap();
    assert_eq!(
        StringTable::parse(&stream.resource_data(index).unwrap()).unwrap(),
        table
    );
    drop(stream);
    let status = ws.status(&game).unwrap();
    assert!(status.is_up_to_date());
    assert_eq!(status.count(ChangeState::Applied), changes);

    // Undo the strings: the text stream goes back to its original bytes.
    assert!(ws.undo().unwrap().is_some());
    let report = ws.apply(&game, ApplyOptions::default()).unwrap();
    assert_eq!(
        report.streams,
        [("gf2/text/dd7d7050".to_owned(), StreamAction::Restored)]
    );
    assert_eq!(read_all()[3], originals[3]);
    check_tocs();

    // The workspace survives a restart; unapply restores every file.
    let mut ws = Workspace::open(ws.dir()).unwrap();
    assert_eq!(ws.redo_label(), Some("Edit strings in gf2/text/dd7d7050"));
    ws.unapply(&game, false).unwrap();
    assert_eq!(read_all(), originals);
    assert_eq!(
        ws.status(&game).unwrap().count(ChangeState::Pending),
        changes - 1
    );
}

/// Parses every resource of type `tag` with `check`; returns the number of resources.
fn for_each_resource(tag: &str, check: impl Fn(&str, &[u8]) -> Result<(), String> + Sync) -> usize {
    let Some(dir) = game_dir() else { return 0 };
    let game = GameInstance::open(&dir).unwrap();
    let type_id = doctor_toolkit::hash::sdbm(tag.as_bytes());
    let failures = Failures::default();
    let count = std::sync::atomic::AtomicUsize::new(0);
    game.streams().par_iter().for_each(|file| {
        let stream = StreamFile::open(&file.path).unwrap();
        for (index, resource) in stream.layout().resources.iter().enumerate() {
            if resource.header.type_id != type_id {
                continue;
            }
            let at = format!("{} #{index}", file.rel_path);
            match stream.resource_data(index) {
                Ok(data) => {
                    if let Err(error) = check(&at, &data) {
                        failures.push(format!("{at}: {error}"));
                    }
                }
                Err(error) => failures.push(format!("{at}: {error}")),
            }
            count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    });
    failures.assert_empty();
    count.into_inner()
}

/// `docs/formats/wgi.md`: every world grid is written back byte for byte,
/// its cells and stream table name streams of the tables of contents, and a
/// grid with a new cell still parses.
#[test]
fn every_world_grid_round_trips() {
    use doctor_toolkit::resource::wgi::{self, WorldGrid};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let Some(dir) = game_dir() else { return };
    let hashes = GameInstance::open(&dir).unwrap().toc_hashes().unwrap();
    let (cells, streams, named) = (
        AtomicUsize::new(0),
        AtomicUsize::new(0),
        AtomicUsize::new(0),
    );
    let count = for_each_resource("wgi", |_, data| {
        let grid = WorldGrid::parse(data).map_err(|e| e.to_string())?;
        if grid.to_bytes().map_err(|e| e.to_string())? != data {
            return Err("does not round-trip".into());
        }
        for cell in &grid.cells {
            let hash = cell.stream_hash.0;
            if doctor_toolkit::toc::guid_hash(&cell.guid.0) != hash {
                return Err(format!(
                    "cell {hash:08x}: the GUID does not fold to the hash"
                ));
            }
            if !hashes.contains(&hash) {
                return Err(format!("cell {hash:08x} is not in a table of contents"));
            }
            if grid.find_stream(hash).is_some() {
                return Err(format!("cell {hash:08x} is also in the stream table"));
            }
            if cell.reserved_64 != 0 || cell.min_distance != 0.0 || cell.max_distance != 0.0 {
                return Err(format!("cell {hash:08x}: unexpected distance fields"));
            }
            named.fetch_add(usize::from(cell.name_hash.0 != 0), Ordering::Relaxed);
        }
        for stream in &grid.streams {
            if !hashes.contains(&stream.hash()) {
                return Err(format!(
                    "stream {:08x} is not in a table of contents",
                    stream.hash()
                ));
            }
        }
        cells.fetch_add(grid.cells.len(), Ordering::Relaxed);
        streams.fetch_add(grid.streams.len(), Ordering::Relaxed);
        // A new cell that needs the first stream of the table.
        let mut edited = grid.clone();
        let mut guid = [0x84u8; 16];
        guid[8..].copy_from_slice(b"XDOCTORT");
        let index = edited
            .add_cell(&wgi::NewCell {
                guid,
                box_min: [0.0, 0.0, 0.0],
                box_max: [40.0, 10.0, 40.0],
                kind: wgi::KIND_WORLD,
                flags: 0,
                lod_mask: grid.main_lod_mask().unwrap_or(wgi::lod::HIGH),
                name: None,
                needs: grid.streams.first().map(|s| s.hash()),
            })
            .map_err(|e| e.to_string())?;
        let bytes = edited.to_bytes().map_err(|e| e.to_string())?;
        let mut again = WorldGrid::parse(&bytes).map_err(|e| e.to_string())?;
        if again != edited {
            return Err("the edited grid does not parse back".into());
        }
        again.remove_cell(index).map_err(|e| e.to_string())?;
        again.pool.truncate(grid.pool.len());
        if again.to_bytes().map_err(|e| e.to_string())? != data {
            return Err("removing the new cell does not give the original".into());
        }
        Ok(())
    });
    eprintln!(
        "world grids: {count}, cells: {}, named: {}, stream entries: {}",
        cells.load(Ordering::Relaxed),
        named.load(Ordering::Relaxed),
        streams.load(Ordering::Relaxed)
    );
    assert_eq!(count, 28);
    assert_eq!(cells.into_inner(), 1483);
    assert_eq!(named.into_inner(), 196);
    assert_eq!(streams.into_inner(), 5254);
}

/// A new stream joins the world grid of every city (recorded in a workspace
/// outside the game; nothing is written): the cell lands in the grid of the
/// high level of detail, needs the `local` folder of the city, and removing
/// it gives the original resource back.
#[test]
fn a_new_stream_joins_the_world_grid() {
    use doctor_toolkit::level::world_grid::{self, ZoneCell};
    use doctor_toolkit::resource::wgi;
    use doctor_toolkit::workspace::Workspace;
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    for city in ["cuba", "miami", "queens"] {
        let level = format!("gf2/{city}");
        let parent = format!("gf2/{city}/local");
        let name = format!("gf2/doctor/grid_{city}");
        let mut ws = Workspace::for_game(&game, Some(&tmp.path().join(city))).unwrap();
        let before = world_grid::level_grids(&game, &ws, &level).unwrap();
        assert_eq!(before.len(), 4, "{level}");
        let stream =
            doctor_toolkit::create::new_stream(&game, &ws, &name, &parent, false, &[], None)
                .unwrap();
        let hash = stream.hash;
        ws.add_stream(&game, stream).unwrap();
        let cell = ZoneCell {
            stream: name.clone(),
            guid: None,
            box_min: [100.0, 0.0, 100.0],
            box_max: [140.0, 20.0, 160.0],
            lod_mask: wgi::lod::HIGH,
            kind: wgi::KIND_WORLD,
            flags: 0,
            name: Some("Doctor_Grid".into()),
        };
        let added = world_grid::add_zone_cell(&game, &mut ws, &level, &cell).unwrap();
        assert_eq!(added.hash, hash);
        assert_eq!(added.needs.as_deref(), Some(parent.as_str()), "{level}");
        // Twice is refused.
        assert!(world_grid::add_zone_cell(&game, &mut ws, &level, &cell).is_err());
        let after = world_grid::level_grids(&game, &ws, &level).unwrap();
        let grid = after.iter().find(|g| g.resource == added.resource).unwrap();
        assert_eq!(grid.grid.main_lod_mask(), Some(wgi::lod::HIGH));
        let new = &grid.grid.cells[added.cell];
        assert_eq!(new.stream_hash.0, hash);
        assert_eq!(new.centre, [120.0, 10.0, 130.0]);
        assert_eq!(new.name_hash.0, doctor_toolkit::hash::sdbm(b"doctor_grid"));
        let needs = grid.grid.list(&new.needs);
        assert_eq!(needs.len(), 1);
        assert_eq!(
            Some(grid.grid.streams[needs[0] as usize].hash()),
            game.stream_hash(&parent).unwrap()
        );
        // The other grids are untouched; removing the cell restores this one.
        for (a, b) in before.iter().zip(&after) {
            assert_eq!(a.grid == b.grid, a.resource != added.resource);
        }
        assert!(world_grid::remove_zone_cell(&game, &mut ws, &level, hash).unwrap());
        assert!(!world_grid::remove_zone_cell(&game, &mut ws, &level, hash).unwrap());
        let restored = world_grid::level_grids(&game, &ws, &level).unwrap();
        for (a, b) in before.iter().zip(&restored) {
            assert_eq!(a.grid, b.grid);
        }
    }
}

/// `docs/formats/simgroup.md` §1: the group hash of a SimGroup is the
/// table-of-contents hash of its stream (0 in the root `*_global` streams).
#[test]
fn simgroup_group_hash_is_the_stream_hash() {
    use doctor_toolkit::resource::simgroup::SimGroup;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let mut by_name = std::collections::HashMap::new();
    for index in 0..game.tocs().len() {
        let toc = game.load_toc(index).unwrap();
        for name in &toc.names {
            by_name.insert(
                name.name_lossy().replace('\\', "/").to_ascii_lowercase(),
                name.hash,
            );
        }
    }
    let (equal, zero) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let failures = Failures::default();
    game.streams().par_iter().for_each(|file| {
        let stream = StreamFile::open(&file.path).unwrap();
        let name = file.stream_name().to_ascii_lowercase();
        for (index, resource) in stream.layout().resources.iter().enumerate() {
            if resource.header.type_id != doctor_toolkit::level::TYPE_SIMGROUP {
                continue;
            }
            let group = SimGroup::parse(&stream.resource_data(index).unwrap()).unwrap();
            if group.group_hash == 0 {
                zero.fetch_add(1, Ordering::Relaxed);
                if !name.ends_with("_global") {
                    failures.push(format!("{name}: group hash 0 outside a root stream"));
                }
            } else if by_name.get(&name) == Some(&group.group_hash) {
                equal.fetch_add(1, Ordering::Relaxed);
            } else {
                failures.push(format!("{name}: group hash {:08x}", group.group_hash));
            }
        }
    });
    failures.assert_empty();
    eprintln!(
        "group hashes: {} equal, {} zero",
        equal.load(Ordering::Relaxed),
        zero.load(Ordering::Relaxed)
    );
    assert_eq!(equal.into_inner(), 1999);
    assert_eq!(zero.into_inner(), 7);
}

#[test]
fn every_rig_parses() {
    use doctor_toolkit::resource::rcb::Rig;
    let bones = std::sync::atomic::AtomicUsize::new(0);
    let named = std::sync::atomic::AtomicUsize::new(0);
    let count = for_each_resource("rcb", |_, data| {
        let rig = Rig::parse(data).map_err(|e| e.to_string())?;
        for bone in rig.skeletons.iter().flat_map(|s| &s.bones) {
            bones.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if bone.name.is_some() {
                named.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        Ok(())
    });
    if game_dir().is_some() {
        eprintln!(
            "rigs: {count}, bones: {}, named: {}",
            bones.into_inner(),
            named.into_inner()
        );
        assert_eq!(count, 122);
    }
}

#[test]
fn every_skinned_mesh_finds_its_skeleton() {
    use doctor_toolkit::resource::mesh::{self, TYPE_MESH};
    use doctor_toolkit::resource::rcb::{Rig, TYPE_RCB};
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let rigs: Vec<Rig> = game
        .streams()
        .par_iter()
        .flat_map_iter(|file| {
            let stream = StreamFile::open(&file.path).unwrap();
            let layout = stream.layout();
            (0..layout.resources.len())
                .filter(|&i| layout.resources[i].header.type_id == TYPE_RCB)
                .map(|i| Rig::parse(&stream.resource_data(i).unwrap()).unwrap())
                .collect::<Vec<_>>()
        })
        .collect();
    let with_bones = std::sync::atomic::AtomicUsize::new(0);
    let missing = Mutex::new(std::collections::BTreeMap::<String, usize>::new());
    game.streams().par_iter().for_each(|file| {
        let stream = StreamFile::open(&file.path).unwrap();
        for (index, resource) in stream.layout().resources.iter().enumerate() {
            if resource.header.type_id != TYPE_MESH {
                continue;
            }
            let model = mesh::load(&stream, index).unwrap();
            if model.bones.is_empty() {
                continue;
            }
            with_bones.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // Weapons and props have one-bone tables, and cloth_rag a table of
            // unknown bones, without a rig.
            let rigged = model.bones.len() > 1
                && model.bones.iter().any(|&(hash, _)| {
                    rigs.iter()
                        .flat_map(|r| &r.skeletons)
                        .any(|s| s.bones.iter().any(|b| b.hash == hash))
                });
            if rigged
                && !rigs
                    .iter()
                    .any(|rig| rig.skeleton_for(&model.bones).is_some())
            {
                *missing
                    .lock()
                    .unwrap()
                    .entry(file.rel_path.clone())
                    .or_default() += 1;
            }
        }
    });
    let missing = missing.into_inner().unwrap();
    eprintln!("meshes with bones: {}", with_bones.into_inner());
    assert!(
        missing.is_empty(),
        "meshes with several bones but no skeleton: {missing:?}"
    );
}

#[test]
fn every_font_parses_and_finds_its_atlas() {
    use doctor_toolkit::resource::ffn::{Font, TYPE_FFN};
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let mut fonts = 0;
    for file in game.streams() {
        let stream = StreamFile::open(&file.path).unwrap();
        for (index, resource) in stream.layout().resources.iter().enumerate() {
            if resource.header.type_id != TYPE_FFN {
                continue;
            }
            let font = Font::parse(&stream.resource_data(index).unwrap())
                .unwrap_or_else(|e| panic!("{} #{index}: {e}", file.rel_path));
            let atlas = font.find_atlas(stream.data(), stream.layout());
            let atlas = atlas.unwrap_or_else(|| panic!("{}: no atlas", font.name));
            for g in &font.glyphs {
                assert!(
                    g.rect[2] <= atlas.header.width as f32
                        && g.rect[3] <= atlas.header.height as f32
                );
            }
            fonts += 1;
        }
    }
    assert_eq!(fonts, 6);
}

#[test]
fn every_lua_chunk_parses() {
    use doctor_toolkit::resource::lua::LuaChunk;
    let count = for_each_resource("lua", |_, data| {
        LuaChunk::parse(data).map(|_| ()).map_err(|e| e.to_string())
    });
    if game_dir().is_some() {
        assert_eq!(count, 30);
    }
}

/// Movie subtitle timings: groups cover the entries in order, entries sorted by start.
#[test]
fn every_tob_parses() {
    use doctor_toolkit::resource::tob::Tob;
    let count = for_each_resource("tob", |_, data| {
        let tob = Tob::parse(data).map_err(|e| e.to_string())?;
        if tob.version != 1 || tob.groups.iter().any(|g| g.name.is_none()) {
            return Err(format!("version {} or unknown group", tob.version));
        }
        for g in &tob.groups {
            if g.entries.windows(2).any(|w| w[0].start_ms > w[1].start_ms) {
                return Err("entries out of order".into());
            }
        }
        Ok(())
    });
    if game_dir().is_some() {
        assert_eq!(count, 15);
    }
}

/// Every script: the chunk writes back byte for byte, and its decompiled
/// source compiles to the same instructions (most to the same bytes).
#[test]
fn every_lua_script_decompiles_and_compiles_back() {
    use doctor_toolkit::resource::lua::{Chunk, Comparison, round_trip};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let identical = AtomicUsize::new(0);
    let count = for_each_resource("lua", |_, data| {
        let chunk = Chunk::parse(data).map_err(|e| e.to_string())?;
        if chunk.to_bytes() != data {
            return Err("the chunk does not write back identically".into());
        }
        let rt = round_trip(&chunk);
        if !rt.decompiled.errors.is_empty() {
            return Err(rt.decompiled.errors.join("; "));
        }
        match rt.comparison {
            Comparison::Identical => {
                identical.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Comparison::SameCode => Ok(()),
            Comparison::Different(why) => Err(why),
        }
    });
    if game_dir().is_some() {
        let identical = identical.into_inner();
        eprintln!("{count} scripts: {identical} recompile byte-identical, all to the same code");
        assert_eq!(count, 30);
        assert!(
            identical >= 27,
            "only {identical} scripts recompile byte-identical"
        );
    }
}

#[test]
fn every_sound_bank_parses_and_decodes() {
    use doctor_toolkit::resource::audio::parse_sbk;
    let sounds = std::sync::atomic::AtomicUsize::new(0);
    let count = for_each_resource("sbk", |_, data| {
        for sound in parse_sbk(data).map_err(|e| e.to_string())? {
            let pcm = sound.sample.decode().map_err(|e| e.to_string())?;
            let h = sound.sample.header;
            if pcm.len() != h.sample_count as usize * h.channels as usize {
                return Err(format!(
                    "sound {} decodes to {} samples",
                    sound.index,
                    pcm.len()
                ));
            }
            sounds.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(())
    });
    if game_dir().is_some() {
        eprintln!("sound banks: {count}, sounds: {}", sounds.into_inner());
        assert_eq!(count, 8);
    }
}

/// Stage 41: the whole `sbk` metadata. Every bank parses, writes back byte
/// for byte, has no broken reference; every emitter resolves its
/// instrument; adding a sound to an emitter and a new emitter gives a bank
/// that parses again, validates and finds the new emitter by name.
#[test]
fn every_sound_bank_round_trips_and_takes_new_sounds() {
    use doctor_toolkit::resource::audio::parse_sbk;
    use doctor_toolkit::resource::sbk::{SoundBank, cmd, name_hash};
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    let emitters = AtomicUsize::new(0);
    let named = AtomicUsize::new(0);
    let tones = AtomicUsize::new(0);
    let variations = AtomicUsize::new(0);
    let commands = AtomicUsize::new(0);
    let count = for_each_resource("sbk", |_, data| {
        let bank = SoundBank::parse(data).map_err(|e| e.to_string())?;
        if bank.to_bytes() != data {
            return Err("the rewritten bank differs".into());
        }
        let problems = bank.validate();
        if !problems.is_empty() {
            return Err(problems.join("; "));
        }
        for i in 0..bank.emitters.len() {
            if bank.instrument_of(i).is_none() {
                return Err(format!("emitter {i} does not resolve its instrument"));
            }
            named.fetch_add(usize::from(bank.emitter_name(i).is_some()), Relaxed);
        }
        emitters.fetch_add(bank.emitters.len(), Relaxed);
        tones.fetch_add(
            bank.instruments.iter().map(|i| i.tones.len()).sum(),
            Relaxed,
        );
        let v = bank.emitters.iter().flat_map(|e| &e.track.variations);
        variations.fetch_add(v.clone().count(), Relaxed);
        commands.fetch_add(
            v.flat_map(|v| &v.commands)
                .filter(|c| c.id != cmd::NONE)
                .count(),
            Relaxed,
        );

        // Edits: a new variation on the first emitter (its own first sample
        // again), a pitch, a new emitter with two sounds.
        let mut edited = bank.clone();
        let first = edited.instrument_of(0).unwrap();
        let sample = edited.instruments[first].tones[0].sample as usize;
        let bytes = edited.samples[sample].bytes.clone();
        edited
            .add_variation(0, bytes.clone())
            .map_err(|e| e.to_string())?;
        edited.set_pitch(0, 1.25).map_err(|e| e.to_string())?;
        let new = edited
            .add_emitter("doctor_toolkit_test", 0, vec![bytes.clone(), bytes])
            .map_err(|e| e.to_string())?;
        let out = edited.to_bytes();
        let again = SoundBank::parse(&out).map_err(|e| format!("edited: {e}"))?;
        if again != edited {
            return Err("the edited bank parses differently".into());
        }
        let problems = again.validate();
        if !problems.is_empty() {
            return Err(format!("edited: {}", problems.join("; ")));
        }
        if again.find_emitter("doctor_toolkit_test.emx") != Some(new)
            || again.emitters[new].template_id != name_hash("doctor_toolkit_test.emx")
            || again.instrument_of(new).is_none()
        {
            return Err("the new emitter is not found".into());
        }
        let sounds = parse_sbk(&out).map_err(|e| e.to_string())?;
        if sounds.len() != bank.samples.len() + 3 {
            return Err(format!("{} sounds after adding 3", sounds.len()));
        }
        Ok(())
    });
    if game_dir().is_some() {
        eprintln!(
            "sound banks: {count}, emitters {} (named {}), tones {}, variations {}, commands {}",
            emitters.load(Relaxed),
            named.load(Relaxed),
            tones.load(Relaxed),
            variations.load(Relaxed),
            commands.load(Relaxed)
        );
        assert_eq!(count, 8);
        assert_eq!(emitters.into_inner(), 942);
    }
}

#[test]
fn every_archived_stream_sound_parses() {
    use doctor_toolkit::container::big::{BigArchive, find_archives};
    use doctor_toolkit::resource::audio::{CODEC_XAS1, Sample};
    let Some(dir) = game_dir() else { return };
    let failures = Failures::default();
    let mut total = 0;
    for path in find_archives(&dir).unwrap() {
        let archive = BigArchive::open(&path).unwrap();
        total += archive.entries.len();
        archive
            .entries
            .par_iter()
            .enumerate()
            .for_each(|(i, entry)| {
                let data = archive.read(entry).unwrap();
                match Sample::parse_snu(&data) {
                    // Decode a sample of the files fully; parsing walks every block.
                    Ok(sample) if sample.header.codec == CODEC_XAS1 => {
                        if i % 200 == 0
                            && let Err(e) = sample.decode()
                        {
                            failures.push(format!("{}: {e}", entry.name));
                        }
                    }
                    Ok(sample) => {
                        failures.push(format!("{}: codec {}", entry.name, sample.header.codec))
                    }
                    Err(e) => failures.push(format!("{}: {e}", entry.name)),
                }
            });
    }
    failures.assert_empty();
    eprintln!("archived sounds: {total}");
    assert_eq!(total, 24_913);
}

#[test]
fn every_havok_packfile_parses_and_holds_physics_only() {
    use doctor_toolkit::resource::hkx::Packfile;
    use doctor_toolkit::resource::hkx::graph::Graph;
    use doctor_toolkit::resource::hkx::mopp::{self, Primitive};
    use doctor_toolkit::resource::hkx::physics::{Physics, ShapeKind};
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    let objects = AtomicUsize::new(0);
    let typed = AtomicUsize::new(0);
    let identical = AtomicUsize::new(0);
    let triangles = AtomicUsize::new(0);
    let not_in_code = AtomicUsize::new(0);
    let game_misses = AtomicUsize::new(0);
    let count = for_each_resource("hkx", |_, data| {
        let p = Packfile::parse(data).map_err(|e| e.to_string())?;
        if p.contents_version != "Havok-5.5.0-r1" || p.root_class != "hkRootLevelContainer" {
            return Err(format!("{} {}", p.contents_version, p.root_class));
        }
        if let Some(o) = p.objects.iter().find(|o| o.class.starts_with("hka")) {
            return Err(format!("animation class {}", o.class));
        }
        objects.fetch_add(p.objects.len(), Relaxed);
        if p.to_bytes() != data {
            return Err("packfile written back differs".into());
        }
        // The data section laid out again from its object graph.
        let mut copy = p.clone();
        Graph::from_packfile(&p)
            .and_then(|g| g.store(&mut copy))
            .map_err(|e| e.to_string())?;
        if copy.to_bytes() != data {
            return Err("graph laid out again differs".into());
        }
        let physics = match Physics::from_packfile(&p) {
            Ok(physics) => physics,
            // One packfile has ragdoll constraints.
            Err(e) if e.to_string().contains("constraints") => return Ok(()),
            Err(e) => return Err(e.to_string()),
        };
        typed.fetch_add(1, Relaxed);
        if physics.other_contents.is_empty() {
            let mut copy = p.clone();
            physics.store(&mut copy).map_err(|e| e.to_string())?;
            if copy.to_bytes() != data {
                return Err("typed physics written back differs".into());
            }
            identical.fetch_add(1, Relaxed);
        }
        // MOPP: the game's code reaches the triangles, the query finds a
        // triangle from its own box, and new code built over the same
        // triangles finds them too.
        for body in physics.bodies() {
            let ShapeKind::Mopp { code, child } = &body.shape.kind else {
                continue;
            };
            let mut prims = Vec::new();
            let mut push = |key: u32, c: [[f32; 3]; 3]| {
                let min = [0, 1, 2].map(|a| c[0][a].min(c[1][a]).min(c[2][a]));
                let max = [0, 1, 2].map(|a| c[0][a].max(c[1][a]).max(c[2][a]));
                prims.push(Primitive { key, min, max });
            };
            match &child.kind {
                ShapeKind::SimpleMesh {
                    vertices,
                    triangles,
                    ..
                } => {
                    for (i, t) in triangles.iter().enumerate() {
                        let corners = t.map(|k| {
                            let v = vertices[k as usize];
                            [v[0], v[1], v[2]]
                        });
                        push(i as u32, corners);
                    }
                }
                ShapeKind::ExtendedMesh(m) => {
                    let bits = 32 - m.subpart_index_bits as u32;
                    for (s, part) in m.triangle_subparts.iter().enumerate() {
                        for (i, t) in part.indices.iter().enumerate() {
                            let corners = [t[0], t[1], t[2]].map(|k| {
                                let v = part.vertices[k as usize];
                                [0, 1, 2].map(|a| v[a] * m.scaling[a])
                            });
                            push(((s as u32) << bits) | i as u32, corners);
                        }
                    }
                }
                _ => continue,
            }
            let keys = code.keys().map_err(|e| e.to_string())?;
            triangles.fetch_add(prims.len(), Relaxed);
            let ours = mopp::build(&prims, 0.01);
            for pr in &prims {
                if keys.binary_search(&pr.key).is_err() {
                    not_in_code.fetch_add(1, Relaxed);
                    continue;
                }
                let found = code.query(pr.min, pr.max).map_err(|e| e.to_string())?;
                if !found.contains(&pr.key) {
                    game_misses.fetch_add(1, Relaxed);
                }
                let found = ours.query(pr.min, pr.max).map_err(|e| e.to_string())?;
                if !found.contains(&pr.key) {
                    return Err(format!("new MOPP query misses key {}", pr.key));
                }
            }
        }
        Ok(())
    });
    if game_dir().is_some() {
        eprintln!(
            "hkx: {count}, objects: {}, typed: {}, written back: {}, MOPP triangles: {}, \
             not in their code: {}",
            objects.load(Relaxed),
            typed.load(Relaxed),
            identical.load(Relaxed),
            triangles.load(Relaxed),
            not_in_code.load(Relaxed)
        );
        assert_eq!(count, 11_258);
        assert_eq!(objects.into_inner(), 192_135);
        assert_eq!(typed.into_inner(), 11_257);
        // 24 packfiles also hold an hkxScene, which is not written.
        assert_eq!(identical.into_inner(), 11_233);
        assert!(not_in_code.into_inner() <= 164);
        // Three flat triangles on the upper edge of their code's space are not
        // found from their own box by the game's code either.
        assert!(game_misses.into_inner() <= 3);
    }
}

/// Runs `check` on the data of every resource with type `tag`, in parallel.
/// Returns the number of resources; `check` reports a failure with `Err`.
fn check_every(tag: &str, check: impl Fn(&[u8]) -> Result<(), String> + Sync) -> usize {
    let Some(dir) = game_dir() else { return 0 };
    let game = GameInstance::open(&dir).unwrap();
    let type_id = type_tags()
        .resolve(tag)
        .unwrap_or_else(|| panic!("unknown tag {tag}"));
    let failures = Failures::default();
    let count = std::sync::atomic::AtomicUsize::new(0);
    game.streams().par_iter().for_each(|file| {
        let stream = StreamFile::open(&file.path).unwrap();
        for (index, resource) in stream.layout().resources.iter().enumerate() {
            if resource.header.type_id != type_id {
                continue;
            }
            let data = stream.resource_data(index).unwrap();
            if let Err(error) = check(&data) {
                failures.push(format!("{} #{index}: {error}", file.rel_path));
            }
            count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    });
    failures.assert_empty();
    count.into_inner()
}

#[test]
fn every_simgroup_parses_and_names_its_objects() {
    use doctor_toolkit::resource::simgroup::{SimGroup, Value};
    let objects = std::sync::atomic::AtomicUsize::new(0);
    let unknown_blobs = std::sync::atomic::AtomicUsize::new(0);
    let transforms = std::sync::atomic::AtomicUsize::new(0);
    let count = check_every("simgroup", |data| {
        let group = SimGroup::parse(data).map_err(|e| e.to_string())?;
        objects.fetch_add(group.objects.len(), std::sync::atomic::Ordering::Relaxed);
        let blobs = group
            .packets
            .iter()
            .flat_map(|p| &p.attrs)
            .filter(|a| matches!(a.value, Value::Bytes(_)))
            .count();
        unknown_blobs.fetch_add(blobs, std::sync::atomic::Ordering::Relaxed);
        transforms.fetch_add(
            group
                .objects
                .iter()
                .filter(|o| group.transform(o).is_some())
                .count(),
            std::sync::atomic::Ordering::Relaxed,
        );
        serde_json::to_string(&group).map_err(|e| e.to_string())?;
        if group.encode() != data {
            return Err("encode(parse(x)) differs from x".into());
        }
        Ok(())
    });
    if count == 0 {
        return;
    }
    assert_eq!(count, 2006);
    assert_eq!(objects.into_inner(), 111_518);
    assert_eq!(transforms.into_inner(), 61_279);
    // Every attribute of the game is in the schema, so no aux value stays untyped.
    assert_eq!(unknown_blobs.into_inner(), 0);
}

#[test]
fn every_metamodel_parses() {
    use doctor_toolkit::resource::metamodel::{MetaModel, PropValue};
    use doctor_toolkit::resource::simgroup::Hash32;
    let properties = std::sync::atomic::AtomicUsize::new(0);
    let unknown = std::sync::atomic::AtomicUsize::new(0);
    let arrays = std::sync::atomic::AtomicUsize::new(0);
    let count = check_every("metamodel", |data| {
        let model = MetaModel::parse(data).map_err(|e| e.to_string())?;
        properties.fetch_add(model.properties.len(), std::sync::atomic::Ordering::Relaxed);
        // Type 0x100 is a u32 array: every one in the game is a
        // `ComponentList` of the same three values.
        for p in &model.properties {
            if let PropValue::Array(values) = &p.value {
                let expected = [Hash32(0x9F3F_A445), Hash32(1), Hash32(0xDC51_EBB1)];
                if p.name != "ComponentList" || values[..] != expected {
                    return Err(format!("array {} = {values:?}", p.name));
                }
                arrays.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        let n = model
            .properties
            .iter()
            .filter(|p| matches!(p.value, PropValue::Unknown(_)))
            .count();
        unknown.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
        serde_json::to_string(&model).map_err(|e| e.to_string())?;
        Ok(())
    });
    if count == 0 {
        return;
    }
    assert_eq!(count, 12_315);
    assert_eq!(properties.into_inner(), 149_798);
    assert_eq!(unknown.into_inner(), 0);
    assert_eq!(arrays.into_inner(), 713);
}

#[test]
fn every_variable_dictionary_parses_and_round_trips() {
    use doctor_toolkit::resource::variabledictionary::VariableDictionary;
    let variables = std::sync::atomic::AtomicUsize::new(0);
    let count = check_every("variabledictionary", |data| {
        let dict = VariableDictionary::parse(data).map_err(|e| e.to_string())?;
        variables.fetch_add(dict.variables.len(), std::sync::atomic::Ordering::Relaxed);
        if dict.encode() != data {
            return Err("encode(parse(x)) differs from x".into());
        }
        Ok(())
    });
    if count == 0 {
        return;
    }
    assert_eq!(count, 26);
    assert_eq!(variables.into_inner(), 98);
}

#[test]
fn every_parts_database_parses_and_reencodes_byte_for_byte() {
    use doctor_toolkit::resource::partsdb::PartsDb;
    let parts = std::sync::atomic::AtomicUsize::new(0);
    let sets = std::sync::atomic::AtomicUsize::new(0);
    let count = check_every("partsdb", |data| {
        let db = PartsDb::parse(data).map_err(|e| e.to_string())?;
        let encoded = db.encode().map_err(|e| e.to_string())?;
        if encoded != data {
            return Err(format!("`{}` does not re-encode byte for byte", db.name));
        }
        let json = serde_json::to_string(&db).map_err(|e| e.to_string())?;
        let back: PartsDb = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        if back != db {
            return Err(format!("`{}` changes through JSON", db.name));
        }
        parts.fetch_add(db.parts.len(), std::sync::atomic::Ordering::Relaxed);
        sets.fetch_add(db.sets.len(), std::sync::atomic::Ordering::Relaxed);
        Ok(())
    });
    if count == 0 {
        return;
    }
    assert_eq!(count, 68);
    assert_eq!(parts.into_inner(), 2599);
    assert_eq!(sets.into_inner(), 1251);
}

/// Tree models (stage 68): every `treemodel` parses and writes back byte for
/// byte; its header hash is the CRC-32 of its source path and its id the
/// SDBM of it; every part's material is in the game and uses the tree
/// shader of the part's kind; corner indices of rotating leaves are whole
/// numbers below 40 and strip indices stay within the vertices.
#[test]
fn every_tree_model_parses_writes_back_and_finds_its_materials() {
    use doctor_toolkit::hash::{crc32, sdbm};
    use doctor_toolkit::resource::material::Material;
    use doctor_toolkit::resource::shading::shader;
    use doctor_toolkit::resource::treemodel::{LEAF_CORNERS, PartKind, TreeModel, Vertices};
    use std::collections::HashMap;
    use std::sync::Mutex;
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let (material, tree) = (sdbm(b"material"), sdbm(b"treemodel"));
    // Lit shader of every material by name hash.
    let shaders: Mutex<HashMap<u32, &'static str>> = Mutex::default();
    let trees: Mutex<Vec<(String, u32, Vec<u8>)>> = Mutex::default();
    game.streams().par_iter().for_each(|file| {
        let stream = StreamFile::open(&file.path).unwrap();
        for (index, resource) in stream.layout().resources.iter().enumerate() {
            let t = resource.header.type_id;
            if t == material {
                let m = Material::parse(&stream.resource_data(index).unwrap()).unwrap();
                let name = shader(m.shaders[1].0).map_or("?", |s| s.name);
                shaders.lock().unwrap().insert(m.name_hash, name);
            } else if t == tree {
                trees.lock().unwrap().push((
                    format!("{} #{index}", file.rel_path),
                    resource.header.ids[3],
                    stream.resource_data(index).unwrap(),
                ));
            }
        }
    });
    let shaders = shaders.into_inner().unwrap();
    let trees = trees.into_inner().unwrap();
    let failures = Failures::default();
    let mut kinds: HashMap<PartKind, (usize, usize)> = HashMap::new();
    let mut empty = Vec::new();
    for (at, id, data) in &trees {
        let model = match TreeModel::parse(data) {
            Ok(m) => m,
            Err(error) => {
                failures.push(format!("{at}: {error}"));
                continue;
            }
        };
        if model.to_bytes() != *data {
            failures.push(format!("{at}: does not write back byte for byte"));
        }
        let path = model.source_path().as_bytes();
        if crc32(path) != model.path_hash || sdbm(path) != *id {
            failures.push(format!("{at}: hashes of the path differ"));
        }
        for part in &model.parts {
            let entry = kinds.entry(part.kind).or_default();
            entry.0 += 1;
            entry.1 += part.meshes.len();
            let lit = shaders.get(&part.material_hash).copied();
            let expected: &[&str] = match part.kind {
                PartKind::Trunk => &["TreeTrunk", "TreeNonRotatingLeaf"],
                PartKind::RotatingLeaves => &["TreeRotatingLeaf"],
                PartKind::Leaves => &["TreeNonRotatingLeaf"],
            };
            if !lit.is_some_and(|s| expected.contains(&s)) {
                failures.push(format!(
                    "{at}: part `{}` ({:?}) has material {:08x} with shader {lit:?}",
                    model.part_name(part),
                    part.kind,
                    part.material_hash
                ));
            }
            for mesh in &part.meshes {
                let n = mesh.vertices.len();
                if mesh.indices.iter().any(|&i| i as usize >= n) {
                    failures.push(format!("{at}: strip index outside the vertices"));
                }
                if let Vertices::Leaves(leaves) = &mesh.vertices
                    && leaves.iter().any(|v| {
                        v.corner.fract() != 0.0 || !(0.0..LEAF_CORNERS as f32).contains(&v.corner)
                    })
                {
                    failures.push(format!("{at}: leaf corner outside the table"));
                }
                if mesh.triangles(part.kind).is_empty() {
                    empty.push(format!("{at}: {} {:?}", model.part_name(part), part.kind));
                }
            }
        }
    }
    failures.assert_empty();
    assert_eq!(trees.len(), 432);
    // (parts, meshes) by kind; 66 frond parts have a second, triangle mesh.
    assert_eq!(kinds[&PartKind::Trunk], (575, 575));
    assert_eq!(kinds[&PartKind::RotatingLeaves], (239, 239));
    assert_eq!(kinds[&PartKind::Leaves], (291, 357));
    // One palm (in the three cities) has a frond quad mesh without vertices.
    assert_eq!(empty.len(), 3, "{empty:?}");
}

/// Stage 70: the colour grading of every city finds its colour tables and
/// the city's lamps are lights of their models, most of them lit only at night.
#[test]
fn every_city_grades_its_colours_and_lights_its_lamps() {
    use doctor_toolkit::level::{Catalog, Level, LevelSource, LightKind, Lookup, Overrides, Scope};
    use doctor_toolkit::time_of_day::CityLight;
    use std::sync::Arc;

    let Some(dir) = game_dir() else { return };
    let game = Arc::new(GameInstance::open(&dir).unwrap());
    let (index, errors) = game.build_index(|_| {});
    assert!(errors.is_empty());
    let catalog = Catalog::new(&game, &index);
    let lookup = Arc::new(Lookup::new(&index));
    let parents = game.stream_parents().unwrap();
    for city in ["cuba", "queens", "miami"] {
        let light = CityLight::read(&game, city).unwrap();
        let tables = light.colour_tables(&game);
        for (_, key) in &light.outdoor.grading {
            assert!(
                tables.contains_key(&key.vfx),
                "{city}: no colour table for vfx {:08x}",
                key.vfx
            );
        }
        // Noon is mostly the day table, midnight the night table; the
        // night table warms a mid grey.
        let noon = light.at(12 * 3600 + 180);
        let main = noon
            .grading
            .tables()
            .iter()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        assert_eq!(tables[&main.0].name, "lutcolor_miami_day", "{city}");
        let night = light.at(180);
        assert_eq!(night.grading.count, 1);
        let table = &tables[&night.grading.tables()[0].0];
        assert_eq!(table.name, "lutcolor_miami_night");
        let grey = table.grade([0.5; 3]);
        assert!(grey[0] > grey[2], "{city}: {grey:?}");
        assert_eq!(night.night, 1.0, "{city}: windows lit at night");
        assert_eq!(noon.night, 0.0, "{city}");

        let name = format!("gf2/{city}/local");
        let streams: Vec<usize> = catalog.under(&name).map(|z| z.stream).collect();
        let scope = Scope::new(&game, &parents, &streams);
        let source = Arc::new(LevelSource::new(
            game.clone(),
            lookup.clone(),
            Overrides::default(),
            scope,
        ));
        let level = Level::load(source, &streams).unwrap();
        let lights: Vec<_> = level
            .models
            .values()
            .flat_map(|m| m.desc.lights.iter())
            .collect();
        let night_only = lights
            .iter()
            .filter(|l| l.day_night.is_some_and(|dn| dn[0] == 0.0 && dn[1] > 0.0))
            .count();
        let spots = lights.iter().filter(|l| l.kind == LightKind::Spot).count();
        assert!(
            lights.len() > 100 && night_only > 50 && spots > 20,
            "{city}: {} lights, {night_only} lit only at night, {spots} spots",
            lights.len()
        );
        for l in &lights {
            assert!(l.radius >= 0.0 && l.radius < 200.0, "{city}: {l:?}");
        }
    }
}

/// World materials of the three cities (stage 69, `renderer.md` §10.7):
/// every material the cities' meshes draw with a world shader finds every
/// texture it samples; the textures it does not find are the engine's
/// built-in ones, named by hash or by their source.
#[test]
fn every_world_material_of_the_cities_finds_its_textures() {
    use doctor_toolkit::level::{
        Catalog, Level, LevelAssets, LevelSource, Lookup, Overrides, Scope,
    };
    use doctor_toolkit::resource::material::ParamValue;
    use doctor_toolkit::resource::world_shading::{Builtin, WorldModel, WorldShading};
    use std::collections::{BTreeMap, HashSet};
    use std::sync::Arc;

    let Some(dir) = game_dir() else { return };
    let game = Arc::new(GameInstance::open(&dir).unwrap());
    let (index, errors) = game.build_index(|_| {});
    assert!(errors.is_empty());
    let catalog = Catalog::new(&game, &index);
    let lookup = Arc::new(Lookup::new(&index));
    let parents = game.stream_parents().unwrap();
    let failures = Failures::default();
    let mut models: BTreeMap<String, usize> = BTreeMap::new();
    let mut world = 0;
    for city in ["gf2/cuba/local", "gf2/queens/local", "gf2/miami/local"] {
        let streams: Vec<usize> = catalog.under(city).map(|z| z.stream).collect();
        let scope = Scope::new(&game, &parents, &streams);
        let source = Arc::new(LevelSource::new(
            game.clone(),
            lookup.clone(),
            Overrides::default(),
            scope,
        ));
        let level = Level::load(source.clone(), &streams).unwrap();
        let assets = LevelAssets::new(&source, &index);
        let hashes: HashSet<u32> = level
            .meshes
            .par_iter()
            .filter(|m| m.found.is_some())
            .flat_map_iter(|m| {
                source
                    .mesh(m.id)
                    .map(|model| model.records.iter().map(|r| r.material_hash).collect())
                    .unwrap_or_else(|_| Vec::new())
            })
            .collect();
        for hash in hashes {
            let Some(material) = assets.material(&source, hash) else {
                failures.push(format!("{city}: material {hash:08x} not found"));
                continue;
            };
            let Some(shading) = WorldShading::from_material(&material) else {
                continue;
            };
            world += 1;
            *models.entry(format!("{:?}", shading.model)).or_default() += 1;
            for texture in shading.textures() {
                if assets.texture(&source, texture).is_none() {
                    failures.push(format!(
                        "{city}: `{}` ({}): texture {texture:08x} not found",
                        material.name, shading.shader
                    ));
                }
            }
            for p in &material.params {
                let ParamValue::Texture(h) = p.value else {
                    continue;
                };
                let by_hash = Builtin::of(h);
                let by_name = Builtin::named(&p.source);
                if by_hash.is_some() && by_name.is_some() && by_hash != by_name {
                    failures.push(format!(
                        "{city}: `{}`: {h:08x} is {by_hash:?}, its source `{}`",
                        material.name, p.source
                    ));
                }
                if by_hash.is_none() && by_name.is_none() && assets.texture(&source, h).is_none() {
                    failures.push(format!(
                        "{city}: `{}`: `{}` = {h:08x} ({}) not found",
                        material.name, p.name, p.source
                    ));
                }
            }
            // The renderer reads the highlight's sharpness from the gloss map.
            if shading.exponent_map.is_some() && shading.exponent_map != shading.gloss {
                failures.push(format!(
                    "{city}: `{}`: exponent map {:08x?} is not the gloss map {:08x?}",
                    material.name, shading.exponent_map, shading.gloss
                ));
            }
            if shading.model == WorldModel::Detail && shading.base.is_none() {
                failures.push(format!("{city}: `{}` has no base layer", material.name));
            }
        }
    }
    failures.assert_empty();
    // Per city (a material shared by two cities counts twice).
    assert_eq!(world, 8_020, "{models:?}");
    assert_eq!(models["Detail"], 4_251);
    assert_eq!(models["Building"], 1_933);
    assert_eq!(models["Basic"], 1_508);
    assert_eq!(models["Window"] + models["Glass"], 284);
    assert_eq!(
        (models["Panorama"], models["Water"], models["Overlay"]),
        (37, 4, 3)
    );
}

#[test]
fn every_point_table_parses() {
    use doctor_toolkit::resource::points::PointTable;
    let points = std::sync::atomic::AtomicUsize::new(0);
    let count = check_every("covernodetable", |data| {
        let table = PointTable::parse(data).map_err(|e| e.to_string())?;
        for p in &table.points {
            let login = &p.guid.0[8..];
            if !login
                .iter()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_')
            {
                return Err(format!("GUID {} has no machine name", p.guid));
            }
        }
        points.fetch_add(table.points.len(), std::sync::atomic::Ordering::Relaxed);
        Ok(())
    });
    if count > 0 {
        assert_eq!(count, 682);
        assert_eq!(points.into_inner(), 52_162);
    }
}

#[test]
fn every_graph_writes_back_byte_for_byte() {
    use doctor_toolkit::resource::graph::{Graph, RTree, Visibility};
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    let nodes = AtomicUsize::new(0);
    let links = AtomicUsize::new(0);
    let shapes = AtomicUsize::new(0);
    let pairs = AtomicUsize::new(0);
    let paths = AtomicUsize::new(0);
    let reproduced = AtomicUsize::new(0);
    let count = check_every("graph", |data| {
        let graph = Graph::parse(data).map_err(|e| e.to_string())?;
        nodes.fetch_add(graph.nodes.len(), Relaxed);
        links.fetch_add(graph.link_count(), Relaxed);
        let bytes = graph.to_bytes().map_err(|e| e.to_string())?;
        if bytes != data {
            let at = bytes.iter().zip(data).position(|(a, b)| a != b);
            return Err(format!(
                "written differently: {} vs {} bytes, first difference at {at:?}",
                bytes.len(),
                data.len()
            ));
        }
        // The JSON form gives back the same graph and the same bytes.
        let json = serde_json::to_string(&graph).map_err(|e| e.to_string())?;
        let back: Graph = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        if back.to_bytes().map_err(|e| e.to_string())? != data {
            return Err("JSON form writes different bytes".into());
        }
        // Derived data: R-tree shapes and plain visibility everywhere, and
        // every stored path table gives shortest paths.
        if matches!(graph.rtree, RTree::Shape(_)) || graph.nodes.is_empty() {
            shapes.fetch_add(1, Relaxed);
        }
        if matches!(graph.visibility, Visibility::Pairs { .. }) {
            pairs.fetch_add(1, Relaxed);
        }
        if let Some(t) = &graph.precomputed_paths {
            if !doctor_toolkit::resource::graph::paths_valid(&graph, t) {
                return Err("stored paths are not shortest paths".into());
            }
            paths.fetch_add(1, Relaxed);
            if doctor_toolkit::resource::graph::compute_paths(&graph).as_ref() == Some(t) {
                reproduced.fetch_add(1, Relaxed);
            }
        }
        if !graph.stale_lengths().is_empty() {
            return Err("a link length differs from its node distance".into());
        }
        // Repairing an unchanged graph changes nothing.
        let mut repaired = graph.clone();
        if !repaired.repair_derived().is_empty() || repaired != graph {
            return Err("repair changed an unchanged graph".into());
        }
        Ok(())
    });
    if count == 0 {
        return;
    }
    assert_eq!(count, 2178);
    assert_eq!(nodes.into_inner(), 58_623);
    assert_eq!(links.into_inner(), 152_629);
    assert_eq!(shapes.into_inner(), 2178);
    assert_eq!(pairs.into_inner(), 2178);
    assert_eq!(paths.into_inner(), 2087);
    eprintln!(
        "path tables computed again equal to the stored ones: {} of 2087",
        reproduced.into_inner()
    );
}

#[test]
fn graph_edits_keep_every_graph_writable() {
    use doctor_toolkit::resource::graph::{Graph, Neighbourhood, compute_paths, paths_valid};
    let count = check_every("graph", |data| {
        let mut graph = Graph::parse(data).map_err(|e| e.to_string())?;
        let n = graph.nodes.len();
        if n < 2 {
            return Ok(());
        }
        let err = |e: doctor_toolkit::Error| e.to_string();
        // Move a node: lengths follow, paths stay shortest.
        let mut p = graph.nodes[0].position;
        p[0] += 1.5;
        graph.move_node(0, p).map_err(err)?;
        // Attach a point next to node 1 and remove the last node.
        let mut q = graph.nodes[1].position;
        q[2] += 2.0;
        graph
            .attach_point(q, 0.5, Neighbourhood::default())
            .map_err(err)?;
        graph.remove_node(n - 1).map_err(err)?;
        let bytes = graph.to_bytes().map_err(err)?;
        let back = Graph::parse(&bytes).map_err(|e| format!("reparse: {e}"))?;
        if back.to_bytes().map_err(err)? != bytes {
            return Err("edited graph does not write back".into());
        }
        if !back.stale_lengths().is_empty() {
            return Err("stale lengths after edits".into());
        }
        if let Some(t) = &back.precomputed_paths
            && (!paths_valid(&back, t) || compute_paths(&back).is_none())
        {
            return Err("paths not valid after edits".into());
        }
        Ok(())
    });
    if count > 0 {
        assert_eq!(count, 2178);
    }
}

/// A small deterministic generator for the edit tests.
struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn float(&mut self) -> f32 {
        (self.next() % 20_000) as f32 / 10.0 - 1000.0
    }
}

type ObjectView = (
    u32,
    String,
    Vec<String>,
    Vec<(u32, Vec<(u16, String)>)>,
    Option<Vec<u32>>,
);

/// Everything a parsed SimGroup says about its objects, comparable.
fn simgroup_view(group: &doctor_toolkit::resource::simgroup::SimGroup) -> Vec<ObjectView> {
    group
        .objects
        .iter()
        .map(|o| {
            let packets = group
                .object_packets(o)
                .map(|p| {
                    let attrs = p
                        .attrs
                        .iter()
                        .map(|a| (a.index, format!("{:?}", a.value)))
                        .collect();
                    (p.class.0, attrs)
                })
                .collect();
            let components = o
                .components
                .map(|c| group.packets[c].attrs.iter().map(|a| a.raw).collect());
            (
                o.class,
                o.guid.to_string(),
                o.links.iter().map(|g| g.to_string()).collect(),
                packets,
                components,
            )
        })
        .collect()
}

#[test]
fn random_simgroup_edits_reparse_with_the_edited_values_only() {
    use doctor_toolkit::resource::simgroup::{
        CLASS_SYSTEM_COMMANDS, Guid, Hash32, Kind, SimGroup, Value,
    };
    use doctor_toolkit::resource::simgroup_edit::Transform;
    use std::collections::HashMap;

    let edits_done = std::sync::atomic::AtomicUsize::new(0);
    let grown = std::sync::atomic::AtomicUsize::new(0);
    let count = check_every("simgroup", |data| {
        let original = SimGroup::parse(data).map_err(|e| e.to_string())?;
        if original.objects.is_empty() {
            return Ok(());
        }
        let mut group = original.clone();
        let mut rng = XorShift(0x9E37_79B9_7F4A_7C15 ^ data.len() as u64);
        let mut wanted: HashMap<(usize, usize, u16), Option<Value>> = HashMap::new();
        let mut tries = 0;
        while wanted.len() < 12 && tries < 200 {
            tries += 1;
            let o = rng.below(group.objects.len());
            let object = &group.objects[o];
            if object.packets.is_empty() {
                continue;
            }
            let slot = rng.below(object.packets.len());
            let packet = &group.packets[object.packets[slot]];
            if packet.attr_count == 0 {
                continue;
            }
            let index = rng.below(packet.attr_count as usize) as u16;
            let value = if rng.below(8) == 0 {
                None
            } else {
                Some(match group.attr_kind(packet.class.0, index) {
                    Some(Kind::Int) => Value::Int(rng.below(2000) as i32 - 1000),
                    Some(Kind::Float) => Value::Float(rng.float()),
                    Some(Kind::Hash) => Value::Hash(Hash32(rng.next() as u32)),
                    Some(Kind::String) => Value::String(format!("edit_{}", rng.below(5))),
                    Some(Kind::Guid) => Value::Guid(Guid::from_resource_id(rng.next() as u32 | 1)),
                    Some(Kind::Vec3) => Value::Vec3([rng.float(), rng.float(), rng.float()]),
                    Some(Kind::Matrix) => Value::Matrix(
                        Transform {
                            position: [rng.float(), rng.float(), rng.float()],
                            rotation: [rng.float(), rng.float(), rng.float()],
                            scale: [1.0, 2.0, 0.5],
                        }
                        .to_matrix(),
                    ),
                    None => continue,
                })
            };
            group
                .set_attr(o, slot, index, value.clone())
                .map_err(|e| format!("object {o} packet {slot} attribute {index}: {e}"))?;
            wanted.insert((o, slot, index), value);
        }
        // Moves of objects with a transform.
        for _ in 0..4 {
            let o = rng.below(group.objects.len());
            if group.transform(&group.objects[o]).is_some() {
                let p = [rng.float(), rng.float(), rng.float()];
                group.set_position(o, p).map_err(|e| e.to_string())?;
                let m = group.transform(&group.objects[o]).unwrap();
                if m[3][..3] != p {
                    return Err(format!("object {o}: position not set"));
                }
                let slot = group.slot_of(o, CLASS_SYSTEM_COMMANDS).unwrap();
                wanted.insert((o, slot, 1), Some(Value::Matrix(m)));
            }
        }
        let bytes = group.encode();
        let reparsed = SimGroup::parse(&bytes).map_err(|e| format!("reparse: {e}"))?;
        if simgroup_view(&reparsed) != simgroup_view(&group) {
            return Err("reparsed group differs from the edited one".into());
        }
        if reparsed.encode() != bytes {
            return Err("encoding the edited group is not stable".into());
        }
        // The edited values are there; everything else is as before.
        let lookup = |g: &SimGroup, o: usize, slot: usize, index: u16| {
            let p = g.objects[o].packets[slot];
            g.packets[p]
                .attrs
                .iter()
                .find(|a| a.index == index)
                .map(|a| format!("{:?}", a.value))
        };
        let is_default = |v: &Value| match v {
            Value::String(s) => s.is_empty(),
            Value::Int(0) | Value::Hash(Hash32(0)) => true,
            Value::Float(f) => f.to_bits() == 0,
            _ => false,
        };
        for (&(o, slot, index), value) in &wanted {
            let expected = value
                .as_ref()
                .filter(|v| !is_default(v))
                .map(|v| format!("{v:?}"));
            if lookup(&reparsed, o, slot, index) != expected {
                return Err(format!(
                    "object {o} packet {slot} attribute {index} not set"
                ));
            }
        }
        let before = simgroup_view(&original);
        let after = simgroup_view(&reparsed);
        for (o, (b, a)) in before.iter().zip(&after).enumerate() {
            if (b.0, &b.1, &b.2, &b.4) != (a.0, &a.1, &a.2, &a.4) || b.3.len() != a.3.len() {
                return Err(format!("object {o} changed outside its attributes"));
            }
            for (slot, (pb, pa)) in b.3.iter().zip(&a.3).enumerate() {
                let strip = |attrs: &[(u16, String)]| -> Vec<(u16, String)> {
                    attrs
                        .iter()
                        .filter(|(i, _)| !wanted.contains_key(&(o, slot, *i)))
                        .cloned()
                        .collect()
                };
                if pb.0 != pa.0 || strip(&pb.1) != strip(&pa.1) {
                    return Err(format!("object {o} packet {slot}: unedited values changed"));
                }
            }
        }
        if bytes.len() > data.len() {
            grown.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        edits_done.fetch_add(wanted.len(), std::sync::atomic::Ordering::Relaxed);
        Ok(())
    });
    if count == 0 {
        return;
    }
    let edits = edits_done.into_inner();
    eprintln!(
        "simgroups: {count}, random edits: {edits}, grown: {}",
        grown.into_inner()
    );
    assert!(edits > 10_000, "{edits} edits");
}

#[test]
fn objects_insert_and_remove_in_every_simgroup() {
    use doctor_toolkit::resource::simgroup::SimGroup;
    use doctor_toolkit::resource::simgroup_edit::new_object_guid;

    let inserted = std::sync::atomic::AtomicUsize::new(0);
    let removed = std::sync::atomic::AtomicUsize::new(0);
    let count = check_every("simgroup", |data| {
        let original = SimGroup::parse(data).map_err(|e| e.to_string())?;
        let n = original.objects.len();
        if n == 0 {
            return Ok(());
        }
        let mut rng = XorShift(0xA076_1D64_78BD_642F ^ data.len() as u64);
        let mut group = original.clone();
        let taken = |g: &SimGroup, guid: &doctor_toolkit::resource::simgroup::Guid| {
            g.objects.iter().any(|o| o.guid == *guid)
        };
        // Duplicates (shared packets), then a moved duplicate.
        let mut sources = Vec::new();
        for k in 0..3 {
            let source = rng.below(n);
            let guid = new_object_guid(&format!("dup{k}"), |g| taken(&group, g));
            let at = group
                .duplicate_object(source, guid)
                .map_err(|e| format!("duplicate {source}: {e}"))?;
            sources.push((source, at));
        }
        if let Some(&(_, at)) = sources
            .iter()
            .find(|(s, _)| group.transform(&group.objects[*s]).is_some())
        {
            group
                .set_position(at, [1.0, 2.0, 3.0])
                .map_err(|e| e.to_string())?;
        }
        let bytes = group.encode();
        let reparsed = SimGroup::parse(&bytes).map_err(|e| format!("reparse: {e}"))?;
        if simgroup_view(&reparsed) != simgroup_view(&group) {
            return Err("reparsed group differs after duplication".into());
        }
        if simgroup_view(&reparsed)[..n] != simgroup_view(&original)[..] {
            return Err("existing objects changed".into());
        }
        if reparsed.encode() != bytes {
            return Err("encoding after duplication is not stable".into());
        }
        // Removing the duplicates again gives the original bytes.
        for &(_, at) in sources.iter().rev() {
            group.remove_object(at).map_err(|e| e.to_string())?;
        }
        if group.encode() != data {
            return Err("removing the duplicates does not restore the bytes".into());
        }
        // A copy into another group's layout: the first object copied into a
        // group without it, then removal of a random original object.
        let guid = new_object_guid("copy", |g| taken(&group, g));
        let source = rng.below(n);
        let copy = group
            .copy_object_from(&original, source, guid)
            .map_err(|e| format!("copy: {e}"))?;
        if copy != n {
            return Err(format!("copy appended at {copy}, not {n}"));
        }
        let victim = rng.below(n);
        group.remove_object(victim).map_err(|e| e.to_string())?;
        let bytes = group.encode();
        let reparsed = SimGroup::parse(&bytes).map_err(|e| format!("reparse: {e}"))?;
        if simgroup_view(&reparsed) != simgroup_view(&group) {
            return Err("reparsed group differs after copy and removal".into());
        }
        let mut expected = simgroup_view(&original);
        let mut copied = expected[source].clone();
        copied.1 = guid.to_string();
        expected.remove(victim);
        expected.push(copied);
        if simgroup_view(&reparsed) != expected {
            return Err(format!(
                "objects other than {victim} changed, or the copy differs"
            ));
        }
        if reparsed.encode() != bytes {
            return Err("encoding after removal is not stable".into());
        }
        inserted.fetch_add(4, std::sync::atomic::Ordering::Relaxed);
        removed.fetch_add(4, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    });
    if count == 0 {
        return;
    }
    let inserted = inserted.into_inner();
    eprintln!(
        "simgroups: {count}, objects inserted: {inserted}, removed: {}",
        removed.into_inner()
    );
    assert!(inserted > 7_000, "{inserted}");
}

#[test]
fn random_metamodel_edits_change_only_the_edited_properties() {
    use doctor_toolkit::resource::metamodel::{MetaModel, PropValue};
    use doctor_toolkit::resource::metamodel_edit::set_property;
    use doctor_toolkit::resource::simgroup::Hash32;

    let edits_done = std::sync::atomic::AtomicUsize::new(0);
    let count = check_every("metamodel", |data| {
        let original = MetaModel::parse(data).map_err(|e| e.to_string())?;
        let n = original.properties.len();
        if n == 0 {
            return Ok(());
        }
        let mut rng = XorShift(0x2545_F491_4F6C_DD1D ^ data.len() as u64);
        let mut bytes = data.to_vec();
        let mut wanted = std::collections::HashMap::new();
        for _ in 0..4 {
            let index = rng.below(n);
            let value = match &original.properties[index].value {
                PropValue::Bool(b) => PropValue::Bool(!b),
                PropValue::Int(_) => PropValue::Int(rng.below(1000) as i32),
                PropValue::Float(_) => PropValue::Float(rng.float()),
                PropValue::Color(_) => PropValue::Color(Hash32(rng.next() as u32)),
                PropValue::String(_) => PropValue::String(format!("edit_{}", rng.below(9))),
                PropValue::Resource(_) => PropValue::Resource(rng.below(original.resources.len())),
                PropValue::Matrix(_) => {
                    PropValue::Matrix([[rng.float(); 4], [1.0; 4], [2.0; 4], [rng.float(); 4]])
                }
                PropValue::Vec3(_) => PropValue::Vec3([rng.float(), rng.float(), rng.float()]),
                PropValue::Array(_) => PropValue::Array(
                    (0..rng.below(6))
                        .map(|_| Hash32(rng.next() as u32))
                        .collect(),
                ),
                _ => continue,
            };
            bytes = set_property(&bytes, index, &value)
                .map_err(|e| format!("property {index}: {e}"))?;
            wanted.insert(index, value);
        }
        let edited = MetaModel::parse(&bytes).map_err(|e| format!("reparse: {e}"))?;
        for (i, (before, after)) in original
            .properties
            .iter()
            .zip(&edited.properties)
            .enumerate()
        {
            let expected = wanted.get(&i).unwrap_or(&before.value);
            if format!("{:?}", after.value) != format!("{expected:?}")
                || (before.name.as_str(), &before.class, before.index)
                    != (after.name.as_str(), &after.class, after.index)
            {
                return Err(format!(
                    "property {i}: {:?} instead of {expected:?}",
                    after.value
                ));
            }
        }
        if (
            &original.states,
            &original.parts,
            &original.resources,
            &original.variables,
        ) != (
            &edited.states,
            &edited.parts,
            &edited.resources,
            &edited.variables,
        ) {
            return Err("tables other than the properties changed".into());
        }
        edits_done.fetch_add(wanted.len(), std::sync::atomic::Ordering::Relaxed);
        Ok(())
    });
    if count == 0 {
        return;
    }
    let edits = edits_done.into_inner();
    eprintln!("metamodels: {count}, random property edits: {edits}");
    assert!(edits > 30_000, "{edits} edits");
}

#[test]
fn every_apx_takes_texture_and_movie_replacements() {
    use doctor_toolkit::resource::apx::{AptFile, TAG_ALIGN, TAG_APT, TAG_TITLE};
    use doctor_toolkit::resource::pixels;

    let replaced = std::sync::atomic::AtomicUsize::new(0);
    let count = check_every("apx", |data| {
        let original = AptFile::parse(data).map_err(|e| e.to_string())?;
        let textures = original.textures().map_err(|e| e.to_string())?;
        let check_aligned = |bytes: &[u8]| -> Result<AptFile, String> {
            let parsed = AptFile::parse(bytes).map_err(|e| e.to_string())?;
            let mut pos = 8;
            for b in &parsed.blocks {
                if b.tag != TAG_TITLE && b.tag != TAG_ALIGN && (pos + 8) % 16 != 0 {
                    return Err(format!("{} misaligned", String::from_utf8_lossy(&b.tag)));
                }
                pos += 8 + b.data.len();
            }
            Ok(parsed)
        };
        let mut file = original.clone();
        file.align_blocks();
        // The apx the old toolkit rewrote is misaligned; aligning it is a fix.
        if check_aligned(data).is_ok() && file != original {
            return Err("align_blocks changed an unedited file".into());
        }
        // A bigger movie, then every embedded texture at another size.
        let mut apt = original.block(TAG_APT).unwrap().data.clone();
        apt.extend_from_slice(&[0; 5]);
        file.set_apt(apt.clone()).map_err(|e| e.to_string())?;
        let mut sizes = Vec::new();
        for (i, texture) in textures.iter().enumerate() {
            if texture.header.kind() != doctor_toolkit::resource::tg4::Kind::Flat {
                continue;
            }
            // Another size, with room for the mip levels the texture keeps.
            let (w, h) = if texture.header.mip_count() > 1 {
                (
                    texture.header.width as usize + 4,
                    texture.header.height as usize,
                )
            } else {
                (
                    (texture.header.width as usize / 2).max(1) + 4,
                    (texture.header.height as usize / 2).max(1),
                )
            };
            let rgba: Vec<u8> = (0..w * h)
                .flat_map(|k| [(k % 251) as u8, 9, 99, 200])
                .collect();
            let png = pixels::encode_png(w, h, &rgba).map_err(|e| e.to_string())?;
            let new = file
                .import_texture(i, &png, true)
                .map_err(|e| format!("texture {i}: {e}"))?;
            sizes.push((i, new.header.width, new.header.height, new.data.len()));
        }
        let bytes = file.to_bytes().map_err(|e| e.to_string())?;
        let parsed = check_aligned(&bytes)?;
        if parsed.block(TAG_APT).unwrap().data != apt {
            return Err("movie not replaced".into());
        }
        let after = parsed.textures().map_err(|e| e.to_string())?;
        if after.len() != textures.len() {
            return Err("texture count changed".into());
        }
        for (i, w, h, len) in &sizes {
            let t = &after[*i];
            if (t.header.width, t.header.height, t.data.len()) != (*w, *h, *len)
                || t.header.name != textures[*i].header.name
                || t.header.name_hash != textures[*i].header.name_hash
            {
                return Err(format!("texture {i} not replaced"));
            }
            t.rgba(0).map_err(|e| e.to_string())?;
        }
        for (i, (a, b)) in textures.iter().zip(&after).enumerate() {
            if !sizes.iter().any(|s| s.0 == i) && (a.header != b.header || a.data != b.data) {
                return Err(format!("texture {i} changed although not replaced"));
            }
        }
        if parsed.title() != original.title() {
            return Err("title changed".into());
        }
        replaced.fetch_add(sizes.len(), std::sync::atomic::Ordering::Relaxed);
        Ok(())
    });
    if count == 0 {
        return;
    }
    let replaced = replaced.into_inner();
    eprintln!("apx files: {count}, embedded textures replaced: {replaced}");
    assert!(replaced >= 400, "{replaced}");
}

#[test]
fn exported_logic_json_imports_back_without_changes() {
    use doctor_toolkit::resource::logic::{self, TYPE_SIMGROUP};
    use doctor_toolkit::resource::logic_import::apply_json;

    for tag in ["simgroup", "metamodel", "variabledictionary"] {
        let type_id = type_tags().resolve(tag).unwrap();
        let moved = std::sync::atomic::AtomicUsize::new(0);
        let count = check_every(tag, |data| {
            let doc = logic::to_json(type_id, data)
                .unwrap()
                .map_err(|e| e.to_string())?;
            let text = serde_json::to_string(&doc).map_err(|e| e.to_string())?;
            let doc: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            let imported = apply_json(type_id, data, &doc).map_err(|e| e.to_string())?;
            if !imported.changes.is_empty() || imported.data != data {
                return Err(format!("unchanged JSON changed {:?}", imported.changes));
            }
            // Moving the first placed object through JSON changes exactly that.
            if type_id == TYPE_SIMGROUP {
                let mut doc = doc;
                let Some(object) = doc["objects"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|o| o.get("position").is_some())
                else {
                    return Ok(());
                };
                object["position"][1] = 12345.5.into();
                let imported = apply_json(type_id, data, &doc).map_err(|e| e.to_string())?;
                if imported.changes.len() != 1 {
                    return Err(format!("moving changed {:?}", imported.changes));
                }
                moved.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            Ok(())
        });
        eprintln!("{tag}: {count} resource(s) round-trip through JSON, {moved:?} moved");
    }
}

/// The old toolkit's patches found in the game, installed as mods into copies
/// of the originals it backed up: every resource the game has gets the
/// patch's data, the tables of contents get the new sizes, an installed patch
/// exports as a mod package that installs again, and restoring gives the
/// originals byte for byte. The game directory is only read.
#[test]
fn old_toolkit_patches_install_as_mods_on_copies() {
    use doctor_toolkit::mods::integrity::detect_old_toolkit;
    use doctor_toolkit::mods::manager::ModApplyOptions;
    use doctor_toolkit::mods::vpat::VpatPatch;
    use doctor_toolkit::mods::{ModArchive, ModManager};
    use doctor_toolkit::workspace::ResourceKey;

    let Some(dir) = game_dir() else { return };
    let Some(old) = detect_old_toolkit(&dir) else {
        return;
    };
    if old.patch_files.is_empty() || old.backups.is_empty() {
        return;
    }
    let real = GameInstance::open(&dir).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for rel in &old.backups {
        let data = std::fs::read(old.dir.join("backups").join(rel)).unwrap();
        files.push((rel.clone(), data));
    }
    let names: Vec<String> = old
        .backups
        .iter()
        .map(|rel| rel.trim_end_matches(".str").to_ascii_lowercase())
        .collect();
    for (toc_index, toc_file) in real.tocs().iter().enumerate() {
        let table = real.load_toc(toc_index).unwrap();
        if names.iter().any(|n| table.find_file(n).is_some()) {
            let data = std::fs::read(&toc_file.path).unwrap();
            files.push((toc_file.rel_path.clone(), data));
        }
    }
    for (rel, data) in &files {
        std::fs::create_dir_all(root.join(rel).parent().unwrap()).unwrap();
        std::fs::write(root.join(rel), data).unwrap();
    }
    let read_all = || -> Vec<(String, Vec<u8>)> {
        files
            .iter()
            .map(|(rel, _)| (rel.clone(), std::fs::read(root.join(rel)).unwrap()))
            .collect()
    };
    let game = GameInstance::open(root).unwrap();
    let mut mods = ModManager::open(&game).unwrap();

    // What every resource should hold afterwards: the last patch wins.
    let mut expected: std::collections::BTreeMap<ResourceKey, Vec<u8>> = Default::default();
    let mut installed = 0;
    for path in &old.patch_files {
        match mods.install_file(&game, path, true) {
            Ok(report) => {
                installed += 1;
                assert!(report.issues.is_empty(), "{:?}", report.issues);
                let patch = VpatPatch::open(path).unwrap();
                for file in &patch.files {
                    for resource in &file.resources {
                        let key = ResourceKey {
                            stream: file.stream.clone(),
                            type_id: resource.type_id,
                            ids: resource.ids,
                        };
                        expected.insert(key, patch.read_data(resource).unwrap());
                    }
                }
            }
            // A patch made for another edition of the game.
            Err(error) => assert!(error.to_string().contains("nothing in"), "{error}"),
        }
    }
    assert!(installed > 0);
    assert_eq!(read_all(), files, "installing does not touch the game");

    let report = mods.apply(&game, ModApplyOptions::default()).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let mut checked = 0;
    for file in game.streams() {
        let stream = StreamFile::open(&file.path).unwrap();
        let name = file.stream_name();
        for (i, resource) in stream.layout().resources.iter().enumerate() {
            let key = ResourceKey::new(&name, &resource.header);
            if let Some(data) = expected.get(&key) {
                assert_eq!(&stream.resource_data(i).unwrap(), data, "{key}");
                checked += 1;
            }
        }
        for toc_index in 0..game.tocs().len() {
            let table = game.load_toc(toc_index).unwrap();
            if let Some(i) = table.find_file(&name) {
                assert_eq!(
                    table.files[i].data_size as u64,
                    stream.layout().total_data_size()
                );
            }
        }
    }
    assert!(checked > 0);
    let status = mods.status(&game).unwrap();
    assert!(status.streams.iter().all(|s| s.up_to_date));

    // A converted patch exports as a mod package that reads back.
    let first = mods.mods()[0].id.clone();
    let package = root.join("exported.zip");
    mods.export(&first, &package).unwrap();
    let mut archive = ModArchive::open(&package).unwrap();
    archive.verify().unwrap();
    assert_eq!(archive.manifest(), &mods.mods()[0].manifest);
    mods.uninstall(&first).unwrap();
    mods.install_file(&game, &package, true).unwrap();
    let report = mods.apply(&game, ModApplyOptions::default()).unwrap();
    assert!(
        report.streams.is_empty(),
        "the same mod again changes nothing"
    );

    mods.unapply(&game, false).unwrap();
    assert_eq!(read_all(), files);
}

/// References between resources resolve in the stream or in the streams the
/// game loads before it (table-of-contents prerequisites): every metamodel
/// and mesh reference does; material textures except 26 engine textures.
#[test]
fn references_resolve_in_the_stream_or_its_ancestors() {
    use doctor_toolkit::create::{ReferenceChecker, TYPE_MATERIAL, Target};
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let mut checker = ReferenceChecker::new(&game).unwrap();
    let mut unresolved = Vec::new();
    for index in 0..game.streams().len() {
        unresolved.extend(checker.check(index).unwrap());
    }
    let mut textures = std::collections::BTreeSet::new();
    let mut models = 0;
    for problem in &unresolved {
        match (problem.type_id, problem.target) {
            (TYPE_MATERIAL, Target::Texture(hash)) => {
                textures.insert(hash);
            }
            (t, Target::Guid(_)) if t == doctor_toolkit::create::TYPE_SIMGROUP => models += 1,
            _ => panic!("unexpected unresolved reference {problem:?}"),
        }
    }
    assert!(textures.len() <= 26, "{} texture hashes", textures.len());
    assert!(models <= 70, "{models} simgroup models");
}

/// Clones a model (metamodel, meshes, materials, textures) inside copies of
/// real streams through the workspace, adds a new stream, and checks that
/// every stream re-parses, the clones resolve, the originals are unchanged
/// and the tables of contents follow; then everything is undone.
#[test]
fn cloned_models_and_new_streams_round_trip_on_copies() {
    use doctor_toolkit::create::{self, CloneOptions, TYPE_METAMODEL};
    use doctor_toolkit::workspace::{
        ApplyOptions, NewResourceEdit, NewStream, StorageChoice, Workspace,
    };

    let Some(dir) = game_dir() else { return };
    let real = GameInstance::open(&dir).unwrap();
    // A small level stream with metamodels, meshes, materials and textures,
    // together with its ancestors and the tables of contents listing them.
    let (index, _) = real.build_index(|_| {});
    let mut candidates: Vec<(usize, u64)> = Vec::new();
    for (s, file) in real.streams().iter().enumerate() {
        let entries = index.entries.iter().filter(|e| e.stream as usize == s);
        let types: std::collections::HashSet<u32> = entries.map(|e| e.header.type_id).collect();
        if types.contains(&TYPE_METAMODEL)
            && types.contains(&create::TYPE_MESH)
            && types.contains(&doctor_toolkit::resource::texture::TYPE_TG4H)
            && file.rel_path.starts_with("gf2/miami/local/")
        {
            candidates.push((s, file.len));
        }
    }
    candidates.sort_by_key(|c| c.1);
    let (source, _) = *candidates
        .iter()
        .find(|(s, _)| self_contained_metamodel(&real.open_stream(*s).unwrap()).is_some())
        .unwrap();
    let source_name = real.streams()[source].stream_name();
    let mut files: Vec<String> = vec![real.streams()[source].rel_path.clone()];
    for ancestor in real.stream_ancestors(&source_name).unwrap() {
        if let Some(i) = real.find_stream(&ancestor) {
            files.push(real.streams()[i].rel_path.clone());
        }
    }
    for (t, toc_file) in real.tocs().iter().enumerate() {
        if real.load_toc(t).unwrap().find_file(&source_name).is_some() {
            files.push(toc_file.rel_path.clone());
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for rel in &files {
        std::fs::create_dir_all(root.join(rel).parent().unwrap()).unwrap();
        std::fs::copy(dir.join(rel), root.join(rel)).unwrap();
    }
    let snapshot = || -> Vec<Vec<u8>> {
        files
            .iter()
            .map(|rel| std::fs::read(root.join(rel)).unwrap())
            .collect()
    };
    let originals = snapshot();
    let game = GameInstance::open(root).unwrap();
    let s = game.find_stream(&source_name).unwrap();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let mut used = ws.used_ids(&index);
    let baseline: std::collections::BTreeSet<_> = create::check_references(&game, s)
        .unwrap()
        .into_iter()
        .map(|u| u.target)
        .collect();

    // The first metamodel whose resources are all in the stream, with its
    // meshes, their materials and the textures of those materials.
    let stream = game.open_stream(s).unwrap();
    let layout = stream.layout();
    let mut chosen = self_contained_metamodel(&stream).unwrap();
    // Materials of the cloned meshes and textures of those materials, when in the stream.
    for round in 0..2 {
        let mut more = Vec::new();
        for &i in &chosen {
            let header = layout.resources[i].header;
            let data = stream.resource_data(i).unwrap();
            for target in create::references(header.type_id, &data).unwrap() {
                for j in 0..layout.resources.len() {
                    let h = layout.resources[j].header;
                    let hit = match target {
                        create::Target::Material(hash) if round == 0 => {
                            h.type_id == create::TYPE_MATERIAL
                                && stream
                                    .resource_data(j)
                                    .is_ok_and(|d| d.get(8..12) == Some(&hash.to_le_bytes()[..]))
                        }
                        create::Target::Texture(hash) if round == 1 => {
                            h.type_id == doctor_toolkit::resource::texture::TYPE_TG4H
                                && h.ids[3] == hash
                        }
                        _ => false,
                    };
                    if hit && !chosen.contains(&j) && !more.contains(&j) {
                        more.push(j);
                    }
                }
            }
        }
        chosen.extend(more);
    }
    let options = CloneOptions {
        seed: "corpus".into(),
        rewrite_references: true,
        storage: None,
    };
    let (clones, remap) = create::clone_resources(&stream, &chosen, &mut used, &options).unwrap();
    assert!(clones.len() >= 2);
    let before_data: Vec<Vec<u8>> = (0..layout.resources.len())
        .map(|i| stream.resource_data(i).unwrap())
        .collect();
    drop(stream);
    let additions: Vec<NewResourceEdit> = clones
        .iter()
        .map(|c| NewResourceEdit {
            stream: source_name.clone(),
            header: c.header,
            data: c.data.clone(),
            storage: StorageChoice::Keep,
            before: None,
        })
        .collect();
    ws.add_resources(&game, "clone a model", &additions)
        .unwrap();

    // A new stream loaded with the source stream, holding a copy of the first clone.
    let name = "gf2/doctor/corpus_test";
    let guid = create::new_stream_guid(name, &real.toc_hashes().unwrap());
    let tocs: Vec<String> = files
        .iter()
        .filter(|f| f.ends_with(".toc"))
        .cloned()
        .collect();
    ws.add_stream(
        &game,
        NewStream {
            name: name.into(),
            guid: guid.iter().map(|b| format!("{b:02x}")).collect(),
            hash: doctor_toolkit::toc::guid_hash(&guid),
            parent: source_name.clone(),
            with_parent: true,
            auto_load: false,
            tocs: tocs.clone(),
            toc_parents: Default::default(),
            label: String::new(),
        },
    )
    .unwrap();
    let extra = &clones[0];
    let mut extra_header = extra.header;
    extra_header.ids = used.new_ids(
        create::IdForm::of(&extra.header),
        extra.header.type_id,
        &extra.header.ids,
        "corpus extra",
    );
    ws.add_resources(
        &game,
        "fill the new stream",
        &[NewResourceEdit {
            stream: name.into(),
            header: extra_header,
            data: extra.data.clone(),
            storage: StorageChoice::Refpack,
            before: None,
        }],
    )
    .unwrap();
    assert_eq!(snapshot(), originals, "recording does not touch the game");

    ws.apply(&game, ApplyOptions::default()).unwrap();
    assert!(ws.status(&game).unwrap().is_up_to_date());
    let stream = game.open_stream(s).unwrap();
    let layout = stream.layout();
    assert_eq!(layout.resources.len(), before_data.len() + clones.len());
    for (i, data) in before_data.iter().enumerate() {
        assert_eq!(&stream.resource_data(i).unwrap(), data, "resource {i}");
    }
    for (k, clone) in clones.iter().enumerate() {
        let i = before_data.len() + k;
        assert_eq!(layout.resources[i].header, clone.header);
        assert_eq!(stream.resource_data(i).unwrap(), clone.data);
        if clone.header.type_id == TYPE_METAMODEL {
            doctor_toolkit::resource::metamodel::MetaModel::parse(&clone.data).unwrap();
        }
    }
    drop(stream);
    // The clones refer to clones, and nothing new is unresolved.
    assert!(!remap.is_empty());
    // (A cloned material repeats the original's references to engine textures.)
    let after: std::collections::BTreeSet<_> = create::check_references(&game, s)
        .unwrap()
        .into_iter()
        .map(|u| u.target)
        .collect();
    assert!(after.is_subset(&baseline), "{after:?} vs {baseline:?}");
    for (t, toc_file) in game.tocs().iter().enumerate() {
        let table = game.load_toc(t).unwrap();
        assert!(table.is_sorted_by_hash());
        let size = |rel: &str| {
            StreamLayout::parse(&std::fs::read(root.join(rel)).unwrap())
                .unwrap()
                .total_data_size()
        };
        let entry = table.find_file(name).unwrap();
        assert_eq!(
            table.files[entry].data_size as u64,
            size("gf2/doctor/corpus_test.str")
        );
        let source_entry = table.find_file(&source_name).unwrap();
        assert_eq!(
            table.files[source_entry].data_size as u64,
            size(&game.streams()[s].rel_path),
            "{}",
            toc_file.rel_path
        );
        assert!(
            table
                .file_deps(source_entry)
                .any(|n| n.name_lossy() == "gf2\\doctor\\corpus_test")
        );
        // The table is the original plus one name, one entry and dependency words.
        let parsed = Toc::parse(&table.to_bytes().unwrap()).unwrap();
        assert_eq!(parsed, table);
    }

    let new_file = root.join("gf2/doctor/corpus_test.str");
    let applied = (snapshot(), std::fs::read(&new_file).unwrap());
    ws.unapply(&game, false).unwrap();
    assert_eq!(snapshot(), originals);
    assert!(!new_file.exists());

    // The same changes as a mod (format version 3, with added resources and a
    // new stream): installed and applied by the mod manager they give the same
    // files, byte for byte.
    use doctor_toolkit::mods::manager::ModApplyOptions;
    use doctor_toolkit::mods::{self, ModArchive, ModInfo, ModManager};
    let package = temp.path().join("clones.zip");
    let info = ModInfo {
        id: "clones".into(),
        name: "Clones".into(),
        version: "1".into(),
        ..ModInfo::default()
    };
    let built = mods::build_from_workspace(&ws, &info, None, None, &package).unwrap();
    assert_eq!(built.added, clones.len() + 1);
    assert_eq!(built.new_streams, 1);
    let mut archive = ModArchive::open(&package).unwrap();
    archive.verify().unwrap();
    assert_eq!(archive.manifest().format_version, 3);
    let mut manager = ModManager::open(&game).unwrap();
    let report = manager.install_file(&game, &package, true).unwrap();
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert!(manager.clashes(true).is_empty());
    manager.apply(&game, ModApplyOptions::default()).unwrap();
    assert_eq!((snapshot(), std::fs::read(&new_file).unwrap()), applied);
    assert!(
        manager
            .status(&game)
            .unwrap()
            .streams
            .iter()
            .all(|s| s.up_to_date)
    );
    // Checked again with the created stream on disk, the mod still fits.
    let reopened = GameInstance::open(root).unwrap();
    assert!(manager.check(&reopened).unwrap().is_empty());
    manager.unapply(&reopened, false).unwrap();
    assert_eq!(snapshot(), originals);
    assert!(!new_file.exists());
}

/// A metamodel whose resource references all point into its own stream,
/// followed by the indices of those resources.
fn self_contained_metamodel(stream: &StreamFile) -> Option<Vec<usize>> {
    use doctor_toolkit::create::{self, TYPE_METAMODEL};
    let layout = stream.layout();
    for (m, resource) in layout.resources.iter().enumerate() {
        if resource.header.type_id != TYPE_METAMODEL {
            continue;
        }
        let data = stream.resource_data(m).ok()?;
        let targets = create::references(TYPE_METAMODEL, &data).ok()?;
        let local: Vec<usize> = targets
            .iter()
            .filter_map(|t| match t {
                create::Target::Guid(g) => layout.resources.iter().position(|r| r.header.ids == *g),
                _ => None,
            })
            .collect();
        let has_mesh = local
            .iter()
            .any(|&i| layout.resources[i].header.type_id == create::TYPE_MESH);
        if has_mesh && local.len() == targets.len() {
            let mut out = vec![m];
            out.extend(local);
            return Some(out);
        }
    }
    None
}

#[test]
fn game_index_covers_every_resource_and_resolves_references() {
    use doctor_toolkit::navigation::GameIndex;

    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let (plain, errors) = game.build_index(|_| {});
    assert!(errors.is_empty());
    let started = std::time::Instant::now();
    let index = GameIndex::build(&game, &|_, _| {});
    let built = started.elapsed();
    assert_eq!(index.len(), plain.entries.len());
    assert_eq!(index.len(), 212_544);
    for entry in &plain.entries {
        let id = index
            .locate(entry.stream as usize, entry.resource as usize)
            .expect("every resource has a record");
        let record = index.record(id);
        assert_eq!(record.header, entry.header);
        assert_eq!(index.find_key(&index.key(id)), Some(id));
    }
    assert_eq!(index.record_errors(), 0, "every parsed type parses");

    // The third id of a texture is the CRC-32 of its package name.
    let tg4h = type_tags().resolve("tg4h").unwrap();
    for id in index.ids() {
        let record = index.record(id);
        if record.header.type_id == tg4h {
            let package = record
                .names
                .iter()
                .find(|(kind, _)| *kind == doctor_toolkit::navigation::NameKind::Package)
                .map(|(_, name)| name.to_ascii_lowercase())
                .unwrap();
            assert_eq!(
                record.header.ids[2],
                doctor_toolkit::hash::crc32(package.as_bytes()),
                "{}",
                index.key(id)
            );
        }
    }

    let rate = |tag: &str| {
        let type_id = type_tags().resolve(tag).unwrap();
        let stats = index
            .resolve_stats()
            .into_iter()
            .find(|s| s.type_id == type_id)
            .unwrap();
        eprintln!(
            "{tag}: {} resources, {} references, {} resolved ({:.1}%)",
            stats.records,
            stats.refs,
            stats.resolved,
            stats.resolved as f64 * 100.0 / stats.refs as f64
        );
        stats.resolved as f64 / stats.refs as f64
    };
    assert_eq!(rate("tg4h"), 1.0);
    assert_eq!(rate("ffn"), 1.0);
    assert!(rate("mesh") > 0.99);
    assert!(rate("metamodel") > 0.99);
    // Metamodel references resolve by GUID: every one does except the 102
    // `emx` sounds, a type that is in no stream.
    let metamodel = type_tags().resolve("metamodel").unwrap();
    let emx = type_tags().resolve("emx").unwrap();
    let mut unresolved = std::collections::BTreeMap::new();
    for id in index.ids() {
        if index.record(id).header.type_id != metamodel {
            continue;
        }
        for (r, targets) in index.uses(id) {
            if targets.is_empty() {
                let doctor_toolkit::navigation::Target::Resource { type_id, .. } = r.target else {
                    panic!("metamodel reference to {:?}", r.target);
                };
                *unresolved.entry(type_id).or_insert(0usize) += 1;
            }
        }
    }
    assert_eq!(
        unresolved,
        std::collections::BTreeMap::from([(emx, 102)]),
        "unresolved metamodel references by type"
    );
    assert!(rate("simgroup") > 0.80);
    // The rest are a few placeholder textures (`g_materialSpecMap` e9930443, …)
    // that are in no stream.
    assert!(rate("material") > 0.79);
    // All types together: a reference that stops resolving is a regression.
    let stats = index.resolve_stats();
    let refs: usize = stats.iter().map(|s| s.refs).sum();
    let resolved: usize = stats.iter().map(|s| s.resolved).sum();
    eprintln!(
        "all: {refs} references, {resolved} resolved ({:.1}%)",
        resolved as f64 * 100.0 / refs as f64
    );
    assert_eq!(refs, 492_214);
    assert!(resolved >= 432_949, "{resolved} resolved");

    // The cache gives the same index.
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("index.bin");
    index.save(&cache).unwrap();
    let size = std::fs::metadata(&cache).unwrap().len();
    let started = std::time::Instant::now();
    let (cached, report) = GameIndex::open(&game, Some(&cache), &|_, _| {});
    eprintln!(
        "index: built in {:.2} s, loaded from a {:.1} MiB cache in {:.2} s",
        built.as_secs_f64(),
        size as f64 / (1 << 20) as f64,
        started.elapsed().as_secs_f64()
    );
    assert_eq!(report.reused, game.streams().len());
    assert_eq!(report.rebuilt, 0);
    assert_eq!(cached.len(), index.len());
    for id in index.ids() {
        assert_eq!(cached.record(id), index.record(id));
    }
}

#[test]
fn dictionary_names_the_hashes_of_the_game() {
    use doctor_toolkit::navigation::dictionary::unknown_hashes;
    use doctor_toolkit::navigation::{Dictionary, GameIndex};
    use std::collections::{BTreeMap, HashSet};

    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let index = GameIndex::build(&game, &|_, _| {});
    let dict = Dictionary::for_game(&index);
    // Places of the hashes of records (bones, classes, string entries, ...),
    // counted as `unknown_hashes` counts them: once per resource.
    let mut places = 0;
    for id in index.ids() {
        let distinct: HashSet<_> = index.record(id).hashes.iter().copied().collect();
        places += distinct.len();
    }
    let mut unknown: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for u in unknown_hashes(&index, &dict) {
        let entry = unknown.entry(u.kind).or_default();
        entry.0 += 1;
        entry.1 += u.count;
    }
    let get = |kind: &str| unknown.get(kind).copied().unwrap_or_default();
    let unnamed: usize = ["string entry", "SimGroup class", "shader", "bone"]
        .iter()
        .chain(&["parts database part", "metamodel name"])
        .map(|k| get(k).1)
        .sum();
    eprintln!(
        "record hashes: {places} places, {} named ({:.1}%); unknown: {unknown:?}",
        places - unnamed,
        (places - unnamed) as f64 * 100.0 / places as f64
    );
    // Stage 40: string entries named by the entry-name table, bones and
    // classes confirmed by hash (only names that occur in the game data), the build paths of `smb` and `sbk`, the tags
    // `treemodel`, `covernodetable`, `hka` and `emx`. Before it: 7 963 string
    // entries, 85 bones, 50 classes, 59 + 8 ids of `smb` and `sbk`.
    assert!(get("string entry").0 <= 507, "{:?}", get("string entry"));
    assert!(get("bone").0 <= 82, "{:?}", get("bone"));
    assert!(get("SimGroup class").0 <= 42, "{:?}", get("SimGroup class"));
    assert_eq!(get("shader"), (0, 0));
    assert_eq!(get("smb id"), (0, 0));
    assert_eq!(get("sbk id"), (0, 0));
    assert_eq!(get("type tag"), (0, 0));
    assert!(unnamed <= 23_063, "{unnamed} unnamed places");
}

/// A directory of symbolic links to some files of the game: writes replace
/// the links, never the game files.
#[cfg(unix)]
fn shadow_game(dir: &std::path::Path, files: &[&str]) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    for rel in files {
        let link = temp.path().join(rel);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(dir.join(rel), &link).unwrap();
    }
    temp
}

#[cfg(unix)]
#[test]
fn unpack_then_pack_records_exactly_the_edited_files() {
    use doctor_toolkit::navigation::GameIndex;
    use doctor_toolkit::navigation::unpack::{self, FileKind, Manifest, PackOptions};
    use doctor_toolkit::resource::pixels;
    use doctor_toolkit::workspace::Workspace;
    use std::sync::atomic::AtomicBool;

    let Some(dir) = game_dir() else { return };
    const FILES: [&str; 4] = [
        "gf2/frontend.str",
        "gf2/frontend.toc",
        "gf2/frontend_global.toc",
        "gf2/text/dd7d7050.str",
    ];
    let originals: Vec<Vec<u8>> = FILES
        .iter()
        .map(|rel| std::fs::read(dir.join(rel)).unwrap())
        .collect();
    let shadow = shadow_game(&dir, &FILES);
    let game = GameInstance::open(shadow.path()).unwrap();
    let index = GameIndex::build(&game, &|_, _| {});
    let out = tempfile::tempdir().unwrap();
    let selection = unpack::select(&game, &[], &[]).unwrap();
    let report = unpack::unpack(
        &game,
        Some(&index),
        &selection,
        out.path(),
        Default::default(),
        &|_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    let manifest = Manifest::read(out.path()).unwrap();
    let covered: std::collections::HashSet<_> =
        manifest.files.iter().flat_map(|f| f.keys.clone()).collect();
    assert_eq!(
        covered.len(),
        index.len(),
        "every resource is in the manifest"
    );

    // No edits: nothing to record, no workspace.
    let options = PackOptions::default();
    let packed = unpack::pack(&game, out.path(), &options).unwrap();
    assert!(packed.changed.is_empty());
    assert_eq!(packed.unchanged, manifest.files.len());
    assert!(packed.workspace.is_none());
    assert!(!Workspace::exists(&Workspace::default_dir(game.root())));

    // Edit one texture and one string.
    let texture = manifest
        .files
        .iter()
        .find(|f| f.kind == FileKind::TexturePng && f.path.contains("/buttons."))
        .unwrap();
    let path = out.path().join(&texture.path);
    let (w, h, mut rgba) = pixels::decode_png(&std::fs::read(&path).unwrap()).unwrap();
    for px in rgba.chunks_mut(4) {
        px[1] = 255 - px[1];
    }
    std::fs::write(&path, pixels::encode_png(w, h, &rgba).unwrap()).unwrap();
    let strings = manifest
        .files
        .iter()
        .find(|f| f.kind == FileKind::Strings)
        .unwrap();
    let path = out.path().join(&strings.path);
    let mut document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let column = document["languages"][0].as_str().unwrap().to_owned();
    document["entries"][3][&column] = "doctor_toolkit pack test".into();
    std::fs::write(&path, serde_json::to_vec_pretty(&document).unwrap()).unwrap();

    // A changed view or an unknown file stops the whole pack.
    let view = manifest
        .files
        .iter()
        .find(|f| f.kind == FileKind::View)
        .unwrap();
    let view_path = out.path().join(&view.path);
    let view_bytes = std::fs::read(&view_path).unwrap();
    std::fs::write(&view_path, b"edited").unwrap();
    let extra = out.path().join("gf2/frontend/new_texture.png");
    std::fs::write(&extra, b"new").unwrap();
    let error = unpack::pack(&game, out.path(), &options)
        .unwrap_err()
        .to_string();
    assert!(error.contains("view for reading only"), "{error}");
    assert!(error.contains("not in the manifest"), "{error}");
    assert!(!Workspace::exists(&Workspace::default_dir(game.root())));
    std::fs::write(&view_path, view_bytes).unwrap();
    std::fs::remove_file(&extra).unwrap();

    let packed = unpack::pack(&game, out.path(), &options).unwrap();
    let paths: Vec<&str> = packed.changed.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, [texture.path.as_str(), strings.path.as_str()]);
    let ws = Workspace::open(packed.workspace.unwrap()).unwrap();
    let mut changed: Vec<_> = ws.changes().map(|c| c.key.clone()).collect();
    changed.sort();
    // The texture header is unchanged by an image of the same shape.
    let mut expected = vec![texture.keys[1].clone(), strings.keys[0].clone()];
    expected.sort();
    assert_eq!(changed, expected);
    let table = doctor_toolkit::resource::lh2::StringTable::parse(
        &ws.data(&strings.keys[0]).unwrap().unwrap(),
    )
    .unwrap();
    assert_eq!(table.strings[0][3], "doctor_toolkit pack test");

    // Packing again records nothing new.
    let head = ws.head();
    let again = unpack::pack(&game, out.path(), &options).unwrap();
    assert!(again.changed.iter().all(|f| !f.recorded));
    assert_eq!(Workspace::open(ws.dir()).unwrap().head(), head);

    for (rel, original) in FILES.iter().zip(&originals) {
        assert_eq!(&std::fs::read(dir.join(rel)).unwrap(), original, "{rel}");
    }
}

#[test]
fn every_movie_parses_rebuilds_and_decodes() {
    use doctor_toolkit::resource::video::{self, Movie, container};
    let Some(dir) = game_dir() else { return };
    let movies = video::game_movies(&dir).unwrap();
    let failures = Failures::default();
    let stats = Mutex::new((0usize, 0usize, 0usize, 0f64));
    movies.par_iter().for_each(|m| {
        let check = || -> Result<(), String> {
            let data = std::fs::read(&m.path).map_err(|e| e.to_string())?;
            if video::is_aiff(&data) {
                let pcm = video::decode_audio(&data)
                    .map_err(|e| e.to_string())?
                    .unwrap();
                if pcm.frames() == 0 {
                    return Err("empty AIFF".into());
                }
                stats.lock().unwrap().2 += 1;
                return Ok(());
            }
            let movie = Movie::parse(&data).map_err(|e| e.to_string())?;
            if container::write_chunks(&movie.chunks) != data {
                return Err("chunks do not rebuild the file".into());
            }
            let v = movie.video.ok_or("no video header")?;
            if &v.codec != b"vp60" || (v.width, v.height) != (1280, 720) {
                return Err(format!("video {:?} {}x{}", v.codec, v.width, v.height));
            }
            let frames = movie.frames().count() as i64;
            if frames != v.frame_count as i64 {
                return Err(format!("{frames} frames, header {}", v.frame_count));
            }
            // The first frame decodes (Huffman-coded key frame).
            let (shown, pic) = video::preview_frame(&data, 0).map_err(|e| e.to_string())?;
            if shown != 0 || (pic.width, pic.height) != (1280, 720) {
                return Err("first frame".into());
            }
            if let Some(a) = &movie.audio {
                let pcm = movie.decode_audio().map_err(|e| e.to_string())?.unwrap();
                let sccl = movie.chunks.iter().find(|c| c.tag == container::SCCL);
                let scdl = movie
                    .chunks
                    .iter()
                    .filter(|c| c.tag == container::SCDL)
                    .count();
                if let Some(c) = sccl
                    && u32::from_be_bytes(c.body.try_into().unwrap()) as usize != scdl
                {
                    return Err("SCCl count".into());
                }
                if pcm.frames() + 28 * 64 < a.sample_count as usize {
                    return Err(format!("{} of {} samples", pcm.frames(), a.sample_count));
                }
                let mut s = stats.lock().unwrap();
                s.1 += 1;
                s.3 += pcm.frames() as f64 / pcm.sample_rate as f64;
            }
            stats.lock().unwrap().0 += 1;
            Ok(())
        };
        if let Err(e) = check() {
            failures.push(format!("{}: {e}", m.rel_path));
        }
    });
    failures.assert_empty();
    let (ea, with_audio, aiff, seconds) = stats.into_inner().unwrap();
    eprintln!("movies: {ea} EA ({with_audio} with audio, {seconds:.0} s), {aiff} AIFF");
    assert_eq!((ea, aiff), (58, 1));
}

#[test]
fn encoded_movie_frames_from_game_pictures_decode_exactly() {
    use doctor_toolkit::resource::video::{self, vp6};
    let Some(dir) = game_dir() else { return };
    let m = video::find_movie(&dir, "common/mainmenu.vp6").unwrap();
    let data = std::fs::read(&m.path).unwrap();
    // Key frames 0, 30 and 60 of the menu background, coded as a short movie.
    let pictures: Vec<vp6::Picture> = [0, 30, 60]
        .iter()
        .map(|&i| video::preview_frame(&data, i).unwrap().1.flipped())
        .collect();
    let mut enc = vp6::Encoder::new(1280, 720, vp6::EncoderConfig::default());
    let mut dec = vp6::Decoder::new();
    let mut bytes = 0;
    for (i, p) in pictures.iter().enumerate() {
        let f = enc.encode(p);
        bytes += f.data.len();
        let out = dec.decode(&f.data).unwrap();
        assert_eq!(out, enc.reconstruction().unwrap());
        let psnr = out.psnr_y(p);
        eprintln!(
            "frame {i}: key {}, {} bytes, PSNR {psnr:.2} dB",
            f.key,
            f.data.len()
        );
        assert!(psnr > 40.0, "frame {i}: {psnr}");
    }
    assert!(bytes < 600_000, "{bytes}");
}

/// The full VP6 decoder on every frame of every movie: no errors, no
/// concealed macroblocks; prints what the streams use and the speed.
#[test]
fn every_movie_decodes_every_frame() {
    use doctor_toolkit::resource::video::{self, Movie, vp6};
    let Some(dir) = game_dir() else { return };
    let movies = video::game_movies(&dir).unwrap();
    let failures = Failures::default();
    #[derive(Default)]
    struct Stats {
        movies: usize,
        frames: usize,
        keys: usize,
        huffman_inter: usize,
        range_inter: usize,
        golden: usize,
        deblock: usize,
        bicubic: usize,
        mb_types: [u64; 10],
        seconds: f64,
    }
    let stats = Mutex::new(Stats::default());
    movies.par_iter().for_each(|m| {
        let check = || -> Result<(), String> {
            let data = std::fs::read(&m.path).map_err(|e| e.to_string())?;
            if video::is_aiff(&data) {
                return Ok(());
            }
            let movie = Movie::parse(&data).map_err(|e| e.to_string())?;
            let mut dec = vp6::Decoder::new();
            let mut local = Stats::default();
            let start = std::time::Instant::now();
            for (i, (key, frame)) in movie.frames().enumerate() {
                dec.decode(frame).map_err(|e| format!("frame {i}: {e}"))?;
                let s = dec.stats();
                if s.key != key {
                    return Err(format!("frame {i}: chunk and frame disagree on key"));
                }
                local.frames += 1;
                local.keys += key as usize;
                if !key {
                    if s.huffman {
                        local.huffman_inter += 1;
                    } else {
                        local.range_inter += 1;
                    }
                    local.golden += s.golden as usize;
                }
                local.deblock += s.deblock as usize;
                local.bicubic += (s.filter_mode != 0) as usize;
                for (a, b) in local.mb_types.iter_mut().zip(s.mb_types) {
                    *a += b as u64;
                }
            }
            let seconds = start.elapsed().as_secs_f64();
            let mut all = stats.lock().unwrap();
            all.movies += 1;
            all.frames += local.frames;
            all.keys += local.keys;
            all.huffman_inter += local.huffman_inter;
            all.range_inter += local.range_inter;
            all.golden += local.golden;
            all.deblock += local.deblock;
            all.bicubic += local.bicubic;
            for (a, b) in all.mb_types.iter_mut().zip(local.mb_types) {
                *a += b;
            }
            all.seconds += seconds;
            Ok(())
        };
        if let Err(e) = check() {
            failures.push(format!("{}: {e}", m.rel_path));
        }
    });
    failures.assert_empty();
    let s = stats.into_inner().unwrap();
    eprintln!(
        "VP6: {} movies, {} frames ({} key; inter: {} Huffman, {} range-coded, {} golden), \
         deblocking {}, bicubic {}; {:.0} frames/s per thread",
        s.movies,
        s.frames,
        s.keys,
        s.huffman_inter,
        s.range_inter,
        s.golden,
        s.deblock,
        s.bicubic,
        s.frames as f64 / s.seconds.max(1e-9)
    );
    eprintln!("macroblock types: {:?}", s.mb_types);
    assert_eq!(s.movies, 58);
}

/// Every frame of a sample of movies against ffmpeg's VP6 decoder (YUV
/// planes byte for byte). Skipped without `ffmpeg` on `PATH` (or `FFMPEG`).
#[test]
fn vp6_decoder_matches_ffmpeg() {
    use doctor_toolkit::resource::video::{self, Movie, vp6};
    use std::io::Read;
    let Some(dir) = game_dir() else { return };
    let ffmpeg = std::env::var("FFMPEG").unwrap_or_else(|_| "ffmpeg".into());
    if std::process::Command::new(&ffmpeg)
        .arg("-version")
        .output()
        .is_err()
    {
        eprintln!("ffmpeg not found: skipped");
        return;
    }
    let names = [
        "common/mainmenu.vp6",
        "common/attract.vp6",
        "common/M6FMV.vp6",
        "en/load.vp6",
        "en/FMV01.vp6",
        "en/TRN07.vp6",
        "en/TRN03.vp6",
        "es/TRN10.vp6",
        "ru/FMV02.vp6",
    ];
    let failures = Failures::default();
    let compared = Mutex::new(0usize);
    names.par_iter().for_each(|name| {
        let check = || -> Result<usize, String> {
            let m = video::find_movie(&dir, name).map_err(|e| e.to_string())?;
            let data = std::fs::read(&m.path).map_err(|e| e.to_string())?;
            let movie = Movie::parse(&data).map_err(|e| e.to_string())?;
            let mut child = std::process::Command::new(&ffmpeg)
                .args(["-v", "error", "-threads", "1", "-i"])
                .arg(&m.path)
                .args([
                    "-map",
                    "0:v:0",
                    "-fps_mode",
                    "passthrough",
                    "-f",
                    "rawvideo",
                ])
                .args(["-pix_fmt", "yuv420p", "-"])
                .stdout(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| e.to_string())?;
            let mut out = child.stdout.take().unwrap();
            let mut dec = vp6::Decoder::new();
            let mut theirs = Vec::new();
            let mut frames = 0;
            for (i, (_, frame)) in movie.frames().enumerate() {
                let ours = dec
                    .decode(frame)
                    .map_err(|e| format!("frame {i}: {e}"))?
                    .flipped();
                let size = ours.planes.iter().map(Vec::len).sum::<usize>();
                theirs.resize(size, 0);
                out.read_exact(&mut theirs)
                    .map_err(|e| format!("ffmpeg gave {i} frames: {e}"))?;
                let mut at = 0;
                for (p, plane) in ours.planes.iter().enumerate() {
                    let other = &theirs[at..at + plane.len()];
                    if plane[..] != other[..] {
                        let diff = plane.iter().zip(other).filter(|(a, b)| a != b).count();
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(format!(
                            "frame {i} ({}): plane {p} differs in {diff} bytes",
                            if dec.stats().key { "key" } else { "inter" }
                        ));
                    }
                    at += plane.len();
                }
                frames += 1;
            }
            let _ = child.kill();
            let _ = child.wait();
            Ok(frames)
        };
        match check() {
            Ok(n) => *compared.lock().unwrap() += n,
            Err(e) => failures.push(format!("{name}: {e}")),
        }
    });
    failures.assert_empty();
    eprintln!(
        "{} frames of {} movies equal to ffmpeg",
        compared.into_inner().unwrap(),
        names.len()
    );
}

/// EA-XAS encoder on the game's own sounds: every `sbk` sound and every 100th
/// archived stream is decoded, encoded again in its own format and decoded;
/// the round trip must stay close to the game's decoded audio, a second round
/// trip must be stable, and putting a sound's own data back into its bank must
/// give the original bank.
#[test]
fn encoder_round_trips_game_sounds() {
    use doctor_toolkit::container::big::{BigArchive, find_archives};
    use doctor_toolkit::resource::audio::{Sample, parse_sbk};
    use doctor_toolkit::resource::audio_encode::{
        SampleSpec, build_inline, build_snu, quality, replace_sbk_sound,
    };
    use doctor_toolkit::resource::video::input::Pcm;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Stats {
        sounds: usize,
        samples: usize,
        snr: Vec<f64>,
        psnr: Vec<f64>,
        second: Vec<f64>,
        noise: f64,
        signal: f64,
    }
    let stats = Mutex::new(Stats::default());
    let check = |sample: &Sample<'_>, snu: Option<&[u8]>| -> Result<(), String> {
        let h = sample.header;
        let pcm = Pcm {
            channels: h.channels as u16,
            sample_rate: h.sample_rate,
            samples: sample.decode().map_err(|e| e.to_string())?,
        };
        let spec = SampleSpec::like(&h);
        let encoded = match snu {
            Some(t) => build_snu(&pcm, &spec, Some(t)),
            None => build_inline(&pcm, &spec),
        }
        .map_err(|e| e.to_string())?;
        let again = match snu {
            Some(_) => Sample::parse_snu(&encoded),
            None => Sample::parse_inline(&encoded, 0),
        }
        .map_err(|e| e.to_string())?;
        if again.header != h {
            return Err(format!("header {:?} != {:?}", again.header, h));
        }
        // The SNU header and metadata written for the same sound are the game's.
        if let Some(t) = snu
            && t[0] == 3
        {
            let start = u32::from_le_bytes(t[8..12].try_into().unwrap()) as usize;
            if encoded.get(..start) != t.get(..start) {
                return Err(format!(
                    "SNU header {:02x?} != {:02x?}",
                    &encoded[..0x20],
                    &t[..0x20]
                ));
            }
        }
        let y = again.decode().map_err(|e| e.to_string())?;
        if y.len() != pcm.samples.len() {
            return Err(format!(
                "{} samples instead of {}",
                y.len(),
                pcm.samples.len()
            ));
        }
        let q = quality(&pcm.samples, &y);
        let second = Pcm {
            samples: y.clone(),
            ..pcm.clone()
        };
        let y2 = build_inline(&second, &spec)
            .and_then(|d| Sample::parse_inline(&d, 0)?.decode())
            .map_err(|e| e.to_string())?;
        let q2 = quality(&y, &y2);
        let noise: f64 = pcm
            .samples
            .iter()
            .zip(&y)
            .map(|(&a, &b)| (a as f64 - b as f64).powi(2))
            .sum();
        let signal: f64 = pcm.samples.iter().map(|&a| (a as f64).powi(2)).sum();
        let mut guard = stats.lock().unwrap();
        let s = &mut *guard;
        s.sounds += 1;
        s.samples += pcm.samples.len();
        for (v, x) in [
            (&mut s.snr, q.snr_db),
            (&mut s.psnr, q.psnr_db),
            (&mut s.second, q2.psnr_db),
        ] {
            if x.is_finite() {
                v.push(x);
            }
        }
        s.noise += noise;
        s.signal += signal;
        // Near-silent sounds aside, the round trip keeps at least 20 dB of SNR.
        if signal / (pcm.samples.len() as f64) > 1e4 && q.snr_db < 20.0 {
            return Err(format!("SNR {:.1} dB", q.snr_db));
        }
        Ok(())
    };
    let banks = for_each_resource("sbk", |_, data| {
        let sounds = parse_sbk(data).map_err(|e| e.to_string())?;
        for (i, sound) in sounds.iter().enumerate() {
            check(&sound.sample, None)?;
            let own = &data[sound.sample.range.clone()];
            if replace_sbk_sound(data, i, own).map_err(|e| e.to_string())? != data {
                return Err(format!("sound {i}: putting it back changes the bank"));
            }
        }
        Ok(())
    });
    let Some(dir) = game_dir() else { return };
    let failures = Failures::default();
    for path in find_archives(&dir).unwrap() {
        let archive = BigArchive::open(&path).unwrap();
        archive
            .entries
            .par_iter()
            .enumerate()
            .filter(|(i, _)| i % 100 == 0)
            .for_each(|(_, entry)| {
                let data = archive.read(entry).unwrap();
                let sample = Sample::parse_snu(&data).unwrap();
                if let Err(e) = check(&sample, Some(&data)) {
                    failures.push(format!("{}: {e}", entry.name));
                }
            });
    }
    failures.assert_empty();
    let mut stats = stats.into_inner().unwrap();
    let s = &mut stats;
    for v in [&mut s.snr, &mut s.psnr, &mut s.second] {
        v.sort_by(f64::total_cmp);
    }
    let median = |v: &[f64]| v.get(v.len() / 2).copied().unwrap_or(f64::NAN);
    let min = |v: &[f64]| v.first().copied().unwrap_or(f64::NAN);
    eprintln!(
        "encoder: {banks} banks, {} sounds, {} samples; SNR median {:.1} dB (min {:.1}), \
         overall {:.1} dB; PSNR median {:.1} dB (min {:.1}); second generation PSNR \
         median {:.1} dB (min {:.1})",
        s.sounds,
        s.samples,
        median(&s.snr),
        min(&s.snr),
        10.0 * (s.signal / s.noise).log10(),
        median(&s.psnr),
        min(&s.psnr),
        median(&s.second),
        min(&s.second),
    );
    assert!(median(&s.snr) > 25.0);
    assert!(median(&s.second) > median(&s.psnr));
}

/// EA-XAS encoder on audio that never was EA-XAS: the soundtracks of the
/// movies (EA-XA, 48 kHz) and the AIFF `logo.vp6`, encoded as the game's
/// streams are (32 kHz) and at their own rate.
#[test]
fn encoder_quality_on_movie_soundtracks() {
    use doctor_toolkit::resource::audio_encode::{
        SampleSpec, build_inline, decode_inline, fit_pcm, quality,
    };
    use doctor_toolkit::resource::video::{decode_audio, game_movies};
    let Some(dir) = game_dir() else { return };
    let mut seen = std::collections::HashSet::new();
    let movies: Vec<_> = game_movies(&dir)
        .unwrap()
        .into_iter()
        .filter(|m| seen.insert(m.len))
        .collect();
    let results: Vec<(String, u32, f64, f64)> = movies
        .par_iter()
        .flat_map_iter(|m| {
            let data = std::fs::read(&m.path).unwrap();
            let Some(pcm) = decode_audio(&data).unwrap() else {
                return Vec::new();
            };
            // The first 20 seconds.
            let keep = (pcm.sample_rate as usize * 20).min(pcm.frames()) * pcm.channels as usize;
            let pcm = doctor_toolkit::resource::video::input::Pcm {
                samples: pcm.samples[..keep].to_vec(),
                ..pcm
            };
            [pcm.sample_rate, 32000]
                .into_iter()
                .map(|rate| {
                    let fitted = fit_pcm(&pcm, pcm.channels as u8, rate);
                    let spec = SampleSpec {
                        channels: fitted.channels as u8,
                        sample_rate: rate,
                        kind: 1,
                        loop_start: None,
                    };
                    let y = decode_inline(&build_inline(&fitted, &spec).unwrap()).unwrap();
                    let q = quality(&fitted.samples, &y);
                    (m.rel_path.clone(), rate, q.snr_db, q.psnr_db)
                })
                .collect()
        })
        .collect();
    for (name, rate, snr, psnr) in &results {
        eprintln!("{name} at {rate} Hz: SNR {snr:.1} dB, PSNR {psnr:.1} dB");
        assert!(*snr > 20.0, "{name}: {snr}");
    }
    assert!(results.len() >= 10);
}

/// Stage 37: the collision of levels placed in the world. Every placement
/// of the three cities reads; terrain objects (`ZoneRenderLOD`) of the
/// `medium` zones carry collision (the ground of the streets).
#[test]
fn level_collision_is_placed_in_the_world() {
    use doctor_toolkit::level::{Catalog, Level, LevelSource, Lookup, Overrides, Scope};
    use std::collections::HashMap;
    use std::sync::Arc;

    let Some(dir) = game_dir() else { return };
    let game = Arc::new(GameInstance::open(&dir).unwrap());
    let (index, _) = game.build_index(|_| {});
    let catalog = Catalog::new(&game, &index);
    let lookup = Arc::new(Lookup::new(&index));
    let parents = game.stream_parents().unwrap();
    let failures = Failures::default();
    // (placements, of terrain objects) by the kind of zone.
    let mut by_kind: HashMap<&str, (usize, usize)> = HashMap::new();
    for name in ["gf2/miami", "gf2/cuba", "gf2/queens"] {
        let streams: Vec<usize> = catalog.under(name).map(|z| z.stream).collect();
        let scope = Scope::new(&game, &parents, &streams);
        let source = Arc::new(LevelSource::new(
            game.clone(),
            lookup.clone(),
            Overrides::default(),
            scope,
        ));
        let level = Level::load(source.clone(), &streams).unwrap();
        let placements = level.collision_placements();
        let mut ids: Vec<u32> = placements.iter().map(|p| p.hkx).collect();
        ids.sort_unstable();
        ids.dedup();
        ids.par_iter().for_each(|&id| {
            if let Err(error) = source.physics(id) {
                failures.push(format!("{name}: hkx {id:08x}: {error}"));
            }
        });
        for p in &placements {
            let group = &level.groups[p.group];
            let object = &group.objects[p.object];
            let stream = game.streams()[group.location.stream].stream_name();
            let kind = ["/high/", "/medium/", "/low/", "/interior/"]
                .into_iter()
                .find(|k| stream.contains(k))
                .unwrap_or("other");
            let entry = by_kind.entry(kind).or_default();
            entry.0 += 1;
            entry.1 += usize::from(object.class_name == Some("ZoneRenderLOD"));
            // The placement is the part's matrix in the object's.
            assert_eq!(p.matrix[3][3], 1.0);
        }
    }
    failures.assert_empty();
    let mut kinds: Vec<_> = by_kind.iter().collect();
    kinds.sort();
    eprintln!("collision placements (all, of terrain) by zone kind: {kinds:?}");
    // How much of the streets each kind covers is measured with the
    // physics of the editor (`level_view::walk::build` tests): high zones
    // alone hold up the player under few of their objects.
    let medium = by_kind.get("/medium/").copied().unwrap_or_default();
    assert!(
        medium.1 > 2000,
        "terrain collision in medium zones: {medium:?}"
    );
    assert!(by_kind.values().map(|k| k.0).sum::<usize>() > 30_000);
}

#[test]
fn every_level_loads_and_shows_its_models() {
    use doctor_toolkit::level::{Catalog, Level, LevelSource, Lookup, Overrides, Scope};
    use std::sync::Arc;

    let Some(dir) = game_dir() else { return };
    let game = Arc::new(GameInstance::open(&dir).unwrap());
    let (index, errors) = game.build_index(|_| {});
    assert!(errors.is_empty());
    let catalog = Catalog::new(&game, &index);
    assert_eq!(catalog.zones.len(), 2006, "one zone per SimGroup stream");
    let lookup = Arc::new(Lookup::new(&index));
    let parents = game.stream_parents().unwrap();
    let failures = Failures::default();
    let (mut objects, mut with_model, mut rendered, mut points) = (0, 0, 0, 0);
    let mut meshes = std::collections::HashSet::new();
    let mut collisions = std::collections::HashSet::new();
    let mut with_collision = 0;
    for name in catalog.levels() {
        let streams: Vec<usize> = catalog.under(name).map(|z| z.stream).collect();
        let scope = Scope::new(&game, &parents, &streams);
        let source = Arc::new(LevelSource::new(
            game.clone(),
            lookup.clone(),
            Overrides::default(),
            scope,
        ));
        let level = match Level::load(source.clone(), &streams) {
            Ok(level) => level,
            Err(error) => {
                failures.push(format!("{name}: {error}"));
                continue;
            }
        };
        let c = level.coverage();
        if c.model_missing > 0 || c.meshes_missing > 0 {
            failures.push(format!("{name}: {c:?}"));
        }
        objects += c.objects;
        with_model += c.with_model;
        rendered += c.rendered;
        points += c.points;
        // Nested metamodels resolve by GUID: none is missing (by the path
        // hash 1 176 references found nothing).
        for (id, model) in &level.models {
            if !model.desc.missing.is_empty() {
                failures.push(format!(
                    "{name}: metamodel {id:08x}: nested {:08x?} not found",
                    model.desc.missing
                ));
            }
        }
        // Collision of every object with a model: every hkx is found and read.
        for group in &level.groups {
            for o in &group.objects {
                let Some(model) = o.model.and_then(|m| level.models.get(&m)) else {
                    continue;
                };
                with_collision += usize::from(!model.desc.collisions.is_empty());
                collisions.extend(model.desc.collisions.iter().map(|c| c.resource));
            }
        }
        // Every mesh of every model decodes.
        let found: Vec<u32> = level
            .meshes
            .iter()
            .filter(|m| m.found.is_some())
            .map(|m| m.id)
            .filter(|id| meshes.insert(*id))
            .collect();
        found.par_iter().for_each(|&id| {
            let result = source.mesh(id).and_then(|model| {
                for record in &model.records {
                    model.vertices(record)?;
                    model.triangles(record)?;
                }
                Ok(())
            });
            if let Err(error) = result {
                failures.push(format!("{name}: mesh {id:08x}: {error}"));
            }
        });
    }
    let collisions: Vec<u32> = collisions.into_iter().collect();
    let lookup_source = LevelSource::new(
        game.clone(),
        lookup.clone(),
        Overrides::default(),
        Scope::new(&game, &parents, &[]),
    );
    let empty = std::sync::atomic::AtomicUsize::new(0);
    collisions
        .par_iter()
        .for_each(|&id| match lookup_source.collision(id) {
            Ok(mesh) => {
                if mesh.triangles.is_empty() {
                    empty.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
            Err(error) => failures.push(format!("hkx {id:08x}: {error}")),
        });
    failures.assert_empty();
    eprintln!(
        "levels: {objects} objects, {with_model} with a model, {rendered} drawn, {} meshes, {points} points, \
         {with_collision} with collision ({} hkx, {} without triangles)",
        meshes.len(),
        collisions.len(),
        empty.load(std::sync::atomic::Ordering::Relaxed)
    );
    assert_eq!(empty.into_inner(), 0, "every collision has triangles");
    assert!(
        with_collision > 20_000,
        "{with_collision} objects with collision"
    );
    assert_eq!(objects, 111_518);
    assert_eq!(points, 52_162);
    assert!(
        rendered * 100 >= with_model * 95,
        "{rendered} of {with_model}"
    );
}

/// The archive writer lays out the game's own archives exactly: the same
/// header bytes (table, `L283` trailer) and the same data offsets.
#[test]
fn archive_writer_reproduces_the_game_archive_headers() {
    use doctor_toolkit::container::big::{BigArchive, archive_header, find_archives};
    use std::io::Read;
    let Some(dir) = game_dir() else { return };
    let archives = find_archives(&dir).unwrap();
    assert_eq!(archives.len(), 3);
    for path in archives {
        let archive = BigArchive::open(&path).unwrap();
        let sizes: Vec<(&str, u64)> = archive
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.size))
            .collect();
        let (header, offsets) = archive_header(&sizes).unwrap();
        let mut original = vec![0u8; header.len()];
        std::fs::File::open(&path)
            .unwrap()
            .read_exact(&mut original)
            .unwrap();
        assert!(header == original, "{}: header differs", path.display());
        let expected: Vec<u64> = archive.entries.iter().map(|e| e.offset).collect();
        assert_eq!(offsets, expected, "{}", path.display());
        let len = std::fs::metadata(&path).unwrap().len();
        let last = archive
            .entries
            .iter()
            .map(|e| e.offset + e.size)
            .max()
            .unwrap();
        assert_eq!(last, len, "{}: data runs to the end", path.display());
    }
}

/// Copies `streams` (stream names), their ancestors and every table of
/// contents into a temporary game directory.
fn copied_game(dir: &std::path::Path, streams: &[&str]) -> (tempfile::TempDir, Vec<String>) {
    let real = GameInstance::open(dir).unwrap();
    let mut files = Vec::new();
    for name in streams {
        let mut names = vec![(*name).to_owned()];
        names.extend(real.stream_ancestors(name).unwrap());
        for n in names {
            if let Some(i) = real.find_stream(&n) {
                let rel = real.streams()[i].rel_path.clone();
                if !files.contains(&rel) {
                    files.push(rel);
                }
            }
        }
    }
    for toc in real.tocs() {
        files.push(toc.rel_path.clone());
    }
    let temp = tempfile::tempdir().unwrap();
    for rel in &files {
        let to = temp.path().join(rel);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(dir.join(rel), to).unwrap();
    }
    (temp, files)
}

#[test]
fn new_strings_are_added_to_every_language_and_referenced_on_copies() {
    use doctor_toolkit::hash::string_id;
    use doctor_toolkit::resource::apx::AptFile;
    use doctor_toolkit::resource::lh2::StringTable;
    use doctor_toolkit::text::{self, NewString};
    use doctor_toolkit::workspace::{ApplyOptions, Workspace};

    let Some(dir) = game_dir() else { return };
    let texts = [
        "gf2/frontend",
        "gf2/text/dd7d7050",
        "gf2/text/ad8bf024",
        "gf2/text/d70e6b6c",
        "gf2/text/856c146c",
    ];
    let (temp, files) = copied_game(&dir, &texts);
    let game = GameInstance::open(temp.path()).unwrap();
    let (index, _) = game.build_index(|_| {});
    let tables = text::language_tables(&game, &index).unwrap();
    let languages: Vec<_> = tables.iter().map(|t| t.language.clone().unwrap()).collect();
    assert_eq!(languages, ["en", "es", "ss", "ll"]);
    let before: Vec<StringTable> = tables
        .iter()
        .map(|t| {
            let s = game.open_stream(t.stream).unwrap();
            StringTable::parse(&s.resource_data(t.resource).unwrap()).unwrap()
        })
        .collect();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let mut new = NewString {
        name: "doctor_corpus".into(),
        text: "Doctor corpus".into(),
        ..Default::default()
    };
    new.translations
        .insert("es".into(), "Doctor corpus ES".into());
    let plan = text::plan_add(&game, &ws, &index, &tables, &new).unwrap();
    assert_eq!(plan.added.hash, string_id("doctor_corpus"));
    plan.commit(&mut ws, &game).unwrap();
    // A name a table of the game already has is refused.
    let taken = NewString {
        name: "fe_press_start".into(),
        ..new.clone()
    };
    assert!(text::plan_add(&game, &ws, &index, &tables, &taken).is_err());
    let frontend = game.find_stream("gf2/frontend").unwrap();
    let changed = text::retarget_apx(
        &game,
        &mut ws,
        frontend,
        20,
        "fe_press_start",
        "doctor_corpus",
    )
    .unwrap();
    assert_eq!(changed, 1);
    ws.apply(&game, ApplyOptions::default()).unwrap();

    let game = GameInstance::open(temp.path()).unwrap();
    for (t, old) in tables.iter().zip(&before) {
        let s = game.open_stream(t.stream).unwrap();
        let table = StringTable::parse(&s.resource_data(t.resource).unwrap()).unwrap();
        assert!(table.hashes.windows(2).all(|w| w[0] < w[1]));
        let expected = if t.language.as_deref() == Some("es") {
            "Doctor corpus ES"
        } else {
            "Doctor corpus"
        };
        assert_eq!(
            table.texts(plan.added.hash).unwrap(),
            vec![expected; table.strings.len()]
        );
        // Every other entry is unchanged.
        assert_eq!(table.hashes.len(), old.hashes.len() + 1);
        for (i, h) in old.hashes.iter().enumerate() {
            assert_eq!(table.texts(*h).unwrap()[0], old.strings[0][i]);
        }
    }
    let s = game.open_stream(frontend).unwrap();
    let movie = AptFile::parse(&s.resource_data(20).unwrap()).unwrap();
    let names: Vec<String> = movie.text_references().into_iter().map(|r| r.2).collect();
    assert!(names.contains(&"doctor_corpus".to_owned()));
    assert!(!names.contains(&"fe_press_start".to_owned()));
    // Undo both and unapply: the copies come back byte for byte.
    drop(s);
    let originals: Vec<Vec<u8>> = files
        .iter()
        .map(|rel| std::fs::read(dir.join(rel)).unwrap())
        .collect();
    ws.undo().unwrap().unwrap();
    ws.undo().unwrap().unwrap();
    ws.apply(&game, ApplyOptions::default()).unwrap();
    ws.unapply(&game, false).unwrap();
    for (rel, original) in files.iter().zip(&originals) {
        assert_eq!(
            &std::fs::read(temp.path().join(rel)).unwrap(),
            original,
            "{rel}"
        );
    }
}

#[test]
fn new_collision_is_recorded_and_applied_on_copies() {
    use doctor_toolkit::create;
    use doctor_toolkit::import::collision;
    use doctor_toolkit::resource::hkx::collision::CollisionKind;
    use doctor_toolkit::resource::hkx::geometry::physics_mesh;
    use doctor_toolkit::resource::hkx::physics::Physics;
    use doctor_toolkit::resource::hkx::physics::ShapeKind;
    use doctor_toolkit::resource::hkx::{TYPE_HKX, mopp::MoppCode};
    use doctor_toolkit::workspace::{ApplyOptions, Workspace};

    let Some(dir) = game_dir() else { return };
    let name = "gf2/cuba/local/vic/high/venue_127";
    let (temp, files) = copied_game(&dir, &[name]);
    let originals: Vec<Vec<u8>> = files
        .iter()
        .map(|rel| std::fs::read(temp.path().join(rel)).unwrap())
        .collect();
    let game = GameInstance::open(temp.path()).unwrap();
    let s = game.find_stream(name).unwrap();
    let (index, _) = game.build_index(|_| {});
    let baseline: std::collections::BTreeSet<_> = create::check_references(&game, s)
        .unwrap()
        .into_iter()
        .map(|u| u.target)
        .collect();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    // Metamodel 405ae276: a fluorescent light with static mesh collision.
    let metamodel = 343;
    let mesh = collision::mesh_from_metamodel(&game, &ws, s, metamodel).unwrap();
    for kind in [CollisionKind::Box, CollisionKind::Hull, CollisionKind::Mesh] {
        let plan = collision::plan(&game, &ws, &index, s, metamodel, &mesh, kind).unwrap();
        assert_eq!(plan.report.references, 1);
        plan.commit(&mut ws, &game).unwrap();
        ws.apply(&game, ApplyOptions::default()).unwrap();

        let game = GameInstance::open(temp.path()).unwrap();
        let stream = game.open_stream(s).unwrap();
        let layout = stream.layout();
        let data = stream.resource_data(metamodel + 1).unwrap();
        let refs = collision::collision_references(&data).unwrap();
        assert_eq!(refs.len(), 1);
        // The new hkx is right before the metamodel and is what it refers to.
        let hkx = &layout.resources[metamodel];
        assert_eq!(hkx.header.type_id, TYPE_HKX);
        assert_eq!(hkx.header.ids, refs[0]);
        let physics = Physics::parse(&stream.resource_data(metamodel).unwrap()).unwrap();
        let (lo, hi) = physics_mesh(&physics).bounds().unwrap();
        let (mlo, mhi) = (
            mesh.positions.iter().fold([f32::MAX; 3], |a, p| {
                [a[0].min(p[0]), a[1].min(p[1]), a[2].min(p[2])]
            }),
            mesh.positions.iter().fold([f32::MIN; 3], |a, p| {
                [a[0].max(p[0]), a[1].max(p[1]), a[2].max(p[2])]
            }),
        );
        for a in 0..3 {
            assert!(
                (lo[a] - mlo[a]).abs() < 0.06 && (hi[a] - mhi[a]).abs() < 0.06,
                "{kind:?}"
            );
        }
        if kind == CollisionKind::Mesh {
            let ShapeKind::Mopp { code, child } = &physics.bodies().next().unwrap().shape.kind
            else {
                panic!("not a MOPP shape");
            };
            let code: &MoppCode = code;
            let ShapeKind::SimpleMesh { triangles, .. } = &child.kind else {
                panic!("not a simple mesh");
            };
            assert_eq!(code.keys().unwrap().len(), triangles.len());
        }
        // No new unresolved reference.
        let after: std::collections::BTreeSet<_> = create::check_references(&game, s)
            .unwrap()
            .into_iter()
            .map(|u| u.target)
            .collect();
        assert!(
            after.is_subset(&baseline),
            "{:?}",
            after.difference(&baseline)
        );
        drop(stream);
        // Undo for the next kind: the stream comes back byte for byte.
        ws.undo().unwrap().unwrap();
        ws.apply(&game, ApplyOptions::default()).unwrap();
        ws.unapply(&game, false).unwrap();
        for (rel, original) in files.iter().zip(&originals) {
            assert_eq!(
                &std::fs::read(temp.path().join(rel)).unwrap(),
                original,
                "{rel}"
            );
        }
    }
}

/// Every `bnk` is an animation bank; every clip (schemes 1, 3 and 4) decodes
/// with its rig bone set, rotations are unit quaternions and translations
/// are near the bind pose.
#[test]
fn every_animation_bank_decodes() {
    use doctor_toolkit::resource::anim::{AnimBank, TYPE_BNK};
    use doctor_toolkit::resource::rcb::{Rig, TYPE_RCB};
    use std::collections::HashMap;
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    // Bone sets and skeletons of every rig.
    let sets: Mutex<HashMap<u32, Vec<usize>>> = Mutex::new(HashMap::new());
    let skeletons = Mutex::new(HashMap::new());
    game.streams().par_iter().for_each(|file| {
        let stream = StreamFile::open(&file.path).unwrap();
        for (index, r) in stream.layout().resources.iter().enumerate() {
            if r.header.type_id == TYPE_RCB {
                let rig = Rig::parse(&stream.resource_data(index).unwrap()).unwrap();
                for set in &rig.bone_sets {
                    sets.lock().unwrap().insert(set.hash, set.bones.clone());
                }
                for s in rig.skeletons {
                    skeletons.lock().unwrap().insert(s.name_hash, s);
                }
            }
        }
    });
    let sets = sets.into_inner().unwrap();
    let skeletons = skeletons.into_inner().unwrap();
    let failures = Failures::default();
    let counts = Mutex::new([0usize; 6]);
    // Angle between decoded frame 0 and the bind pose: as stored, and conjugated.
    let angles = Mutex::new((0f64, 0f64, 0usize));
    game.streams().par_iter().for_each(|file| {
        let stream = StreamFile::open(&file.path).unwrap();
        for (index, r) in stream.layout().resources.iter().enumerate() {
            if r.header.type_id != TYPE_BNK {
                continue;
            }
            let data = stream.resource_data(index).unwrap();
            let bank = match AnimBank::parse(&data) {
                Ok(b) => b,
                Err(e) => {
                    failures.push(format!("{} #{index}: {e}", file.rel_path));
                    continue;
                }
            };
            let skeleton = skeletons.get(&bank.skeleton);
            let locals = skeleton.map(|s| s.bind_locals());
            let mut c = counts.lock().unwrap();
            c[0] += 1;
            drop(c);
            for clip in &bank.clips {
                let mut c = counts.lock().unwrap();
                c[1] += 1;
                c[2] += clip.frames as usize;
                drop(c);
                if clip.scheme == 3 {
                    counts.lock().unwrap()[3] += 1;
                }
                let Some(bones) = sets.get(&clip.bone_set) else {
                    counts.lock().unwrap()[4] += 1;
                    continue;
                };
                match clip.decode(&data, bones) {
                    Ok(tracks) => {
                        counts.lock().unwrap()[5] += 1;
                        for t in &tracks {
                            for q in &t.rotations {
                                let n = q.iter().map(|x| x * x).sum::<f32>();
                                if !(0.98..1.02).contains(&n) {
                                    failures.push(format!(
                                        "{} #{index} clip {:08x}: rotation norm {n}",
                                        file.rel_path, clip.hash
                                    ));
                                    break;
                                }
                            }
                            if let Some(locals) = &locals
                                && let Some(m) = locals.get(t.bone)
                            {
                                let b = quat_of(m);
                                let q = t.rotations[0];
                                let d1 =
                                    (b[0] * q[0] + b[1] * q[1] + b[2] * q[2] + b[3] * q[3]).abs();
                                let d2 =
                                    (-b[0] * q[0] - b[1] * q[1] - b[2] * q[2] + b[3] * q[3]).abs();
                                let mut a = angles.lock().unwrap();
                                a.0 += 2.0 * (d1.min(1.0) as f64).acos();
                                a.1 += 2.0 * (d2.min(1.0) as f64).acos();
                                a.2 += 1;
                            }
                        }
                    }
                    Err(e) => failures.push(format!(
                        "{} #{index} clip {:08x}: {e}",
                        file.rel_path, clip.hash
                    )),
                }
            }
        }
    });
    let c = counts.into_inner().unwrap();
    let a = angles.into_inner().unwrap();
    eprintln!(
        "banks {} clips {} frames {} scheme3 {} no bone set {} decoded {}; mean angle to bind: stored {:.3} rad, conjugated {:.3} rad over {} bones",
        c[0],
        c[1],
        c[2],
        c[3],
        c[4],
        c[5],
        a.0 / a.2.max(1) as f64,
        a.1 / a.2.max(1) as f64,
        a.2
    );
    failures.assert_empty();
    assert_eq!(c[0], 2751);
    assert_eq!(c[1], 6115);
    assert_eq!(c[5], 6115, "every clip decodes");
}

/// Rotation of a column-major affine matrix as a quaternion (x, y, z, w).
fn quat_of(m: &[f32; 16]) -> [f32; 4] {
    let (m00, m11, m22) = (m[0], m[5], m[10]);
    let (m01, m02, m10, m12, m20, m21) = (m[4], m[8], m[1], m[9], m[2], m[6]);
    let trace = m00 + m11 + m22;
    if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [(m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, 0.25 * s]
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        [0.25 * s, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s]
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        [(m01 + m10) / s, 0.25 * s, (m12 + m21) / s, (m02 - m20) / s]
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        [(m02 + m20) / s, (m12 + m21) / s, 0.25 * s, (m10 - m01) / s]
    }
}

#[test]
fn the_mob_face_editor_parses() {
    use doctor_toolkit::resource::mobface::MobFace;
    let found = std::sync::Mutex::new(Vec::new());
    let count = check_every("mobface", |data| {
        let m = MobFace::parse(data).map_err(|e| e.to_string())?;
        found.lock().unwrap().push(m);
        Ok(())
    });
    if count == 0 {
        return;
    }
    let m = found.into_inner().unwrap().remove(0);
    assert_eq!(
        (m.menus.len(), m.sliders.len(), m.palettes.len()),
        (11, 16, 13)
    );
    assert_eq!(m.palettes[5].name, "tintskin");
    assert_eq!(m.palettes[5].entries.len(), 36);
    assert_eq!(m.sliders[0].name, "head_shape_1");
    let targets: Vec<&str> = m.sliders[0]
        .axes
        .iter()
        .flat_map(|a| &a.ranges)
        .flat_map(|r| &r.targets)
        .map(|t| t.name.as_str())
        .collect();
    assert!(targets.contains(&"headnawd"), "{targets:?}");
}

/// Every head of every character database (all its levels of detail) skins
/// within its palette, with at most four weights summing to one on bones of
/// the human skeleton; its materials and their base textures are found, eye
/// materials (`gf_char_eye`) have an iris mask in the base alpha, and the
/// eyelashes (`gf_char_hair_aniso`) an alpha-only base texture.
#[test]
fn every_head_skins_within_its_palette_and_finds_its_textures() {
    use doctor_toolkit::character::MeshLocator;
    use doctor_toolkit::export::gltf::AssetSource;
    use doctor_toolkit::game::GameAssets;
    use doctor_toolkit::resource::mesh;
    use doctor_toolkit::resource::partsdb::{KIND_CHARACTER, PartsDb};
    use doctor_toolkit::resource::shading::{Family, Shading};
    use doctor_toolkit::resource::tg4::Format;
    let Some(dir) = game_dir() else { return };
    let game = std::sync::Arc::new(GameInstance::open(&dir).unwrap());
    let (index, _) = game.build_index(|_| {});
    let meshes = MeshLocator::new(&index);
    let mut assets = GameAssets::new(game.clone(), &index);
    let failures = Failures::default();
    let mut heads = std::collections::BTreeSet::new();
    let (mut records, mut eyes, mut lashes, mut influences) = (0, 0, 0, [0usize; 5]);
    let mut open: std::collections::HashMap<usize, StreamFile> = Default::default();
    for e in &index.entries {
        if e.header.type_id != doctor_toolkit::resource::logic::TYPE_PARTSDB {
            continue;
        }
        let file = open
            .entry(e.stream as usize)
            .or_insert_with(|| game.open_stream(e.stream as usize).unwrap());
        let db = PartsDb::parse(&file.resource_data(e.resource as usize).unwrap()).unwrap();
        if db.kind != KIND_CHARACTER {
            continue;
        }
        for part in db.parts.iter().filter(|p| p.name.contains("head")) {
            for id in part.mesh_ids() {
                if !heads.insert(id) {
                    continue;
                }
                let Some((s, r)) = meshes.find(id, e.stream as usize) else {
                    failures.push(format!("{}/{}: mesh {id:08x} missing", db.name, part.name));
                    continue;
                };
                let stream = open
                    .entry(s)
                    .or_insert_with(|| game.open_stream(s).unwrap());
                let model = mesh::load(stream, r).unwrap();
                let name = &model.name;
                if model.bones.iter().any(|b| b.1 >= 102) {
                    failures.push(format!("{name}: bone outside the human skeleton"));
                }
                if assets.skeleton(&model.bones).is_none() {
                    failures.push(format!("{name}: no skeleton"));
                }
                for record in &model.records {
                    records += 1;
                    let v = model.vertices(record).unwrap();
                    let at = format!("{name}/{}", record.name);
                    if let (Some(joints), Some(weights)) = (&v.joints, &v.weights) {
                        for (j, w) in joints.iter().zip(weights) {
                            let used = w.iter().filter(|&&x| x > 0.0).count();
                            influences[used] += 1;
                            if (w.iter().sum::<f32>() - 1.0).abs() > 1e-3 {
                                failures.push(format!(
                                    "{at}: weights sum to {}",
                                    w.iter().sum::<f32>()
                                ));
                                break;
                            }
                            if (0..4).any(|k| w[k] > 0.0 && j[k] as usize >= record.palette.len()) {
                                failures.push(format!("{at}: joint outside the palette"));
                                break;
                            }
                        }
                        if record.influences as usize > 4 {
                            failures.push(format!("{at}: {} influences", record.influences));
                        }
                    }
                    if record.palette.iter().any(|p| p.0 >= 102) {
                        failures.push(format!("{at}: palette bone outside the skeleton"));
                    }
                    let Some(material) = assets.material(record.material_hash) else {
                        failures.push(format!(
                            "{at}: material {:08x} missing",
                            record.material_hash
                        ));
                        continue;
                    };
                    let shading = Shading::from_material(&material);
                    let Some(base) = shading.base.and_then(|h| assets.texture(h)) else {
                        failures.push(format!("{at}: base texture of {} missing", material.name));
                        continue;
                    };
                    match shading.family {
                        Family::Eye => {
                            eyes += 1;
                            let (_, _, rgba) = base.rgba(0).unwrap();
                            let iris = rgba.chunks_exact(4).filter(|p| p[3] > 128).count();
                            let n = rgba.len() / 4;
                            if iris == 0 || iris == n {
                                failures.push(format!("{at}: eye base alpha is not an iris mask"));
                            }
                        }
                        Family::PlainHair => {
                            lashes += 1;
                            if base.header.format() != Format::A8 {
                                failures
                                    .push(format!("{at}: eyelashes {:?}", base.header.format()));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    eprintln!(
        "{} head meshes, {records} records, {eyes} eye and {lashes} eyelash records; vertices by weights used: {influences:?}",
        heads.len()
    );
    assert!(heads.len() > 100, "{}", heads.len());
    assert!(eyes > 50 && lashes > 0, "{eyes} {lashes}");
    failures.assert_empty();
}

/// Cut-scene clips move the bones to the proportions of the actor's own rig
/// (`Animated` attribute 2): in the first frame, the face bones of every
/// clip of a character lie where the bind pose of that rig's skeleton puts
/// them, not where the other human rig does. Female actors name `be9a8908`,
/// whose `this_lod1` differs from its `puppet`; male actors `b2b17079`.
#[test]
fn cutscene_clips_fit_the_rig_of_their_actor() {
    use doctor_toolkit::cutscene::cast::Cast;
    use doctor_toolkit::cutscene::{Cutscene, Source, list_shots};
    use doctor_toolkit::game::GameAssets;
    use doctor_toolkit::resource::rcb::{Skeleton, skeleton_for};
    use doctor_toolkit::resource::trinity::Command;
    let Some(dir) = game_dir() else { return };
    let game = std::sync::Arc::new(GameInstance::open(&dir).unwrap());
    let (index, _) = game.build_index(|_| {});
    let mut assets = GameAssets::new(game.clone(), &index);
    let rigs = assets.rigs().to_vec();
    let mut cast = Cast::new(&game, &index, rigs.clone());
    let source = Source::new(&game, None);
    let shots = list_shots(&game).unwrap();
    let mut streams: Vec<usize> = shots.iter().map(|s| s.stream).collect();
    streams.dedup();
    let failures = Failures::default();
    let mut rig_counts: std::collections::BTreeMap<u32, usize> = Default::default();
    let (mut checked, mut uniform_rigs) = (0, 0);
    let mut mismatches = Vec::new();
    for &stream in &streams {
        let Ok(cutscene) = Cutscene::load(&source, stream, None) else {
            continue;
        };
        for shot in &cutscene.shots {
            for actor in shot.actors.iter().filter(|a| a.assembly.is_some()) {
                let Some(rig) = actor.rig else {
                    failures.push(format!("{}: {} names no rig", shot.stream_name, actor.name));
                    continue;
                };
                let Some(rig_entry) = rigs.iter().find(|r| r.id == rig) else {
                    failures.push(format!("{}: rig {rig:08x} not found", actor.name));
                    continue;
                };
                *rig_counts.entry(rig).or_default() += 1;
                uniform_rigs += rig_entry.is_uniform() as usize;
                let Some(clip) = shot.block().commands.iter().find_map(|c| match c {
                    Command::ActorAnim { actor: a, clip, .. } if *a == actor.guid => {
                        Some(clip.text.clone())
                    }
                    _ => None,
                }) else {
                    continue;
                };
                let Ok(anim) = cast.animation(&source, shot, &clip) else {
                    continue;
                };
                let table: Vec<(u32, u32)> = anim
                    .skeleton
                    .bones
                    .iter()
                    .enumerate()
                    .map(|(i, b)| (b.hash, i as u32))
                    .collect();
                // The actor's skeleton, and one of a human rig unlike it.
                let own = skeleton_for(&rigs, Some(rig), &table).unwrap();
                let differs = |s: &Skeleton| {
                    s.bones
                        .iter()
                        .zip(&own.bones)
                        .any(|(a, b)| (a.inverse_bind[13] - b.inverse_bind[13]).abs() > 1e-3)
                };
                let Some(other) = rigs
                    .iter()
                    .filter_map(|r| r.skeleton_for(&table))
                    .find(|s| differs(s))
                else {
                    continue;
                };
                let neck = own.bones.iter().position(|b| b.name == Some("m_neck2"));
                let face: Vec<usize> = (0..own.bones.len())
                    .filter(|&i| {
                        let p = own.bones[i].parent;
                        p == neck || p.is_some_and(|p| own.bones[p].parent == neck)
                    })
                    .collect();
                let error = |s: &Skeleton| -> f32 {
                    let locals = s.bind_locals();
                    anim.tracks
                        .iter()
                        .filter(|t| face.contains(&t.bone) && !t.translations.is_empty())
                        .map(|t| {
                            let p = t.translations[0];
                            (0..3)
                                .map(|k| (p[k] - locals[t.bone][12 + k]).powi(2))
                                .sum::<f32>()
                                .sqrt()
                        })
                        .sum()
                };
                checked += 1;
                let (mine, theirs) = (error(own), error(other));
                if mine > theirs {
                    mismatches.push(format!(
                        "{} {clip} ({:?} {:?}): rig {rig:08x} {mine:.4}, the other rig {theirs:.4}",
                        actor.name, actor.assembly, actor.set
                    ));
                }
            }
        }
    }
    eprintln!(
        "actors by rig {:?}; {uniform_rigs} uniform; {checked} clips compared; \
         fitting the other rig: {mismatches:#?}",
        rig_counts
            .iter()
            .map(|(r, n)| format!("{r:08x}: {n}"))
            .collect::<Vec<_>>()
    );
    assert!(checked > 300, "{checked}");
    // Two extras of igc01 (female shoppers of `mid_class_female`) play
    // clips of businessmen made for the male proportions.
    assert!(mismatches.len() <= 2, "{mismatches:#?}");
    failures.assert_empty();
}

/// A part added to the `player` database (a hat from its meshes) is worn by
/// the `_all` set after a write and parse, hides the hair (hat culling tag),
/// and the character exports to glTF; every character database's every set
/// assembles and finds the meshes of its first choices.
#[test]
fn characters_assemble_and_take_new_parts() {
    use doctor_toolkit::character::{Character, MeshLocator};
    use doctor_toolkit::game::GameAssets;
    use doctor_toolkit::resource::partsdb::{KIND_CHARACTER, Part, PartsDb, Selection};
    use doctor_toolkit::resource::simgroup::Hash32;
    let Some(dir) = game_dir() else { return };
    let game = std::sync::Arc::new(GameInstance::open(&dir).unwrap());
    let stream = game
        .find_stream("gf2/globalfolder/characters/allcharacters")
        .unwrap();
    let file = game.open_stream(stream).unwrap();
    let (index, _) = game.build_index(|_| {});
    let meshes = MeshLocator::new(&index);
    let mut player = None;
    let mut sets = 0;
    let mut missing = Vec::new();
    for (i, r) in file.layout().resources.iter().enumerate() {
        if r.header.type_id != doctor_toolkit::resource::logic::TYPE_PARTSDB {
            continue;
        }
        let db = PartsDb::parse(&file.resource_data(i).unwrap()).unwrap();
        if db.kind != KIND_CHARACTER {
            continue;
        }
        for set in 0..db.sets.len() {
            let c = Character::load(&game, &meshes, stream, &db, set, &Selection::default(), 0)
                .unwrap();
            missing.extend(
                c.missing
                    .iter()
                    .map(|m| format!("{}/{}: {}", db.name, db.sets[set].name, m.0)),
            );
            sets += 1;
        }
        if db.name == "player" {
            player = Some(db);
        }
    }
    assert!(sets > 1000, "{sets}");
    eprintln!(
        "{sets} sets assembled; parts without a mesh: {}",
        missing.len()
    );
    let mut db = player.unwrap();
    let hat = |name: &str| {
        file.layout()
            .resources
            .iter()
            .enumerate()
            .filter(|(_, r)| r.header.type_id == doctor_toolkit::resource::mesh::TYPE_MESH)
            .find(|(i, _)| {
                doctor_toolkit::resource::mesh::load(&file, *i).is_ok_and(|m| m.name == name)
            })
            .map(|(_, r)| r.header.ids[3])
            .unwrap()
    };
    let slot = db.slot_index("a_hat").unwrap();
    let part = db
        .add_part(Part {
            name: "doctor_hat".into(),
            slot: Some(slot),
            models: [
                Hash32(hat("hat_dress_lod1")),
                Hash32(hat("hat_dress_lod2")),
                Hash32(0),
                Hash32(0),
                Hash32(0),
            ],
            label: String::new(),
            tags: Vec::new(),
        })
        .unwrap();
    let variant = db.add_variant("doctor_hat", vec![part as u16]).unwrap();
    let all = db.set_index("_all").unwrap();
    assert!(db.allow_variant(all, slot, variant, true).unwrap());
    let db = PartsDb::parse(&db.encode().unwrap()).unwrap();
    let c = Character::load(&game, &meshes, stream, &db, all, &Selection::default(), 0).unwrap();
    assert!(c.parts.iter().any(|p| p.name == "doctor_hat"));
    assert!(
        c.assembly
            .culled
            .iter()
            .any(|&p| db.parts[p].tags.iter().any(|t| t == "hatCulled"))
    );
    let mut assets = GameAssets::new(game.clone(), &index);
    let glb = c.to_glb(&mut assets).unwrap();
    assert_eq!(&glb[..4], b"glTF");
}

/// Every parameter of `data/gf2/tunables.toml` finds its objects, holds a
/// value within its bounds, and can be set and set back without changing
/// the rest of its SimGroup.
#[test]
fn every_curated_tunable_resolves_and_round_trips() {
    use doctor_toolkit::resource::simgroup::SimGroup;
    use doctor_toolkit::tunables::{self, Change};
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let list = tunables::gf2();
    for t in list {
        let reading = t
            .read(&game, None)
            .unwrap_or_else(|e| panic!("{}: {e}", t.id));
        assert!(!reading.objects.is_empty(), "{}", t.id);
        for o in &reading.objects {
            assert!(
                t.check(o.value).is_ok(),
                "{}: {} = {}",
                t.id,
                o.label,
                o.value
            );
        }
        // Setting the current values back changes nothing.
        let same = Change {
            tunable: t.clone(),
            values: reading
                .objects
                .iter()
                .map(|o| (o.object, o.value))
                .collect(),
        };
        let (edits, diff) = tunables::plan(&game, None, &[same]).unwrap();
        assert!(edits.is_empty() && diff.is_empty(), "{}", t.id);
        // A new value is read back, and only that attribute changes.
        let first = &reading.objects[0];
        let new: f64 = if first.value == 2.0 { 3.0 } else { 2.0 };
        let new = new.clamp(t.min.unwrap_or(f64::MIN), t.max.unwrap_or(f64::MAX));
        let change = Change {
            tunable: t.clone(),
            values: vec![(first.object, new)],
        };
        let (edits, diff) = tunables::plan(&game, None, &[change]).unwrap();
        assert_eq!(diff.len(), 1, "{}", t.id);
        let group = SimGroup::parse(&edits[0].1[0].data).unwrap();
        let back = t.read_group(&group).unwrap();
        assert_eq!(back[0].value, new, "{}", t.id);
        for (a, b) in back.iter().zip(&reading.objects).skip(1) {
            assert_eq!(a.value, b.value, "{}: {}", t.id, a.label);
        }
    }
    eprintln!("{} tunables resolve", list.len());
}

/// Dialogue trees: every `dlg` parses and writes back byte for byte.
#[test]
fn every_dialogue_parses_and_writes_back() {
    use doctor_toolkit::resource::dlg::{Dialogue, KIND_END};
    let count = for_each_resource("dlg", |_, data| {
        let d = Dialogue::parse(data).map_err(|e| e.to_string())?;
        if d.count(KIND_END) != 1 {
            return Err(format!("{} end nodes", d.count(KIND_END)));
        }
        let back = d.to_bytes().map_err(|e| e.to_string())?;
        if back != data {
            let at = back.iter().zip(data).position(|(a, b)| a != b);
            return Err(format!(
                "differs at {at:?}, {} vs {} bytes",
                back.len(),
                data.len()
            ));
        }
        Ok(())
    });
    if game_dir().is_some() {
        assert_eq!(count, 143);
    }
}

/// Chatter sets: every `chv` writes back byte for byte, and every line's
/// sound id is an entry of a speech bank (`smb`).
#[test]
fn every_chatter_set_writes_back_and_finds_its_sounds() {
    use doctor_toolkit::resource::chv::{ChatterSet, event_hash};
    use doctor_toolkit::resource::smb;
    let sounds = Mutex::new(std::collections::HashSet::new());
    let banks = for_each_resource("smb", |_, data| {
        let entries = smb::parse(data).map_err(|e| e.to_string())?;
        sounds
            .lock()
            .unwrap()
            .extend(entries.into_iter().map(|e| e.sound));
        Ok(())
    });
    let sounds = sounds.into_inner().unwrap();
    let count = for_each_resource("chv", |_, data| {
        let set = ChatterSet::parse(data).map_err(|e| e.to_string())?;
        if set.to_bytes().map_err(|e| e.to_string())? != data {
            return Err("does not write back identically".into());
        }
        for event in &set.events {
            if event_hash(&event.name) != event.hash {
                return Err(format!("{}: hash {:08x}", event.name, event.hash));
            }
            for line in &event.lines {
                if !sounds.contains(&line.sound()) {
                    return Err(format!(
                        "{}: sound {:08x} in no bank",
                        event.name,
                        line.sound()
                    ));
                }
            }
        }
        Ok(())
    });
    if game_dir().is_some() {
        assert_eq!((count, banks), (259, 59));
    }
}

/// Speech banks: every `smb` writes back byte for byte, its entries are in
/// the order the game's binary search needs, and the path built from an
/// entry the way the game builds it names a file of the archives.
#[test]
fn every_speech_bank_writes_back_and_names_archive_files() {
    use doctor_toolkit::container::big::{BigArchive, find_archives, normalize_name};
    use doctor_toolkit::resource::smb::{
        ENTRY_TAG, FLAG_LOCALISED, SpeechBank, VERSION, name_hash,
    };
    let Some(dir) = game_dir() else { return };
    let mut files = std::collections::HashSet::new();
    for path in find_archives(&dir).unwrap() {
        for entry in BigArchive::open(&path).unwrap().entries {
            files.insert(normalize_name(&entry.name));
        }
    }
    let entries = std::sync::atomic::AtomicUsize::new(0);
    let localised = std::sync::atomic::AtomicUsize::new(0);
    let odd_hashes = std::sync::atomic::AtomicUsize::new(0);
    let count = for_each_resource("smb", |_, data| {
        let bank = SpeechBank::parse(data).map_err(|e| e.to_string())?;
        if bank.to_bytes().map_err(|e| e.to_string())? != data {
            return Err("does not write back identically".into());
        }
        if bank.version != VERSION {
            return Err(format!("version {}", bank.version));
        }
        if let Some(problem) = bank.validate().first() {
            return Err(problem.clone());
        }
        for e in &bank.entries {
            if e.tag != ENTRY_TAG || e.id.iter().any(|&w| w != e.id[0]) {
                return Err(format!("{}: tag {} id {:08x?}", e.file, e.tag, e.id));
            }
            if bank.find(&e.id).map(|f| &f.file) != Some(&e.file) {
                return Err(format!("{}: not found by its id", e.file));
            }
            if e.name_hash != name_hash(&e.file) {
                odd_hashes.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            let path = e.stream_path(None);
            if !files.contains(&path) {
                return Err(format!("{}: no archive has {path}", e.file));
            }
            if e.flags & FLAG_LOCALISED != 0 {
                let es = e.stream_path(Some("es"));
                if !files.contains(&es) {
                    return Err(format!("{}: no archive has {es}", e.file));
                }
                localised.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        entries.fetch_add(bank.entries.len(), std::sync::atomic::Ordering::Relaxed);
        Ok(())
    });
    assert_eq!(count, 59);
    assert_eq!(entries.into_inner(), 12_918);
    // Two entries (`gf2_dons_view_01.exa` is one) keep the hash of another name.
    assert_eq!(odd_hashes.into_inner(), 2);
    eprintln!("localised entries: {}", localised.into_inner());
}

/// The speech event table: `get` writes back byte for byte, its records are
/// in hash order, and a new record goes to its place.
#[test]
fn speech_event_table_writes_back_and_covers_chatter_events() {
    use doctor_toolkit::resource::chv::ChatterSet;
    use doctor_toolkit::resource::get::{SpeechEvent, SpeechEventTable};
    let table = Mutex::new(None);
    let count = for_each_resource("get", |_, data| {
        let t = SpeechEventTable::parse(data).map_err(|e| e.to_string())?;
        if t.to_bytes().map_err(|e| e.to_string())? != data {
            return Err("does not write back identically".into());
        }
        if let Some(problem) = t.validate().first() {
            return Err(problem.clone());
        }
        let mut grown = t.clone();
        if !grown.insert(SpeechEvent::new("DOCTOR_TOOLKIT_NEW_EVENT")) {
            return Err("the test event exists".into());
        }
        let back = SpeechEventTable::parse(&grown.to_bytes().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        if back != grown || !back.validate().is_empty() || back.events.len() != t.events.len() + 1 {
            return Err("a new record does not survive".into());
        }
        *table.lock().unwrap() = Some(t);
        Ok(())
    });
    if game_dir().is_none() {
        return;
    }
    assert_eq!(count, 1);
    let table = table.into_inner().unwrap().unwrap();
    assert_eq!(table.events.len(), 1372);
    // Events of the chatter sets and the table: the game reads the record of
    // an event it is about to say without a check.
    let missing = Mutex::new(std::collections::BTreeSet::new());
    let events = std::sync::atomic::AtomicUsize::new(0);
    for_each_resource("chv", |_, data| {
        let set = ChatterSet::parse(data).map_err(|e| e.to_string())?;
        if let Some(problem) = set.validate().first() {
            return Err(problem.clone());
        }
        for event in &set.events {
            events.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if table.find(event.hash).is_none() {
                missing.lock().unwrap().insert(event.name.clone());
            }
        }
        Ok(())
    });
    let missing = missing.into_inner().unwrap();
    eprintln!(
        "chatter events: {}, names without a record in get: {} {:?}",
        events.into_inner(),
        missing.len(),
        missing.iter().take(12).collect::<Vec<_>>()
    );
}

/// Audio configuration and mixer snapshots: every `auc` and `mix` writes
/// back byte for byte; mixes and speech banks name categories of `auc`.
#[test]
fn audio_config_and_mixes_write_back() {
    use doctor_toolkit::resource::auc::AudioConfig;
    use doctor_toolkit::resource::mix::Mix;
    use doctor_toolkit::resource::smb::SpeechBank;
    let config = Mutex::new(None);
    let configs = for_each_resource("auc", |_, data| {
        let c = AudioConfig::parse(data).map_err(|e| e.to_string())?;
        if c.to_bytes().map_err(|e| e.to_string())? != data {
            return Err("does not write back identically".into());
        }
        if let Some(problem) = c.validate().first() {
            return Err(problem.clone());
        }
        let mut grown = c.clone();
        grown
            .add_category("DOCTOR_VOICE", "DIALOG")
            .map_err(|e| e.to_string())?;
        let back = AudioConfig::parse(&grown.to_bytes().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        if back != grown || !back.validate().is_empty() {
            return Err("a new category does not survive".into());
        }
        *config.lock().unwrap() = Some(c);
        Ok(())
    });
    if game_dir().is_none() {
        return;
    }
    let config = config.into_inner().unwrap().unwrap();
    assert_eq!(
        (
            config.categories.len(),
            config.pools.len(),
            config.ducks.len()
        ),
        (49, 6, 3)
    );
    let channels = std::sync::atomic::AtomicUsize::new(0);
    let mixes = for_each_resource("mix", |_, data| {
        let mix = Mix::parse(data).map_err(|e| e.to_string())?;
        if mix.to_bytes().map_err(|e| e.to_string())? != data {
            return Err("does not write back identically".into());
        }
        for channel in &mix.channels {
            if config.name_of(channel.category).is_none() {
                return Err(format!(
                    "{}: category {:08x} is not in auc",
                    mix.name, channel.category
                ));
            }
        }
        channels.fetch_add(mix.channels.len(), std::sync::atomic::Ordering::Relaxed);
        Ok(())
    });
    assert_eq!((configs, mixes), (2, 43));
    assert_eq!(channels.into_inner(), 1749);
    let unknown = Mutex::new(std::collections::BTreeSet::new());
    for_each_resource("smb", |_, data| {
        let bank = SpeechBank::parse(data).map_err(|e| e.to_string())?;
        for e in &bank.entries {
            if config.name_of(e.category).is_none() {
                unknown.lock().unwrap().insert(e.category);
            }
        }
        Ok(())
    });
    eprintln!(
        "speech bank categories that auc does not name: {:08x?}",
        unknown.into_inner().unwrap()
    );
}

/// Speech files: the subtitle table of every tenth archived file is read and
/// written back byte for byte.
#[test]
fn speech_subtitles_write_back() {
    use doctor_toolkit::container::big::{BigArchive, find_archives};
    use doctor_toolkit::resource::speech;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let Some(dir) = game_dir() else { return };
    let failures = Failures::default();
    let with = AtomicUsize::new(0);
    for path in find_archives(&dir).unwrap() {
        let archive = BigArchive::open(&path).unwrap();
        archive
            .entries
            .par_iter()
            .enumerate()
            .filter(|(i, _)| i % 10 == 0)
            .for_each(|(_, entry)| {
                let data = archive.read(entry).unwrap();
                match speech::subtitles(&data) {
                    Ok(Some(table)) => {
                        with.fetch_add(1, Ordering::Relaxed);
                        match speech::with_subtitles(&data, &table) {
                            Ok(back) if back == data => {}
                            Ok(_) => failures.push(format!("{}: differs", entry.name)),
                            Err(e) => failures.push(format!("{}: {e}", entry.name)),
                        }
                    }
                    Ok(None) => {}
                    Err(e) => failures.push(format!("{}: {e}", entry.name)),
                }
            });
    }
    failures.assert_empty();
    let with = with.into_inner();
    eprintln!("speech files with subtitles (every tenth): {with}");
    assert!(with > 400);
}

#[test]
fn logic_graphs_of_every_mission_build_and_resolve_messages() {
    use doctor_toolkit::logic_graph::{BuildOptions, MessageIndex, MessageState, ScopeSpec, scope};
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let (index, _) = game.build_index(|_| {});
    let messages = MessageIndex::build(&game, &index, None);
    assert!(messages.errors.is_empty(), "{:?}", messages.errors.first());
    let stats = messages.stats();
    eprintln!("whole game: {stats:?}");
    assert!(stats.messages > 20_000, "{stats:?}");
    // Most messages are sent and heard somewhere.
    assert!(stats.connected * 10 > stats.messages * 4, "{stats:?}");
    let missions = scope::missions(&game);
    assert!(missions.len() >= 70, "{} missions", missions.len());
    let failures = Failures::default();
    let totals = Mutex::new([0usize; 8]);
    missions.par_iter().for_each(|m| {
        let spec = ScopeSpec::Mission(m.name.clone());
        let (mut graph, errors) =
            match scope::build(&game, &index, None, &spec, BuildOptions::default()) {
                Ok(g) => g,
                Err(e) => return failures.push(format!("{}: {e}", m.name)),
            };
        if let Some((l, e)) = errors.first() {
            failures.push(format!("{}: group {l:?}: {e}", m.name));
        }
        graph.annotate(&messages);
        for w in &graph.wires {
            let ok = graph
                .nodes
                .get(w.from.node)
                .is_some_and(|n| w.from.pin < n.pins.len())
                && graph.nodes.get(w.to.node).is_some();
            if !ok {
                failures.push(format!("{}: bad wire {w:?}", m.name));
            }
        }
        let o = graph.overview();
        let mut t = totals.lock().unwrap();
        t[0] += 1;
        t[1] += o.nodes;
        t[2] += o.wires;
        t[3] += o.messages;
        t[4] += o.states.get("connected").copied().unwrap_or(0);
        t[5] += graph
            .messages
            .values()
            .filter(|m| matches!(m.state, MessageState::FromEngine | MessageState::ToEngine))
            .count();
        t[6] += graph.dangling().count();
        t[7] += o.dangling_in_game;
    });
    failures.assert_empty();
    let t = totals.into_inner().unwrap();
    eprintln!(
        "{} missions: {} nodes, {} wires, {} messages: {} connected, {} engine, {} dangling in \
         the mission ({} nowhere in the game)",
        t[0], t[1], t[2], t[3], t[4], t[5], t[6], t[7]
    );
    assert!(
        t[4] * 2 > t[3],
        "fewer than half of the messages connect: {t:?}"
    );
}

#[test]
fn logic_graph_edits_round_trip_in_every_mission() {
    use doctor_toolkit::level::Location;
    use doctor_toolkit::logic_graph::{
        BuildOptions, GraphBuilder, LogicSchema, ScopeSpec, WireKind, edit, scope,
    };
    use doctor_toolkit::resource::simgroup::SimGroup;
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let (index, _) = game.build_index(|_| {});
    let failures = Failures::default();
    let edits = std::sync::atomic::AtomicUsize::new(0);
    let bytes_differ = std::sync::atomic::AtomicUsize::new(0);
    let build = |groups: &[(Location, String, SimGroup)]| {
        let mut b = GraphBuilder::new(LogicSchema::gf2());
        for (l, s, g) in groups {
            b.add_group(*l, s, g);
        }
        b.build(BuildOptions {
            hub_threshold: 0,
            ..BuildOptions::default()
        })
    };
    scope::missions(&game).par_iter().for_each(|m| {
        let spec = ScopeSpec::Mission(m.name.clone());
        let locations = scope::groups(&game, &index, &spec).unwrap();
        let (loaded, _) = scope::load(&game, None, &locations);
        let groups: Vec<_> = loaded
            .into_iter()
            .map(|g| (g.location, g.stream, g.group))
            .collect();
        let graph = build(&groups);
        // Disconnect up to 5 message wires one at a time: the message loses
        // exactly one end; restoring the value gives the original bytes.
        let wires: Vec<usize> = (0..graph.wires.len())
            .filter(|&w| graph.wires[w].kind == WireKind::Message)
            .step_by(7)
            .take(5)
            .collect();
        for w in wires {
            let h = graph.wires[w].message.unwrap();
            let before = &graph.messages[&h];
            let changes = match edit::plan_disconnect(&graph, w) {
                Ok(c) => c,
                Err(e) => return failures.push(format!("{}: wire {w}: {e}", m.name)),
            };
            let at = changes[0].at;
            let gi = groups.iter().position(|(l, _, _)| *l == at.group).unwrap();
            let original = groups[gi].2.encode();
            let old = groups[gi]
                .2
                .attr(&groups[gi].2.objects[at.object], at.class, at.index)
                .cloned();
            let mut edited = groups.clone();
            let refs: Vec<&edit::Change> = changes.iter().collect();
            edit::apply_to(&mut edited[gi].2, &refs).unwrap();
            edited[gi].2 = SimGroup::parse(&edited[gi].2.encode()).unwrap();
            let after = build(&edited);
            let (s0, l0) = (before.senders.len(), before.listeners.len());
            let (s1, l1) = after
                .messages
                .get(&h)
                .map_or((0, 0), |m| (m.senders.len(), m.listeners.len()));
            if s0 + l0 != s1 + l1 + 1 {
                failures.push(format!(
                    "{}: {} had {s0}/{l0} ends, {s1}/{l1} after the disconnect",
                    m.name, before.name
                ));
            }
            let mut back = groups[gi].2.clone();
            let refs: Vec<&edit::Change> = changes.iter().collect();
            edit::apply_to(&mut back, &refs).unwrap();
            back.set_class_attr(at.object, at.class, at.index, old)
                .unwrap();
            // The bytes come back unless the cleared packet merged into
            // another one (then the packet order differs); the values do.
            let values = |g: &SimGroup| -> Vec<Vec<(u32, Vec<String>)>> {
                g.objects
                    .iter()
                    .map(|o| {
                        g.object_packets(o)
                            .map(|p| {
                                let v = p.attrs.iter().map(|a| format!("{}", a.value)).collect();
                                (p.class.0, v)
                            })
                            .collect()
                    })
                    .collect()
            };
            let restored = SimGroup::parse(&back.encode()).unwrap();
            if values(&restored) != values(&SimGroup::parse(&original).unwrap()) {
                failures.push(format!(
                    "{}: restoring {} changes the values",
                    m.name, before.name
                ));
            } else if back.encode() != original {
                bytes_differ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            edits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    });
    failures.assert_empty();
    eprintln!(
        "{} disconnects checked, {} restored with other packet order",
        edits.into_inner(),
        bytes_differ.into_inner()
    );
}

#[test]
fn every_animation_bank_reencodes_identically() {
    use doctor_toolkit::resource::anim::{AnimBank, TYPE_BNK};
    use doctor_toolkit::resource::anim_image::BankImage;
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let failures = Failures::default();
    let counts = Mutex::new([0usize; 4]);
    game.streams().par_iter().for_each(|file| {
        let stream = StreamFile::open(&file.path).unwrap();
        for (index, r) in stream.layout().resources.iter().enumerate() {
            if r.header.type_id != TYPE_BNK {
                continue;
            }
            let data = stream.resource_data(index).unwrap();
            let image = match BankImage::parse(&data) {
                Ok(image) => image,
                Err(e) => {
                    failures.push(format!("{} #{index}: {e}", file.rel_path));
                    continue;
                }
            };
            match image.encode() {
                Ok(out) if out == data => {}
                Ok(out) => {
                    let at = out.iter().zip(&data).position(|(a, b)| a != b);
                    failures.push(format!(
                        "{} #{index}: differs at {at:x?} ({} vs {} bytes)",
                        file.rel_path,
                        out.len(),
                        data.len()
                    ));
                    continue;
                }
                Err(e) => {
                    failures.push(format!("{} #{index}: encode: {e}", file.rel_path));
                    continue;
                }
            }
            // The structured events match the reader's.
            let bank = AnimBank::parse(&data).unwrap();
            for (k, clip) in bank.clips.iter().enumerate() {
                if image.events(k) != clip.events {
                    failures.push(format!(
                        "{} #{index} clip {k}: events differ",
                        file.rel_path
                    ));
                }
            }
            let mut c = counts.lock().unwrap();
            c[0] += 1;
            c[1] += image.clips.len();
            c[2] += image.clips.iter().map(|c| c.events.len()).sum::<usize>();
            c[3] += image.entries.len();
        }
    });
    let c = counts.into_inner().unwrap();
    eprintln!(
        "{} banks, {} clips, {} events, {} named entries re-encoded",
        c[0], c[1], c[2], c[3]
    );
    failures.assert_empty();
    assert_eq!(c[0], 2751);
}

/// The node graph of every bank (`resource/anim_graph.rs`): parsed into
/// trees, laid out again byte for byte, slots numbered again as the game's
/// exporter did, through JSON and back unchanged, and no errors from the
/// checks (`docs/formats/anim-graph.md`).
#[test]
fn every_animation_graph_rewrites_identically() {
    use doctor_toolkit::resource::anim::TYPE_BNK;
    use doctor_toolkit::resource::anim_graph::{AnimGraph, GraphJson, Severity};
    use doctor_toolkit::resource::anim_image::BankImage;
    use std::collections::BTreeMap;
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let failures = Failures::default();
    let kinds = Mutex::new(BTreeMap::<u8, usize>::new());
    let counts = Mutex::new([0usize; 4]);
    game.streams().par_iter().for_each(|file| {
        let stream = StreamFile::open(&file.path).unwrap();
        for (index, r) in stream.layout().resources.iter().enumerate() {
            if r.header.type_id != TYPE_BNK {
                continue;
            }
            let at = format!("{} #{index}", file.rel_path);
            let data = stream.resource_data(index).unwrap();
            let image = BankImage::parse(&data).unwrap();
            let graph = match AnimGraph::parse(&image) {
                Ok(g) => g,
                Err(e) => {
                    failures.push(format!("{at}: {e}"));
                    continue;
                }
            };
            match graph.to_block() {
                Ok(block) if block == image.graph => {}
                Ok(block) => {
                    let first = block
                        .data
                        .iter()
                        .zip(&image.graph.data)
                        .position(|(a, b)| a != b);
                    failures.push(format!(
                        "{at}: graph differs at {first:x?} ({} vs {} bytes, {} vs {} pointers)",
                        block.data.len(),
                        image.graph.data.len(),
                        block.pointers.len(),
                        image.graph.pointers.len()
                    ));
                    continue;
                }
                Err(e) => {
                    failures.push(format!("{at}: layout: {e}"));
                    continue;
                }
            }
            if graph.slot_count() != image.node_count {
                failures.push(format!(
                    "{at}: {} slots, header {}",
                    graph.slot_count(),
                    image.node_count
                ));
            }
            let mut renumbered = graph.clone();
            let slots = renumbered.renumber();
            if renumbered != graph || slots != image.node_count {
                let n = renumbered
                    .nodes
                    .iter()
                    .zip(&graph.nodes)
                    .position(|(a, b)| a != b);
                failures.push(format!("{at}: numbering differs at node {n:?}"));
            }
            let names: Vec<String> = image.entry_list().into_iter().map(|e| e.name).collect();
            let ours: Vec<String> = graph.entries.iter().map(|e| e.name.clone()).collect();
            if names != ours {
                failures.push(format!("{at}: entry names differ"));
            }
            let text = serde_json::to_string(&graph.to_json(&image)).unwrap();
            let back: GraphJson = serde_json::from_str(&text).unwrap();
            match AnimGraph::from_json(&back) {
                Ok(g) if g == graph => {}
                Ok(_) => failures.push(format!("{at}: JSON gives another graph")),
                Err(e) => failures.push(format!("{at}: JSON: {e}")),
            }
            let issues = graph.check(&image);
            for i in issues
                .iter()
                .filter(|i| i.severity == Severity::Error)
                .take(3)
            {
                failures.push(format!("{at}: node {:?}: {}", i.node, i.text));
            }
            let mut stored = image.clone();
            graph.store(&mut stored).unwrap();
            if stored != image {
                failures.push(format!("{at}: storing the graph changes the bank"));
            }
            let mut k = kinds.lock().unwrap();
            for n in &graph.nodes {
                *k.entry(n.kind).or_insert(0) += 1;
            }
            let mut c = counts.lock().unwrap();
            c[0] += 1;
            c[1] += graph.entries.len();
            c[2] += graph.nodes.len();
            c[3] += issues.len();
        }
    });
    let c = counts.into_inner().unwrap();
    eprintln!(
        "{} banks, {} entries, {} nodes rewritten; {} warnings; nodes by type: {:x?}",
        c[0],
        c[1],
        c[2],
        c[3],
        kinds.into_inner().unwrap()
    );
    failures.assert_empty();
    assert_eq!(c[0], 2751);
}

/// Every game clip decoded, encoded again and decoded: the error against the
/// game's values stays within the budget; sizes are reported.
#[test]
fn every_animation_clip_reencodes_within_budget() {
    use doctor_toolkit::resource::anim::{AnimBank, TYPE_BNK};
    use doctor_toolkit::resource::anim_encode::{EncodeOptions, reencode};
    use doctor_toolkit::resource::rcb::{Rig, TYPE_RCB};
    use std::collections::HashMap;
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let sets: Mutex<HashMap<u32, Vec<usize>>> = Mutex::new(HashMap::new());
    game.streams().par_iter().for_each(|file| {
        let stream = StreamFile::open(&file.path).unwrap();
        for (index, r) in stream.layout().resources.iter().enumerate() {
            if r.header.type_id == TYPE_RCB {
                let rig = Rig::parse(&stream.resource_data(index).unwrap()).unwrap();
                for set in rig.bone_sets {
                    sets.lock().unwrap().insert(set.hash, set.bones);
                }
            }
        }
    });
    let sets = sets.into_inner().unwrap();
    let options = EncodeOptions::default();
    let failures = Failures::default();
    // Per scheme: clips, original bytes, encoded bytes; all angles and shifts.
    let sizes = Mutex::new(HashMap::<u32, [usize; 3]>::new());
    let errors = Mutex::new((Vec::new(), Vec::new()));
    let codecs = Mutex::new([0usize; 16]);
    game.streams().par_iter().for_each(|file| {
        let stream = StreamFile::open(&file.path).unwrap();
        for (index, r) in stream.layout().resources.iter().enumerate() {
            if r.header.type_id != TYPE_BNK {
                continue;
            }
            let data = stream.resource_data(index).unwrap();
            let bank = AnimBank::parse(&data).unwrap();
            for clip in &bank.clips {
                let Some(bones) = sets.get(&clip.bone_set) else {
                    continue;
                };
                // Scheme 1 clips are written with scheme 1 (its sign stream
                // is inverted), the others with scheme 4.
                let options = EncodeOptions {
                    scheme: if clip.scheme == 1 { 1 } else { 4 },
                    ..options
                };
                match reencode(clip, &data, bones, &options) {
                    Ok(r) => {
                        if r.max_angle > options.max_angle * 1.01
                            || r.max_translation > options.max_translation * 1.01
                        {
                            failures.push(format!(
                                "{} #{index} {}: angle {} shift {}",
                                file.rel_path,
                                clip.display_name(),
                                r.max_angle,
                                r.max_translation
                            ));
                        }
                        let mut s = sizes.lock().unwrap();
                        let e = s.entry(r.scheme).or_default();
                        e[0] += 1;
                        e[1] += r.original_size;
                        e[2] += r.encoded_size;
                        drop(s);
                        let mut e = errors.lock().unwrap();
                        e.0.push(r.max_angle);
                        e.1.push(r.max_translation);
                        drop(e);
                        for (a, b) in codecs.lock().unwrap().iter_mut().zip(r.codecs) {
                            *a += b;
                        }
                    }
                    Err(e) => failures.push(format!(
                        "{} #{index} {}: {e}",
                        file.rel_path,
                        clip.display_name()
                    )),
                }
            }
        }
    });
    let (mut angles, mut shifts) = errors.into_inner().unwrap();
    angles.sort_by(f32::total_cmp);
    shifts.sort_by(f32::total_cmp);
    let pct = |v: &[f32], p: f64| v[((v.len() - 1) as f64 * p) as usize];
    let mut sizes: Vec<_> = sizes.into_inner().unwrap().into_iter().collect();
    sizes.sort();
    for (scheme, [n, original, encoded]) in &sizes {
        eprintln!(
            "scheme {scheme}: {n} clips, {original} bytes in the game, {encoded} re-encoded ({:.2}x)",
            *encoded as f64 / *original as f64
        );
    }
    eprintln!(
        "max angle per clip: median {:.5}, p99 {:.5}, max {:.5} rad; \
         max shift: median {:.6}, p99 {:.6}, max {:.6}",
        pct(&angles, 0.5),
        pct(&angles, 0.99),
        pct(&angles, 1.0),
        pct(&shifts, 0.5),
        pct(&shifts, 0.99),
        pct(&shifts, 1.0)
    );
    eprintln!("codecs: {:?}", codecs.into_inner().unwrap());
    failures.assert_empty();
    assert!(angles.len() > 6000);
}

/// Gameplay clips exported to glTF, FBX and USD, imported back over
/// themselves and decoded: the bank still parses, every other clip is
/// untouched, and the replaced clip matches the original within the
/// encoder's budget.
#[test]
fn animation_clips_import_back_from_gltf_fbx_usd() {
    use doctor_toolkit::export::gltf::{NoAssets, SceneAnimation, scene_to_glb};
    use doctor_toolkit::import::anim::{
        ClipTarget, ImportOptions, import_clip, read_gltf, read_scene,
    };
    use doctor_toolkit::resource::anim::{AnimBank, FRAME_RATE, TYPE_BNK, bone_set, find_skeleton};
    use doctor_toolkit::resource::anim_encode::quat_angle;
    use doctor_toolkit::resource::rcb::{Rig, TYPE_RCB};
    use doctor_toolkit::scene::{self, ModelFormat};
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let mut rigs = Vec::new();
    for file in game.streams() {
        if !file.rel_path.starts_with("gf2/globals_global") {
            continue;
        }
        let stream = StreamFile::open(&file.path).unwrap();
        for (index, r) in stream.layout().resources.iter().enumerate() {
            if r.header.type_id == TYPE_RCB {
                rigs.push(Rig::parse(&stream.resource_data(index).unwrap()).unwrap());
            }
        }
    }
    let file = game
        .streams()
        .iter()
        .find(|f| f.rel_path == "gf2/globals_global.str")
        .unwrap();
    let stream = StreamFile::open(&file.path).unwrap();
    let banks: Vec<usize> = (0..stream.layout().resources.len())
        .filter(|&i| stream.layout().resources[i].header.type_id == TYPE_BNK)
        .collect();
    let options = ImportOptions::default();
    let failures = Failures::default();
    let done = Mutex::new(0usize);
    banks.par_iter().for_each(|&index| {
        let data = stream.resource_data(index).unwrap();
        let bank = AnimBank::parse(&data).unwrap();
        let skeleton = find_skeleton(&rigs, bank.skeleton).unwrap();
        // A few clips per bank: the first, the last and one in the middle.
        let n = bank.clips.len();
        let mut picks = vec![0, n / 2, n - 1];
        picks.dedup();
        for k in picks {
            let clip = &bank.clips[k];
            let Some(bones) = bone_set(&rigs, clip.bone_set) else {
                continue;
            };
            let tracks = clip.decode(&data, bones).unwrap();
            let animation = SceneAnimation {
                name: clip.display_name(),
                skeleton,
                tracks: tracks.clone(),
                frame_rate: FRAME_RATE,
            };
            let glb = scene_to_glb(
                &[],
                Some(skeleton),
                std::slice::from_ref(&animation),
                &mut NoAssets,
            )
            .unwrap();
            let mut sources = vec![("glb", read_gltf(&glb, None).unwrap().remove(0))];
            let built =
                scene::game::build(&[], Some(skeleton), &[animation], &mut NoAssets).unwrap();
            for format in [ModelFormat::Fbx, ModelFormat::Usdc] {
                let bytes = scene::to_bytes(&built, format).unwrap();
                let read = scene::read(&bytes, format, None, "clip").unwrap();
                sources.push((format.extension(), read_scene(&read).unwrap().remove(0)));
            }
            for (format, source) in sources {
                let (out, summary) =
                    match import_clip(&data, &rigs, &source, &ClipTarget::Replace(k), &options) {
                        Ok(r) => r,
                        Err(e) => {
                            failures.push(format!("#{index} clip {k} ({format}): {e}"));
                            continue;
                        }
                    };
                let after = AnimBank::parse(&out).unwrap();
                assert_eq!(after.clips.len(), n);
                for (j, (a, b)) in bank.clips.iter().zip(&after.clips).enumerate() {
                    if a.hash != b.hash || a.events != b.events {
                        failures.push(format!("#{index} clip {j} changed"));
                    }
                    if j != k
                        && let Some(set) = bone_set(&rigs, a.bone_set)
                        && a.decode(&data, set).unwrap() != b.decode(&out, set).unwrap()
                    {
                        failures.push(format!("#{index} clip {j} decodes differently"));
                    }
                }
                let back = after.clips[k].decode(&out, bones).unwrap();
                let mut angle = 0f32;
                let mut shift = 0f32;
                for (a, b) in tracks.iter().zip(&back) {
                    for (q, r) in a.rotations.iter().zip(&b.rotations) {
                        angle = angle.max(quat_angle(*q, *r));
                    }
                    for (s, t) in a.translations.iter().zip(&b.translations) {
                        for c in 0..3 {
                            shift = shift.max((s[c] - t[c]).abs());
                        }
                    }
                }
                if back[0].rotations.len() != tracks[0].rotations.len()
                    || angle > 2.0 * options.encode.max_angle
                    || shift > 2.0 * options.encode.max_translation
                    || !summary.unmapped.is_empty()
                {
                    failures.push(format!(
                        "#{index} clip {k} ({}, {format}): {} frames → {}, angle {angle}, \
                     shift {shift}, unmapped {:?}",
                        clip.display_name(),
                        tracks[0].rotations.len(),
                        back[0].rotations.len(),
                        summary.unmapped
                    ));
                }
                *done.lock().unwrap() += 1;
            }
        }
    });
    failures.assert_empty();
    assert!(*done.lock().unwrap() > 60);
}

/// Cut-scene shots: every `trinity_seq_master` parses, writes back byte for
/// byte (also through JSON), every command is understood, the resource id is
/// the SDBM of the source path and the waits stay within the duration.
#[test]
fn every_trinity_sequence_round_trips() {
    use doctor_toolkit::resource::trinity::{Command, SeqMaster, TYPE_TRINITY_SEQ_MASTER};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let failures = Failures::default();
    let (shots, commands) = (AtomicUsize::new(0), AtomicUsize::new(0));
    game.streams().par_iter().for_each(|file| {
        let stream = StreamFile::open(&file.path).unwrap();
        for (index, resource) in stream.layout().resources.iter().enumerate() {
            if resource.header.type_id != TYPE_TRINITY_SEQ_MASTER {
                continue;
            }
            let at = format!("{} #{index}", file.rel_path);
            let data = stream.resource_data(index).unwrap();
            let seq = match SeqMaster::parse(&data) {
                Ok(seq) => seq,
                Err(e) => return failures.push(format!("{at}: {e}")),
            };
            shots.fetch_add(1, Ordering::Relaxed);
            if seq.to_bytes().ok().as_deref() != Some(&data[..]) {
                failures.push(format!("{at}: does not write back identically"));
            }
            let json = serde_json::to_string(&seq).unwrap();
            let back: SeqMaster = serde_json::from_str(&json).unwrap();
            if back.to_bytes().ok().as_deref() != Some(&data[..]) {
                failures.push(format!("{at}: JSON does not write back identically"));
            }
            if resource.header.ids[3] != SeqMaster::id_for_path(&seq.path.text) {
                failures.push(format!("{at}: id is not the SDBM of `{}`", seq.path.text));
            }
            if seq.blocks.len() != 1 {
                failures.push(format!("{at}: {} blocks", seq.blocks.len()));
            }
            for block in &seq.blocks {
                commands.fetch_add(block.commands.len(), Ordering::Relaxed);
                if let Some(raw) = block
                    .commands
                    .iter()
                    .find(|c| matches!(c, Command::Raw { .. }))
                {
                    failures.push(format!("{at}: unknown {}", raw.label()));
                }
                if block
                    .commands
                    .iter()
                    .any(|c| matches!(c, Command::Wait { seconds } if *seconds < 0.0))
                {
                    failures.push(format!("{at}: negative wait"));
                }
                if block.wait_total() > block.duration + 0.05 {
                    failures.push(format!(
                        "{at}: waits {} past the duration {}",
                        block.wait_total(),
                        block.duration
                    ));
                }
            }
        }
    });
    failures.assert_empty();
    assert_eq!(shots.into_inner(), 260);
    assert!(commands.into_inner() > 10_000);
}

/// Cut-scenes: every stream with shots loads as a cut-scene; shots find their
/// streams, actors their SimGroup objects, clips their banks; sequences have
/// sound tracks with subtitles.
#[test]
fn every_cutscene_resolves() {
    use doctor_toolkit::cutscene::{Cutscene, Source, list_shots, sequences_in};
    use doctor_toolkit::resource::trinity::Command;
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let source = Source::new(&game, None);
    let shots = list_shots(&game).unwrap();
    assert_eq!(shots.len(), 260);
    let mut streams: Vec<usize> = shots.iter().map(|s| s.stream).collect();
    streams.dedup();
    let failures = Failures::default();
    let (mut sequences, mut in_sequences, mut actors, mut clips, mut with_audio) = (0, 0, 0, 0, 0);
    let mut missing_stream = Vec::new();
    for &stream in &streams {
        let found = sequences_in(&source, stream).unwrap();
        sequences += found.len();
        let name = game.streams()[stream].stream_name();
        // The sequence's shots, then each shot no sequence plays.
        let mut loaded = Vec::new();
        if !found.is_empty() {
            loaded.push(Cutscene::load(&source, stream, None).unwrap());
        }
        let linked: Vec<u32> = found.iter().flat_map(|s| s.shots.clone()).collect();
        for s in shots
            .iter()
            .filter(|s| s.stream == stream && !linked.contains(&s.id))
        {
            loaded.push(Cutscene::load(&source, stream, Some(s.resource)).unwrap());
        }
        for cutscene in &loaded {
            if cutscene.sequence.is_some() {
                in_sequences += cutscene.shots.len();
                if cutscene.audio.iter().any(|a| !a.subtitles.is_empty()) {
                    with_audio += 1;
                }
                // Subtitle tables write back byte for byte.
                for a in &cutscene.audio {
                    let key = doctor_toolkit::workspace::FileKey::archive_entry(&a.entry);
                    let snu = source.file(&key).unwrap();
                    if let Some((tob, range)) = doctor_toolkit::resource::tob::Tob::from_snu(&snu)
                        && tob.to_bytes() != snu[range]
                    {
                        failures.push(format!("{}: subtitles differ", a.entry));
                    }
                }
            }
            for shot in &cutscene.shots {
                if shot.stream.is_none() {
                    missing_stream.push(shot.stream_name.clone());
                    continue;
                }
                for a in &shot.actors {
                    actors += 1;
                    if a.class.is_none() {
                        failures.push(format!("{name}: actor {} {} has no object", a.name, a.guid));
                    }
                }
                for c in &shot.block().commands {
                    if let Command::ActorAnim { clip, .. } = c {
                        clips += 1;
                        if shot.clip(&clip.text).is_none() {
                            failures.push(format!("{}: no clip {}", shot.stream_name, clip.text));
                        }
                    }
                }
            }
        }
    }
    failures.assert_empty();
    println!(
        "{} streams, {sequences} sequences ({in_sequences} shots), {actors} actors, {clips} clips, \
         {with_audio} sequences with subtitles; shots without a stream: {missing_stream:?}",
        streams.len()
    );
    assert_eq!(actors, 1502);
    assert_eq!(clips, 1502);
    assert_eq!(sequences, 25);
    assert_eq!(with_audio, 16);
}

/// Cut-scene keys are world coordinates: no sequence object has a matrix,
/// every block transform is the identity, and the zones chosen around the
/// camera path and the actors (stage 61) hold the actors' points.
#[test]
fn cutscene_worlds_hold_their_actors() {
    use doctor_toolkit::cutscene::world::{
        PointKind, ZoneKind, candidate_boxes, choose_zones, is_identity, path_points,
    };
    use doctor_toolkit::cutscene::{Cutscene, Source, list_shots};
    let Some(dir) = game_dir() else { return };
    let game = std::sync::Arc::new(GameInstance::open(&dir).unwrap());
    let (index, _) = game.build_index(|_| {});
    let source = Source::new(&game, None);
    let mut streams: Vec<usize> = list_shots(&game)
        .unwrap()
        .iter()
        .map(|s| s.stream)
        .collect();
    streams.dedup();
    let failures = Failures::default();
    let mut cities = std::collections::BTreeSet::new();
    for &stream in &streams {
        let cutscene = Cutscene::load(&source, stream, None).unwrap();
        let folder = game.streams()[stream].stream_name();
        if let Some(seq) = &cutscene.sequence
            && seq.matrix.is_some()
        {
            failures.push(format!("{folder}: the sequence object has a matrix"));
        }
        for s in 0..cutscene.shots.len() {
            if !is_identity(&cutscene.anchor(s)) {
                failures.push(format!("{folder}: shot {s} has an anchor"));
            }
        }
        let started = std::time::Instant::now();
        let boxes = candidate_boxes(&game, &index, &folder).unwrap();
        let points = path_points(&cutscene, 0.5);
        let chosen = choose_zones(&boxes, &points);
        let elapsed = started.elapsed();
        let chosen_boxes: Vec<_> = boxes
            .iter()
            .filter(|b| chosen.iter().any(|c| c.stream == b.stream))
            .filter(|b| {
                matches!(
                    b.kind,
                    ZoneKind::High | ZoneKind::Interior | ZoneKind::Medium
                )
            })
            .collect();
        // Actors in the level's area: igc02 plays its aeroplane scenes on a
        // set of props 100 m above the origin, far from any zone.
        let actors: Vec<_> = points
            .iter()
            .filter(|p| p.kind == PointKind::Actor)
            .filter(|p| {
                boxes.iter().any(|b| {
                    b.kind != ZoneKind::Sky && !b.is_empty() && b.distance(p.position) < 50.0
                })
            })
            .collect();
        let inside = actors
            .iter()
            .filter(|p| chosen_boxes.iter().any(|b| b.distance(p.position) <= 5.0))
            .count();
        let kinds = |k: ZoneKind| {
            boxes
                .iter()
                .filter(|b| b.kind == k && chosen.iter().any(|c| c.stream == b.stream))
                .count()
        };
        eprintln!(
            "{:<28} {:>4} actor points, {:>4} in zones; {} zones (high {}, medium {}, interior {}, mission {}) of {} in {:.2} s",
            cutscene.name,
            actors.len(),
            inside,
            chosen.len(),
            kinds(ZoneKind::High),
            kinds(ZoneKind::Medium),
            kinds(ZoneKind::Interior),
            kinds(ZoneKind::Mission),
            boxes.len(),
            elapsed.as_secs_f32()
        );
        if let Some(city) = doctor_toolkit::cutscene::world::city_of(&folder) {
            cities.insert(city.to_owned());
        }
        if inside * 10 < actors.len() * 9 {
            failures.push(format!(
                "{folder}: {inside} of {} actor points in chosen zones",
                actors.len()
            ));
        }
        if kinds(ZoneKind::Sky) != 1 {
            failures.push(format!("{folder}: no sky"));
        }
    }
    assert_eq!(cities.len(), 3);
    failures.assert_empty();
}

/// Cut-scene sound: every track of every sequence decodes to its length;
/// the cut-scene's own track starts with it, music where its message is
/// sent, with the volume of the audio script.
#[test]
fn every_cutscene_sound_plays() {
    use doctor_toolkit::cutscene::{Cutscene, Source, list_shots, sequences_in, sound};
    use doctor_toolkit::workspace::FileKey;
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let source = Source::new(&game, None);
    let cues = sound::stream_cues(&sound::sound_scripts(&source).unwrap());
    let mut streams: Vec<usize> = list_shots(&game)
        .unwrap()
        .iter()
        .map(|s| s.stream)
        .collect();
    streams.dedup();
    let failures = Failures::default();
    let (mut sequences, mut tracks, mut own, mut cued, mut outside) = (0, 0, 0, 0, 0);
    let mut seconds = 0.0;
    let mut music = Vec::new();
    for &stream in &streams {
        if sequences_in(&source, stream).unwrap().is_empty() {
            continue;
        }
        sequences += 1;
        let cutscene = Cutscene::load(&source, stream, None).unwrap();
        for a in &cutscene.audio {
            tracks += 1;
            let p = &a.placement;
            match (p.start, &p.message) {
                (Some(t), None) => {
                    own += 1;
                    assert_eq!(t, 0.0);
                    assert_eq!(a.channels, 6, "{}", a.entry);
                }
                (Some(t), Some(m)) => {
                    cued += 1;
                    music.push(format!("{} {t:.3} {m} {}", sound::stem(&a.entry), p.volume));
                }
                (None, _) => outside += 1,
            }
            let snu = source.file(&FileKey::archive_entry(&a.entry)).unwrap();
            match sound::decode(&snu) {
                Ok(pcm) => {
                    let length = pcm.frames() as f64 / pcm.sample_rate as f64;
                    seconds += length;
                    if pcm.channels != a.channels as u16
                        || (length * 1000.0 - a.duration_ms as f64).abs() > 50.0
                    {
                        failures.push(format!(
                            "{}: {} channels, {length:.3} s; the header says {} and {} ms",
                            a.entry, pcm.channels, a.channels, a.duration_ms
                        ));
                    }
                }
                Err(e) => failures.push(format!("{}: {e}", a.entry)),
            }
        }
        // Every cut-scene music a message of the cut-scene starts is a track.
        for (_, m) in sound::messages(&cutscene.shots) {
            for cue in cues.iter().filter(|c| c.message.eq_ignore_ascii_case(&m)) {
                if cue.stream.starts_with("mus_igc")
                    && !cutscene
                        .audio
                        .iter()
                        .any(|a| sound::stem(&a.entry) == cue.stream)
                {
                    failures.push(format!(
                        "{}: `{m}` plays {}, not a track",
                        cutscene.name, cue.stream
                    ));
                }
            }
        }
    }
    failures.assert_empty();
    music.sort();
    println!(
        "{sequences} sequences, {tracks} tracks ({own} own, {cued} started by messages, \
         {outside} started outside), {seconds:.0} s decoded, {} cues\n{}",
        cues.len(),
        music.join("\n")
    );
    assert_eq!(sequences, 25);
    assert_eq!(own, 16);
    assert_eq!(cued, 13);
    assert_eq!(outside, 2);
    assert!(music.contains(&"mus_igc06_02 42.200 music_igc06_02 1".to_owned()));
    assert!(music.contains(&"mus_igc10 11.433 music_igc_10 0.4".to_owned()));
}

// ------------------------------------------------------------ model formats

/// Every rig of the game.
fn all_rigs(game: &GameInstance) -> Vec<doctor_toolkit::resource::rcb::Rig> {
    use doctor_toolkit::resource::rcb::{Rig, TYPE_RCB};
    game.streams()
        .par_iter()
        .flat_map_iter(|file| {
            let stream = StreamFile::open(&file.path).unwrap();
            let layout = stream.layout();
            (0..layout.resources.len())
                .filter(|&i| layout.resources[i].header.type_id == TYPE_RCB)
                .map(|i| Rig::parse(&stream.resource_data(i).unwrap()).unwrap())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Skeletons only (no materials): geometry and skins.
struct RigSource<'a>(&'a [doctor_toolkit::resource::rcb::Rig]);

impl doctor_toolkit::export::gltf::AssetSource for RigSource<'_> {
    fn material(&mut self, _: u32) -> Option<doctor_toolkit::resource::material::Material> {
        None
    }
    fn texture(&mut self, _: u32) -> Option<doctor_toolkit::resource::texture::Texture> {
        None
    }
    fn skeleton(
        &mut self,
        bones: &[(u32, u32)],
    ) -> Option<doctor_toolkit::resource::rcb::Skeleton> {
        self.0.iter().find_map(|r| r.skeleton_for(bones)).cloned()
    }
}

/// Builds the meshes of `scene` (read back from a file) with the original
/// as template and compares the triangles with the original's: positions
/// exact, normals within 0.02, UVs 1e-3, skin joints equal and weights
/// within 1e-3. Returns the number of triangles.
fn import_back(
    original: &doctor_toolkit::resource::mesh::Model,
    template: &doctor_toolkit::resource::mesh_image::MeshImage,
    skeleton: Option<&doctor_toolkit::resource::rcb::Skeleton>,
    scene: &doctor_toolkit::scene::Scene,
    compare_skin: bool,
) -> Result<usize, String> {
    use doctor_toolkit::export::gltf::vertex_skin;
    use doctor_toolkit::import::model;
    use doctor_toolkit::resource::mesh::Model;
    use doctor_toolkit::resource::mesh_build::{self, BuildOptions};

    type Corner = ([f32; 3], [f32; 3], [f32; 2], [u16; 4], [f32; 4]);
    fn corners(model: &Model, bones: usize, records: &[usize]) -> Vec<[Corner; 3]> {
        let mut out = Vec::new();
        for &r in records {
            let record = &model.records[r];
            let v = model.vertices(record).unwrap();
            let skin = vertex_skin(&record.palette, record.rigid_bone, &v, bones);
            for t in model.triangles(record).unwrap() {
                let c = t.map(|i| {
                    let i = i as usize;
                    let (mut j, mut w) = if bones > 0 {
                        skin[i]
                    } else {
                        ([0; 4], [0.0; 4])
                    };
                    let mut pairs: Vec<(u16, f32)> = (0..4)
                        .filter(|&k| w[k] > 0.0)
                        .map(|k| (j[k], w[k]))
                        .collect();
                    pairs.sort_by_key(|p| p.0);
                    (j, w) = ([0; 4], [0.0; 4]);
                    for (k, p) in pairs.iter().enumerate() {
                        j[k] = p.0;
                        w[k] = p.1;
                    }
                    (
                        v.positions[i],
                        v.normals.as_ref().map_or([0.0; 3], |n| n[i]),
                        v.uv0.as_ref().map_or([0.0; 2], |u| u[i]),
                        j,
                        w,
                    )
                });
                let r = (0..3)
                    .min_by(|&a, &b| c[a].0.partial_cmp(&c[b].0).unwrap())
                    .unwrap();
                out.push([c[r], c[(r + 1) % 3], c[(r + 2) % 3]]);
            }
        }
        out.sort_by(|a, b| {
            let key = |t: &[Corner; 3]| [t[0].0, t[1].0, t[2].0];
            key(a).partial_cmp(&key(b)).unwrap()
        });
        out
    }

    let kept: Vec<usize> = (0..original.records.len())
        .filter(|&r| !original.triangles(&original.records[r]).unwrap().is_empty())
        .collect();
    if scene.parts.len() != kept.len() {
        return Err(format!(
            "{} parts for {} records",
            scene.parts.len(),
            kept.len()
        ));
    }
    let mut parts: Vec<_> = scene.parts.iter().map(|p| p.geometry.clone()).collect();
    for (part, &r) in parts.iter_mut().zip(&kept) {
        part.material_hash = original.records[r].material_hash;
        part.name = format!("part{r}");
    }
    if let Some(s) = skeleton {
        model::bind_to_skeleton(scene, s, &mut parts).map_err(|e| e.to_string())?;
    }
    let (built, _) = mesh_build::build(
        &parts,
        &BuildOptions {
            name: original.name.clone(),
            skeleton: skeleton.map(|s| s.bones.iter().map(|b| b.hash).collect()),
            template: Some(template),
            ..Default::default()
        },
    )
    .map_err(|e| e.to_string())?;
    let (m, v) = built.encode().map_err(|e| e.to_string())?;
    let back = Model::parse(&m, &v).map_err(|e| e.to_string())?;
    let bones = skeleton.map_or(0, |s| s.bones.len());
    let mut count = 0;
    for &r in &kept {
        let name = format!("part{r}");
        let records: Vec<usize> = (0..back.records.len())
            .filter(|&b| back.records[b].name == name)
            .collect();
        let a = corners(original, bones, &[r]);
        let b = corners(&back, bones, &records);
        if a.len() != b.len() {
            return Err(format!(
                "record {r}: {} triangles, {} back",
                a.len(),
                b.len()
            ));
        }
        for (ta, tb) in a.iter().zip(&b) {
            for (ca, cb) in ta.iter().zip(tb) {
                let close = |x: &[f32], y: &[f32], tol: f32| {
                    x.iter().zip(y).all(|(p, q)| (p - q).abs() <= tol)
                };
                if ca.0 != cb.0 {
                    return Err(format!("record {r}: position {:?} vs {:?}", ca.0, cb.0));
                }
                if !close(&ca.1, &cb.1, 0.02) {
                    return Err(format!("record {r}: normal {:?} vs {:?}", ca.1, cb.1));
                }
                if !close(
                    &ca.2,
                    &cb.2,
                    1e-3 * (1.0 + ca.2[0].abs().max(ca.2[1].abs())),
                ) {
                    return Err(format!("record {r}: uv {:?} vs {:?}", ca.2, cb.2));
                }
                if compare_skin && (ca.3 != cb.3 || !close(&ca.4, &cb.4, 1e-3)) {
                    return Err(format!(
                        "record {r}: skin {:?} {:?} vs {:?} {:?}",
                        ca.3, ca.4, cb.3, cb.4
                    ));
                }
            }
        }
        count += a.len();
    }
    Ok(count)
}

/// Every model written as FBX, OBJ, USD text, USD crate and USDZ, read back
/// and imported with the original as template gives the same triangles as
/// the original (see [`import_back`]); OBJ, which keeps no skeleton, is
/// compared as geometry. Joints and bind matrices of skinned models come
/// back within 1e-4.
#[test]
fn exported_models_round_trip_through_every_format() {
    use doctor_toolkit::resource::mesh::{Model, TYPE_MESH, volatile_for};
    use doctor_toolkit::resource::mesh_image::MeshImage;
    use doctor_toolkit::scene::{self, ModelFormat, SideFiles};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let rigs = all_rigs(&game);
    const FORMATS: [ModelFormat; 5] = [
        ModelFormat::Fbx,
        ModelFormat::Obj,
        ModelFormat::Usda,
        ModelFormat::Usdc,
        ModelFormat::Usdz,
    ];
    let failures = Failures::default();
    let models = AtomicUsize::new(0);
    let skinned = AtomicUsize::new(0);
    let triangles: Vec<AtomicUsize> = FORMATS.iter().map(|_| AtomicUsize::new(0)).collect();
    let bytes: Vec<AtomicUsize> = FORMATS.iter().map(|_| AtomicUsize::new(0)).collect();
    let started = std::time::Instant::now();
    game.streams().par_iter().for_each(|file| {
        let stream = StreamFile::open(&file.path).unwrap();
        let mut source = RigSource(&rigs);
        for (index, resource) in stream.layout().resources.iter().enumerate() {
            if resource.header.type_id != TYPE_MESH {
                continue;
            }
            let at = format!("{} #{index}", file.rel_path);
            let mesh = stream.resource_data(index).unwrap();
            let volatile = volatile_for(stream.layout(), index)
                .map(|v| stream.resource_data(v).unwrap())
                .unwrap_or_default();
            let original = Model::parse(&mesh, &volatile).unwrap();
            let template = MeshImage::parse(&mesh, &volatile).unwrap();
            models.fetch_add(1, Ordering::Relaxed);
            let Ok(built) = scene::game::model_scene(&original, &mut source) else {
                continue; // no triangles
            };
            let skeleton = if original.bones.is_empty() {
                None
            } else {
                doctor_toolkit::export::gltf::AssetSource::skeleton(&mut source, &original.bones)
            };
            if skeleton.is_some() {
                skinned.fetch_add(1, Ordering::Relaxed);
            }
            for (k, &format) in FORMATS.iter().enumerate() {
                let check = || -> Result<usize, String> {
                    let files = scene::write(&built, format, "model").map_err(|e| e.to_string())?;
                    bytes[k]
                        .fetch_add(files.iter().map(|f| f.bytes.len()).sum(), Ordering::Relaxed);
                    let memory: HashMap<String, Vec<u8>> = files
                        .iter()
                        .map(|f| (f.name.clone(), f.bytes.clone()))
                        .collect();
                    let side = SideFiles::in_memory(memory);
                    let back = match format {
                        ModelFormat::Fbx => scene::fbx::read(&files[0].bytes, &side),
                        ModelFormat::Obj => scene::obj::read(&files[0].bytes, &side, "model"),
                        ModelFormat::Usdz => scene::usd::read_usdz(&files[0].bytes),
                        _ => scene::usd::read(&files[0].bytes, &side),
                    }
                    .map_err(|e| e.to_string())?;
                    back.validate().map_err(|e| e.to_string())?;
                    if format.has_skeletons() {
                        if back.joints.len() != built.joints.len() {
                            return Err(format!(
                                "{} joints, {} back",
                                built.joints.len(),
                                back.joints.len()
                            ));
                        }
                        for (a, b) in built.joints.iter().zip(&back.joints) {
                            let delta = a
                                .inverse_bind
                                .iter()
                                .zip(&b.inverse_bind)
                                .map(|(x, y)| (x - y).abs())
                                .fold(0f32, f32::max);
                            if a.name != b.name || a.parent != b.parent || delta > 1e-4 {
                                return Err(format!(
                                    "joint {} ({:?}) vs {} ({:?}): bind differs by {delta}",
                                    a.name, a.parent, b.name, b.parent
                                ));
                            }
                        }
                        import_back(&original, &template, skeleton.as_ref(), &back, true)
                    } else {
                        // No skin in the file: rigid records take the template's bone.
                        import_back(&original, &template, skeleton.as_ref(), &back, false)
                    }
                };
                match check() {
                    Ok(n) => {
                        triangles[k].fetch_add(n, Ordering::Relaxed);
                    }
                    Err(error) => {
                        failures.push(format!("{at} {} as {format}: {error}", original.name))
                    }
                }
            }
        }
    });
    eprintln!(
        "{} models ({} skinned) in {:.0?}",
        models.load(Ordering::Relaxed),
        skinned.load(Ordering::Relaxed),
        started.elapsed()
    );
    for (k, format) in FORMATS.iter().enumerate() {
        eprintln!(
            "  {format}: {} triangles back, {} MB written",
            triangles[k].load(Ordering::Relaxed),
            bytes[k].load(Ordering::Relaxed) >> 20
        );
    }
    failures.assert_empty();
    assert_eq!(models.into_inner(), 17_622);
}

/// Every animation clip of the game written as glTF, FBX and USD (text,
/// crate, package) on its skeleton and read back onto the same skeleton
/// ([`doctor_toolkit::scene::anim::clips_to_game`]) gives the decoded
/// tracks again: rotations within 1e-4 (|dot| > 0.9999), translations
/// within 1e-4 relative to their size (f32 precision of world matrices).
#[test]
fn animation_clips_round_trip_through_fbx_and_usd() {
    use doctor_toolkit::resource::anim::{AnimBank, FRAME_RATE, TYPE_BNK, bone_set, find_skeleton};
    use doctor_toolkit::scene::anim::{ClipOptions, RestPose, clips_scene, clips_to_game};
    use doctor_toolkit::scene::{self, ModelFormat, SideFiles};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let rigs = all_rigs(&game);
    const FORMATS: [ModelFormat; 5] = [
        ModelFormat::Glb,
        ModelFormat::Fbx,
        ModelFormat::Usda,
        ModelFormat::Usdc,
        ModelFormat::Usdz,
    ];
    let failures = Failures::default();
    let clips = AtomicUsize::new(0);
    let frames = AtomicUsize::new(0);
    let worst_rotation = Mutex::new(1f32);
    let worst_translation = Mutex::new(0f32);
    let started = std::time::Instant::now();
    game.streams().par_iter().for_each(|file| {
        let stream = StreamFile::open(&file.path).unwrap();
        for (index, r) in stream.layout().resources.iter().enumerate() {
            if r.header.type_id != TYPE_BNK {
                continue;
            }
            let data = stream.resource_data(index).unwrap();
            let bank = AnimBank::parse(&data).unwrap();
            let Some(skeleton) = find_skeleton(&rigs, bank.skeleton) else {
                continue;
            };
            for clip in &bank.clips {
                let Some(bones) = bone_set(&rigs, clip.bone_set) else {
                    continue;
                };
                let tracks = clip.decode(&data, bones).unwrap();
                let name = clip.display_name();
                let at = format!("{} #{index} {name}", file.rel_path);
                clips.fetch_add(1, Ordering::Relaxed);
                frames.fetch_add(clip.frames as usize, Ordering::Relaxed);
                let built = clips_scene(skeleton, &[(name.clone(), tracks.clone())], FRAME_RATE);
                // Every format for the first clip of a bank, FBX and the
                // crate for the others (time).
                let formats: &[ModelFormat] = if clip.offset == bank.clips[0].offset {
                    &FORMATS
                } else {
                    &[ModelFormat::Fbx, ModelFormat::Usdc]
                };
                for &format in formats {
                    let check = || -> Result<(), String> {
                        let files =
                            scene::write(&built, format, "clip").map_err(|e| e.to_string())?;
                        let back = match format {
                            ModelFormat::Glb => {
                                doctor_toolkit::import::gltf::read(&files[0].bytes, None)
                            }
                            ModelFormat::Fbx => {
                                scene::fbx::read(&files[0].bytes, &SideFiles::new(None))
                            }
                            ModelFormat::Usdz => scene::usd::read_usdz(&files[0].bytes),
                            _ => scene::usd::read(&files[0].bytes, &SideFiles::new(None)),
                        }
                        .map_err(|e| e.to_string())?;
                        let game_clips = clips_to_game(&back, skeleton, &ClipOptions::default())
                            .map_err(|e| e.to_string())?;
                        let g = &game_clips[0];
                        if g.rest != RestPose::AsIs {
                            return Err(format!("rest pose read as {:?}", g.rest));
                        }
                        if g.frames != clip.frames.max(1) as usize {
                            return Err(format!("{} frames, {} back", clip.frames, g.frames));
                        }
                        let mut rot = 1f32;
                        let mut tra = 0f32;
                        for t in &tracks {
                            let b = &g.tracks[t.bone];
                            for f in 0..t.rotations.len() {
                                let (q, p) = (t.rotations[f], b.rotations[f]);
                                let dot = (0..4).map(|i| q[i] * p[i]).sum::<f32>().abs();
                                rot = rot.min(dot);
                                let (x, y) = (t.translations[f], b.translations[f]);
                                // Relative to the size: cut-scene props move hundreds of metres.
                                let size = (0..3).map(|i| x[i].abs()).fold(1f32, f32::max);
                                let d = (0..3).map(|i| (x[i] - y[i]).abs()).fold(0f32, f32::max);
                                tra = tra.max(d / size);
                            }
                        }
                        {
                            let mut w = worst_rotation.lock().unwrap();
                            *w = w.min(rot);
                            let mut w = worst_translation.lock().unwrap();
                            *w = w.max(tra);
                        }
                        if rot < 0.9999 || tra > 1e-4 {
                            return Err(format!("rotation dot {rot}, translation {tra}"));
                        }
                        Ok(())
                    };
                    if let Err(error) = check() {
                        failures.push(format!("{at} as {format}: {error}"));
                    }
                }
            }
        }
    });
    eprintln!(
        "{} clips, {} frames in {:.0?}; worst rotation dot {}, worst translation {}",
        clips.load(Ordering::Relaxed),
        frames.load(Ordering::Relaxed),
        started.elapsed(),
        worst_rotation.into_inner().unwrap(),
        worst_translation.into_inner().unwrap()
    );
    failures.assert_empty();
    assert_eq!(clips.into_inner(), 6115);
}

/// Unpacking models as OBJ, FBX and USD text and packing an edited model
/// (moved vertices) and an edited texture of another model's side files
/// records mesh replacements in the workspace of a copy of the game; the
/// unedited models record nothing.
#[cfg(unix)]
#[test]
fn unpacked_models_of_any_format_pack_back() {
    use doctor_toolkit::navigation::GameIndex;
    use doctor_toolkit::navigation::unpack::{
        self, FileKind, Manifest, PackOptions, UnpackOptions,
    };
    use doctor_toolkit::resource::mesh::TYPE_MESH;
    use doctor_toolkit::scene::ModelFormat;
    use doctor_toolkit::workspace::Workspace;
    use std::sync::atomic::AtomicBool;

    let Some(dir) = game_dir() else { return };
    const FILES: [&str; 2] = ["gf2/globals_global.str", "gf2/globals_global.toc"];
    for format in [ModelFormat::Obj, ModelFormat::Fbx, ModelFormat::Usda] {
        let shadow = shadow_game(&dir, &FILES);
        let game = GameInstance::open(shadow.path()).unwrap();
        let index = GameIndex::build(&game, &|_, _| {});
        let out = tempfile::tempdir().unwrap();
        let stream = game.find_stream("gf2/globals_global").unwrap();
        let selection = vec![(
            stream,
            unpack::select(&game, &["gf2/globals_global".into()], &[TYPE_MESH])
                .unwrap()
                .remove(0)
                .1
                .into_iter()
                .take(6)
                .collect::<Vec<_>>(),
        )];
        let report = unpack::unpack(
            &game,
            Some(&index),
            &selection,
            out.path(),
            UnpackOptions {
                model_format: format,
                raw: false,
                ..Default::default()
            },
            &|_, _| {},
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(report.failed.is_empty(), "{format}: {:?}", report.failed);
        let manifest = Manifest::read(out.path()).unwrap();
        let models: Vec<_> = manifest
            .files
            .iter()
            .filter(|f| f.kind == FileKind::Model)
            .collect();
        assert_eq!(models.len(), 6, "{format}");
        assert!(models.iter().all(|m| m.path.ends_with(format.extension())));
        let options = PackOptions::default();
        assert!(
            unpack::pack(&game, out.path(), &options)
                .unwrap()
                .changed
                .is_empty()
        );

        // Move the first model: read, shift every position, write again.
        let first = out.path().join(&models[0].path);
        let mut moved = doctor_toolkit::scene::read_file(&first).unwrap();
        for part in &mut moved.parts {
            for p in &mut part.geometry.positions {
                p[1] += 0.25;
            }
        }
        // Only the model file is written again; its side files stay.
        let stem = first.file_stem().unwrap().to_str().unwrap().to_owned();
        let files = doctor_toolkit::scene::write_with_textures(
            &moved,
            format,
            &stem,
            &format!("{stem}_textures/"),
        )
        .unwrap();
        std::fs::write(&first, &files[0].bytes).unwrap();
        // Recolour a texture of another model, if a side file exists.
        let side = manifest.files.iter().find(|f| {
            f.kind == FileKind::ModelFile && f.path.ends_with(".png") && f.keys != models[0].keys
        });
        if let Some(side) = side {
            let path = out.path().join(&side.path);
            let (w, h, mut rgba) =
                doctor_toolkit::resource::pixels::decode_png(&std::fs::read(&path).unwrap())
                    .unwrap();
            for px in rgba.chunks_mut(4) {
                px[0] = 255 - px[0];
            }
            std::fs::write(
                &path,
                doctor_toolkit::resource::pixels::encode_png(w, h, &rgba).unwrap(),
            )
            .unwrap();
        }
        let report = unpack::pack(&game, out.path(), &options).unwrap();
        let expected = 1 + side.is_some() as usize;
        assert_eq!(
            report.changed.len(),
            expected,
            "{format}: {:?}",
            report.changed
        );
        assert!(report.changed.iter().all(|c| c.recorded), "{format}");
        let ws = Workspace::open(report.workspace.unwrap()).unwrap();
        let changed: Vec<String> = ws.changes().map(|c| c.key.to_string()).collect();
        assert!(
            changed.iter().any(|k| *k == models[0].keys[0].to_string()),
            "{format}: {changed:?}"
        );
        eprintln!(
            "{format}: {} change(s) recorded: {changed:?}",
            changed.len()
        );
    }
}

/// Visual scripts (stage 28): a compiled graph is deployed into the always
/// loaded `gf2/globals_global` and into a city stream, updated and removed,
/// in a workspace outside the game (the game is only read).
#[test]
fn visual_scripts_deploy_update_and_remove_in_a_workspace() {
    use doctor_toolkit::level::{Location, edit::current_group};
    use doctor_toolkit::resource::lua::{Chunk, TYPE_LUA};
    use doctor_toolkit::resource::simgroup::{SimGroup, Value};
    use doctor_toolkit::script::{Node, ScriptGraph, compile, deploy, sim::Sim};
    use doctor_toolkit::workspace::Workspace;

    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let (index, _) = game.build_index(|_| {});
    let tmp = tempfile::tempdir().unwrap();
    let mut ws = Workspace::for_game(&game, Some(&tmp.path().join("ws"))).unwrap();

    let mut g = ScriptGraph::new("doctor_corpus_test");
    let ev = g.add(Node::new(0, "event.message", [0.0; 2]).param("message", "audio_init"));
    let send = g.add(Node::new(0, "action.send_message", [0.0; 2]).input("message", "doctor_x"));
    g.connect((ev, "then"), (send, "exec"));
    let c = compile(&g);
    let chunk = c.chunk.clone().unwrap();
    let mut sim = Sim::new();
    sim.load(&chunk).unwrap();
    sim.start("doctor_corpus_test.lua").unwrap();
    sim.send("audio_init").unwrap();
    assert_eq!(sim.sent(), ["doctor_x"]);

    for name in ["gf2/globals_global", "gf2/cuba/local"] {
        let stream = game.find_stream(name).unwrap();
        let group_at = Location {
            stream,
            resource: deploy::simgroup_of(&game, stream).unwrap(),
        };
        let before = current_group(&game, &ws, group_at).unwrap().objects.len();
        let d = deploy::deploy(&game, &index, &mut ws, stream, &g, &chunk).unwrap();
        assert!(d.changed && !d.updated, "{name}");
        assert_eq!(d.object, before, "{name}: the object is appended");
        let group = current_group(&game, &ws, group_at).unwrap();
        assert_eq!(group.objects.len(), before + 1);
        let again = SimGroup::parse(&group.encode()).unwrap();
        assert_eq!(again.objects.len(), before + 1);
        let o = &group.objects[d.object];
        assert_eq!(o.class, deploy::CLASS_LUA_AUDIO_SCRIPT);
        let found = deploy::find(&game, &ws, stream, "doctor_corpus_test.lua")
            .unwrap()
            .unwrap();
        assert_eq!(found.objects, [d.object]);
        assert_eq!(
            group.attr(o, deploy::CLASS_LUA_AUDIO_SCRIPT, 3),
            Some(&Value::Guid(found.guid()))
        );
        assert_eq!(d.key.type_id, TYPE_LUA);
        let stored = ws.data(&d.key).unwrap().unwrap();
        assert_eq!(
            Chunk::parse(&stored).unwrap().source(),
            "doctor_corpus_test.lua"
        );
        // The new id is not used by any resource of the game.
        assert!(
            index
                .entries
                .iter()
                .all(|e| !e.header.ids.contains(&d.key.ids[0]))
        );

        let mut g2 = g.clone();
        g2.start.on_load = false;
        g2.start.message = Some("audio_init_delay".into());
        let d2 = deploy::deploy(&game, &index, &mut ws, stream, &g2, &chunk).unwrap();
        assert!(d2.updated && d2.changed);
        assert_eq!((d2.object, &d2.key), (d.object, &d.key));
        assert!(deploy::remove(&game, &mut ws, stream, "doctor_corpus_test.lua").unwrap());
        assert_eq!(
            current_group(&game, &ws, group_at).unwrap().objects.len(),
            before
        );
        assert!(ws.change(&d.key).is_none());
    }
}

/// Spawn points for `play --stream … --spawn` (stage 32): the PlayerSpawn
/// objects of the cities and their GUIDs in the text form the game parses.
#[test]
fn player_spawns_of_the_cities() {
    use doctor_toolkit::play::start::{self, StartOptions};
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let cuba = start::spawn_points(&game, "gf2/cuba").unwrap();
    assert_eq!(cuba.len(), 2, "{cuba:#?}");
    assert!(
        cuba.iter()
            .all(|p| p.kind == Some(2) && p.stream == "gf2/cuba")
    );
    // The hotel terrace: the point the first checkpoint of the story loads.
    let terrace = cuba
        .iter()
        .find(|p| p.message.as_deref() == Some("checkpoint_001_load"))
        .expect("terrace spawn");
    assert_eq!(terrace.guid.to_string(), "26d7c8323dfa11aa464f574c45523130");
    assert_eq!(terrace.arg(), "{32C8D726-FA3D-AA11-464F-574C45523130}");
    let at = terrace.position.unwrap();
    assert!((at[0] - 166.76).abs() < 0.01 && (at[1] - 21.2).abs() < 0.01);
    assert!((at[2] + 28.09).abs() < 0.01);
    let local = start::spawn_points(&game, "gf2\\cuba\\local").unwrap();
    assert_eq!(local.len(), 2, "{local:#?}");
    assert!(
        local
            .iter()
            .any(|p| p.arg() == "{32C85C9C-E2F0-1888-5245-4953444F5246}")
    );
    for city in [
        "gf2/miami",
        "gf2/queens",
        "gf2/miami/local",
        "gf2/queens/local",
    ] {
        let points = start::spawn_points(&game, city).unwrap();
        assert!(!points.is_empty(), "{city}");
        for point in points {
            let options = StartOptions {
                streams: vec![city.into()],
                spawn: Some(point.guid.to_string()),
                ..StartOptions::default()
            };
            let line = options.cmd_txt().unwrap();
            assert!(line.contains(&point.arg()), "{line}");
            assert!(line.ends_with("\r\n") && !line.contains(".str"), "{line}");
        }
    }
    assert!(start::spawn_points(&game, "gf2/no_such_stream").is_err());

    // Stage 33: the streams the Play options offer, "Play from here".
    let levels = start::level_streams(&game);
    assert_eq!(levels[..3], ["gf2/cuba", "gf2/miami", "gf2/queens"]);
    assert!(levels.iter().any(|s| s == "mp_mall/mp_mall"), "{levels:?}");
    assert!(
        levels
            .iter()
            .all(|s| s.split('/').count() == 2 && !s.ends_with("_global") && s != "gf2/frontend"),
        "{levels:?}"
    );
    assert_eq!(
        start::top_level_stream("gf2\\cuba\\local\\high\\business_019.str").as_deref(),
        Some("gf2/cuba")
    );
    // The point of a PlayerSpawn leads back to its object.
    for point in &cuba {
        let file = game
            .open_stream(game.find_stream("gf2/cuba").unwrap())
            .unwrap();
        let group = doctor_toolkit::resource::simgroup::SimGroup::parse(
            &file.resource_data(point.resource).unwrap(),
        )
        .unwrap();
        assert_eq!(group.objects[point.object].guid, point.guid);
    }
    // The nearest point to start at skips points that start a mission.
    let (near, _) = start::nearest_spawn(&cuba, terrace.position.unwrap()).unwrap();
    assert!(near.message.is_none());
}

#[test]
fn logic_runs_of_missions_go_without_the_game() {
    use doctor_toolkit::logic_graph::ScopeSpec;
    use doctor_toolkit::logic_graph::run::{
        self, Input, Model, Run, RunOptions, Stop, Until, What,
    };
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let (index, _) = game.build_index(|_| {});
    // Inputs: load, the engine's checkpoint messages, the player walking
    // into every trigger volume one second after another.
    let start = |spec: &ScopeSpec| {
        let (input, errors) = run::load(&game, &index, None, spec).unwrap();
        assert!(errors.is_empty(), "{:?}", errors.first());
        let mut r = Run::new(input, RunOptions::default());
        r.schedule(0.0, Input::Load);
        r.schedule(0.0, Input::Message("iMsgNewCheckpointLoaded".into()));
        r.schedule(0.5, Input::Message("iMsgNewCheckpointStarted".into()));
        let triggers: Vec<usize> = (0..r.actors().len())
            .filter(|&i| matches!(r.actors()[i].model, Model::Trigger { .. }))
            .collect();
        for (k, i) in triggers.iter().enumerate() {
            r.schedule(1.0 + k as f64, Input::Enter(format!("#{i}")));
        }
        r
    };
    let spec = ScopeSpec::parse(&game, "mission:killingcastro").unwrap();
    let mut r = start(&spec);
    let stop = r.run(Until::Time(600.0), 20_000);
    assert!(
        matches!(stop, Stop::Time | Stop::Idle | Stop::Limit),
        "{stop:?}"
    );
    let sends = r
        .journal()
        .iter()
        .filter(|e| matches!(e.what, What::Send { .. }))
        .count();
    let black = r.actors().iter().filter(|a| a.model.is_black_box()).count();
    eprintln!(
        "killingcastro: {} objects ({black} black boxes), {} steps, {} entries, {sends} messages, {:.1} s",
        r.actors().len(),
        r.steps(),
        r.journal().len(),
        r.time()
    );
    assert!(sends > 20, "{sends} messages");
    // Deterministic: the same inputs give the same journal.
    let mut again = start(&spec);
    again.run(Until::Time(600.0), 20_000);
    assert_eq!(
        serde_json::to_string(r.journal()).unwrap(),
        serde_json::to_string(again.journal()).unwrap()
    );

    // Every mission runs without a panic.
    let mut totals = [0usize; 4];
    for m in doctor_toolkit::logic_graph::scope::missions(&game) {
        let mut r = start(&ScopeSpec::Mission(m.name.clone()));
        r.run(Until::Time(600.0), 20_000);
        totals[0] += 1;
        totals[1] += r.actors().len();
        totals[2] += r.actors().iter().filter(|a| a.model.is_black_box()).count();
        totals[3] += r
            .journal()
            .iter()
            .filter(|e| matches!(e.what, What::Send { .. }))
            .count();
    }
    eprintln!("missions, objects, black boxes, messages: {totals:?}");
    assert!(totals[0] >= 70 && totals[3] > 500, "{totals:?}");

    // The sound scripts of the game start on the Lua VM.
    let spec = ScopeSpec::parse(&game, "stream:gf2/globals_global").unwrap();
    let (input, _) = run::load(&game, &index, None, &spec).unwrap();
    assert!(input.chunks.len() >= 25, "{} chunks", input.chunks.len());
    let mut r = Run::new(input, RunOptions::default());
    r.schedule(0.0, Input::Load);
    r.run(Until::Time(30.0), 20_000);
    let loaded = r
        .journal()
        .iter()
        .filter(|e| matches!(&e.what, What::Lua { text } if text.ends_with("loaded, startup")))
        .count();
    eprintln!("globals_global: {loaded} scripts started");
    assert!(loaded >= 5, "{loaded} scripts started");
}

/// Stage 47: a move made in the running game goes into the workspace as
/// the matrix of its SimGroup record (the G-STAN plane of Havana, found by
/// GUID through the names of the live link), one operation; undo puts the
/// record back.
#[test]
fn moves_of_the_live_link_go_into_the_workspace() {
    use doctor_toolkit::level::edit::current_group;
    use doctor_toolkit::live::{commit, names::LiveNames, proto};
    use doctor_toolkit::workspace::Workspace;

    let Some(dir) = game_dir() else { return };
    const REL: &str = "gf2/cuba/local/yuri/medium/zone4.str";
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir_all(root.join(REL).parent().unwrap()).unwrap();
    std::fs::copy(dir.join(REL), root.join(REL)).unwrap();
    // A table of contents near the top makes the folder a game.
    std::fs::copy(dir.join("gf2/cuba.toc"), root.join("gf2/cuba.toc")).unwrap();
    let game = GameInstance::open(root).unwrap();
    let (index, _) = game.build_index(|_| {});
    let names = LiveNames::build(&game, &index, None);
    let plane = proto::parse_guid("ff32c9362ecd3b0547524148414d3034").unwrap();
    let (at, object) = commit::locate(&game, &names, &plane).expect("the plane");
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let before = current_group(&game, &ws, at).unwrap();
    let old = before.transform(&before.objects[object]).unwrap();
    let flat: proto::Matrix = std::array::from_fn(|i| old[i / 4][i % 4]);
    let moved = proto::edit::with_position(
        &proto::edit::rotate_y(&flat, 45f32.to_radians()),
        [288.08, 0.0, 369.44],
    );
    let lost = proto::parse_guid("00112233445566778899aabbccddeeff").unwrap();
    let done =
        commit::commit_moves(&game, &mut ws, &names, &[(plane, moved), (lost, moved)]).unwrap();
    assert_eq!(done.written, vec![(plane, at, object, true)]);
    assert_eq!(done.skipped.len(), 1);
    assert_eq!(ws.head(), 1);
    assert!(done.operation.is_some());
    let after = current_group(&game, &ws, at).unwrap();
    assert_eq!(
        after.transform(&after.objects[object]).unwrap(),
        commit::rows(&moved)
    );
    assert_eq!(after.objects.len(), before.objects.len());
    ws.undo().unwrap();
    let back = current_group(&game, &ws, at).unwrap();
    assert_eq!(back.transform(&back.objects[object]).unwrap(), old);
}

/// Stage 62: a camera key of every shot changed in place — the shot reads
/// back with the new key, and its bytes differ only in that value; the
/// same for a key of the focal length.
#[test]
fn camera_keys_change_in_place() {
    use doctor_toolkit::cutscene::edit::{KeyEdit, set_camera, set_focal_length};
    use doctor_toolkit::cutscene::{Source, list_shots};
    use doctor_toolkit::resource::trinity::{Command, SeqMaster, eval_curves};
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let source = Source::new(&game, None);
    let failures = Failures::default();
    let (mut checked, mut skipped, mut focal) = (0, 0, 0);
    for s in list_shots(&game).unwrap() {
        let file = game.open_stream(s.stream).unwrap();
        let data = source.resource(s.stream, &file, s.resource).unwrap();
        let seq = SeqMaster::parse(&data).unwrap();
        let Some(block) = seq.block() else { continue };
        let times = block.times();
        let Some((index, curves)) = block
            .commands
            .iter()
            .enumerate()
            .find_map(|(i, c)| match c {
                Command::CameraCurves { curves, .. } => Some((i, curves)),
                _ => None,
            })
        else {
            continue;
        };
        // A time every curve has a key at.
        let Some(key) = curves[0].iter().map(|k| k.time).find(|&t| {
            curves
                .iter()
                .all(|c| c.iter().any(|k| (k.time - t).abs() < 1e-6))
        }) else {
            skipped += 1;
            continue;
        };
        let mut values = eval_curves(curves, key);
        values[0] += 1.0;
        let mut edited = seq.clone();
        match set_camera(&mut edited, times[index] + key, values) {
            Ok(KeyEdit::Replaced) => {}
            other => {
                failures.push(format!("{}: {other:?}", s.name));
                continue;
            }
        }
        let bytes = edited.to_bytes().unwrap();
        let changed = data.iter().zip(&bytes).filter(|(a, b)| a != b).count();
        if bytes.len() != data.len() || changed == 0 || changed > 4 {
            failures.push(format!(
                "{}: {} → {} bytes, {changed} changed",
                s.name,
                data.len(),
                bytes.len()
            ));
        }
        if SeqMaster::parse(&bytes).unwrap() != edited {
            failures.push(format!("{}: reads back otherwise", s.name));
        }
        // The focal length at the start of its curve.
        if let Some(i) = block
            .commands
            .iter()
            .position(|c| matches!(c, Command::CameraFov { keys, .. } if keys.first().is_some_and(|k| k.time == 0.0 && k.value != 33.0)))
        {
            let mut fov = seq.clone();
            if set_focal_length(&mut fov, times[i], 33.0).ok() != Some(KeyEdit::Replaced) {
                failures.push(format!("{}: no focal length key", s.name));
            }
            let bytes = fov.to_bytes().unwrap();
            let changed = data.iter().zip(&bytes).filter(|(a, b)| a != b).count();
            if bytes.len() != data.len() || changed == 0 || changed > 4 {
                failures.push(format!("{}: focal length: {changed} bytes changed", s.name));
            }
            focal += 1;
        }
        checked += 1;
    }
    println!(
        "{checked} shots keyed in place ({focal} focal lengths), {skipped} without a key on every curve"
    );
    assert!(checked > 200, "{checked}");
    failures.assert_empty();
}

#[test]
fn every_city_keys_its_light_by_the_time_of_day() {
    use doctor_toolkit::time_of_day::CityLight;
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let sum = |c: [f32; 3]| c[0] + c[1] + c[2];
    for city in ["cuba", "queens", "miami"] {
        let light = CityLight::read(&game, city).unwrap();
        let seq = &light.outdoor;
        assert!(
            seq.fog.len() >= 4
                && seq.sky.len() >= 2
                && !seq.lighting.is_empty()
                && !seq.ambient.is_empty(),
            "{city} outdoors: {} fog, {} sky, {} lighting, {} ambient keys",
            seq.fog.len(),
            seq.sky.len(),
            seq.lighting.len(),
            seq.ambient.len()
        );
        let noon = light.at(12 * 3600);
        let night = light.at(0);
        assert!(
            sum(noon.sky.high) > sum(night.sky.high),
            "{city}: the day sky is brighter"
        );
        assert!(noon.fog.end > noon.fog.start, "{city}: {:?}", noon.fog);
        let up = noon.ambient.eval([0.0, 1.0, 0.0]);
        assert!(sum(up) > 0.0, "{city}: ambient light at noon {up:?}");
    }
    // Havana a little after noon, as the game shows it (`live peek`, renderer.md §10).
    let cuba = CityLight::read(&game, "cuba").unwrap();
    let noon = cuba.at(12 * 3600 + 180);
    assert!((noon.lighting.shadow - 0.675).abs() < 1e-3);
    assert!((noon.lighting.wrap - 0.25).abs() < 1e-3);
    assert!(
        noon.fog.start > 10.0 && noon.fog.end < 2000.0,
        "{:?}",
        noon.fog
    );
}

/// Stage 75 (docs/analysis/handing-out.md §7): how the dialogues of the game
/// use choices and the three variables whose assignment makes the other
/// actor send a message (0x23, 0x27, 0x24).
#[test]
fn dialogue_choices_and_result_variables() {
    use doctor_toolkit::resource::dlg::{Dialogue, KIND_ASSIGN, KIND_CHOICE};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let choices = AtomicUsize::new(0);
    let with_choice = AtomicUsize::new(0);
    let with_prelude = AtomicUsize::new(0);
    let sent = [0x23u32, 0x27, 0x24].map(|v| (v, AtomicUsize::new(0)));
    let count = for_each_resource("dlg", |_, data| {
        let d = Dialogue::parse(data).map_err(|e| e.to_string())?;
        let mut has_choice = false;
        let mut has_prelude = false;
        for node in &d.nodes {
            if node.kind == KIND_CHOICE {
                has_choice = true;
                choices.fetch_add(1, Ordering::Relaxed);
                // Options 0, 1, 2, the time-out, option 4: never more.
                if !matches!(node.children.len(), 2 | 4 | 5) {
                    return Err(format!("a choice with {} branches", node.children.len()));
                }
            }
            if node.kind == KIND_ASSIGN {
                let variable = node.word(2).unwrap_or(u32::MAX);
                has_prelude |= variable == 0x49;
                for (v, n) in &sent {
                    if variable == *v {
                        if node.word(0) != Some(0) || node.word(3) != Some(1) {
                            return Err(format!("variable {v:#x} is not set to true"));
                        }
                        n.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        if has_choice {
            with_choice.fetch_add(1, Ordering::Relaxed);
            if has_prelude {
                with_prelude.fetch_add(1, Ordering::Relaxed);
            }
        }
        Ok(())
    });
    if game_dir().is_some() {
        assert_eq!(count, 143);
        assert_eq!(choices.load(Ordering::Relaxed), 122);
        assert_eq!(with_choice.load(Ordering::Relaxed), 45);
        assert_eq!(with_prelude.load(Ordering::Relaxed), 41);
        let sent: Vec<usize> = sent
            .iter()
            .map(|(_, n)| n.load(Ordering::Relaxed))
            .collect();
        assert_eq!(sent, [139, 6, 9]);
    }
}

/// The game builds `<folder>/<stream>` in a 256-byte buffer (exe
/// `0x00AC37C0`): the longest stream path of the game decides how long the
/// path of the game folder may be (docs/analysis/story.md §9).
#[test]
fn the_longest_stream_path_is_142_characters() {
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let (stream, len) = doctor_toolkit::path_limit::longest_stream(&game).unwrap();
    assert_eq!(
        len,
        doctor_toolkit::path_limit::LONGEST_GAME_STREAM,
        "{stream}"
    );
    assert!(
        stream.ends_with("m11_common_inc_restart_folderstream.str"),
        "{stream}"
    );
    // The stream of checkpoint 326, whose path overflowed the buffer on the bench.
    let cp_326 = game
        .streams()
        .iter()
        .find(|f| f.rel_path.ends_with("m03_carmine_compound_exp.str"))
        .unwrap();
    assert_eq!(cp_326.rel_path.len(), 132);
}

/// `docs/formats/bricknmortarres.md`: every venue table is written back byte
/// for byte, its numbers and GUIDs are unique, and a table with a venue
/// added after one of the game still parses.
#[test]
fn every_venue_table_round_trips() {
    use doctor_toolkit::resource::bricknmortar::VenueTable;
    let count = for_each_resource("bricknmortarres", |_, data| {
        let table = VenueTable::parse(data).map_err(|e| e.to_string())?;
        if table.to_bytes().map_err(|e| e.to_string())? != data {
            return Err("does not round-trip".into());
        }
        if let Some(problem) = table.validate().first() {
            return Err(problem.clone());
        }
        let mut more = table.clone();
        let mut copy = table.venues[0].clone();
        copy.id = 9999;
        copy.guid = "00112233445566778899aabbccddeeff".into();
        more.venues.push(copy);
        let again = VenueTable::parse(&more.to_bytes().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        if again != more {
            return Err("a table with a new venue does not parse back".into());
        }
        Ok(())
    });
    if game_dir().is_some() {
        assert!(count >= 8, "{count} venue table(s)");
    }
}

/// `docs/formats/bricknmortarres.md` §3: the game keeps the venues of every
/// loaded city in one table sorted by number, so the numbers of the three
/// cities of the single-player game never repeat (the multiplayer maps are
/// never loaded together and share 997 and 998).
#[test]
fn venue_numbers_of_the_cities_are_unique() {
    use doctor_toolkit::level::venues;
    use doctor_toolkit::workspace::Workspace;
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let ws =
        Workspace::for_game(&game, Some(&std::env::temp_dir().join("doctor-venues-ws"))).unwrap();
    let tables = venues::all_venue_tables(&game, &ws).unwrap();
    let mut seen = std::collections::HashMap::new();
    for (name, city) in &tables {
        if !matches!(name.as_str(), "gf2/queens" | "gf2/miami" | "gf2/cuba") {
            continue;
        }
        for v in &city.table.venues {
            if let Some(other) = seen.insert(v.id, name.clone()) {
                panic!("venue {} is in {other} and {name}", v.id);
            }
        }
    }
    assert_eq!(seen.get(&230).map(String::as_str), Some("gf2/cuba"));
    assert!(tables.len() >= 8, "{} venue tables", tables.len());
}

/// A mesh of the game in two streams under the same ids (the van of two
/// assemblies of `vehicle_shared`): replacing it in one replaces the copy
/// too, with the new material and texture in both streams; the game takes
/// the mesh from whichever stream it loaded. `this_stream_only` leaves the
/// copy as it was.
#[test]
fn model_import_replaces_every_copy_of_a_mesh() {
    use doctor_toolkit::create;
    use doctor_toolkit::export::gltf as export;
    use doctor_toolkit::game::GameAssets;
    use doctor_toolkit::import::{gltf as import, model};
    use doctor_toolkit::resource::mesh::{self, TYPE_MESH};
    use doctor_toolkit::workspace::{ApplyOptions, Workspace};

    let Some(dir) = game_dir() else { return };
    let real = GameInstance::open(&dir).unwrap();
    let names = [
        "gf2/globalfolder/vehicles/assembly/vehicle_shared/van60_01",
        "gf2/globalfolder/vehicles/assembly/vehicle_shared/van60_02",
    ];
    let mut files: Vec<String> = Vec::new();
    for name in names {
        files.push(
            real.streams()[real.find_stream(name).unwrap()]
                .rel_path
                .clone(),
        );
        for ancestor in real.stream_ancestors(name).unwrap() {
            if let Some(i) = real.find_stream(&ancestor) {
                let rel = real.streams()[i].rel_path.clone();
                if !files.contains(&rel) {
                    files.push(rel);
                }
            }
        }
    }
    for toc in real.tocs() {
        files.push(toc.rel_path.clone());
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for rel in &files {
        std::fs::create_dir_all(root.join(rel).parent().unwrap()).unwrap();
        std::fs::copy(dir.join(rel), root.join(rel)).unwrap();
    }
    let game = std::sync::Arc::new(GameInstance::open(root).unwrap());
    let (a, b) = (
        game.find_stream(names[0]).unwrap(),
        game.find_stream(names[1]).unwrap(),
    );
    let (index, _) = game.build_index(|_| {});
    let mut assets = GameAssets::new(game.clone(), &index);
    let baseline = |s: usize| -> std::collections::BTreeSet<_> {
        create::check_references(&game, s)
            .unwrap()
            .into_iter()
            .map(|u| u.target)
            .collect()
    };
    let (base_a, base_b) = (baseline(a), baseline(b));

    // A mesh of the first stream the second has too.
    let (mesh_a, ids) = {
        let sa = game.open_stream(a).unwrap();
        let sb = game.open_stream(b).unwrap();
        sa.layout()
            .resources
            .iter()
            .enumerate()
            .filter(|(_, r)| r.header.type_id == TYPE_MESH)
            .find(|(_, r)| {
                sb.layout()
                    .resources
                    .iter()
                    .any(|x| x.header.type_id == TYPE_MESH && x.header.ids == r.header.ids)
            })
            .map(|(i, r)| (i, r.header.ids))
            .expect("the two assemblies share a mesh")
    };
    let original = mesh::load(&game.open_stream(a).unwrap(), mesh_a).unwrap();
    let glb = export::model_to_glb(&original, &mut assets).unwrap();
    let mut gltf = import::read(&glb, None).unwrap();
    // Recoloured: a new material with a new texture.
    let red: Vec<u8> = (0..64 * 64).flat_map(|_| [200, 20, 20, 255]).collect();
    let png = doctor_toolkit::resource::pixels::encode_png(64, 64, &red).unwrap();
    let material = gltf.parts[0].material.expect("the export has materials");
    gltf.materials[material].base_color = Some(import::GltfImage {
        name: "red".into(),
        mime_type: "image/png".into(),
        bytes: png,
    });

    // Only this stream: nothing for the copy.
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let only = model::plan(
        &game,
        &ws,
        &index,
        &mut assets,
        a,
        mesh_a,
        &gltf,
        &model::ImportOptions {
            this_stream_only: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(only.summary.copies.is_empty(), "{:?}", only.summary);

    let plan = model::plan(
        &game,
        &ws,
        &index,
        &mut assets,
        a,
        mesh_a,
        &gltf,
        &model::ImportOptions::default(),
    )
    .unwrap();
    assert_eq!(
        plan.summary.copies,
        vec![names[1].to_owned()],
        "{:?}",
        plan.summary
    );
    assert_eq!(plan.summary.materials_new, 1, "{:?}", plan.summary);
    plan.commit(&mut ws, &game).unwrap().unwrap();
    ws.apply(&game, ApplyOptions::default()).unwrap();

    // Both streams hold the same new mesh, and what it names resolves.
    let mesh_data = |s: usize| {
        let file = game.open_stream(s).unwrap();
        let i = file
            .layout()
            .resources
            .iter()
            .position(|r| r.header.type_id == TYPE_MESH && r.header.ids == ids)
            .unwrap();
        (
            file.resource_data(i).unwrap(),
            mesh::load(&file, i).unwrap(),
        )
    };
    let ((data_a, new_a), (data_b, new_b)) = (mesh_data(a), mesh_data(b));
    assert_eq!(data_a, data_b);
    assert_ne!(new_a.records.len(), 0);
    for record in &new_b.records {
        new_b.vertices(record).unwrap();
        new_b.triangles(record).unwrap();
    }
    for (s, base) in [(a, &base_a), (b, &base_b)] {
        let after = baseline(s);
        assert!(
            after.is_subset(base),
            "{}: new unresolved {:?}",
            game.streams()[s].stream_name(),
            after.difference(base)
        );
    }

    // Undone and unapplied: the original files again.
    ws.undo().unwrap().unwrap();
    ws.apply(&game, ApplyOptions::default()).unwrap();
    ws.unapply(&game, false).unwrap();
    for rel in &files {
        assert_eq!(
            std::fs::read(root.join(rel)).unwrap(),
            std::fs::read(dir.join(rel)).unwrap(),
            "{rel}"
        );
    }
}
