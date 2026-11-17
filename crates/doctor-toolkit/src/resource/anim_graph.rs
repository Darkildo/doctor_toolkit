//! The animation graph of a bank (`bnk`): named entries, each a tree of
//! nodes whose leaves play clips ([`AnimGraph`]), with editing, slot
//! numbering, checks and a JSON form.
//!
//! In the image (`docs/formats/anim-graph.md`) every node is
//!
//! ```text
//! u8  type (low 6 bits; the game masks with 0x3F and knows types below 0x23)
//! u8  children
//! u16 number: the node's slot in the per-entry slot array
//! ptr parameters (or null)
//! ptr extra data: what the parent needs to know of the child (or null)
//! ptr child × children   | clip nodes (type 0x16): ptr clip record
//! ```
//!
//! laid out depth first after the entry record `{ptr name, ptr root}` and
//! the name, each object aligned to 16 bytes. The game evaluates a node
//! through three tables of functions indexed by the type (`godfather2.exe`
//! 0x00F294A8 sampling, 0x00F29538 duration, 0x00F295C8 update); what the
//! parameters mean comes from those functions ([`kind_info`]).
//!
//! Slots: each entry has an array of floats; a node's number is its slot
//! (the duration of the node, written by its parent), and some types keep
//! state in further slots named by their parameters (the current and
//! previous child of a choice, a normalised control value). Numbers are
//! given depth first — the node, then its state slots, then its children
//! ([`AnimGraph::renumber`]); the header word 0x20 is the largest slot count
//! of any entry.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::resource::anim::TICKS_PER_FRAME;
use crate::resource::anim_image::{BankImage, Block, Target};
use crate::{Error, Result};

/// Node type of clip nodes (leaves).
pub const NODE_CLIP: u8 = 0x16;
/// Types the game's tables have functions for: 1..=0x22 except 0x18.
pub const KIND_LIMIT: u8 = 0x23;

/// The game's animation controls, in the order of its table
/// (`godfather2.exe` 0x00E52698, 43 entries `{name, read, write}`): the
/// values that blend (type 5), select (type 6) and copy (type 0x21) nodes
/// read by index.
pub const CONTROLS: [&str; 43] = [
    "null entry",
    "true",
    "false",
    "pl stick speed",
    "pl run turn blend",
    "turn angle",
    "pl grab strangle blend",
    "hit strength blend",
    "pl strafe blend",
    "grab movement blend",
    "pl aim circumference",
    "pl aim intensity",
    "pl peek blend",
    "npc reaction blend",
    "l toe effector",
    "l foot effector",
    "r toe effector",
    "r foot effector",
    "l hand effector",
    "r hand effector",
    "aim pitch",
    "engage idle",
    "vehicle steering",
    "idle time",
    "sprint",
    "combat anim type",
    "prev pl stick speed",
    "pl stick speed raw",
    "pl adjust turn",
    "prev pl turn angle",
    "gender",
    "family category",
    "npc tension zone",
    "pl walk run threshold",
    "pl velocity",
    "prev pl velocity",
    "posture",
    "pl run",
    "pl walk",
    "engage idle float",
    "last damage level",
    "pl lunge attack",
    "pl run turn blend raw",
];

/// Name of control `index`, if the game has one.
pub fn control_name(index: u32) -> Option<&'static str> {
    CONTROLS.get(index as usize).copied()
}

/// How a parameter word is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum FieldType {
    U32,
    F32,
    /// Two u16, low half first.
    Pair,
}

/// A named parameter word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Field {
    pub name: &'static str,
    pub ty: FieldType,
    /// What the word means.
    pub help: &'static str,
}

const fn u(name: &'static str, help: &'static str) -> Field {
    Field {
        name,
        ty: FieldType::U32,
        help,
    }
}

const fn f(name: &'static str, help: &'static str) -> Field {
    Field {
        name,
        ty: FieldType::F32,
        help,
    }
}

const fn p(name: &'static str, help: &'static str) -> Field {
    Field {
        name,
        ty: FieldType::Pair,
        help,
    }
}

/// How many children a type takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Arity {
    /// A clip node: no children, a clip.
    Clip,
    /// Exactly this many.
    Exactly(u8),
    /// From `min` to `max`.
    Range(u8, u8),
}

impl Arity {
    pub fn allows(self, n: usize) -> bool {
        match self {
            Arity::Clip => n == 0,
            Arity::Exactly(k) => n == k as usize,
            Arity::Range(a, b) => (a as usize..=b as usize).contains(&n),
        }
    }

    /// Children can be added and removed.
    pub fn variable(self) -> bool {
        matches!(self, Arity::Range(..))
    }
}

/// What the game does with a node type (from its functions in the exe).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct KindInfo {
    pub kind: u8,
    /// Short description of the behaviour (not a name from the game).
    pub label: &'static str,
    pub help: &'static str,
    pub arity: Arity,
    /// Parameter words; `None`: the type has no parameter block.
    pub params: Option<&'static [Field]>,
    /// Extra data its children carry.
    pub child_extra: Option<&'static [Field]>,
    /// Sampling function (table 0x00F294A8).
    pub sample_fn: u32,
}

