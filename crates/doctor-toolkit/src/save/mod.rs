//! Save files of The Godfather II (PC 1.0.789.0): `GF2-EN-autosave` and
//! `GF2-EN-<digits>` in the profile folder. Read-only: the toolkit shows a
//! save and never changes the player's files.
//!
//! # Layout
//!
//! Little-endian; a save is always 272 740 bytes (`docs/formats/save.md`).
//!
//! ```text
//! 0x0000  file header, 0x2028 bytes: "RGMH", u32 1, u32 0x2028, 28 zero
//!         bytes, then four UTF-16 strings of 0x800 bytes: game title,
//!         profile name, city, play time (exe 0x00ACD8A0)
//! 0x2028  name block, 0x80C bytes: "NMSD", u32 1, u32 0x80C, UTF-16 name
//!         shown in the list of saves (exe 0x00ACFFF0, 0x00AD0070)
//! 0x2834  container header, 0x1C bytes: u32 0x4D433032 ("20CM" on disk),
//!         u32 size of the container, u32 size of block 1, u32 size of
//!         block 2, u32 checksum of block 1, u32 checksum of block 2,
//!         u32 checksum of the 0x18 bytes above (exe 0x00AD1330, 0x00AD2540)
//! 0x2850  block 1, 0x114 bytes: u32 6, u32 play time in seconds,
//!         u32 capacity of block 2, u32 bytes used in block 2, zeros
//! 0x2964  block 2, 0x40000 bytes: u32 mask, then chunks
//!         { u32 id, u32 0, u32 size, data padded to 4 bytes } up to the
//!         used size (exe 0x004C03C0)
//! ```
//!
//! A chunk belongs to one "persistable" object of the game; its id is a
//! constant of the code. [`CHUNKS`] lists the 25 ids found in saves.

pub mod view;

use std::path::Path;

use serde::Serialize;

use crate::io::{self, Endian};
use crate::resource::simgroup::{Guid, Hash32};
use crate::{Error, Result};

const WHAT: &str = "save";
const LE: Endian = Endian::Little;

/// Size of the file header.
pub const FILE_HEADER_SIZE: usize = 0x2028;
/// Size of the name block.
pub const NAME_BLOCK_SIZE: usize = 0x80C;
/// Size of the container header.
pub const CONTAINER_HEADER_SIZE: usize = 0x1C;
/// Size of block 1 in every known save.
pub const INFO_SIZE: usize = 0x114;
/// Size of block 2 in every known save.
pub const DATA_CAPACITY: usize = 0x40000;
/// Size of a save file.
pub const FILE_SIZE: usize =
    FILE_HEADER_SIZE + NAME_BLOCK_SIZE + CONTAINER_HEADER_SIZE + INFO_SIZE + DATA_CAPACITY;

const FILE_MAGIC: u32 = u32::from_le_bytes(*b"RGMH");
const NAME_MAGIC: u32 = u32::from_le_bytes(*b"NMSD");
const CONTAINER_MAGIC: u32 = 0x4D43_3032;
const STRING_SIZE: usize = 0x800;
const TITLE: usize = 0x28;
const PROFILE: usize = 0x828;
const CITY: usize = 0x1028;
const PLAY_TIME: usize = 0x1828;
const CONTAINER: usize = FILE_HEADER_SIZE + NAME_BLOCK_SIZE;
const INFO: usize = CONTAINER + CONTAINER_HEADER_SIZE;

/// Value of the player's family in the money field of the strategy chunk:
/// the money itself is in the chunk of the player's family (exe 0x008C1850).
pub const MONEY_ELSEWHERE: f32 = -1_000_000.0;
const END_MARK: u32 = 0xABBA;
/// Hash of the player's family, `sdbm("corleone")`.
pub const CORLEONE: u32 = 0x0637_B907;

/// The checksum of the container (exe 0x00D373D0): CRC-32 with the polynomial
/// 0x04C11DB7, most significant bit first, where the first four bytes are the
/// inverted start value. Data shorter than four bytes gives 0.
pub fn checksum(data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = (i as u32) << 24;
            let mut k = 0;
            while k < 8 {
                c = if c & 0x8000_0000 != 0 {
                    (c << 1) ^ 0x04C1_1DB7
                } else {
                    c << 1
                };
                k += 1;
            }
            table[i] = c;
            i += 1;
        }
        table
    };
    let Some((head, rest)) = data.split_first_chunk::<4>() else {
        return 0;
    };
    let mut crc = !u32::from_be_bytes(*head);
    for &byte in rest {
        crc = (crc << 8 | u32::from(byte)) ^ TABLE[(crc >> 24) as usize];
    }
    !crc
}

/// A stored checksum and the one computed from the data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Check {
    pub stored: Hash32,
    pub computed: Hash32,
}

impl Check {
    pub fn ok(&self) -> bool {
        self.stored == self.computed
    }
}

/// The three checksums of the container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Checksums {
    pub header: Check,
    pub info: Check,
    pub data: Check,
}

impl Checksums {
    pub fn ok(&self) -> bool {
        self.header.ok() && self.info.ok() && self.data.ok()
    }
}

