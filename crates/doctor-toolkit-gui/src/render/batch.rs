//! Cheaper draw calls (stage 67): the vertices and indices of every mesh
//! in a few shared buffers ([`Arena`]), so that the buffers are bound once
//! per page instead of once per mesh; parts with the same material sharing
//! one bind group, set again only when it changes; and neighbouring parts
//! of one buffer and material in runs ([`batches`]) that one indirect
//! multi-draw call draws.
//!
//! The parts keep the order of the renderer before: by mesh, then by part,
//! opaque parts before alpha-blended ones. Surfaces that lie in one plane
//! (the ground and its patches) take the colour of the part drawn last, so
//! another order changes pixels; sorting by material (`sorted`, see
//! [`Batching`]) halves the calls again but is only an experiment.
//!
//! The shaders, pipelines and vertex layouts stay as they are: a part is
//! still an indexed, instanced draw (`first_index`, `base_vertex` and
//! `first_instance` into the shared buffers).

use eframe::wgpu;

/// Vertices of a shared page (48 bytes each: 48 MiB).
pub const PAGE_VERTICES: u32 = 1 << 20;
/// Indices of a shared page (4 bytes each: 16 MiB).
pub const PAGE_INDICES: u32 = 4 << 20;

/// Free ranges of a buffer, sorted and never touching each other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreeList {
    ranges: Vec<(u32, u32)>,
    capacity: u32,
}

impl FreeList {
    pub fn new(capacity: u32) -> Self {
        Self {
            ranges: if capacity > 0 {
                vec![(0, capacity)]
            } else {
                Vec::new()
            },
            capacity,
        }
    }

    /// The start of `len` free elements (first fit).
    pub fn alloc(&mut self, len: u32) -> Option<u32> {
        if len == 0 {
            return Some(0);
        }
        let i = self.ranges.iter().position(|&(_, l)| l >= len)?;
        let (start, l) = self.ranges[i];
        if l == len {
            self.ranges.remove(i);
        } else {
            self.ranges[i] = (start + len, l - len);
        }
        Some(start)
    }

    /// Gives back `len` elements at `start`, joining them with neighbours.
    pub fn free(&mut self, start: u32, len: u32) {
        if len == 0 {
            return;
        }
        let i = self.ranges.partition_point(|&(s, _)| s < start);
        let joins_previous = i > 0 && {
            let (s, l) = self.ranges[i - 1];
            s + l == start
        };
        let joins_next = i < self.ranges.len() && start + len == self.ranges[i].0;
        match (joins_previous, joins_next) {
            (true, true) => {
                let next = self.ranges.remove(i).1;
                self.ranges[i - 1].1 += len + next;
            }
            (true, false) => self.ranges[i - 1].1 += len,
            (false, true) => self.ranges[i] = (start, len + self.ranges[i].1),
            (false, false) => self.ranges.insert(i, (start, len)),
        }
    }

    /// Whether nothing is allocated.
    pub fn is_empty(&self) -> bool {
        self.capacity == 0 || self.ranges == [(0, self.capacity)]
    }

    /// Free elements in all.
    #[cfg(test)]
    pub fn free_len(&self) -> u32 {
        self.ranges.iter().map(|r| r.1).sum()
    }
}

/// Where a mesh lives in the arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub page: u32,
    pub first_vertex: u32,
    pub vertex_count: u32,
    pub first_index: u32,
    pub index_count: u32,
}

/// A vertex and an index buffer shared by many meshes.
pub struct Page {
    pub vertices: wgpu::Buffer,
    pub indices: wgpu::Buffer,
    vertex_free: FreeList,
    index_free: FreeList,
    /// Made for one mesh larger than a page; dropped when it goes.
    own: bool,
}

/// Shared vertex and index buffers of the meshes of a renderer.
pub struct Arena {
    pages: Vec<Option<Page>>,
    vertex_size: u64,
    /// Largest buffer the device takes, in bytes.
    max_buffer: u64,
    /// Meshes share pages; without, every mesh has its own page and starts
    /// at vertex 0 (devices without a base vertex in indexed draws).
    shared: bool,
}

impl Arena {
    pub fn new(vertex_size: usize, max_buffer: u64, shared: bool) -> Self {
        Self {
            pages: Vec::new(),
            vertex_size: vertex_size as u64,
            max_buffer,
            shared,
        }
    }

    pub fn page(&self, page: u32) -> Option<&Page> {
        self.pages.get(page as usize)?.as_ref()
    }

    /// Bytes of GPU memory the pages take.
    pub fn bytes(&self) -> u64 {
        self.pages
            .iter()
            .flatten()
            .map(|p| p.vertices.size() + p.indices.size())
            .sum()
    }