static SEQUENCE: [Field; 2] = [
    u(
        "priority",
        "an entry root: a later bank replaces an entry of the same name only with a higher priority",
    ),
    u("tag", "0x12032045 on every entry root of the game"),
];
static RANDOM: [Field; 3] = [
    p(
        "slots",
        "state slots: current child, previous child (numbered by the graph)",
    ),
    p(
        "mode_time",
        "when to pick (0: when the child ends, 1: the same but never the current child again, 2: on every update, 3: once) and the slot of the start time",
    ),
    f(
        "crossfade",
        "ticks of blending into the next child (0: none)",
    ),
];
static BLEND: [Field; 4] = [
    u("control", "control read each update"),
    f("min", "control value of the first child"),
    f("range", "control span to the second child"),
    u(
        "slot",
        "slot of the normalised value (control − min) / range",
    ),
];
static SELECT: [Field; 4] = [
    p("slots", "state slots: current child, previous child"),
    p(
        "control_time",
        "control read, and the slot of the start time",
    ),
    u(
        "immediate",
        "1: re-select on every update; 0: only when the child ends",
    ),
    f("crossfade", "ticks of blending into the new child"),
];
static SCALE_TIME: [Field; 1] = [f(
    "scale",
    "duration factor: the child runs 1/scale times as fast",
)];
static OPTIONAL: [Field; 2] = [
    u("flag", "0 in 184 of 186 nodes"),
    u(
        "slot",
        "slot the parent sequence toggles between 1.0 and a mark",
    ),
];
static ROOT_SCALE: [Field; 3] = [
    f("x", "root motion factor along x"),
    f("y", "root motion factor along y"),
    f("z", "root motion factor along z"),
];
static SPEED: [Field; 4] = [
    u("slot", "slot with the speed"),
    u("mode", "0: speed = motion × scale + offset"),
    f("offset", "speed offset"),
    f("scale", "speed per unit of motion"),
];
static CLIP: [Field; 3] = [
    f("start", "start tick (0 everywhere)"),
    f(
        "duration",
        "length in ticks: frames × 2 when looping, (frames − 1) × 2 otherwise",
    ),
    f(
        "ticks_per_frame",
        "2.0 everywhere: clips run at 29.97 frames/s",
    ),
];
static HASH: [Field; 1] = [u("hash", "0xEDFCDFBA in every node of the game")];
static FEET: [Field; 8] = [
    p("bones_a", "bone indices"),
    p("bones_b", "bone indices"),
    p("bones_c", "bone indices"),
    f("a", "0.012 everywhere"),
    f("b", "0.05 everywhere"),
    f("c", "−0.1 or −0"),
    u("d", "0 everywhere"),
    u("e", "0 everywhere"),
];
static COPY: [Field; 2] = [
    u("from", "control read"),
    u("to", "control written with the value"),
];
static WEIGHT: [Field; 1] = [f("weight", "chance of the child against its siblings")];
static RANGE: [Field; 2] = [
    f("min", "the child plays when the control is ≥ min…"),
    f("max", "…and ≤ max"),
];
static FLAG: [Field; 1] = [u(
    "flag",
    "0 or 1 (443 and 225 children); the time-scale functions do not read it",
)];

const fn info(
    kind: u8,
    label: &'static str,
    help: &'static str,
    arity: Arity,
    params: Option<&'static [Field]>,
    child_extra: Option<&'static [Field]>,
    sample_fn: u32,
) -> KindInfo {
    KindInfo {
        kind,
        label,
        help,
        arity,
        params,
        child_extra,
        sample_fn,
    }
}

static KINDS: [KindInfo; 16] = [
    info(
        0x02,
        "sequence",
        "plays the children one after another (an entry's root: one child)",
        Arity::Range(1, 64),
        Some(&SEQUENCE),
        None,
        0x0058_AA10,
    ),
    info(
        0x03,
        "random",
        "picks a child at random by the weights in the children's extra data",
        Arity::Range(1, 64),
        Some(&RANDOM),
        Some(&WEIGHT),
        0x0058_B4F0,
    ),
    info(
        0x04,
        "reverse",
        "plays the child backwards",
        Arity::Exactly(1),
        None,
        None,
        0x0058_B540,
    ),
    info(
        0x05,
        "blend by control",
        "blends two children by a control value",
        Arity::Exactly(2),
        Some(&BLEND),
        None,
        0x0058_BA30,
    ),
    info(
        0x06,
        "select by control",
        "plays the child whose range in its extra data holds the control value",
        Arity::Range(1, 64),
        Some(&SELECT),
        Some(&RANGE),
        0x0058_BBA0,
    ),
    info(
        0x08,
        "time scale",
        "stretches the child's time by a factor",
        Arity::Exactly(1),
        Some(&SCALE_TIME),
        Some(&FLAG),
        0x0058_BC70,
    ),
    info(
        0x09,
        "optional part",
        "a part of a sequence the parent switches by a slot",
        Arity::Range(1, 64),
        Some(&OPTIONAL),
        None,
        0x0058_BCF0,
    ),
    info(
        0x0A,
        "root motion scale",
        "multiplies the root motion of the child",
        Arity::Exactly(1),
        Some(&ROOT_SCALE),
        None,
        0x0058_BD30,
    ),
    info(
        0x0D,
        "add",
        "adds the second child (a difference pose) to the first; with three, adds second − third",
        Arity::Range(2, 3),
        None,
        None,
        0x0058_BE80,
    ),
    info(
        0x0E,
        "difference",
        "the first child relative to the second (a pose to add elsewhere)",
        Arity::Exactly(2),
        None,
        None,
        0x0058_C340,
    ),
    info(
        0x12,
        "loop over",
        "the second child loops over the first and is combined with it",
        Arity::Exactly(2),
        None,
        None,
        0x0058_C9C0,
    ),
    info(
        0x14,
        "speed",
        "plays the child at a speed from a slot or from the motion",
        Arity::Exactly(1),
        Some(&SPEED),
        None,
        0x0058_D200,
    ),
    info(
        0x16,
        "clip",
        "plays a clip of the bank",
        Arity::Clip,
        Some(&CLIP),
        None,
        0x0059_0C40,
    ),
    info(
        0x19,
        "sequence with call",
        "a sequence that afterwards calls engine code with a hash",
        Arity::Exactly(1),
        Some(&HASH),
        None,
        0x0058_BC00,
    ),
    info(
        0x1C,
        "feet",
        "the child's pose with the legs adjusted (effector controls 14–17)",
        Arity::Exactly(1),
        Some(&FEET),
        None,
        0x0058_F2D0,
    ),
    info(
        0x21,
        "copy control",
        "copies a control into another (the previous value), then plays the child",
        Arity::Exactly(1),
        Some(&COPY),
        None,
        0x0058_FCE0,
    ),
];

/// What is known of node type `kind` (low 6 bits), if it occurs in the game.
pub fn kind_info(kind: u8) -> Option<&'static KindInfo> {
    KINDS.iter().find(|k| k.kind == kind & 0x3F)
}

/// Every type found in the game's banks.
pub fn kinds() -> &'static [KindInfo] {
    &KINDS
}

/// The game has functions for this type (1..=0x22 except 0x18).
pub fn kind_known_to_game(kind: u8) -> bool {
    let k = kind & 0x3F;
    k != 0 && k < KIND_LIMIT && k != 0x18
}

/// Word `i` of a parameter list, typed by `fields`.
pub fn field(fields: Option<&'static [Field]>, i: usize) -> Field {
    fields
        .and_then(|f| f.get(i).copied())
        .unwrap_or(u("word", "not known"))
}

