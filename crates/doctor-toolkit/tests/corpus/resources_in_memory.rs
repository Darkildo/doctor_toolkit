//! Stage 100: the stream the live module composes in memory from a stream
//! of the game and a resource patch, against the file "apply" writes.

use rayon::prelude::*;

use doctor_toolkit::container::ustream::{
    self, BlockKind, Insertion, Replacement, ResourceHeader, Storage, StreamLayout,
};
use doctor_toolkit::game::{GameInstance, StreamFile};
use doctor_toolkit::workspace::deploy::resource_patch;
use doctor_toolkit_live::resources::{self, Kind, Patch};

use super::{Failures, game_dir};

/// A small generator, the same on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 24
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn bytes(&mut self, len: usize) -> Vec<u8> {
        let mut x = self.next() as u32 | 1;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect()
    }
}

/// The scan of the module sees the blocks and resources the toolkit's
/// parser sees.
fn same_layout(scanned: &resources::Layout, parsed: &StreamLayout) -> bool {
    scanned.blocks.len() == parsed.blocks.len()
        && scanned.resources.len() == parsed.resources.len()
        && scanned.blocks.iter().zip(&parsed.blocks).all(|(a, b)| {
            a.offset == b.offset as u64
                && a.size as usize == b.size
                && match b.kind {
                    BlockKind::Header => a.kind == Kind::Header,
                    BlockKind::Fill => a.kind == Kind::Fill,
                    _ => a.kind == Kind::Data,
                }
        })
        && scanned
            .resources
            .iter()
            .zip(&parsed.resources)
            .all(|(a, b)| {
                a.header_block == b.header_block
                    && a.blocks == b.blocks
                    && a.header.type_id == b.header.type_id
                    && a.header.ids == b.header.ids
                    && a.header.data_size == b.header.data_size
                    && a.header.mem_policy == b.header.mem_policy
                    && a.header.alignment == b.header.alignment
            })
}

/// Every stream of the game, with a few replaced and added resources drawn
/// from its name: the stream the module composes is, byte for byte, the
/// file `ustream::rebuild_with` writes with raw storage — the file "apply"
/// writes when it stores the changed resources raw, and the same resources
/// in the same order otherwise. The data size the patch reports is the one
/// the table of contents gets.
#[test]
fn composed_streams_are_the_files_apply_writes() {
    let Some(dir) = game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let failures = Failures::default();
    let composed_streams = std::sync::atomic::AtomicUsize::new(0);
    let scan_reads = std::sync::atomic::AtomicU64::new(0);
    game.streams().par_iter().for_each(|file| {
        let fail = |what: String| failures.push(format!("{}: {what}", file.rel_path));
        let stream = match StreamFile::open(&file.path) {
            Ok(stream) => stream,
            Err(error) => return fail(error.to_string()),
        };
        let (data, layout) = (stream.data(), stream.layout());
        let mut source = data;
        let scanned = match resources::scan(&mut source) {
            Ok(l) => l,
            Err(error) => return fail(format!("scan: {error}")),
        };
        if !same_layout(&scanned, layout) {
            return fail("the scan differs from the parser".into());
        }
        scan_reads.fetch_add(u64::from(scanned.reads), std::sync::atomic::Ordering::Relaxed);
        let count = layout.resources.len();
        if count == 0 {
            return;
        }
        let seed = file
            .rel_path
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
                (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
            });
        let mut rng = Rng(seed);
        // One to three replacements: the same size, smaller, larger (up to
        // a few pages); up to two added resources.
        let mut datas: Vec<(usize, Vec<u8>)> = Vec::new();
        for _ in 0..1 + rng.below(3) {
            let index = rng.below(count);
            if datas.iter().any(|d| d.0 == index) {
                continue;
            }
            let old = layout.resources[index].header.data_size as usize;
            let len = match rng.below(4) {
                0 => old,
                1 => old / 2,
                2 => old + 1 + rng.below(5000),
                _ => rng.below(400_000),
            };
            datas.push((index, rng.bytes(len)));
        }
        let mut added: Vec<(usize, ResourceHeader, Vec<u8>)> = Vec::new();
        for n in 0..rng.below(3) {
            let before = rng.below(count + 1);
            let header = ResourceHeader {
                mem_policy: layout.resources[0].header.mem_policy,
                alignment: 16,
                type_id: 0x0D0C_7000 + n as u32,
                ids: [0xD0C7_0100 + n as u32; 4],
                data_size: 0,
            };
            let len = rng.below(200_000);
            added.push((before, header, rng.bytes(len)));
        }
        added.sort_by_key(|a| a.0);
        let replacements: Vec<Replacement> = datas
            .iter()
            .map(|(index, data)| Replacement {
                index: *index,
                data,
                storage: Storage::Raw,
            })
            .collect();
        let insertions: Vec<Insertion> = added
            .iter()
            .map(|(before, header, data)| Insertion {
                before: *before,
                header: *header,
                data,
                storage: Storage::Raw,
            })
            .collect();
        let rebuilt = match ustream::rebuild_with(data, layout, &replacements, &insertions) {
            Ok(r) => r,
            Err(error) => return fail(format!("rebuild: {error}")),
        };
        let (patch, data_size) = match resource_patch(data, layout, &replacements, &insertions, None)
        {
            Ok(p) => p,
            Err(error) => return fail(format!("patch: {error}")),
        };
        let (mut o, mut p) = (data, &patch[..]);
        let read = Patch::read(&mut p).and_then(|table| {
            let composed = resources::compose(&scanned, &table)?;
            let mut p = &patch[..];
            Ok((composed.to_vec(&mut o, &mut p)?, composed))
        });
        let (all, composed) = match read {
            Ok(r) => r,
            Err(error) => return fail(format!("compose: {error}")),
        };
        if all != rebuilt {
            return fail(format!(
                "the composed stream differs from the rebuilt one ({} and {} bytes, {} replaced, {} added)",
                all.len(),
                rebuilt.len(),
                replacements.len(),
                insertions.len()
            ));
        }
        let total = StreamLayout::parse(&rebuilt).map(|l| l.total_data_size());
        if total.ok() != Some(data_size) || composed.data_size != data_size {
            return fail("the data size differs".into());
        }
        // The game reads it in pieces of 128 KiB from any place.
        let mut buf = vec![0u8; 0x2_0000];
        for _ in 0..4 {
            let at = rng.below(all.len());
            let (mut o, mut p) = (data, &patch[..]);
            match composed.read(at as u64, &mut buf, &mut o, &mut p) {
                Ok(n) if all[at..at + n] == buf[..n] && n == buf.len().min(all.len() - at) => {}
                Ok(_) => return fail(format!("a read at {at:#x} differs")),
                Err(error) => return fail(format!("read: {error}")),
            }
        }
        composed_streams.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    });
    failures.assert_empty();
    let composed = composed_streams.into_inner();
    assert!(composed > 2000, "{composed}");
    eprintln!(
        "{composed} streams composed; the scans took {} reads of 4 KiB in all",
        scan_reads.into_inner()
    );
}
