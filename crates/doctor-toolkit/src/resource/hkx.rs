//! `hkx` resources: Havok 5.5.0 binary packfiles.
//!
//! In The Godfather II every `hkx` holds physics only: an `hkRootLevelContainer`
//! with `hkpPhysicsData` (rigid bodies, collision shapes, MOPP trees) and EA's
//! `EAMaterialArray`. There are no skeletons or animations (`hkaSkeleton`,
//! `hkaAnimation`); rigs are in `rcb` resources (see [`crate::resource::rcb`]).
//!
//! ```text
//! 0x00  u32  magic 0x57E0E057, u32 magic 0x10C0C010
//! 0x08  i32  user tag
//! 0x0C  i32  file version, 5
//! 0x10  u8×4 layout: pointer size 4, little-endian 1, reuse padding 0, empty base 1
//! 0x14  i32  section count, 3 (__classnames__, __types__, __data__)
//! 0x18  i32  contents section, i32 offset; class name section, i32 offset
//! 0x28  char[16] "Havok-5.5.0-r1"
//! 0x38  i32  flags, i32 pad (both -1)
//! 0x40  sections, 0x30 bytes each:
//!       char[19] tag + 0xFF, u32 data start, then offsets relative to it:
//!       local fixups, global fixups, virtual fixups, exports, imports, end
//! ```
//!
//! The class name section lists `{u32 signature, u8 0x09, name, NUL}`. Virtual
//! fixups `{u32 object offset, u32 section, u32 class name offset}` give the class
//! of every object; local fixups `{src, dst}` and global fixups
//! `{src, section, dst}` resolve pointers. Every table ends with `0xFF` bytes up
//! to a multiple of 16.
//!
//! Layers on top of [`Packfile`]:
//!
//! - [`graph`]: the data section as objects and data blocks joined by pointers,
//!   laid out again the way the Havok writer does;
//! - [`physics`]: rigid bodies, collision shapes and EA materials as typed data,
//!   read and written;
//! - [`mopp`]: the MOPP bounding-volume code: the game's query machine and a
//!   code generator for new meshes;
//! - [`collision`]: collision for new objects built from triangle meshes;
//! - [`geometry`]: shapes as triangles for viewing and export.

pub mod collision;
pub mod geometry;
pub mod graph;
pub mod mopp;
pub mod physics;

use serde::Serialize;

use crate::io::{self, Endian};
use crate::{Error, Result};

/// Type id of `hkx` resources.
pub const TYPE_HKX: u32 = crate::hash::sdbm(b"hkx");

/// Type id that most metamodels write in their references to collision
/// `hkx` resources (`CollisionHavok4Part.CollisionModel`): `hka`
/// (`0x33A1671E`); others write [`TYPE_HKX`], the type of the resources
/// themselves.
pub const TYPE_HKX_REFERENCE: u32 = crate::hash::sdbm(b"hka");

const MAGIC: [u32; 2] = [0x57E0_E057, 0x10C0_C010];
const HEADER_SIZE: usize = 0x40;
const SECTION_SIZE: usize = 0x30;
const MAX_SECTIONS: usize = 16;
const TABLE_ALIGN: usize = 16;
const END: u32 = u32::MAX;

/// One section of a packfile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Section {
    pub tag: String,
    pub start: u32,
    /// Offsets relative to `start`.
    pub local_fixups: u32,
    pub global_fixups: u32,
    pub virtual_fixups: u32,
    pub exports: u32,
    pub imports: u32,
    pub end: u32,
}

/// An object found through the virtual fixups.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Object {
    pub section: usize,
    pub offset: u32,
    pub class: String,
}

/// The contents of one section: its data and its fixup tables.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SectionBody {
    /// The 20 bytes of the tag field as stored (NUL-padded name, `0xFF`).
    pub tag_field: [u8; 20],
    pub data: Vec<u8>,
    /// `(source, destination)` offsets in this section. In all three tables
    /// entries whose source is `0xFFFFFFFF` are unused: the Havok writer leaves
    /// some at the end of the global table.
    pub local_fixups: Vec<(u32, u32)>,
    /// `(source, target section, target offset)`.
    pub global_fixups: Vec<(u32, u32, u32)>,
    /// `(object offset, class name section, class name offset)`.
    pub virtual_fixups: Vec<(u32, u32, u32)>,
    pub exports: Vec<u8>,
    pub imports: Vec<u8>,
}

