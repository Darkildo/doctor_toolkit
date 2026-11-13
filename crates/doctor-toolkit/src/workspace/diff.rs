//! Differences between the original and the edited data of a resource.
//!
//! [`byte_diff`] works for every type. [`semantic_diff`] is the hook for
//! type-aware summaries: string tables, texture headers, game-logic
//! resources (SimGroup, metamodel, variable dictionary) and `apx` files.

use std::ops::Range;

use crate::resource::apx::{AptFile, TAG_ALIGN};
use crate::resource::lh2::StringTable;
use crate::resource::logic::{TYPE_METAMODEL, TYPE_SIMGROUP, TYPE_VARIABLE_DICTIONARY};
use crate::resource::metamodel::MetaModel;
use crate::resource::simgroup::SimGroup;
use crate::resource::simgroup_edit;
use crate::resource::texture::TYPE_TG4H;
use crate::resource::tg4::Tg4Header;
use crate::resource::variabledictionary::VariableDictionary;

const TYPE_LH2: u32 = crate::hash::sdbm(b"lh2");
const TYPE_APX: u32 = crate::hash::sdbm(b"apx");

/// How many changed values [`LogicDiff`] lists at most.
pub const MAX_VALUES: usize = 1000;

/// How many changed ranges [`byte_diff`] lists at most.
pub const MAX_RANGES: usize = 64;

/// Byte-level comparison of two buffers, position by position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByteDiff {
    pub old_len: usize,
    pub new_len: usize,
    /// Positions inside the common length whose bytes differ, plus the
    /// length difference.
    pub changed_bytes: usize,
    /// The first [`MAX_RANGES`] ranges of differing positions (in the new
    /// data); a length change shows as a range at the end.
    pub ranges: Vec<Range<usize>>,
    /// Whether more ranges exist than are listed.
    pub truncated: bool,
}

impl ByteDiff {
    pub fn is_identical(&self) -> bool {
        self.changed_bytes == 0
    }
}

/// Compares `old` and `new` byte by byte.
pub fn byte_diff(old: &[u8], new: &[u8]) -> ByteDiff {
    let common = old.len().min(new.len());
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut truncated = false;
    let mut changed = 0;
    let mut start = None;
    for i in 0..common {
        let differs = old[i] != new[i];
        changed += differs as usize;
        match (differs, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                push_range(&mut ranges, &mut truncated, s..i);
                start = None;
            }
            _ => {}
        }
    }
    let longest = old.len().max(new.len());
    if let Some(s) = start {
        push_range(&mut ranges, &mut truncated, s..common);
    }
    if longest > common {
        match ranges.last_mut() {
            Some(last) if last.end == common => last.end = longest,
            _ => push_range(&mut ranges, &mut truncated, common..longest),
        }
    }
    ByteDiff {
        old_len: old.len(),
        new_len: new.len(),
        changed_bytes: changed + (longest - common),
        ranges,
        truncated,
    }
}

fn push_range(ranges: &mut Vec<Range<usize>>, truncated: &mut bool, range: Range<usize>) {
    if ranges.len() < MAX_RANGES {
        ranges.push(range);
    } else {
        *truncated = true;
    }
}

/// A type-aware description of a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticDiff {
    Strings(StringTableDiff),
    Texture(TextureDiff),
    /// Changed values of a SimGroup, metamodel or variable dictionary.
    Logic(LogicDiff),
    /// Changed blocks and embedded textures of an `apx` file.
    Apx(ApxDiff),
}

/// Changed values of a game-logic resource.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogicDiff {
    /// Positions of the changed SimGroup objects, properties or variables.
    pub changed: Vec<usize>,
    /// The first [`MAX_VALUES`] changed values.
    pub values: Vec<ValueChange>,
    pub truncated: bool,
    /// Differences that are not value changes, e.g. objects added or removed.
    pub other: Vec<String>,
}

/// One value that differs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueChange {
    /// Where the value is, e.g. `object 12 (Door) CSystemCommands.CMD_LoadMatrix`.
    pub place: String,
    pub old: String,
    pub new: String,
}

