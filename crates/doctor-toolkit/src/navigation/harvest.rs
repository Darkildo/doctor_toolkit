//! Reading names, references and hashes out of resource data.
//!
//! One function per resource type. Types whose data is large and holds no
//! names (texture pixels, vertex data, Havok physics, sound samples) are not
//! read at all; types that are not parsed yet contribute their NUL-terminated
//! strings.

use std::collections::HashSet;

use rayon::prelude::*;

use crate::game::{GameFile, StreamFile};
use crate::hash::sdbm;
use crate::resource::logic::{
    TYPE_GRAPH, TYPE_METAMODEL, TYPE_PARTSDB, TYPE_SIMGROUP, TYPE_VARIABLE_DICTIONARY,
};
use crate::resource::mesh::{TYPE_MESH, TYPE_MESH_VOLATILE};
use crate::resource::texture::{TYPE_TG4D, TYPE_TG4H};
use crate::resource::{
    anim, apx, audio, ffn, graph, hkx, lh2, lua, material, material_swap, mesh, metamodel, partsdb,
    rcb, simgroup, tg4, treemodel, variabledictionary,
};

use super::index::{Anchor, Chunk, HashKind, NameKind, Record, Ref, Stamp, Target, TextEntry};

const TYPE_MATERIAL: u32 = sdbm(b"material");
const TYPE_APX: u32 = sdbm(b"apx");
const TYPE_LH2: u32 = sdbm(b"lh2");
const TYPE_VFX: u32 = sdbm(b"vfx");

/// Types whose data is not read: large and without names.
const SKIPPED: [u32; 4] = [
    TYPE_TG4D,
    TYPE_MESH_VOLATILE,
    hkx::TYPE_HKX,
    audio::TYPE_SBK,
];

/// At most this many strings are kept from a resource of an unparsed type.
const MAX_LOOSE_STRINGS: usize = 64;

/// Reads every resource of a stream.
pub(crate) fn stream_chunk(file: &GameFile) -> Chunk {
    let stamp = Stamp::of(&file.path).unwrap_or_default();
    let mut chunk = Chunk {
        rel_path: file.rel_path.clone(),
        stamp,
        ..Chunk::default()
    };
    let stream = match StreamFile::open(&file.path) {
        Ok(stream) => stream,
        Err(error) => {
            chunk.error = Some(error.to_string());
            return chunk;
        }
    };
    let layout = stream.layout();
    chunk.records = (0..layout.resources.len())
        .into_par_iter()
        .map(|i| resource_record(&stream, i))
        .collect();
    chunk
}

/// The record of resource `index` of `stream`.
pub(crate) fn resource_record(stream: &StreamFile, index: usize) -> Record {
    let layout = stream.layout();
    let header = layout.resources[index].header;
    let mut record = Record {
        resource: index as u32,
        header,
        stored_size: layout.stored_size(index) as u32,
        compressed: layout.is_compressed(index),
        ..Record::default()
    };
    if SKIPPED.contains(&header.type_id) {
        return record;
    }
    let data = match stream.resource_data(index) {
        Ok(data) => data,
        Err(error) => {
            record.error = Some(error.to_string());
            return record;
        }
    };
    if let Err(error) = harvest(stream, index, &data, &mut record) {
        record.error = Some(error.to_string());
    }
    dedup(&mut record);
    record
}

/// Removes repeated names, references, hashes and anchors, keeping the first.
fn dedup(record: &mut Record) {
    fn keep_first<T: Clone + Eq + std::hash::Hash>(list: &mut Vec<T>) {
        let mut seen = HashSet::new();
        list.retain(|item| seen.insert(item.clone()));
    }
    record.names.retain(|(_, name)| !name.is_empty());
    keep_first(&mut record.names);
    keep_first(&mut record.hashes);
    keep_first(&mut record.anchors);
    keep_first(&mut record.pairs);
    // References: one per target; the first label wins.
    let mut seen = HashSet::new();
    record.refs.retain(|r| seen.insert(r.target.clone()));
}

