//! Data of a graph that follows from its nodes and links: link lengths,
//! visibility bits, the R-tree, precomputed paths, and a summary.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use serde::{Deserialize, Serialize};

use super::{
    Graph, Node, PathTable, RShape, RTree, Reader, Visibility, flag_names, graph_flags,
    path_table_len, visibility_len,
};
use crate::Result;

const NONE: u16 = 0xFFFF;
/// Most nodes in an R-tree leaf in the game files.
pub const LEAF_SIZE: usize = 4;

/// A node of the R-tree, 36 bytes: box, then parent, first child, next
/// sibling, "thread", first index and index count (leaves only).
///
/// In the game files every inner node is followed by its first child
/// (preorder), the box of a leaf is the box of its node positions and the
/// box of an inner node the union of its children; `thread` is 0xFFFF for a
/// node with a next sibling and otherwise the next sibling of the nearest
/// ancestor that has one. The game walks child and sibling links only
/// (`godfather2.exe` `0x00601999`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RTreeNode {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub parent: u16,
    pub child: u16,
    pub sibling: u16,
    pub thread: u16,
    pub first: u16,
    pub count: u16,
}

impl RTreeNode {
    pub(super) fn read(r: &Reader, at: usize) -> Result<Self> {
        let f = |k: usize| r.f32(at + 4 * k);
        Ok(Self {
            min: [f(0)?, f(1)?, f(2)?],
            max: [f(3)?, f(4)?, f(5)?],
            parent: r.u16(at + 0x18)?,
            child: r.u16(at + 0x1A)?,
            sibling: r.u16(at + 0x1C)?,
            thread: r.u16(at + 0x1E)?,
            first: r.u16(at + 0x20)?,
            count: r.u16(at + 0x22)?,
        })
    }

