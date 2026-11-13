//! Character animations: the clips inside `bnk` resources.
//!
//! The 2 751 `bnk` resources were taken for sound banks (their `.emx` names
//! are sound events the animations trigger); they are animation banks of the
//! human rig: gameplay moves in `globals_global`, one clip per actor and shot
//! in cut-scene streams (`igc*`), conversation gestures in
//! `globalfolder/dlg`, ambient actions in `globalfolder/interestingobjectanimations`.
//! Layout and codecs come from `godfather2.exe` (loader 0x004621A7, clip
//! preparation 0x00570860, channel decoders 0x00579AA0, 0x0057C590, 0x0057D4E0).
//!
//! ```text
//! bank (relocatable image, version 9; see `resource::image`)
//! 0x08  u32  bank hash
//! 0x0C  u32  skeleton: SDBM of its name ("puppet", the 102-bone human skeleton)
//! 0x20  u32  nodes of the largest entry tree (blend graph: u8 type, u8 children, u16 index; 0x16 = clip node)
//! 0x24  u32, ptr  named entries {ptr name, ptr root node}, registered under SDBM of the name
//! 0x2C  u32, ptr  clips: list of pointers
//! 0x34  u32, ptr  event objects (transition rules; kept as they are by `anim_image`)
//!
//! clip, current layout (u8 kind at +0x04 = 10)
//! +0x00 u32 SDBM of the clip name, +0x08 u32 frames, +0x10 u32 index,
//! +0x14 u32 bone set (hash of a bone set of the rig), +0x18 u32 looping,
//! +0x20 u32 root-motion mask, +0x24 i32 root-motion bone (−1: none),
//! +0x28 u32 0x80000000 | scheme (1, 3, 4), +0x30 u32 count, ptr events
//! {f32 time in ticks (2 per frame), ptr name, u32 signature, ptr arguments};
//! channel streams from +0x40
//! clip, old layout (kind 0; cut-scenes): +0x08 looping, +0x0C mask,
//! +0x10 motion bone, +0x14 bone set, +0x18 frames, +0x20 scheme, streams
//! from +0x30
//! ```
//!
//! For every bone of the bone set, in skeleton order, seven streams follow:
//! rotation x, y, z, the sign of w (w = ±√(1 − x² − y² − z²); 1 means a
//! negative w in scheme 4 and a positive one in scheme 1) and
//! translation x, y, z, in the bone's parent space. A stream starts with a
//! u16: codec in the low 4 bits, frame count above. Values called f24 are the
//! upper 24 bits of an f32.
//!
//! | Codec | Value at local frame t | Size |
//! |---|---|---|
//! | 0–3 | polynomial of degree 0–3 in t, f24 coefficients, highest first | 2 + 3·(c + 1) |
//! | 4 | f32 per frame, linear in between | 2 + 4n |
//! | 5 | n segments sharing their end frames (scheme 4: u16 total size after the header) | |
//! | 6 / 7 | f24 offset, f24 scale, u8 / u16 per frame: q · scale + offset | 2 + 6 + n / 2n |
//! | 0xC / 0xD | 0 / 1 | 2 |
//! | 0xE | one bit per frame, 1 or 0 | 2 + ⌈n / 8⌉ |
//!
//! Scheme 3 (37 clips) has two forms. With the sign bit of the format word
//! set, the streams are laid out as in scheme 4 but the fourth stream of a
//! bone holds `w` itself. Without it the clip stores raw floats: for every
//! frame, for every bone, rotation x, y, z, w, translation x, y, z and one
//! padding float (cut-scene props). Format words come from the evaluator
//! dispatch at 0x00593BC0.

use serde::{Deserialize, Serialize};

use crate::hash::sdbm;
use crate::resource::image::Image;
use crate::resource::rcb::{Rig, Skeleton};
use crate::{Error, Result};

/// Type id of `bnk` resources.
pub const TYPE_BNK: u32 = sdbm(b"bnk");