/// Slots a type keeps state in: (word, half) of its parameters; half
/// `None` is the whole word, `Some(0|1)` a u16 of it.
fn state_slots(kind: u8) -> &'static [(usize, Option<usize>)] {
    match kind & 0x3F {
        0x03 | 0x06 => &[(0, Some(0)), (0, Some(1)), (1, Some(1))],
        0x05 => &[(3, None)],
        0x09 => &[(1, None)],
        0x14 => &[(0, None)],
        _ => &[],
    }
}

/// A node of the graph.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphNode {
    /// The type byte as stored (type in the low 6 bits).
    pub kind: u8,
    /// Slot of the node (see [`AnimGraph::renumber`]).
    pub number: u16,
    /// Parameter words; `None`: null pointer.
    pub params: Option<Vec<u32>>,
    /// Extra data for the parent; `None`: null pointer.
    pub extra: Option<Vec<u32>>,
    /// Children (indices into [`AnimGraph::nodes`]).
    pub children: Vec<usize>,
    /// Clip nodes: the clip (index in the bank); `None`: null pointer.
    pub clip: Option<u32>,
}

impl GraphNode {
    pub fn kind(&self) -> u8 {
        self.kind & 0x3F
    }

    pub fn is_clip(&self) -> bool {
        self.kind() == NODE_CLIP
    }

    pub fn param(&self, i: usize) -> Option<u32> {
        self.params.as_ref()?.get(i).copied()
    }

    pub fn param_f32(&self, i: usize) -> Option<f32> {
        self.param(i).map(f32::from_bits)
    }

    fn set_half(&mut self, word: usize, half: Option<usize>, value: u32) {
        let Some(w) = self.params.as_mut().and_then(|p| p.get_mut(word)) else {
            return;
        };
        *w = match half {
            None => value,
            Some(0) => (*w & 0xFFFF_0000) | (value & 0xFFFF),
            Some(_) => (*w & 0xFFFF) | (value << 16),
        };
    }

    fn get_half(&self, word: usize, half: Option<usize>) -> Option<u32> {
        let w = self.param(word)?;
        Some(match half {
            None => w,
            Some(0) => w & 0xFFFF,
            Some(_) => w >> 16,
        })
    }
}

/// A named entry: what the game asks for by name.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphEntry {
    pub name: String,
    pub root: usize,
}

/// The node trees of a bank.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnimGraph {
    pub entries: Vec<GraphEntry>,
    pub nodes: Vec<GraphNode>,
}

fn bad(reason: impl Into<String>) -> Error {
    Error::malformed("bnk graph", 0, reason)
}

fn invalid(reason: impl Into<String>) -> Error {
    Error::InvalidArgument(reason.into())
}

fn word(block: &Block, at: u32) -> Result<u32> {
    crate::io::read_u32(&block.data, at as usize, crate::io::Endian::Little)
}

impl AnimGraph {
    /// Reads the graph block of a bank.
    pub fn parse(bank: &BankImage) -> Result<Self> {
        let g = &bank.graph;
        // Object starts: every pointer target in the graph and every entry.
        let mut starts: BTreeSet<u32> = g
            .pointers
            .values()
            .filter_map(|t| match t {
                Target::Graph(o) => Some(*o),
                _ => None,
            })
            .collect();
        for e in &bank.entries {
            if let Target::Graph(o) = e {
                starts.insert(*o);
            }
        }
        let end = g.data.len() as u32;
        // Words of the object at `at`, up to the next object, without the
        // final padding (rounded up to whole words).
        let object = |at: u32| -> Result<Vec<u32>> {
            let next = starts
                .range(at + 1..)
                .next()
                .copied()
                .unwrap_or(end)
                .min(end);
            if next <= at {
                return Err(bad(format!("object at {at:#x} is empty")));
            }
            let mut len = (next - at) as usize;
            let bytes = &g.data[at as usize..next as usize];
            let mut cut = 0;
            while len > 0 && cut < 15 && bytes[len - 1] == b'_' {
                len -= 1;
                cut += 1;
            }
            let words = len.div_ceil(4);
            if at as usize + 4 * words > g.data.len() {
                return Err(bad(format!("object at {at:#x} runs out of the graph")));
            }
            (0..words).map(|k| word(g, at + 4 * k as u32)).collect()
        };
        let mut out = AnimGraph::default();
        let mut seen = BTreeMap::new();
        for e in &bank.entries {
            let Target::Graph(e) = *e else {
                return Err(bad("an entry outside the graph"));
            };
            let name = match g.pointers.get(&e) {
                Some(&Target::Graph(n)) => g
                    .cstr(n)
                    .map(|s| String::from_utf8_lossy(s).into_owned())
                    .ok_or_else(|| bad("entry name without an end"))?,
                _ => return Err(bad(format!("entry at {e:#x} has no name in the graph"))),
            };
            let root = match g.pointers.get(&(e + 4)) {
                Some(&Target::Graph(r)) => r,
                _ => return Err(bad(format!("entry `{name}` has no root"))),
            };
            let root = out.parse_node(bank, root, &object, &mut seen, 0)?;
            out.entries.push(GraphEntry { name, root });
        }
        Ok(out)
    }

    fn parse_node(
        &mut self,
        bank: &BankImage,
        at: u32,
        object: &dyn Fn(u32) -> Result<Vec<u32>>,
        seen: &mut BTreeMap<u32, usize>,
        depth: usize,
    ) -> Result<usize> {
        let g = &bank.graph;
        if depth > 256 {
            return Err(bad("graph too deep"));
        }
        if seen.contains_key(&at) {
            return Err(bad(format!("node at {at:#x} has two parents")));
        }
        let head = word(g, at)?;
        let kind = head as u8;
        let count = (head >> 8) as u8 as usize;
        let number = (head >> 16) as u16;
        let ptr = |off: u32| g.pointers.get(&(at + off)).copied();
        let block = |off: u32| -> Result<Option<Vec<u32>>> {
            match ptr(off) {
                Some(Target::Graph(o)) => Ok(Some(object(o)?)),
                Some(_) => Err(bad(format!("node at {at:#x}: data outside the graph"))),
                None if word(g, at + off)? == 0 => Ok(None),
                None => Err(bad(format!("node at {at:#x}: data is not a pointer"))),
            }
        };
        let params = block(4)?;
        let extra = block(8)?;
        let index = self.nodes.len();
        seen.insert(at, index);
        self.nodes.push(GraphNode {
            kind,
            number,
            params,
            extra,
            children: Vec::new(),
            clip: None,
        });
        if kind & 0x3F == NODE_CLIP {
            if count != 0 {
                return Err(bad(format!("clip node at {at:#x} with children")));
            }
            self.nodes[index].clip = match ptr(12) {
                Some(Target::Clip(c)) => Some(c),
                None if word(g, at + 12)? == 0 => None,
                _ => return Err(bad(format!("clip node at {at:#x}: not a clip"))),
            };
            return Ok(index);
        }
        if count == 0 {
            return Err(bad(format!(
                "node at {at:#x} of type {kind:#x} without children"
            )));
        }
        let mut children = Vec::with_capacity(count);
        for k in 0..count as u32 {
            match ptr(12 + 4 * k) {
                Some(Target::Graph(c)) => {
                    children.push(self.parse_node(bank, c, object, seen, depth + 1)?)
                }
                _ => return Err(bad(format!("node at {at:#x}: child {k} is not a node"))),
            }
        }
        self.nodes[index].children = children;
        Ok(index)
    }