impl SectionBody {
    /// An empty section named `tag`.
    pub fn named(tag: &str) -> Self {
        let mut tag_field = [0u8; 20];
        let len = tag.len().min(19);
        tag_field[..len].copy_from_slice(&tag.as_bytes()[..len]);
        tag_field[19] = 0xFF;
        Self {
            tag_field,
            ..Self::default()
        }
    }

    fn tag(&self) -> String {
        let end = self.tag_field[..19]
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(19);
        String::from_utf8_lossy(&self.tag_field[..end]).into_owned()
    }
}

/// A parsed packfile: header, section contents and the object list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Packfile {
    pub user_tag: i32,
    pub file_version: i32,
    pub layout: [u8; 4],
    pub contents_version: String,
    pub root_class: String,
    pub sections: Vec<Section>,
    /// Classes declared in the class name section, with their signatures.
    pub classes: Vec<(u32, String)>,
    pub objects: Vec<Object>,
    /// `(section, offset)` of the root object.
    #[serde(skip)]
    pub contents: (u32, u32),
    /// `(section, offset)` of the root class name.
    #[serde(skip)]
    pub contents_class_name: (u32, u32),
    /// The 16 bytes of the version field as stored.
    #[serde(skip)]
    pub version_field: [u8; 16],
    #[serde(skip)]
    pub flags: [u32; 2],
    #[serde(skip)]
    pub bodies: Vec<SectionBody>,
}

fn read_table<const N: usize>(data: &[u8], from: usize, to: usize) -> Result<Vec<[u32; N]>> {
    let le = Endian::Little;
    let mut out = Vec::new();
    let mut at = from;
    while at + 4 * N <= to {
        let mut entry = [0u32; N];
        for (k, value) in entry.iter_mut().enumerate() {
            *value = io::read_u32(data, at + 4 * k, le)?;
        }
        out.push(entry);
        at += 4 * N;
    }
    Ok(out)
}

fn pad_table(out: &mut Vec<u8>, start: usize) {
    while !(out.len() - start).is_multiple_of(TABLE_ALIGN) {
        out.push(0xFF);
    }
}