fn harvest(
    stream: &StreamFile,
    index: usize,
    data: &[u8],
    record: &mut Record,
) -> crate::Result<()> {
    let type_id = record.header.type_id;
    match type_id {
        TYPE_TG4H => {
            let header = tg4::Tg4Header::parse(data)?;
            record.names.push((NameKind::Name, header.name.clone()));
            record
                .names
                .push((NameKind::Package, header.package.clone()));
            record.pairs.push((header.name_hash, header.name));
            if let Some(pair) = crate::resource::texture::pair_of(stream.layout(), index) {
                let pixels = &stream.layout().resources[pair.data].header;
                record.refs.push(Ref {
                    label: "pixels".into(),
                    target: Target::Resource {
                        type_id: TYPE_TG4D,
                        id: pixels.ids[3],
                    },
                });
            }
        }
        TYPE_MATERIAL => {
            let m = material::Material::parse(data)?;
            record.names.push((NameKind::Name, m.name.clone()));
            record.anchors.push(Anchor::Material(m.name_hash));
            record.pairs.push((m.name_hash, m.name.clone()));
            for (hash, _, _) in m.shaders {
                if hash != 0 {
                    record.hashes.push((HashKind::Shader, hash));
                }
            }
            for param in &m.params {
                if !param.source.is_empty() {
                    record.names.push((NameKind::Text, param.source.clone()));
                }
                if let material::ParamValue::Texture(hash) = param.value {
                    record.refs.push(Ref {
                        label: param.name.clone(),
                        target: Target::Resource {
                            type_id: TYPE_TG4H,
                            id: hash,
                        },
                    });
                }
            }
        }
        TYPE_MESH => {
            let model = mesh::Model::parse(data, &[])?;
            record.names.push((NameKind::Name, model.name.clone()));
            for r in &model.records {
                record.names.push((NameKind::Part, r.name.clone()));
                record.refs.push(Ref {
                    label: r.name.clone(),
                    target: Target::Material(r.material_hash),
                });
            }
            if !model.bones.is_empty() {
                record
                    .hashes
                    .extend(model.bones.iter().map(|b| (HashKind::Bone, b.0)));
                record.refs.push(Ref {
                    label: "skeleton".into(),
                    target: Target::Bones(model.bones.clone().into_boxed_slice()),
                });
            }
            if let Some(v) = mesh::volatile_for(stream.layout(), index) {
                record.refs.push(Ref {
                    label: "vertex data".into(),
                    target: Target::Resource {
                        type_id: TYPE_MESH_VOLATILE,
                        id: stream.layout().resources[v].header.ids[3],
                    },
                });
            }
        }
        TYPE_METAMODEL => metamodel_record(data, record)?,
        TYPE_SIMGROUP => simgroup_record(data, record)?,
        TYPE_GRAPH => {
            let g = graph::Graph::parse(data)?;
            record.anchors.push(Anchor::Guid(g.guid.0));
        }
        TYPE_VARIABLE_DICTIONARY => {
            let dict = variabledictionary::VariableDictionary::parse(data)?;
            for v in dict.variables {
                record.names.push((NameKind::Variable, v.name.clone()));
                record.pairs.push((v.hash.0, v.name));
            }
        }
        TYPE_PARTSDB => {
            let db = partsdb::PartsDb::parse(data)?;
            record.names.push((NameKind::Name, db.name));
            // Parts name their meshes by resource id; preset items and part
            // choices name materials.
            for part in &db.parts {
                for id in part.mesh_ids() {
                    record.refs.push(Ref {
                        label: part.name.clone(),
                        target: Target::Resource {
                            type_id: TYPE_MESH,
                            id,
                        },
                    });
                }
            }
            for preset in &db.presets {
                for item in &preset.items {
                    for swap in &item.swaps {
                        for hash in [swap.to.0, swap.damaged.0] {
                            if hash != 0 {
                                record.refs.push(Ref {
                                    label: format!("{}/{}", preset.name, item.name),
                                    target: Target::Material(hash),
                                });
                            }
                        }
                    }
                }
            }
            for set in &db.sets {
                for m in &set.materials {
                    for c in &m.materials {
                        for to in &c.candidates {
                            record.refs.push(Ref {
                                label: format!("{}/{}", set.name, db.parts[m.part].name),
                                target: Target::Material(to.0),
                            });
                        }
                    }
                }
            }
            for part in db.parts {
                record.names.push((NameKind::Part, part.name));
            }
            let pieces = db.variants.into_iter().map(|v| v.name);
            let pieces = pieces
                .chain(db.slots)
                .chain(db.sets.into_iter().map(|s| s.name));
            let pieces = pieces.chain(db.groups.into_iter().map(|g| g.name));
            record.names.extend(pieces.map(|n| (NameKind::Part, n)));
        }
        rcb::TYPE_RCB => {
            let rig = rcb::Rig::parse(data)?;
            for skeleton in rig.skeletons {
                record.names.push((NameKind::Name, skeleton.name.clone()));
                if skeleton.name_hash != 0 && !skeleton.name.is_empty() {
                    record.pairs.push((skeleton.name_hash, skeleton.name));
                }
                let bones: Vec<u32> = skeleton.bones.iter().map(|b| b.hash).collect();
                record
                    .hashes
                    .extend(bones.iter().map(|&h| (HashKind::Bone, h)));
                record
                    .anchors
                    .push(Anchor::Skeleton(bones.into_boxed_slice()));
            }
        }
        ffn::TYPE_FFN => {
            let font = ffn::Font::parse(data)?;
            record.refs.push(Ref {
                label: "atlas".into(),
                target: Target::Named {
                    type_id: TYPE_TG4H,
                    name: font.name.clone(),
                },
            });
            record.names.push((NameKind::Name, font.name));
        }
        TYPE_APX => {
            let file = apx::AptFile::parse(data)?;
            if let Some(title) = file.title() {
                record.names.push((NameKind::Name, title));
            }
            for texture in file.textures()? {
                record.names.push((NameKind::Part, texture.header.name));
            }
            // `$name` text references: names of string table entries.
            for (_, _, name) in file.text_references() {
                if name.len() > 1 {
                    record.names.push((NameKind::Text, name));
                }
            }
        }
        lua::TYPE_LUA => {
            let chunk = lua::LuaChunk::parse(data)?;
            record
                .names
                .push((NameKind::Name, chunk.file_name().to_owned()));
            record.names.push((NameKind::Text, chunk.source.clone()));
            for s in chunk.strings.into_iter().take(MAX_LOOSE_STRINGS * 4) {
                if is_word(&s) {
                    record.names.push((NameKind::Text, s));
                }
            }
        }
        anim::TYPE_BNK => {
            let bank = anim::AnimBank::parse(data)?;
            let mut names = bank.names;
            for clip in &bank.clips {
                names.extend(clip.events.iter().map(|e| e.name.clone()));
            }
            names.sort();
            names.dedup();
            record
                .names
                .extend(names.into_iter().map(|n| (NameKind::Event, n)));
        }
        TYPE_LH2 => {
            let table = lh2::StringTable::parse(data)?;
            record.languages = if table.languages.is_empty() {
                (0..table.strings.len())
                    .map(|i| {
                        if i == 0 {
                            "text".into()
                        } else {
                            format!("text{i}")
                        }
                    })
                    .collect()
            } else {
                table.languages.clone()
            };
            for (language, strings) in table.strings.iter().enumerate() {
                for (entry, text) in strings.iter().enumerate() {
                    record.texts.push(TextEntry {
                        hash: table.hashes[entry],
                        language: language as u16,
                        text: text.clone(),
                    });
                }
            }
            record
                .hashes
                .extend(table.hashes.iter().map(|&h| (HashKind::StringEntry, h)));
        }
        material_swap::TYPE_MATERIAL_SWAP_LIST => {
            // The ids are the GUID of the SimGroup object whose model the
            // swaps apply to; the object does not refer to the list.
            let list = material_swap::MaterialSwapList::parse(data)?;
            for swap in list.lists.iter().flatten() {
                record.refs.push(Ref {
                    label: "swap from".into(),
                    target: Target::Material(swap.from),
                });
                record.refs.push(Ref {
                    label: "swap to".into(),
                    target: Target::Material(swap.to),
                });
            }
        }
        _ => {
            if type_id == treemodel::TYPE_TREE {
                let tree = treemodel::TreeModel::parse(data)?;
                for part in &tree.parts {
                    record.refs.push(Ref {
                        label: tree.part_name(part).to_owned(),
                        target: Target::Material(part.material_hash),
                    });
                }
            }
            let strings = loose_strings(data);
            if type_id == TYPE_VFX
                && let Some(first) = strings.first()
                && data.starts_with(first.as_bytes())
            {
                record.names.push((NameKind::Name, first.clone()));
            }
            record
                .names
                .extend(strings.into_iter().map(|s| (NameKind::Text, s)));
        }
    }
    Ok(())
}

