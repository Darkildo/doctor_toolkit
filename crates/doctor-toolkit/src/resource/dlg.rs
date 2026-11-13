//! `dlg` resources: face-to-face (F2F) dialogue trees.
//!
//! A dialogue is a graph of nodes the game walks from node 0: a node either
//! speaks a line, offers a choice, tests or sets a value, or ends the
//! conversation (`godfather2.exe`: loader `0x006FB320`, relocation
//! `0x006F0310`, node classes `0x006EFF10`). The file is a memory image with
//! offsets instead of pointers; every dialogue of the game is laid out the
//! same way, and [`Dialogue::to_bytes`] reproduces each one byte for byte.
//!
//! ```text
//! 0x00  u32  version = 4
//! 0x04  u32  0
//! 0x08  u32  number of nodes N
//! 0x0C  u32  offset of the nodes = 0x34
//! 0x10  u32  0
//! 0x14  u32  number of animation groups M
//! 0x18  u32  offset of the groups, from 0x14
//! 0x1C  u32  offset of the animation records
//! 0x20  u32  offset of the choice label list       (0 when empty)
//! 0x24  u32  offset of the line slot list           (runtime, 4 bytes 0x24 per line)
//! 0x28  u32  offset of the line state list          (runtime, 20 zero bytes per line)
//! 0x2C  u32  offset of the camera/actor state list  (runtime, 2 × 20 zero bytes per line and choice)
//! 0x30  u32  offset of the animation list
//! 0x34  N × node { u32 0 (class pointer at run time), u32 kind, u32 child count,
//!                  u32 offset of the children, u32 offset of the payload } —
//!       both offsets from the node itself; payload 0 for the end node
//!       children of every node in node order: i32 offsets of the target
//!       nodes from the node, 0 for none
//!       payloads: other nodes in node order, then choices, then lines
//!       M × { u32 offset from the entry, u32 count }, then the values:
//!       indices of the animations of each group
//!       per animation { u32 index, u32 flags, u32 0 }
//!       lists: offsets of the records, 0, 0, then the records
//! ```
//!
//! Lists hold NUL-terminated strings padded to 4 bytes (choice labels: five
//! slots per choice node, string table names such as `hud_csa_f2f_choicea`,
//! empty for an unused slot; animations: 16 zero bytes and the clip name,
//! `f2f_player_talk_neutral_1`) or zero-filled space the game fills at run
//! time; the latter are rebuilt from the number of lines and choices.
//!
//! Node kinds seen in the game: 1 line, 2 condition `==`, 0x20 and 0x21 set
//! an NPC flag, 0x22 choice, 0x23 end, 0x24 assignment. The engine knows
//! kinds 0..0x2C ([`kind_name`]).

use serde::{Deserialize, Serialize};

use crate::hash::sdbm;
use crate::io::{self, Endian};
use crate::{Error, Result};

/// Type id of `dlg` resources.
pub const TYPE_DLG: u32 = sdbm(b"dlg");

const VERSION: u32 = 4;
const HEADER_SIZE: usize = 0x34;
const NODE_SIZE: usize = 20;
/// Choice label slots per choice node.
pub const CHOICE_SLOTS: usize = 5;
/// "No value" in payload index fields.
pub const NONE: u32 = u32::MAX;

/// Node kind: speaks a line.
pub const KIND_LINE: u32 = 1;
/// Node kind: compares a value, `children[1]` when equal, `children[0]` otherwise.
pub const KIND_IF_EQUAL: u32 = 2;
/// Node kind: sets one of the NPC's flags (value − 0x13).
pub const KIND_SET_FLAG: u32 = 0x20;
/// Node kind: sets one of the NPC's other flags (value − 0x1A).
pub const KIND_SET_FLAG_B: u32 = 0x21;
/// Node kind: offers the player a choice.
pub const KIND_CHOICE: u32 = 0x22;
/// Node kind: ends the conversation.
pub const KIND_END: u32 = 0x23;
/// Node kind: assigns a value.
pub const KIND_ASSIGN: u32 = 0x24;

