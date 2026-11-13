//! The data section of a packfile as a graph of objects and data blocks.
//!
//! Objects are the targets of virtual fixups (they have a class); data blocks
//! are everything a local fixup points at (array contents, strings). Each node
//! keeps its bytes with the padding that follows it, so laying the graph out
//! again reproduces the section.
//!
//! The Havok 5.5 writer lays out depth first, and [`Graph::layout`] does the same:
//!
//! 1. an object, padded to 16 bytes;
//! 2. the data blocks it points at, in the order of the pointer fields, each
//!    followed at once by the blocks it points at in turn; a local fixup is
//!    recorded when its block is placed;
//! 3. global fixups are recorded in the order the pointers are met;
//! 4. then the objects met through global pointers, each laid out the same way.

use crate::{Error, Result};

use super::{Packfile, SectionBody};

/// A pointer field inside a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pointer {
    /// Offset of the pointer field in the node.
    pub at: u32,
    pub target: usize,
    /// Offset in the target node (0 except for pointers into a block).
    pub offset: u32,
    /// `true` for global fixups (pointers to objects).
    pub global: bool,
}

/// An object or a data block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// Class name for objects, `None` for data blocks.
    pub class: Option<String>,
    pub bytes: Vec<u8>,
    /// Sorted by `at`.
    pub pointers: Vec<Pointer>,
    /// Alignment of the node's start: 16, except for a few strings the Havok
    /// writer packs right after the block that points at them.
    pub align: u32,
}

impl Node {
    pub fn object(class: &str, bytes: Vec<u8>) -> Self {
        Self {
            class: Some(class.to_owned()),
            bytes,
            pointers: Vec::new(),
            align: 16,
        }
    }

    pub fn data(bytes: Vec<u8>) -> Self {
        Self {
            class: None,
            bytes,
            pointers: Vec::new(),
            align: 16,
        }
    }

    pub fn u32(&self, at: u32) -> u32 {
        let at = at as usize;
        self.bytes
            .get(at..at + 4)
            .map_or(0, |b| u32::from_le_bytes(b.try_into().expect("4 bytes")))
    }

    pub fn f32(&self, at: u32) -> f32 {
        f32::from_bits(self.u32(at))
    }

    /// The pointer stored at `at`, if any.
    pub fn pointer(&self, at: u32) -> Option<&Pointer> {
        self.pointers
            .binary_search_by_key(&at, |p| p.at)
            .ok()
            .map(|i| &self.pointers[i])
    }
}

/// The objects and data blocks of a data section.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Graph {
    pub nodes: Vec<Node>,
    /// The root object (the contents of the packfile).
    pub root: usize,
}

/// A data section laid out from a graph.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Layout {
    pub data: Vec<u8>,
    pub local_fixups: Vec<(u32, u32)>,
    /// `(source, target offset)`, all in the data section.
    pub global_fixups: Vec<(u32, u32)>,
    /// `(object offset, class name)`.
    pub objects: Vec<(u32, String)>,
    /// Offset of each node, `None` for nodes the root does not reach.
    pub offsets: Vec<Option<u32>>,
}

