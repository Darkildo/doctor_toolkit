//! `partsdb` resources: part databases of characters and vehicles (magic `PtDB`).
//!
//! A database lists the pieces a group of characters (`player`, `wrk_male`,
//! `fam_granados`) or one vehicle (`sedan60_03`) is assembled from:
//!
//! - **parts** (T0): one model each — the ids of its mesh resources at every
//!   level of detail (characters) or damage state (vehicles), the slot it
//!   occupies and the culling tags that hide it;
//! - **variants** (T1): named combinations of parts;
//! - **slots** (T2): `head`, `upper_body`, `a_hat`, ...;
//! - **presets** (T3): named lists of items, each a list of material swaps
//!   (the colour and pattern choices) and optionally a body blender;
//! - **blenders** (T4): morph target weights (`fat`, `skinny`, facial shapes);
//! - **sets** (T5): one list of allowed variants per slot (T7), the material
//!   candidates of single parts and the preset items the set may use;
//! - **groups** (T6): named lists of sets;
//! - one database descriptor (T8) that repeats the table directory.
//!
//! # Layout
//!
//! A memory image: every pointer is an absolute offset from the start of the
//! resource. All values are little-endian; nothing is aligned.
//!
//! ```text
//! 0x00  u32  magic 0x50744442 ("BDtP" on disk)
//! 0x04  u16  0x20; u16 version 1
//! 0x08  u32  0
//! 0x0C  u32  size of the resource
//! 0x10  9 × (u32 count, u32 pointer): tables T0..T8
//! 0x58  u32  pointer to the database name
//! 0x5C  u32  0
//! 0x60  T0, T1, T2, T5, T6, T7, T8, set material lists, T3 and its items
//!       and swaps, set preset lists, T4 and its weights, variant part lists,
//!       slot choice lists, set preset item lists, group set lists, part
//!       tag lists, strings
//! ```
//!
//! | Table | Stride | Entry |
//! |---|---|---|
//! | T0 parts | 0x3C | `*name, *slot (T2 or 0), 5 × u32 mesh id, 5 × u32 0, *label, u32 count, *list of *tag` |
//! | T1 variants | 0x10 | `*name, u32 0, u32 count, *list of u16 part index` |
//! | T2 slots | 4 | `*name` |
//! | T3 presets | 0x0C | `*name, u32 count, *items {*name, *T4 or 0, u32 count, *swaps {u32 from, u32 to, u32 damaged}}` |
//! | T4 blenders | 0x0C | `*name, u32 count, *weights {*morph name, f32 min, f32 max}` |
//! | T5 sets | 0x20 | `*name, *T8, u32 count, *first T7, u32 count, *part materials, u32 count, *preset choices` |
//! | T6 groups | 0x0C | `*name, u32 count, *list of *T5` |
//! | T7 slot choices | 0x0C | `*slot (T2), u32 count, *list of *T1` |
//! | T8 descriptor | 0x48 | `u32 0, u32 kind (2 or 4), *name, 7 × (count, pointer) of T0..T6, u32 0` |
//!
//! Part materials: `{*T0, u32 count, *{u32 material, u32 count, *list of u32 candidate}}`.
//! Preset choices: `{*T3, u32 count, *list of *preset item}`.
//!
//! Sets own consecutive runs of T7 entries, in set order. Strings are unique
//! and appear in the order of first use: database name, each part's name,
//! label and tags, variants, slots, presets with their items, blenders with
//! their weights, sets, groups. An empty list points to where it would have
//! been written — except part tag lists, which repeat the pointer of the last
//! non-empty tag list before them (0 before the first one).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::io::{self, Endian};
use crate::resource::simgroup::Hash32;
use crate::{Error, Result};

/// The magic as read as a little-endian `u32`.
pub const MAGIC: u32 = 0x5074_4442;
const HEADER_SIZE: usize = 0x60;
const TABLES: usize = 9;
const STRIDES: [usize; TABLES] = [0x3C, 0x10, 4, 0x0C, 0x0C, 0x20, 0x0C, 0x0C, 0x48];
/// Mesh ids per part.
pub const MODEL_SLOTS: usize = 5;

/// `kind` of character databases: two levels of detail per part.
pub const KIND_CHARACTER: u32 = 2;
/// `kind` of vehicle databases: damage states per part.
pub const KIND_VEHICLE: u32 = 4;

fn is_zero(h: &Hash32) -> bool {
    h.0 == 0
}

/// A part (T0): one model in every level of detail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub name: String,
    /// Index into [`PartsDb::slots`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<usize>,
    /// Ids of the `mesh` resources (0 = none). Characters: `<name>_lod1`,
    /// `<name>_lod2`. Vehicles: `_dmg0`, `_dmg1`, `_dmg2`, `_dmg3`, `_lod2`.
    pub models: [Hash32; MODEL_SLOTS],
    /// Vehicles: the collision piece (`pcollision_door`); empty for characters.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// Culling tags (`hatCulled`, `longSleeveCulled`): the part is hidden
    /// when the character wears something that sets the tag.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

impl Part {
    /// The mesh ids that are set.
    pub fn mesh_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.models.iter().map(|h| h.0).filter(|&h| h != 0)
    }
}

/// A variant (T1): a named combination of parts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Variant {
    pub name: String,
    /// Indices into [`PartsDb::parts`].
    pub parts: Vec<u16>,
}