/// Payload words of a line node.
pub const LINE_WORDS: usize = 16;
/// Line payload: speaker, 1 or 2.
pub const LINE_SPEAKER: usize = 0;
/// Line payload: index of the line (its runtime slots), twice.
pub const LINE_INDEX: usize = 6;
/// Line payload: hash of the speech event (`chv`).
pub const LINE_EVENT: usize = 8;
/// Line payload: animation group played before the line, or [`NONE`].
pub const LINE_ANIM_LEAD: usize = 9;
/// Line payload: animation group of the speaker.
pub const LINE_ANIM_SPEAKER: usize = 10;
/// Line payload: animation group of the listener.
pub const LINE_ANIM_LISTENER: usize = 12;
/// Choice payload: first label slot of the five options.
pub const CHOICE_LABELS: usize = 11;

/// Name of a node kind.
pub fn kind_name(kind: u32) -> String {
    match kind {
        KIND_LINE => "line".into(),
        KIND_IF_EQUAL => "if ==".into(),
        3 => "if !=".into(),
        4 => "if <".into(),
        5 => "if >".into(),
        KIND_SET_FLAG => "set NPC flag".into(),
        KIND_SET_FLAG_B => "set NPC flag (B)".into(),
        KIND_CHOICE => "choice".into(),
        KIND_END => "end".into(),
        KIND_ASSIGN => "assign".into(),
        other => format!("kind {other:#x}"),
    }
}

/// Payload size in words of a node of `kind` created by the editor.
pub fn payload_words(kind: u32) -> Option<usize> {
    match kind {
        KIND_LINE | KIND_CHOICE => Some(16),
        KIND_SET_FLAG | KIND_SET_FLAG_B => Some(1),
        KIND_END => None,
        _ => Some(4),
    }
}

/// One node of the graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub kind: u32,
    /// Target node of each branch; `None` for an empty branch.
    pub children: Vec<Option<usize>>,
    /// Payload words; `None` only for the end node.
    pub payload: Option<Vec<u32>>,
}

impl Node {
    pub fn word(&self, i: usize) -> Option<u32> {
        self.payload.as_ref()?.get(i).copied()
    }
}

/// An animation clip of the dialogue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Animation {
    /// Clip name in the animation banks of the dialogue's stream.
    pub name: String,
    /// Bits 0x02 / 0x04: actor 1 / 2 (speaker values of lines); 0x20..0x200:
    /// role of the clip; 0x40000 on most.
    pub flags: u32,
}

/// A parsed `dlg` resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dialogue {
    pub nodes: Vec<Node>,
    /// Animation groups: indices into [`Self::animations`]; lines and
    /// choices refer to groups, the game picks one clip of a group.
    pub groups: Vec<Vec<u32>>,
    pub animations: Vec<Animation>,
    /// Five label slots per choice node, in node order: string table names
    /// (`hud_csa_f2f_choicea`), empty when unused.
    pub labels: Vec<String>,
}

fn rel(base: usize, offset: u32) -> usize {
    base.wrapping_add(offset as i32 as isize as usize)
}

impl Dialogue {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let le = Endian::Little;
        let u = |at: usize| io::read_u32(data, at, le);
        let version = u(0)?;
        if version != VERSION {
            return Err(Error::malformed("dlg", 0, format!("version {version}")));
        }
        let n = u(8)? as usize;
        let nodes_at = u(0x0C)? as usize;
        if nodes_at != HEADER_SIZE {
            return Err(Error::malformed(
                "dlg",
                0x0C,
                format!("nodes at {nodes_at:#x}"),
            ));
        }
        io::slice(data, nodes_at, n.saturating_mul(NODE_SIZE))?;
        let group_count = u(0x14)? as usize;
        let groups_rel = u(0x18)? as usize;
        let anims_at = u(0x1C)? as usize;
        let lists: Vec<usize> = (0..5)
            .map(|i| u(0x20 + 4 * i).map(|v| v as usize))
            .collect::<Result<_>>()?;