    pub(super) fn write(&self, out: &mut [u8]) {
        for (k, v) in self.min.iter().chain(&self.max).enumerate() {
            out[4 * k..4 * k + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (k, v) in [
            self.parent,
            self.child,
            self.sibling,
            self.thread,
            self.first,
            self.count,
        ]
        .into_iter()
        .enumerate()
        {
            out[0x18 + 2 * k..0x1A + 2 * k].copy_from_slice(&v.to_le_bytes());
        }
    }
}

/// Distance between two positions, as a link length.
pub fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// Bit of the pair `a < b` among `n` nodes.
pub fn visibility_bit(n: usize, a: usize, b: usize) -> usize {
    let (a, b) = if a < b { (a, b) } else { (b, a) };
    a * (2 * n - a - 1) / 2 + (b - a - 1)
}

/// Plain bytes of run-length coded visibility (a zero byte is followed by
/// a count of zero bytes, `godfather2.exe` `0x005FC210`).
fn unpack_rle(bytes: &[u8], len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut k = 0;
    while k < bytes.len() && out.len() < len {
        if bytes[k] == 0 {
            let run = bytes.get(k + 1).copied().unwrap_or(0) as usize;
            out.extend(std::iter::repeat_n(0, run.min(len - out.len())));
            k += 2;
        } else {
            out.push(bytes[k]);
            k += 1;
        }
    }
    out.resize(len, 0);
    out
}

/// Visibility of a parsed graph: pairs when the bits are plain and the
/// unused bits set, else raw.
pub(super) fn visibility_from_bits(n: usize, rle: u32, bytes: &[u8]) -> Visibility {
    let raw = || Visibility::Raw {
        rle,
        bytes: bytes.to_vec(),
    };
    if rle != 0 || bytes.len() != visibility_len(n) {
        return raw();
    }
    let bits = n * n.saturating_sub(1) / 2;
    if (bits..bytes.len() * 8).any(|b| bytes[b / 8] & (1 << (b % 8)) == 0) {
        return raw();
    }
    let mut hidden = Vec::new();
    if bytes.iter().any(|&b| b != 0xFF) {
        for a in 0..n {
            for b in a + 1..n {
                let bit = visibility_bit(n, a, b);
                if bytes[bit / 8] & (1 << (bit % 8)) == 0 {
                    hidden.push([a as u16, b as u16]);
                }
            }
        }
    }
    Visibility::Pairs { hidden }
}

/// The visibility bytes to write.
pub(super) fn visibility_bytes(n: usize, v: &Visibility) -> Vec<u8> {
    match v {
        Visibility::Raw { bytes, .. } => bytes.clone(),
        Visibility::Pairs { hidden } => {
            let mut out = vec![0xFF; visibility_len(n)];
            for &[a, b] in hidden {
                let bit = visibility_bit(n, a as usize, b as usize);
                out[bit / 8] &= !(1 << (bit % 8));
            }
            out
        }
    }
}

/// Hidden pairs of any visibility, for editing.
pub(super) fn hidden_pairs(n: usize, v: &Visibility) -> Vec<[u16; 2]> {
    match v {
        Visibility::Pairs { hidden } => hidden.clone(),
        Visibility::Raw { rle, bytes } => {
            let plain = if *rle != 0 {
                unpack_rle(bytes, visibility_len(n))
            } else {
                let mut b = bytes.clone();
                b.resize(visibility_len(n), 0xFF);
                b
            };
            let mut hidden = Vec::new();
            for a in 0..n {
                for b in a + 1..n {
                    let bit = visibility_bit(n, a, b);
                    if plain[bit / 8] & (1 << (bit % 8)) == 0 {
                        hidden.push([a as u16, b as u16]);
                    }
                }
            }
            hidden
        }
    }
}

/// Every node index of a shape at most once; marks them in `seen`.
pub(super) fn shape_covers(shape: &RShape, seen: &mut [bool]) -> bool {
    match shape {
        RShape::Leaf(ids) => ids.iter().all(|&i| match seen.get_mut(i as usize) {
            Some(s) if !*s => {
                *s = true;
                true
            }
            _ => false,
        }),
        RShape::Inner(kids) => !kids.is_empty() && kids.iter().all(|k| shape_covers(k, seen)),
    }
}

/// R-tree records of a shape over the node positions.
fn shape_records(nodes: &[Node], shape: &RShape) -> (Vec<RTreeNode>, Vec<u16>) {
    fn walk(
        nodes: &[Node],
        shape: &RShape,
        parent: u16,
        escape: u16,
        out: &mut Vec<RTreeNode>,
        indices: &mut Vec<u16>,
    ) -> usize {
        let me = out.len();
        out.push(RTreeNode {
            min: [f32::INFINITY; 3],
            max: [f32::NEG_INFINITY; 3],
            parent,
            child: NONE,
            sibling: NONE,
            thread: escape,
            first: 0,
            count: 0,
        });
        let (mut min, mut max) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        match shape {
            RShape::Leaf(ids) => {
                out[me].first = indices.len() as u16;
                out[me].count = ids.len() as u16;
                for &i in ids {
                    indices.push(i);
                    let p = nodes[i as usize].position;
                    for a in 0..3 {
                        min[a] = min[a].min(p[a]);
                        max[a] = max[a].max(p[a]);
                    }
                }
            }
            RShape::Inner(kids) => {
                out[me].child = (me + 1) as u16;
                let mut prev: Option<usize> = None;
                for (k, kid) in kids.iter().enumerate() {
                    // A node with a sibling has no thread; the last child
                    // escapes to where its parent escapes.
                    let at = out.len();
                    if let Some(p) = prev {
                        out[p].sibling = at as u16;
                        out[p].thread = NONE;
                    }
                    let kid_escape = if k + 1 == kids.len() { escape } else { NONE };
                    walk(nodes, kid, me as u16, kid_escape, out, indices);
                    for a in 0..3 {
                        min[a] = min[a].min(out[at].min[a]);
                        max[a] = max[a].max(out[at].max[a]);
                    }
                    prev = Some(at);
                }
            }
        }
        out[me].min = min;
        out[me].max = max;
        me
    }
    let mut out = Vec::new();
    let mut indices = Vec::new();
    walk(nodes, shape, NONE, NONE, &mut out, &mut indices);
    (out, indices)
}

/// Fixes the threads of [`shape_records`]: a node with a next sibling has
/// none; otherwise the escape of its parent chain.
fn fix_threads(out: &mut [RTreeNode]) {
    for k in 0..out.len() {
        if out[k].sibling != NONE {
            out[k].thread = NONE;
            continue;
        }
        let mut at = out[k].parent;
        let mut thread = NONE;
        let mut steps = 0;
        while at != NONE && steps < out.len() {
            if out[at as usize].sibling != NONE {
                thread = out[at as usize].sibling;
                break;
            }
            at = out[at as usize].parent;
            steps += 1;
        }
        out[k].thread = thread;
    }
}

/// The records to write: `None` for an empty tree.
pub(super) fn rtree_records(nodes: &[Node], tree: &RTree) -> Option<(Vec<RTreeNode>, Vec<u16>)> {
    match tree {
        RTree::Empty => None,
        RTree::Raw { nodes, indices } => Some((nodes.clone(), indices.clone())),
        RTree::Shape(shape) => {
            let (mut out, indices) = shape_records(nodes, shape);
            fix_threads(&mut out);
            Some((out, indices))
        }
    }
}

/// The shape of a parsed tree when its records are exactly what the shape
/// gives back, else the raw records.
pub(super) fn rtree_from_raw(nodes: &[Node], raw: Vec<RTreeNode>, indices: Vec<u16>) -> RTree {
    fn shape_of(
        raw: &[RTreeNode],
        indices: &[u16],
        k: usize,
        depth: usize,
        visited: &mut usize,
    ) -> Option<RShape> {
        *visited += 1;
        if depth > 64 || *visited > raw.len() {
            return None;
        }
        let r = raw.get(k)?;
        if r.child == NONE {
            let first = r.first as usize;
            let ids = indices.get(first..first + r.count as usize)?;
            return Some(RShape::Leaf(ids.to_vec()));
        }
        let mut kids = Vec::new();
        let mut at = r.child;
        while at != NONE {
            kids.push(shape_of(raw, indices, at as usize, depth + 1, visited)?);
            at = raw.get(at as usize)?.sibling;
            if kids.len() > raw.len() {
                return None;
            }
        }
        Some(RShape::Inner(kids))
    }
    let mut visited = 0;
    let mut seen = vec![false; nodes.len()];
    if let Some(shape) = shape_of(&raw, &indices, 0, 0, &mut visited)
        && shape_covers(&shape, &mut seen)
        && seen.iter().all(|s| *s)
    {
        let tree = RTree::Shape(shape);
        if let Some((records, idx)) = rtree_records(nodes, &tree)
            && idx == indices
            && records.len() == raw.len()
            && records.iter().zip(&raw).all(|(a, b)| same_record(a, b))
        {
            return tree;
        }
    }
    RTree::Raw {
        nodes: raw,
        indices,
    }
}

fn same_record(a: &RTreeNode, b: &RTreeNode) -> bool {
    let bits = |v: &[f32; 3]| v.map(f32::to_bits);
    bits(&a.min) == bits(&b.min)
        && bits(&a.max) == bits(&b.max)
        && (a.parent, a.child, a.sibling, a.thread, a.first, a.count)
            == (b.parent, b.child, b.sibling, b.thread, b.first, b.count)
}

/// A new R-tree: halves split along the longest side of the box of the
/// positions, leaves of up to [`LEAF_SIZE`] nodes.
pub fn build_rtree(nodes: &[Node]) -> RTree {
    fn split(nodes: &[Node], mut ids: Vec<u16>) -> RShape {
        if ids.len() <= LEAF_SIZE {
            return RShape::Leaf(ids);
        }
        let (mut min, mut max) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for &i in &ids {
            let p = nodes[i as usize].position;
            for a in 0..3 {
                min[a] = min[a].min(p[a]);
                max[a] = max[a].max(p[a]);
            }
        }
        let axis = (0..3)
            .max_by(|&a, &b| {
                (max[a] - min[a])
                    .partial_cmp(&(max[b] - min[b]))
                    .unwrap_or(Ordering::Equal)
            })
            .unwrap_or(0);
        ids.sort_by(|&x, &y| {
            let (px, py) = (
                nodes[x as usize].position[axis],
                nodes[y as usize].position[axis],
            );
            px.partial_cmp(&py)
                .unwrap_or(Ordering::Equal)
                .then(x.cmp(&y))
        });
        let right = ids.split_off(ids.len().div_ceil(2));
        RShape::Inner(vec![split(nodes, ids), split(nodes, right)])
    }
    if nodes.is_empty() {
        return RTree::Empty;
    }
    RTree::Shape(split(nodes, (0..nodes.len() as u16).collect()))
}

#[derive(PartialEq)]
struct Item(f64, usize);

impl Eq for Item {}

impl Ord for Item {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .0
            .partial_cmp(&self.0)
            .unwrap_or(Ordering::Equal)
            .then(other.1.cmp(&self.1))
    }
}

impl PartialOrd for Item {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Shortest distances from `from` over the links (link lengths as costs).
fn distances(graph: &Graph, from: usize) -> Vec<f64> {
    let n = graph.nodes.len();
    let mut dist = vec![f64::INFINITY; n];
    dist[from] = 0.0;
    let mut heap = BinaryHeap::new();
    heap.push(Item(0.0, from));
    while let Some(Item(d, u)) = heap.pop() {
        if d > dist[u] {
            continue;
        }
        for l in &graph.nodes[u].links {
            let v = l.to as usize;
            let nd = d + l.length.max(0.0) as f64;
            if v < n && nd < dist[v] {
                dist[v] = nd;
                heap.push(Item(nd, v));
            }
        }
    }
    dist
}

/// A new path table: for every pair, the first link of a shortest path.
/// `None` when a node has more than 15 links (a nibble cannot name them).
pub fn compute_paths(graph: &Graph) -> Option<PathTable> {
    let n = graph.nodes.len();
    if graph.nodes.iter().any(|node| node.links.len() > 15) {
        return None;
    }
    let mut table = vec![0u8; path_table_len(n)];
    // Distances to every target, from the reversed graph.
    let mut reverse = vec![Vec::new(); n];
    for (from, l) in graph.links() {
        reverse[l.to as usize].push((from, l.length.max(0.0) as f64));
    }
    for to in 0..n {
        let mut dist = vec![f64::INFINITY; n];
        dist[to] = 0.0;
        let mut heap = BinaryHeap::new();
        heap.push(Item(0.0, to));
        while let Some(Item(d, u)) = heap.pop() {
            if d > dist[u] {
                continue;
            }
            for &(v, len) in &reverse[u] {
                let nd = d + len;
                if nd < dist[v] {
                    dist[v] = nd;
                    heap.push(Item(nd, v));
                }
            }
        }
        for from in 0..n {
            if from == to || !dist[from].is_finite() {
                continue;
            }
            let best = graph.nodes[from]
                .links
                .iter()
                .enumerate()
                .map(|(k, l)| (k, l.length.max(0.0) as f64 + dist[l.to as usize]))
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal));
            if let Some((k, _)) = best {
                let i = from * n + to;
                table[i / 2] |= ((k + 1) as u8) << (4 * (i % 2));
            }
        }
    }
    Some(PathTable(table))
}