/// One material replacement of a preset item.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MaterialSwap {
    /// Name hash (CRC-32) of the material the meshes use.
    pub from: Hash32,
    /// The material drawn instead.
    pub to: Hash32,
    /// Vehicles: the material of the damaged state (`car_body_dent_blue`); 0 otherwise.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub damaged: Hash32,
}

/// An item of a preset: one look (colours, patterns) and body shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetItem {
    pub name: String,
    /// Index into [`PartsDb::blenders`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blender: Option<usize>,
    pub swaps: Vec<MaterialSwap>,
}

/// A preset (T3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub items: Vec<PresetItem>,
}

/// A morph target weight of a blender.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MorphWeight {
    /// Morph target (`fat`, `skinny`, `headnawd`).
    pub name: String,
    pub min: f32,
    pub max: f32,
}

/// A body blender (T4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Blender {
    pub name: String,
    pub weights: Vec<MorphWeight>,
}

/// The variants a set allows in one slot (T7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlotChoice {
    /// Index into [`PartsDb::slots`].
    pub slot: usize,
    /// Indices into [`PartsDb::variants`]; the game picks one.
    pub variants: Vec<usize>,
}

/// The materials one material of a part may be replaced with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaterialChoice {
    /// Name hash of the material the part's mesh uses.
    pub material: Hash32,
    /// Candidates; the game picks one.
    pub candidates: Vec<Hash32>,
}

/// Material choices of one part in a set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartMaterials {
    /// Index into [`PartsDb::parts`].
    pub part: usize,
    pub materials: Vec<MaterialChoice>,
}

/// The items of one preset a set may use.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetChoice {
    /// Index into [`PartsDb::presets`].
    pub preset: usize,
    /// Indices into the preset's items.
    pub items: Vec<usize>,
}

/// A set (T5): one character (or vehicle) type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Set {
    pub name: String,
    pub choices: Vec<SlotChoice>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub materials: Vec<PartMaterials>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub presets: Vec<PresetChoice>,
}

/// A group of sets (T6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub name: String,
    /// Indices into [`PartsDb::sets`].
    pub sets: Vec<usize>,
}

/// A parsed parts database.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartsDb {
    pub name: String,
    /// [`KIND_CHARACTER`] or [`KIND_VEHICLE`].
    pub kind: u32,
    pub slots: Vec<String>,
    pub parts: Vec<Part>,
    pub variants: Vec<Variant>,
    pub presets: Vec<Preset>,
    pub blenders: Vec<Blender>,
    pub sets: Vec<Set>,
    pub groups: Vec<Group>,
}

struct Reader<'a> {
    data: &'a [u8],
    tables: [(usize, usize); TABLES],
}

impl Reader<'_> {
    fn u32(&self, at: usize) -> Result<u32> {
        io::read_u32(self.data, at, Endian::Little)
    }

    fn f32(&self, at: usize) -> Result<f32> {
        self.u32(at).map(f32::from_bits)
    }

    fn string_at(&self, at: usize, field: usize) -> Result<String> {
        if at < HEADER_SIZE {
            return Err(Error::malformed("partsdb", field, "string in the header"));
        }
        Ok(String::from_utf8_lossy(io::cstr(self.data, at)?).into_owned())
    }

    fn string(&self, field: usize) -> Result<String> {
        self.string_at(self.u32(field)? as usize, field)
    }

    fn entries(&self, table: usize) -> impl Iterator<Item = usize> + use<> {
        let (start, count) = self.tables[table];
        (0..count).map(move |k| start + STRIDES[table] * k)
    }

    /// Index of the entry of `table` at `pointer`.
    fn entry(&self, table: usize, pointer: u32, field: usize) -> Result<usize> {
        let (start, count) = self.tables[table];
        (pointer as usize)
            .checked_sub(start)
            .filter(|o| o % STRIDES[table] == 0 && o / STRIDES[table] < count)
            .map(|o| o / STRIDES[table])
            .ok_or_else(|| Error::malformed("partsdb", field, format!("not a T{table} entry")))
    }

    /// `(count, pointer)` at `field`, with `count × size` bytes checked.
    fn list(&self, field: usize, size: usize) -> Result<(usize, usize)> {
        let count = self.u32(field)? as usize;
        let at = self.u32(field + 4)? as usize;
        if count > 0 {
            io::slice(self.data, at, count.saturating_mul(size))?;
        }
        Ok((count, at))
    }

    /// The u32 list described by `(count, pointer)` at `field`.
    fn words(&self, field: usize) -> Result<Vec<u32>> {
        let (count, at) = self.list(field, 4)?;
        (0..count).map(|k| self.u32(at + 4 * k)).collect()
    }
}

impl PartsDb {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let le = Endian::Little;
        let magic = io::read_u32(data, 0, le)?;
        if magic != MAGIC {
            return Err(Error::BadMagic {
                what: "partsdb",
                found: magic,
            });
        }
        let version = io::read_u16(data, 6, le)?;
        if version != 1 {
            return Err(Error::UnsupportedVersion {
                what: "partsdb",
                version: version.into(),
            });
        }
        io::slice(data, 0, HEADER_SIZE)?;
        let mut tables = [(0usize, 0usize); TABLES];
        for (i, table) in tables.iter_mut().enumerate() {
            let count = io::read_u32(data, 0x10 + 8 * i, le)? as usize;
            let start = io::read_u32(data, 0x14 + 8 * i, le)? as usize;
            if count > 0 {
                if start < HEADER_SIZE {
                    return Err(Error::malformed(
                        "partsdb",
                        0x14 + 8 * i,
                        "table in the header",
                    ));
                }
                // Bounds the allocations below by the size of the resource.
                io::slice(data, start, count.saturating_mul(STRIDES[i]))?;
            }
            *table = (start, count);
        }
        if tables[8].1 != 1 {
            return Err(Error::malformed("partsdb", 0x50, "not one descriptor"));
        }
        let r = Reader { data, tables };
        let name = r.string(0x58)?;
        let kind = r.u32(tables[8].0 + 4)?;