/// What the game knows about a chunk id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ChunkInfo {
    pub id: u32,
    /// Short name given by the toolkit.
    pub name: &'static str,
    /// What the chunk holds, and the function of `godfather2.exe` 1.0.789.0
    /// that registers its owner.
    pub about: &'static str,
}

/// The chunks of a save, in the order the game writes them.
pub const CHUNKS: &[ChunkInfo] = &[
    chunk(
        0x87E0_3A5D,
        "mobface",
        "the player's face and body (MobFace, 0x009BE6A0); the id is sdbm(\"mobface\")",
    ),
    chunk(
        0xC14C_90E8,
        "root",
        "empty in every known save (0x00839D40)",
    ),
    chunk(
        0x584F_55BF,
        "c584f55bf",
        "48 bytes, the same in every known save (0x0090CCA0)",
    ),
    chunk(
        0x2531_31B5,
        "player-family",
        "the player's money and family members (0x008BCA30)",
    ),
    chunk(
        0xE60B_027D,
        "variables",
        "story variables with the save flag (0x00441E60)",
    ),
    chunk(
        0x23AE_A276,
        "score",
        "score and accomplishments (0x00893BD0)",
    ),
    chunk(
        0x964A_0392,
        "c964a0392",
        "a list of { hash, flag, flag } (0x00813E20)",
    ),
    chunk(0xA889_6ED8, "ca8896ed8", "five numbers (0x00826730)"),
    chunk(
        0x32B7_D466,
        "buildings",
        "businesses and other buildings: owner, state, guards (0x006B01E0)",
    ),
    chunk(
        0x874F_7077,
        "cities",
        "the state of the three cities (0x00849910)",
    ),
    chunk(
        0xC8ED_3B85,
        "jobs",
        "the generator of jobs and favours (0x008E5FC0)",
    ),
    chunk(
        0xE655_30E4,
        "ce65530e4",
        "records with object GUIDs; the owner is not found",
    ),
    chunk(
        0x38EF_E5A9,
        "c38efe5a9",
        "one number; the owner is not found",
    ),
    chunk(
        0x0B60_4DD4,
        "families",
        "the strategy game: families, their money, buildings and members (0x008C6F70)",
    ),
    chunk(
        0x73A2_895D,
        "c73a2895d",
        "a mask and object GUIDs (0x00897DE0)",
    ),
    chunk(
        0x9AB6_7F60,
        "safehouses",
        "the state of 25 safehouses (0x008DD770)",
    ),
    chunk(
        0x2592_0C72,
        "first-success",
        "bits of the hints \"first success with ...\" (0x006F85C0)",
    ),
    chunk(
        0xDED5_1537,
        "cded51537",
        "1040 bytes of counters (0x007E90D0)",
    ),
    chunk(
        0x8D94_6A1B,
        "c8d946a1b",
        "records { number, object GUID } (0x008B7CD0)",
    ),
    chunk(
        0x7B80_C13A,
        "player",
        "two numbers of the player and a list of hashes (0x007AABE0)",
    ),
    chunk(
        0x460A_9E80,
        "history",
        "the history log of the pause menu (0x00938820)",
    ),
    chunk(
        0xC914_145C,
        "cc914145c",
        "a version and one mask (0x00922D90)",
    ),
    chunk(
        0x5760_0D39,
        "c57600d39",
        "40 records { object GUID, number, number } (0x00891D40)",
    ),
    chunk(
        0x04AB_A815,
        "checkpoint",
        "the current checkpoint and the player's transform (0x008FCCC0)",
    ),
    chunk(
        0x5BEE_A32A,
        "inventory",
        "the player's weapons and ammunition (0x00704D70)",
    ),
];

const fn chunk(id: u32, name: &'static str, about: &'static str) -> ChunkInfo {
    ChunkInfo { id, name, about }
}

/// The description of a chunk id, when the id is known.
pub fn chunk_info(id: u32) -> Option<&'static ChunkInfo> {
    CHUNKS.iter().find(|c| c.id == id)
}

/// One chunk of block 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub id: u32,
    /// Offset of the chunk header in the file.
    pub offset: usize,
    pub data: Vec<u8>,
}

impl Chunk {
    /// The toolkit's name of the chunk: a known name or `c<id>`.
    pub fn name(&self) -> String {
        match chunk_info(self.id) {
            Some(info) => info.name.to_owned(),
            None => format!("c{:08x}", self.id),
        }
    }

    /// Decodes the chunk as far as its layout is known.
    pub fn content(&self) -> Result<Content> {
        let d = &self.data[..];
        Ok(match self.id {
            0xE60B_027D => Content::Variables(variables(d)?),
            0x2531_31B5 => Content::PlayerFamily(player_family(d)?),
            0x0B60_4DD4 => Content::Families(families(d)?),
            0x32B7_D466 => Content::Buildings(buildings(d)?),
            0x874F_7077 => Content::Cities(cities(d)?),
            0x460A_9E80 => Content::History(history(d)?),
            0x5BEE_A32A => Content::Inventory(inventory(d)?),
            0x7B80_C13A => Content::Player(player(d)?),
            0x04AB_A815 => Content::Checkpoint(checkpoint(d)?),
            0x9AB6_7F60 => Content::Safehouses(safehouses(d)?),
            _ => Content::Raw,
        })
    }
}

