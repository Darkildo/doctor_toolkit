//! New resources and streams: identifiers, cloning with rewritten
//! references, and reference checks.
//!
//! # How the game identifies resources
//!
//! The four ids of a resource header are one 128-bit GUID (`godfather2.exe`,
//! `0x00421470`). When a resource is loaded, the engine looks its GUID up in a
//! global table of loaded resources (`0x00421170`); if it is there, the data
//! is skipped and the loaded resource gains a reference. Otherwise the data
//! is loaded and registered under its GUID (`0x00421550`). The type is not
//! part of the key, and a GUID of zeros (simgroup and other one-per-stream
//! types) is never registered.
//!
//! So a resource that appears in several streams is loaded once, and two
//! different resources must never share a GUID while both can be loaded.
//! The game builds GUIDs in four forms ([`IdForm`]):
//!
//! - a 32-bit id repeated four times (most types); references from metamodel
//!   and SimGroup data are exactly these 16 bytes;
//! - `0, type id, package hash, name hash` for `tg4h` and `tg4d`;
//! - the bitwise complement of the mesh id for `meshvolatile`;
//! - a real GUID, 8 random bytes and the author's login, for `graph` and
//!   `materialswaplistresource`.
//!
//! Some references do not use the GUID: draw records of a mesh name their
//! material by the CRC-32 stored in the material data, and materials name
//! textures by the name hash of the texture (its last id).

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::container::ustream::{ResourceHeader, Storage, StreamLayout};
use crate::game::{GameInstance, ResourceIndex, StreamFile};
use crate::hash::{fnv1a, sdbm};
use crate::io::{self, Endian};
use crate::resource::neoguid::{self, MachineName};
use crate::resource::simgroup::Guid;
use crate::resource::texture::{TYPE_TG4D, TYPE_TG4H};
use crate::resource::tg4;
use crate::{Error, Result};

pub const TYPE_MESH: u32 = sdbm(b"mesh");
pub const TYPE_MESHVOLATILE: u32 = sdbm(b"meshvolatile");
pub const TYPE_MATERIAL: u32 = sdbm(b"material");
pub const TYPE_METAMODEL: u32 = sdbm(b"metamodel");
pub const TYPE_SIMGROUP: u32 = sdbm(b"simgroup");
pub const TYPE_GRAPH: u32 = sdbm(b"graph");

/// Memory policy and load alignment of a resource type. In the game files
/// both depend only on the type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeDefaults {
    pub mem_policy: u32,
    pub alignment: u32,
}

const POLICY_DEFAULT: u32 = sdbm(b"default");

/// Memory policy and alignment of every resource of a type in The Godfather II
/// (PC), checked on all 212 544 resources. Types not listed use `default`, 0x10.
const TYPE_DEFAULTS: &[(&[u8], u32, u32)] = &[
    (b"mesh", sdbm(b"geometry"), 0x80),
    (b"meshvolatile", 0x67A6_002E, 0x80),
    (b"material", sdbm(b"volatile"), 0x10),
    (b"vfx", POLICY_DEFAULT, 0x20),
    (b"sbk", 0x7A10_372B, 0x1000),
    (b"auc", 0x58B1_F4D8, 0x10),
    (b"cec", POLICY_DEFAULT, 0x40),
    (b"dlg", POLICY_DEFAULT, 0x40),
    (b"grt", POLICY_DEFAULT, 0x40),
];

/// Memory policy and alignment for new resources of `type_id`.
pub fn type_defaults(type_id: u32) -> TypeDefaults {
    TYPE_DEFAULTS
        .iter()
        .find(|(tag, _, _)| sdbm(tag) == type_id)
        .map_or(
            TypeDefaults {
                mem_policy: POLICY_DEFAULT,
                alignment: 0x10,
            },
            |&(_, mem_policy, alignment)| TypeDefaults {
                mem_policy,
                alignment,
            },
        )
}

/// How the four ids of a header form the resource GUID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdForm {
    /// All zero: not registered, not referenced (simgroup, lighttoc, ...).
    Zero,
    /// One non-zero id repeated four times.
    Repeated,
    /// `0, type id, package hash, name hash` (tg4h, tg4d).
    Texture,
    /// Any other 128 bits: a real GUID (graph, materialswaplistresource).
    Guid,
}

impl IdForm {
    pub fn of(header: &ResourceHeader) -> Self {
        let ids = header.ids;
        if ids == [0; 4] {
            Self::Zero
        } else if ids.iter().all(|&i| i == ids[0]) {
            Self::Repeated
        } else if ids[0] == 0 && ids[1] == header.type_id {
            Self::Texture
        } else {
            Self::Guid
        }
    }

    /// The form new resources of a type get.
    pub fn for_type(type_id: u32) -> Self {
        match type_id {
            TYPE_TG4H | TYPE_TG4D => Self::Texture,
            TYPE_GRAPH => Self::Guid,
            t if t == sdbm(b"materialswaplistresource") => Self::Guid,
            _ => Self::Repeated,
        }
    }
}

/// The GUID of a header as 16 bytes, as references store it.
pub fn guid_bytes(ids: &[u32; 4]) -> [u8; 16] {
    let mut out = [0; 16];
    for (chunk, id) in out.chunks_exact_mut(4).zip(ids) {
        chunk.copy_from_slice(&id.to_le_bytes());
    }
    out
}

/// Identifiers in use, so that new ones do not collide.
///
/// A new 32-bit id is refused when any resource uses it as any of its four
/// ids, or when its complement is used (`meshvolatile` takes the complement
/// of its mesh id).
#[derive(Debug, Clone, Default)]
pub struct UsedIds {
    guids: HashSet<[u32; 4]>,
    words: HashSet<u32>,
}

