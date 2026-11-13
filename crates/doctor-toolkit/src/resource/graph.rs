//! `graph` resources: node graphs placed in a level (patrol routes, AI paths).
//!
//! Only the first layer of nodes and edges is decoded; the rest of the
//! header is kept as raw words.
//!
//! # Layout
//!
//! Little-endian; pointers are absolute offsets.
//!
//! ```text
//! 0x00  8 zero bytes
//! 0x08  u32  0x10: offset of the GUID
//! 0x0C  u16  node count N, u16 edge count E
//! 0x10  16-byte GUID; SimGroup attributes (`PatrolRelay` 0, ...) reference it
//! 0x20  u32  pointer to N node records (0x24 bytes each)
//! 0x24  u32  pointer to E edge records (0x10 bytes each)
//! 0x28  words up to 0x8C, partly pointers to further layers (not decoded);
//!       0x88 is sdbm("graph") in 1677 of 2178 resources
//! ```
//!
//! A node: `f32 x, y, z, f32 radius, u16 first edge, u16 (0xFFFF in 95 %),
//! u8 edge count, u8, u16, 12 bytes`. The edges of node `k` are the run
//! `first .. first + count`; the runs are consecutive. An edge:
//! `f32 length, u16 target node, 10 bytes`.

use serde::Serialize;

use crate::io::{self, Endian};
use crate::resource::simgroup::Guid;
use crate::{Error, Result};

const NODE_SIZE: usize = 0x24;
const EDGE_SIZE: usize = 0x10;
const HEADER_WORDS: usize = (0x8C - 0x28) / 4;

/// A graph node.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Node {
    pub position: [f32; 3],
    pub radius: f32,
    pub first_edge: u16,
    pub edge_count: u8,
    /// Fields at +0x12, +0x15, +0x16 and the 12 bytes at +0x18, not understood.
    pub unknown: (u16, u8, u16, [u32; 3]),
}

/// A directed edge.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Edge {
    pub length: f32,
    /// Index into [`Graph::nodes`].
    pub target: u16,
    /// Bytes 6..16, not understood.
    pub unknown: [u16; 5],
}

/// A parsed graph (first layer).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Graph {
    pub guid: Guid,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// Header words 0x28..0x8C.
    pub header: Vec<u32>,
}

impl Graph {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let le = Endian::Little;
        let u32_at = |o: usize| io::read_u32(data, o, le);
        let u16_at = |o: usize| io::read_u16(data, o, le);
        let f32_at = |o: usize| u32_at(o).map(f32::from_bits);
        io::slice(data, 0, 0x8C)?;
        if u32_at(8)? != 0x10 {
            return Err(Error::malformed("graph", 8, "GUID offset is not 0x10"));
        }
        let node_count = u16_at(0x0C)? as usize;
        let edge_count = u16_at(0x0E)? as usize;
        let guid = Guid(io::slice(data, 0x10, 16)?.try_into().expect("16 bytes"));
        let nodes_at = u32_at(0x20)? as usize;
        let edges_at = u32_at(0x24)? as usize;
        // Bounds the allocations below by the size of the resource.
        if node_count > 0 {
            io::slice(data, nodes_at, node_count * NODE_SIZE)?;
        }
        if edge_count > 0 {
            io::slice(data, edges_at, edge_count * EDGE_SIZE)?;
        }
        let nodes = (0..node_count)
            .map(|k| {
                let at = nodes_at + NODE_SIZE * k;
                let node = Node {
                    position: [f32_at(at)?, f32_at(at + 4)?, f32_at(at + 8)?],
                    radius: f32_at(at + 0x0C)?,
                    first_edge: u16_at(at + 0x10)?,
                    edge_count: data[at + 0x14],
                    unknown: (
                        u16_at(at + 0x12)?,
                        data[at + 0x15],
                        u16_at(at + 0x16)?,
                        [u32_at(at + 0x18)?, u32_at(at + 0x1C)?, u32_at(at + 0x20)?],
                    ),
                };
                if node.first_edge as usize + node.edge_count as usize > edge_count {
                    return Err(Error::malformed("graph", at + 0x10, "edges out of range"));
                }
                Ok(node)
            })
            .collect::<Result<_>>()?;
        let edges = (0..edge_count)
            .map(|k| {
                let at = edges_at + EDGE_SIZE * k;
                let target = u16_at(at + 4)?;
                if target as usize >= node_count {
                    return Err(Error::malformed(
                        "graph",
                        at + 4,
                        "edge target out of range",
                    ));
                }
                Ok(Edge {
                    length: f32_at(at)?,
                    target,
                    unknown: std::array::from_fn(|i| u16_at(at + 6 + 2 * i).unwrap_or(0)),
                })
            })
            .collect::<Result<_>>()?;
        let header = (0..HEADER_WORDS)
            .map(|i| u32_at(0x28 + 4 * i))
            .collect::<Result<_>>()?;
        Ok(Self {
            guid,
            nodes,
            edges,
            header,
        })
    }

    /// Edges leaving node `index`.
    pub fn node_edges(&self, index: usize) -> &[Edge] {
        let n = &self.nodes[index];
        let start = n.first_edge as usize;
        &self.edges[start..start + n.edge_count as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two nodes joined both ways.
    fn sample() -> Vec<u8> {
        let mut d = vec![0u8; 0x100];
        let put =
            |d: &mut Vec<u8>, at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut d, 8, 0x10);
        put(&mut d, 0x0C, 0x0002_0002);
        d[0x18..0x20].copy_from_slice(b"CREATOR1");
        put(&mut d, 0x20, 0x94);
        put(&mut d, 0x24, 0xDC);
        put(&mut d, 0x88, crate::hash::sdbm(b"graph"));
        for (k, x) in [1.0f32, 2.0].into_iter().enumerate() {
            let at = 0x94 + NODE_SIZE * k;
            put(&mut d, at, x.to_bits());
            put(&mut d, at + 0x0C, 0.5f32.to_bits());
            put(&mut d, at + 0x10, 0xFFFF_0000 | k as u32);
            d[at + 0x14] = 1;
            let e = 0xDC + EDGE_SIZE * k;
            put(&mut d, e, 1.0f32.to_bits());
            put(&mut d, e + 4, 1 - k as u32);
        }
        d
    }

    #[test]
    fn parses_nodes_and_edges() {
        let g = Graph::parse(&sample()).unwrap();
        assert_eq!(&g.guid.0[8..], b"CREATOR1");
        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.nodes[1].position, [2.0, 0.0, 0.0]);
        assert_eq!(g.node_edges(0)[0].target, 1);
        assert_eq!(g.node_edges(1)[0].target, 0);
        assert_eq!(g.header[(0x88 - 0x28) / 4], crate::hash::sdbm(b"graph"));
    }

    #[test]
    fn rejects_bad_input() {
        let d = sample();
        assert!(Graph::parse(&d[..0x80]).is_err());
        let mut bad = d.clone();
        bad[0xE0] = 7;
        assert!(Graph::parse(&bad).is_err());
        let mut huge = d.clone();
        huge[0x0C..0x10].copy_from_slice(&[0xFF; 4]);
        assert!(Graph::parse(&huge).is_err());
        for i in 0..d.len() {
            let mut c = d.clone();
            c[i] ^= 0x41;
            let _ = Graph::parse(&c);
        }
    }
}
