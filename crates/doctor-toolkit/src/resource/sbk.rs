//! `sbk` sound banks as a whole: the session metadata before `sdat`
//! (emitters, their tracks and variations, the sequencer commands,
//! instruments and tones, lookup tables) and the samples in `sdat`.
//!
//! Layout (`docs/formats/audio.md` §3; little-endian on PC):
//!
//! ```text
//! 0x00  "sbnk", u32 206, u32 metadata size (from 0x18), u32 sdat size,
//!       u32 sdat offset (metadata end aligned to 0x40), u32 sdat size
//! 0x18  session header (0x58): u32 206, type, samples, offsets and sizes of
//!       the pools, emitter and instrument counts, name table, GUID
//!       u32 emitter table[emitters]   offsets into the emitter pool
//!       emitters (0x54 each: 0x20 emitter + 0x34 track)
//!       instrument banks (u32 count, u32 offset into the instrument table)
//!       u32 instrument table[instruments]   offsets into the instrument pool
//!       instruments (u16 tones, u8 flags, u8 pad, tones of 0x94)
//!       commands (8 bytes: u16 delay ms, u8 id, i8 arg, i32 arg)
//!       variation headers (8 bytes: auto-duck start, stop)
//!       emitter lookup, instrument lookup (24 bytes each)
//!       name table (0x30 per emitter; absent in every bank of the game)
//! sdat  "sdat", u32 N, N × {u32 index, u32 0x0313BABE, u32 offset, u32 0xFEEDFEED},
//!       samples (SNR header and SNS blocks), packed
//! ```
//!
//! The game rebuilds every pool pointer from the emitter pool offset and the
//! pool sizes (`godfather2.exe` `0x005F0990`), so [`SoundBank::to_bytes`]
//! derives all offsets the same way; for the banks of the game the result is
//! the original file, byte for byte.
//!
//! An emitter is found by name: the game hashes the lower-case `.emx` name
//! with SDBM and scans the emitter lookup of every loaded bank for that hash
//! (`0x005F0560`). An emitter's track names its instrument by the same hash,
//! which the game finds by binary search in the instrument lookup
//! (`0x005F0BE0`); both lookups are sorted by their first field.

use std::collections::HashSet;

use serde::Serialize;

use crate::hash::sdbm;
use crate::resource::audio::Sample;
use crate::{Error, Result};

/// Version of the bank and of its session header.
pub const VERSION: u32 = 206;
/// Size of the bank header; the session header follows it.
pub const BANK_HEADER: usize = 0x18;
/// Size of the session header; the emitter table follows it.
pub const SESSION_HEADER: usize = 0x58;
/// Size of an emitter with its track.
pub const EMITTER_SIZE: usize = 0x54;
/// Size of a tone (envelope and sample number).
pub const TONE_SIZE: usize = 0x94;
/// Size of a lookup table entry.
pub const LOOKUP_SIZE: usize = 24;
/// Size of a name table entry.
pub const NAME_SIZE: usize = 0x30;
/// Alignment of the `sdat` section.
pub const SDAT_ALIGN: usize = 0x40;
/// Name hash slot of every `sdat` entry in the game.
pub const SAMPLE_HASH: u32 = 0x0313_BABE;
/// Runtime pointer slot of every `sdat` entry on disk.
pub const SAMPLE_POINTER: u32 = 0xFEED_FEED;
/// Track instrument values that mean "no instrument".
pub const NO_INSTRUMENT: [u32; 2] = [0xDEAD_BEEF, 0xFFFF_FFFF];

/// Sequencer command ids (`TrackCmd::Id`).
pub mod cmd {
    /// Does nothing; pads variations to the shared command count.
    pub const NONE: u8 = 0;
    /// Ends the variation; the delay is the loop delay, the 8-bit argument
    /// how many commands to skip when looping.
    pub const END: u8 = 1;
    /// Starts tone `arg8` of the instrument with velocity `arg32` (0–127).
    pub const KEY_ON: u8 = 2;
    /// Releases tone `arg8`.
    pub const KEY_OFF: u8 = 3;
    /// Sets the pitch to `arg32 / 65536`.
    pub const PITCH_BEND: u8 = 4;
    /// Sets the pitch change per tick to `arg32`.
    pub const PITCH_RAMP: u8 = 5;
    /// Sets the modulation rate to `arg32 / 65536`.
    pub const MOD_PITCH: u8 = 6;
    /// Number of command ids the game dispatches.
    pub const COUNT: u8 = 7;

    /// Readable name of a command id.
    pub fn name(id: u8) -> &'static str {
        match id {
            NONE => "none",
            END => "end",
            KEY_ON => "key on",
            KEY_OFF => "key off",
            PITCH_BEND => "pitch bend",
            PITCH_RAMP => "pitch ramp",
            MOD_PITCH => "mod pitch",
            _ => "unknown",
        }
    }
}

/// Emitter flag bits.
pub mod flag {
    /// Positioned in the world (3D); clear for a 2D sound.
    pub const THREE_D: u8 = 0x01;
    /// Doppler, scaled by the track's doppler factor.
    pub const DOPPLER: u8 = 0x02;
    /// Reverb send from distance.
    pub const REVERB_FROM_DISTANCE: u8 = 0x04;
    /// Bell attenuation curve.
    pub const ATTN_BELL: u8 = 0x08;
    /// Inverse bell attenuation curve.
    pub const ATTN_INVERSE_BELL: u8 = 0x10;
    /// Squared exponential attenuation curve.
    pub const ATTN_EXP2: u8 = 0x20;
}

/// SDBM hash of the lower-case name: the key the game finds emitters by.
pub fn name_hash(name: &str) -> u32 {
    sdbm(name.to_ascii_lowercase().as_bytes())
}

/// Fields of the session header that are not derived from the layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Session {
    pub version: u32,
    /// 0 for a sound bank.
    pub kind: u32,
    /// 0 in every bank; the sample count is in `sdat`.
    pub samples: u32,
    /// Overwritten by the game at load time.
    pub guid: u32,
    pub reserved: [u32; 2],
}

/// A playable sound: emitter parameters and its track.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Emitter {
    /// SDBM hash of the lower-case `.emx` name.
    pub template_id: u32,
    /// Voice stealing priority.
    pub priority: u8,
    /// Most simultaneous performances.
    pub poly_max: u8,
    /// Runtime counter, 0 on disk.
    pub poly_cur: u8,
    /// [`flag`] bits.
    pub flags: u8,
    /// Distance of full attenuation.
    pub falloff: f32,
    /// Distance where attenuation starts (0 in every bank).
    pub begin_falloff: f32,
    /// Precomputed inverses, 0 on disk.
    pub inv_falloff: f32,
    pub begin_inv_falloff: f32,
    /// Mixer category (a hash listed in `mix` resources).
    pub mix_category: u32,
    /// Ducking category, 0 for none.
    pub duck_category: u32,
    pub track: Track,
}

