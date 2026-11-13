//! Writer for animation banks (`bnk`): a structured form of the version-9
//! relocatable image ([`BankImage`]) and its encoder.
//!
//! [`BankImage::parse`] splits the image into the header lists, the blend
//! graph, the clips (record, channel streams, events and their arguments) and
//! the tail (event objects, their nodes and the string pool). Pointers become
//! symbolic [`Target`]s, so blocks can grow and move; [`BankImage::encode`]
//! lays the image out again the way the game's exporter does and rebuilds the
//! relocation table. The corpus test checks `encode(parse(x)) == x` byte for
//! byte on every bank of the game.
//!
//! ```text
//! 0x00  header (0x44 bytes)                  A10, padding '_'
//!       entry list {ptr entry}               A10
//!       clip list {ptr clip}                 A10
//!       event object list {ptr object}       A10
//!       graph: named entries {ptr name, ptr root}, node trees,
//!              names inline; depth first     A10 per object
//!       clips, in list order:
//!         record (0x38, padded to 0x40; old layout 0x30)
//!         channel streams                    A10
//!         events {f32 frame, ptr name, u32 signature, ptr args}
//!         argument arrays {u32 tag, u32 value}, in event order  A10
//!       tail: event objects and their nodes (packed, A4), string pool  A10
//!       relocation table: pointer fields, descending, then 0
//! ```
//!
//! The graph and the tail are kept as relocatable blocks (bytes and pointer
//! fields): editing clips and events never needs their inner layout. New named
//! entries are appended to the graph block ([`BankImage::add_entry`]).

use std::collections::{BTreeMap, BTreeSet};

use crate::io::{self, Endian};
use serde::Serialize;

use crate::resource::anim::{EventArg, TICKS_PER_FRAME, layout_streams_len};
use crate::resource::anim_encode::EncodedClip;
use crate::resource::image::Image;
use crate::{Error, Result};

const LE: Endian = Endian::Little;
const PAD: u8 = b'_';
const HEADER: usize = 0x44;
/// Record sizes and stream offsets of the two clip layouts.
const RECORD: usize = 0x38;
const RECORD_STREAMS: usize = 0x40;
const OLD_RECORD: usize = 0x30;

/// Where a pointer points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    /// Offset in the graph block.
    Graph(u32),
    /// A clip record (index in [`BankImage::clips`]).
    Clip(u32),
    /// Offset in the tail block.
    Tail(u32),
}

/// Bytes with pointer fields: the graph or the tail.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Block {
    /// Data without the final padding.
    pub data: Vec<u8>,
    /// Pointer fields by offset in `data`; their bytes in `data` are zero.
    pub pointers: BTreeMap<u32, Target>,
}

impl Block {
    /// Appends `bytes` at the next multiple of `align`, returning their offset.
    pub(crate) fn push(&mut self, bytes: &[u8], align: usize) -> u32 {
        while !self.data.len().is_multiple_of(align) {
            self.data.push(PAD);
        }
        let at = self.data.len() as u32;
        self.data.extend_from_slice(bytes);
        at
    }

    /// The NUL-terminated string at `at`.
    pub fn cstr(&self, at: u32) -> Option<&[u8]> {
        let rest = self.data.get(at as usize..)?;
        let end = rest.iter().position(|&b| b == 0)?;
        Some(&rest[..end])
    }

    /// Offset of a string equal to `text` that starts a string of the block
    /// (preceded by NUL or padding), if any.
    fn find_string(&self, text: &[u8]) -> Option<u32> {
        let mut needle = text.to_vec();
        needle.push(0);
        let d = &self.data;
        (0..d.len().saturating_sub(needle.len() - 1))
            .find(|&i| d[i..].starts_with(&needle) && (i == 0 || d[i - 1] == 0 || d[i - 1] == PAD))
            .map(|i| i as u32)
    }

    /// Offset of `text` in the block's string pool, appending it when the
    /// block has no such string.
    pub fn intern(&mut self, text: &str) -> u32 {
        if let Some(at) = self.find_string(text.as_bytes()) {
            return at;
        }
        let at = self.data.len() as u32;
        self.data.extend_from_slice(text.as_bytes());
        self.data.push(0);
        at
    }
}

/// A value of an event argument: a word, or a pointer (strings).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ArgValue {
    Word(u32),
    Pointer(Target),
}

/// An event of a clip as stored.
#[derive(Debug, Clone, PartialEq)]
pub struct EventImage {
    pub frame: f32,
    pub name: Target,
    pub signature: u32,
    /// `None`: null argument pointer.
    pub args: Option<Vec<(u32, ArgValue)>>,
}

/// A clip as stored: record, channel streams and events.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipImage {
    /// The record: 0x38 bytes (current layout) or 0x30 (old layout); the
    /// event count and pointer fields are zero (they come from `events`).
    pub record: Vec<u8>,
    /// Channel streams, without the final padding.
    pub streams: Vec<u8>,
    pub events: Vec<EventImage>,
}

impl ClipImage {
    pub fn old_layout(&self) -> bool {
        self.record.len() == OLD_RECORD
    }