/// Whether a path table still leads every pair along a shortest path (and
/// marks exactly the unreachable pairs with 0), within float rounding.
pub fn paths_valid(graph: &Graph, table: &PathTable) -> bool {
    let n = graph.nodes.len();
    if table.0.len() != path_table_len(n) {
        return false;
    }
    if n % 2 == 1 && n > 0 && table.0[table.0.len() - 1] >> 4 != 0 {
        return false;
    }
    let all: Vec<Vec<f64>> = (0..n).map(|from| distances(graph, from)).collect();
    for (from, dist) in all.iter().enumerate() {
        for to in 0..n {
            let v = table.get(n, from, to) as usize;
            if from == to || !dist[to].is_finite() {
                if v != 0 {
                    return false;
                }
                continue;
            }
            let Some(l) = v
                .checked_sub(1)
                .and_then(|k| graph.nodes[from].links.get(k))
            else {
                return false;
            };
            // The first link plus the rest of the way must be a shortest path.
            let total = l.length.max(0.0) as f64 + all[l.to as usize][to];
            if total > dist[to] * (1.0 + 1e-5) + 1e-4 {
                return false;
            }
        }
    }
    true
}

/// A summary of a graph.
#[derive(Debug, Clone, Serialize)]
pub struct GraphInfo {
    pub version: u32,
    pub guid: String,
    pub flags: String,
    pub identity: String,
    pub nodes: usize,
    pub links: usize,
    pub one_way_links: usize,
    pub neighbour_links: usize,
    pub neighbours: usize,
    pub property_sets: usize,
    pub property_instances: usize,
    pub hidden_pairs: Option<usize>,
    pub precomputed_paths: bool,
    pub rtree_nodes: usize,
    pub rtree_form: &'static str,
    pub min: Option<[f32; 3]>,
    pub max: Option<[f32; 3]>,
    pub stale_lengths: usize,
}