/// A parsed save file.
#[derive(Debug, Clone, PartialEq)]
pub struct SaveFile {
    /// Game title of the file header.
    pub title: String,
    pub profile: String,
    pub city: String,
    /// Play time as the game wrote it, `h:mm:ss`.
    pub play_time: String,
    /// Name in the list of saves.
    pub description: String,
    /// First number of block 1; 6 in every known save.
    pub version: u32,
    pub play_seconds: u32,
    pub capacity: u32,
    /// Bytes of block 2 in use.
    pub used: u32,
    pub checksums: Checksums,
    /// First number of block 2: the mask of the saved objects (0xB in every
    /// known save).
    pub mask: u32,
    pub chunks: Vec<Chunk>,
}

impl SaveFile {
    /// Reads and parses a save file.
    pub fn open(path: &Path) -> Result<Self> {
        let data = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        Self::parse(&data)
    }

    pub fn parse(data: &[u8]) -> Result<Self> {
        expect(data, 0, FILE_MAGIC, "file magic")?;
        expect(data, 4, 1, "file header version")?;
        expect(data, 8, FILE_HEADER_SIZE as u32, "file header size")?;
        expect(data, FILE_HEADER_SIZE, NAME_MAGIC, "name block magic")?;
        expect(data, FILE_HEADER_SIZE + 4, 1, "name block version")?;
        expect(
            data,
            FILE_HEADER_SIZE + 8,
            NAME_BLOCK_SIZE as u32,
            "name block size",
        )?;
        expect(data, CONTAINER, CONTAINER_MAGIC, "container magic")?;

        let total = u32_at(data, CONTAINER + 4)? as usize;
        let info_size = u32_at(data, CONTAINER + 8)? as usize;
        let data_size = u32_at(data, CONTAINER + 12)? as usize;
        if info_size
            .checked_add(data_size)
            .and_then(|n| n.checked_add(CONTAINER_HEADER_SIZE))
            != Some(total)
        {
            return Err(Error::malformed(
                WHAT,
                CONTAINER + 4,
                "the sizes of the container disagree",
            ));
        }
        if info_size < 16 {
            return Err(Error::malformed(
                WHAT,
                CONTAINER + 8,
                "block 1 is shorter than 16 bytes",
            ));
        }
        let info = io::slice(data, INFO, info_size)?;
        let block = io::slice(data, INFO + info_size, data_size)?;
        let checksums = Checksums {
            header: Check {
                stored: Hash32(u32_at(data, CONTAINER + 24)?),
                computed: Hash32(checksum(io::slice(data, CONTAINER, 24)?)),
            },
            info: Check {
                stored: Hash32(u32_at(data, CONTAINER + 16)?),
                computed: Hash32(checksum(info)),
            },
            data: Check {
                stored: Hash32(u32_at(data, CONTAINER + 20)?),
                computed: Hash32(checksum(block)),
            },
        };

        let used = u32_at(info, 12)?;
        let base = INFO + info_size;
        if used as usize > block.len() || (used != 0 && used < 4) {
            return Err(Error::malformed(
                WHAT,
                INFO + 12,
                format!("used size {used:#x} is out of range"),
            ));
        }
        let mut chunks = Vec::new();
        let mut at = 4usize;
        while at + 12 <= used as usize {
            let id = u32_at(block, at)?;
            let size = u32_at(block, at + 8)? as usize;
            let body = io::slice(block, at + 12, size)
                .ok()
                .filter(|_| at + 12 + size <= used as usize)
                .ok_or_else(|| {
                    Error::malformed(
                        WHAT,
                        base + at,
                        format!("chunk {id:08x} of {size:#x} bytes runs past the used size"),
                    )
                })?;
            chunks.push(Chunk {
                id,
                offset: base + at,
                data: body.to_vec(),
            });
            at += 12 + size.next_multiple_of(4);
        }

        Ok(Self {
            title: utf16(data, TITLE)?,
            profile: utf16(data, PROFILE)?,
            city: utf16(data, CITY)?,
            play_time: utf16(data, PLAY_TIME)?,
            description: utf16(data, FILE_HEADER_SIZE + 12)?,
            version: u32_at(info, 0)?,
            play_seconds: u32_at(info, 4)?,
            capacity: u32_at(info, 8)?,
            used,
            checksums,
            mask: if used >= 4 { u32_at(block, 0)? } else { 0 },
            chunks,
        })
    }

    /// The chunk with this id.
    pub fn chunk(&self, id: u32) -> Option<&Chunk> {
        self.chunks.iter().find(|c| c.id == id)
    }

    /// The chunk with this name ([`Chunk::name`]) or hexadecimal id.
    pub fn chunk_named(&self, name: &str) -> Option<&Chunk> {
        let id = u32::from_str_radix(name.trim_start_matches("0x"), 16).ok();
        self.chunks
            .iter()
            .find(|c| c.name() == name || Some(c.id) == id)
    }