impl Packfile {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let le = Endian::Little;
        let u = |o: usize| io::read_u32(data, o, le);
        if [u(0)?, u(4)?] != MAGIC {
            return Err(Error::BadMagic {
                what: "hkx",
                found: u(0)?,
            });
        }
        let user_tag = u(0x08)? as i32;
        let file_version = u(0x0C)? as i32;
        let layout: [u8; 4] = io::slice(data, 0x10, 4)?.try_into().expect("4 bytes");
        if layout[0] != 4 || layout[1] != 1 {
            return Err(Error::malformed(
                "hkx",
                0x10,
                "not a 32-bit little-endian packfile",
            ));
        }
        let count = u(0x14)? as usize;
        if count > MAX_SECTIONS {
            return Err(Error::malformed("hkx", 0x14, format!("{count} sections")));
        }
        let version_field: [u8; 16] = io::slice(data, 0x28, 16)?.try_into().expect("16 bytes");
        let end = version_field.iter().position(|&b| b == 0).unwrap_or(16);
        let contents_version = String::from_utf8_lossy(&version_field[..end]).into_owned();
        let mut sections = Vec::with_capacity(count);
        let mut bodies = Vec::with_capacity(count);
        for i in 0..count {
            let at = HEADER_SIZE + SECTION_SIZE * i;
            let tag_field: [u8; 20] = io::slice(data, at, 20)?.try_into().expect("20 bytes");
            let f = |k: usize| u(at + 20 + 4 * k);
            let mut body = SectionBody {
                tag_field,
                ..SectionBody::default()
            };
            let section = Section {
                tag: body.tag(),
                start: f(0)?,
                local_fixups: f(1)?,
                global_fixups: f(2)?,
                virtual_fixups: f(3)?,
                exports: f(4)?,
                imports: f(5)?,
                end: f(6)?,
            };
            let offsets = [
                section.local_fixups,
                section.global_fixups,
                section.virtual_fixups,
                section.exports,
                section.imports,
                section.end,
            ];
            if offsets.windows(2).any(|w| w[0] > w[1]) {
                return Err(Error::malformed("hkx", at, "section offsets out of order"));
            }
            let base = section.start as usize;
            let raw = io::slice(data, base, section.end as usize)?;
            let at_off = |o: u32| base + o as usize;
            body.data = raw[..section.local_fixups as usize].to_vec();
            body.local_fixups = read_table::<2>(
                data,
                at_off(section.local_fixups),
                at_off(section.global_fixups),
            )?
            .into_iter()
            .map(|[a, b]| (a, b))
            .collect();
            body.global_fixups = read_table::<3>(
                data,
                at_off(section.global_fixups),
                at_off(section.virtual_fixups),
            )?
            .into_iter()
            .map(|[a, b, c]| (a, b, c))
            .collect();
            body.virtual_fixups = read_table::<3>(
                data,
                at_off(section.virtual_fixups),
                at_off(section.exports),
            )?
            .into_iter()
            .map(|[a, b, c]| (a, b, c))
            .collect();
            body.exports = raw[section.exports as usize..section.imports as usize].to_vec();
            body.imports = raw[section.imports as usize..section.end as usize].to_vec();
            sections.push(section);
            bodies.push(body);
        }
        let contents = (u(0x18)?, u(0x1C)?);
        let contents_class_name = (u(0x20)?, u(0x24)?);
        let flags = [u(0x38)?, u(0x3C)?];
        let mut packfile = Self {
            user_tag,
            file_version,
            layout,
            contents_version,
            root_class: String::new(),
            sections,
            classes: Vec::new(),
            objects: Vec::new(),
            contents,
            contents_class_name,
            version_field,
            flags,
            bodies,
        };
        packfile.root_class = packfile
            .class_name(contents_class_name.0, contents_class_name.1)?
            .to_owned();
        packfile.classes = packfile.read_class_list();
        let mut objects = Vec::new();
        for (index, body) in packfile.bodies.iter().enumerate() {
            for &(offset, section, name) in body.virtual_fixups.iter().filter(|v| v.0 != END) {
                objects.push(Object {
                    section: index,
                    offset,
                    class: packfile.class_name(section, name)?.to_owned(),
                });
            }
        }
        packfile.objects = objects;
        Ok(packfile)
    }

    /// The class name at `offset` in `section`.
    pub fn class_name(&self, section: u32, offset: u32) -> Result<&str> {
        let body = self.bodies.get(section as usize).ok_or_else(|| {
            Error::malformed("hkx", 0, format!("section {section} does not exist"))
        })?;
        if offset as usize >= body.data.len() {
            return Err(Error::malformed("hkx", 0, "class name outside its section"));
        }
        let name = io::cstr(&body.data, offset as usize)?;
        std::str::from_utf8(name).map_err(|_| Error::malformed("hkx", 0, "class name not UTF-8"))
    }

    fn read_class_list(&self) -> Vec<(u32, String)> {
        let Some(names) = self.bodies.iter().find(|b| b.tag() == "__classnames__") else {
            return Vec::new();
        };
        parse_class_names(&names.data)
            .into_iter()
            .map(|(signature, name, _)| (signature, name))
            .collect()
    }

    /// Index of the section tagged `tag`.
    pub fn section_index(&self, tag: &str) -> Option<usize> {
        self.bodies.iter().position(|b| b.tag() == tag)
    }

    /// Serialises the packfile. Parsing and writing back a game file yields the same bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let le = Endian::Little;
        let mut out = Vec::new();
        for value in [
            MAGIC[0],
            MAGIC[1],
            self.user_tag as u32,
            self.file_version as u32,
        ] {
            io::put_u32(&mut out, value, le);
        }
        out.extend_from_slice(&self.layout);
        io::put_u32(&mut out, self.bodies.len() as u32, le);
        for value in [
            self.contents.0,
            self.contents.1,
            self.contents_class_name.0,
            self.contents_class_name.1,
        ] {
            io::put_u32(&mut out, value, le);
        }
        out.extend_from_slice(&self.version_field);
        io::put_u32(&mut out, self.flags[0], le);
        io::put_u32(&mut out, self.flags[1], le);
        let table_at = out.len();
        out.resize(table_at + SECTION_SIZE * self.bodies.len(), 0);
        for (i, body) in self.bodies.iter().enumerate() {
            let start = out.len();
            out.extend_from_slice(&body.data);
            let local = out.len() - start;
            for &(a, b) in &body.local_fixups {
                io::put_u32(&mut out, a, le);
                io::put_u32(&mut out, b, le);
            }
            pad_table(&mut out, start);
            let global = out.len() - start;
            for &(a, b, c) in &body.global_fixups {
                for v in [a, b, c] {
                    io::put_u32(&mut out, v, le);
                }
            }
            pad_table(&mut out, start);
            let virtuals = out.len() - start;
            for &(a, b, c) in &body.virtual_fixups {
                for v in [a, b, c] {
                    io::put_u32(&mut out, v, le);
                }
            }
            pad_table(&mut out, start);
            let exports = out.len() - start;
            out.extend_from_slice(&body.exports);
            let imports = out.len() - start;
            out.extend_from_slice(&body.imports);
            let end = out.len() - start;
            let at = table_at + SECTION_SIZE * i;
            out[at..at + 20].copy_from_slice(&body.tag_field);
            for (k, v) in [start, local, global, virtuals, exports, imports, end]
                .into_iter()
                .enumerate()
            {
                out[at + 20 + 4 * k..at + 24 + 4 * k].copy_from_slice(&(v as u32).to_le_bytes());
            }
        }
        out
    }
}