/// A known SimGroup attribute that references a resource of a known type.
const TYPED_ATTRIBUTES: [(u32, u16, u32); 1] = [(
    simgroup::CLASS_ENTITY,
    simgroup::ATTR_SET_MODEL,
    TYPE_METAMODEL,
)];

fn simgroup_record(data: &[u8], record: &mut Record) -> crate::Result<()> {
    use crate::resource::names::sim_classes;
    use simgroup::Value;

    let group = simgroup::SimGroup::parse(data)?;
    let own: HashSet<[u8; 16]> = group.objects.iter().map(|o| o.guid.0).collect();
    let class_label = |class: u32| match sim_classes().get(class) {
        Some(name) => name.to_owned(),
        None => format!("{class:08x}"),
    };
    for object in &group.objects {
        record.anchors.push(Anchor::Guid(object.guid.0));
        record.hashes.push((HashKind::SimClass, object.class));
        if let Some(name) = object.class_name() {
            record.names.push((NameKind::Class, name.to_owned()));
        }
        if let Some(name) = group.object_name(object) {
            record.names.push((NameKind::Object, name.to_owned()));
        }
        for link in &object.links {
            if let Some(id) = link.resource_id() {
                record.refs.push(Ref {
                    label: format!("{} link", class_label(object.class)),
                    target: Target::Resource { type_id: 0, id },
                });
            }
        }
    }
    for packet in &group.packets {
        record.hashes.push((HashKind::SimClass, packet.class.0));
        let class = class_label(packet.class.0);
        for attr in &packet.attrs {
            let label = || match attr.name {
                Some(name) => format!("{class}.{name}"),
                None => format!("{class}.{}", attr.index),
            };
            match &attr.value {
                Value::String(text) if attr.name != Some("CMD_SetName") => {
                    record.names.push((NameKind::Text, text.clone()));
                }
                Value::Guid(_) => {
                    if let Some(id) = attr.resource.map(|h| h.0) {
                        let type_id = TYPED_ATTRIBUTES
                            .iter()
                            .find(|t| t.0 == packet.class.0 && t.1 == attr.index)
                            .map_or(0, |t| t.2);
                        record.refs.push(Ref {
                            label: label(),
                            target: Target::Resource { type_id, id },
                        });
                    } else if let Some(guid) = super::placement::guid_reference(attr, &own) {
                        record.refs.push(Ref {
                            label: label(),
                            target: Target::Guid(guid),
                        });
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn metamodel_record(data: &[u8], record: &mut Record) -> crate::Result<()> {
    use metamodel::PropValue;

    let model = metamodel::MetaModel::parse(data)?;
    let mut labels = vec![None; model.resources.len()];
    for property in &model.properties {
        record.pairs.push((property.hash.0, property.name.clone()));
        record.hashes.push((HashKind::Property, property.hash.0));
        match &property.value {
            PropValue::Resource(i) => {
                if let Some(slot) = labels.get_mut(*i)
                    && slot.is_none()
                {
                    *slot = Some(format!("{}.{}", property.class, property.name));
                }
            }
            PropValue::String(text) => record.names.push((NameKind::Text, text.clone())),
            _ => {}
        }
    }
    // By GUID, as the game resolves them (the path hash `id` differs for
    // nested metamodels split by level of detail).
    for (r, label) in model.resources.iter().zip(labels) {
        record.refs.push(Ref {
            label: label.unwrap_or_else(|| "resource".into()),
            target: Target::Resource {
                type_id: r.type_id.0,
                id: r.target_id(),
            },
        });
    }
    for state in &model.states {
        record.names.push((NameKind::Part, state.name.clone()));
        record
            .names
            .push((NameKind::Class, state.type_name.clone()));
        record
            .pairs
            .push((state.type_hash, state.type_name.clone()));
    }
    for part in model.parts.iter().chain(&model.sub_objects) {
        record.names.push((NameKind::Part, part.name.clone()));
        record.names.push((NameKind::Class, part.type_name.clone()));
        record.pairs.push((part.type_hash, part.type_name.clone()));
    }
    for variable in &model.variables {
        record
            .names
            .push((NameKind::Variable, variable.name.clone()));
        record.pairs.push((variable.hash.0, variable.name.clone()));
    }
    Ok(())
}

/// NUL-terminated runs of at least four printable ASCII characters with at
/// least two letters, in order of appearance, without repeats.
pub(crate) fn loose_strings(data: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut start = 0;
    for (i, &b) in data.iter().enumerate() {
        if (0x20..0x7F).contains(&b) {
            continue;
        }
        if b == 0 && i - start >= 4 {
            let text = &data[start..i];
            if text.iter().filter(|c| c.is_ascii_alphabetic()).count() >= 2 {
                let text = String::from_utf8_lossy(text).into_owned();
                if seen.insert(text.clone()) {
                    out.push(text);
                    if out.len() == MAX_LOOSE_STRINGS {
                        break;
                    }
                }
            }
        }
        start = i + 1;
    }
    out
}

/// Whether a script string looks like a name rather than code or punctuation.
fn is_word(s: &str) -> bool {
    s.len() >= 3 && s.chars().filter(char::is_ascii_alphabetic).count() >= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loose_strings_need_terminator_letters_and_length() {
        let data = b"abc\0fire_01\0\x01xyzw\0\x7f12345\0fire_01\0ab12\0no end";
        assert_eq!(loose_strings(data), ["fire_01", "xyzw", "ab12"]);
    }

    #[test]
    fn script_words() {
        assert!(is_word("play_music"));
        assert!(!is_word("a"));
        assert!(!is_word("1.5"));
    }
}