    /// The graph block as the game's exporter lays it out.
    pub fn to_block(&self) -> Result<Block> {
        Ok(self.layout()?.0)
    }

    /// The graph block and the offsets of the entry records.
    fn layout(&self) -> Result<(Block, Vec<u32>)> {
        let mut g = Block::default();
        let mut records = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            let at = g.push(&[0; 8], 16);
            records.push(at);
            let mut name = entry.name.as_bytes().to_vec();
            name.push(0);
            let name_at = g.push(&name, 16);
            g.pointers.insert(at, Target::Graph(name_at));
            let root = self.write_node(&mut g, entry.root, 0)?;
            g.pointers.insert(at + 4, Target::Graph(root));
        }
        Ok((g, records))
    }

    fn write_node(&self, g: &mut Block, index: usize, depth: usize) -> Result<u32> {
        let node = self
            .nodes
            .get(index)
            .ok_or_else(|| invalid(format!("no node {index}")))?;
        if depth > 256 {
            return Err(invalid("graph too deep (a cycle?)"));
        }
        let clip = node.is_clip();
        if clip && !node.children.is_empty() {
            return Err(invalid(format!("clip node {index} has children")));
        }
        if !clip && node.children.is_empty() {
            return Err(invalid(format!("node {index} has no children")));
        }
        if node.children.len() > 255 {
            return Err(invalid(format!("node {index} has too many children")));
        }
        let slots = node.children.len().max(1);
        let mut record = vec![0u8; 12 + 4 * slots];
        record[0] = node.kind;
        record[1] = node.children.len() as u8;
        record[2..4].copy_from_slice(&node.number.to_le_bytes());
        let at = g.push(&record, 16);
        let words = |w: &[u32]| w.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        if let Some(params) = &node.params {
            let p = g.push(&words(params), 16);
            g.pointers.insert(at + 4, Target::Graph(p));
        }
        if let Some(extra) = &node.extra {
            let x = g.push(&words(extra), 16);
            g.pointers.insert(at + 8, Target::Graph(x));
        }
        if clip && let Some(c) = node.clip {
            g.pointers.insert(at + 12, Target::Clip(c));
        }
        for (k, &child) in node.children.iter().enumerate() {
            let c = self.write_node(g, child, depth + 1)?;
            g.pointers.insert(at + 12 + 4 * k as u32, Target::Graph(c));
        }
        Ok(at)
    }

    /// Nodes of the tree under `root`, depth first, with their parents.
    pub fn walk(&self, root: usize) -> Vec<(usize, Option<usize>)> {
        let mut out = Vec::new();
        let mut stack = vec![(root, None)];
        let mut seen = BTreeSet::new();
        while let Some((n, parent)) = stack.pop() {
            if !seen.insert(n) || n >= self.nodes.len() {
                continue;
            }
            out.push((n, parent));
            for &c in self.nodes[n].children.iter().rev() {
                stack.push((c, Some(n)));
            }
        }
        out
    }

    /// The parent of every node (`None` for roots and unreachable nodes).
    pub fn parents(&self) -> Vec<Option<usize>> {
        let mut out = vec![None; self.nodes.len()];
        for (i, n) in self.nodes.iter().enumerate() {
            for &c in &n.children {
                if let Some(p) = out.get_mut(c) {
                    *p = Some(i);
                }
            }
        }
        out
    }

    /// The entry whose tree holds `node`.
    pub fn entry_of(&self, node: usize) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| self.walk(e.root).iter().any(|&(n, _)| n == node))
    }

    /// Gives every node and state slot its number, depth first: the node,
    /// its state slots, then its children. Returns the largest slot count
    /// of an entry (header word 0x20).
    pub fn renumber(&mut self) -> u32 {
        let mut most = 0;
        for e in 0..self.entries.len() {
            let mut next: u32 = 0;
            for (n, _) in self.walk(self.entries[e].root) {
                let node = &mut self.nodes[n];
                node.number = next as u16;
                next += 1;
                for &(w, half) in state_slots(node.kind) {
                    if node.param(w).is_some() {
                        node.set_half(w, half, next);
                        next += 1;
                    }
                }
            }
            most = most.max(next);
        }
        most
    }

    /// Slot count the graph needs (header word 0x20), from the numbers as
    /// they are.
    pub fn slot_count(&self) -> u32 {
        let mut most = 0u32;
        for n in &self.nodes {
            most = most.max(n.number as u32 + 1);
            for &(w, half) in state_slots(n.kind) {
                if let Some(v) = n.get_half(w, half) {
                    most = most.max(v + 1);
                }
            }
        }
        most
    }

    /// Writes the graph into the bank: graph block, entry list and slot count.
    pub fn store(&self, bank: &mut BankImage) -> Result<()> {
        let (block, entries) = self.layout()?;
        bank.graph = block;
        bank.entries = entries.into_iter().map(Target::Graph).collect();
        bank.node_count = self.slot_count();
        Ok(())
    }

    // ---------------------------------------------------------------
    // Edits.

    fn node_mut(&mut self, node: usize) -> Result<&mut GraphNode> {
        self.nodes
            .get_mut(node)
            .ok_or_else(|| invalid(format!("no node {node}")))
    }

    /// Sets parameter word `i` of `node`.
    pub fn set_param(&mut self, node: usize, i: usize, value: u32) -> Result<()> {
        let n = self.node_mut(node)?;
        let w = n
            .params
            .as_mut()
            .and_then(|p| p.get_mut(i))
            .ok_or_else(|| invalid(format!("node {node} has no parameter {i}")))?;
        *w = value;
        Ok(())
    }

    /// Sets extra word `i` of `node`.
    pub fn set_extra(&mut self, node: usize, i: usize, value: u32) -> Result<()> {
        let n = self.node_mut(node)?;
        let w = n
            .extra
            .as_mut()
            .and_then(|p| p.get_mut(i))
            .ok_or_else(|| invalid(format!("node {node} has no extra word {i}")))?;
        *w = value;
        Ok(())
    }

    /// Points clip node `node` at clip `clip`.
    pub fn set_clip(&mut self, node: usize, clip: u32) -> Result<()> {
        let n = self.node_mut(node)?;
        if !n.is_clip() {
            return Err(invalid(format!("node {node} is not a clip node")));
        }
        n.clip = Some(clip);
        Ok(())
    }

    /// The extra data a new child of `parent` needs.
    fn default_extra(&self, parent: usize) -> Option<Vec<u32>> {
        match self.nodes[parent].kind() {
            0x03 => Some(vec![1f32.to_bits()]),
            0x06 => {
                // After the last sibling's range.
                let top = self.nodes[parent]
                    .children
                    .iter()
                    .filter_map(|&c| self.nodes[c].extra.as_ref()?.get(1).copied())
                    .map(f32::from_bits)
                    .fold(-1f32, f32::max);
                let v = (top + 1.0).max(0.0);
                Some(vec![v.to_bits(), v.to_bits()])
            }
            0x08 => Some(vec![0]),
            _ => None,
        }
    }

    /// Adds a clip node playing `clip` as the last child of `parent` (a
    /// sequence, random or select node); returns it. Its duration comes
    /// from the clip (`frames`, `looping`); numbers are given again.
    pub fn add_clip_node(
        &mut self,
        parent: usize,
        clip: u32,
        frames: u32,
        looping: bool,
    ) -> Result<usize> {
        let p = self
            .nodes
            .get(parent)
            .ok_or_else(|| invalid(format!("no node {parent}")))?;
        let arity = kind_info(p.kind).map(|k| k.arity);
        if !arity.is_some_and(Arity::variable) {
            return Err(invalid(format!(
                "node {parent} (type {:#04x}) takes a fixed number of children",
                p.kind()
            )));
        }
        if p.children.len() >= 255 {
            return Err(invalid("too many children"));
        }
        let duration = if looping {
            frames as f32
        } else {
            (frames as f32 - 1.0).max(0.0)
        } * TICKS_PER_FRAME;
        let extra = self.default_extra(parent);
        let node = GraphNode {
            kind: NODE_CLIP,
            number: 0,
            params: Some(vec![0, duration.to_bits(), TICKS_PER_FRAME.to_bits()]),
            extra,
            children: Vec::new(),
            clip: Some(clip),
        };
        self.nodes.push(node);
        let index = self.nodes.len() - 1;
        self.nodes[parent].children.push(index);
        let map = self.compact();
        self.renumber();
        Ok(map[index])
    }

    /// Removes `node` and its subtree. Only children of nodes with a
    /// variable number of children can go, and a parent keeps at least one.
    pub fn remove_node(&mut self, node: usize) -> Result<()> {
        let parents = self.parents();
        let parent = parents
            .get(node)
            .copied()
            .flatten()
            .ok_or_else(|| invalid(format!("node {node} is a root or not in the graph")))?;
        let p = &self.nodes[parent];
        let info = kind_info(p.kind);
        if !info.is_some_and(|k| k.arity.variable()) {
            return Err(invalid(format!(
                "node {parent} (type {:#04x}) needs all its children",
                p.kind()
            )));
        }
        if p.children.len() <= 1 {
            return Err(invalid(format!("node {parent} would have no children")));
        }
        self.nodes[parent].children.retain(|&c| c != node);
        self.compact();
        self.renumber();
        Ok(())
    }

    /// Moves child `from` of `parent` to position `to`.
    pub fn move_child(&mut self, parent: usize, from: usize, to: usize) -> Result<()> {
        let p = self.node_mut(parent)?;
        if from >= p.children.len() || to >= p.children.len() {
            return Err(invalid("no such child"));
        }
        let c = p.children.remove(from);
        p.children.insert(to, c);
        self.compact();
        self.renumber();
        Ok(())
    }

    /// Renames entry `entry` (the game finds entries by SDBM of the
    /// lower-case name).
    pub fn rename_entry(&mut self, entry: usize, name: &str) -> Result<()> {
        if name.is_empty() || name.contains('\0') {
            return Err(invalid("bad entry name"));
        }
        if self
            .entries
            .iter()
            .enumerate()
            .any(|(i, e)| i != entry && e.name.eq_ignore_ascii_case(name))
        {
            return Err(invalid(format!("the bank already has an entry `{name}`")));
        }
        self.entries
            .get_mut(entry)
            .ok_or_else(|| invalid(format!("no entry {entry}")))?
            .name = name.to_owned();
        Ok(())
    }

    /// Removes entry `entry` and its tree.
    pub fn remove_entry(&mut self, entry: usize) -> Result<()> {
        if entry >= self.entries.len() {
            return Err(invalid(format!("no entry {entry}")));
        }
        self.entries.remove(entry);
        self.compact();
        self.renumber();
        Ok(())
    }

    /// Drops nodes no entry reaches and puts the arena in layout order;
    /// returns the new index of every old node (`usize::MAX`: dropped).
    fn compact(&mut self) -> Vec<usize> {
        let mut order = Vec::new();
        for e in &self.entries {
            order.extend(self.walk(e.root).into_iter().map(|(n, _)| n));
        }
        let mut map = vec![usize::MAX; self.nodes.len()];
        for (new, &old) in order.iter().enumerate() {
            map[old] = new;
        }
        let mut nodes: Vec<GraphNode> = order.iter().map(|&o| self.nodes[o].clone()).collect();
        for n in &mut nodes {
            for c in &mut n.children {
                *c = map[*c];
            }
        }
        for e in &mut self.entries {
            e.root = map[e.root];
        }
        self.nodes = nodes;
        map
    }

    // ---------------------------------------------------------------
    // Checks.

    /// Problems that would make the game misread or misplay the graph.
    pub fn check(&self, bank: &BankImage) -> Vec<Issue> {
        let mut out = Vec::new();
        let mut push = |node: Option<usize>, severity: Severity, text: String| {
            out.push(Issue {
                node,
                severity,
                text,
            })
        };
        let parents = self.parents();
        let mut names = BTreeSet::new();
        for (i, e) in self.entries.iter().enumerate() {
            if !names.insert(crate::hash::sdbm(e.name.to_ascii_lowercase().as_bytes())) {
                push(
                    None,
                    Severity::Error,
                    format!(
                        "entry {i} `{}`: another entry has the same name hash",
                        e.name
                    ),
                );
            }
            if self.nodes.get(e.root).is_none() {
                push(
                    None,
                    Severity::Error,
                    format!("entry `{}` has no root", e.name),
                );
                continue;
            }
            if self.nodes[e.root].kind() != 0x02 {
                push(
                    Some(e.root),
                    Severity::Warning,
                    format!(
                        "entry `{}`: the root is not a sequence (type 2) as in every game entry; its priority is not read",
                        e.name
                    ),
                );
            }
            // Numbers unique within the entry.
            let mut used = BTreeMap::new();
            for (n, _) in self.walk(e.root) {
                let node = &self.nodes[n];
                let mut slots = vec![node.number as u32];
                for &(w, half) in state_slots(node.kind) {
                    if let Some(v) = node.get_half(w, half) {
                        slots.push(v);
                    }
                }
                for s in slots {
                    if let Some(other) = used.insert(s, n)
                        && other != n
                    {
                        push(
                            Some(n),
                            Severity::Error,
                            format!("slot {s} is used by node {other} too"),
                        );
                    }
                    if s >= bank.node_count.max(self.slot_count()) {
                        push(
                            Some(n),
                            Severity::Error,
                            format!("slot {s} beyond the slot count"),
                        );
                    }
                }
            }
        }
        for (i, node) in self.nodes.iter().enumerate() {
            let kind = node.kind();
            if !kind_known_to_game(node.kind) {
                push(
                    Some(i),
                    Severity::Error,
                    format!("type {kind:#04x} has no function in the game"),
                );
                continue;
            }
            if node.kind & 0xC0 != 0 {
                push(
                    Some(i),
                    Severity::Warning,
                    format!("type byte {:#04x} has its top bits set", node.kind),
                );
            }
            let Some(info) = kind_info(kind) else {
                push(
                    Some(i),
                    Severity::Warning,
                    format!("type {kind:#04x} does not occur in the game's banks"),
                );
                continue;
            };
            let n = node.children.len();
            if !info.arity.allows(n) {
                push(
                    Some(i),
                    Severity::Error,
                    format!("{} node with {n} children", info.label),
                );
            }
            let want = info.params.map(<[Field]>::len);
            let have = node.params.as_ref().map(Vec::len);
            match (want, have) {
                (Some(w), Some(h)) if h < w => push(
                    Some(i),
                    Severity::Error,
                    format!("{h} parameter words, the type reads {w}"),
                ),
                (Some(_), None) if kind != 0x02 => {
                    push(Some(i), Severity::Error, "no parameters".to_owned())
                }
                _ => {}
            }
            // Extra data the parent reads.
            if let Some(p) = parents[i] {
                let need = kind_info(self.nodes[p].kind).and_then(|k| k.child_extra);
                if let Some(fields) = need
                    && node.extra.as_ref().is_none_or(|x| x.len() < fields.len())
                {
                    push(
                        Some(i),
                        Severity::Error,
                        format!(
                            "the parent ({}) reads {} extra word(s)",
                            kind_info(self.nodes[p].kind).map_or("?", |k| k.label),
                            fields.len()
                        ),
                    );
                }
            }
            if kind == 0x05
                && let Some(c) = node.param(0)
                && control_name(c).is_none()
            {
                push(
                    Some(i),
                    Severity::Error,
                    format!("control {c} is not in the game"),
                );
            }
            if kind == 0x06
                && let Some(c) = node.get_half(1, Some(0))
                && control_name(c).is_none()
            {
                push(
                    Some(i),
                    Severity::Error,
                    format!("control {c} is not in the game"),
                );
            }
            if kind == 0x21 {
                for w in 0..2 {
                    if let Some(c) = node.param(w)
                        && control_name(c).is_none()
                    {
                        push(
                            Some(i),
                            Severity::Error,
                            format!("control {c} is not in the game"),
                        );
                    }
                }
            }
            if node.is_clip() {
                match node.clip {
                    None => push(Some(i), Severity::Error, "clip node without a clip".into()),
                    Some(c) if c as usize >= bank.clips.len() => push(
                        Some(i),
                        Severity::Error,
                        format!("clip {c} is not in the bank"),
                    ),
                    Some(c) => {
                        let clip = &bank.clips[c as usize];
                        let rate = node.param_f32(2).unwrap_or(TICKS_PER_FRAME);
                        let frames = clip.frames() as f32;
                        let want = if clip.looping() {
                            frames
                        } else {
                            (frames - 1.0).max(0.0)
                        } * rate;
                        if let Some(d) = node.param_f32(1)
                            && (d - want).abs() > 0.5
                        {
                            push(
                                Some(i),
                                Severity::Warning,
                                format!(
                                    "duration {d} ticks, the clip has {} frames ({want} ticks)",
                                    clip.frames()
                                ),
                            );
                        }
                        if rate != TICKS_PER_FRAME {
                            push(
                                Some(i),
                                Severity::Warning,
                                format!("{rate} ticks per frame (2.0 in every game node)"),
                            );
                        }
                    }
                }
            }
            if kind == 0x08
                && node
                    .param_f32(0)
                    .is_some_and(|s| s <= 0.0 || !s.is_finite())
            {
                push(
                    Some(i),
                    Severity::Error,
                    "time scale must be positive".into(),
                );
            }
            if kind == 0x05
                && node
                    .param_f32(2)
                    .is_some_and(|r| r == 0.0 || !r.is_finite())
            {
                push(Some(i), Severity::Error, "blend range must not be 0".into());
            }
        }
        out
    }

    // ---------------------------------------------------------------
    // JSON.

    /// The graph as nested JSON-ready entries.
    pub fn to_json(&self, bank: &BankImage) -> GraphJson {
        // Clips are named like the entries that play them (SDBM of the name).
        let names: BTreeMap<u32, &str> = self
            .entries
            .iter()
            .map(|e| {
                (
                    crate::hash::sdbm(e.name.to_ascii_lowercase().as_bytes()),
                    e.name.as_str(),
                )
            })
            .collect();
        let clip_name = |c: u32| {
            let h = bank.clips.get(c as usize)?.hash();
            Some(
                names
                    .get(&h)
                    .map_or_else(|| format!("{h:08x}"), |n| (*n).to_owned()),
            )
        };
        fn node_json(
            g: &AnimGraph,
            n: usize,
            parent: Option<u8>,
            clip_name: &dyn Fn(u32) -> Option<String>,
        ) -> NodeJson {
            let node = &g.nodes[n];
            let info = kind_info(node.kind);
            let pfields = info.and_then(|k| k.params);
            let xfields = parent.and_then(kind_info).and_then(|k| k.child_extra);
            NodeJson {
                kind: node.kind,
                what: info.map(|k| k.label.to_owned()),
                number: Some(node.number),
                params: node.params.as_ref().map(|p| words_json(p, pfields)),
                extra: node.extra.as_ref().map(|x| words_json(x, xfields)),
                clip: node.clip,
                clip_name: node.clip.and_then(clip_name),
                children: node
                    .children
                    .iter()
                    .map(|&c| node_json(g, c, Some(node.kind), clip_name))
                    .collect(),
            }
        }
        GraphJson {
            entries: self
                .entries
                .iter()
                .map(|e| EntryJson {
                    name: e.name.clone(),
                    root: node_json(self, e.root, None, &clip_name),
                })
                .collect(),
        }
    }

    /// A graph from its JSON form. Numbers given in the JSON are kept;
    /// missing ones are given by [`AnimGraph::renumber`].
    pub fn from_json(json: &GraphJson) -> Result<Self> {
        let mut g = AnimGraph::default();
        let mut missing = false;
        fn add(
            g: &mut AnimGraph,
            n: &NodeJson,
            parent: Option<u8>,
            missing: &mut bool,
            depth: usize,
        ) -> Result<usize> {
            if depth > 256 {
                return Err(invalid("graph too deep"));
            }
            let info = kind_info(n.kind);
            let pfields = info.and_then(|k| k.params);
            let xfields = parent.and_then(kind_info).and_then(|k| k.child_extra);
            let params = n
                .params
                .as_ref()
                .map(|p| words_from_json(p, pfields))
                .transpose()?;
            let extra = n
                .extra
                .as_ref()
                .map(|x| words_from_json(x, xfields))
                .transpose()?;
            if n.number.is_none() {
                *missing = true;
            }
            let index = g.nodes.len();
            g.nodes.push(GraphNode {
                kind: n.kind,
                number: n.number.unwrap_or(0),
                params,
                extra,
                children: Vec::new(),
                clip: n.clip,
            });
            let mut children = Vec::with_capacity(n.children.len());
            for c in &n.children {
                children.push(add(g, c, Some(n.kind), missing, depth + 1)?);
            }
            g.nodes[index].children = children;
            Ok(index)
        }
        for e in &json.entries {
            let root = add(&mut g, &e.root, None, &mut missing, 0)?;
            g.entries.push(GraphEntry {
                name: e.name.clone(),
                root,
            });
        }
        if missing {
            g.renumber();
        }
        Ok(g)
    }
}