    /// The saved story variables; empty when the chunk is absent.
    pub fn variables(&self) -> Result<Vec<SavedVariable>> {
        self.chunk(0xE60B_027D)
            .map_or(Ok(Vec::new()), |c| variables(&c.data))
    }

    /// The player's money, when the chunk of the player's family is present.
    pub fn money(&self) -> Result<Option<f32>> {
        self.chunk(0x2531_31B5)
            .map(|c| player_family(&c.data).map(|f| f.money))
            .transpose()
    }
}

/// Builds a save file from chunks, as the game lays it out. For tests and
/// synthetic samples: the toolkit does not write saves of the player.
pub fn build(
    profile: &str,
    city: &str,
    play_seconds: u32,
    mask: u32,
    chunks: &[(u32, Vec<u8>)],
) -> Result<Vec<u8>> {
    let time = format!(
        "{}:{:02}:{:02}",
        play_seconds / 3600,
        play_seconds / 60 % 60,
        play_seconds % 60
    );
    let mut out = vec![0u8; FILE_SIZE];
    put(&mut out, 0, FILE_MAGIC);
    put(&mut out, 4, 1);
    put(&mut out, 8, FILE_HEADER_SIZE as u32);
    put_utf16(&mut out, TITLE, "The Godfather\u{2122} II")?;
    put_utf16(&mut out, PROFILE, profile)?;
    put_utf16(&mut out, CITY, city)?;
    put_utf16(&mut out, PLAY_TIME, &time)?;
    put(&mut out, FILE_HEADER_SIZE, NAME_MAGIC);
    put(&mut out, FILE_HEADER_SIZE + 4, 1);
    put(&mut out, FILE_HEADER_SIZE + 8, NAME_BLOCK_SIZE as u32);
    put_utf16(
        &mut out,
        FILE_HEADER_SIZE + 12,
        &format!("{profile} - {city} - {time}"),
    )?;

    let mut block = Vec::new();
    io::put_u32(&mut block, mask, LE);
    for (id, data) in chunks {
        io::put_u32(&mut block, *id, LE);
        io::put_u32(&mut block, 0, LE);
        io::put_u32(&mut block, data.len() as u32, LE);
        block.extend_from_slice(data);
        block.resize(block.len().next_multiple_of(4), 0);
    }
    if block.len() > DATA_CAPACITY {
        return Err(Error::InvalidArgument(format!(
            "the chunks take {:#x} bytes; a save holds {DATA_CAPACITY:#x}",
            block.len()
        )));
    }
    let base = INFO + INFO_SIZE;
    out[base..base + block.len()].copy_from_slice(&block);
    put(&mut out, INFO, 6);
    put(&mut out, INFO + 4, play_seconds);
    put(&mut out, INFO + 8, DATA_CAPACITY as u32);
    put(&mut out, INFO + 12, block.len() as u32);

    put(&mut out, CONTAINER, CONTAINER_MAGIC);
    put(
        &mut out,
        CONTAINER + 4,
        (CONTAINER_HEADER_SIZE + INFO_SIZE + DATA_CAPACITY) as u32,
    );
    put(&mut out, CONTAINER + 8, INFO_SIZE as u32);
    put(&mut out, CONTAINER + 12, DATA_CAPACITY as u32);
    let info = checksum(&out[INFO..INFO + INFO_SIZE]);
    let data = checksum(&out[base..]);
    put(&mut out, CONTAINER + 16, info);
    put(&mut out, CONTAINER + 20, data);
    let header = checksum(&out[CONTAINER..CONTAINER + 24]);
    put(&mut out, CONTAINER + 24, header);
    Ok(out)
}

/// The decoded content of a chunk.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Content {
    Variables(Vec<SavedVariable>),
    PlayerFamily(PlayerFamily),
    Families(Families),
    Buildings(Buildings),
    Cities(Vec<City>),
    History(Vec<String>),
    Inventory(Inventory),
    Player(Player),
    Checkpoint(Checkpoint),
    Safehouses(Vec<u32>),
    /// The layout is not known; see [`Chunk::data`].
    Raw,
}

/// A story variable: the hash of its record in the `variabledictionary`
/// (the first field of the entry, not `sdbm` of the name) and its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SavedVariable {
    pub hash: Hash32,
    pub value: i32,
}

/// Chunk `variables` (exe 0x00442550): `u32 count`, then `count` pairs
/// `{ u32 hash, u32 value }`.
pub fn variables(d: &[u8]) -> Result<Vec<SavedVariable>> {
    let count = u32_at(d, 0)? as usize;
    if count.checked_mul(8).and_then(|n| n.checked_add(4)) != Some(d.len()) {
        return Err(Error::malformed(
            WHAT,
            0,
            format!("{count} variables do not fill {} bytes", d.len()),
        ));
    }
    (0..count)
        .map(|i| {
            Ok(SavedVariable {
                hash: Hash32(u32_at(d, 4 + 8 * i)?),
                value: u32_at(d, 8 + 8 * i)? as i32,
            })
        })
        .collect()
}