impl LogicDiff {
    fn push(&mut self, place: String, old: String, new: String) {
        if self.values.len() < MAX_VALUES {
            self.values.push(ValueChange { place, old, new });
        } else {
            self.truncated = true;
        }
    }
}

/// Changed blocks and embedded textures of an `apx` file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApxDiff {
    /// Blocks other than `alig` whose data differs, added or removed.
    pub blocks: Vec<BlockChange>,
    /// Embedded textures whose header or data differ.
    pub textures: Vec<TextureDiff>,
}

/// One `apx` block that differs; `None` size = the block is not there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockChange {
    pub tag: String,
    /// Which block with this tag: 0 for the first.
    pub ordinal: usize,
    pub old_size: Option<usize>,
    pub new_size: Option<usize>,
}

/// Changed, added and removed entries of a string table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StringTableDiff {
    pub changed: Vec<StringChange>,
    /// Hashes present only in the new table.
    pub added: Vec<u32>,
    /// Hashes present only in the original table.
    pub removed: Vec<u32>,
}

/// One string that differs between the original and the new table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringChange {
    pub hash: u32,
    /// Column index in the new table.
    pub language: usize,
    /// Language code, or `column N` when the table does not name its languages.
    pub language_name: String,
    pub old: String,
    pub new: String,
}

/// Shape of a texture before and after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureDiff {
    pub name: String,
    pub old: TextureShape,
    pub new: TextureShape,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureShape {
    pub width: u32,
    pub height: u32,
    pub format: String,
    pub mips: usize,
}

impl std::fmt::Display for TextureShape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}×{} {}, {} mip(s)",
            self.width, self.height, self.format, self.mips
        )
    }
}

/// Compares the original and new data of a resource of type `type_id`.
/// Returns `None` for types without a semantic comparison or data that does
/// not parse.
pub fn semantic_diff(type_id: u32, old: &[u8], new: &[u8]) -> Option<SemanticDiff> {
    match type_id {
        TYPE_LH2 => {
            let old = StringTable::parse(old).ok()?;
            let new = StringTable::parse(new).ok()?;
            Some(SemanticDiff::Strings(string_table_diff(&old, &new)))
        }
        TYPE_TG4H => {
            let shape = |h: &Tg4Header| TextureShape {
                width: h.width as u32,
                height: h.height as u32,
                format: h.format().name().to_owned(),
                mips: h.mip_count(),
            };
            let old = Tg4Header::parse(old).ok()?;
            let new = Tg4Header::parse(new).ok()?;
            Some(SemanticDiff::Texture(TextureDiff {
                name: new.name.clone(),
                old: shape(&old),
                new: shape(&new),
            }))
        }
        TYPE_SIMGROUP => {
            let old = SimGroup::parse(old).ok()?;
            let new = SimGroup::parse(new).ok()?;
            Some(SemanticDiff::Logic(simgroup_diff(&old, &new)))
        }
        TYPE_METAMODEL => {
            let old = MetaModel::parse(old).ok()?;
            let new = MetaModel::parse(new).ok()?;
            Some(SemanticDiff::Logic(metamodel_diff(&old, &new)))
        }
        TYPE_VARIABLE_DICTIONARY => {
            let old = VariableDictionary::parse(old).ok()?;
            let new = VariableDictionary::parse(new).ok()?;
            Some(SemanticDiff::Logic(variables_diff(&old, &new)))
        }
        TYPE_APX => {
            let old = AptFile::parse(old).ok()?;
            let new = AptFile::parse(new).ok()?;
            Some(SemanticDiff::Apx(apx_diff(&old, &new)))
        }
        _ => None,
    }
}

fn texture_shape(h: &Tg4Header) -> TextureShape {
    TextureShape {
        width: h.width as u32,
        height: h.height as u32,
        format: h.format().name().to_owned(),
        mips: h.mip_count(),
    }
}

