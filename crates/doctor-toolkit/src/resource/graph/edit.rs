//! Editing a graph: nodes, links and flags, with the data that follows from
//! them (link lengths, visibility, R-tree, precomputed paths) kept right.
//!
//! Moving a node keeps the shape of the R-tree (its boxes follow the
//! positions); adding or removing a node builds a new tree
//! ([`derive::build_rtree`]). A path table that no longer gives shortest
//! paths is computed again ([`derive::compute_paths`]); one that still does
//! is kept, so an edit that changes no path changes no byte of it.

use serde::{Deserialize, Serialize};

use super::derive::{self, distance};
use super::{Graph, Link, MAX_LINKS, Node, RShape, RTree, Visibility, path_table_len};
use crate::resource::simgroup::Guid;
use crate::{Error, Result};

/// How [`Graph::attach_point`] links a new node into the graph.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Neighbourhood {
    /// Most nodes to link to, nearest first.
    pub max_links: usize,
    /// Only nodes closer than this.
    pub max_distance: f32,
    /// Link back from each of them too.
    pub two_way: bool,
}

impl Default for Neighbourhood {
    fn default() -> Self {
        Self {
            max_links: 2,
            max_distance: 15.0,
            two_way: true,
        }
    }
}

fn bad(what: impl Into<String>) -> Error {
    Error::InvalidArgument(what.into())
}

impl Graph {
    fn check_node(&self, k: usize) -> Result<()> {
        if k < self.nodes.len() {
            Ok(())
        } else {
            Err(bad(format!(
                "node {k} does not exist (the graph has {})",
                self.nodes.len()
            )))
        }
    }

    /// Computes the path table again if it no longer gives shortest paths.
    pub fn refresh_paths(&mut self) {
        if let Some(t) = &self.precomputed_paths
            && !derive::paths_valid(self, t)
        {
            self.precomputed_paths = derive::compute_paths(self);
        }
    }

    /// A new R-tree for the current positions.
    pub fn rebuild_rtree(&mut self) {
        self.rtree = derive::build_rtree(&self.nodes);
    }

    /// Sets every link length to the distance between its nodes.
    pub fn fix_lengths(&mut self) -> usize {
        let stale = self.stale_lengths();
        for &(k, j) in &stale {
            let to = self.nodes[k].links[j].to as usize;
            self.nodes[k].links[j].length =
                distance(self.nodes[k].position, self.nodes[to].position);
        }
        if !stale.is_empty() {
            self.refresh_paths();
        }
        stale.len()
    }

    /// Moves a node; lengths of the links at it follow.
    pub fn move_node(&mut self, k: usize, position: [f32; 3]) -> Result<()> {
        self.check_node(k)?;
        if !position.iter().all(|v| v.is_finite()) {
            return Err(bad("position is not a number"));
        }
        self.nodes[k].position = position;
        for from in 0..self.nodes.len() {
            for j in 0..self.nodes[from].links.len() {
                let to = self.nodes[from].links[j].to as usize;
                if from == k || to == k {
                    self.nodes[from].links[j].length =
                        distance(self.nodes[from].position, self.nodes[to].position);
                }
            }
        }
        if matches!(self.rtree, RTree::Raw { .. }) {
            self.rebuild_rtree();
        }
        self.refresh_paths();
        Ok(())
    }

    pub fn set_radius(&mut self, k: usize, radius: f32) -> Result<()> {
        self.check_node(k)?;
        if !radius.is_finite() || radius < 0.0 {
            return Err(bad("radius must be a non-negative number"));
        }
        self.nodes[k].radius = radius;
        Ok(())
    }

    pub fn set_node_flags(&mut self, k: usize, flags: u16) -> Result<()> {
        self.check_node(k)?;
        self.nodes[k].flags = flags;
        Ok(())
    }

    /// Visibility as hidden pairs (raw bits are decoded).
    fn visibility_pairs(&mut self) -> &mut Vec<[u16; 2]> {
        if let Visibility::Raw { .. } = self.visibility {
            let hidden = derive::hidden_pairs(self.nodes.len(), &self.visibility);
            self.visibility = Visibility::Pairs { hidden };
        }
        match &mut self.visibility {
            Visibility::Pairs { hidden } => hidden,
            Visibility::Raw { .. } => unreachable!("converted above"),
        }
    }