/// Animation time units per second: the game advances clip time by
/// `dt × 59.94` (constant at 0x011260B0, read by 0x005715D0, 0x0045FA90).
pub const TICKS_PER_SECOND: f32 = 59.94;

/// Ticks per clip frame: the third parameter of every clip node (2.0 in all
/// 7 847 clip nodes of the game); the sampler divides time by it (0x00593770).
pub const TICKS_PER_FRAME: f32 = 2.0;

/// Clip frames per second: 59.94 / 2 = 29.97.
pub const FRAME_RATE: f32 = TICKS_PER_SECOND / TICKS_PER_FRAME;

/// A clip read from an animation file, before it is fitted to a game
/// skeleton: joints with their rest pose and local samples at
/// [`FRAME_RATE`]. glTF import produces it (`import::anim`); other formats
/// (FBX, USD) plug in by producing it too.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SourceAnimation {
    pub name: String,
    /// Frames at [`FRAME_RATE`]; every sampled joint has this many samples.
    pub frames: usize,
    pub joints: Vec<SourceJoint>,
}

/// A joint of a [`SourceAnimation`].
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SourceJoint {
    pub name: String,
    /// Index of the parent joint in [`SourceAnimation::joints`].
    pub parent: Option<usize>,
    /// Rest pose in the parent's space: rotation (x, y, z, w) and translation.
    pub rest_rotation: [f32; 4],
    pub rest_translation: [f32; 3],
    /// Local rotation per frame, `None` when not animated.
    pub rotations: Option<Vec<[f32; 4]>>,
    /// Local translation per frame, `None` when not animated.
    pub translations: Option<Vec<[f32; 3]>>,
}

/// One clip of a bank.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Clip {
    /// Offset of the clip record in the bank.
    pub offset: usize,
    /// SDBM of the clip name.
    pub hash: u32,
    /// The name, when a string of the bank hashes to `hash`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub frames: u32,
    /// Hash of the rig bone set the clip animates.
    pub bone_set: u32,
    /// Compression scheme: 1, 3 or 4.
    pub scheme: u32,
    /// The raw format word: the scheme in the low bits; its sign bit means
    /// rotation and translation streams (scheme 3 without it stores raw floats).
    pub format: u32,
    /// Offset of the first channel stream.
    pub streams_at: usize,
    /// Old record layout (cut-scenes).
    pub old_layout: bool,
    /// Events of the clip (current layout only), such as `audio: play` with a
    /// sound name or `footstep`.
    pub events: Vec<ClipEvent>,
}

/// The value of an event argument.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EventArg {
    Int(i32),
    Float(f32),
    Text(String),
}

/// An event of a clip: `{f32 frame, ptr name, u32 signature, ptr arguments}`,
/// arguments `{u32 tag (1 float, 2 string, else integer), u32 value}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipEvent {
    /// Time in ticks: 2 per clip frame, 59.94 per second.
    pub frame: f32,
    pub name: String,
    /// Unknown; the low byte is the argument count.
    pub signature: u32,
    pub args: Vec<EventArg>,
}

fn parse_events(image: &Image<'_>, count: usize, at: usize) -> Result<Vec<ClipEvent>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    image.array(at, count, 16)?;
    (0..count)
        .map(|i| {
            let e = at + 16 * i;
            let signature = image.u32(e + 8)?;
            let args_at = image.u32(e + 12)? as usize;
            let mut args = Vec::new();
            // Some events carry fewer arguments than the signature claims;
            // stop at the first argument outside the image.
            for k in 0..(signature & 0xFF) as usize {
                let (Ok(tag), Ok(value)) =
                    (image.u32(args_at + 8 * k), image.u32(args_at + 8 * k + 4))
                else {
                    break;
                };
                args.push(match tag {
                    1 => EventArg::Float(f32::from_bits(value)),
                    2 => match image.cstr(value as usize) {
                        Ok(s) => EventArg::Text(s),
                        Err(_) => EventArg::Int(value as i32),
                    },
                    _ => EventArg::Int(value as i32),
                });
            }
            Ok(ClipEvent {
                frame: image.f32(e)?,
                name: image.cstr(image.u32(e + 4)? as usize)?,
                signature,
                args,
            })
        })
        .collect()
}

