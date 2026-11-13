//! Layered (Sugiyama-style) layout for graphs that come without positions.
//!
//! 1. Weakly connected components are laid out separately and stacked,
//!    largest first; nodes without wires go into a grid below.
//! 2. Cycles are broken by reversing the edges that close them in a
//!    depth-first search.
//! 3. Layers: the longest path from a source (a node without inputs).
//! 4. Order inside layers: barycentre sweeps down and up, a few rounds.
//! 5. Coordinates: layers left to right, nodes of a layer top to bottom.

use std::collections::{BTreeMap, HashMap, HashSet};

use egui::{Pos2, Vec2};

use super::{Graph, NodeId};

/// Spacing of [`layered`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutOptions {
    /// Gap between layers.
    pub layer_gap: f32,
    /// Gap between nodes of a layer.
    pub node_gap: f32,
    /// Gap between components.
    pub component_gap: f32,
    /// Barycentre rounds.
    pub sweeps: usize,
    /// Nodes per row of the grid of unconnected nodes.
    pub grid_columns: usize,
    /// A layer with more nodes is split into several columns.
    pub max_column: usize,
    /// Least width of a shelf of components.
    pub shelf_width: f32,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            layer_gap: 90.0,
            node_gap: 24.0,
            component_gap: 70.0,
            sweeps: 6,
            grid_columns: 8,
            max_column: 12,
            shelf_width: 2400.0,
        }
    }
}

/// Positions (top-left corners) for every node of `graph`. Only the nodes in
/// `only` are placed when it is given; wires to other nodes are ignored.
pub fn layered(
    graph: &Graph,
    only: Option<&HashSet<NodeId>>,
    options: &LayoutOptions,
) -> HashMap<NodeId, Pos2> {
    let nodes: Vec<NodeId> = graph
        .nodes
        .iter()
        .map(|n| n.id)
        .filter(|id| only.is_none_or(|o| o.contains(id)))
        .collect();
    let index: HashMap<NodeId, usize> = nodes.iter().enumerate().map(|(i, &n)| (n, i)).collect();
    let size: Vec<Vec2> = nodes
        .iter()
        .map(|&id| graph.node(id).expect("listed").size())
        .collect();
    let mut edges: Vec<(usize, usize)> = graph
        .wires
        .iter()
        .filter_map(|w| Some((*index.get(&w.from.0)?, *index.get(&w.to.0)?)))
        .filter(|(a, b)| a != b)
        .collect();
    edges.sort_unstable();
    edges.dedup();
    let n = nodes.len();
    // Components.
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while p[r] != r {
            r = p[r];
        }
        let mut y = x;
        while p[y] != r {
            let next = p[y];
            p[y] = r;
            y = next;
        }
        r
    }
    for &(a, b) in &edges {
        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
        if ra != rb {
            parent[ra] = rb;
        }
    }
    let mut components: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let has_edge: HashSet<usize> = edges.iter().flat_map(|&(a, b)| [a, b]).collect();
    let mut loners = Vec::new();
    for v in 0..n {
        if has_edge.contains(&v) {
            let r = find(&mut parent, v);
            components.entry(r).or_default().push(v);
        } else {
            loners.push(v);
        }
    }
    let mut components: Vec<Vec<usize>> = components.into_values().collect();
    components.sort_by_key(|c| std::cmp::Reverse(c.len()));

    // Components side by side in shelves no wider than the widest one
    // (at least `shelf_width`).
    let mut out = HashMap::new();
    let mut placed: Vec<(usize, HashMap<usize, Pos2>, Vec2)> = Vec::new();
    for comp in &components {
        let set: HashSet<usize> = comp.iter().copied().collect();
        let local: Vec<(usize, usize)> = edges
            .iter()
            .copied()
            .filter(|(a, _)| set.contains(a))
            .collect();
        let mut pos = HashMap::new();
        place_component(comp, &local, &size, options, 0.0, &mut |v, p| {
            pos.insert(v, p);
        });
        let extent = pos
            .iter()
            .map(|(&v, p)| p.to_vec2() + size[v])
            .fold(Vec2::ZERO, |a, b| a.max(b));
        placed.push((comp.len(), pos, extent));
    }
    let shelf = placed
        .iter()
        .map(|p| p.2.x)
        .fold(options.shelf_width, f32::max);
    let (mut x, mut y0, mut row_height) = (0.0f32, 0.0f32, 0.0f32);
    for (_, pos, extent) in &placed {
        if x > 0.0 && x + extent.x > shelf {
            x = 0.0;
            y0 += row_height + options.component_gap;
            row_height = 0.0;
        }
        for (&v, p) in pos {
            out.insert(nodes[v], *p + Vec2::new(x, y0));
        }
        x += extent.x + options.component_gap;
        row_height = row_height.max(extent.y);
    }
    if !placed.is_empty() {
        y0 += row_height + options.component_gap;
    }
    // Grid of nodes without wires.
    let mut x = 0.0f32;
    let mut row_height = 0.0f32;
    for (k, &v) in loners.iter().enumerate() {
        if k > 0 && k % options.grid_columns.max(1) == 0 {
            x = 0.0;
            y0 += row_height + options.node_gap;
            row_height = 0.0;
        }
        out.insert(nodes[v], Pos2::new(x, y0));
        x += size[v].x + options.node_gap;
        row_height = row_height.max(size[v].y);
    }
    out
}