        // Nodes and their children.
        let mut raw = Vec::with_capacity(n);
        let mut starts = Vec::new();
        for i in 0..n {
            let base = nodes_at + NODE_SIZE * i;
            let kind = u(base + 4)?;
            let count = u(base + 8)? as usize;
            let children_at = rel(base, u(base + 12)?);
            let payload_off = u(base + 16)?;
            io::slice(data, children_at, count.saturating_mul(4))?;
            let children = (0..count)
                .map(|k| {
                    let r = u(children_at + 4 * k)?;
                    if r == 0 {
                        return Ok(None);
                    }
                    let target = rel(base, r).wrapping_sub(nodes_at);
                    if !target.is_multiple_of(NODE_SIZE) || target / NODE_SIZE >= n {
                        return Err(Error::malformed(
                            "dlg",
                            children_at + 4 * k,
                            "child is not a node",
                        ));
                    }
                    Ok(Some(target / NODE_SIZE))
                })
                .collect::<Result<Vec<_>>>()?;
            let payload_at = (payload_off != 0).then(|| rel(base, payload_off));
            if let Some(at) = payload_at {
                starts.push(at);
            }
            raw.push((kind, children, payload_at));
        }
        // Payload sizes: up to the next payload, the last one up to the groups.
        let groups_at = (group_count > 0).then_some(0x14 + groups_rel);
        let region_end = [groups_at, Some(anims_at).filter(|&a| a != 0)]
            .into_iter()
            .chain(lists.iter().map(|&l| Some(l).filter(|&l| l != 0)))
            .flatten()
            .min()
            .unwrap_or(data.len());
        starts.sort_unstable();
        let mut nodes = Vec::with_capacity(n);
        for (kind, children, payload_at) in raw {
            let payload = match payload_at {
                Some(at) => {
                    let i = starts.partition_point(|&s| s <= at);
                    let end = starts.get(i).copied().unwrap_or(region_end);
                    if end < at || !(end - at).is_multiple_of(4) {
                        return Err(Error::malformed("dlg", at, "payload size"));
                    }
                    let words = (0..(end - at) / 4)
                        .map(|k| u(at + 4 * k))
                        .collect::<Result<Vec<_>>>()?;
                    Some(words)
                }
                None => None,
            };
            nodes.push(Node {
                kind,
                children,
                payload,
            });
        }

        let mut groups = Vec::with_capacity(group_count);
        if let Some(at) = groups_at {
            io::slice(data, at, group_count.saturating_mul(8))?;
            for k in 0..group_count {
                let e = at + 8 * k;
                let values_at = rel(e, u(e)?);
                let count = u(e + 4)? as usize;
                io::slice(data, values_at, count.saturating_mul(4))?;
                groups.push(
                    (0..count)
                        .map(|j| u(values_at + 4 * j))
                        .collect::<Result<_>>()?,
                );
            }
        }