impl UsedIds {
    /// The identifiers of every resource of the game.
    pub fn from_index(index: &ResourceIndex) -> Self {
        let mut used = Self::default();
        for entry in &index.entries {
            used.insert(&entry.header.ids);
        }
        used
    }

    /// Marks a GUID as used.
    pub fn insert(&mut self, ids: &[u32; 4]) {
        if *ids == [0; 4] {
            return;
        }
        self.guids.insert(*ids);
        for &word in ids {
            self.words.insert(word);
        }
    }

    /// Marks a single id (e.g. a material name hash) as used.
    pub fn insert_id(&mut self, id: u32) {
        self.words.insert(id);
    }

    pub fn contains_guid(&self, ids: &[u32; 4]) -> bool {
        self.guids.contains(ids)
    }

    /// Whether `id` can be given to a new resource.
    pub fn is_free(&self, id: u32) -> bool {
        id != 0 && id != u32::MAX && !self.words.contains(&id) && !self.words.contains(&!id)
    }

    /// A free id derived from `seed`: FNV-1a of the seed, rehashed with a
    /// counter until it is free. The same seed gives the same id for the same
    /// game, so recorded operations are reproducible. The id is marked used.
    pub fn allocate(&mut self, seed: &str) -> u32 {
        let mut id = fnv1a(seed.as_bytes());
        let mut n = 0u32;
        while !self.is_free(id) {
            n += 1;
            id = fnv1a(format!("{seed}#{n}").as_bytes());
        }
        self.words.insert(id);
        self.words.insert(!id);
        id
    }

    /// New ids of `form` for a resource of `type_id`, derived from `seed`.
    /// `template` supplies the package hash of textures.
    pub fn new_ids(
        &mut self,
        form: IdForm,
        type_id: u32,
        template: &[u32; 4],
        seed: &str,
    ) -> [u32; 4] {
        let ids = match form {
            IdForm::Zero => [0; 4],
            IdForm::Repeated => [self.allocate(seed); 4],
            IdForm::Texture => [0, type_id, template[2], self.allocate(seed)],
            IdForm::Guid => self.allocate_guid(seed),
        };
        self.insert(&ids);
        ids
    }

    /// A new GUID for a resource whose ids are a NeoGuid (`graph`,
    /// `materialswaplistresource`): canonical, of the tool's machine name,
    /// with keys derived from `seed` ([`neoguid::guid_for_name`]) and the two
    /// key words free. The words are marked used.
    pub fn allocate_guid(&mut self, seed: &str) -> [u32; 4] {
        let ids = (0u32..)
            .map(|n| guid_words(&neoguid::guid_for_name(&MachineName::TOOLKIT, seed, n)))
            .find(|ids| self.is_free(ids[0]) && self.is_free(ids[1]) && !self.guids.contains(ids))
            .expect("2^32 attempts");
        for word in [ids[0], ids[1]] {
            self.words.insert(word);
            self.words.insert(!word);
        }
        ids
    }
}

/// The GUID as four little-endian words, as resource headers store it.
pub fn guid_words(guid: &Guid) -> [u32; 4] {
    std::array::from_fn(|k| {
        u32::from_le_bytes(guid.0[4 * k..4 * k + 4].try_into().expect("4 bytes"))
    })
}

/// A resource to be added to a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewResource {
    pub header: ResourceHeader,
    pub data: Vec<u8>,
    pub storage: Storage,
    /// Index of the resource it was cloned from, in the source stream.
    pub source: Option<usize>,
}

impl NewResource {
    /// A new resource of `type_id` with the type's memory policy and
    /// alignment. `data_size` is set from `data`.
    pub fn new(type_id: u32, ids: [u32; 4], data: Vec<u8>, storage: Storage) -> Result<Self> {
        let defaults = type_defaults(type_id);
        let data_size = u32::try_from(data.len())
            .map_err(|_| Error::InvalidArgument("resource data larger than 4 GiB".into()))?;
        Ok(Self {
            header: ResourceHeader {
                mem_policy: defaults.mem_policy,
                alignment: defaults.alignment,
                type_id,
                ids,
                data_size,
            },
            data,
            storage,
            source: None,
        })
    }
}

/// Old → new identifiers of cloned resources.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Remap {
    /// Resource GUIDs (header ids), as referenced by metamodels and SimGroups.
    pub guids: HashMap<[u32; 4], [u32; 4]>,
    /// Name hashes: texture name hashes (last id of tg4h/tg4d) and material
    /// name hashes (CRC-32 in the material data).
    pub names: HashMap<u32, u32>,
}

impl Remap {
    pub fn is_empty(&self) -> bool {
        self.guids.is_empty() && self.names.is_empty()
    }

    /// Adds the pairs of `other`.
    pub fn extend(&mut self, other: &Remap) {
        self.guids.extend(other.guids.iter().map(|(a, b)| (*a, *b)));
        self.names.extend(other.names.iter().map(|(a, b)| (*a, *b)));
    }
}

/// Options for [`clone_resources`].
#[derive(Debug, Clone, Default)]
pub struct CloneOptions {
    /// Text the new ids are derived from; the same seed gives the same ids.
    pub seed: String,
    /// Rewrite references between the cloned resources: a cloned mesh uses
    /// the cloned materials, a cloned material the cloned textures, a cloned
    /// metamodel the cloned meshes and metamodels.
    pub rewrite_references: bool,
    /// Storage of the clones; `None` keeps the storage of each source.
    pub storage: Option<Storage>,
}