/// An animation bank.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnimBank {
    pub hash: u32,
    /// SDBM of the skeleton name the bank animates.
    pub skeleton: u32,
    pub clips: Vec<Clip>,
    /// Strings of the bank (clip, node and sound event names).
    pub names: Vec<String>,
}

/// Rotation (x, y, z, w) and translation of one bone at every frame.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoneTrack {
    /// Skeleton bone index.
    pub bone: usize,
    pub rotations: Vec<[f32; 4]>,
    pub translations: Vec<[f32; 3]>,
}

impl AnimBank {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let image = Image::parse(data, "bnk")?;
        if image.version() != 9 {
            return Err(Error::UnsupportedVersion {
                what: "bnk",
                version: image.version(),
            });
        }
        let mut names: Vec<String> = image
            .relocations()
            .into_iter()
            .filter_map(|field| image.u32(field).ok().map(|t| t as usize))
            .filter_map(|t| {
                let s = crate::io::cstr(&data[..image.table_at], t).ok()?;
                (s.len() >= 2 && s.iter().all(|b| b.is_ascii_graphic()))
                    .then(|| String::from_utf8_lossy(s).into_owned())
            })
            .collect();
        names.sort();
        names.dedup();
        let by_hash: std::collections::HashMap<u32, &str> = names
            .iter()
            .map(|n| (sdbm(n.to_ascii_lowercase().as_bytes()), n.as_str()))
            .collect();
        let count = image.u32(0x2C)? as usize;
        let list = image.u32(0x30)? as usize;
        image.array(list, count, 4)?;
        let mut clips = Vec::with_capacity(count);
        for k in 0..count {
            let at = image.u32(list + 4 * k)? as usize;
            let kind = image.u32(at + 4)? & 0xFF;
            let old_layout = kind < 10;
            let hash = image.u32(at)?;
            let (frames, scheme, streams_at) = if old_layout {
                (image.u32(at + 0x18)?, image.u32(at + 0x20)?, at + 0x30)
            } else {
                (image.u32(at + 8)?, image.u32(at + 0x28)?, at + 0x40)
            };
            clips.push(Clip {
                offset: at,
                hash,
                name: by_hash.get(&hash).map(|s| (*s).to_owned()),
                frames,
                bone_set: image.u32(at + 0x14)?,
                scheme: scheme & 0x0FFF_FFFF,
                format: scheme,
                streams_at,
                old_layout,
                events: if old_layout {
                    Vec::new()
                } else {
                    parse_events(
                        &image,
                        image.u32(at + 0x30)? as usize,
                        image.u32(at + 0x34)? as usize,
                    )?
                },
            });
        }
        Ok(Self {
            hash: image.u32(8)?,
            skeleton: image.u32(0x0C)?,
            clips,
            names,
        })
    }
}

/// The skeleton a bank animates: the skeleton of any rig whose name hash is
/// [`AnimBank::skeleton`].
pub fn find_skeleton(rigs: &[Rig], hash: u32) -> Option<&Skeleton> {
    rigs.iter()
        .flat_map(|r| &r.skeletons)
        .find(|s| s.name_hash == hash)
}

/// The bones (skeleton indices, ascending) of the bone set `hash` of any rig.
pub fn bone_set(rigs: &[Rig], hash: u32) -> Option<&[usize]> {
    rigs.iter()
        .flat_map(|r| &r.bone_sets)
        .find(|s| s.hash == hash)
        .map(|s| s.bones.as_slice())
}

impl Clip {
    /// The clip name, or its hash in hexadecimal.
    pub fn display_name(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("{:08x}", self.hash))
    }
}

fn f24(data: &[u8], at: usize) -> Result<f32> {
    let b = crate::io::slice(data, at, 3)?;
    Ok(f32::from_bits(
        (b[0] as u32) << 8 | (b[1] as u32) << 16 | (b[2] as u32) << 24,
    ))
}