/// Lays out one component starting at `y0`; returns its height.
fn place_component(
    comp: &[usize],
    edges: &[(usize, usize)],
    size: &[Vec2],
    options: &LayoutOptions,
    y0: f32,
    place: &mut dyn FnMut(usize, Pos2),
) -> f32 {
    let mut out_edges: HashMap<usize, Vec<usize>> = HashMap::new();
    for &(a, b) in edges {
        out_edges.entry(a).or_default().push(b);
    }
    // Break cycles: edges to a node on the DFS stack are reversed.
    let mut state: HashMap<usize, u8> = HashMap::new(); // 1 on stack, 2 done
    let mut acyclic: Vec<(usize, usize)> = Vec::new();
    for &start in comp {
        if state.contains_key(&start) {
            continue;
        }
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        state.insert(start, 1);
        while let Some(&mut (v, ref mut k)) = stack.last_mut() {
            let next = out_edges.get(&v).and_then(|e| e.get(*k)).copied();
            *k += 1;
            match next {
                Some(w) => match state.get(&w) {
                    Some(1) => acyclic.push((w, v)),
                    Some(_) => acyclic.push((v, w)),
                    None => {
                        acyclic.push((v, w));
                        state.insert(w, 1);
                        stack.push((w, 0));
                    }
                },
                None => {
                    state.insert(v, 2);
                    stack.pop();
                }
            }
        }
    }
    acyclic.sort_unstable();
    acyclic.dedup();
    // Longest-path layers in topological order (Kahn).
    let mut indeg: HashMap<usize, usize> = comp.iter().map(|&v| (v, 0)).collect();
    let mut succ: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut pred: HashMap<usize, Vec<usize>> = HashMap::new();
    for &(a, b) in &acyclic {
        *indeg.get_mut(&b).expect("in component") += 1;
        succ.entry(a).or_default().push(b);
        pred.entry(b).or_default().push(a);
    }
    let mut layer: HashMap<usize, usize> = HashMap::new();
    let mut queue: Vec<usize> = comp.iter().copied().filter(|v| indeg[v] == 0).collect();
    queue.sort_unstable();
    let mut head = 0;
    while head < queue.len() {
        let v = queue[head];
        head += 1;
        let lv = *layer.entry(v).or_insert(0);
        for &w in succ.get(&v).into_iter().flatten() {
            let lw = layer.entry(w).or_insert(0);
            *lw = (*lw).max(lv + 1);
            let d = indeg.get_mut(&w).expect("in component");
            *d -= 1;
            if *d == 0 {
                queue.push(w);
            }
        }
    }
    let layers_count = layer.values().max().map_or(1, |m| m + 1);
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); layers_count];
    for &v in comp {
        layers[layer.get(&v).copied().unwrap_or(0)].push(v);
    }
    // Order by barycentres.
    let mut pos: HashMap<usize, f32> = HashMap::new();
    let renumber = |layers: &[Vec<usize>], pos: &mut HashMap<usize, f32>| {
        for l in layers {
            for (i, &v) in l.iter().enumerate() {
                pos.insert(v, i as f32);
            }
        }
    };
    renumber(&layers, &mut pos);
    for round in 0..options.sweeps {
        let down = round % 2 == 0;
        let order: Vec<usize> = if down {
            (1..layers.len()).collect()
        } else {
            (0..layers.len().saturating_sub(1)).rev().collect()
        };
        for li in order {
            let neighbours = if down { &pred } else { &succ };
            let mut keyed: Vec<(f32, usize)> = layers[li]
                .iter()
                .map(|&v| {
                    let ns: Vec<f32> = neighbours
                        .get(&v)
                        .into_iter()
                        .flatten()
                        .filter_map(|w| pos.get(w).copied())
                        .collect();
                    let key = if ns.is_empty() {
                        pos[&v]
                    } else {
                        ns.iter().sum::<f32>() / ns.len() as f32
                    };
                    (key, v)
                })
                .collect();
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            layers[li] = keyed.into_iter().map(|(_, v)| v).collect();
            for (i, &v) in layers[li].iter().enumerate() {
                pos.insert(v, i as f32);
            }
        }
    }
    // Coordinates: layers left to right; a long layer becomes several
    // columns; columns are centred on the tallest one.
    let columns: Vec<&[usize]> = layers
        .iter()
        .flat_map(|l| l.chunks(options.max_column.max(1)))
        .collect();
    let heights: Vec<f32> = columns
        .iter()
        .map(|c| {
            c.iter().map(|&v| size[v].y).sum::<f32>()
                + options.node_gap * c.len().saturating_sub(1) as f32
        })
        .collect();
    let total = heights.iter().copied().fold(0.0, f32::max);
    let mut x = 0.0f32;
    for (ci, c) in columns.iter().enumerate() {
        let width = c.iter().map(|&v| size[v].x).fold(0.0, f32::max);
        let mut y = y0 + (total - heights[ci]) / 2.0;
        for &v in *c {
            place(v, Pos2::new(x, y));
            y += size[v].y + options.node_gap;
        }
        x += width + options.layer_gap;
    }
    total
}