impl Graph {
    /// Splits the data section of `packfile` into nodes.
    pub fn from_packfile(packfile: &Packfile) -> Result<Self> {
        let section = packfile
            .section_index("__data__")
            .ok_or_else(|| Error::malformed("hkx", 0, "no __data__ section"))?;
        let body = &packfile.bodies[section];
        let len = body.data.len() as u32;
        let mut starts: Vec<u32> = body
            .virtual_fixups
            .iter()
            .filter(|v| v.0 != super::END)
            .map(|v| v.0)
            .chain(
                body.local_fixups
                    .iter()
                    .filter(|l| l.0 != super::END)
                    .map(|l| l.1),
            )
            .chain(std::iter::once(packfile.contents.1))
            .collect();
        starts.sort_unstable();
        starts.dedup();
        if starts.last().is_some_and(|&s| s >= len) {
            return Err(Error::malformed("hkx", 0, "object or pointer outside data"));
        }
        let classes: std::collections::HashMap<u32, String> = packfile
            .objects
            .iter()
            .filter(|o| o.section == section)
            .map(|o| (o.offset, o.class.clone()))
            .collect();
        let mut nodes: Vec<Node> = starts
            .iter()
            .enumerate()
            .map(|(i, &s)| {
                let end = starts.get(i + 1).copied().unwrap_or(len);
                Node {
                    class: classes.get(&s).cloned(),
                    bytes: body.data[s as usize..end as usize].to_vec(),
                    pointers: Vec::new(),
                    align: 1 << s.trailing_zeros().min(4),
                }
            })
            .collect();
        let node_of = |offset: u32| -> Result<(usize, u32)> {
            if offset >= len {
                return Err(Error::malformed("hkx", 0, "pointer outside data"));
            }
            let i = starts.partition_point(|&s| s <= offset);
            if i == 0 {
                return Err(Error::malformed("hkx", 0, "pointer before the first node"));
            }
            Ok((i - 1, offset - starts[i - 1]))
        };
        let mut add = |src: u32, dst: u32, global: bool| -> Result<()> {
            let (node, at) = node_of(src)?;
            let (target, offset) = node_of(dst)?;
            if at + 4 > nodes[node].bytes.len() as u32 {
                return Err(Error::malformed("hkx", 0, "pointer field across nodes"));
            }
            nodes[node].pointers.push(Pointer {
                at,
                target,
                offset,
                global,
            });
            Ok(())
        };
        for &(src, dst) in body.local_fixups.iter().filter(|l| l.0 != super::END) {
            add(src, dst, false)?;
        }
        for &(src, target_section, dst) in body.global_fixups.iter().filter(|g| g.0 != super::END) {
            if target_section as usize != section {
                return Err(Error::malformed("hkx", 0, "pointer into another section"));
            }
            add(src, dst, true)?;
        }
        for node in &mut nodes {
            node.pointers.sort_by_key(|p| p.at);
            if node.pointers.windows(2).any(|w| w[0].at == w[1].at) {
                return Err(Error::malformed("hkx", 0, "two fixups for one pointer"));
            }
        }
        let root = node_of(packfile.contents.1)?.0;
        Ok(Self { nodes, root })
    }

    /// Adds a node and returns its index.
    pub fn add(&mut self, node: Node) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    /// Records a pointer from `node` at `at` to `target`.
    pub fn link(&mut self, node: usize, at: u32, target: usize) {
        let global = self.nodes[target].class.is_some();
        let pointers = &mut self.nodes[node].pointers;
        let index = pointers.partition_point(|p| p.at < at);
        pointers.insert(
            index,
            Pointer {
                at,
                target,
                offset: 0,
                global,
            },
        );
    }

    /// Lays the nodes reachable from the root out the way the Havok writer does.
    pub fn layout(&self) -> Layout {
        let mut out = Layout {
            offsets: vec![None; self.nodes.len()],
            ..Layout::default()
        };
        let mut globals: Vec<(u32, usize, u32)> = Vec::new();
        self.write_object(self.root, &mut out, &mut globals);
        out.global_fixups = globals
            .into_iter()
            .map(|(src, target, offset)| {
                (
                    src,
                    out.offsets[target].expect("targets are laid out") + offset,
                )
            })
            .collect();
        out
    }

    fn place(&self, node: usize, out: &mut Layout) {
        let align = self.nodes[node].align.max(1) as usize;
        while !out.data.len().is_multiple_of(align) {
            out.data.push(0);
        }
        let at = out.data.len() as u32;
        out.offsets[node] = Some(at);
        out.data.extend_from_slice(&self.nodes[node].bytes);
        if let Some(class) = &self.nodes[node].class {
            out.objects.push((at, class.clone()));
        }
    }