/// How an emitter plays: random spreads, volume and its variations.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Track {
    /// Voices per performance.
    pub voice_poly: u8,
    /// 0 selects the variation explicitly, otherwise the game picks one
    /// (3 in most banks, 1 in a few).
    pub select: u8,
    /// Commands per variation, shared by every variation.
    pub commands_per_variation: u8,
    /// Last played variation, 0xFF on disk.
    pub last_variation: u8,
    pub priority: u8,
    pub pad: [u8; 2],
    /// Track flag bits (stereo 0x01, azimuth offset 0x02, hold random pitch
    /// 0x04, hold random attenuation 0x08, looping voices 0x10, play when
    /// silent 0x20, no restart 0x40).
    pub flags: u32,
    pub distance_factor: f32,
    /// Random pitch spread, 0–1.
    pub pitch_range: f32,
    /// Random attenuation spread, 0–1.
    pub attn_range: f32,
    /// Random start offset spread.
    pub offset_range: f32,
    pub doppler: f32,
    /// Volume, 0–1.
    pub volume: f32,
    pub azimuth: f32,
    /// Instrument lookup key: the emitter's name hash, or a [`NO_INSTRUMENT`] value.
    pub instrument: u32,
    pub variations: Vec<Variation>,
}

/// One alternative of a track: its commands and auto-duck times.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Variation {
    pub duck_start: u32,
    pub duck_stop: u32,
    pub commands: Vec<Command>,
}

/// A sequencer command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Command {
    /// Wait before the command, in milliseconds.
    pub delay: u16,
    /// [`cmd`] id.
    pub id: u8,
    pub arg8: i8,
    pub arg32: i32,
}

impl Command {
    /// A [`cmd::NONE`] command with no delay: padding.
    pub const PAD: Self = Self {
        delay: 0,
        id: cmd::NONE,
        arg8: 0,
        arg32: 0,
    };

    /// Starts `tone` with `velocity`.
    pub fn key_on(tone: u8, velocity: i32) -> Self {
        Self {
            delay: 0,
            id: cmd::KEY_ON,
            arg8: tone as i8,
            arg32: velocity,
        }
    }

    /// Sets the pitch to `ratio`.
    pub fn pitch_bend(ratio: f32) -> Self {
        Self {
            delay: 0,
            id: cmd::PITCH_BEND,
            arg8: 0,
            arg32: (f64::from(ratio) * 65536.0).round() as i32,
        }
    }
}

/// An instrument: the tones an emitter's commands play.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Instrument {
    pub flags: u8,
    pub pad: u8,
    pub tones: Vec<Tone>,
}

/// A tone: a sample with its envelope.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Tone {
    pub pre_volume: f32,
    /// Breakpoints (volume, seconds); only the first `segment_count` are used,
    /// the others are (1, 0) in every bank of the game.
    pub segments: [(f32, f32); 16],
    pub release: (f32, f32),
    pub segment_count: i32,
    /// Number of the sample in `sdat`.
    pub sample: u32,
}

/// An entry of a lookup table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LookupItem {
    /// Search key; the tables are sorted by it.
    pub key: u32,
    /// Emitter number (emitter lookup) or instrument number in its bank.
    pub index: u32,
    /// Bank number (instrument lookup) and two zeros.
    pub values: [u32; 3],
    /// SDBM hash of the lower-case name.
    pub name_hash: u32,
}

/// A sample of `sdat`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BankSample {
    pub index: u32,
    pub hash: u32,
    pub pointer: u32,
    /// SNR header and SNS blocks.
    #[serde(skip)]
    pub bytes: Vec<u8>,
}

/// A whole `sbk` bank.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SoundBank {
    /// `sbnk` read as a little-endian `u32`.
    pub magic: u32,
    pub version: u32,
    pub session: Session,
    pub emitters: Vec<Emitter>,
    /// Instruments per instrument bank; the banks own consecutive instruments.
    pub instrument_banks: Vec<u32>,
    pub instruments: Vec<Instrument>,
    pub emitter_lookup: Vec<LookupItem>,
    pub instrument_lookup: Vec<LookupItem>,
    /// Raw name table (0x30 bytes per emitter), when the bank has one.
    #[serde(skip)]
    pub name_table: Option<Vec<Vec<u8>>>,
    /// `sdat` read as a little-endian `u32`.
    pub sdat_magic: u32,
    pub samples: Vec<BankSample>,
}

/// Bounds-checked little-endian reader.
struct Reader<'a> {
    data: &'a [u8],
}