/// A member of the player's family.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Member {
    pub guid: Guid,
    /// Last number of the record (1, 2 and 4 in known saves); its meaning is
    /// not known.
    pub value: u32,
}

/// Chunk `player-family` (exe 0x008BCBC0).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlayerFamily {
    pub version: u32,
    /// The player's money.
    pub money: f32,
    pub flag: bool,
    pub members: Vec<Member>,
}

pub fn player_family(d: &[u8]) -> Result<PlayerFamily> {
    const RECORD: usize = 0x2C;
    let count = u32_at(d, 8)? as usize;
    let members = (0..count)
        .map(|i| {
            let at = 0x20 + RECORD * i;
            Ok(Member {
                guid: guid_at(d, at + 0x18)?,
                value: u32_at(d, at + 0x28)?,
            })
        })
        .collect::<Result<_>>()?;
    Ok(PlayerFamily {
        version: u32_at(d, 0)?,
        money: f32_at(d, 4)?,
        flag: u32_at(d, 12)? != 0,
        members,
    })
}

/// A family of the strategy game.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Family {
    /// Hash of the family name (`sdbm`, attribute 1 of the `Family` object).
    pub name: Hash32,
    /// Money of the family; [`MONEY_ELSEWHERE`] for the player's family.
    pub money: f32,
    /// Five numbers of the family object: `+0x150`, `+0x148`, `+0x14C`,
    /// `+0x154`, `+0x158`.
    pub timers: [f32; 5],
    pub byte_15c: u32,
    /// Flags of the family object, `+0x84`.
    pub flags: u32,
    pub value_120: u32,
    /// Ids of the buildings of the first list (`+0x108`).
    pub buildings: Vec<u32>,
    /// Ids of the buildings of the second list (`+0x114`).
    pub buildings_2: Vec<u32>,
    /// Other families and a number for each (`+0x18C`).
    pub relations: Vec<(Hash32, u32)>,
    /// GUIDs of the members (`+0x1A8`); the rest of a member record is not
    /// decoded.
    pub members: Vec<Guid>,
    /// Count of the records of the last list (`+0x130`), not decoded.
    pub records: u32,
    /// Books of the player's family: four lists of amounts.
    pub books: Option<[Vec<f32>; 4]>,
}

/// Chunk `families` (exe 0x008C72D0, family record 0x008C3CD0).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Families {
    pub version: u16,
    /// `u16` at `+0x14` of the player's crew object (`*0x0112989C`).
    pub crew_value: u16,
    pub families: Vec<Family>,
}

pub fn families(d: &[u8]) -> Result<Families> {
    let count = io::read_u16(d, 2, LE)? as usize;
    let mut at = 0x18usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let word = |k: usize| u32_at(d, at + 4 * k);
        let name = word(0)?;
        let (n_a, n_b, n_c) = (word(9)? as usize, word(10)? as usize, word(11)? as usize);
        let (n_d, extra_d) = (word(12)? as usize, word(13)? as usize);
        let (n_e, extra_e) = (word(14)? as usize, word(15)? as usize);
        let mut family = Family {
            name: Hash32(name),
            money: f32_at(d, at + 4)?,
            timers: [
                f32_at(d, at + 12)?,
                f32_at(d, at + 16)?,
                f32_at(d, at + 20)?,
                f32_at(d, at + 24)?,
                f32_at(d, at + 28)?,
            ],
            byte_15c: word(8)?,
            flags: word(16)?,
            value_120: word(19)?,
            buildings: Vec::new(),
            buildings_2: Vec::new(),
            relations: Vec::new(),
            members: Vec::new(),
            records: n_e as u32,
            books: None,
        };
        at += 0x60;
        family.buildings = u32_list(d, &mut at, n_a)?;
        family.buildings_2 = u32_list(d, &mut at, n_b)?;
        for _ in 0..n_c {
            family
                .relations
                .push((Hash32(u32_at(d, at)?), u32_at(d, at + 4)?));
            at += 8;
        }
        for i in 0..n_d {
            family.members.push(guid_at(d, at + 0x50 * i)?);
        }
        at = advance(d, at, sized(n_d, 0x50)?.checked_add(sized(extra_d, 4)?))?;
        at = advance(d, at, sized(n_e, 0x10)?.checked_add(sized(extra_e, 0x10)?))?;
        if name == CORLEONE && at < d.len() {
            // exe 0x00892760: 0x20 zero bytes, then four lists, each with a
            // count, 12 zero bytes and the amounts.
            at = advance(d, at, Some(0x20))?;
            let mut books: [Vec<f32>; 4] = Default::default();
            for list in &mut books {
                let n = u32_at(d, at)? as usize;
                at = advance(d, at, Some(0x10))?;
                for _ in 0..n {
                    list.push(f32_at(d, at)?);
                    at += 4;
                }
            }
            family.books = Some(books);
        }
        out.push(family);
    }
    Ok(Families {
        version: io::read_u16(d, 0, LE)?,
        crew_value: io::read_u16(d, 4, LE)?,
        families: out,
    })
}