fn header(data: &[u8], at: usize) -> Result<(u8, usize)> {
    let b = crate::io::slice(data, at, 2)?;
    Ok((b[0] & 0xF, ((b[1] as usize) << 4) | (b[0] >> 4) as usize))
}

fn bad(at: usize, what: &str) -> Error {
    Error::malformed("bnk clip", at, what)
}

/// A decoded channel stream: a list of segments `(offset, codec, frames)`.
pub(crate) struct Stream {
    pub(crate) segments: Vec<(usize, u8, usize)>,
    pub(crate) len: usize,
    /// Clip frame where each segment ends (segments share boundary frames).
    ends: Vec<usize>,
}

fn simple_len(codec: u8, n: usize, at: usize) -> Result<usize> {
    Ok(match codec {
        0..=3 => 2 + 3 * (codec as usize + 1),
        4 => 2 + 4 * n,
        6 => 8 + n,
        7 => 8 + 2 * n,
        0xC | 0xD => 2,
        0xE => 2 + n.div_ceil(8),
        _ => return Err(bad(at, "unknown channel codec")),
    })
}

pub(crate) fn stream(data: &[u8], at: usize, scheme: u32, depth: usize) -> Result<Stream> {
    let (segments, len) = stream_parts(data, at, scheme, depth)?;
    let mut end = 0;
    let ends = segments
        .iter()
        .map(|s| {
            end += s.2.saturating_sub(1);
            end
        })
        .collect();
    Ok(Stream {
        segments,
        len,
        ends,
    })
}

type Segments = (Vec<(usize, u8, usize)>, usize);

fn stream_parts(data: &[u8], at: usize, scheme: u32, depth: usize) -> Result<Segments> {
    let (codec, n) = header(data, at)?;
    if codec != 5 {
        return Ok((vec![(at, codec, n)], simple_len(codec, n, at)?));
    }
    if depth > 2 || n == 0 {
        return Err(bad(at, "bad piecewise stream"));
    }
    let mut q = at + if scheme == 4 { 4 } else { 2 };
    let mut segments = Vec::new();
    for _ in 0..n {
        let (inner, inner_len) = stream_parts(data, q, scheme, depth + 1)?;
        q += inner_len;
        segments.extend(inner);
    }
    let len = if scheme == 4 {
        crate::io::read_u16(data, at + 2, crate::io::Endian::Little)? as usize
    } else {
        q - at
    };
    if len < 4 {
        return Err(bad(at, "piecewise stream shorter than its header"));
    }
    Ok((segments, len))
}

/// Bytes of the channel streams of `clip` for `bones` bones.
pub fn streams_len(clip: &Clip, data: &[u8], bones: usize) -> Result<usize> {
    let frames = clip.frames.clamp(1, 100_000) as usize;
    if clip.scheme == 3 && clip.format & 0x8000_0000 == 0 {
        return Ok(frames * bones * 32);
    }
    let layout = if clip.scheme == 1 { 1 } else { 4 };
    let mut at = clip.streams_at;
    for _ in 0..7 * bones {
        at += stream(data, at, layout, 0)?.len;
    }
    Ok(at - clip.streams_at)
}

/// Length of the channel streams of a clip that starts at `start` and ends
/// before `limit`: whole bones (seven streams each) whose streams parse and
/// span the clip. `None` when no stream parses.
pub(crate) fn layout_streams_len(
    data: &[u8],
    start: usize,
    limit: usize,
    format: u32,
    frames: u32,
) -> Option<usize> {
    let scheme = format & 0x0FFF_FFFF;
    let frames = frames.clamp(1, 100_000) as usize;
    if scheme == 3 && format & 0x8000_0000 == 0 {
        let unit = frames * 32;
        return Some((limit.saturating_sub(start)) / unit * unit);
    }
    let layout = if scheme == 1 { 1 } else { 4 };
    let data = data.get(..limit)?;
    let mut at = start;
    let mut bone_end = None;
    let mut count = 0;
    while let Ok(s) = stream(data, at, layout, 0) {
        if at + s.len > limit || (s.frames() != frames && s.frames() > 1) {
            break;
        }
        at += s.len;
        count += 1;
        if count % 7 == 0 {
            bone_end = Some(at - start);
        }
    }
    bone_end
}