impl Reader<'_> {
    fn bytes(&self, at: usize, len: usize) -> Result<&[u8]> {
        crate::io::slice(self.data, at, len)
    }
    fn u8(&self, at: usize) -> Result<u8> {
        Ok(self.bytes(at, 1)?[0])
    }
    fn u16(&self, at: usize) -> Result<u16> {
        let b = self.bytes(at, 2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&self, at: usize) -> Result<u32> {
        let b = self.bytes(at, 4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn f32(&self, at: usize) -> Result<f32> {
        self.u32(at).map(f32::from_bits)
    }
    fn i32(&self, at: usize) -> Result<i32> {
        self.u32(at).map(|v| v as i32)
    }
}

fn malformed(offset: usize, reason: impl Into<String>) -> Error {
    Error::malformed("sbk", offset, reason.into())
}

/// `base + offset`, checked.
fn at(base: usize, offset: u32) -> Result<usize> {
    base.checked_add(offset as usize)
        .ok_or_else(|| malformed(base, "offset overflows"))
}

/// Refuses counts whose records cannot fit in `len` bytes.
fn check_count(count: u32, size: usize, len: usize, what: &str) -> Result<usize> {
    let count = count as usize;
    if count.saturating_mul(size) > len {
        return Err(malformed(
            0,
            format!("{count} {what} do not fit in the bank"),
        ));
    }
    Ok(count)
}

impl SoundBank {
    /// Parses a bank.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let r = Reader { data };
        let magic = r.u32(0)?;
        if &magic.to_le_bytes() != b"sbnk" {
            return Err(Error::BadMagic {
                what: "sbk",
                found: magic,
            });
        }
        let version = r.u32(4)?;
        if version != VERSION {
            return Err(Error::UnsupportedVersion {
                what: "sbk",
                version,
            });
        }
        let sdat = r.u32(0x10)? as usize;
        let sdat_size = r.u32(0x14)? as usize;
        let s = BANK_HEADER;
        let field = |i: usize| r.u32(s + 4 * i);
        let session = Session {
            version: field(0)?,
            kind: field(1)?,
            samples: field(2)?,
            guid: field(17)?,
            reserved: [field(18)?, field(19)?],
        };
        let len = data.len();
        let banks = check_count(field(4)?, 8, len, "instrument banks")?;
        let pool_size = field(7)? as usize;
        let seq_size = field(9)?;
        let emitter_lookup = field(12)?;
        let emitters = check_count(field(14)?, EMITTER_SIZE, len, "emitters")?;
        let instruments = check_count(field(15)?, 4, len, "instruments")?;
        let name_table = field(16)?;
        let emit_pool = field(20)?;
        let emit_size = field(21)?;
        // The pool walk of the game (0x005F0990).
        let emit_base = at(s, emit_pool)?;
        let bank_base = at(emit_base, emit_size)?;
        let table_base = bank_base
            .checked_add(banks * 8)
            .ok_or_else(|| malformed(s, "pool walk overflows"))?;
        let pool_base = table_base
            .checked_add(instruments * 4)
            .ok_or_else(|| malformed(s, "pool walk overflows"))?;
        let seq_base = pool_base
            .checked_add(pool_size)
            .ok_or_else(|| malformed(s, "pool walk overflows"))?;
        let var_base = at(seq_base, seq_size)?;
        let lookup_base = at(s, emitter_lookup)?;
        let ins_lookup_base = lookup_base
            .checked_add(emitters * LOOKUP_SIZE)
            .ok_or_else(|| malformed(s, "pool walk overflows"))?;

        let mut emitter_list = Vec::with_capacity(emitters);
        for i in 0..emitters {
            let offset = r.u32(s + SESSION_HEADER + 4 * i)?;
            let e = at(emit_base, offset)?;
            r.bytes(e, EMITTER_SIZE)?;
            let t = e + 0x20;
            let variations = r.u8(t + 2)? as usize;
            let per = r.u8(t + 3)? as usize;
            let commands_at = at(seq_base, r.u32(t + 0x2C)?)?;
            let headers_at = at(var_base, r.u32(t + 0x30)?)?;
            r.bytes(commands_at, variations * per * 8)?;
            r.bytes(headers_at, variations * 8)?;
            let mut list = Vec::with_capacity(variations);
            for v in 0..variations {
                let commands = (0..per)
                    .map(|k| {
                        let c = commands_at + 8 * (v * per + k);
                        Ok(Command {
                            delay: r.u16(c)?,
                            id: r.u8(c + 2)?,
                            arg8: r.u8(c + 3)? as i8,
                            arg32: r.i32(c + 4)?,
                        })
                    })
                    .collect::<Result<_>>()?;
                list.push(Variation {
                    duck_start: r.u32(headers_at + 8 * v)?,
                    duck_stop: r.u32(headers_at + 8 * v + 4)?,
                    commands,
                });
            }
            emitter_list.push(Emitter {
                template_id: r.u32(e)?,
                priority: r.u8(e + 4)?,
                poly_max: r.u8(e + 5)?,
                poly_cur: r.u8(e + 6)?,
                flags: r.u8(e + 7)?,
                falloff: r.f32(e + 8)?,
                begin_falloff: r.f32(e + 0x0C)?,
                inv_falloff: r.f32(e + 0x10)?,
                begin_inv_falloff: r.f32(e + 0x14)?,
                mix_category: r.u32(e + 0x18)?,
                duck_category: r.u32(e + 0x1C)?,
                track: Track {
                    voice_poly: r.u8(t)?,
                    select: r.u8(t + 1)?,
                    commands_per_variation: per as u8,
                    last_variation: r.u8(t + 4)?,
                    priority: r.u8(t + 5)?,
                    pad: [r.u8(t + 6)?, r.u8(t + 7)?],
                    flags: r.u32(t + 8)?,
                    distance_factor: r.f32(t + 0x0C)?,
                    pitch_range: r.f32(t + 0x10)?,
                    attn_range: r.f32(t + 0x14)?,
                    offset_range: r.f32(t + 0x18)?,
                    doppler: r.f32(t + 0x1C)?,
                    volume: r.f32(t + 0x20)?,
                    azimuth: r.f32(t + 0x24)?,
                    instrument: r.u32(t + 0x28)?,
                    variations: list,
                },
            });
        }

        let mut instrument_banks = Vec::with_capacity(banks);
        let mut slot = 0u64;
        for b in 0..banks {
            let count = r.u32(bank_base + 8 * b)?;
            let offset = r.u32(bank_base + 8 * b + 4)?;
            if u64::from(offset) != slot * 4 {
                return Err(malformed(
                    bank_base + 8 * b,
                    format!("instrument bank {b} does not follow the previous one"),
                ));
            }
            slot += u64::from(count);
            instrument_banks.push(count);
        }
        if slot != instruments as u64 {
            return Err(malformed(
                bank_base,
                format!("instrument banks hold {slot} instruments, the bank has {instruments}"),
            ));
        }
        let mut instrument_list = Vec::with_capacity(instruments);
        for i in 0..instruments {
            let offset = r.u32(table_base + 4 * i)? as usize;
            if offset.saturating_add(4) > pool_size {
                return Err(malformed(table_base + 4 * i, "instrument outside its pool"));
            }
            let p = pool_base + offset;
            let count = r.u16(p)? as usize;
            if offset + 4 + count * TONE_SIZE > pool_size {
                return Err(malformed(p, "instrument tones run past their pool"));
            }
            let tones = (0..count)
                .map(|k| {
                    let o = p + 4 + k * TONE_SIZE;
                    let mut segments = [(0.0, 0.0); 16];
                    for (j, seg) in segments.iter_mut().enumerate() {
                        *seg = (r.f32(o + 4 + 8 * j)?, r.f32(o + 8 + 8 * j)?);
                    }
                    Ok(Tone {
                        pre_volume: r.f32(o)?,
                        segments,
                        release: (r.f32(o + 0x84)?, r.f32(o + 0x88)?),
                        segment_count: r.i32(o + 0x8C)?,
                        sample: r.u32(o + 0x90)?,
                    })
                })
                .collect::<Result<_>>()?;
            instrument_list.push(Instrument {
                flags: r.u8(p + 2)?,
                pad: r.u8(p + 3)?,
                tones,
            });
        }

        let lookup = |base: usize, count: usize| -> Result<Vec<LookupItem>> {
            r.bytes(base, count * LOOKUP_SIZE)?;
            (0..count)
                .map(|i| {
                    let o = base + LOOKUP_SIZE * i;
                    Ok(LookupItem {
                        key: r.u32(o)?,
                        index: r.u32(o + 4)?,
                        values: [r.u32(o + 8)?, r.u32(o + 12)?, r.u32(o + 16)?],
                        name_hash: r.u32(o + 20)?,
                    })
                })
                .collect()
        };
        let emitter_lookup = lookup(lookup_base, emitters)?;
        let instrument_lookup = lookup(ins_lookup_base, instruments)?;
        let name_table = if name_table == 0 {
            None
        } else {
            let base = at(s, name_table)?;
            r.bytes(base, emitters * NAME_SIZE)?;
            Some(
                (0..emitters)
                    .map(|i| data[base + NAME_SIZE * i..base + NAME_SIZE * (i + 1)].to_vec())
                    .collect(),
            )
        };

        // sdat: samples run from their offset to the next one.
        let sdat_magic = r.u32(sdat)?;
        if &sdat_magic.to_le_bytes() != b"sdat" {
            return Err(malformed(sdat, "missing sdat section"));
        }
        let end = sdat.saturating_add(sdat_size).min(len);
        let count = check_count(r.u32(sdat + 4)?, 16, len, "samples")?;
        r.bytes(sdat + 8, count * 16)?;
        let offsets: Vec<usize> = (0..count)
            .map(|i| r.u32(sdat + 8 + 16 * i + 8).map(|o| sdat + o as usize))
            .collect::<Result<_>>()?;
        let mut samples = Vec::with_capacity(count);
        for i in 0..count {
            let start = offsets[i];
            let stop = offsets.get(i + 1).copied().unwrap_or(end);
            if stop < start || stop > len {
                return Err(malformed(sdat + 8 + 16 * i, "sample offsets out of order"));
            }
            let e = sdat + 8 + 16 * i;
            samples.push(BankSample {
                index: r.u32(e)?,
                hash: r.u32(e + 4)?,
                pointer: r.u32(e + 12)?,
                bytes: data[start..stop].to_vec(),
            });
        }
        Ok(Self {
            magic,
            version,
            session,
            emitters: emitter_list,
            instrument_banks,
            instruments: instrument_list,
            emitter_lookup,
            instrument_lookup,
            name_table,
            sdat_magic,
            samples,
        })
    }

    /// Size of the instrument pool.
    fn instrument_pool_size(&self) -> usize {
        self.instruments
            .iter()
            .map(|i| 4 + i.tones.len() * TONE_SIZE)
            .sum()
    }

    /// Writes the bank. Offsets and sizes are derived from the content the
    /// way the game walks them.
    pub fn to_bytes(&self) -> Vec<u8> {
        let ne = self.emitters.len();
        let ni = self.instruments.len();
        let emit_pool = SESSION_HEADER + 4 * ne;
        let emit_size = EMITTER_SIZE * ne;
        let bank_pool = emit_pool + emit_size;
        let table = bank_pool + 8 * self.instrument_banks.len();
        let pool = table + 4 * ni;
        let pool_size = self.instrument_pool_size();
        let seq = pool + pool_size;
        let seq_size: usize = self
            .emitters
            .iter()
            .flat_map(|e| &e.track.variations)
            .map(|v| 8 * v.commands.len())
            .sum();
        let var = seq + seq_size;
        let var_size: usize = self
            .emitters
            .iter()
            .map(|e| 8 * e.track.variations.len())
            .sum();
        let lookup = var + var_size;
        let ins_lookup = lookup + LOOKUP_SIZE * ne;
        let mut meta_end = ins_lookup + LOOKUP_SIZE * ni;
        let names = self.name_table.as_ref().map(|_| meta_end);
        if names.is_some() {
            meta_end += NAME_SIZE * ne;
        }
        let sdat = (BANK_HEADER + meta_end).next_multiple_of(SDAT_ALIGN);
        let table_end = 8 + 16 * self.samples.len();
        let sdat_size = table_end + self.samples.iter().map(|s| s.bytes.len()).sum::<usize>();

        let mut out = Vec::with_capacity(sdat + sdat_size);
        let put = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
        for v in [
            self.magic,
            self.version,
            meta_end as u32,
            sdat_size as u32,
            sdat as u32,
            sdat_size as u32,
        ] {
            put(&mut out, v);
        }
        let s = &self.session;
        for v in [
            s.version,
            s.kind,
            s.samples,
            bank_pool as u32,
            self.instrument_banks.len() as u32,
            table as u32,
            pool as u32,
            pool_size as u32,
            seq as u32,
            seq_size as u32,
            var as u32,
            var_size as u32,
            lookup as u32,
            ins_lookup as u32,
            ne as u32,
            ni as u32,
            names.unwrap_or(0) as u32,
            s.guid,
            s.reserved[0],
            s.reserved[1],
            emit_pool as u32,
            emit_size as u32,
        ] {
            put(&mut out, v);
        }
        for i in 0..ne {
            put(&mut out, (i * EMITTER_SIZE) as u32);
        }
        let (mut cmd_at, mut var_at) = (0u32, 0u32);
        for e in &self.emitters {
            put(&mut out, e.template_id);
            out.extend_from_slice(&[e.priority, e.poly_max, e.poly_cur, e.flags]);
            for f in [
                e.falloff,
                e.begin_falloff,
                e.inv_falloff,
                e.begin_inv_falloff,
            ] {
                put(&mut out, f.to_bits());
            }
            put(&mut out, e.mix_category);
            put(&mut out, e.duck_category);
            let t = &e.track;
            out.extend_from_slice(&[
                t.voice_poly,
                t.select,
                t.variations.len() as u8,
                t.commands_per_variation,
                t.last_variation,
                t.priority,
                t.pad[0],
                t.pad[1],
            ]);
            put(&mut out, t.flags);
            for f in [
                t.distance_factor,
                t.pitch_range,
                t.attn_range,
                t.offset_range,
                t.doppler,
                t.volume,
                t.azimuth,
            ] {
                put(&mut out, f.to_bits());
            }
            put(&mut out, t.instrument);
            put(&mut out, cmd_at);
            put(&mut out, var_at);
            cmd_at += t
                .variations
                .iter()
                .map(|v| 8 * v.commands.len() as u32)
                .sum::<u32>();
            var_at += 8 * t.variations.len() as u32;
        }
        let mut slot = 0u32;
        for &count in &self.instrument_banks {
            put(&mut out, count);
            put(&mut out, slot * 4);
            slot += count;
        }
        let mut offset = 0u32;
        for i in &self.instruments {
            put(&mut out, offset);
            offset += (4 + i.tones.len() * TONE_SIZE) as u32;
        }
        for i in &self.instruments {
            out.extend_from_slice(&(i.tones.len() as u16).to_le_bytes());
            out.extend_from_slice(&[i.flags, i.pad]);
            for tone in &i.tones {
                put(&mut out, tone.pre_volume.to_bits());
                for (v, sec) in tone.segments {
                    put(&mut out, v.to_bits());
                    put(&mut out, sec.to_bits());
                }
                put(&mut out, tone.release.0.to_bits());
                put(&mut out, tone.release.1.to_bits());
                put(&mut out, tone.segment_count as u32);
                put(&mut out, tone.sample);
            }
        }
        for c in self
            .emitters
            .iter()
            .flat_map(|e| &e.track.variations)
            .flat_map(|v| &v.commands)
        {
            out.extend_from_slice(&c.delay.to_le_bytes());
            out.extend_from_slice(&[c.id, c.arg8 as u8]);
            put(&mut out, c.arg32 as u32);
        }
        for v in self.emitters.iter().flat_map(|e| &e.track.variations) {
            put(&mut out, v.duck_start);
            put(&mut out, v.duck_stop);
        }
        for item in self.emitter_lookup.iter().chain(&self.instrument_lookup) {
            for v in [
                item.key,
                item.index,
                item.values[0],
                item.values[1],
                item.values[2],
                item.name_hash,
            ] {
                put(&mut out, v);
            }
        }
        if let Some(names) = &self.name_table {
            for i in 0..ne {
                let mut record = [0u8; NAME_SIZE];
                if let Some(name) = names.get(i) {
                    let n = name.len().min(NAME_SIZE);
                    record[..n].copy_from_slice(&name[..n]);
                }
                out.extend_from_slice(&record);
            }
        }
        out.resize(sdat, 0);
        put(&mut out, self.sdat_magic);
        put(&mut out, self.samples.len() as u32);
        let mut data_at = table_end as u32;
        for sample in &self.samples {
            put(&mut out, sample.index);
            put(&mut out, sample.hash);
            put(&mut out, data_at);
            put(&mut out, sample.pointer);
            data_at += sample.bytes.len() as u32;
        }
        for sample in &self.samples {
            out.extend_from_slice(&sample.bytes);
        }
        out
    }

    /// The emitter's name, when the name list knows its hash.
    pub fn emitter_name(&self, emitter: usize) -> Option<&'static str> {
        self.emitters
            .get(emitter)
            .and_then(|e| crate::resource::names::emitter_names().get(e.template_id))
    }

    /// Display name of an emitter: its name, or its hash in hexadecimal.
    pub fn emitter_label(&self, emitter: usize) -> String {
        match (self.emitter_name(emitter), self.emitters.get(emitter)) {
            (Some(name), _) => name.to_owned(),
            (None, Some(e)) => format!("{:08x}", e.template_id),
            (None, None) => format!("#{emitter}"),
        }
    }

    /// Finds an emitter by `.emx` name, hexadecimal hash or `#number`.
    pub fn find_emitter(&self, text: &str) -> Option<usize> {
        if let Some(n) = text.strip_prefix('#') {
            return n.parse().ok().filter(|&i: &usize| i < self.emitters.len());
        }
        let by_hash = |h: u32| self.emitters.iter().position(|e| e.template_id == h);
        by_hash(name_hash(text))
            .or_else(|| by_hash(name_hash(&format!("{text}.emx"))))
            .or_else(|| {
                u32::from_str_radix(text.trim_start_matches("0x"), 16)
                    .ok()
                    .and_then(by_hash)
            })
    }

    /// The instrument an emitter plays, found as the game finds it: binary
    /// search of the instrument lookup, then the bank and the slot in it.
    pub fn instrument_of(&self, emitter: usize) -> Option<usize> {
        let key = self.emitters.get(emitter)?.track.instrument;
        if NO_INSTRUMENT.contains(&key) {
            return None;
        }
        let item = self
            .instrument_lookup
            .binary_search_by_key(&key, |i| i.key)
            .ok()
            .map(|i| self.instrument_lookup[i])?;
        let bank = item.values[0] as usize;
        let count = *self.instrument_banks.get(bank)? as usize;
        if item.index as usize >= count {
            return None;
        }
        let first: usize = self.instrument_banks[..bank]
            .iter()
            .map(|&c| c as usize)
            .sum();
        Some(first + item.index as usize).filter(|&i| i < self.instruments.len())
    }

    /// Problems that would make the game skip or misplay something: counts
    /// that disagree, unsorted lookups, references that do not resolve.
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let ne = self.emitters.len();
        let ni = self.instruments.len();
        if self.emitter_lookup.len() != ne {
            problems.push(format!(
                "the emitter lookup has {} entries for {ne} emitters",
                self.emitter_lookup.len()
            ));
        }
        if self.instrument_lookup.len() != ni {
            problems.push(format!(
                "the instrument lookup has {} entries for {ni} instruments",
                self.instrument_lookup.len()
            ));
        }
        let banked: u64 = self.instrument_banks.iter().map(|&c| u64::from(c)).sum();
        if banked != ni as u64 {
            problems.push(format!(
                "instrument banks hold {banked} instruments, the bank has {ni}"
            ));
        }
        for (name, table) in [
            ("emitter", &self.emitter_lookup),
            ("instrument", &self.instrument_lookup),
        ] {
            if table.windows(2).any(|w| w[0].key >= w[1].key) {
                problems.push(format!(
                    "the {name} lookup is not sorted by key or has a key twice"
                ));
            }
        }
        let mut seen = HashSet::new();
        for item in &self.emitter_lookup {
            match self.emitters.get(item.index as usize) {
                None => problems.push(format!(
                    "emitter lookup {:08x} points at emitter {}, which does not exist",
                    item.key, item.index
                )),
                Some(e) if e.template_id != item.name_hash => problems.push(format!(
                    "emitter lookup {:08x} has name hash {:08x}, emitter {} has {:08x}",
                    item.key, item.name_hash, item.index, e.template_id
                )),
                Some(_) => {}
            }
            if !seen.insert(item.name_hash) {
                problems.push(format!(
                    "two emitters have the name hash {:08x}; the game plays the first",
                    item.name_hash
                ));
            }
        }
        for item in &self.instrument_lookup {
            let bank = item.values[0] as usize;
            match self.instrument_banks.get(bank) {
                None => problems.push(format!(
                    "instrument lookup {:08x} names bank {bank}, which does not exist",
                    item.key
                )),
                Some(&count) if item.index >= count => problems.push(format!(
                    "instrument lookup {:08x} names instrument {} of bank {bank}, which has {count}",
                    item.key, item.index
                )),
                Some(_) => {}
            }
        }
        let samples = self.samples.len();
        for (i, instrument) in self.instruments.iter().enumerate() {
            if instrument.tones.is_empty() {
                problems.push(format!("instrument {i} has no tones"));
            }
            for (k, tone) in instrument.tones.iter().enumerate() {
                if tone.sample as usize >= samples {
                    problems.push(format!(
                        "tone {k} of instrument {i} plays sample {}, the bank has {samples}",
                        tone.sample
                    ));
                }
            }
        }
        for (i, sample) in self.samples.iter().enumerate() {
            if sample.index as usize != i {
                problems.push(format!("sample {i} is numbered {}", sample.index));
            }
            if Sample::parse_inline(&sample.bytes, 0).is_err() {
                problems.push(format!("sample {i} is not an SNR sample"));
            }
        }
        for (i, e) in self.emitters.iter().enumerate() {
            let label = self.emitter_label(i);
            let t = &e.track;
            let tones = match self.instrument_of(i) {
                Some(ins) => Some(self.instruments[ins].tones.len()),
                None if NO_INSTRUMENT.contains(&t.instrument) => None,
                None => {
                    problems.push(format!(
                        "emitter {label}: instrument {:08x} is not in the bank; the game drops the emitter",
                        t.instrument
                    ));
                    None
                }
            };
            if t.variations.len() > 255 {
                problems.push(format!("emitter {label}: more than 255 variations"));
            }
            for (name, value) in [
                ("volume", t.volume),
                ("pitch range", t.pitch_range),
                ("attenuation range", t.attn_range),
            ] {
                if !(0.0..=1.0).contains(&value) {
                    problems.push(format!("emitter {label}: {name} {value} is outside 0–1"));
                }
            }
            for (v, variation) in t.variations.iter().enumerate() {
                if variation.commands.len() != usize::from(t.commands_per_variation) {
                    problems.push(format!(
                        "emitter {label}: variation {v} has {} commands, the track {}",
                        variation.commands.len(),
                        t.commands_per_variation
                    ));
                }
                for c in &variation.commands {
                    if c.id >= cmd::COUNT {
                        problems.push(format!("emitter {label}: command id {}", c.id));
                    }
                    if c.id == cmd::KEY_ON {
                        if !(0..=127).contains(&c.arg32) {
                            problems.push(format!(
                                "emitter {label}: velocity {} is outside 0–127",
                                c.arg32
                            ));
                        }
                        if let Some(n) = tones
                            && (c.arg8 < 0 || c.arg8 as usize >= n)
                        {
                            problems.push(format!(
                                "emitter {label}: variation {v} plays tone {}, the instrument has {n}",
                                c.arg8
                            ));
                        }
                    }
                }
            }
        }
        problems
    }

    /// Appends a sample (SNR header and SNS blocks) to `sdat`; returns its number.
    pub fn add_sample(&mut self, bytes: Vec<u8>) -> Result<u32> {
        Sample::parse_inline(&bytes, 0)?;
        let index = self.samples.len() as u32;
        self.samples.push(BankSample {
            index,
            hash: SAMPLE_HASH,
            pointer: SAMPLE_POINTER,
            bytes,
        });
        Ok(index)
    }

    /// Adds a sound to an emitter: the sample, a tone of its instrument (the
    /// envelope copied from the instrument's first tone) and a variation that
    /// plays the tone (the velocity and auto-duck times of the first
    /// variation). Returns the new variation's number.
    pub fn add_variation(&mut self, emitter: usize, sample: Vec<u8>) -> Result<usize> {
        let ins = self.instrument_of(emitter).ok_or_else(|| {
            Error::InvalidArgument(format!(
                "emitter {} has no instrument in this bank",
                self.emitter_label(emitter)
            ))
        })?;
        let track = &self.emitters[emitter].track;
        if track.variations.len() >= 255 {
            return Err(Error::InvalidArgument(
                "the emitter has 255 variations, the most a track can hold".into(),
            ));
        }
        let tones = self.instruments[ins].tones.len();
        if tones >= 128 {
            return Err(Error::InvalidArgument(
                "the instrument has 128 tones, the most a command can address".into(),
            ));
        }
        let velocity = first_velocity(track).unwrap_or(127);
        let (duck_start, duck_stop) = track
            .variations
            .first()
            .map_or((0, u32::MAX), |v| (v.duck_start, v.duck_stop));
        let number = self.add_sample(sample)?;
        let template = self.instruments[ins]
            .tones
            .first()
            .cloned()
            .unwrap_or_else(default_tone);
        self.instruments[ins].tones.push(Tone {
            sample: number,
            ..template
        });
        let track = &mut self.emitters[emitter].track;
        let mut commands = vec![Command::key_on(tones as u8, velocity)];
        pad_commands(track, &mut commands);
        track.variations.push(Variation {
            duck_start,
            duck_stop,
            commands,
        });
        Ok(track.variations.len() - 1)
    }

    /// Adds an emitter named `name` (`.emx` appended when missing) that plays
    /// `samples`, one variation each, with the parameters of emitter
    /// `template`. Returns the new emitter's number.
    pub fn add_emitter(
        &mut self,
        name: &str,
        template: usize,
        samples: Vec<Vec<u8>>,
    ) -> Result<usize> {
        let name = emx_name(name);
        let hash = name_hash(&name);
        if samples.is_empty() || samples.len() > 128 {
            return Err(Error::InvalidArgument(
                "an emitter needs 1 to 128 sounds".into(),
            ));
        }
        if self.emitters.iter().any(|e| e.template_id == hash)
            || self.instrument_lookup.iter().any(|i| i.key == hash)
        {
            return Err(Error::InvalidArgument(format!(
                "the bank already has an emitter {name} ({hash:08x})"
            )));
        }
        let source =
            self.emitters.get(template).cloned().ok_or_else(|| {
                Error::InvalidArgument(format!("the bank has no emitter {template}"))
            })?;
        let tone = self
            .instrument_of(template)
            .and_then(|i| self.instruments[i].tones.first().cloned())
            .unwrap_or_else(default_tone);
        let velocity = first_velocity(&source.track).unwrap_or(127);
        let (duck_start, duck_stop) = source
            .track
            .variations
            .first()
            .map_or((0, u32::MAX), |v| (v.duck_start, v.duck_stop));
        for s in &samples {
            Sample::parse_inline(s, 0)?;
        }
        let mut tones = Vec::with_capacity(samples.len());
        let mut variations = Vec::with_capacity(samples.len());
        for (k, bytes) in samples.into_iter().enumerate() {
            let number = self.add_sample(bytes)?;
            tones.push(Tone {
                sample: number,
                ..tone.clone()
            });
            variations.push(Variation {
                duck_start,
                duck_stop,
                commands: vec![Command::key_on(k as u8, velocity)],
            });
        }
        let bank = self.instrument_banks.len() as u32;
        self.instrument_banks.push(1);
        self.instruments.push(Instrument {
            flags: 0,
            pad: 0,
            tones,
        });
        insert_sorted(
            &mut self.instrument_lookup,
            LookupItem {
                key: hash,
                index: 0,
                values: [bank, 0, 0],
                name_hash: hash,
            },
        );
        let number = self.emitters.len();
        let mut track = source.track;
        track.commands_per_variation = 1;
        track.last_variation = 0xFF;
        track.instrument = hash;
        track.variations = variations;
        self.emitters.push(Emitter {
            template_id: hash,
            poly_cur: 0,
            track,
            ..source
        });
        let key = self.new_emitter_key(hash);
        insert_sorted(
            &mut self.emitter_lookup,
            LookupItem {
                key,
                index: number as u32,
                values: [0; 3],
                name_hash: hash,
            },
        );
        if let Some(names) = &mut self.name_table {
            let mut record = name.into_bytes();
            record.resize(NAME_SIZE, 0);
            record[NAME_SIZE - 1] = 0;
            names.push(record);
        }
        Ok(number)
    }

    /// A key for a new emitter lookup entry: the keys of the game are
    /// multiples of 4 (folded asset GUIDs); start from the name hash and take
    /// the first free one.
    fn new_emitter_key(&self, hash: u32) -> u32 {
        let used: HashSet<u32> = self.emitter_lookup.iter().map(|i| i.key).collect();
        let mut key = hash & !3;
        while used.contains(&key) {
            key = key.wrapping_add(4);
        }
        key
    }

    /// Sets a fixed pitch (`ratio`, 1 = as recorded) for every variation of
    /// an emitter: updates its pitch bend commands, or adds one before the
    /// first key on (taking a padding command when there is one, otherwise
    /// growing every variation by one command).
    pub fn set_pitch(&mut self, emitter: usize, ratio: f32) -> Result<()> {
        if !(ratio > 0.0 && ratio <= 16.0) {
            return Err(Error::InvalidArgument(format!(
                "pitch {ratio} is outside 0–16"
            )));
        }
        let track = &mut self
            .emitters
            .get_mut(emitter)
            .ok_or_else(|| Error::InvalidArgument(format!("no emitter {emitter}")))?
            .track;
        let bend = Command::pitch_bend(ratio);
        let need_slot = track.variations.iter().any(|v| {
            !v.commands.iter().any(|c| c.id == cmd::PITCH_BEND)
                && !v.commands.first().is_some_and(|c| *c == Command::PAD)
        });
        if need_slot {
            if track.commands_per_variation == u8::MAX {
                return Err(Error::InvalidArgument(
                    "the variations have 255 commands, no room for a pitch command".into(),
                ));
            }
            track.commands_per_variation += 1;
            for v in &mut track.variations {
                v.commands.insert(0, Command::PAD);
            }
        }
        for v in &mut track.variations {
            let mut found = false;
            for c in v.commands.iter_mut().filter(|c| c.id == cmd::PITCH_BEND) {
                c.arg32 = bend.arg32;
                found = true;
            }
            if found {
                continue;
            }
            // Leading padding: the last pad before the first real command.
            let pads = v
                .commands
                .iter()
                .take_while(|c| **c == Command::PAD)
                .count();
            if pads > 0 {
                v.commands[pads - 1] = bend;
            }
        }
        Ok(())
    }

    /// Sets the velocity of every key on of an emitter (0–127).
    pub fn set_velocity(&mut self, emitter: usize, velocity: i32) -> Result<()> {
        if !(0..=127).contains(&velocity) {
            return Err(Error::InvalidArgument(format!(
                "velocity {velocity} is outside 0–127"
            )));
        }
        let track = &mut self
            .emitters
            .get_mut(emitter)
            .ok_or_else(|| Error::InvalidArgument(format!("no emitter {emitter}")))?
            .track;
        for c in track
            .variations
            .iter_mut()
            .flat_map(|v| &mut v.commands)
            .filter(|c| c.id == cmd::KEY_ON)
        {
            c.arg32 = velocity;
        }
        Ok(())
    }

    /// Removes variation `variation` of an emitter. Tones and samples stay.
    pub fn remove_variation(&mut self, emitter: usize, variation: usize) -> Result<()> {
        let track = &mut self
            .emitters
            .get_mut(emitter)
            .ok_or_else(|| Error::InvalidArgument(format!("no emitter {emitter}")))?
            .track;
        if variation >= track.variations.len() || track.variations.len() == 1 {
            return Err(Error::InvalidArgument(format!(
                "cannot remove variation {variation} of {}",
                track.variations.len()
            )));
        }
        track.variations.remove(variation);
        Ok(())
    }

    /// Pitch of a variation set by its pitch bend command, if it has one.
    pub fn variation_pitch(variation: &Variation) -> Option<f32> {
        variation
            .commands
            .iter()
            .find(|c| c.id == cmd::PITCH_BEND)
            .map(|c| c.arg32 as f32 / 65536.0)
    }

    /// Tones a variation keys on.
    pub fn variation_tones(variation: &Variation) -> Vec<usize> {
        variation
            .commands
            .iter()
            .filter(|c| c.id == cmd::KEY_ON && c.arg8 >= 0)
            .map(|c| c.arg8 as usize)
            .collect()
    }
}