/// A guard record of a building.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Guard {
    pub value: u32,
    pub guid: Guid,
}

/// A building of the `BricknMortar` manager.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Building {
    pub id: u32,
    pub flags: u32,
    /// Hash of the family that owns the building now (`+0x40`).
    pub owner: Hash32,
    /// Hash of the family at `+0x3C`: the owner at the start of the game in
    /// known saves.
    pub first_owner: Hash32,
    pub state: u32,
    pub value: f32,
    /// Numbers of the building object at `+0x5C`, `+0x60`, `+0x68`, `+0x6C`,
    /// `+0x74`, `+0x7C`, `+0x80`, `+0x64`, `+0xC8`, `+0x174`, `+0x178`.
    pub numbers: [u32; 11],
    pub guards: Vec<Guard>,
    pub guards_2: Vec<Guard>,
}

/// Chunk `buildings` (exe 0x006B0C90, record 0x006B2360).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Buildings {
    pub version: u16,
    pub buildings: Vec<Building>,
}

pub fn buildings(d: &[u8]) -> Result<Buildings> {
    let count = io::read_u16(d, 2, LE)? as usize;
    let mut at = 0x14usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let word = |k: usize| u32_at(d, at + 4 * k);
        let (n1, n2) = (word(14)? as usize, word(15)? as usize);
        let mut building = Building {
            id: word(0)?,
            flags: word(1)?,
            owner: Hash32(word(2)?),
            first_owner: Hash32(word(3)?),
            state: word(4)?,
            value: f32_at(d, at + 20)?,
            numbers: [
                word(7)?,
                word(8)?,
                word(9)?,
                word(10)?,
                word(11)?,
                word(12)?,
                word(13)?,
                word(16)?,
                word(17)?,
                word(19)?,
                word(20)?,
            ],
            guards: Vec::new(),
            guards_2: Vec::new(),
        };
        at += 0x60;
        for (list, n) in [(&mut building.guards, n1), (&mut building.guards_2, n2)] {
            for _ in 0..n {
                list.push(Guard {
                    value: u32_at(d, at)?,
                    guid: guid_at(d, at + 4)?,
                });
                at += 0x14;
            }
        }
        out.push(building);
    }
    Ok(Buildings {
        version: io::read_u16(d, 0, LE)?,
        buildings: out,
    })
}

/// A city: the hash of its name (`cuba`, `florida`, `newyork`) and a state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct City {
    pub name: Hash32,
    pub state: u32,
}

/// Chunk `cities` (exe 0x00849AC0): `u32 1, u32 count`, then pairs.
pub fn cities(d: &[u8]) -> Result<Vec<City>> {
    let count = u32_at(d, 4)? as usize;
    (0..count)
        .map(|i| {
            Ok(City {
                name: Hash32(u32_at(d, 8 + 8 * i)?),
                state: u32_at(d, 12 + 8 * i)?,
            })
        })
        .collect()
}

/// Chunk `history` (exe 0x009385A0): `u32 1, u32 count`, then `count`
/// NUL-terminated UTF-8 strings with the markup of the interface.
pub fn history(d: &[u8]) -> Result<Vec<String>> {
    let count = u32_at(d, 4)? as usize;
    let mut at = 8usize;
    let mut out = Vec::new();
    for _ in 0..count {
        let text = io::cstr(d, at)?;
        at += text.len() + 1;
        out.push(String::from_utf8_lossy(text).into_owned());
    }
    Ok(out)
}

/// A weapon slot of the player.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Slot {
    /// Index of the slot, 0–10.
    pub slot: u32,
    pub count: u32,
    /// GUID at `+0x264` of the weapon object.
    pub weapon: Guid,
    /// GUID at `-0x28` of the weapon object.
    pub object: Guid,
    /// Two numbers of the first five slots: `+0x4C8` of the weapon (0–4)
    /// and the result of 0x007013C0.
    pub level: Option<u32>,
    pub ammo: Option<u32>,
}

/// Chunk `inventory` (exe 0x00705CF0).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Inventory {
    /// 35 flags of the inventory (`+0x118`…) and, in bit 31 of the second
    /// word, the flag `+0xF4`.
    pub flags: [u32; 2],
    /// The slots that hold a weapon.
    pub slots: Vec<Slot>,
    pub extra: Vec<u8>,
}

pub fn inventory(d: &[u8]) -> Result<Inventory> {
    let flags = [u32_at(d, 0)?, u32_at(d, 4)?];
    let mut at = 8usize;
    let mut slots = Vec::new();
    for slot in 0..11u32 {
        let count = u32_at(d, at)?;
        at += 4;
        if count == 0 {
            continue;
        }
        let (weapon, object) = (guid_at(d, at)?, guid_at(d, at + 16)?);
        at += 32;
        let (mut level, mut ammo) = (None, None);
        if slot < 5 {
            level = Some(u32_at(d, at)?);
            ammo = Some(u32_at(d, at + 4)?);
            at += 8;
        }
        slots.push(Slot {
            slot,
            count,
            weapon,
            object,
            level,
            ammo,
        });
    }
    let extra_len = u32_at(d, at)? as usize;
    let extra = io::slice(d, at + 4, extra_len)?.to_vec();
    if u32_at(d, at + 4 + extra_len)? != END_MARK {
        return Err(Error::malformed(
            WHAT,
            at + 4 + extra_len,
            "the inventory does not end with 0xABBA",
        ));
    }
    Ok(Inventory {
        flags,
        slots,
        extra,
    })
}