/// Resources that belong together and are always cloned together:
/// a texture header and its data, a mesh and its `meshvolatile`.
pub fn companions(layout: &StreamLayout, index: usize) -> Vec<usize> {
    let Some(resource) = layout.resources.get(index) else {
        return Vec::new();
    };
    let header = resource.header;
    match header.type_id {
        TYPE_TG4H | TYPE_TG4D => crate::resource::texture::pair_of(layout, index)
            .map_or_else(|| vec![index], |p| vec![p.header, p.data]),
        TYPE_MESH => {
            let next = layout.resources.get(index + 1);
            match next {
                Some(n)
                    if n.header.type_id == TYPE_MESHVOLATILE
                        && n.header.ids == header.ids.map(|i| !i) =>
                {
                    vec![index, index + 1]
                }
                _ => vec![index],
            }
        }
        TYPE_MESHVOLATILE if index > 0 => {
            let prev = &layout.resources[index - 1].header;
            if prev.type_id == TYPE_MESH && prev.ids == header.ids.map(|i| !i) {
                vec![index - 1, index]
            } else {
                vec![index]
            }
        }
        _ => vec![index],
    }
}

/// Clones resources of a stream under new ids.
///
/// Companions are cloned with them (see [`companions`]). Every clone gets a
/// new GUID of its source's form; textures and materials also get a new name
/// hash, written into their data, and metamodels their new id in their own
/// header. Returns the clones in stream order and the old → new identifiers.
///
/// The clones are not referenced by anything yet: point existing data at
/// them with [`rewrite_references`].
pub fn clone_resources(
    stream: &StreamFile,
    indices: &[usize],
    used: &mut UsedIds,
    options: &CloneOptions,
) -> Result<(Vec<NewResource>, Remap)> {
    let layout = stream.layout();
    let mut set: Vec<usize> = Vec::new();
    for &index in indices {
        if index >= layout.resources.len() {
            return Err(Error::InvalidArgument(format!(
                "resource index {index} out of range"
            )));
        }
        for i in companions(layout, index) {
            if !set.contains(&i) {
                set.push(i);
            }
        }
    }
    set.sort_unstable();

    // New identifiers first, so that references between clones can be rewritten.
    let mut remap = Remap::default();
    let mut new_ids: BTreeMap<usize, [u32; 4]> = BTreeMap::new();
    let mut texture_names: HashMap<u32, u32> = HashMap::new();
    for &index in &set {
        let header = layout.resources[index].header;
        let seed = format!(
            "{}:{:08x}:{:08x}.{:08x}.{:08x}.{:08x}",
            options.seed,
            header.type_id,
            header.ids[0],
            header.ids[1],
            header.ids[2],
            header.ids[3]
        );
        let ids = match (IdForm::of(&header), header.type_id) {
            (IdForm::Repeated, TYPE_MESHVOLATILE) => {
                // The complement of the mesh clone, when the mesh is cloned too.
                let mesh_ids = header.ids.map(|i| !i);
                match remap.guids.get(&mesh_ids) {
                    Some(new_mesh) => new_mesh.map(|i| !i),
                    None => used.new_ids(IdForm::Repeated, header.type_id, &header.ids, &seed),
                }
            }
            (IdForm::Texture, _) => {
                let name = *texture_names
                    .entry(header.ids[3])
                    .or_insert_with(|| used.allocate(&seed));
                let ids = [0, header.type_id, header.ids[2], name];
                used.insert(&ids);
                ids
            }
            (form, type_id) => used.new_ids(form, type_id, &header.ids, &seed),
        };
        remap.guids.insert(header.ids, ids);
        new_ids.insert(index, ids);
    }
    remap.names.extend(texture_names);
    // Materials are found by the name hash in their data.
    for &index in &set {
        let header = layout.resources[index].header;
        if header.type_id == TYPE_MATERIAL {
            let data = stream.resource_data(index)?;
            let old = io::read_u32(&data, 8, Endian::Little)?;
            let new = used.allocate(&format!("{}:material:{old:08x}", options.seed));
            remap.names.insert(old, new);
        }
    }

    let mut out = Vec::with_capacity(set.len());
    for &index in &set {
        let header = layout.resources[index].header;
        let mut data = stream.resource_data(index)?;
        let ids = new_ids[&index];
        rewrite_identity(header.type_id, &header.ids, &ids, &remap, &mut data)?;
        if options.rewrite_references {
            rewrite_references(header.type_id, &mut data, &remap)?;
        }
        let storage = options.storage.unwrap_or(if layout.is_compressed(index) {
            Storage::RefPack
        } else {
            Storage::Raw
        });
        out.push(NewResource {
            header: ResourceHeader {
                ids,
                data_size: data.len() as u32,
                ..header
            },
            data,
            storage,
            source: Some(index),
        });
    }
    Ok((out, remap))
}

/// Writes the new identity of a clone into its data: the name hash of a
/// texture or material, the own id of a metamodel, the GUID of a graph.
fn rewrite_identity(
    type_id: u32,
    old_ids: &[u32; 4],
    new_ids: &[u32; 4],
    remap: &Remap,
    data: &mut [u8],
) -> Result<()> {
    match type_id {
        TYPE_TG4H => {
            let mut set = tg4::parse_set(data)?;
            for texture in &mut set {
                if let Some(&new) = remap.names.get(&texture.name_hash) {
                    texture.name_hash = new;
                }
            }
            let bytes = tg4::set_to_bytes(&set)?;
            if bytes.len() != data.len() {
                return Err(Error::InvalidArgument(
                    "texture header changed size when rewritten".into(),
                ));
            }
            data.copy_from_slice(&bytes);
        }
        TYPE_MATERIAL => {
            let old = io::read_u32(data, 8, Endian::Little)?;
            if let Some(&new) = remap.names.get(&old) {
                for at in [8, 12] {
                    if read_u32(data, at)? == old {
                        write_u32(data, at, new);
                    }
                }
            }
        }
        TYPE_METAMODEL => {
            let old = old_ids[0];
            for at in [0x10, 0x14, 0x18, 0x1C, 0x24, 0x28] {
                if read_u32(data, at)? == old {
                    write_u32(data, at, new_ids[0]);
                }
            }
        }
        _ => {
            // A GUID stored at 0x10 equal to the header ids (graph).
            let old = guid_bytes(old_ids);
            if *old_ids != [0; 4] && data.get(0x10..0x20) == Some(&old[..]) {
                data[0x10..0x20].copy_from_slice(&guid_bytes(new_ids));
            }
        }
    }
    Ok(())
}