/// Parses a class name section into `(signature, name, offset of the name)`.
pub fn parse_class_names(data: &[u8]) -> Vec<(u32, String, u32)> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 5 < data.len() {
        let signature = u32::from_le_bytes(data[at..at + 4].try_into().expect("4 bytes"));
        if signature == END || data[at + 4] != 0x09 {
            break;
        }
        let Ok(name) = io::cstr(data, at + 5) else {
            break;
        };
        out.push((
            signature,
            String::from_utf8_lossy(name).into_owned(),
            (at + 5) as u32,
        ));
        at += 5 + name.len() + 1;
    }
    out
}

/// Builds a class name section from `(signature, name)` pairs; returns the
/// section data and the offset of each name.
pub fn build_class_names(classes: &[(u32, &str)]) -> (Vec<u8>, Vec<u32>) {
    let mut data = Vec::new();
    let mut offsets = Vec::with_capacity(classes.len());
    for &(signature, name) in classes {
        data.extend_from_slice(&signature.to_le_bytes());
        data.push(0x09);
        offsets.push(data.len() as u32);
        data.extend_from_slice(name.as_bytes());
        data.push(0);
    }
    pad_table(&mut data, 0);
    (data, offsets)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let (names, offsets) = build_class_names(&[(0x1234, "hkRoot")]);
        let mut data_section = SectionBody::named("__data__");
        data_section.data = vec![0; 16];
        data_section.virtual_fixups = vec![(0, 0, offsets[0])];
        let mut class_section = SectionBody::named("__classnames__");
        class_section.data = names;
        let packfile = Packfile {
            user_tag: 0,
            file_version: 5,
            layout: [4, 1, 0, 1],
            contents_version: String::new(),
            root_class: String::new(),
            sections: Vec::new(),
            classes: Vec::new(),
            objects: Vec::new(),
            contents: (1, 0),
            contents_class_name: (0, offsets[0]),
            version_field: *b"Havok-5.5.0-r1\0\xFF",
            flags: [END, END],
            bodies: vec![class_section, data_section],
        };
        packfile.to_bytes()
    }

    #[test]
    fn parses_header_and_objects() {
        let bytes = sample();
        let p = Packfile::parse(&bytes).unwrap();
        assert_eq!(p.contents_version, "Havok-5.5.0-r1");
        assert_eq!(p.root_class, "hkRoot");
        assert_eq!(p.classes, vec![(0x1234, "hkRoot".to_owned())]);
        assert_eq!(p.objects.len(), 1);
        assert_eq!(p.objects[0].class, "hkRoot");
        assert_eq!(p.to_bytes(), bytes);
        // Tables are padded with 0xFF to 16 bytes.
        let data = &p.sections[1];
        assert_eq!(data.virtual_fixups, 16);
        assert_eq!(data.exports, 32);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(Packfile::parse(&[]).is_err());
        let bytes = sample();
        for len in 0..bytes.len() {
            let _ = Packfile::parse(&bytes[..len]);
        }
        let mut d = bytes.clone();
        d[0x14] = 99;
        assert!(Packfile::parse(&d).is_err());
        let mut d = bytes.clone();
        d[0x11] = 0; // big-endian
        assert!(Packfile::parse(&d).is_err());
        // Class name in a missing section.
        let p = Packfile::parse(&bytes).unwrap();
        let at = p.sections[1].start as usize + p.sections[1].virtual_fixups as usize + 4;
        let mut d = bytes.clone();
        d[at] = 7;
        assert!(Packfile::parse(&d).is_err());
    }
}