fn eval_simple(data: &[u8], at: usize, codec: u8, n: usize, t: f32) -> Result<f32> {
    let q = at + 2;
    Ok(match codec {
        0..=3 => {
            let mut v = f24(data, q)?;
            for i in 1..=codec as usize {
                v = v * t + f24(data, q + 3 * i)?;
            }
            v
        }
        4 | 6 | 7 => {
            let last = n.saturating_sub(1);
            let i = (t.max(0.0) as usize).min(last);
            let j = (i + 1).min(last);
            let fr = (t - i as f32).clamp(0.0, 1.0);
            let value = |k: usize| -> Result<f32> {
                Ok(match codec {
                    4 => f32::from_bits(crate::io::read_u32(
                        data,
                        q + 4 * k,
                        crate::io::Endian::Little,
                    )?),
                    6 => {
                        let v = crate::io::slice(data, q + 6 + k, 1)?[0] as f32;
                        v * f24(data, q + 3)? + f24(data, q)?
                    }
                    _ => {
                        let v = crate::io::read_u16(data, q + 6 + 2 * k, crate::io::Endian::Little)?
                            as f32;
                        v * f24(data, q + 3)? + f24(data, q)?
                    }
                })
            };
            let (a, b) = (value(i)?, value(j)?);
            a + (b - a) * fr
        }
        0xC => 0.0,
        0xD => 1.0,
        0xE => {
            let i = (t.max(0.0) as usize).min(n.saturating_sub(1));
            let byte = crate::io::slice(data, q + i / 8, 1)?[0];
            (byte >> (i % 8) & 1) as f32
        }
        _ => return Err(bad(at, "unknown channel codec")),
    })
}

impl Stream {
    /// Value at clip frame `t`; segments share their boundary frames (a
    /// boundary frame belongs to the earlier segment).
    pub(crate) fn eval(&self, data: &[u8], t: f32) -> Result<f32> {
        let last = self.segments.len() - 1;
        // The first segment whose end is not before `t`.
        let k = self.ends.partition_point(|&e| (e as f32) < t).min(last);
        let (at, codec, n) = self.segments[k];
        let start = self.ends[k] - n.saturating_sub(1);
        eval_simple(data, at, codec, n, t - start as f32)
    }

    pub(crate) fn frames(&self) -> usize {
        if self.segments.len() == 1 {
            self.segments[0].2
        } else {
            self.segments
                .iter()
                .map(|s| s.2.saturating_sub(1))
                .sum::<usize>()
                + 1
        }
    }
}

impl Clip {
    /// Decodes the tracks of the clip for the given bones of the bone set
    /// (skeleton indices in ascending order).
    pub fn decode(&self, data: &[u8], bones: &[usize]) -> Result<Vec<BoneTrack>> {
        let raw = self.scheme == 3 && self.format & 0x8000_0000 == 0;
        if raw {
            return self.decode_raw(data, bones);
        }
        if !matches!(self.scheme, 1 | 3 | 4) {
            return Err(bad(self.offset, "unknown compression scheme"));
        }
        // Scheme 3 streams are laid out like scheme 4 (piecewise streams with
        // a byte length) but store `w` itself instead of its sign.
        let layout = if self.scheme == 1 { 1 } else { 4 };
        let stored_w = self.scheme == 3;
        let frames = self.frames.clamp(1, 100_000) as usize;
        let mut at = self.streams_at;
        let mut tracks = Vec::with_capacity(bones.len());
        for &bone in bones {
            let mut channels = Vec::with_capacity(7);
            for _ in 0..7 {
                let s = stream(data, at, layout, 0)?;
                if s.frames() != frames && s.frames() > 1 {
                    return Err(bad(at, "channel length differs from the clip"));
                }
                at += s.len;
                channels.push(s);
            }
            let mut rotations = Vec::with_capacity(frames);
            let mut translations = Vec::with_capacity(frames);
            for f in 0..frames {
                let t = f as f32;
                let v = |c: usize| channels[c].eval(data, t);
                let (x, y, z) = (v(0)?, v(1)?, v(2)?);
                if stored_w {
                    rotations.push(normalized([x, y, z, v(3)?]));
                } else {
                    let mut w = (1.0 - x * x - y * y - z * z).max(0.0).sqrt();
                    // The sign stream holds 1 for a negative `w` in scheme 4
                    // and for a positive one in scheme 1: read the other way,
                    // every scheme 1 clip starts far from the bind pose
                    // (corpus: all 1 466 are nearer to it as written here).
                    if (v(3)? != 0.0) != (self.scheme == 1) {
                        w = -w;
                    }
                    rotations.push([x, y, z, w]);
                }
                translations.push([v(4)?, v(5)?, v(6)?]);
            }
            tracks.push(BoneTrack {
                bone,
                rotations,
                translations,
            });
        }
        Ok(tracks)
    }
}