fn read_u32(data: &[u8], at: usize) -> Result<u32> {
    io::read_u32(data, at, Endian::Little)
}

fn write_u32(data: &mut [u8], at: usize, value: u32) {
    data[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

/// Rewrites the references of one resource according to `remap`. Returns
/// how many references were changed.
///
/// | Type | References |
/// |---|---|
/// | `mesh` | material name hash of every draw record |
/// | `material` | texture name hash of every texture parameter |
/// | `metamodel` | id and GUID of every resource entry (T0) |
/// | `simgroup` | every 16-byte resource GUID in the aux region (models, sounds, effects) |
///
/// Other types are left unchanged. Values are rewritten in place; the size
/// of the data never changes.
pub fn rewrite_references(type_id: u32, data: &mut [u8], remap: &Remap) -> Result<usize> {
    if remap.is_empty() {
        return Ok(0);
    }
    let mut changed = 0;
    match type_id {
        TYPE_MESH => {
            for at in mesh_material_offsets(data)? {
                if let Some(&new) = remap.names.get(&read_u32(data, at)?) {
                    write_u32(data, at, new);
                    changed += 1;
                }
            }
        }
        TYPE_MATERIAL => {
            for at in material_texture_offsets(data)? {
                if let Some(&new) = remap.names.get(&read_u32(data, at)?) {
                    write_u32(data, at, new);
                    changed += 1;
                }
            }
        }
        TYPE_METAMODEL => {
            for at in metamodel_ref_offsets(data)? {
                // u32 hash, u32 id, u32 type id, 4 × u32 ids, u32 0
                let ids = [
                    read_u32(data, at + 12)?,
                    read_u32(data, at + 16)?,
                    read_u32(data, at + 20)?,
                    read_u32(data, at + 24)?,
                ];
                if let Some(new) = remap.guids.get(&ids) {
                    let id = read_u32(data, at + 4)?;
                    if id == ids[0] {
                        write_u32(data, at + 4, new[0]);
                    }
                    for (k, word) in new.iter().enumerate() {
                        write_u32(data, at + 12 + 4 * k, *word);
                    }
                    changed += 1;
                }
            }
        }
        TYPE_SIMGROUP => {
            let (start, end) = simgroup_aux(data)?;
            let mut at = start;
            while at + 16 <= end {
                let ids = [
                    read_u32(data, at)?,
                    read_u32(data, at + 4)?,
                    read_u32(data, at + 8)?,
                    read_u32(data, at + 12)?,
                ];
                if let Some(new) = remap.guids.get(&ids) {
                    data[at..at + 16].copy_from_slice(&guid_bytes(new));
                    changed += 1;
                    at += 16;
                } else {
                    at += 4;
                }
            }
        }
        _ => {}
    }
    Ok(changed)
}

/// Offsets of the material hashes of the draw records of a mesh.
fn mesh_material_offsets(data: &[u8]) -> Result<Vec<usize>> {
    const RECORD_SIZE: usize = 0xB0;
    let count = read_u32(data, 0x34)? as usize;
    let records = read_u32(data, 0x50)? as usize;
    io::slice(data, records, count.saturating_mul(RECORD_SIZE))?;
    Ok((0..count)
        .map(|i| records + i * RECORD_SIZE + 0x0C)
        .collect())
}

/// Offsets of the values of the texture parameters of a material.
fn material_texture_offsets(data: &[u8]) -> Result<Vec<usize>> {
    let u16_at = |o: usize| io::read_u16(data, o, Endian::Little);
    let count = u16_at(0x46)? as usize;
    let table = u16_at(0x48)? as usize;
    let values = u16_at(0x4C)? as usize;
    let mut out = Vec::new();
    for i in 0..count {
        let at = table + 12 * i;
        if u16_at(at + 4)? == 5 {
            let value = values + u16_at(at + 6)? as usize;
            io::slice(data, value, 4)?;
            out.push(value);
        }
    }
    Ok(out)
}

/// Offsets of the resource entries (table T0) of a metamodel.
fn metamodel_ref_offsets(data: &[u8]) -> Result<Vec<usize>> {
    if read_u32(data, 0)? != crate::resource::metamodel::MAGIC {
        return Err(Error::BadMagic {
            what: "metamodel",
            found: read_u32(data, 0)?,
        });
    }
    let start = read_u32(data, 0x34)? as usize;
    let count = io::read_u16(data, 0x58, Endian::Little)? as usize;
    io::slice(data, start, count * 0x20)?;
    Ok((0..count).map(|i| start + i * 0x20).collect())
}

/// The aux region of a SimGroup: GUIDs, vectors and matrices.
fn simgroup_aux(data: &[u8]) -> Result<(usize, usize)> {
    const MAGIC: u32 = 0x5369_6D47;
    if read_u32(data, 8)? != MAGIC {
        return Err(Error::BadMagic {
            what: "simgroup",
            found: read_u32(data, 8)?,
        });
    }
    let objects = read_u32(data, 0x18)? as usize;
    let aux_len = read_u32(data, 0x20)? as usize;
    let table = read_u32(data, 0x28)? as usize;
    let start = table + 4 * objects;
    io::slice(data, start, aux_len)?;
    Ok((start, start + aux_len))
}

/// What a resource refers to, for [`check_references`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Target {
    /// A resource by GUID.
    Guid([u32; 4]),
    /// A material by the name hash in its data.
    Material(u32),
    /// A texture by name hash.
    Texture(u32),
}

/// The references of one resource.
pub fn references(type_id: u32, data: &[u8]) -> Result<Vec<Target>> {
    let mut out = Vec::new();
    match type_id {
        TYPE_MESH => {
            for at in mesh_material_offsets(data)? {
                out.push(Target::Material(read_u32(data, at)?));
            }
        }
        TYPE_MATERIAL => {
            for at in material_texture_offsets(data)? {
                out.push(Target::Texture(read_u32(data, at)?));
            }
        }
        TYPE_METAMODEL => {
            for r in crate::resource::metamodel::resource_refs(data)? {
                // Two referenced types are never stored in streams.
                if r.type_id.0 == sdbm(b"hka") || r.type_id.0 == sdbm(b"emx") {
                    continue;
                }
                out.push(Target::Guid(r.guid()));
            }
        }
        TYPE_SIMGROUP => {
            let group = crate::resource::simgroup::SimGroup::parse(data)?;
            for object in &group.objects {
                if let Some(id) = group.model(object) {
                    out.push(Target::Guid([id; 4]));
                }
            }
        }
        _ => {}
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// What one stream provides to references: GUIDs, material and texture names.
#[derive(Debug, Clone, Default)]
pub struct Provided {
    pub guids: HashSet<[u32; 4]>,
    pub materials: HashSet<u32>,
    pub textures: HashSet<u32>,
}

impl Provided {
    pub fn provides(&self, target: &Target) -> bool {
        match target {
            Target::Guid(g) => self.guids.contains(g),
            Target::Material(h) => self.materials.contains(h),
            Target::Texture(h) => self.textures.contains(h),
        }
    }

    /// Adds a resource.
    pub fn add(&mut self, header: &ResourceHeader, data: Option<&[u8]>) {
        if header.ids != [0; 4] {
            self.guids.insert(header.ids);
        }
        match header.type_id {
            TYPE_TG4H | TYPE_TG4D => {
                self.textures.insert(header.ids[3]);
            }
            TYPE_MATERIAL => {
                if let Some(hash) = data.and_then(|d| read_u32(d, 8).ok()) {
                    self.materials.insert(hash);
                }
            }
            _ => {}
        }
    }

    /// What a stream file provides.
    pub fn of_stream(stream: &StreamFile) -> Result<Self> {
        let mut provided = Self::default();
        for (index, resource) in stream.layout().resources.iter().enumerate() {
            let data = if resource.header.type_id == TYPE_MATERIAL {
                Some(stream.resource_data(index)?)
            } else {
                None
            };
            provided.add(&resource.header, data.as_deref());
        }
        Ok(provided)
    }
}

/// A reference that no loaded stream can satisfy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    pub stream: String,
    pub resource: usize,
    pub type_id: u32,
    pub target: Target,
}

/// Checks that the references of the resources of `stream_index` resolve in
/// the stream itself or in the streams loaded before it (its table-of-contents
/// ancestors). In the game files every metamodel resource reference does.
pub fn check_references(game: &GameInstance, stream_index: usize) -> Result<Vec<Unresolved>> {
    ReferenceChecker::new(game)?.check(stream_index)
}

/// [`check_references`] for many streams, reading each stream once.
pub struct ReferenceChecker<'a> {
    game: &'a GameInstance,
    parents: HashMap<String, Vec<String>>,
    provided: HashMap<usize, Provided>,
}

impl<'a> ReferenceChecker<'a> {
    pub fn new(game: &'a GameInstance) -> Result<Self> {
        Ok(Self {
            game,
            parents: game.stream_parents()?,
            provided: HashMap::new(),
        })
    }

    fn provided(&mut self, index: usize) -> Result<&Provided> {
        if !self.provided.contains_key(&index) {
            let provided = Provided::of_stream(&self.game.open_stream(index)?)?;
            self.provided.insert(index, provided);
        }
        Ok(&self.provided[&index])
    }

    /// Whether `target` resolves for stream `stream_index`: in the stream
    /// itself or in a stream loaded before it.
    pub fn provides(&mut self, stream_index: usize, target: &Target) -> Result<bool> {
        let name = self
            .game
            .streams()
            .get(stream_index)
            .ok_or_else(|| {
                Error::InvalidArgument(format!("stream index {stream_index} out of range"))
            })?
            .stream_name();
        let mut scope = vec![stream_index];
        for ancestor in crate::game::ancestors(&self.parents, &name) {
            if let Some(index) = self.game.find_stream(&ancestor) {
                scope.push(index);
            }
        }
        for index in scope {
            if self.provided(index)?.provides(target) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Unresolved references of stream `stream_index`.
    pub fn check(&mut self, stream_index: usize) -> Result<Vec<Unresolved>> {
        let game = self.game;
        let file = game.streams().get(stream_index).ok_or_else(|| {
            Error::InvalidArgument(format!("stream index {stream_index} out of range"))
        })?;
        let name = file.stream_name();
        let mut scope = vec![stream_index];
        for ancestor in crate::game::ancestors(&self.parents, &name) {
            if let Some(index) = game.find_stream(&ancestor) {
                scope.push(index);
            }
        }
        for &index in &scope {
            self.provided(index)?;
        }
        let provides = |target: &Target| scope.iter().any(|i| self.provided[i].provides(target));
        let stream = game.open_stream(stream_index)?;
        let mut out = Vec::new();
        for (index, resource) in stream.layout().resources.iter().enumerate() {
            let type_id = resource.header.type_id;
            if !matches!(
                type_id,
                TYPE_MESH | TYPE_MATERIAL | TYPE_METAMODEL | TYPE_SIMGROUP
            ) {
                continue;
            }
            let data = stream.resource_data(index)?;
            for target in references(type_id, &data)? {
                if !provides(&target) {
                    out.push(Unresolved {
                        stream: name.clone(),
                        resource: index,
                        type_id,
                        target,
                    });
                }
            }
        }
        Ok(out)
    }
}

/// A new stream GUID: a canonical NeoGuid of the tool's machine name with
/// keys derived from `seed` (the stream name, [`neoguid::new_guid`]), with a
/// table-of-contents hash that no stream of the game uses.
pub fn new_stream_guid(seed: &str, used_hashes: &HashSet<u32>) -> [u8; 16] {
    neoguid::new_guid(&MachineName::TOOLKIT, seed, |guid| {
        let hash = crate::toc::guid_hash(&guid.0);
        hash == 0 || used_hashes.contains(&hash)
    })
    .0
}

/// A stream for [`crate::workspace::Workspace::add_stream`] as `add-stream`
/// makes it: `name` depending on `parent`, a parented stream of it when
/// `with_parent`, listed in the tables of contents `tocs` (relative paths,
/// optionally `toc=parent` with another parent there; every table that
/// lists the parent when empty), with GUID `guid` (32 hexadecimal digits;
/// derived from the name with an unused hash when `None`).
pub fn new_stream(
    game: &GameInstance,
    ws: &crate::workspace::Workspace,
    name: &str,
    parent: &str,
    with_parent: bool,
    tocs: &[String],
    guid: Option<&str>,
) -> Result<crate::workspace::NewStream> {
    use crate::workspace::key::normalize_stream;
    let name = normalize_stream(name);
    let parent = normalize_stream(parent);
    let mut list = Vec::new();
    let mut toc_parents = BTreeMap::new();
    for spec in tocs {
        match spec.split_once('=') {
            Some((toc, p)) => {
                list.push(toc.to_owned());
                toc_parents.insert(toc.to_owned(), normalize_stream(p));
            }
            None => list.push(spec.clone()),
        }
    }
    if list.is_empty() {
        for (index, toc) in game.tocs().iter().enumerate() {
            if game.load_toc(index)?.find_file(&parent).is_some() {
                list.push(toc.rel_path.clone());
            }
        }
    }
    let guid: [u8; 16] = match guid {
        Some(text) => parse_guid(text)?,
        None => {
            let mut used = game.toc_hashes()?;
            used.extend(ws.new_streams().map(|s| s.hash));
            new_stream_guid(&name, &used)
        }
    };
    Ok(crate::workspace::NewStream {
        name,
        guid: guid.iter().map(|b| format!("{b:02x}")).collect(),
        hash: crate::toc::guid_hash(&guid),
        parent,
        with_parent,
        tocs: list,
        toc_parents,
        label: String::new(),
    })
}

/// 32 hexadecimal digits as 16 bytes.
pub fn parse_guid(text: &str) -> Result<[u8; 16]> {
    let text = text.trim();
    let bad = || Error::InvalidArgument(format!("`{text}`: a GUID is 32 hexadecimal digits"));
    if text.len() != 32 || !text.is_ascii() {
        return Err(bad());
    }
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).map_err(|_| bad())?;
    }
    Ok(out)
}

/// A resource type: a tag such as `lh2` or a hexadecimal id.
pub fn parse_type(text: &str) -> Result<u32> {
    if let Some(id) = crate::resource::names::type_tags().resolve(text) {
        return Ok(id);
    }
    u32::from_str_radix(text.trim().trim_start_matches("0x"), 16)
        .map_err(|_| Error::InvalidArgument(format!("unknown resource type `{text}`")))
}

/// Resource ids: one hexadecimal id (repeated four times) or four separated by dots.
pub fn parse_ids(text: &str) -> Result<[u32; 4]> {
    let parts: Vec<u32> = text
        .split('.')
        .map(|p| u32::from_str_radix(p.trim().trim_start_matches("0x"), 16))
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| Error::InvalidArgument(format!("bad ids `{text}`")))?;
    match parts.as_slice() {
        [id] => Ok([*id; 4]),
        [a, b, c, d] => Ok([*a, *b, *c, *d]),
        _ => Err(Error::InvalidArgument(
            "ids are one or four hexadecimal numbers separated by dots".into(),
        )),
    }
}