/// Chunk `player` (exe 0x007AAD80).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Player {
    /// Results of the virtual calls `+0xC0` and `+0xC4` of the player object
    /// (4500 and 4500 in known saves).
    pub values: [f32; 2],
    /// Hashes of a list of a component of the player; 0 for an entry the
    /// game skipped.
    pub items: Vec<Hash32>,
}

pub fn player(d: &[u8]) -> Result<Player> {
    let count = u32_at(d, 8)? as usize;
    Ok(Player {
        values: [f32_at(d, 0)?, f32_at(d, 4)?],
        items: (0..count)
            .map(|i| u32_at(d, 16 + 4 * i).map(Hash32))
            .collect::<Result<_>>()?,
    })
}

/// Chunk `checkpoint` (exe 0x008FD0F0).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Checkpoint {
    /// Number of the current checkpoint (`Checkpoint +0x74`).
    pub checkpoint: u32,
    /// Whether the transform of the player was stored.
    pub has_transform: bool,
    /// The 4×4 matrix at `+0x10`; the position is the last row.
    pub transform: [f32; 16],
}

pub fn checkpoint(d: &[u8]) -> Result<Checkpoint> {
    let mut transform = [0f32; 16];
    for (i, value) in transform.iter_mut().enumerate() {
        *value = f32_at(d, 0x10 + 4 * i)?;
    }
    Ok(Checkpoint {
        checkpoint: u32_at(d, 0)?,
        has_transform: u32_at(d, 4)? != 0,
        transform,
    })
}

/// Chunk `safehouses` (exe 0x008DD5C0): a state 0–3 for each of 25
/// safehouses, then `0xABBA`.
pub fn safehouses(d: &[u8]) -> Result<Vec<u32>> {
    let mut out = Vec::new();
    for at in (0..d.len()).step_by(4) {
        let value = u32_at(d, at)?;
        if value == END_MARK {
            break;
        }
        out.push(value);
    }
    Ok(out)
}

fn u32_at(d: &[u8], at: usize) -> Result<u32> {
    io::read_u32(d, at, LE)
}

fn f32_at(d: &[u8], at: usize) -> Result<f32> {
    u32_at(d, at).map(f32::from_bits)
}

fn guid_at(d: &[u8], at: usize) -> Result<Guid> {
    Ok(Guid(io::slice(d, at, 16)?.try_into().expect("16 bytes")))
}

fn u32_list(d: &[u8], at: &mut usize, count: usize) -> Result<Vec<u32>> {
    let mut out = Vec::new();
    for _ in 0..count {
        out.push(u32_at(d, *at)?);
        *at += 4;
    }
    Ok(out)
}

fn sized(count: usize, size: usize) -> Result<usize> {
    count
        .checked_mul(size)
        .ok_or_else(|| Error::malformed(WHAT, 0, "a count is out of range"))
}

/// `at + len`, checked against the chunk.
fn advance(d: &[u8], at: usize, len: Option<usize>) -> Result<usize> {
    len.and_then(|len| at.checked_add(len))
        .filter(|&end| end <= d.len())
        .ok_or(Error::UnexpectedEof {
            offset: at,
            needed: len.unwrap_or(usize::MAX),
            len: d.len(),
        })
}

fn expect(data: &[u8], at: usize, value: u32, what: &str) -> Result<()> {
    let found = u32_at(data, at)?;
    if found != value {
        return Err(Error::malformed(
            WHAT,
            at,
            format!("{what}: {found:#x}, expected {value:#x}"),
        ));
    }
    Ok(())
}

/// The NUL-terminated UTF-16 string in the 0x800 bytes at `at`.
fn utf16(data: &[u8], at: usize) -> Result<String> {
    let units: Vec<u16> = io::slice(data, at, STRING_SIZE)?
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&c| c != 0)
        .collect();
    Ok(String::from_utf16_lossy(&units))
}