    /// Adds a node without links; returns its index.
    pub fn add_node(&mut self, position: [f32; 3], radius: f32) -> Result<u16> {
        if self.nodes.len() >= u16::MAX as usize {
            return Err(bad("the graph is full"));
        }
        if !position.iter().all(|v| v.is_finite()) || !radius.is_finite() || radius < 0.0 {
            return Err(bad("position or radius is not a valid number"));
        }
        self.visibility_pairs();
        self.nodes.push(Node {
            position,
            radius,
            flags: 0,
            properties: None,
            light_set: 0,
            links: Vec::new(),
            neighbour_links: Vec::new(),
        });
        self.rebuild_rtree();
        if self.precomputed_paths.is_some() {
            self.precomputed_paths = derive::compute_paths(self);
        }
        Ok((self.nodes.len() - 1) as u16)
    }

    /// Removes a node and every link to it. Returns the new index of every
    /// old node (`None` for the removed one), for [`Self::remap_links_into`]
    /// on graphs that link into this one.
    pub fn remove_node(&mut self, k: usize) -> Result<Vec<Option<u16>>> {
        self.check_node(k)?;
        let map: Vec<Option<u16>> = (0..self.nodes.len())
            .map(|i| match i.cmp(&k) {
                std::cmp::Ordering::Less => Some(i as u16),
                std::cmp::Ordering::Equal => None,
                std::cmp::Ordering::Greater => Some((i - 1) as u16),
            })
            .collect();
        let hidden = self.visibility_pairs();
        *hidden = hidden
            .iter()
            .filter_map(|&[a, b]| Some([map[a as usize]?, map[b as usize]?]))
            .collect();
        self.nodes.remove(k);
        for node in &mut self.nodes {
            node.links.retain(|l| l.to as usize != k);
            for l in &mut node.links {
                l.to = map[l.to as usize].expect("links to the node are gone");
            }
        }
        self.rebuild_rtree();
        if self.precomputed_paths.is_some() {
            self.precomputed_paths = derive::compute_paths(self);
        }
        Ok(map)
    }

    /// Adds a one-way link `from → to` with its length.
    pub fn add_link(&mut self, from: usize, to: usize, flags: u16) -> Result<()> {
        self.check_node(from)?;
        self.check_node(to)?;
        if from == to {
            return Err(bad("a node cannot link to itself"));
        }
        if self.nodes[from].links.iter().any(|l| l.to as usize == to) {
            return Err(bad(format!("link {from} → {to} exists")));
        }
        if self.nodes[from].links.len() >= MAX_LINKS {
            return Err(bad(format!("node {from} has 255 links")));
        }
        if self.link_count() >= u16::MAX as usize {
            return Err(bad("the graph has 65535 links"));
        }
        let length = distance(self.nodes[from].position, self.nodes[to].position);
        self.nodes[from].links.push(Link {
            to: to as u16,
            length,
            flags,
            properties: None,
        });
        if self.precomputed_paths.is_some() {
            // A new first link may be shorter; a node with 16 links has no table.
            self.refresh_paths();
        }
        Ok(())
    }

    /// Removes the link `from → to`.
    pub fn remove_link(&mut self, from: usize, to: usize) -> Result<()> {
        self.check_node(from)?;
        let links = &mut self.nodes[from].links;
        let before = links.len();
        links.retain(|l| l.to as usize != to);
        if links.len() == before {
            return Err(bad(format!("no link {from} → {to}")));
        }
        if self.precomputed_paths.is_some() {
            // Indices of later links moved: the table is always computed again.
            self.precomputed_paths = derive::compute_paths(self);
        }
        Ok(())
    }

    pub fn set_link_flags(&mut self, from: usize, to: usize, flags: u16) -> Result<()> {
        self.check_node(from)?;
        let link = self.nodes[from]
            .links
            .iter_mut()
            .find(|l| l.to as usize == to)
            .ok_or_else(|| bad(format!("no link {from} → {to}")))?;
        link.flags = flags;
        Ok(())
    }