    fn word(&self, at: usize) -> u32 {
        io::read_u32(&self.record, at, LE).unwrap_or(0)
    }

    fn set_word(&mut self, at: usize, value: u32) {
        self.record[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    pub fn hash(&self) -> u32 {
        self.word(0)
    }

    pub fn frames(&self) -> u32 {
        self.word(if self.old_layout() { 0x18 } else { 0x08 })
    }

    pub fn bone_set(&self) -> u32 {
        self.word(0x14)
    }

    /// The format word: `0x80000000 | scheme`.
    pub fn format(&self) -> u32 {
        self.word(if self.old_layout() { 0x20 } else { 0x28 })
    }

    pub fn set_hash(&mut self, hash: u32) {
        self.set_word(0, hash);
    }

    pub fn set_frames(&mut self, frames: u32) {
        self.set_word(if self.old_layout() { 0x18 } else { 0x08 }, frames);
    }

    pub fn set_format(&mut self, format: u32) {
        self.set_word(if self.old_layout() { 0x20 } else { 0x28 }, format);
    }

    /// Offset of the streams from the record.
    pub fn streams_offset(&self) -> usize {
        if self.old_layout() {
            OLD_RECORD
        } else {
            RECORD_STREAMS
        }
    }

    /// Whether the clip loops (current layout +0x18, old +0x08): its clip
    /// nodes then last `frames × 2` ticks instead of `(frames − 1) × 2`.
    pub fn looping(&self) -> bool {
        self.word(if self.old_layout() { 0x08 } else { 0x18 }) != 0
    }

    pub fn set_looping(&mut self, looping: bool) {
        self.set_word(if self.old_layout() { 0x08 } else { 0x18 }, looping as u32);
    }

    /// A new current-layout record: name hash, frames, bone set and format
    /// word; no root motion (+0x24 = −1), not looping.
    pub fn new_record(hash: u32, frames: u32, bone_set: u32, format: u32) -> Vec<u8> {
        let mut r = vec![0u8; RECORD];
        let mut put = |at: usize, v: u32| r[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(0x00, hash);
        put(0x04, 10);
        put(0x08, frames);
        put(0x14, bone_set);
        put(0x1C, (-1.0f32).to_bits());
        put(0x24, u32::MAX);
        put(0x28, format);
        r
    }

    /// A new current-layout record with the flags of this clip (root-motion
    /// bone and mask, looping, bone set) under another name hash.
    pub fn record_like(&self, hash: u32) -> Vec<u8> {
        let mut r = Self::new_record(hash, self.frames(), self.bone_set(), self.format());
        let mut put = |at: usize, v: u32| r[at..at + 4].copy_from_slice(&v.to_le_bytes());
        if self.old_layout() {
            // Old +0x0C mask, +0x10 motion bone, +0x1C index, +0x08 loop.
            put(0x20, self.word(0x0C));
            put(0x24, self.word(0x10));
            put(0x10, self.word(0x1C));
            put(0x18, self.word(0x08));
        } else {
            for at in [0x0C, 0x10, 0x18, 0x1C, 0x20, 0x24] {
                put(at, self.word(at));
            }
        }
        r
    }
}

/// An animation bank in structured form.
#[derive(Debug, Clone, PartialEq)]
pub struct BankImage {
    pub hash: u32,
    /// SDBM of the skeleton name.
    pub skeleton: u32,
    /// Header words 0x10..0x1C (0, 0, 0, 1 in every bank).
    pub words: [u32; 4],
    /// Header 0x20: the number of nodes the largest entry tree needs.
    pub node_count: u32,
    /// Named entries `{ptr name, ptr root}` (header 0x24).
    pub entries: Vec<Target>,
    /// Event objects (header 0x34).
    pub event_objects: Vec<Target>,
    pub graph: Block,
    pub clips: Vec<ClipImage>,
    pub tail: Block,
}

fn bad(at: usize, reason: impl Into<String>) -> Error {
    Error::malformed("bnk", at, reason)
}

fn align(v: usize, a: usize) -> usize {
    v.div_ceil(a) * a
}

/// Length of `data[start..end]` without its final padding: at most 15 bytes
/// of `'_'`, so that padding to 16 again restores the region (`start` and
/// `end` are multiples of 16).
fn unpadded(data: &[u8], start: usize, end: usize) -> usize {
    let mut e = end;
    while e > start && end - e < 15 && data[e - 1] == PAD {
        e -= 1;
    }
    e - start
}

/// Pointer maps of the image regions.
struct Regions {
    graph: (usize, usize),
    clips: Vec<usize>,
    tail: usize,
    end: usize,
}

impl Regions {
    fn target(&self, t: usize, field: usize) -> Result<Target> {
        if t >= self.tail && t <= self.end {
            return Ok(Target::Tail((t - self.tail) as u32));
        }
        if let Some(i) = self.clips.iter().position(|&c| c == t) {
            return Ok(Target::Clip(i as u32));
        }
        if t >= self.graph.0 && t < self.graph.1 {
            return Ok(Target::Graph((t - self.graph.0) as u32));
        }
        Err(bad(field, format!("pointer {t:#x} into no known object")))
    }
}

impl BankImage {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let image = Image::parse(data, "bnk")?;
        if image.version() != 9 {
            return Err(Error::UnsupportedVersion {
                what: "bnk",
                version: image.version(),
            });
        }
        let end = image.table_at;
        let relocs: BTreeSet<usize> = image.relocations().into_iter().collect();
        if data.len() != end + 4 * (relocs.len() + 1) {
            return Err(bad(end, "data after the relocation table"));
        }
        let word = |at: usize| image.u32(at);
        let is_ptr = |at: usize| relocs.contains(&at);
        // Header and lists.
        if word(0x3C)? != 0 || word(0x40)? != HEADER as u32 || !is_ptr(0x40) {
            return Err(bad(0x3C, "unexpected list at 0x3C"));
        }
        let mut at = align(HEADER, 16);
        let mut lists = Vec::new();
        for field in [0x24, 0x2C, 0x34] {
            let count = word(field)? as usize;
            let list = word(field + 4)? as usize;
            if list != at || !is_ptr(field + 4) {
                return Err(bad(field, "list not where the exporter puts it"));
            }
            image.array(list, count, 4)?;
            let mut items = Vec::with_capacity(count);
            for k in 0..count {
                if !is_ptr(list + 4 * k) {
                    return Err(bad(list + 4 * k, "list item is not a pointer"));
                }
                items.push(word(list + 4 * k)? as usize);
            }
            lists.push(items);
            at = align(list + 4 * count, 16);
        }
        let graph_start = at;
        let clips_at = lists[1].clone();
        if clips_at.is_empty() || clips_at.windows(2).any(|w| w[0] >= w[1]) {
            return Err(bad(0x30, "clips missing or not in ascending order"));
        }
        if clips_at[0] < graph_start {
            return Err(bad(clips_at[0], "clip before the graph"));
        }
        // Clips: everything up to the next clip; the last one ends where its
        // content does.
        let mut clips = Vec::with_capacity(clips_at.len());
        let mut raw_clips = Vec::with_capacity(clips_at.len());
        let mut next_end = 0;
        for (i, &c) in clips_at.iter().enumerate() {
            let limit = match clips_at.get(i + 1) {
                Some(&n) => n,
                None => {
                    // The first pointer field or target after the clip's streams.
                    let old = word(c + 4)? & 0xFF < 10;
                    let streams = c + if old { OLD_RECORD } else { RECORD_STREAMS };
                    relocs
                        .iter()
                        .flat_map(|&f| [f, word(f).unwrap_or(0) as usize])
                        .filter(|&x| x > streams)
                        .min()
                        .unwrap_or(end)
                        .min(end)
                }
            };
            let (clip, content_end) = parse_clip(&image, &relocs, c, limit)?;
            if clips_at.get(i + 1).is_some() && align(content_end, 16) != limit {
                return Err(bad(c, "clip does not end where the next begins"));
            }
            next_end = align(content_end, 16);
            raw_clips.push((c, content_end));
            clips.push(clip);
        }
        let regions = Regions {
            graph: (graph_start, clips_at[0]),
            clips: clips_at.clone(),
            tail: next_end,
            end,
        };
        if next_end > end {
            return Err(bad(next_end, "last clip runs into the relocation table"));
        }
        // Resolve clip event pointers.
        for (clip, &(c, _)) in clips.iter_mut().zip(&raw_clips) {
            for (k, e) in clip.events.iter_mut().enumerate() {
                let rec = word(c + 0x34)? as usize + 16 * k;
                e.name = regions.target(word(rec + 4)? as usize, rec + 4)?;
                if let Some(args) = &mut e.args {
                    let args_at = word(rec + 12)? as usize;
                    for (j, (_, v)) in args.iter_mut().enumerate() {
                        let field = args_at + 8 * j + 4;
                        if is_ptr(field) {
                            *v = ArgValue::Pointer(regions.target(word(field)? as usize, field)?);
                        }
                    }
                }
            }
        }
        let block = |start: usize, stop: usize| -> Result<Block> {
            let len = unpadded(data, start, stop);
            let mut block = Block {
                data: data[start..start + len].to_vec(),
                pointers: BTreeMap::new(),
            };
            for &f in relocs.range(start..stop) {
                if f + 4 > start + len {
                    return Err(bad(f, "pointer field in padding"));
                }
                let target = regions.target(word(f)? as usize, f)?;
                block.pointers.insert((f - start) as u32, target);
                block.data[f - start..f - start + 4].fill(0);
            }
            Ok(block)
        };
        let graph = block(graph_start, clips_at[0])?;
        let tail = block(next_end, end)?;
        let targets = |items: &[usize], field: usize| -> Result<Vec<Target>> {
            items.iter().map(|&t| regions.target(t, field)).collect()
        };
        let parsed = Self {
            hash: word(0x08)?,
            skeleton: word(0x0C)?,
            words: [word(0x10)?, word(0x14)?, word(0x18)?, word(0x1C)?],
            node_count: word(0x20)?,
            entries: targets(&lists[0], 0x28)?,
            event_objects: targets(&lists[2], 0x38)?,
            graph,
            clips,
            tail,
        };
        // Every relocation must be accounted for.
        let expected = 4 + lists.iter().map(Vec::len).sum::<usize>() + parsed.pointer_count();
        if expected != relocs.len() {
            return Err(bad(end, "pointer fields the writer does not know"));
        }
        Ok(parsed)
    }

    fn pointer_count(&self) -> usize {
        let clip_ptrs: usize = self
            .clips
            .iter()
            .map(|c| {
                let events = &c.events;
                (!events.is_empty()) as usize
                    + events
                        .iter()
                        .map(|e| {
                            1 + e.args.as_ref().map_or(0, |a| {
                                1 + a
                                    .iter()
                                    .filter(|(_, v)| matches!(v, ArgValue::Pointer(_)))
                                    .count()
                            })
                        })
                        .sum::<usize>()
            })
            .sum();
        self.graph.pointers.len() + self.tail.pointers.len() + clip_ptrs
    }

    /// Lays the image out and writes it.
    pub fn encode(&self) -> Result<Vec<u8>> {
        // Positions.
        let mut at = align(HEADER, 16);
        let mut list_at = [0usize; 3];
        for (k, n) in [
            self.entries.len(),
            self.clips.len(),
            self.event_objects.len(),
        ]
        .into_iter()
        .enumerate()
        {
            list_at[k] = at;
            at = align(at + 4 * n, 16);
        }
        let graph_at = at;
        at = align(graph_at + self.graph.data.len(), 16);
        let mut clip_at = Vec::with_capacity(self.clips.len());
        for clip in &self.clips {
            clip_at.push(at);
            at = align(at + clip_len(clip), 16);
        }
        let tail_at = at;
        let table_at = align(tail_at + self.tail.data.len(), 16);
        let resolve = |t: Target| -> Result<u32> {
            Ok(match t {
                Target::Graph(o) => (graph_at + o as usize) as u32,
                Target::Tail(o) => (tail_at + o as usize) as u32,
                Target::Clip(i) => *clip_at
                    .get(i as usize)
                    .ok_or_else(|| Error::InvalidArgument(format!("no clip {i}")))?
                    as u32,
            })
        };
        let mut out = vec![PAD; table_at];
        let mut relocs = Vec::new();
        let put = |out: &mut Vec<u8>, at: usize, v: u32| {
            out[at..at + 4].copy_from_slice(&v.to_le_bytes());
        };
        put(&mut out, 0, table_at as u32);
        put(&mut out, 4, 9);
        put(&mut out, 8, self.hash);
        put(&mut out, 0x0C, self.skeleton);
        for (k, w) in self.words.iter().enumerate() {
            put(&mut out, 0x10 + 4 * k, *w);
        }
        put(&mut out, 0x20, self.node_count);
        let lists: [Vec<u32>; 3] = [
            self.entries
                .iter()
                .map(|&t| resolve(t))
                .collect::<Result<_>>()?,
            clip_at.iter().map(|&c| c as u32).collect(),
            self.event_objects
                .iter()
                .map(|&t| resolve(t))
                .collect::<Result<_>>()?,
        ];
        for (k, items) in lists.iter().enumerate() {
            put(&mut out, 0x24 + 8 * k, items.len() as u32);
            put(&mut out, 0x28 + 8 * k, list_at[k] as u32);
            relocs.push(0x28 + 8 * k);
            for (j, &v) in items.iter().enumerate() {
                put(&mut out, list_at[k] + 4 * j, v);
                relocs.push(list_at[k] + 4 * j);
            }
        }
        put(&mut out, 0x3C, 0);
        put(&mut out, 0x40, HEADER as u32);
        relocs.push(0x40);
        for (block, base) in [(&self.graph, graph_at), (&self.tail, tail_at)] {
            out[base..base + block.data.len()].copy_from_slice(&block.data);
            for (&f, &t) in &block.pointers {
                let f = base + f as usize;
                if f + 4 > base + block.data.len() {
                    return Err(Error::InvalidArgument(format!(
                        "pointer field {f:#x} outside its block"
                    )));
                }
                put(&mut out, f, resolve(t)?);
                relocs.push(f);
            }
        }
        for (clip, &c) in self.clips.iter().zip(&clip_at) {
            out[c..c + clip.record.len()].copy_from_slice(&clip.record);
            let s = c + clip.streams_offset();
            out[s..s + clip.streams.len()].copy_from_slice(&clip.streams);
            if clip.old_layout() {
                if !clip.events.is_empty() {
                    return Err(Error::InvalidArgument(
                        "old-layout clips have no events".into(),
                    ));
                }
                continue;
            }
            let n = clip.events.len();
            put(&mut out, c + 0x30, n as u32);
            if n == 0 {
                put(&mut out, c + 0x34, 0);
                continue;
            }
            let events_at = align(s + clip.streams.len(), 16);
            put(&mut out, c + 0x34, events_at as u32);
            relocs.push(c + 0x34);
            let mut args_at = events_at + 16 * n;
            for (k, e) in clip.events.iter().enumerate() {
                let r = events_at + 16 * k;
                put(&mut out, r, e.frame.to_bits());
                put(&mut out, r + 4, resolve(e.name)?);
                relocs.push(r + 4);
                put(&mut out, r + 8, e.signature);
                match &e.args {
                    None => put(&mut out, r + 12, 0),
                    Some(args) => {
                        put(&mut out, r + 12, args_at as u32);
                        relocs.push(r + 12);
                        for &(tag, v) in args {
                            put(&mut out, args_at, tag);
                            match v {
                                ArgValue::Word(w) => put(&mut out, args_at + 4, w),
                                ArgValue::Pointer(t) => {
                                    put(&mut out, args_at + 4, resolve(t)?);
                                    relocs.push(args_at + 4);
                                }
                            }
                            args_at += 8;
                        }
                    }
                }
            }
        }
        relocs.sort_unstable();
        relocs.dedup();
        for &f in relocs.iter().rev() {
            io::put_u32(&mut out, f as u32, LE);
        }
        io::put_u32(&mut out, 0, LE);
        Ok(out)
    }

    /// Text of the string at `target` (event names and string arguments).
    pub fn string(&self, target: Target) -> Option<String> {
        let block = match target {
            Target::Graph(_) => &self.graph,
            Target::Tail(_) => &self.tail,
            Target::Clip(_) => return None,
        };
        let (Target::Graph(o) | Target::Tail(o)) = target else {
            return None;
        };
        block
            .cstr(o)
            .map(|s| String::from_utf8_lossy(s).into_owned())
    }

    /// Index of the clip whose name hash is `hash`.
    pub fn clip_index(&self, hash: u32) -> Option<usize> {
        self.clips.iter().position(|c| c.hash() == hash)
    }

    /// The events of clip `index` with their strings.
    pub fn events(&self, index: usize) -> Vec<crate::resource::anim::ClipEvent> {
        self.clips[index]
            .events
            .iter()
            .map(|e| crate::resource::anim::ClipEvent {
                frame: e.frame,
                name: self.string(e.name).unwrap_or_default(),
                signature: e.signature,
                args: e
                    .args
                    .iter()
                    .flatten()
                    .map(|&(tag, v)| match (tag, v) {
                        (1, ArgValue::Word(w)) => EventArg::Float(f32::from_bits(w)),
                        (2, ArgValue::Pointer(t)) => {
                            EventArg::Text(self.string(t).unwrap_or_default())
                        }
                        (_, ArgValue::Word(w)) => EventArg::Int(w as i32),
                        (_, ArgValue::Pointer(_)) => EventArg::Int(0),
                    })
                    .collect(),
            })
            .collect()
    }

    /// Replaces the events of clip `index`. Strings go to the tail's pool;
    /// the argument count is stored in the low byte of the signature, whose
    /// upper bytes are kept from `events`.
    pub fn set_events(
        &mut self,
        index: usize,
        events: &[crate::resource::anim::ClipEvent],
    ) -> Result<()> {
        if self.clips.get(index).is_none_or(ClipImage::old_layout) {
            return Err(Error::InvalidArgument(format!(
                "clip {index} cannot carry events"
            )));
        }
        let mut out = Vec::with_capacity(events.len());
        for e in events {
            if e.args.len() > 0xFF {
                return Err(Error::InvalidArgument("too many event arguments".into()));
            }
            let name = Target::Tail(self.tail.intern(&e.name));
            let args: Vec<(u32, ArgValue)> = e
                .args
                .iter()
                .map(|a| match a {
                    EventArg::Int(v) => (0, ArgValue::Word(*v as u32)),
                    EventArg::Float(v) => (1, ArgValue::Word(v.to_bits())),
                    EventArg::Text(t) => (2, ArgValue::Pointer(Target::Tail(self.tail.intern(t)))),
                })
                .collect();
            out.push(EventImage {
                frame: e.frame,
                name,
                signature: (e.signature & !0xFF) | args.len() as u32,
                args: (!args.is_empty()).then_some(args),
            });
        }
        self.clips[index].events = out;
        Ok(())
    }

    fn graph_word(&self, at: u32) -> u32 {
        io::read_u32(&self.graph.data, at as usize, LE).unwrap_or(0)
    }

    fn graph_pointer(&self, at: u32) -> Option<Target> {
        self.graph.pointers.get(&at).copied()
    }

    /// The named entries: what the game asks for by name.
    pub fn entry_list(&self) -> Vec<Entry> {
        self.entries
            .iter()
            .map(|&e| {
                let Target::Graph(e) = e else {
                    return Entry::default();
                };
                let name = self
                    .graph_pointer(e)
                    .and_then(|t| self.string(t))
                    .unwrap_or_default();
                let root = self.graph_pointer(e + 4);
                let priority = match root {
                    Some(Target::Graph(n)) if self.graph.data.get(n as usize) == Some(&2) => {
                        self.graph_pointer(n + 4).and_then(|p| match p {
                            Target::Graph(p) => Some(self.graph_word(p)),
                            _ => None,
                        })
                    }
                    _ => None,
                };
                let mut clips = Vec::new();
                let mut stack: Vec<u32> = root
                    .into_iter()
                    .filter_map(|t| match t {
                        Target::Graph(n) => Some(n),
                        _ => None,
                    })
                    .collect();
                let mut seen = BTreeSet::new();
                while let Some(n) = stack.pop() {
                    if !seen.insert(n) || seen.len() > 10_000 {
                        continue;
                    }
                    let kind = self.graph.data.get(n as usize).copied().unwrap_or(0) & 0x3F;
                    let count = self.graph.data.get(n as usize + 1).copied().unwrap_or(0) as u32;
                    if kind == NODE_CLIP {
                        if let Some(Target::Clip(c)) = self.graph_pointer(n + 12) {
                            clips.push(c as usize);
                        }
                        continue;
                    }
                    for k in (0..count).rev() {
                        if let Some(Target::Graph(c)) = self.graph_pointer(n + 12 + 4 * k) {
                            stack.push(c);
                        }
                    }
                }
                Entry {
                    name,
                    priority,
                    clips,
                }
            })
            .collect()
    }

    /// Graph offsets of the parameters `{u32 0, f32 duration, f32 ticks per
    /// frame}` of the clip nodes that play clip `index`.
    pub fn clip_node_params(&self, index: usize) -> Vec<u32> {
        self.graph
            .pointers
            .iter()
            .filter(|&(_, &t)| t == Target::Clip(index as u32))
            .filter_map(|(&f, _)| {
                let node = f.checked_sub(12)?;
                if self.graph.data.get(node as usize)? & 0x3F != NODE_CLIP {
                    return None;
                }
                match self.graph_pointer(node + 4)? {
                    Target::Graph(p) if p as usize + 12 <= self.graph.data.len() => Some(p),
                    _ => None,
                }
            })
            .collect()
    }

    /// Sets the duration of the clip nodes of clip `index` from its frames:
    /// `frames × 2` ticks for looping clips, `(frames − 1) × 2` otherwise,
    /// as in every clip node of the game.
    fn update_durations(&mut self, index: usize) {
        let clip = &self.clips[index];
        let frames = clip.frames() as f32;
        let loops = clip.looping();
        for p in self.clip_node_params(index) {
            let at = p as usize;
            let rate = f32::from_bits(self.graph_word(p + 8));
            let rate = if rate > 0.0 { rate } else { TICKS_PER_FRAME };
            let duration = if loops {
                frames
            } else {
                (frames - 1.0).max(0.0)
            } * rate;
            self.graph.data[at + 4..at + 8].copy_from_slice(&duration.to_le_bytes());
        }
    }

    /// Replaces the animation of clip `index`, keeping its name, bone set,
    /// flags and events (events past the new end are kept as they are).
    pub fn replace_clip(&mut self, index: usize, encoded: &EncodedClip) -> Result<()> {
        let clip = self
            .clips
            .get_mut(index)
            .ok_or_else(|| Error::InvalidArgument(format!("no clip {index}")))?;
        clip.set_frames(encoded.frames);
        clip.set_format(encoded.format);
        clip.streams = encoded.streams.clone();
        self.update_durations(index);
        Ok(())
    }

    /// Adds a clip with `record` (see [`ClipImage::new_record`]) and the
    /// encoded animation; returns its index.
    pub fn add_clip(&mut self, record: Vec<u8>, encoded: &EncodedClip) -> Result<usize> {
        if record.len() != RECORD {
            return Err(Error::InvalidArgument(
                "clip record of the wrong size".into(),
            ));
        }
        if self.clip_index(io::read_u32(&record, 0, LE)?).is_some() {
            return Err(Error::InvalidArgument(
                "the bank already has a clip with this name".into(),
            ));
        }
        let mut clip = ClipImage {
            record,
            streams: Vec::new(),
            events: Vec::new(),
        };
        clip.set_frames(encoded.frames);
        clip.set_format(encoded.format);
        clip.streams = encoded.streams.clone();
        self.clips.push(clip);
        Ok(self.clips.len() - 1)
    }

    /// Adds a named entry `name` that plays clip `clip`: a wrapper node
    /// (type 2, its priority decides which bank wins when several register
    /// the same name) over a clip node, laid out like the game's entries.
    pub fn add_entry(&mut self, name: &str, clip: usize, priority: u32) -> Result<()> {
        if clip >= self.clips.len() {
            return Err(Error::InvalidArgument(format!("no clip {clip}")));
        }
        if name.is_empty() || name.contains('\0') {
            return Err(Error::InvalidArgument("bad entry name".into()));
        }
        if self
            .entry_list()
            .iter()
            .any(|e| e.name.eq_ignore_ascii_case(name))
        {
            return Err(Error::InvalidArgument(format!(
                "the bank already has an entry `{name}`"
            )));
        }
        let g = &mut self.graph;
        let entry = g.push(&[0; 8], 16);
        let mut text = name.as_bytes().to_vec();
        text.push(0);
        let name_at = g.push(&text, 16);
        // Root: type 2, one child, node 0; parameters {priority, 0x12032045}.
        let root = g.push(&[2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], 16);
        let mut params = priority.to_le_bytes().to_vec();
        params.extend_from_slice(&WRAPPER_TAG.to_le_bytes());
        let root_params = g.push(&params, 16);
        // Clip node: type 0x16, node 1; parameters {0, duration, 2.0}.
        let node = g.push(
            &[NODE_CLIP, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            16,
        );
        let mut params = vec![0u8; 4];
        params.extend_from_slice(&0f32.to_le_bytes());
        params.extend_from_slice(&TICKS_PER_FRAME.to_le_bytes());
        let node_params = g.push(&params, 16);
        for (field, target) in [
            (entry, Target::Graph(name_at)),
            (entry + 4, Target::Graph(root)),
            (root + 4, Target::Graph(root_params)),
            (root + 12, Target::Graph(node)),
            (node + 4, Target::Graph(node_params)),
            (node + 12, Target::Clip(clip as u32)),
        ] {
            g.pointers.insert(field, target);
        }
        self.entries.push(Target::Graph(entry));
        self.node_count = self.node_count.max(2);
        self.update_durations(clip);
        Ok(())
    }
}

/// Node type of clip nodes.
const NODE_CLIP: u8 = 0x16;
/// Second parameter of every entry's wrapper node (type 2) in the game.
const WRAPPER_TAG: u32 = 0x1203_2045;

/// A named entry of a bank.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Entry {
    pub name: String,
    /// Priority of the wrapper node (type 2): a bank loaded later replaces an
    /// entry of the same name only with a higher priority (0x00573200).
    pub priority: Option<u32>,
    /// Clips the entry's node tree plays.
    pub clips: Vec<usize>,
}

/// Bytes of a clip up to the end of its content (before the final padding).
fn clip_len(clip: &ClipImage) -> usize {
    let s = clip.streams_offset() + clip.streams.len();
    if clip.events.is_empty() {
        return s;
    }
    align(s, 16)
        + 16 * clip.events.len()
        + 8 * clip
            .events
            .iter()
            .map(|e| e.args.as_ref().map_or(0, Vec::len))
            .sum::<usize>()
}

/// Parses the clip at `c`, which ends before `limit`; returns it and the end
/// of its content. Event pointers are resolved by the caller.
fn parse_clip(
    image: &Image<'_>,
    relocs: &BTreeSet<usize>,
    c: usize,
    limit: usize,
) -> Result<(ClipImage, usize)> {
    let data = image.data;
    let word = |at: usize| image.u32(at);
    let old = word(c + 4)? & 0xFF < 10;
    let (record_len, streams_at) = if old {
        (OLD_RECORD, c + OLD_RECORD)
    } else {
        (RECORD, c + RECORD_STREAMS)
    };
    if streams_at > limit {
        return Err(bad(c, "clip record cut short"));
    }
    let mut record = io::slice(data, c, record_len)?.to_vec();
    if relocs.range(c..streams_at).any(|&f| old || f != c + 0x34) {
        return Err(bad(c, "unexpected pointer in a clip record"));
    }
    if !old && data[c + RECORD..streams_at].iter().any(|&b| b != PAD) {
        return Err(bad(c + RECORD, "clip record padding"));
    }
    let count = if old { 0 } else { word(c + 0x30)? as usize };
    let events_at = if count > 0 {
        let p = word(c + 0x34)? as usize;
        if !relocs.contains(&(c + 0x34)) || p < streams_at || !p.is_multiple_of(16) || p > limit {
            return Err(bad(c + 0x34, "events not after the streams"));
        }
        Some(p)
    } else {
        if word(c + 0x34)? != 0 && !old {
            return Err(bad(c + 0x34, "event pointer without events"));
        }
        None
    };
    if !old {
        record[0x30..0x38].fill(0);
    }
    let streams_end = match events_at {
        Some(p) => p,
        None => limit,
    };
    // The streams: everything up to the events (or the limit) without the
    // padding; for the last clip the end comes from the streams themselves.
    let format = word(c + if old { 0x20 } else { 0x28 })?;
    let frames = word(c + if old { 0x18 } else { 0x08 })?;
    let mut len = unpadded(data, streams_at, streams_end);
    if let Some(exact) = layout_streams_len(data, streams_at, streams_end, format, frames)
        && exact <= len
        && data[streams_at + exact..streams_end]
            .iter()
            .all(|&b| b == PAD)
    {
        len = exact;
    }
    let streams = data[streams_at..streams_at + len].to_vec();
    let mut events = Vec::with_capacity(count);
    let mut content_end = streams_at + len;
    if let Some(p) = events_at {
        image.array(p, count, 16)?;
        let arg_ptrs: Vec<Option<usize>> = (0..count)
            .map(|k| {
                let f = p + 16 * k + 12;
                Ok(relocs.contains(&f).then_some(word(f)? as usize))
            })
            .collect::<Result<_>>()?;
        let mut expected = p + 16 * count;
        for (k, &arg_ptr) in arg_ptrs.iter().enumerate() {
            let rec = p + 16 * k;
            if !relocs.contains(&(rec + 4)) {
                return Err(bad(rec + 4, "event name is not a pointer"));
            }
            let signature = word(rec + 8)?;
            let args = match arg_ptr {
                None => {
                    if word(rec + 12)? != 0 {
                        return Err(bad(rec + 12, "event arguments without a pointer"));
                    }
                    None
                }
                Some(a) => {
                    if a != expected {
                        return Err(bad(rec + 12, "event arguments out of order"));
                    }
                    // The low byte of the signature is the argument count.
                    let n = (signature & 0xFF) as usize;
                    if a + 8 * n > image.table_at {
                        return Err(bad(a, "event arguments run past the image"));
                    }
                    let mut args = Vec::with_capacity(n);
                    for j in 0..n {
                        args.push((word(a + 8 * j)?, ArgValue::Word(word(a + 8 * j + 4)?)));
                    }
                    expected = a + 8 * n;
                    Some(args)
                }
            };
            events.push(EventImage {
                frame: f32::from_bits(word(rec)?),
                name: Target::Graph(0),
                signature,
                args,
            });
        }
        content_end = expected;
    }
    Ok((
        ClipImage {
            record,
            streams,
            events,
        },
        content_end,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bank with one entry, one clip with one event and one event object.
    pub(crate) fn sample() -> BankImage {
        let mut graph = Block::default();
        // Entry {ptr name, ptr root}, name, root node (type 0x16, clip).
        let entry = graph.push(&[0; 8], 16);
        let name = graph.push(b"idle\0", 16);
        let node = graph.push(&[0x16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], 16);
        graph.pointers.insert(entry, Target::Graph(name));
        graph.pointers.insert(entry + 4, Target::Graph(node));
        graph.pointers.insert(node + 12, Target::Clip(0));
        let mut tail = Block::default();
        let object = tail.push(&[4, 0, 0, 0, 0, 0, 0, 0], 16);
        let mut clip = ClipImage {
            record: ClipImage::new_record(0x1234, 2, 0x78E0_3D81, 0x8000_0004),
            streams: vec![0x2C, 0x00, 0, 0, 0],
            events: Vec::new(),
        };
        clip.events.push(EventImage {
            frame: 1.0,
            name: Target::Tail(0),
            signature: 0x0300_0001,
            args: Some(vec![(1, ArgValue::Word(2.0f32.to_bits()))]),
        });
        let mut bank = BankImage {
            hash: 0xABCD,
            skeleton: 0x50F9_2C54,
            words: [0, 0, 0, 1],
            node_count: 1,
            entries: vec![Target::Graph(entry)],
            event_objects: vec![Target::Tail(object)],
            graph,
            clips: vec![clip],
            tail,
        };
        let footstep = bank.tail.intern("footstep");
        bank.clips[0].events[0].name = Target::Tail(footstep);
        bank
    }

    #[test]
    fn encodes_and_parses_back() {
        let bank = sample();
        let data = bank.encode().unwrap();
        let parsed = BankImage::parse(&data).unwrap();
        assert_eq!(parsed, bank);
        assert_eq!(parsed.encode().unwrap(), data);
        // The reader agrees.
        let read = crate::resource::anim::AnimBank::parse(&data).unwrap();
        assert_eq!(read.clips[0].hash, 0x1234);
        assert_eq!(read.clips[0].events[0].name, "footstep");
        assert_eq!(read.clips[0].events[0].args, [EventArg::Float(2.0)]);
    }

    #[test]
    fn edits_events_and_interns_strings() {
        let mut bank = sample();
        let before = bank.tail.data.len();
        let events = vec![crate::resource::anim::ClipEvent {
            frame: 4.0,
            name: "footstep".into(),
            signature: 0x0300_0000,
            args: vec![EventArg::Text("step.emx".into()), EventArg::Int(3)],
        }];
        bank.set_events(0, &events).unwrap();
        // "footstep" is reused, "step.emx" is new.
        assert_eq!(bank.tail.data.len(), before + 9);
        let data = bank.encode().unwrap();
        let back = BankImage::parse(&data).unwrap();
        let read = back.events(0);
        assert_eq!(read[0].name, "footstep");
        assert_eq!(read[0].signature, 0x0300_0002);
        assert_eq!(
            read[0].args,
            [EventArg::Text("step.emx".into()), EventArg::Int(3)]
        );
    }

    #[test]
    fn rejects_damaged_images() {
        let data = sample().encode().unwrap();
        for cut in [0, 8, 0x40, data.len() / 2] {
            assert!(BankImage::parse(&data[..cut]).is_err());
        }
        let mut bad = data.clone();
        bad[0x40] = 0x48; // list at 0x3C moved
        assert!(BankImage::parse(&bad).is_err());
    }
}