    /// Room for a mesh of `vertices` and `indices`; `None` when it does not
    /// fit in a buffer of the device.
    pub fn alloc(&mut self, device: &wgpu::Device, vertices: u32, indices: u32) -> Option<Slot> {
        for (p, page) in self.pages.iter_mut().enumerate() {
            let Some(page) = page.as_mut().filter(|p| !p.own) else {
                continue;
            };
            if let Some(slot) = page.take(p as u32, vertices, indices) {
                return Some(slot);
            }
        }
        let own = !self.shared || vertices > PAGE_VERTICES || indices > PAGE_INDICES;
        let (vertex_capacity, index_capacity) = if own {
            (vertices.max(1), indices.max(1))
        } else {
            (PAGE_VERTICES, PAGE_INDICES)
        };
        // Smaller pages on devices with small buffers.
        let max_vertices = (self.max_buffer / self.vertex_size).min(u32::MAX as u64) as u32;
        let max_indices = (self.max_buffer / 4).min(u32::MAX as u64) as u32;
        let vertex_capacity = vertex_capacity.min(max_vertices);
        let index_capacity = index_capacity.min(max_indices);
        if vertices > vertex_capacity || indices > index_capacity {
            return None;
        }
        let buffer = |label, size: u64, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size.max(4).next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT),
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let mut page = Page {
            vertices: buffer(
                "mesh vertices",
                vertex_capacity as u64 * self.vertex_size,
                wgpu::BufferUsages::VERTEX,
            ),
            indices: buffer(
                "mesh indices",
                index_capacity as u64 * 4,
                wgpu::BufferUsages::INDEX,
            ),
            vertex_free: FreeList::new(vertex_capacity),
            index_free: FreeList::new(index_capacity),
            own,
        };
        let index = match self.pages.iter().position(Option::is_none) {
            Some(i) => i,
            None => {
                self.pages.push(None);
                self.pages.len() - 1
            }
        };
        let slot = page.take(index as u32, vertices, indices);
        self.pages[index] = Some(page);
        slot
    }

    /// Gives the room of a mesh back.
    pub fn free(&mut self, slot: Slot) {
        let Some(page) = self.pages.get_mut(slot.page as usize) else {
            return;
        };
        let Some(p) = page.as_mut() else { return };
        p.vertex_free.free(slot.first_vertex, slot.vertex_count);
        p.index_free.free(slot.first_index, slot.index_count);
        if p.own && p.vertex_free.is_empty() && p.index_free.is_empty() {
            *page = None;
        }
    }

    /// Writes the data of a mesh into its room.
    pub fn write(&self, queue: &wgpu::Queue, slot: Slot, vertices: &[u8], indices: &[u8]) {
        let Some(page) = self.page(slot.page) else {
            return;
        };
        if !vertices.is_empty() {
            queue.write_buffer(
                &page.vertices,
                slot.first_vertex as u64 * self.vertex_size,
                vertices,
            );
        }
        if !indices.is_empty() {
            queue.write_buffer(&page.indices, slot.first_index as u64 * 4, indices);
        }
    }
}

impl Page {
    fn take(&mut self, page: u32, vertices: u32, indices: u32) -> Option<Slot> {
        let first_vertex = self.vertex_free.alloc(vertices)?;
        let Some(first_index) = self.index_free.alloc(indices) else {
            self.vertex_free.free(first_vertex, vertices);
            return None;
        };
        Some(Slot {
            page,
            first_vertex,
            vertex_count: vertices,
            first_index,
            index_count: indices,
        })
    }
}

/// What a run of parts shares: the pass (opaque or blended), the buffers and
/// the material.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DrawKey {
    pub blend: bool,
    pub page: u32,
    pub material: u32,
}

/// One part of one mesh for all its visible instances.
#[derive(Debug, Clone, Copy)]
pub struct DrawCmd {
    pub key: DrawKey,
    pub args: wgpu::util::DrawIndexedIndirectArgs,
}

/// Commands `first..first + count` of a sorted list, drawn with one call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    pub key: DrawKey,
    pub first: u32,
    pub count: u32,
}

/// Cuts the commands (given in the order the renderer draws them) into
/// runs of one key. With `sort` the opaque parts are first grouped by
/// buffer and material, otherwise in their order; alpha-blended parts
/// always keep their order, since it decides how they mix.
pub fn batches(cmds: &mut [DrawCmd], sort: bool) -> Vec<Run> {
    if sort {
        // A stable sort: the order within a key stays.
        cmds.sort_by_key(|c| {
            if c.key.blend {
                (true, 0, 0)
            } else {
                (false, c.key.page, c.key.material)
            }
        });
    }
    let mut runs: Vec<Run> = Vec::new();
    for (i, cmd) in cmds.iter().enumerate() {
        match runs.last_mut() {
            Some(run) if run.key == cmd.key => run.count += 1,
            _ => runs.push(Run {
                key: cmd.key,
                first: i as u32,
                count: 1,
            }),
        }
    }
    runs
}

/// How the parts of a frame are sent to the GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Batching {
    /// Runs of more than one part go in one indirect multi-draw call
    /// (devices with indirect draws that start at an instance); otherwise
    /// every part is a direct draw call.
    pub indirect: bool,
    /// Opaque parts are grouped by buffer and material (fewer calls, but
    /// pixels where surfaces lie in one plane may change).
    pub sort: bool,
}