/// A memory policy: its name or a hexadecimal hash.
pub fn parse_mem_policy(text: &str) -> Result<u32> {
    if let Some(hash) = crate::resource::names::mem_policies().resolve(text) {
        return Ok(hash);
    }
    u32::from_str_radix(text.trim().trim_start_matches("0x"), 16)
        .map_err(|_| Error::InvalidArgument(format!("unknown memory policy `{text}`")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::ustream::{self, StreamLayout};
    use crate::resource::tg4::{PARAM_DATA_SIZE, PARAM_FORMAT, ParamValue, Tg4Header};

    fn header(type_id: u32, ids: [u32; 4]) -> ResourceHeader {
        let d = type_defaults(type_id);
        ResourceHeader {
            mem_policy: d.mem_policy,
            alignment: d.alignment,
            type_id,
            ids,
            data_size: 0,
        }
    }

    fn texture_header(name_hash: u32) -> Vec<u8> {
        let mut tex = Tg4Header {
            version: 3,
            budget_hash: 0,
            package: "pkg".into(),
            name_hash,
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
        tex.to_bytes().unwrap()
    }

    /// A material with one texture parameter.
    fn material(name_hash: u32, texture: u32) -> Vec<u8> {
        let mut d = vec![0u8; 0x80];
        d[0..4].copy_from_slice(&0x0AA0_DEEDu32.to_le_bytes());
        d[4..8].copy_from_slice(&19u32.to_le_bytes());
        d[8..12].copy_from_slice(&name_hash.to_le_bytes());
        d[12..16].copy_from_slice(&name_hash.to_le_bytes());
        d[0x46..0x48].copy_from_slice(&1u16.to_le_bytes());
        d[0x48..0x4A].copy_from_slice(&0x60u16.to_le_bytes());
        d[0x4C..0x4E].copy_from_slice(&0x70u16.to_le_bytes());
        d[0x50..0x52].copy_from_slice(&0x78u16.to_le_bytes());
        d[0x64..0x66].copy_from_slice(&5u16.to_le_bytes());
        d[0x70..0x74].copy_from_slice(&texture.to_le_bytes());
        d
    }

    /// A mesh with one draw record using `material`.
    fn mesh(material: u32) -> Vec<u8> {
        let mut d = vec![0u8; 0x100 + 0xB0];
        d[0x34..0x38].copy_from_slice(&1u32.to_le_bytes());
        d[0x50..0x54].copy_from_slice(&0x100u32.to_le_bytes());
        d[0x10C..0x110].copy_from_slice(&material.to_le_bytes());
        d
    }

    /// A metamodel referencing the mesh with id `mesh_id`.
    fn metamodel(id: u32, mesh_id: u32) -> Vec<u8> {
        let mut d = vec![0u8; 0x88];
        let put =
            |d: &mut Vec<u8>, at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut d, 0, crate::resource::metamodel::MAGIC);
        put(&mut d, 4, 14);
        for at in [0x10, 0x14, 0x18, 0x1C, 0x24] {
            put(&mut d, at, id);
        }
        put(&mut d, 0x34, 0x68);
        d[0x58..0x5A].copy_from_slice(&1u16.to_le_bytes());
        put(&mut d, 0x68, 0xABCD);
        put(&mut d, 0x6C, mesh_id);
        put(&mut d, 0x70, TYPE_MESH);
        for k in 0..4 {
            put(&mut d, 0x74 + 4 * k, mesh_id);
        }
        d
    }

    fn sample_stream() -> (tempfile::TempDir, StreamFile) {
        let tex = 0x55;
        let resources: Vec<(ResourceHeader, Vec<u8>)> = vec![
            (
                header(TYPE_TG4H, [0, TYPE_TG4H, 7, tex]),
                texture_header(tex),
            ),
            (header(TYPE_TG4D, [0, TYPE_TG4D, 7, tex]), vec![0; 8]),
            (header(TYPE_MATERIAL, [0x66; 4]), material(0x77, tex)),
            (header(TYPE_MESH, [3; 4]), mesh(0x77)),
            (header(TYPE_MESHVOLATILE, [!3; 4]), vec![1; 16]),
            (header(TYPE_METAMODEL, [9; 4]), metamodel(9, 3)),
        ];
        let borrowed: Vec<_> = resources
            .iter()
            .map(|(h, d)| (*h, d.as_slice(), Storage::Raw))
            .collect();
        let bytes = ustream::build(&borrowed).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.str");
        std::fs::write(&path, bytes).unwrap();
        let file = StreamFile::open(&path).unwrap();
        (dir, file)
    }

    #[test]
    fn defaults_and_forms() {
        assert_eq!(type_defaults(TYPE_MESH).alignment, 0x80);
        assert_eq!(type_defaults(TYPE_MESH).mem_policy, sdbm(b"geometry"));
        assert_eq!(
            type_defaults(sdbm(b"simgroup")).mem_policy,
            sdbm(b"default")
        );
        let h = |t, ids| header(t, ids);
        assert_eq!(IdForm::of(&h(TYPE_SIMGROUP, [0; 4])), IdForm::Zero);
        assert_eq!(IdForm::of(&h(TYPE_MESH, [5; 4])), IdForm::Repeated);
        assert_eq!(
            IdForm::of(&h(TYPE_TG4D, [0, TYPE_TG4D, 1, 2])),
            IdForm::Texture
        );
        assert_eq!(IdForm::of(&h(TYPE_GRAPH, [1, 2, 3, 4])), IdForm::Guid);
        assert_eq!(IdForm::for_type(TYPE_TG4H), IdForm::Texture);
        assert_eq!(guid_bytes(&[1, 0, 0, 0])[0], 1);
    }

    #[test]
    fn allocated_ids_are_free_and_reproducible() {
        let mut used = UsedIds::default();
        let first = used.allocate("seed");
        assert_eq!(first, fnv1a(b"seed"));
        // The same seed again gives another id, since the first is now used.
        let second = used.allocate("seed");
        assert_ne!(first, second);
        assert!(!used.is_free(first) && !used.is_free(!first));
        let mut fresh = UsedIds::default();
        fresh.insert(&[fnv1a(b"seed"); 4]);
        assert_eq!(fresh.allocate("seed"), second);
        let guid = fresh.new_ids(IdForm::Guid, TYPE_GRAPH, &[0; 4], "g");
        let canonical = neoguid::guid_for_name(&MachineName::TOOLKIT, "g", 0);
        assert_eq!(guid_bytes(&guid), canonical.0);
        assert!(fresh.contains_guid(&guid));
        assert!(!fresh.is_free(guid[0]) && !fresh.is_free(guid[1]));
        // The same seed again steps to the next canonical GUID.
        let next = fresh.new_ids(IdForm::Guid, TYPE_GRAPH, &[0; 4], "g");
        assert_eq!(
            guid_bytes(&next),
            neoguid::guid_for_name(&MachineName::TOOLKIT, "g", 1).0
        );
    }

    #[test]
    fn cloning_a_model_rewrites_its_references() {
        let (_dir, stream) = sample_stream();
        let mut used = UsedIds::default();
        for r in &stream.layout().resources {
            used.insert(&r.header.ids);
        }
        used.insert_id(0x77);
        let options = CloneOptions {
            seed: "test".into(),
            rewrite_references: true,
            storage: None,
        };
        // The metamodel, the mesh (with its meshvolatile), the material and the texture data.
        let (clones, remap) = clone_resources(&stream, &[5, 3, 2, 1], &mut used, &options).unwrap();
        let types: Vec<u32> = clones.iter().map(|c| c.header.type_id).collect();
        assert_eq!(
            types,
            [
                TYPE_TG4H,
                TYPE_TG4D,
                TYPE_MATERIAL,
                TYPE_MESH,
                TYPE_MESHVOLATILE,
                TYPE_METAMODEL
            ]
        );
        let ids: Vec<[u32; 4]> = clones.iter().map(|c| c.header.ids).collect();
        let new_tex = ids[0][3];
        assert_eq!(ids[0], [0, TYPE_TG4H, 7, new_tex]);
        assert_eq!(ids[1], [0, TYPE_TG4D, 7, new_tex]);
        assert_eq!(
            ids[4],
            ids[3].map(|i| !i),
            "meshvolatile is the complement of the mesh"
        );
        for id in &ids {
            assert!(
                stream
                    .layout()
                    .resources
                    .iter()
                    .all(|r| r.header.ids != *id)
            );
        }
        // Texture name hash in the header.
        assert_eq!(
            tg4::parse_set(&clones[0].data).unwrap()[0].name_hash,
            new_tex
        );
        // Material: new name hash, uses the cloned texture.
        let new_material = remap.names[&0x77];
        assert_eq!(read_u32(&clones[2].data, 8).unwrap(), new_material);
        assert_eq!(read_u32(&clones[2].data, 12).unwrap(), new_material);
        assert_eq!(read_u32(&clones[2].data, 0x70).unwrap(), new_tex);
        // Mesh uses the cloned material; the metamodel the cloned mesh and its own new id.
        assert_eq!(read_u32(&clones[3].data, 0x10C).unwrap(), new_material);
        assert_eq!(
            references(TYPE_METAMODEL, &clones[5].data).unwrap(),
            [Target::Guid(ids[3])]
        );
        assert_eq!(read_u32(&clones[5].data, 0x6C).unwrap(), ids[3][0]);
        assert_eq!(read_u32(&clones[5].data, 0x24).unwrap(), ids[5][0]);
        assert_eq!(clones[5].header.data_size as usize, clones[5].data.len());

        // The clones go into the stream and every reference resolves.
        let insertions: Vec<_> = clones
            .iter()
            .map(|c| ustream::Insertion {
                before: stream.layout().resources.len(),
                header: c.header,
                data: &c.data,
                storage: c.storage,
            })
            .collect();
        let rebuilt =
            ustream::rebuild_with(stream.data(), stream.layout(), &[], &insertions).unwrap();
        let layout = StreamLayout::parse(&rebuilt).unwrap();
        assert_eq!(layout.resources.len(), 12);
        let mut provided = Provided::default();
        for (i, r) in layout.resources.iter().enumerate() {
            provided.add(&r.header, Some(&layout.resource_data(&rebuilt, i).unwrap()));
        }
        for (i, r) in layout.resources.iter().enumerate() {
            let data = layout.resource_data(&rebuilt, i).unwrap();
            for target in references(r.header.type_id, &data).unwrap() {
                assert!(provided.provides(&target), "{target:?} of resource {i}");
            }
        }
    }

    #[test]
    fn existing_data_is_pointed_at_clones() {
        let mut remap = Remap::default();
        remap.guids.insert([3; 4], [0x30; 4]);
        remap.names.insert(0x77, 0x770);
        let mut mm = metamodel(9, 3);
        assert_eq!(
            rewrite_references(TYPE_METAMODEL, &mut mm, &remap).unwrap(),
            1
        );
        assert_eq!(
            references(TYPE_METAMODEL, &mm).unwrap(),
            [Target::Guid([0x30; 4])]
        );
        let mut m = mesh(0x77);
        assert_eq!(rewrite_references(TYPE_MESH, &mut m, &remap).unwrap(), 1);
        assert_eq!(
            references(TYPE_MESH, &m).unwrap(),
            [Target::Material(0x770)]
        );
        // Unknown types and empty maps change nothing.
        let mut other = vec![3u8; 64];
        assert_eq!(
            rewrite_references(sdbm(b"lua"), &mut other, &remap).unwrap(),
            0
        );
        assert_eq!(
            rewrite_references(TYPE_MESH, &mut m, &Remap::default()).unwrap(),
            0
        );
        // Malformed data is an error, not a panic.
        assert!(rewrite_references(TYPE_MESH, &mut [0u8; 8], &remap).is_err());
        assert!(rewrite_references(TYPE_SIMGROUP, &mut [0u8; 8], &remap).is_err());
    }

    #[test]
    fn stream_guids_avoid_used_hashes() {
        let guid = new_stream_guid("gf2/doctor/x", &HashSet::new());
        assert_eq!(
            guid,
            neoguid::guid_for_name(&MachineName::TOOLKIT, "gf2/doctor/x", 0).0
        );
        assert_eq!(guid[3], b'X');
        let hash = crate::toc::guid_hash(&guid);
        let mut used = HashSet::new();
        used.insert(hash);
        let other = new_stream_guid("gf2/doctor/x", &used);
        assert_ne!(crate::toc::guid_hash(&other), hash);
    }
}