/// `name` with `.emx` appended when it has no extension, lower case.
pub fn emx_name(name: &str) -> String {
    let name = name.trim().to_ascii_lowercase();
    if name.ends_with(".emx") {
        name
    } else {
        format!("{name}.emx")
    }
}

/// Velocity of the first key on of a track.
fn first_velocity(track: &Track) -> Option<i32> {
    track
        .variations
        .iter()
        .flat_map(|v| &v.commands)
        .find(|c| c.id == cmd::KEY_ON && c.arg32 > 0)
        .map(|c| c.arg32)
}

/// Brings `commands` and the track's variations to one command count,
/// padding at the start as the game's banks do.
fn pad_commands(track: &mut Track, commands: &mut Vec<Command>) {
    let want = usize::from(track.commands_per_variation).max(commands.len());
    if want > usize::from(track.commands_per_variation) {
        let grow = want - usize::from(track.commands_per_variation);
        for v in &mut track.variations {
            v.commands
                .splice(0..0, std::iter::repeat_n(Command::PAD, grow));
        }
        track.commands_per_variation = want as u8;
    }
    let pad = want - commands.len();
    commands.splice(0..0, std::iter::repeat_n(Command::PAD, pad));
}

fn insert_sorted(table: &mut Vec<LookupItem>, item: LookupItem) {
    let at = table.partition_point(|i| i.key < item.key);
    table.insert(at, item);
}