    /// The node nearest to a position.
    pub fn nearest_node(&self, position: [f32; 3]) -> Option<usize> {
        (0..self.nodes.len()).min_by(|&a, &b| {
            distance(self.nodes[a].position, position)
                .partial_cmp(&distance(self.nodes[b].position, position))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    /// Adds a node at `position` and links it with the nearest nodes, so
    /// that AI can reach a new object: up to `max_links` nodes closer than
    /// `max_distance`, enabled ones only. Returns the new node and the
    /// nodes it was linked with; fails when no node is close enough.
    pub fn attach_point(
        &mut self,
        position: [f32; 3],
        radius: f32,
        how: Neighbourhood,
    ) -> Result<(u16, Vec<u16>)> {
        let mut near: Vec<(f32, usize)> = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.flags & super::node_flags::DISABLED == 0)
            .map(|(k, n)| (distance(n.position, position), k))
            .filter(|(d, _)| *d <= how.max_distance)
            .collect();
        near.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        near.truncate(how.max_links.max(1));
        let near: Vec<usize> = near
            .into_iter()
            .map(|(_, k)| k)
            .filter(|&k| self.nodes[k].links.len() < MAX_LINKS)
            .collect();
        if near.is_empty() {
            return Err(bad(format!(
                "no node of the graph is within {} of the point",
                how.max_distance
            )));
        }
        let paths = self.precomputed_paths.take();
        let added = self.add_node(position, radius)?;
        for &k in &near {
            self.add_link(added as usize, k, 0)?;
            if how.two_way {
                self.add_link(k, added as usize, 0)?;
            }
        }
        if paths.is_some() {
            self.precomputed_paths = derive::compute_paths(self);
        }
        Ok((added, near.iter().map(|&k| k as u16).collect()))
    }

    /// Follows a removal or renumbering of the nodes of the graph `target`:
    /// neighbour links to removed nodes go, the others get the new index.
    /// Returns how many links changed.
    pub fn remap_links_into(&mut self, target: Guid, map: &[Option<u16>]) -> usize {
        let Some(slot) = self.neighbours.iter().position(|g| *g == target) else {
            return 0;
        };
        let mut changed = 0;
        for node in &mut self.nodes {
            let before = node.neighbour_links.len();
            node.neighbour_links.retain(|l| {
                l.neighbour as usize != slot
                    || map.get(l.node as usize).is_some_and(|m| m.is_some())
            });
            changed += before - node.neighbour_links.len();
            for l in &mut node.neighbour_links {
                if l.neighbour as usize == slot
                    && let Some(Some(new)) = map.get(l.node as usize)
                    && *new != l.node
                {
                    l.node = *new;
                    changed += 1;
                }
            }
        }
        changed
    }

    /// Repairs what an edit of the JSON form may have left behind: stale
    /// link lengths, an R-tree that misses nodes, a path table of the wrong
    /// size or with wrong paths, hidden pairs out of range.
    pub fn repair_derived(&mut self) -> Vec<String> {
        let mut notes = Vec::new();
        let n = self.nodes.len();
        let fixed = self.fix_lengths();
        if fixed > 0 {
            notes.push(format!("{fixed} link length(s) set to the node distance"));
        }
        let tree_ok = match &self.rtree {
            RTree::Empty => n == 0,
            RTree::Shape(shape) => {
                let mut seen = vec![false; n];
                derive::shape_covers(shape, &mut seen) && seen.iter().all(|s| *s)
            }
            RTree::Raw { indices, .. } => indices.len() == n,
        };
        if !tree_ok {
            self.rebuild_rtree();
            notes.push("R-tree built again".into());
        }
        if let Some(t) = &self.precomputed_paths
            && (t.0.len() != path_table_len(n) || !derive::paths_valid(self, t))
        {
            self.precomputed_paths = derive::compute_paths(self);
            notes.push(match self.precomputed_paths {
                Some(_) => "precomputed paths computed again".into(),
                None => "precomputed paths dropped: a node has more than 15 links".into(),
            });
        }
        if let Visibility::Pairs { hidden } = &mut self.visibility {
            let before = hidden.len();
            hidden.retain(|&[a, b]| a != b && (a as usize) < n && (b as usize) < n);
            if hidden.len() != before {
                notes.push("hidden pairs out of range dropped".into());
            }
        }
        notes
    }

    /// Leaves of the R-tree with their node indices, for display.
    pub fn rtree_leaves(&self) -> Vec<Vec<u16>> {
        fn walk(s: &RShape, out: &mut Vec<Vec<u16>>) {
            match s {
                RShape::Leaf(ids) => out.push(ids.clone()),
                RShape::Inner(kids) => kids.iter().for_each(|k| walk(k, out)),
            }
        }
        let mut out = Vec::new();
        if let RTree::Shape(s) = &self.rtree {
            walk(s, &mut out);
        }
        out
    }
}