    /// Objects depth first (an explicit stack: game files nest a few levels,
    /// damaged ones could nest thousands).
    fn write_object(&self, root: usize, out: &mut Layout, globals: &mut Vec<(u32, usize, u32)>) {
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            if out.offsets[node].is_some() {
                continue;
            }
            self.place(node, out);
            let mut pending = Vec::new();
            self.write_children(node, out, globals, &mut pending);
            stack.extend(pending.into_iter().rev());
        }
    }

    /// The data blocks of `start`, each followed at once by its own blocks.
    fn write_children(
        &self,
        start: usize,
        out: &mut Layout,
        globals: &mut Vec<(u32, usize, u32)>,
        pending: &mut Vec<usize>,
    ) {
        let mut stack = vec![(start, 0usize)];
        while let Some((node, i)) = stack.pop() {
            let Some(p) = self.nodes[node].pointers.get(i).copied() else {
                continue;
            };
            stack.push((node, i + 1));
            let base = out.offsets[node].expect("placed");
            if p.global {
                globals.push((base + p.at, p.target, p.offset));
                pending.push(p.target);
            } else {
                let fresh = out.offsets[p.target].is_none();
                if fresh {
                    self.place(p.target, out);
                }
                let dst = out.offsets[p.target].expect("placed") + p.offset;
                out.local_fixups.push((base + p.at, dst));
                if fresh {
                    stack.push((p.target, 0));
                }
            }
        }
    }

    /// Writes the laid-out graph into the data section of `packfile`, with
    /// class names from its class name section. Every class must be declared there.
    pub fn store(&self, packfile: &mut Packfile) -> Result<()> {
        let names_section = packfile
            .section_index("__classnames__")
            .ok_or_else(|| Error::malformed("hkx", 0, "no __classnames__ section"))?;
        let data_section = packfile
            .section_index("__data__")
            .ok_or_else(|| Error::malformed("hkx", 0, "no __data__ section"))?;
        let names = super::parse_class_names(&packfile.bodies[names_section].data);
        let name_offset = |class: &str| -> Result<u32> {
            names
                .iter()
                .find(|(_, name, _)| name == class)
                .map(|(_, _, offset)| *offset)
                .ok_or_else(|| {
                    Error::InvalidArgument(format!("class {class} is not declared in the packfile"))
                })
        };
        let mut layout = self.layout();
        while !layout.data.len().is_multiple_of(16) {
            layout.data.push(0);
        }
        let body: &mut SectionBody = &mut packfile.bodies[data_section];
        // Keep the unused entries the Havok writer left, so that an unchanged
        // graph gives back the same bytes.
        let unused =
            |count: usize| std::iter::repeat_n((super::END, super::END, super::END), count);
        let unused_locals = body
            .local_fixups
            .iter()
            .filter(|f| f.0 == super::END)
            .count();
        let unused_globals = body
            .global_fixups
            .iter()
            .filter(|f| f.0 == super::END)
            .count();
        let unused_virtuals = body
            .virtual_fixups
            .iter()
            .filter(|f| f.0 == super::END)
            .count();
        body.data = layout.data;
        body.local_fixups = layout.local_fixups;
        body.local_fixups
            .extend(unused(unused_locals).map(|(a, b, _)| (a, b)));
        body.global_fixups = layout
            .global_fixups
            .iter()
            .map(|&(src, dst)| (src, data_section as u32, dst))
            .chain(unused(unused_globals))
            .collect();
        body.virtual_fixups = layout
            .objects
            .iter()
            .map(|(at, class)| Ok((*at, names_section as u32, name_offset(class)?)))
            .collect::<Result<Vec<_>>>()?;
        body.virtual_fixups.extend(unused(unused_virtuals));
        packfile.contents = (
            data_section as u32,
            layout.offsets[self.root].expect("root"),
        );
        // Refresh the derived views.
        let reparsed = Packfile::parse(&packfile.to_bytes())?;
        *packfile = reparsed;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lays_out_depth_first() {
        let mut g = Graph::default();
        let root = g.add(Node::object("Root", vec![0; 8]));
        let array = g.add(Node::data(vec![0; 8]));
        let a = g.add(Node::object("A", vec![0; 4]));
        let b = g.add(Node::object("B", vec![0; 4]));
        let child = g.add(Node::object("C", vec![0; 4]));
        let name = g.add(Node::data(b"name\0".to_vec()));
        g.root = root;
        g.link(root, 0, array);
        g.link(array, 0, a);
        g.link(array, 4, b);
        g.link(a, 0, child);
        g.link(root, 4, name);
        let layout = g.layout();
        let offsets: Vec<u32> = layout.offsets.iter().map(|o| o.unwrap()).collect();
        // root, its array, its name, then A with its child C, then B.
        assert_eq!(offsets, vec![0, 16, 48, 80, 64, 32]);
        assert_eq!(layout.local_fixups, vec![(0, 16), (4, 32)]);
        assert_eq!(layout.global_fixups, vec![(16, 48), (20, 80), (48, 64)]);
    }

    #[test]
    fn long_chains_do_not_exhaust_the_stack() {
        let mut g = Graph::default();
        let mut previous = g.add(Node::object("Root", vec![0; 16]));
        let mut block = g.add(Node::data(vec![0; 16]));
        g.link(previous, 0, block);
        for _ in 0..200_000 {
            let next = g.add(Node::object("Link", vec![0; 16]));
            g.link(previous, 4, next);
            previous = next;
            let data = g.add(Node::data(vec![0; 16]));
            g.link(block, 0, data);
            block = data;
        }
        let layout = g.layout();
        assert!(layout.offsets.iter().all(Option::is_some));
    }
}
