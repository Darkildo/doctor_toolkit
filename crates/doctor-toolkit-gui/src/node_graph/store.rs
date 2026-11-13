//! Node positions and comments kept beside the edited data (a sidecar JSON
//! file), keyed by strings the caller chooses for its nodes.

use std::collections::BTreeMap;
use std::path::Path;

use egui::{Color32, Pos2, Rect};
use serde::{Deserialize, Serialize};

use super::{Comment, Graph, NodeId};

/// A comment as stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredComment {
    pub text: String,
    /// `[x0, y0, x1, y1]` in graph space.
    pub rect: [f32; 4],
    /// RGBA.
    pub color: [u8; 4],
}

/// What the sidecar file holds.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LayoutStore {
    pub version: u32,
    /// Node key → top-left corner.
    pub positions: BTreeMap<String, [f32; 2]>,
    #[serde(default)]
    pub comments: Vec<StoredComment>,
}

impl LayoutStore {
    /// Reads a sidecar file; a missing or broken file is an empty store.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Writes the file (creating its directory).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text + "\n")?;
        std::fs::rename(tmp, path)
    }

    /// Records the positions and comments of `graph`; `key` gives the
    /// stored key of a node (`None`: not stored).
    pub fn capture(&mut self, graph: &Graph, key: &dyn Fn(NodeId) -> Option<String>) {
        self.version = 1;
        for n in &graph.nodes {
            if let Some(k) = key(n.id) {
                self.positions.insert(k, [n.pos.x, n.pos.y]);
            }
        }
        self.comments = graph
            .comments
            .iter()
            .map(|c| StoredComment {
                text: c.text.clone(),
                rect: [c.rect.min.x, c.rect.min.y, c.rect.max.x, c.rect.max.y],
                color: c.color.to_array(),
            })
            .collect();
    }

    /// Puts stored positions and comments into `graph`. Returns the nodes
    /// that had no stored position.
    pub fn restore(
        &self,
        graph: &mut Graph,
        key: &dyn Fn(NodeId) -> Option<String>,
    ) -> Vec<NodeId> {
        let mut missing = Vec::new();
        for n in &mut graph.nodes {
            match key(n.id).and_then(|k| self.positions.get(&k)) {
                Some(&[x, y]) => n.pos = Pos2::new(x, y),
                None => missing.push(n.id),
            }
        }
        graph.comments = self
            .comments
            .iter()
            .enumerate()
            .map(|(i, c)| Comment {
                id: i as u64 + 1,
                rect: Rect::from_min_max(
                    Pos2::new(c.rect[0], c.rect[1]),
                    Pos2::new(c.rect[2], c.rect[3]),
                ),
                text: c.text.clone(),
                color: Color32::from_rgba_premultiplied(
                    c.color[0], c.color[1], c.color[2], c.color[3],
                ),
            })
            .collect();
        missing
    }
}