/// How bad an [`Issue`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Warning,
    Error,
}

/// A problem found by [`AnimGraph::check`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Issue {
    pub node: Option<usize>,
    pub severity: Severity,
    pub text: String,
}

/// A parameter word in JSON: a number, or a pair of u16.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WordJson {
    Int(u32),
    Float(f32),
    Pair([u16; 2]),
    /// Raw bits as `"0x…"`: floats JSON cannot hold (NaN, infinities).
    Bits(String),
}

fn words_json(words: &[u32], fields: Option<&'static [Field]>) -> Vec<WordJson> {
    words
        .iter()
        .enumerate()
        .map(|(i, &w)| match field(fields, i).ty {
            FieldType::U32 => WordJson::Int(w),
            FieldType::F32 if f32::from_bits(w).is_finite() => WordJson::Float(f32::from_bits(w)),
            FieldType::F32 => WordJson::Bits(format!("{w:#010x}")),
            FieldType::Pair => WordJson::Pair([w as u16, (w >> 16) as u16]),
        })
        .collect()
}

fn words_from_json(words: &[WordJson], fields: Option<&'static [Field]>) -> Result<Vec<u32>> {
    words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let ty = field(fields, i).ty;
            if let WordJson::Bits(text) = w {
                return u32::from_str_radix(text.trim_start_matches("0x"), 16)
                    .map_err(|_| invalid(format!("word {i}: `{text}` is not hexadecimal bits")));
            }
            Ok(match (ty, w.clone()) {
                (FieldType::F32, WordJson::Float(v)) => v.to_bits(),
                (FieldType::F32, WordJson::Int(v)) => (v as f32).to_bits(),
                (_, WordJson::Pair([lo, hi])) => lo as u32 | (hi as u32) << 16,
                (_, WordJson::Int(v)) => v,
                (_, WordJson::Float(v)) if v.fract() == 0.0 && v >= 0.0 && v <= u32::MAX as f32 => {
                    v as u32
                }
                (_, WordJson::Float(v)) => {
                    return Err(invalid(format!("word {i}: {v} is not a whole number")));
                }
                (_, WordJson::Bits(_)) => unreachable!("handled above"),
            })
        })
        .collect()
}