impl Clip {
    /// Scheme 3 without the stream flag: for every frame, for every bone,
    /// eight `f32`: rotation x, y, z, w, translation x, y, z and padding.
    fn decode_raw(&self, data: &[u8], bones: &[usize]) -> Result<Vec<BoneTrack>> {
        const FLOATS: usize = 8;
        let frames = self.frames.clamp(1, 100_000) as usize;
        let size = frames * bones.len() * FLOATS * 4;
        let all = crate::io::slice(data, self.streams_at, size)?;
        let f = |i: usize| f32::from_le_bytes(all[4 * i..4 * i + 4].try_into().unwrap());
        Ok(bones
            .iter()
            .enumerate()
            .map(|(k, &bone)| {
                let mut rotations = Vec::with_capacity(frames);
                let mut translations = Vec::with_capacity(frames);
                for frame in 0..frames {
                    let b = (frame * bones.len() + k) * FLOATS;
                    rotations.push([f(b), f(b + 1), f(b + 2), f(b + 3)]);
                    translations.push([f(b + 4), f(b + 5), f(b + 6)]);
                }
                BoneTrack {
                    bone,
                    rotations,
                    translations,
                }
            })
            .collect())
    }
}

/// A quaternion scaled to unit length (unchanged when it is zero).
fn normalized(q: [f32; 4]) -> [f32; 4] {
    let n = q.iter().map(|v| v * v).sum::<f32>().sqrt();
    if n > 1e-6 { q.map(|v| v / n) } else { q }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f24_bytes(v: f32) -> [u8; 3] {
        let b = v.to_bits().to_le_bytes();
        [b[1], b[2], b[3]]
    }

    #[test]
    fn decodes_every_codec() {
        let mut d = Vec::new();
        // Constant 0.5 over 3 frames.
        d.extend_from_slice(&[0x30, 0x00]);
        d.extend_from_slice(&f24_bytes(0.5));
        // Linear 2t + 1.
        d.extend_from_slice(&[0x31, 0x00]);
        d.extend_from_slice(&f24_bytes(2.0));
        d.extend_from_slice(&f24_bytes(1.0));
        // f32 per frame.
        d.extend_from_slice(&[0x34, 0x00]);
        for v in [0.0f32, 1.0, 4.0] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        // u8 quantised: offset 1, scale 0.5.
        d.extend_from_slice(&[0x36, 0x00]);
        d.extend_from_slice(&f24_bytes(1.0));
        d.extend_from_slice(&f24_bytes(0.5));
        d.extend_from_slice(&[0, 2, 4]);
        // Bits 1, 0, 1.
        d.extend_from_slice(&[0x3E, 0x00, 0b101]);
        // Zero and one.
        d.extend_from_slice(&[0x3C, 0x00, 0x3D, 0x00]);
        let mut at = 0;
        let mut values = Vec::new();
        for _ in 0..7 {
            let s = stream(&d, at, 4, 0).unwrap();
            assert_eq!(s.frames(), 3);
            values.push(
                (0..3)
                    .map(|f| s.eval(&d, f as f32).unwrap())
                    .collect::<Vec<_>>(),
            );
            at += s.len;
        }
        assert_eq!(at, d.len());
        assert_eq!(values[0], [0.5; 3]);
        assert_eq!(values[1], [1.0, 3.0, 5.0]);
        assert_eq!(values[2], [0.0, 1.0, 4.0]);
        assert_eq!(values[3], [1.0, 2.0, 3.0]);
        assert_eq!(values[4], [1.0, 0.0, 1.0]);
        assert_eq!(values[5], [0.0; 3]);
        assert_eq!(values[6], [1.0; 3]);
        // Piecewise: two linear segments of 2 frames share frame 1 → 3 frames.
        let mut p = vec![0x25, 0x00, 0, 0];
        for (a, b) in [(1.0f32, 0.0f32), (2.0, 1.0)] {
            p.extend_from_slice(&[0x21, 0x00]);
            p.extend_from_slice(&f24_bytes(a));
            p.extend_from_slice(&f24_bytes(b));
        }
        let len = p.len() as u16;
        p[2..4].copy_from_slice(&len.to_le_bytes());
        let s = stream(&p, 0, 4, 0).unwrap();
        assert_eq!((s.frames(), s.len), (3, p.len()));
        assert_eq!(s.eval(&p, 2.0).unwrap(), 3.0);
        assert!(stream(&[0x3F, 0], 0, 4, 0).is_err());
        // A piecewise stream claiming zero bytes (found by fuzzing: the
        // stream scan of the writer looped on it).
        let zero = [0x15, 0x00, 0, 0, 0x1C, 0x00];
        assert!(stream(&zero, 0, 4, 0).is_err());
        assert_eq!(
            layout_streams_len(&zero, 0, zero.len(), 0x8000_0004, 1),
            None
        );
    }

    fn clip(scheme: u32, format: u32, frames: u32) -> Clip {
        Clip {
            offset: 0,
            hash: 0,
            name: None,
            frames,
            bone_set: 0,
            scheme,
            format,
            streams_at: 0,
            old_layout: false,
            events: Vec::new(),
        }
    }

    #[test]
    fn decodes_scheme_3() {
        // Raw floats: 2 frames, 1 bone, 8 floats each.
        let mut d = Vec::new();
        for f in 0..2 {
            for v in [0.0f32, 0.0, 0.0, 1.0, f as f32, 2.0, 3.0, 0.0] {
                d.extend_from_slice(&v.to_le_bytes());
            }
        }
        let tracks = clip(3, 3, 2).decode(&d, &[5]).unwrap();
        assert_eq!(tracks[0].bone, 5);
        assert_eq!(tracks[0].rotations, [[0.0, 0.0, 0.0, 1.0]; 2]);
        assert_eq!(tracks[0].translations[1], [1.0, 2.0, 3.0]);
        assert!(clip(3, 3, 3).decode(&d, &[5]).is_err());
        // Streams with `w` stored: x = 0.6, y = z = 0, w = 0.8 (scaled by 2).
        let mut s = Vec::new();
        for v in [0.6f32, 0.0, 0.0, 1.6, 1.0, 0.0, 0.0] {
            s.extend_from_slice(&[0x10, 0x00]);
            s.extend_from_slice(&f24_bytes(v));
        }
        let tracks = clip(3, 0x8000_0003, 1).decode(&s, &[0]).unwrap();
        let q = tracks[0].rotations[0];
        assert!(
            (q[0] - 0.351).abs() < 0.01 && (q[3] - 0.936).abs() < 0.01,
            "{q:?}"
        );
        assert_eq!(tracks[0].translations[0], [1.0, 0.0, 0.0]);
        // Scheme 4 reads the fourth stream as the sign of w.
        let tracks = clip(4, 0x8000_0004, 1).decode(&s, &[0]).unwrap();
        assert!((tracks[0].rotations[0][3] + 0.8).abs() < 1e-3);
    }
}