/// The envelope most tones of the game have: full volume, no segments,
/// a 10 ms release.
fn default_tone() -> Tone {
    Tone {
        pre_volume: 1.0,
        segments: [(1.0, 0.0); 16],
        release: (0.0, 0.01),
        segment_count: 0,
        sample: 0,
    }
}

#[cfg(any(test, feature = "testing"))]
pub(crate) mod samples {
    use super::*;
    use crate::resource::audio_encode::{SampleSpec, build_inline};
    use crate::resource::video::input::Pcm;

    pub(crate) fn sample(frames: usize) -> Vec<u8> {
        let pcm = Pcm {
            channels: 1,
            sample_rate: 22050,
            samples: (0..frames).map(|i| ((i * 37) % 2000) as i16).collect(),
        };
        let spec = SampleSpec {
            channels: 1,
            sample_rate: 22050,
            kind: 0,
            loop_start: None,
        };
        build_inline(&pcm, &spec).unwrap()
    }

    /// A bank of two emitters (`a.emx` with two variations, `b.emx` with
    /// one) and three samples.
    pub(crate) fn bank() -> SoundBank {
        let tone = |s| Tone {
            sample: s,
            ..default_tone()
        };
        let track = |instrument, tones: &[u8]| Track {
            voice_poly: 1,
            select: 3,
            commands_per_variation: 1,
            last_variation: 0xFF,
            priority: 0,
            pad: [0; 2],
            flags: 0x20,
            distance_factor: 1.0,
            pitch_range: 0.1,
            attn_range: 0.0,
            offset_range: 0.0,
            doppler: 0.0,
            volume: 0.8,
            azimuth: 0.0,
            instrument,
            variations: tones
                .iter()
                .map(|&t| Variation {
                    duck_start: 0,
                    duck_stop: u32::MAX,
                    commands: vec![Command::key_on(t, 100)],
                })
                .collect(),
        };
        let a = name_hash("a.emx");
        let b = name_hash("b.emx");
        let emitter = |id, track| Emitter {
            template_id: id,
            priority: 100,
            poly_max: 2,
            poly_cur: 0,
            flags: flag::THREE_D,
            falloff: 30.0,
            begin_falloff: 0.0,
            inv_falloff: 0.0,
            begin_inv_falloff: 0.0,
            mix_category: 0x1234_5678,
            duck_category: 0,
            track,
        };
        let mut lookup = vec![
            LookupItem {
                key: 0x100,
                index: 0,
                values: [0; 3],
                name_hash: a,
            },
            LookupItem {
                key: 0x200,
                index: 1,
                values: [0; 3],
                name_hash: b,
            },
        ];
        lookup.sort_by_key(|i| i.key);
        let mut ins_lookup = vec![
            LookupItem {
                key: a,
                index: 0,
                values: [0, 0, 0],
                name_hash: a,
            },
            LookupItem {
                key: b,
                index: 0,
                values: [1, 0, 0],
                name_hash: b,
            },
        ];
        ins_lookup.sort_by_key(|i| i.key);
        SoundBank {
            magic: u32::from_le_bytes(*b"sbnk"),
            version: VERSION,
            session: Session {
                version: VERSION,
                kind: 0,
                samples: 0,
                guid: 0x5555,
                reserved: [0; 2],
            },
            emitters: vec![emitter(a, track(a, &[0, 1])), emitter(b, track(b, &[0]))],
            instrument_banks: vec![1, 1],
            instruments: vec![
                Instrument {
                    flags: 0,
                    pad: 0,
                    tones: vec![tone(0), tone(1)],
                },
                Instrument {
                    flags: 0,
                    pad: 0,
                    tones: vec![tone(2)],
                },
            ],
            emitter_lookup: lookup,
            instrument_lookup: ins_lookup,
            name_table: None,
            sdat_magic: u32::from_le_bytes(*b"sdat"),
            samples: (0..3)
                .map(|i| BankSample {
                    index: i,
                    hash: SAMPLE_HASH,
                    pointer: SAMPLE_POINTER,
                    bytes: sample(300 + 100 * i as usize),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::samples::{bank, sample};
    use super::*;

    #[test]
    fn bank_round_trips() {
        let b = bank();
        assert!(b.validate().is_empty(), "{:?}", b.validate());
        let bytes = b.to_bytes();
        assert_eq!(&bytes[..4], b"sbnk");
        assert_eq!(
            u32::from_le_bytes(bytes[0x10..0x14].try_into().unwrap()) % 0x40,
            0
        );
        let parsed = SoundBank::parse(&bytes).unwrap();
        assert_eq!(parsed, b);
        assert_eq!(parsed.to_bytes(), bytes);
        // The old sample reader sees the same samples.
        let sounds = crate::resource::audio::parse_sbk(&bytes).unwrap();
        assert_eq!(sounds.len(), 3);
        assert_eq!(parsed.instrument_of(1), Some(1));
        assert_eq!(parsed.find_emitter("a"), Some(0));
        assert_eq!(parsed.find_emitter("B.EMX"), Some(1));
        assert_eq!(parsed.find_emitter("#1"), Some(1));
    }

    #[test]
    fn variation_and_emitter_are_added() {
        let mut b = bank();
        let v = b.add_variation(1, sample(500)).unwrap();
        assert_eq!(v, 1);
        assert_eq!(b.instruments[1].tones.len(), 2);
        assert_eq!(b.instruments[1].tones[1].sample, 3);
        assert_eq!(b.emitters[1].track.variations[1].commands[0].arg8, 1);
        let e = b
            .add_emitter("new_sound", 0, vec![sample(200), sample(250)])
            .unwrap();
        assert_eq!(e, 2);
        assert!(b.validate().is_empty(), "{:?}", b.validate());
        assert_eq!(b.find_emitter("new_sound.emx"), Some(2));
        assert_eq!(b.instrument_of(2), Some(2));
        assert_eq!(b.instruments[2].tones[1].sample, 5);
        assert!(b.add_emitter("new_sound", 0, vec![sample(10)]).is_err());
        let bytes = b.to_bytes();
        let parsed = SoundBank::parse(&bytes).unwrap();
        assert_eq!(parsed, b);
        assert!(parsed.validate().is_empty());
        assert_eq!(crate::resource::audio::parse_sbk(&bytes).unwrap().len(), 6);
    }

    #[test]
    fn padding_grows_every_variation() {
        let mut b = bank();
        b.set_pitch(0, 1.5).unwrap();
        let t = &b.emitters[0].track;
        assert_eq!(t.commands_per_variation, 2);
        for v in &t.variations {
            assert_eq!(v.commands[0].id, cmd::PITCH_BEND);
            assert_eq!(SoundBank::variation_pitch(v), Some(1.5));
            assert_eq!(v.commands[1].id, cmd::KEY_ON);
        }
        // A second change updates the commands in place.
        b.set_pitch(0, 0.5).unwrap();
        assert_eq!(b.emitters[0].track.commands_per_variation, 2);
        assert_eq!(
            SoundBank::variation_pitch(&b.emitters[0].track.variations[1]),
            Some(0.5)
        );
        // A variation with more commands pads the others at the start.
        let track = &mut b.emitters[1].track;
        let mut commands = vec![Command::key_on(0, 1), Command::key_on(0, 2)];
        pad_commands(track, &mut commands);
        assert_eq!(track.commands_per_variation, 2);
        assert_eq!(track.variations[0].commands[0], Command::PAD);
        b.set_velocity(0, 64).unwrap();
        assert!(b.set_velocity(0, 200).is_err());
        assert!(b.validate().is_empty(), "{:?}", b.validate());
    }

    /// Found by the fuzz target: an instrument without tones.
    #[test]
    fn a_sound_is_added_to_an_instrument_without_tones() {
        let mut b = bank();
        b.instruments[1].tones.clear();
        assert!(b.validate().iter().any(|p| p.contains("no tones")));
        b.add_variation(1, sample(100)).unwrap();
        assert_eq!(b.instruments[1].tones.len(), 1);
        assert_eq!(b.instruments[1].tones[0].segments[0], (1.0, 0.0));
        let bytes = b.to_bytes();
        assert_eq!(SoundBank::parse(&bytes).unwrap().to_bytes(), bytes);
    }

    #[test]
    fn validate_finds_broken_references() {
        let mut b = bank();
        b.emitters[0].track.instrument = 0x1111;
        b.instruments[1].tones[0].sample = 9;
        b.emitter_lookup.swap(0, 1);
        b.emitters[1].track.variations[0].commands[0].arg8 = 5;
        let problems = b.validate();
        assert!(problems.iter().any(|p| p.contains("not in the bank")));
        assert!(problems.iter().any(|p| p.contains("plays sample 9")));
        assert!(problems.iter().any(|p| p.contains("not sorted")));
        assert!(problems.iter().any(|p| p.contains("plays tone 5")));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(SoundBank::parse(b"").is_err());
        assert!(SoundBank::parse(b"sbnk").is_err());
        let mut bytes = bank().to_bytes();
        bytes[4] = 205;
        assert!(SoundBank::parse(&bytes).is_err());
        let good = bank().to_bytes();
        // Every truncation fails cleanly.
        for len in (0..good.len()).step_by(97) {
            let _ = SoundBank::parse(&good[..len]);
        }
        // Huge counts are refused before allocating.
        let mut huge = good.clone();
        huge[BANK_HEADER + 0x38..BANK_HEADER + 0x3C].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(SoundBank::parse(&huge).is_err());
    }
}