/// A node in JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeJson {
    /// Type byte.
    pub kind: u8,
    /// What the type does (ignored when read).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub what: Option<String>,
    /// Slot number; given again when any node lacks one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Vec<WordJson>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<Vec<WordJson>>,
    /// Clip index (clip nodes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<u32>,
    /// Clip name or hash (ignored when read).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<NodeJson>,
}

/// A named entry in JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntryJson {
    pub name: String,
    pub root: NodeJson,
}

/// The graph in JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphJson {
    pub entries: Vec<EntryJson>,
}

/// Counts of a bank's graph, for listings.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct GraphStats {
    pub entries: usize,
    pub nodes: usize,
    /// Nodes per type byte.
    pub kinds: BTreeMap<u8, usize>,
    pub slots: u32,
}

impl AnimGraph {
    pub fn stats(&self) -> GraphStats {
        let mut kinds = BTreeMap::new();
        for n in &self.nodes {
            *kinds.entry(n.kind).or_insert(0) += 1;
        }
        GraphStats {
            entries: self.entries.len(),
            nodes: self.nodes.len(),
            kinds,
            slots: self.slot_count(),
        }
    }
}

/// A bank with two clips and an entry `idle`: sequence → random of two
/// clip nodes.
#[cfg(any(test, feature = "testing"))]
pub(crate) fn sample_bank() -> BankImage {
    use crate::resource::anim_image::ClipImage;
    let clip = |hash| ClipImage {
        record: ClipImage::new_record(hash, 3, 0x78E0_3D81, 0x8000_0004),
        streams: vec![0x2C, 0x00, 0, 0, 0],
        events: Vec::new(),
    };
    let mut bank = BankImage {
        hash: 1,
        skeleton: 0x50F9_2C54,
        words: [0, 0, 0, 1],
        node_count: 0,
        entries: Vec::new(),
        event_objects: Vec::new(),
        graph: Block::default(),
        clips: vec![clip(0x10), clip(0x20)],
        tail: Block::default(),
    };
    let leaf = |c: u32, w: f32| GraphNode {
        kind: NODE_CLIP,
        number: 0,
        params: Some(vec![0, 4f32.to_bits(), 2f32.to_bits()]),
        extra: Some(vec![w.to_bits()]),
        children: vec![],
        clip: Some(c),
    };
    let mut g = AnimGraph {
        entries: vec![GraphEntry {
            name: "idle".into(),
            root: 0,
        }],
        nodes: vec![
            GraphNode {
                kind: 2,
                number: 0,
                params: Some(vec![0, 0x1203_2045]),
                extra: None,
                children: vec![1],
                clip: None,
            },
            GraphNode {
                kind: 3,
                number: 0,
                params: Some(vec![0, 1, 20f32.to_bits()]),
                extra: None,
                children: vec![2, 3],
                clip: None,
            },
            leaf(0, 1.0),
            leaf(1, 2.0),
        ],
    };
    assert_eq!(g.renumber(), 7);
    g.store(&mut bank).unwrap();
    bank
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bank() -> BankImage {
        super::sample_bank()
    }

    #[test]
    fn numbers_slots_depth_first() {
        let b = bank();
        let g = AnimGraph::parse(&b).unwrap();
        let numbers: Vec<u16> = g.nodes.iter().map(|n| n.number).collect();
        // Root 0, random 1 with slots 2, 3 and 4, then its children.
        assert_eq!(numbers, [0, 1, 5, 6]);
        assert_eq!(
            g.nodes[1].params.as_ref().unwrap()[..2],
            [0x0003_0002, 0x0004_0001]
        );
        assert_eq!(b.node_count, 7);
    }

    #[test]
    fn graph_round_trips_through_the_image_and_json() {
        let b = bank();
        let data = b.encode().unwrap();
        let back = BankImage::parse(&data).unwrap();
        assert_eq!(back, b);
        let g = AnimGraph::parse(&back).unwrap();
        assert_eq!(g.to_block().unwrap(), back.graph);
        let entries = back.entry_list();
        assert_eq!(entries[0].name, "idle");
        assert_eq!(entries[0].clips, [0, 1]);
        let json = serde_json::to_string(&g.to_json(&back)).unwrap();
        let again = AnimGraph::from_json(&serde_json::from_str(&json).unwrap()).unwrap();
        assert_eq!(again, g);
        assert!(g.check(&back).is_empty(), "{:?}", g.check(&back));
    }

    #[test]
    fn edits_keep_the_graph_consistent() {
        let mut b = bank();
        let mut g = AnimGraph::parse(&b).unwrap();
        // Add a third clip node to the random node; it gets a weight.
        let n = g.add_clip_node(1, 1, 5, true).unwrap();
        assert_eq!(g.nodes[n].extra, Some(vec![1f32.to_bits()]));
        assert_eq!(g.nodes[n].param_f32(1), Some(10.0));
        g.store(&mut b).unwrap();
        let back = BankImage::parse(&b.encode().unwrap()).unwrap();
        assert_eq!(back.entry_list()[0].clips, [0, 1, 1]);
        let issues = g.check(&back);
        // The new node's duration disagrees with clip 1 (3 frames).
        assert!(issues.iter().all(|i| i.severity == Severity::Warning));
        // Remove a clip node; the root keeps its only child.
        g.remove_node(2).unwrap();
        assert_eq!(g.nodes.len(), 4);
        assert!(g.remove_node(1).is_err());
        assert!(g.remove_node(0).is_err());
        g.set_param(1, 2, 0).unwrap();
        g.move_child(1, 0, 1).unwrap();
        g.rename_entry(0, "walk").unwrap();
        g.store(&mut b).unwrap();
        let back = BankImage::parse(&b.encode().unwrap()).unwrap();
        let again = AnimGraph::parse(&back).unwrap();
        assert_eq!(again, g);
        assert_eq!(back.entry_list()[0].name, "walk");
        // A select node without the children's ranges is an error.
        g.nodes[1].kind = 6;
        assert!(g.check(&back).iter().any(|i| i.severity == Severity::Error));
        g.remove_entry(0).unwrap();
        assert!(g.nodes.is_empty());
    }

    #[test]
    fn json_words_follow_the_type() {
        let w = words_json(&[3, 0x0004_0001, 1f32.to_bits()], Some(&RANDOM));
        assert_eq!(
            w,
            [
                WordJson::Pair([3, 0]),
                WordJson::Pair([1, 4]),
                WordJson::Float(1.0)
            ]
        );
        let text = serde_json::to_string(&w).unwrap();
        assert_eq!(text, "[[3,0],[1,4],1.0]");
        let back: Vec<WordJson> = serde_json::from_str("[[3,0],[1,4],2]").unwrap();
        assert_eq!(
            words_from_json(&back, Some(&RANDOM)).unwrap(),
            [3, 0x0004_0001, 2f32.to_bits()]
        );
        assert!(words_from_json(&[WordJson::Float(0.5)], Some(&BLEND)).is_err());
        // NaN keeps its bits.
        let nan = words_json(&[0x7FC0_0001], Some(&SCALE_TIME));
        assert_eq!(nan, [WordJson::Bits("0x7fc00001".into())]);
        let text = serde_json::to_string(&nan).unwrap();
        let back: Vec<WordJson> = serde_json::from_str(&text).unwrap();
        assert_eq!(
            words_from_json(&back, Some(&SCALE_TIME)).unwrap(),
            [0x7FC0_0001]
        );
    }
}