impl Batching {
    /// Whether the device draws indirect calls that start at an instance.
    pub fn device_can_draw_indirect(device: &wgpu::Device, adapter: &wgpu::Adapter) -> bool {
        device
            .features()
            .contains(wgpu::Features::INDIRECT_FIRST_INSTANCE)
            && adapter
                .get_downlevel_capabilities()
                .flags
                .contains(wgpu::DownlevelFlags::INDIRECT_EXECUTION)
    }

    /// The best the device can do in the order of the parts, unless
    /// `DOCTOR_TOOLKIT_GUI_BATCHING` says otherwise: comma-separated
    /// `direct` (no indirect calls), `sorted` (group opaque parts).
    pub fn for_device(device: &wgpu::Device, adapter: &wgpu::Adapter) -> Self {
        let spec = std::env::var("DOCTOR_TOOLKIT_GUI_BATCHING").unwrap_or_default();
        let has = |word: &str| spec.split(',').any(|w| w.trim() == word);
        Self {
            indirect: Self::device_can_draw_indirect(device, adapter) && !has("direct"),
            sort: has("sorted"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_list_allocates_first_fit_and_joins_neighbours() {
        let mut list = FreeList::new(100);
        assert_eq!(list.alloc(30), Some(0));
        assert_eq!(list.alloc(30), Some(30));
        assert_eq!(list.alloc(30), Some(60));
        assert_eq!(list.alloc(30), None);
        assert_eq!(list.free_len(), 10);
        list.free(30, 30);
        assert_eq!(list.alloc(20), Some(30));
        list.free(0, 30);
        // 0..30 free joined with nothing (30..50 used), 50..60 and 90..100.
        assert_eq!(list.free_len(), 50);
        list.free(30, 20);
        // 0..60 in one piece.
        assert_eq!(list.alloc(60), Some(0));
        list.free(0, 60);
        list.free(60, 30);
        assert!(list.is_empty());
        assert_eq!(list.alloc(100), Some(0));
        assert_eq!(list.alloc(0), Some(0));
        assert!(FreeList::new(0).is_empty());
    }

    #[test]
    fn free_list_survives_random_use() {
        let mut list = FreeList::new(1000);
        let mut used: Vec<(u32, u32)> = Vec::new();
        let mut seed = 12345u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            seed >> 16
        };
        for _ in 0..2000 {
            if next() % 3 != 0 || used.is_empty() {
                let len = next() % 50 + 1;
                if let Some(start) = list.alloc(len) {
                    // Never overlaps a used range.
                    assert!(
                        used.iter()
                            .all(|&(s, l)| start + len <= s || s + l <= start)
                    );
                    used.push((start, len));
                }
            } else {
                let (s, l) = used.swap_remove(next() as usize % used.len());
                list.free(s, l);
            }
            let in_use: u32 = used.iter().map(|r| r.1).sum();
            assert_eq!(list.free_len() + in_use, 1000);
        }
        for (s, l) in used.drain(..) {
            list.free(s, l);
        }
        assert!(list.is_empty());
    }

    fn cmd(blend: bool, page: u32, material: u32, first_index: u32) -> DrawCmd {
        DrawCmd {
            key: DrawKey {
                blend,
                page,
                material,
            },
            args: wgpu::util::DrawIndexedIndirectArgs {
                index_count: 3,
                instance_count: 1,
                first_index,
                base_vertex: 0,
                first_instance: 0,
            },
        }
    }

    #[test]
    fn opaque_parts_are_grouped_and_blended_ones_keep_their_order() {
        let mut cmds = vec![
            cmd(false, 0, 2, 0),
            cmd(true, 0, 5, 1),
            cmd(false, 0, 1, 2),
            cmd(true, 0, 4, 3),
            cmd(false, 0, 2, 4),
            cmd(true, 0, 4, 5),
            cmd(false, 1, 1, 6),
        ];
        let runs = batches(&mut cmds, true);
        let order: Vec<u32> = cmds.iter().map(|c| c.args.first_index).collect();
        assert_eq!(order, [2, 0, 4, 6, 1, 3, 5]);
        let shape: Vec<(bool, u32, u32, u32)> = runs
            .iter()
            .map(|r| (r.key.blend, r.key.material, r.first, r.count))
            .collect();
        assert_eq!(
            shape,
            [
                (false, 1, 0, 1),
                (false, 2, 1, 2),
                (false, 1, 3, 1),
                (true, 5, 4, 1),
                (true, 4, 5, 2)
            ]
        );
        // Unsorted: neighbours only.
        let mut cmds = vec![
            cmd(false, 0, 2, 0),
            cmd(false, 0, 2, 1),
            cmd(false, 0, 1, 2),
        ];
        assert_eq!(batches(&mut cmds, false).len(), 2);
        assert!(batches(&mut [], true).is_empty());
    }
}