/// Compares two versions of a SimGroup object by object.
pub fn simgroup_diff(old: &SimGroup, new: &SimGroup) -> LogicDiff {
    let mut diff = LogicDiff::default();
    let (attrs, other) = simgroup_edit::compare(old, new);
    let label = |i: usize| {
        let object = &new.objects[i];
        let class = object
            .class_name()
            .map_or_else(|| format!("{:08x}", object.class), str::to_owned);
        match new.object_name(object) {
            Some(name) => format!("object {i} ({class} {name:?})"),
            None => format!("object {i} ({class})"),
        }
    };
    for a in &attrs {
        let class = crate::resource::names::sim_classes()
            .get(a.class)
            .map_or_else(|| format!("{:08x}", a.class), str::to_owned);
        let name = a.name.map_or_else(|| a.index.to_string(), str::to_owned);
        diff.push(
            format!("{} {class}.{name}", label(a.object)),
            simgroup_edit::describe(a.old.as_ref()),
            simgroup_edit::describe(a.new.as_ref()),
        );
        diff.changed.push(a.object);
    }
    if old.objects.len() != new.objects.len() {
        diff.other.push(format!(
            "{} object(s) instead of {}",
            new.objects.len(),
            old.objects.len()
        ));
    }
    let common = old.objects.len().min(new.objects.len());
    for i in other.into_iter().filter(|&i| i < common) {
        diff.other.push(format!(
            "{}: class, GUID, links or packets differ",
            label(i)
        ));
        diff.changed.push(i);
    }
    diff.changed.sort_unstable();
    diff.changed.dedup();
    diff
}

/// Compares two versions of a metamodel property by property.
pub fn metamodel_diff(old: &MetaModel, new: &MetaModel) -> LogicDiff {
    let mut diff = LogicDiff::default();
    for (i, (a, b)) in old.properties.iter().zip(&new.properties).enumerate() {
        let (x, y) = (format!("{:?}", a.value), format!("{:?}", b.value));
        if x != y {
            diff.push(format!("property {i} {}.{}", b.class, b.name), x, y);
            diff.changed.push(i);
        }
    }
    for (i, (a, b)) in old.conditions.iter().zip(&new.conditions).enumerate() {
        if a.value != b.value {
            let variable = new
                .variables
                .get(b.variable)
                .map_or("?", |v| v.name.as_str());
            diff.push(
                format!("condition {i} on {variable}"),
                old.condition_value(a).to_string(),
                new.condition_value(b).to_string(),
            );
        }
    }
    let shape = |m: &MetaModel| {
        (
            m.properties.len(),
            m.conditions.len(),
            m.states.len(),
            m.parts.len(),
            m.resources.len(),
        )
    };
    if shape(old) != shape(new) {
        diff.other.push("the tables have other sizes".into());
    }
    diff
}

/// Compares two versions of a variable dictionary variable by variable.
pub fn variables_diff(old: &VariableDictionary, new: &VariableDictionary) -> LogicDiff {
    let mut diff = LogicDiff::default();
    for (i, (a, b)) in old.variables.iter().zip(&new.variables).enumerate() {
        if (a.kind, a.value, &a.name) != (b.kind, b.value, &b.name) {
            diff.push(
                format!("variable {i} {}", b.name),
                format!("kind {}, value {}", a.kind, a.value),
                format!("kind {}, value {}", b.kind, b.value),
            );
            diff.changed.push(i);
        }
    }
    if old.variables.len() != new.variables.len() {
        diff.other.push(format!(
            "{} variable(s) instead of {}",
            new.variables.len(),
            old.variables.len()
        ));
    }
    diff
}

/// Compares two versions of an `apx` file: blocks by tag and position among
/// the blocks with that tag, embedded textures by position.
pub fn apx_diff(old: &AptFile, new: &AptFile) -> ApxDiff {
    let mut diff = ApxDiff::default();
    fn data(f: &AptFile, tag: [u8; 4], ordinal: usize) -> Option<&Vec<u8>> {
        f.blocks
            .iter()
            .filter(|x| x.tag == tag)
            .nth(ordinal)
            .map(|x| &x.data)
    }
    let mut keys: Vec<([u8; 4], usize)> = Vec::new();
    for f in [old, new] {
        let mut counts: Vec<([u8; 4], usize)> = Vec::new();
        for b in f.blocks.iter().filter(|b| b.tag != TAG_ALIGN) {
            let ordinal = counts.iter().filter(|x| x.0 == b.tag).count();
            counts.push((b.tag, ordinal));
            if !keys.contains(&(b.tag, ordinal)) {
                keys.push((b.tag, ordinal));
            }
        }
    }
    for (tag, ordinal) in keys {
        let (x, y) = (data(old, tag, ordinal), data(new, tag, ordinal));
        if x != y {
            diff.blocks.push(BlockChange {
                tag: String::from_utf8_lossy(&tag).into_owned(),
                ordinal,
                old_size: x.map(Vec::len),
                new_size: y.map(Vec::len),
            });
        }
    }
    let old_textures = old.textures().unwrap_or_default();
    let new_textures = new.textures().unwrap_or_default();
    for (a, b) in old_textures.iter().zip(&new_textures) {
        if a.header != b.header || a.data != b.data {
            diff.textures.push(TextureDiff {
                name: b.header.name.clone(),
                old: texture_shape(&a.header),
                new: texture_shape(&b.header),
            });
        }
    }
    diff
}