fn put(out: &mut [u8], at: usize, value: u32) {
    out[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_utf16(out: &mut [u8], at: usize, text: &str) -> Result<()> {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() * 2 + 2 > STRING_SIZE {
        return Err(Error::InvalidArgument(format!(
            "`{text}` is longer than 1023 characters"
        )));
    }
    for (i, unit) in units.iter().enumerate() {
        out[at + 2 * i..at + 2 * i + 2].copy_from_slice(&unit.to_le_bytes());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    fn sample() -> Vec<u8> {
        let mut history = words(&[1, 2]);
        history.extend_from_slice(b"first\0second\0");
        let mut family = words(&[4, 1500.5f32.to_bits(), 1, 0, 0, 0, 0, 0]);
        family.extend_from_slice(&[0; 0x18]);
        family.extend_from_slice(b"\x01\x02\x03A\x05\x06\x07\x08MACHINE1");
        family.extend_from_slice(&2u32.to_le_bytes());
        build(
            "Profile 1",
            "Cuba",
            134,
            0xB,
            &[
                (
                    0xE60B_027D,
                    words(&[2, 0x1111_1111, 1, 0x2222_2222, 0xFFFF_FFFF]),
                ),
                (0x460A_9E80, history),
                (0x2531_31B5, family),
                (0x874F_7077, words(&[1, 1, 0x3821_E851, 0])),
                (0x1234_5678, vec![1, 2, 3]),
            ],
        )
        .unwrap()
    }

    #[test]
    fn checksum_matches_the_game() {
        // Header of a save made on the bench: the stored checksum of its
        // first 0x18 bytes is 0x1508F423.
        let header = words(&[
            0x4D43_3032,
            0x40130,
            0x114,
            0x40000,
            0x8157_7CAA,
            0xB42C_4AD5,
        ]);
        assert_eq!(checksum(&header), 0x1508_F423);
        assert_eq!(checksum(&[1, 2, 3]), 0);
    }

    #[test]
    fn a_built_save_parses_back() {
        let data = sample();
        assert_eq!(data.len(), 272_740);
        let save = SaveFile::parse(&data).unwrap();
        assert_eq!(save.profile, "Profile 1");
        assert_eq!(save.city, "Cuba");
        assert_eq!(save.play_time, "0:02:14");
        assert_eq!(save.description, "Profile 1 - Cuba - 0:02:14");
        assert_eq!((save.version, save.play_seconds, save.mask), (6, 134, 0xB));
        assert!(save.checksums.ok());
        assert_eq!(save.chunks.len(), 5);
        assert_eq!(
            save.variables().unwrap(),
            [
                SavedVariable {
                    hash: Hash32(0x1111_1111),
                    value: 1
                },
                SavedVariable {
                    hash: Hash32(0x2222_2222),
                    value: -1
                },
            ]
        );
        assert_eq!(save.money().unwrap(), Some(1500.5));
        let history = save.chunk_named("history").unwrap().content().unwrap();
        assert_eq!(
            history,
            Content::History(vec!["first".into(), "second".into()])
        );
        let Content::PlayerFamily(family) = save
            .chunk_named("player-family")
            .unwrap()
            .content()
            .unwrap()
        else {
            panic!("player family expected");
        };
        assert_eq!(family.members.len(), 1);
        assert_eq!(family.members[0].value, 2);
        // An unknown chunk with a size that is not a multiple of four.
        let unknown = save.chunk_named("12345678").unwrap();
        assert_eq!(unknown.name(), "c12345678");
        assert_eq!(unknown.data, [1, 2, 3]);
        assert_eq!(unknown.content().unwrap(), Content::Raw);
    }

    #[test]
    fn damage_is_reported() {
        let mut data = sample();
        let last = data.len() - 1;
        data[last] ^= 1;
        let save = SaveFile::parse(&data).unwrap();
        assert!(!save.checksums.data.ok());
        assert!(save.checksums.header.ok() && save.checksums.info.ok());

        let mut data = sample();
        data[0] = b'X';
        assert!(SaveFile::parse(&data).is_err());
        assert!(SaveFile::parse(&sample()[..0x3000]).is_err());

        // A chunk that claims more bytes than are in use.
        let mut data = sample();
        let at = INFO + INFO_SIZE + 4 + 8;
        data[at..at + 4].copy_from_slice(&0x0010_0000u32.to_le_bytes());
        assert!(SaveFile::parse(&data).is_err());
    }

    #[test]
    fn inventory_and_safehouses_end_with_the_mark() {
        let mut d = words(&[0, 0x8000_0000, 1]);
        d.extend_from_slice(&[0x11; 16]);
        d.extend_from_slice(&[0x22; 16]);
        d.extend(words(&[3, 14]));
        d.extend(words(&[0; 10]));
        d.extend(words(&[0, END_MARK]));
        let inventory = inventory(&d).unwrap();
        assert_eq!(inventory.slots.len(), 1);
        assert_eq!(
            (inventory.slots[0].level, inventory.slots[0].ammo),
            (Some(3), Some(14))
        );
        assert!(super::inventory(&d[..d.len() - 4]).is_err());
        assert_eq!(safehouses(&words(&[0, 3, 1, END_MARK])).unwrap(), [0, 3, 1]);
    }

    #[test]
    fn chunk_ids_are_unique() {
        for (i, a) in CHUNKS.iter().enumerate() {
            assert!(
                CHUNKS[i + 1..]
                    .iter()
                    .all(|b| b.id != a.id && b.name != a.name)
            );
        }
        assert_eq!(crate::hash::sdbm(b"mobface"), 0x87E0_3A5D);
        assert_eq!(crate::hash::sdbm(b"corleone"), CORLEONE);
    }
}