        let records = |li: usize| -> Result<Vec<&[u8]>> {
            let at = lists[li];
            if at == 0 {
                return Ok(Vec::new());
            }
            let mut ptrs = Vec::new();
            let mut o = at;
            loop {
                let p = u(o)? as usize;
                if p == 0 {
                    break;
                }
                ptrs.push(p);
                o += 4;
            }
            let end = lists[li + 1..]
                .iter()
                .copied()
                .find(|&l| l != 0)
                .unwrap_or(data.len());
            let mut out = Vec::with_capacity(ptrs.len());
            for (k, &p) in ptrs.iter().enumerate() {
                let next = ptrs.get(k + 1).copied().unwrap_or(end);
                if next < p {
                    return Err(Error::malformed("dlg", o, "list records out of order"));
                }
                out.push(io::slice(data, p, next - p)?);
            }
            Ok(out)
        };
        let string_of = |r: &[u8], at: usize| -> Result<String> {
            let end = r.iter().position(|&b| b == 0).unwrap_or(r.len());
            String::from_utf8(r[at.min(end)..end].to_vec())
                .map_err(|_| Error::malformed("dlg", 0, "name is not UTF-8"))
        };
        let labels = records(0)?
            .into_iter()
            .map(|r| string_of(r, 0))
            .collect::<Result<Vec<_>>>()?;
        let anim_records = records(4)?;
        let mut animations = Vec::with_capacity(anim_records.len());
        for (k, r) in anim_records.into_iter().enumerate() {
            if r.len() < 16 || r[..16].iter().any(|&b| b != 0) {
                return Err(Error::malformed("dlg", lists[4], "animation record"));
            }
            let name = r[16..]
                .split(|&b| b == 0)
                .next()
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .unwrap_or_default();
            let at = anims_at + 12 * k;
            if anims_at == 0 || u(at)? as usize != k || u(at + 8)? != 0 {
                return Err(Error::malformed("dlg", at, "animation flags record"));
            }
            animations.push(Animation {
                name,
                flags: u(at + 4)?,
            });
        }
        let dialogue = Self {
            nodes,
            groups,
            animations,
            labels,
        };
        // The runtime lists must be what the writer rebuilds.
        let lines = dialogue.count(KIND_LINE);
        let slots = 2 * (lines + dialogue.count(KIND_CHOICE));
        let ok = |li: usize, count: usize, record: &[u8]| -> Result<()> {
            let r = records(li)?;
            if r.len() != count || r.iter().any(|x| *x != record) {
                return Err(Error::malformed(
                    "dlg",
                    lists[li],
                    format!("runtime list {li} does not match the node counts"),
                ));
            }
            Ok(())
        };
        ok(1, lines, &[0x24, 0, 0, 0])?;
        ok(2, lines, &[0; 20])?;
        ok(3, slots, &[0; 20])?;
        Ok(dialogue)
    }

    /// Number of nodes of a kind.
    pub fn count(&self, kind: u32) -> usize {
        self.nodes.iter().filter(|n| n.kind == kind).count()
    }

    /// Serialises the dialogue in the game's layout.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.check()?;
        let n = self.nodes.len();
        let mut out = vec![0u8; HEADER_SIZE + NODE_SIZE * n];
        let put = |out: &mut Vec<u8>, at: usize, v: u32| {
            out[at..at + 4].copy_from_slice(&v.to_le_bytes());
        };
        let node_at = |i: usize| HEADER_SIZE + NODE_SIZE * i;
        let mut children_at = Vec::with_capacity(n);
        for node in &self.nodes {
            children_at.push(out.len());
            out.resize(out.len() + 4 * node.children.len(), 0);
        }
        let rank = |k: u32| match k {
            KIND_LINE => 2,
            KIND_CHOICE => 1,
            _ => 0,
        };
        let mut order: Vec<usize> = (0..n)
            .filter(|&i| self.nodes[i].payload.is_some())
            .collect();
        order.sort_by_key(|&i| (rank(self.nodes[i].kind), i));
        let mut payload_at = vec![0usize; n];
        for i in order {
            payload_at[i] = out.len();
            for &w in self.nodes[i].payload.as_deref().unwrap_or_default() {
                out.extend_from_slice(&w.to_le_bytes());
            }
        }
        for (i, node) in self.nodes.iter().enumerate() {
            let base = node_at(i);
            put(&mut out, base + 4, node.kind);
            put(&mut out, base + 8, node.children.len() as u32);
            put(&mut out, base + 12, (children_at[i] - base) as u32);
            let p = if node.payload.is_some() {
                (payload_at[i] - base) as u32
            } else {
                0
            };
            put(&mut out, base + 16, p);
            for (k, child) in node.children.iter().enumerate() {
                let v = child.map_or(0, |t| (node_at(t) as i64 - base as i64) as i32 as u32);
                put(&mut out, children_at[i] + 4 * k, v);
            }
        }
        let mut header = [0u32; 13];
        header[0] = VERSION;
        header[2] = n as u32;
        header[3] = HEADER_SIZE as u32;
        header[5] = self.groups.len() as u32;
        if !self.groups.is_empty() {
            let at = out.len();
            header[6] = (at - 0x14) as u32;
            out.resize(at + 8 * self.groups.len(), 0);
            for (k, group) in self.groups.iter().enumerate() {
                let e = at + 8 * k;
                let off = (out.len() - e) as u32;
                put(&mut out, e, off);
                put(&mut out, e + 4, group.len() as u32);
                for &v in group {
                    out.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        if !self.animations.is_empty() {
            header[7] = out.len() as u32;
            for (k, a) in self.animations.iter().enumerate() {
                for v in [k as u32, a.flags, 0] {
                    out.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        let lines = self.count(KIND_LINE);
        let slots = 2 * (lines + self.count(KIND_CHOICE));
        let padded = |s: &str, lead: usize| {
            let mut r = vec![0u8; lead];
            r.extend_from_slice(s.as_bytes());
            r.push(0);
            r.resize(r.len().next_multiple_of(4), 0);
            r
        };
        let lists: [Vec<Vec<u8>>; 5] = [
            self.labels.iter().map(|s| padded(s, 0)).collect(),
            vec![vec![0x24, 0, 0, 0]; lines],
            vec![vec![0; 20]; lines],
            vec![vec![0; 20]; slots],
            self.animations
                .iter()
                .map(|a| padded(&a.name, 16))
                .collect(),
        ];
        for (li, records) in lists.iter().enumerate() {
            if records.is_empty() {
                continue;
            }
            let at = out.len();
            header[8 + li] = at as u32;
            out.resize(at + 4 * records.len() + 8, 0);
            for (k, r) in records.iter().enumerate() {
                let p = out.len() as u32;
                put(&mut out, at + 4 * k, p);
                out.extend_from_slice(r);
            }
        }
        for (k, v) in header.iter().enumerate() {
            put(&mut out, 4 * k, *v);
        }
        Ok(out)
    }

    /// Checks references between the parts.
    pub fn check(&self) -> Result<()> {
        let n = self.nodes.len();
        for (i, node) in self.nodes.iter().enumerate() {
            if node.children.iter().flatten().any(|&c| c >= n) {
                return Err(Error::InvalidArgument(format!(
                    "node {i} links to a node that does not exist"
                )));
            }
            if node.kind == KIND_LINE && node.payload.as_ref().is_none_or(|p| p.len() < LINE_WORDS)
            {
                return Err(Error::InvalidArgument(format!(
                    "line node {i} needs {LINE_WORDS} payload words"
                )));
            }
            if node.kind == KIND_CHOICE && node.payload.as_ref().is_none_or(|p| p.len() < 16) {
                return Err(Error::InvalidArgument(format!(
                    "choice node {i} needs 16 payload words"
                )));
            }
        }
        if self.labels.len() != CHOICE_SLOTS * self.count(KIND_CHOICE) {
            return Err(Error::InvalidArgument(format!(
                "{} choice label slot(s) for {} choice(s); every choice has {CHOICE_SLOTS}",
                self.labels.len(),
                self.count(KIND_CHOICE)
            )));
        }
        let anims = self.animations.len() as u32;
        if self.groups.iter().flatten().any(|&a| a >= anims) {
            return Err(Error::InvalidArgument(
                "an animation group names a clip that does not exist".into(),
            ));
        }
        if self
            .labels
            .iter()
            .chain(self.animations.iter().map(|a| &a.name))
            .any(|s| s.contains('\0'))
        {
            return Err(Error::InvalidArgument("a name contains NUL".into()));
        }
        Ok(())
    }

    /// Choice nodes in node order; the label slots of choice `k` are
    /// `labels[5k..5k + 5]`.
    pub fn choice_ordinal(&self, node: usize) -> Option<usize> {
        if self.nodes.get(node)?.kind != KIND_CHOICE {
            return None;
        }
        Some(
            self.nodes[..node]
                .iter()
                .filter(|n| n.kind == KIND_CHOICE)
                .count(),
        )
    }

    /// Label of option `option` of choice node `node`: the string table
    /// name, `None` when the option has no label.
    pub fn option_label(&self, node: usize, option: usize) -> Option<&str> {
        let slot = self.nodes.get(node)?.word(CHOICE_LABELS + option)?;
        if slot == NONE || option >= CHOICE_SLOTS {
            return None;
        }
        self.labels
            .get(slot as usize)
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    }

    /// Points option `option` of choice `node` at the string named `name`
    /// (empty: no label).
    pub fn set_option_label(&mut self, node: usize, option: usize, name: &str) -> Result<()> {
        let ordinal = self
            .choice_ordinal(node)
            .ok_or_else(|| Error::InvalidArgument(format!("node {node} is not a choice")))?;
        if option >= CHOICE_SLOTS {
            return Err(Error::InvalidArgument(format!("no option {option}")));
        }
        let payload = self.nodes[node]
            .payload
            .as_mut()
            .expect("checked by choice_ordinal");
        let word = &mut payload[CHOICE_LABELS + option];
        let slot = if *word == NONE {
            (CHOICE_SLOTS * ordinal + option) as u32
        } else {
            *word
        };
        if name.is_empty() {
            *word = NONE;
        } else {
            *word = slot;
        }
        if let Some(label) = self.labels.get_mut(slot as usize) {
            *label = name.to_owned();
        }
        Ok(())
    }

    /// Swaps two options of a choice: their branches and labels.
    pub fn swap_options(&mut self, node: usize, a: usize, b: usize) -> Result<()> {
        let choice = self
            .nodes
            .get_mut(node)
            .filter(|n| n.kind == KIND_CHOICE)
            .ok_or_else(|| Error::InvalidArgument(format!("node {node} is not a choice")))?;
        if a >= CHOICE_SLOTS || b >= CHOICE_SLOTS {
            return Err(Error::InvalidArgument("no such option".into()));
        }
        let payload = choice.payload.as_mut().expect("choices have payloads");
        payload.swap(CHOICE_LABELS + a, CHOICE_LABELS + b);
        let len = choice.children.len().max(a.max(b) + 1);
        choice.children.resize(len, None);
        choice.children.swap(a, b);
        Ok(())
    }

    /// Sets branch `slot` of `node` to `target`.
    pub fn set_child(&mut self, node: usize, slot: usize, target: Option<usize>) -> Result<()> {
        if target.is_some_and(|t| t >= self.nodes.len()) {
            return Err(Error::InvalidArgument("no such target node".into()));
        }
        let n = self
            .nodes
            .get_mut(node)
            .ok_or_else(|| Error::InvalidArgument(format!("no node {node}")))?;
        let child = n
            .children
            .get_mut(slot)
            .ok_or_else(|| Error::InvalidArgument(format!("node {node} has no branch {slot}")))?;
        *child = target;
        Ok(())
    }

    /// Inserts a new line on branch `slot` of `node`: the new line goes to
    /// where the branch went. Its payload is a copy of line `template` with
    /// another speech event. Returns the new node.
    pub fn insert_line(
        &mut self,
        node: usize,
        slot: usize,
        template: usize,
        event: u32,
    ) -> Result<usize> {
        let mut payload = self
            .nodes
            .get(template)
            .filter(|t| t.kind == KIND_LINE)
            .and_then(|t| t.payload.clone())
            .ok_or_else(|| Error::InvalidArgument(format!("node {template} is not a line")))?;
        payload[LINE_EVENT] = event;
        let next = *self
            .nodes
            .get(node)
            .and_then(|n| n.children.get(slot))
            .ok_or_else(|| Error::InvalidArgument(format!("node {node} has no branch {slot}")))?;
        let new = self.nodes.len();
        self.nodes.push(Node {
            kind: KIND_LINE,
            children: vec![next],
            payload: Some(payload),
        });
        self.nodes[node].children[slot] = Some(new);
        self.renumber_lines();
        Ok(new)
    }

    /// Removes a line, a condition or an assignment; branches that led to it
    /// lead to its first branch. Node numbers above it move down by one.
    pub fn remove_node(&mut self, node: usize) -> Result<()> {
        let removed = self
            .nodes
            .get(node)
            .ok_or_else(|| Error::InvalidArgument(format!("no node {node}")))?;
        if node == 0 || matches!(removed.kind, KIND_CHOICE | KIND_END) {
            return Err(Error::InvalidArgument(
                "the start node, choices and the end node cannot be removed".into(),
            ));
        }
        let next = removed.children.first().copied().flatten();
        let next = next.filter(|&t| t != node);
        self.nodes.remove(node);
        let fix = |t: usize| if t > node { t - 1 } else { t };
        for n in &mut self.nodes {
            for c in &mut n.children {
                *c = match *c {
                    Some(t) if t == node => next.map(fix),
                    Some(t) => Some(fix(t)),
                    None => None,
                };
            }
        }
        self.renumber_lines();
        Ok(())
    }

    /// Gives the lines their index in node order (the game's files do).
    pub fn renumber_lines(&mut self) {
        let mut k = 0u32;
        for n in &mut self.nodes {
            if n.kind == KIND_LINE
                && let Some(p) = n.payload.as_mut()
                && p.len() > LINE_INDEX + 1
            {
                p[LINE_INDEX] = k;
                p[LINE_INDEX + 1] = k;
                k += 1;
            }
        }
    }

    /// Nodes that lead to `node`: `(parent, branch)`.
    pub fn parents(&self, node: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for (i, n) in self.nodes.iter().enumerate() {
            for (k, c) in n.children.iter().enumerate() {
                if *c == Some(node) {
                    out.push((i, k));
                }
            }
        }
        out
    }

    /// Clip names of an animation group.
    pub fn group_clips(&self, group: u32) -> Vec<&str> {
        self.groups
            .get(group as usize)
            .map(|g| {
                g.iter()
                    .filter_map(|&a| self.animations.get(a as usize))
                    .map(|a| a.name.as_str())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Speech event hashes of the lines, in node order.
    pub fn events(&self) -> Vec<(usize, u32)> {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.kind == KIND_LINE)
            .filter_map(|(i, n)| Some((i, n.word(LINE_EVENT)?)))
            .collect()
    }
}

/// A condition or assignment payload: `{value type, scope, variable, value}`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Test {
    /// 0 bool, 1 int, 2 float.
    pub value_type: u32,
    pub scope: u32,
    /// Engine query or variable number (`0x0C`..`0x12`: NPC flags 0..6, …).
    pub variable: u32,
    pub raw_value: u32,
}

impl Test {
    pub fn of(node: &Node) -> Option<Self> {
        let p = node.payload.as_ref()?;
        if p.len() != 4 {
            return None;
        }
        Some(Self {
            value_type: p[0],
            scope: p[1],
            variable: p[2],
            raw_value: p[3],
        })
    }

    /// The value as text.
    pub fn value(&self) -> String {
        match self.value_type {
            0 => (if self.raw_value & 0xFF != 0 {
                "true"
            } else {
                "false"
            })
            .into(),
            1 => (self.raw_value as i32).to_string(),
            2 => f32::from_bits(self.raw_value).to_string(),
            _ => format!("{:#x}", self.raw_value),
        }
    }
}

/// One-line summary of a node (without text lookups).
pub fn summary(d: &Dialogue, i: usize) -> String {
    let node = &d.nodes[i];
    match node.kind {
        KIND_LINE => {
            let speaker = node.word(LINE_SPEAKER).unwrap_or(0);
            let event = node.word(LINE_EVENT).unwrap_or(0);
            format!("actor {speaker} says event {event:08x}")
        }
        KIND_CHOICE => {
            let labels: Vec<&str> = (0..CHOICE_SLOTS)
                .filter_map(|o| d.option_label(i, o))
                .collect();
            format!("choice: {}", labels.join(" | "))
        }
        KIND_SET_FLAG => format!(
            "set NPC flag {}",
            node.word(0).unwrap_or(0).wrapping_sub(0x13) as i32
        ),
        KIND_SET_FLAG_B => format!(
            "set NPC flag B {}",
            node.word(0).unwrap_or(0).wrapping_sub(0x1A) as i32
        ),
        KIND_END => "end".into(),
        k => match Test::of(node) {
            Some(t) if k == KIND_ASSIGN => format!("var {:#x} := {}", t.variable, t.value()),
            Some(t) => format!("{} var {:#x}, {}", kind_name(k), t.variable, t.value()),
            None => kind_name(k),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dialogue like the smallest of the game: test, flag, assign, two
    /// lines, end.
    pub(crate) fn sample() -> Dialogue {
        let line = |speaker: u32, index: u32, event: u32| {
            let mut p = vec![
                speaker,
                0x14,
                8,
                8,
                1,
                0,
                index,
                index,
                event,
                NONE,
                0,
                NONE,
                NONE,
                0,
                0x1AD4,
                0x3F80_0000,
            ];
            p[LINE_ANIM_SPEAKER] = 0;
            p
        };
        Dialogue {
            nodes: vec![
                Node {
                    kind: KIND_IF_EQUAL,
                    children: vec![Some(4), Some(1)],
                    payload: Some(vec![0, 0, 0xE, 0]),
                },
                Node {
                    kind: KIND_SET_FLAG,
                    children: vec![Some(2)],
                    payload: Some(vec![0x15]),
                },
                Node {
                    kind: KIND_ASSIGN,
                    children: vec![Some(3)],
                    payload: Some(vec![0, 0, 0x23, 1]),
                },
                Node {
                    kind: KIND_LINE,
                    children: vec![Some(5)],
                    payload: Some(line(2, 0, 0x44C5_F8C2)),
                },
                Node {
                    kind: KIND_LINE,
                    children: vec![Some(5)],
                    payload: Some(line(2, 1, 0x44C5_F8C2)),
                },
                Node {
                    kind: KIND_END,
                    children: vec![],
                    payload: None,
                },
            ],
            groups: vec![vec![0, 1]],
            animations: vec![
                Animation {
                    name: "f2f_player_talk_shrug".into(),
                    flags: 0x40044,
                },
                Animation {
                    name: "f2f_player_palm_empty".into(),
                    flags: 0x40044,
                },
            ],
            labels: vec![],
        }
    }

    fn with_choice() -> Dialogue {
        let mut d = sample();
        let mut p = vec![0x4416_0000, 4, 4, 5, 5, 0x40041, 0, 0, 0x1080, 8, 8];
        p.extend([0, 1, NONE, NONE, NONE]);
        d.nodes[0].children[1] = Some(6);
        d.nodes.push(Node {
            kind: KIND_CHOICE,
            children: vec![Some(1), Some(5), None, None, None],
            payload: Some(p),
        });
        d.labels = vec![
            "hud_csa_f2f_choicea".into(),
            "hud_csa_f2f_cancel".into(),
            String::new(),
            String::new(),
            String::new(),
        ];
        d
    }

    #[test]
    fn round_trips() {
        for d in [sample(), with_choice()] {
            let bytes = d.to_bytes().unwrap();
            assert_eq!(Dialogue::parse(&bytes).unwrap(), d);
        }
        // The game's smallest dialogue has this size.
        assert_eq!(sample().to_bytes().unwrap().len(), 680);
    }

    #[test]
    fn edits_choices() {
        let mut d = with_choice();
        assert_eq!(d.option_label(6, 0), Some("hud_csa_f2f_choicea"));
        assert_eq!(d.option_label(6, 2), None);
        d.set_option_label(6, 2, "hud_csa_not_now").unwrap();
        assert_eq!(d.option_label(6, 2), Some("hud_csa_not_now"));
        d.swap_options(6, 0, 1).unwrap();
        assert_eq!(d.option_label(6, 0), Some("hud_csa_f2f_cancel"));
        assert_eq!(d.nodes[6].children[0], Some(5));
        d.set_option_label(6, 1, "").unwrap();
        assert_eq!(d.option_label(6, 1), None);
        let back = Dialogue::parse(&d.to_bytes().unwrap()).unwrap();
        assert_eq!(back, d);
        assert!(d.set_option_label(0, 0, "x").is_err());
    }

    #[test]
    fn inserts_and_removes_lines() {
        let mut d = sample();
        let new = d.insert_line(1, 0, 3, 0x1234).unwrap();
        assert_eq!(new, 6);
        assert_eq!(d.nodes[1].children[0], Some(6));
        assert_eq!(d.nodes[6].children[0], Some(2));
        let indices: Vec<u32> = d
            .nodes
            .iter()
            .filter(|n| n.kind == KIND_LINE)
            .map(|n| n.word(LINE_INDEX).unwrap())
            .collect();
        assert_eq!(indices, [0, 1, 2]);
        let back = Dialogue::parse(&d.to_bytes().unwrap()).unwrap();
        assert_eq!(back, d);
        d.remove_node(6).unwrap();
        assert_eq!(d, sample());
        assert!(d.remove_node(5).is_err());
        assert!(d.remove_node(0).is_err());
        d.remove_node(3).unwrap();
        assert_eq!(d.nodes[2].children[0], Some(4));
        assert_eq!(d.parents(4).len(), 2);
    }

    #[test]
    fn rejects_bad_input() {
        let bytes = sample().to_bytes().unwrap();
        let mut b = bytes.clone();
        b[0] = 5;
        assert!(Dialogue::parse(&b).is_err());
        let mut b = bytes.clone();
        b[8] = 200;
        assert!(Dialogue::parse(&b).is_err());
        assert!(Dialogue::parse(&bytes[..100]).is_err());
        let mut d = sample();
        d.nodes[0].children[0] = Some(99);
        assert!(d.to_bytes().is_err());
        let mut d = with_choice();
        d.labels.pop();
        assert!(d.to_bytes().is_err());
    }

    #[test]
    fn summarises() {
        let d = with_choice();
        assert_eq!(summary(&d, 0), "if == var 0xe, false");
        assert_eq!(summary(&d, 1), "set NPC flag 2");
        assert_eq!(summary(&d, 2), "var 0x23 := true");
        assert!(summary(&d, 6).contains("hud_csa_f2f_choicea"));
        assert_eq!(
            d.group_clips(0),
            ["f2f_player_talk_shrug", "f2f_player_palm_empty"]
        );
    }
}