/// Compares two string tables entry by entry, matching entries by hash and
/// columns by position.
pub fn string_table_diff(old: &StringTable, new: &StringTable) -> StringTableDiff {
    let mut diff = StringTableDiff::default();
    for (row, &hash) in new.hashes.iter().enumerate() {
        let Some(old_row) = old.find(hash) else {
            diff.added.push(hash);
            continue;
        };
        for (language, column) in new.strings.iter().enumerate() {
            let old_text = old
                .strings
                .get(language)
                .and_then(|c| c.get(old_row))
                .map_or("", String::as_str);
            let new_text = column.get(row).map_or("", String::as_str);
            if old_text != new_text {
                diff.changed.push(StringChange {
                    hash,
                    language,
                    language_name: new
                        .languages
                        .get(language)
                        .cloned()
                        .unwrap_or_else(|| format!("column {language}")),
                    old: old_text.to_owned(),
                    new: new_text.to_owned(),
                });
            }
        }
    }
    diff.removed = old
        .hashes
        .iter()
        .copied()
        .filter(|&h| new.find(h).is_none())
        .collect();
    diff
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_diff_lists_ranges() {
        let d = byte_diff(b"abcdef", b"abXdeY");
        assert_eq!(d.changed_bytes, 2);
        assert_eq!(d.ranges, [2..3, 5..6]);
        assert!(!d.truncated);
        let d = byte_diff(b"abc", b"abcdef");
        assert_eq!(
            (d.changed_bytes, d.ranges.len(), d.ranges[0].clone()),
            (3, 1, 3..6)
        );
        let d = byte_diff(b"abcdef", b"abX");
        assert_eq!(
            (d.changed_bytes, d.ranges.len(), d.ranges[0].clone()),
            (4, 1, 2..6)
        );
        assert!(byte_diff(b"same", b"same").is_identical());
        let old = vec![0u8; 1000];
        let new: Vec<u8> = (0..1000).map(|i| (i % 2) as u8).collect();
        let d = byte_diff(&old, &new);
        assert_eq!(d.changed_bytes, 500);
        assert_eq!(d.ranges.len(), MAX_RANGES);
        assert!(d.truncated);
    }

    #[test]
    fn string_tables_compare_by_hash() {
        let old = StringTable {
            version: 1,
            flags: 0x4000,
            languages: vec!["en".into()],
            hashes: vec![1, 2, 3],
            strings: vec![vec!["a".into(), "b".into(), "c".into()]],
        };
        let mut new = old.clone();
        new.strings[0][1] = "B".into();
        let bytes = |t: &StringTable| t.to_bytes().unwrap();
        let Some(SemanticDiff::Strings(diff)) = semantic_diff(TYPE_LH2, &bytes(&old), &bytes(&new))
        else {
            panic!("string diff expected");
        };
        assert_eq!(diff.changed.len(), 1);
        let change = &diff.changed[0];
        assert_eq!(
            (
                change.hash,
                change.language_name.as_str(),
                &*change.old,
                &*change.new
            ),
            (2, "en", "b", "B")
        );
        new.hashes = vec![1, 2, 4];
        let diff = string_table_diff(&old, &new);
        assert_eq!((diff.added, diff.removed), (vec![4], vec![3]));
        assert!(semantic_diff(TYPE_LH2, b"junk", b"junk").is_none());
        assert!(semantic_diff(0, b"a", b"b").is_none());
    }
}