        let slots = r.entries(2).map(|at| r.string(at)).collect::<Result<_>>()?;
        let parts = r
            .entries(0)
            .map(|at| {
                let slot = match r.u32(at + 4)? {
                    0 => None,
                    p => Some(r.entry(2, p, at + 4)?),
                };
                let mut models = [Hash32(0); MODEL_SLOTS];
                for (k, m) in models.iter_mut().enumerate() {
                    *m = Hash32(r.u32(at + 8 + 4 * k)?);
                }
                let tags = r
                    .words(at + 0x34)?
                    .into_iter()
                    .map(|p| r.string_at(p as usize, at + 0x38))
                    .collect::<Result<_>>()?;
                Ok(Part {
                    name: r.string(at)?,
                    slot,
                    models,
                    label: r.string(at + 0x30)?,
                    tags,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let variants = r
            .entries(1)
            .map(|at| {
                let (count, list) = r.list(at + 8, 2)?;
                let parts = (0..count)
                    .map(|k| {
                        let index = io::read_u16(data, list + 2 * k, le)?;
                        if index as usize >= parts.len() {
                            return Err(Error::malformed(
                                "partsdb",
                                list + 2 * k,
                                "part index out of range",
                            ));
                        }
                        Ok(index)
                    })
                    .collect::<Result<_>>()?;
                Ok(Variant {
                    name: r.string(at)?,
                    parts,
                })
            })
            .collect::<Result<_>>()?;
        let blenders = r
            .entries(4)
            .map(|at| {
                let (count, list) = r.list(at + 4, 12)?;
                Ok(Blender {
                    name: r.string(at)?,
                    weights: (0..count)
                        .map(|k| {
                            let w = list + 12 * k;
                            Ok(MorphWeight {
                                name: r.string(w)?,
                                min: r.f32(w + 4)?,
                                max: r.f32(w + 8)?,
                            })
                        })
                        .collect::<Result<_>>()?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        // Preset item address -> (preset, item), for the preset choices of sets.
        let mut items_at = HashMap::new();
        let presets = r
            .entries(3)
            .enumerate()
            .map(|(p, at)| {
                let (count, list) = r.list(at + 4, 0x10)?;
                let items = (0..count)
                    .map(|k| {
                        let it = list + 0x10 * k;
                        items_at.insert(it as u32, (p, k));
                        let blender = match r.u32(it + 4)? {
                            0 => None,
                            b => Some(r.entry(4, b, it + 4)?),
                        };
                        let (swaps, swap_at) = r.list(it + 8, 12)?;
                        Ok(PresetItem {
                            name: r.string(it)?,
                            blender,
                            swaps: (0..swaps)
                                .map(|q| {
                                    let s = swap_at + 12 * q;
                                    Ok(MaterialSwap {
                                        from: Hash32(r.u32(s)?),
                                        to: Hash32(r.u32(s + 4)?),
                                        damaged: Hash32(r.u32(s + 8)?),
                                    })
                                })
                                .collect::<Result<_>>()?,
                        })
                    })
                    .collect::<Result<_>>()?;
                Ok(Preset {
                    name: r.string(at)?,
                    items,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let choice = |at: usize| -> Result<SlotChoice> {
            Ok(SlotChoice {
                slot: r.entry(2, r.u32(at)?, at)?,
                variants: r
                    .words(at + 4)?
                    .into_iter()
                    .map(|p| r.entry(1, p, at + 8))
                    .collect::<Result<_>>()?,
            })
        };
        let mut next_choice = 0;
        let sets = r
            .entries(5)
            .map(|at| {
                if r.u32(at + 4)? as usize != tables[8].0 {
                    return Err(Error::malformed("partsdb", at + 4, "not the descriptor"));
                }
                let count = r.u32(at + 8)? as usize;
                if count > 0 {
                    let first = r.entry(7, r.u32(at + 0x0C)?, at + 0x0C)?;
                    if first != next_choice {
                        return Err(Error::malformed(
                            "partsdb",
                            at + 0x0C,
                            "slot choices out of order",
                        ));
                    }
                }
                if next_choice + count > tables[7].1 {
                    return Err(Error::malformed("partsdb", at + 8, "too many slot choices"));
                }
                let choices = (next_choice..next_choice + count)
                    .map(|k| choice(tables[7].0 + STRIDES[7] * k))
                    .collect::<Result<_>>()?;
                next_choice += count;
                let (count, list) = r.list(at + 0x10, 12)?;
                let materials = (0..count)
                    .map(|k| {
                        let m = list + 12 * k;
                        let (n, slots) = r.list(m + 4, 12)?;
                        Ok(PartMaterials {
                            part: r.entry(0, r.u32(m)?, m)?,
                            materials: (0..n)
                                .map(|q| {
                                    let c = slots + 12 * q;
                                    Ok(MaterialChoice {
                                        material: Hash32(r.u32(c)?),
                                        candidates: r
                                            .words(c + 4)?
                                            .into_iter()
                                            .map(Hash32)
                                            .collect(),
                                    })
                                })
                                .collect::<Result<_>>()?,
                        })
                    })
                    .collect::<Result<_>>()?;
                let (count, list) = r.list(at + 0x18, 12)?;
                let presets = (0..count)
                    .map(|k| {
                        let c = list + 12 * k;
                        let preset = r.entry(3, r.u32(c)?, c)?;
                        let items = r
                            .words(c + 4)?
                            .into_iter()
                            .map(|p| match items_at.get(&p) {
                                Some(&(owner, item)) if owner == preset => Ok(item),
                                _ => Err(Error::malformed(
                                    "partsdb",
                                    c + 8,
                                    "not an item of the preset",
                                )),
                            })
                            .collect::<Result<_>>()?;
                        Ok(PresetChoice { preset, items })
                    })
                    .collect::<Result<_>>()?;
                Ok(Set {
                    name: r.string(at)?,
                    choices,
                    materials,
                    presets,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        if next_choice != tables[7].1 {
            return Err(Error::malformed(
                "partsdb",
                0x48,
                "slot choices no set owns",
            ));
        }
        let groups = r
            .entries(6)
            .map(|at| {
                Ok(Group {
                    name: r.string(at)?,
                    sets: r
                        .words(at + 4)?
                        .into_iter()
                        .map(|p| r.entry(5, p, at + 8))
                        .collect::<Result<_>>()?,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            name,
            kind,
            slots,
            parts,
            variants,
            presets,
            blenders,
            sets,
            groups,
        })
    }

    /// Checks that every index points into its table.
    pub fn validate(&self) -> Result<()> {
        let bad = |what: String| Err(Error::InvalidArgument(what));
        if self.parts.len() > u16::MAX as usize + 1 {
            return bad(format!("{} parts; at most 65536", self.parts.len()));
        }
        for (i, part) in self.parts.iter().enumerate() {
            if part.slot.is_some_and(|s| s >= self.slots.len()) {
                return bad(format!("part {i} `{}`: no such slot", part.name));
            }
        }
        for (i, v) in self.variants.iter().enumerate() {
            if let Some(p) = v.parts.iter().find(|&&p| p as usize >= self.parts.len()) {
                return bad(format!("variant {i} `{}`: no part {p}", v.name));
            }
        }
        for (i, p) in self.presets.iter().enumerate() {
            for item in &p.items {
                if item.blender.is_some_and(|b| b >= self.blenders.len()) {
                    return bad(format!("preset {i} `{}`: no such blender", p.name));
                }
            }
        }
        for (i, set) in self.sets.iter().enumerate() {
            let at = format!("set {i} `{}`", set.name);
            for c in &set.choices {
                if c.slot >= self.slots.len() {
                    return bad(format!("{at}: no slot {}", c.slot));
                }
                if let Some(v) = c.variants.iter().find(|&&v| v >= self.variants.len()) {
                    return bad(format!("{at}: no variant {v}"));
                }
            }
            for m in &set.materials {
                if m.part >= self.parts.len() {
                    return bad(format!("{at}: no part {}", m.part));
                }
            }
            for p in &set.presets {
                let Some(preset) = self.presets.get(p.preset) else {
                    return bad(format!("{at}: no preset {}", p.preset));
                };
                if let Some(item) = p.items.iter().find(|&&k| k >= preset.items.len()) {
                    return bad(format!("{at}: preset `{}` has no item {item}", preset.name));
                }
            }
        }
        for (i, g) in self.groups.iter().enumerate() {
            if let Some(s) = g.sets.iter().find(|&&s| s >= self.sets.len()) {
                return bad(format!("group {i} `{}`: no set {s}", g.name));
            }
        }
        Ok(())
    }

    /// Writes the database in the layout the game files use; `encode(parse(x)) == x`.
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        Ok(Writer::default().write(self))
    }

    // ---- lookups and edits ----

    pub fn slot_index(&self, name: &str) -> Option<usize> {
        self.slots.iter().position(|s| s.eq_ignore_ascii_case(name))
    }

    pub fn part_index(&self, name: &str) -> Option<usize> {
        self.parts
            .iter()
            .position(|p| p.name.eq_ignore_ascii_case(name))
    }

    pub fn variant_index(&self, name: &str) -> Option<usize> {
        self.variants
            .iter()
            .position(|v| v.name.eq_ignore_ascii_case(name))
    }

    pub fn set_index(&self, name: &str) -> Option<usize> {
        self.sets
            .iter()
            .position(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// Adds a part; returns its index.
    pub fn add_part(&mut self, part: Part) -> Result<usize> {
        if part.slot.is_some_and(|s| s >= self.slots.len()) {
            return Err(Error::InvalidArgument(format!(
                "part `{}`: no such slot",
                part.name
            )));
        }
        if self.parts.len() > u16::MAX as usize {
            return Err(Error::InvalidArgument("too many parts".into()));
        }
        self.parts.push(part);
        Ok(self.parts.len() - 1)
    }

    /// Adds a variant made of `parts`; returns its index.
    pub fn add_variant(&mut self, name: &str, parts: Vec<u16>) -> Result<usize> {
        if let Some(p) = parts.iter().find(|&&p| p as usize >= self.parts.len()) {
            return Err(Error::InvalidArgument(format!("no part {p}")));
        }
        self.variants.push(Variant {
            name: name.to_owned(),
            parts,
        });
        Ok(self.variants.len() - 1)
    }

    /// Lets `set` pick `variant` in `slot`. With `only`, the variant becomes
    /// the only choice of the slot. Returns whether the set changed.
    pub fn allow_variant(
        &mut self,
        set: usize,
        slot: usize,
        variant: usize,
        only: bool,
    ) -> Result<bool> {
        if variant >= self.variants.len() || slot >= self.slots.len() {
            return Err(Error::InvalidArgument("no such variant or slot".into()));
        }
        let set = self
            .sets
            .get_mut(set)
            .ok_or_else(|| Error::InvalidArgument("no such set".into()))?;
        let choice = match set.choices.iter_mut().find(|c| c.slot == slot) {
            Some(choice) => choice,
            None => {
                set.choices.push(SlotChoice {
                    slot,
                    variants: Vec::new(),
                });
                set.choices.last_mut().expect("just pushed")
            }
        };
        if only {
            if choice.variants == [variant] {
                return Ok(false);
            }
            choice.variants = vec![variant];
            return Ok(true);
        }
        if choice.variants.contains(&variant) {
            return Ok(false);
        }
        choice.variants.push(variant);
        Ok(true)
    }

    /// Copies set `from` under a new name and adds it to the groups of the
    /// original; returns the index of the copy.
    pub fn clone_set(&mut self, from: usize, name: &str) -> Result<usize> {
        let mut set = self
            .sets
            .get(from)
            .cloned()
            .ok_or_else(|| Error::InvalidArgument("no such set".into()))?;
        set.name = name.to_owned();
        self.sets.push(set);
        let index = self.sets.len() - 1;
        for group in &mut self.groups {
            if group.sets.contains(&from) {
                group.sets.push(index);
            }
        }
        Ok(index)
    }
}

/// Lays the database out like the game's tool does.
#[derive(Default)]
struct Writer {
    out: Vec<u8>,
    /// `(offset of the pointer, string)` in the order strings are first used.
    strings: Vec<(usize, String)>,
}

impl Writer {
    fn put(&mut self, at: usize, value: u32) {
        self.out[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn reserve(&mut self, size: usize) -> usize {
        let at = self.out.len();
        self.out.resize(at + size, 0);
        at
    }

    fn push(&mut self, value: u32) {
        self.out.extend_from_slice(&value.to_le_bytes());
    }

    /// Writes `(count, pointer to the current end)` at `at`.
    fn list_here(&mut self, at: usize, count: usize) {
        let here = self.out.len() as u32;
        self.put(at, count as u32);
        self.put(at + 4, here);
    }

    fn string(&mut self, at: usize, text: &str) {
        self.strings.push((at, text.to_owned()));
    }

    fn write(mut self, db: &PartsDb) -> Vec<u8> {
        let choices: usize = db.sets.iter().map(|s| s.choices.len()).sum();
        let counts = [
            db.parts.len(),
            db.variants.len(),
            db.slots.len(),
            db.presets.len(),
            db.blenders.len(),
            db.sets.len(),
            db.groups.len(),
            choices,
            1,
        ];
        let mut starts = [0usize; TABLES];
        self.reserve(HEADER_SIZE);
        for table in [0, 1, 2, 5, 6, 7, 8] {
            starts[table] = self.reserve(STRIDES[table] * counts[table]);
        }
        macro_rules! entry {
            ($table:expr, $index:expr) => {
                (starts[$table] + STRIDES[$table] * $index) as u32
            };
        }

        // Material choices of parts: entries, then choices, then candidates.
        let mut part_lists = Vec::new();
        for (k, set) in db.sets.iter().enumerate() {
            self.list_here(entry!(5, k) as usize + 0x10, set.materials.len());
            for m in &set.materials {
                part_lists.push((self.reserve(12), m));
            }
        }
        let mut candidate_lists = Vec::new();
        for (at, m) in part_lists {
            self.put(at, entry!(0, m.part));
            self.list_here(at + 4, m.materials.len());
            for c in &m.materials {
                candidate_lists.push((self.reserve(12), c));
            }
        }
        for (at, c) in candidate_lists {
            self.put(at, c.material.0);
            self.list_here(at + 4, c.candidates.len());
            for h in &c.candidates {
                self.push(h.0);
            }
        }

        // Presets: entries, items, swaps.
        starts[3] = self.reserve(STRIDES[3] * counts[3]);
        let mut item_at: Vec<Vec<u32>> = Vec::with_capacity(db.presets.len());
        let mut items = Vec::new();
        for (k, preset) in db.presets.iter().enumerate() {
            self.list_here(entry!(3, k) as usize + 4, preset.items.len());
            let mut here = Vec::with_capacity(preset.items.len());
            for item in &preset.items {
                let at = self.reserve(0x10);
                here.push(at as u32);
                items.push((at, item));
            }
            item_at.push(here);
        }
        for (at, item) in &items {
            self.list_here(at + 8, item.swaps.len());
            for s in &item.swaps {
                self.push(s.from.0);
                self.push(s.to.0);
                self.push(s.damaged.0);
            }
        }

        // Preset choices of sets.
        let mut preset_lists = Vec::new();
        for (k, set) in db.sets.iter().enumerate() {
            self.list_here(entry!(5, k) as usize + 0x18, set.presets.len());
            for p in &set.presets {
                preset_lists.push((self.reserve(12), p));
            }
        }

        // Blenders.
        starts[4] = self.reserve(STRIDES[4] * counts[4]);
        let mut weight_at = Vec::with_capacity(db.blenders.len());
        for (k, blender) in db.blenders.iter().enumerate() {
            self.list_here(entry!(4, k) as usize + 4, blender.weights.len());
            let at = self.out.len();
            for w in &blender.weights {
                self.reserve(4);
                self.push(w.min.to_bits());
                self.push(w.max.to_bits());
            }
            weight_at.push(at);
        }

        // Part lists of variants.
        for (k, variant) in db.variants.iter().enumerate() {
            self.list_here(entry!(1, k) as usize + 8, variant.parts.len());
            for p in &variant.parts {
                self.out.extend_from_slice(&p.to_le_bytes());
            }
        }

        // Slot choices: the T7 run of each set, then their variant lists.
        let mut choice = 0;
        for (k, set) in db.sets.iter().enumerate() {
            let at = entry!(5, k) as usize;
            self.put(at + 4, starts[8] as u32);
            self.put(at + 8, set.choices.len() as u32);
            self.put(at + 0x0C, entry!(7, choice));
            for c in &set.choices {
                let e = entry!(7, choice) as usize;
                self.put(e, entry!(2, c.slot));
                self.list_here(e + 4, c.variants.len());
                for &v in &c.variants {
                    self.push(entry!(1, v));
                }
                choice += 1;
            }
        }

        for (at, p) in preset_lists {
            self.put(at, entry!(3, p.preset));
            self.list_here(at + 4, p.items.len());
            for &item in &p.items {
                self.push(item_at[p.preset][item]);
            }
        }

        for (k, group) in db.groups.iter().enumerate() {
            self.list_here(entry!(6, k) as usize + 4, group.sets.len());
            for &s in &group.sets {
                self.push(entry!(5, s));
            }
        }

        // Tag lists; an empty one repeats the last pointer.
        let mut tag_at = Vec::with_capacity(db.parts.len());
        let mut last = 0u32;
        for (k, part) in db.parts.iter().enumerate() {
            let at = entry!(0, k) as usize;
            if !part.tags.is_empty() {
                last = self.out.len() as u32;
            }
            self.put(at + 0x34, part.tags.len() as u32);
            self.put(at + 0x38, last);
            tag_at.push(self.reserve(4 * part.tags.len()));
        }

        // Fixed fields and strings, in the order strings are first used.
        self.string(0x58, &db.name);
        for (k, part) in db.parts.iter().enumerate() {
            let at = entry!(0, k) as usize;
            self.string(at, &part.name);
            self.put(at + 4, part.slot.map_or(0, |s| entry!(2, s)));
            for (i, m) in part.models.iter().enumerate() {
                self.put(at + 8 + 4 * i, m.0);
            }
            self.string(at + 0x30, &part.label);
            for (j, tag) in part.tags.iter().enumerate() {
                self.string(tag_at[k] + 4 * j, tag);
            }
        }
        for (k, v) in db.variants.iter().enumerate() {
            self.string(entry!(1, k) as usize, &v.name);
        }
        for (k, s) in db.slots.iter().enumerate() {
            self.string(entry!(2, k) as usize, s);
        }
        for (k, preset) in db.presets.iter().enumerate() {
            self.string(entry!(3, k) as usize, &preset.name);
            for (j, item) in preset.items.iter().enumerate() {
                let at = item_at[k][j] as usize;
                self.string(at, &item.name);
                self.put(at + 4, item.blender.map_or(0, |b| entry!(4, b)));
            }
        }
        for (k, blender) in db.blenders.iter().enumerate() {
            self.string(entry!(4, k) as usize, &blender.name);
            for (j, w) in blender.weights.iter().enumerate() {
                self.string(weight_at[k] + 12 * j, &w.name);
            }
        }
        for (k, set) in db.sets.iter().enumerate() {
            self.string(entry!(5, k) as usize, &set.name);
        }
        for (k, group) in db.groups.iter().enumerate() {
            self.string(entry!(6, k) as usize, &group.name);
        }
        let mut placed: HashMap<String, u32> = HashMap::new();
        for (at, text) in std::mem::take(&mut self.strings) {
            let pointer = match placed.get(&text) {
                Some(&p) => p,
                None => {
                    let p = self.out.len() as u32;
                    self.out.extend_from_slice(text.as_bytes());
                    self.out.push(0);
                    placed.insert(text, p);
                    p
                }
            };
            self.put(at, pointer);
        }

        // Descriptor and header.
        let d = starts[8];
        self.put(d + 4, db.kind);
        self.put(d + 8, placed[&db.name]);
        for i in 0..7 {
            self.put(d + 12 + 8 * i, counts[i] as u32);
            self.put(d + 16 + 8 * i, starts[i] as u32);
        }
        self.put(0, MAGIC);
        self.out[4..8].copy_from_slice(&[0x20, 0, 1, 0]);
        let size = self.out.len() as u32;
        self.put(0x0C, size);
        for i in 0..TABLES {
            self.put(0x10 + 8 * i, counts[i] as u32);
            self.put(0x14 + 8 * i, starts[i] as u32);
        }
        self.put(0x58, placed[&db.name]);
        self.out
    }
}

/// What a character (or vehicle) looks like for one choice of variants and items.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Assembly {
    /// Chosen variant per slot choice of the set: `(slot, variant)`.
    pub variants: Vec<(usize, usize)>,
    /// Parts to draw, after culling.
    pub parts: Vec<usize>,
    /// Parts hidden by culling tags.
    pub culled: Vec<usize>,
    /// Material replacements `(from, to)`: preset item swaps, then part choices.
    pub swaps: Vec<(u32, u32)>,
    /// Material replacements that apply to one part only: `(part, from, to)`.
    pub part_swaps: Vec<(usize, u32, u32)>,
    /// Morph target weights of the blender: `(name, weight)`.
    pub morphs: Vec<(String, f32)>,
}

/// Choices for [`PartsDb::assemble`]; unset entries take the first option.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Selection {
    /// Variant index per slot index.
    pub variants: HashMap<usize, usize>,
    /// Preset item per preset choice of the set: `(preset choice, item position)`.
    pub items: HashMap<usize, usize>,
    /// Candidate per `(part, material hash)`.
    pub materials: HashMap<(usize, u32), usize>,
    /// Culling tags in effect, in addition to the ones derived from the parts.
    pub tags: Vec<String>,
}

/// The culling tags a part sets when it is worn, by slot and name (the game
/// sets them from the menus of `mobface`; this is an approximation).
pub fn tags_set_by(slot: &str, part: &str) -> Vec<&'static str> {
    let part = part.to_ascii_lowercase();
    let mut tags = Vec::new();
    if slot.eq_ignore_ascii_case("a_hat") || slot.eq_ignore_ascii_case("hat") {
        tags.push("hatCulled");
    }
    if part.contains("glove") {
        tags.push("gloveCulled");
    }
    if part.contains("_ut") || part.contains("untucked") {
        tags.push("untuckedShirtCulled");
    }
    tags
}

impl PartsDb {
    /// Resolves set `set` with `selection` to parts, material swaps and morphs.
    pub fn assemble(&self, set: usize, selection: &Selection) -> Result<Assembly> {
        let set = self
            .sets
            .get(set)
            .ok_or_else(|| Error::InvalidArgument(format!("no set {set}")))?;
        let mut out = Assembly::default();
        let mut parts = Vec::new();
        let mut tags: Vec<String> = selection.tags.clone();
        for choice in &set.choices {
            let Some(&first) = choice.variants.first() else {
                continue;
            };
            let variant = selection
                .variants
                .get(&choice.slot)
                .copied()
                .filter(|v| choice.variants.contains(v))
                .unwrap_or(first);
            out.variants.push((choice.slot, variant));
            for &p in &self.variants[variant].parts {
                let p = p as usize;
                if !parts.contains(&p) {
                    parts.push(p);
                    let slot = self.parts[p].slot.map_or("", |s| self.slots[s].as_str());
                    if !self.parts[p].mesh_ids().any(|_| true) {
                        continue;
                    }
                    tags.extend(
                        tags_set_by(slot, &self.parts[p].name)
                            .into_iter()
                            .map(str::to_owned),
                    );
                }
            }
        }
        for p in parts {
            let hidden = self.parts[p]
                .tags
                .iter()
                .any(|t| tags.iter().any(|s| s.eq_ignore_ascii_case(t)));
            if hidden {
                out.culled.push(p);
            } else {
                out.parts.push(p);
            }
        }
        for (k, choice) in set.presets.iter().enumerate() {
            let Some(&first) = choice.items.first() else {
                continue;
            };
            let item = selection
                .items
                .get(&k)
                .and_then(|&i| choice.items.get(i))
                .copied()
                .unwrap_or(first);
            let item = &self.presets[choice.preset].items[item];
            out.swaps
                .extend(item.swaps.iter().map(|s| (s.from.0, s.to.0)));
            if let Some(b) = item.blender {
                for w in &self.blenders[b].weights {
                    out.morphs.push((w.name.clone(), w.max));
                }
            }
        }
        for m in &set.materials {
            for c in &m.materials {
                let pick = selection
                    .materials
                    .get(&(m.part, c.material.0))
                    .and_then(|&i| c.candidates.get(i))
                    .or(c.candidates.first());
                if let Some(to) = pick {
                    out.part_swaps.push((m.part, c.material.0, to.0));
                }
            }
        }
        Ok(out)
    }

    /// The material a mesh of `part` draws instead of `material` in `assembly`.
    pub fn resolve_material(assembly: &Assembly, part: usize, material: u32) -> u32 {
        if let Some(&(_, _, to)) = assembly
            .part_swaps
            .iter()
            .find(|&&(p, from, _)| p == part && from == material)
        {
            return to;
        }
        assembly
            .swaps
            .iter()
            .find(|&&(from, _)| from == material)
            .map_or(material, |&(_, to)| to)
    }
}

/// A small parts database: two slots, three parts, a preset and a blender.
#[cfg(any(test, feature = "testing"))]
pub(crate) fn sample_db() -> PartsDb {
    PartsDb {
        name: "db".into(),
        kind: KIND_CHARACTER,
        slots: vec!["head".into(), "a_hat".into()],
        parts: vec![
            Part {
                name: "head".into(),
                slot: Some(0),
                models: [
                    Hash32(0xAABB_CCDD),
                    Hash32(0x1122_3344),
                    Hash32(0),
                    Hash32(0),
                    Hash32(0),
                ],
                label: String::new(),
                tags: vec![],
            },
            Part {
                name: "hair".into(),
                slot: Some(0),
                models: [Hash32(5), Hash32(0), Hash32(0), Hash32(0), Hash32(0)],
                label: String::new(),
                tags: vec!["hatCulled".into()],
            },
            Part {
                name: "hat".into(),
                slot: Some(1),
                models: [Hash32(6), Hash32(7), Hash32(0), Hash32(0), Hash32(0)],
                label: String::new(),
                tags: vec![],
            },
        ],
        variants: vec![
            Variant {
                name: "head".into(),
                parts: vec![0, 1],
            },
            Variant {
                name: "hat".into(),
                parts: vec![2],
            },
            Variant {
                name: "no_hat".into(),
                parts: vec![],
            },
        ],
        presets: vec![Preset {
            name: "db".into(),
            items: vec![PresetItem {
                name: "a_default".into(),
                blender: Some(0),
                swaps: vec![MaterialSwap {
                    from: Hash32(1),
                    to: Hash32(2),
                    damaged: Hash32(0),
                }],
            }],
        }],
        blenders: vec![Blender {
            name: "bodyblender_fat".into(),
            weights: vec![MorphWeight {
                name: "fat".into(),
                min: 0.75,
                max: 0.75,
            }],
        }],
        sets: vec![Set {
            name: "db".into(),
            choices: vec![
                SlotChoice {
                    slot: 0,
                    variants: vec![0],
                },
                SlotChoice {
                    slot: 1,
                    variants: vec![2, 1],
                },
            ],
            materials: vec![PartMaterials {
                part: 0,
                materials: vec![MaterialChoice {
                    material: Hash32(9),
                    candidates: vec![Hash32(10), Hash32(11)],
                }],
            }],
            presets: vec![PresetChoice {
                preset: 0,
                items: vec![0],
            }],
        }],
        groups: vec![Group {
            name: "_all".into(),
            sets: vec![0],
        }],
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sample_db() -> PartsDb {
        super::sample_db()
    }

    #[test]
    fn encodes_and_parses_back() {
        let db = sample_db();
        let data = db.encode().unwrap();
        assert_eq!(&data[0..4], b"BDtP");
        let back = PartsDb::parse(&data).unwrap();
        assert_eq!(back, db);
        assert_eq!(back.encode().unwrap(), data);
        // JSON round trip.
        let json = serde_json::to_string(&db).unwrap();
        let from_json: PartsDb = serde_json::from_str(&json).unwrap();
        assert_eq!(from_json, db);
        assert!(json.contains("\"aabbccdd\""));
    }

    #[test]
    fn empty_tag_lists_repeat_the_last_pointer() {
        let data = sample_db().encode().unwrap();
        let ptr =
            |part: usize| io::read_u32(&data, 0x60 + 0x3C * part + 0x38, Endian::Little).unwrap();
        assert_eq!(ptr(0), 0);
        assert_ne!(ptr(1), 0);
        assert_eq!(ptr(2), ptr(1));
    }

    #[test]
    fn edits_and_assembly() {
        let mut db = sample_db();
        let a = db.assemble(0, &Selection::default()).unwrap();
        assert_eq!(a.parts, vec![0, 1]);
        assert_eq!(a.swaps, vec![(1, 2)]);
        assert_eq!(a.part_swaps, vec![(0, 9, 10)]);
        assert_eq!(a.morphs, vec![("fat".to_owned(), 0.75)]);
        assert_eq!(PartsDb::resolve_material(&a, 0, 9), 10);
        assert_eq!(PartsDb::resolve_material(&a, 1, 1), 2);
        // A hat hides the hair.
        let mut sel = Selection::default();
        sel.variants.insert(1, 1);
        let a = db.assemble(0, &sel).unwrap();
        assert_eq!(a.parts, vec![0, 2]);
        assert_eq!(a.culled, vec![1]);

        let p = db
            .add_part(Part {
                name: "coat".into(),
                slot: Some(1),
                models: [Hash32(42), Hash32(0), Hash32(0), Hash32(0), Hash32(0)],
                label: String::new(),
                tags: vec![],
            })
            .unwrap();
        let v = db.add_variant("coat", vec![p as u16]).unwrap();
        assert!(db.allow_variant(0, 1, v, true).unwrap());
        let copy = db.clone_set(0, "db_copy").unwrap();
        assert_eq!(db.groups[0].sets, vec![0, copy]);
        let data = db.encode().unwrap();
        assert_eq!(PartsDb::parse(&data).unwrap(), db);
        let a = db.assemble(copy, &Selection::default()).unwrap();
        assert!(a.parts.contains(&p));
        assert!(db.add_variant("bad", vec![99]).is_err());
        db.variants[0].parts.push(500);
        assert!(db.encode().is_err());
    }

    #[test]
    fn rejects_bad_input() {
        let d = sample_db().encode().unwrap();
        assert!(PartsDb::parse(&d[..0x40]).is_err());
        let mut bad = d.clone();
        bad[0] ^= 1;
        assert!(matches!(PartsDb::parse(&bad), Err(Error::BadMagic { .. })));
        let mut huge = d.clone();
        huge[0x10..0x14].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(PartsDb::parse(&huge).is_err());
        for i in 0..d.len() {
            let mut c = d.clone();
            c[i] ^= 0x41;
            let _ = PartsDb::parse(&c);
            let _ = PartsDb::parse(&d[..i]);
        }
    }
}