impl Graph {
    /// A summary for `graph-info`.
    pub fn info(&self) -> GraphInfo {
        let (mut min, mut max) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for n in &self.nodes {
            for a in 0..3 {
                min[a] = min[a].min(n.position[a]);
                max[a] = max[a].max(n.position[a]);
            }
        }
        let has = |from: usize, to: u16| {
            self.nodes[to as usize]
                .links
                .iter()
                .any(|l| l.to as usize == from)
        };
        GraphInfo {
            version: self.version,
            guid: self.guid.to_string(),
            flags: flag_names(self.flags, &graph_flags::NAMES),
            identity: self.identity.to_string(),
            nodes: self.nodes.len(),
            links: self.link_count(),
            one_way_links: self.links().filter(|(f, l)| !has(*f, l.to)).count(),
            neighbour_links: self.neighbour_link_count(),
            neighbours: self.neighbours.len() + self.external_neighbours.len(),
            property_sets: self.property_sets.len(),
            property_instances: self.property_instances.len(),
            hidden_pairs: match &self.visibility {
                Visibility::Pairs { hidden } => Some(hidden.len()),
                Visibility::Raw { .. } => None,
            },
            precomputed_paths: self.precomputed_paths.is_some(),
            rtree_nodes: rtree_records(&self.nodes, &self.rtree).map_or(0, |(r, _)| r.len()),
            rtree_form: match self.rtree {
                RTree::Empty => "empty",
                RTree::Shape(_) => "shape",
                RTree::Raw { .. } => "raw",
            },
            min: (!self.nodes.is_empty()).then_some(min),
            max: (!self.nodes.is_empty()).then_some(max),
            stale_lengths: self.stale_lengths().len(),
        }
    }

    /// Links whose length differs from the distance between their nodes
    /// by more than float rounding: `(from, index in node)`.
    pub fn stale_lengths(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for (k, node) in self.nodes.iter().enumerate() {
            for (j, l) in node.links.iter().enumerate() {
                let Some(to) = self.nodes.get(l.to as usize) else {
                    continue;
                };
                let d = distance(node.position, to.position);
                if (l.length - d).abs() > 1e-3 * d.max(1.0) {
                    out.push((k, j));
                }
            }
        }
        out
    }
}
