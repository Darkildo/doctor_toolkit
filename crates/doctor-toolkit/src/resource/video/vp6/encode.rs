//! VP6 encoder: key frames and inter frames built from "no vector, previous
//! frame" macroblocks (the previous picture plus a coded residual), with
//! range-coded coefficients and per-frame probability updates. No motion
//! search: slow or static pictures compress well, fast motion costs bits but
//! keeps the chosen quality.

use super::Picture;
use super::dct;
use super::model::{DcState, Model, Ref, dcct_prob};
use super::rac::{RangeEncoder, bit_cost, tree_path};
use super::tables::*;

/// Encoder settings.
#[derive(Debug, Clone, Copy)]
pub struct EncoderConfig {
    /// VP6 quantizer index, 0 (coarsest) to 63 (finest).
    pub quantizer: u8,
    /// A key frame at least every this many frames.
    pub keyframe_interval: u32,
    /// Mean absolute luma difference to the previous picture above which a
    /// frame is coded as a key frame (scene cut).
    pub scene_cut: u32,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            quantizer: 52,
            keyframe_interval: 30,
            scene_cut: 40,
        }
    }
}

/// The VP6 sub-version written into key frames (as in the game's movies).
pub const SUB_VERSION: u8 = 7;

/// One encoded frame.
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    pub key: bool,
    pub data: Vec<u8>,
}

pub struct Encoder {
    cfg: EncoderConfig,
    model: Model,
    mb_width: usize,
    mb_height: usize,
    recon: Option<Picture>,
    since_key: u32,
}

const SLOT_DCCV: u16 = 0;
const SLOT_DCCT: u16 = 22;
const SLOT_RACT: u16 = 52;
const SLOT_RUNV: u16 = 448;
const SLOT_FIXED: u16 = 476;
const SLOTS: usize = 732;

fn dccv(pt: usize, node: usize) -> u16 {
    SLOT_DCCV + (pt * 11 + node) as u16
}
fn dcct(pt: usize, ctx: usize, node: usize) -> u16 {
    SLOT_DCCT + ((pt * 3 + ctx) * 5 + node) as u16
}
fn ract(pt: usize, ct: usize, cg: usize, node: usize) -> u16 {
    SLOT_RACT + (((pt * 3 + ct) * 6 + cg) * 11 + node) as u16
}
fn runv(cg: usize, node: usize) -> u16 {
    SLOT_RUNV + (cg * 14 + node) as u16
}
fn fixed(prob: u8) -> u16 {
    SLOT_FIXED + prob as u16
}

/// A recorded binary decision: slot in the low 15 bits, the bit on top.
#[derive(Clone, Copy)]
struct Ev(u16);

struct Events {
    list: Vec<Ev>,
    counts: Vec<[u32; 2]>,
}

impl Events {
    fn new() -> Self {
        Self {
            list: Vec::new(),
            counts: vec![[0; 2]; SLOTS],
        }
    }

    fn push(&mut self, slot: u16, bit: bool) {
        self.list.push(Ev(slot | (bit as u16) << 15));
        self.counts[slot as usize][bit as usize] += 1;
    }

    fn cost(&self, slot: u16, prob: u8) -> u64 {
        let [n0, n1] = self.counts[slot as usize];
        n0 as u64 * bit_cost(prob, false) as u64 + n1 as u64 * bit_cost(prob, true) as u64
    }
}

/// The node/first-bit layout of a coefficient model: DC tokens use
/// `dcct` for nodes 0..4 and `dccv` for the rest.
#[derive(Clone, Copy)]
enum Ctx {
    Dc { pt: usize, ctx: usize },
    Ac { pt: usize, ct: usize, cg: usize },
}

impl Ctx {
    /// Slot of `model2[node]` (nodes 0..4).
    fn m2(self, node: usize) -> u16 {
        match self {
            Ctx::Dc { pt, ctx } => dcct(pt, ctx, node),
            Ctx::Ac { pt, ct, cg } => ract(pt, ct, cg, node),
        }
    }
    /// Slot of `model1[node]` (nodes 5..10).
    fn m1(self, node: usize) -> u16 {
        match self {
            Ctx::Dc { pt, .. } => dccv(pt, node),
            Ctx::Ac { pt, ct, cg } => ract(pt, ct, cg, node),
        }
    }
}

/// Records the tokens of one block (`c` in scan order, `c[0]` the DC delta).
/// Returns the scan index where the decoder stops (for its IDCT selector).
fn tokenize(c: &[i32; 64], pt: usize, ctx: usize, ev: &mut Events) -> usize {
    let last = c.iter().rposition(|&v| v != 0);
    let mut ct = 1usize;
    let mut idx = 0usize;
    let mut m = Ctx::Dc { pt, ctx };
    loop {
        let forced = idx > 1 && ct == 0;
        if last.is_none_or(|l| idx > l) {
            debug_assert!(!forced);
            ev.push(m.m2(0), false);
            if idx == 0 {
                ct = 0;
                idx = 1;
            } else {
                ev.push(m.m2(1), false);
                break;
            }
        } else if c[idx] != 0 {
            if !forced {
                ev.push(m.m2(0), true);
            }
            let a = c[idx].unsigned_abs().min(2047 + 67);
            if a == 1 {
                ev.push(m.m2(2), false);
                ct = 1;
            } else {
                ev.push(m.m2(2), true);
                ct = 2;
                if a <= 4 {
                    ev.push(m.m2(3), false);
                    if a == 2 {
                        ev.push(m.m2(4), false);
                    } else {
                        ev.push(m.m2(4), true);
                        ev.push(m.m1(5), a == 4);
                    }
                } else {
                    ev.push(m.m2(3), true);
                    let cat = match a {
                        5..=6 => 0,
                        7..=10 => 1,
                        11..=18 => 2,
                        19..=34 => 3,
                        35..=66 => 4,
                        _ => 5,
                    };
                    for (prob, bit) in tree_path(&PC_TREE, cat as u8) {
                        ev.push(m.m1(prob as usize), bit);
                    }
                    let extra = a - COEFF_BIAS[cat + 5] as u32;
                    for i in (0..=COEFF_BIT_LENGTH[cat] as usize).rev() {
                        ev.push(fixed(COEFF_PARSE_TABLE[cat][i]), extra >> i & 1 != 0);
                    }
                }
            }
            ev.push(fixed(128), c[idx] < 0);
            idx += 1;
        } else {
            ev.push(m.m2(0), false);
            if idx == 0 {
                ct = 0;
                idx = 1;
            } else {
                ev.push(m.m2(1), true);
                let next = (idx..64).find(|&i| c[i] != 0).expect("a later coefficient");
                let run = next - idx;
                let cg = (idx >= 6) as usize;
                if run <= 8 {
                    for (prob, bit) in tree_path(&PCR_TREE, run as u8) {
                        ev.push(runv(cg, prob as usize), bit);
                    }
                } else {
                    for (prob, bit) in tree_path(&PCR_TREE, 0) {
                        ev.push(runv(cg, prob as usize), bit);
                    }
                    for i in 0..6 {
                        ev.push(runv(cg, i + 8), (run - 9) >> i & 1 != 0);
                    }
                }
                ct = 0;
                idx = next;
            }
        }
        if idx >= 64 {
            break;
        }
        m = Ctx::Ac {
            pt,
            ct,
            cg: COEFF_GROUPS[idx] as usize,
        };
    }
    idx
}

impl Encoder {
    /// An encoder for pictures of `width`×`height` (rounded up to whole macroblocks).
    pub fn new(width: usize, height: usize, cfg: EncoderConfig) -> Self {
        Self {
            cfg: EncoderConfig {
                quantizer: cfg.quantizer.min(63),
                keyframe_interval: cfg.keyframe_interval.max(1),
                ..cfg
            },
            model: Model::default(),
            mb_width: width.div_ceil(16),
            mb_height: height.div_ceil(16),
            recon: None,
            since_key: 0,
        }
    }

    /// The decoder's picture after the last encoded frame (coded orientation).
    pub fn reconstruction(&self) -> Option<&Picture> {
        self.recon.as_ref()
    }

    /// Encodes one picture in coded orientation (already padded to whole
    /// macroblocks and, for `vp60` streams, flipped bottom-up).
    pub fn encode(&mut self, src: &Picture) -> EncodedFrame {
        assert_eq!(src.width, self.mb_width * 16);
        assert_eq!(src.height, self.mb_height * 16);
        let key = match &self.recon {
            None => true,
            Some(prev) => {
                self.since_key + 1 >= self.cfg.keyframe_interval
                    || mean_abs_diff(&src.planes[0], &prev.planes[0]) > self.cfg.scene_cut
            }
        };
        let data = self.encode_frame(src, key);
        self.since_key = if key { 0 } else { self.since_key + 1 };
        EncodedFrame { key, data }
    }

    fn encode_frame(&mut self, src: &Picture, key: bool) -> Vec<u8> {
        let q = self.cfg.quantizer as usize;
        let dq_dc = (DC_DEQUANT[q] as i32) << 2;
        let dq_ac = (AC_DEQUANT[q] as i32) << 2;
        if key {
            self.model.reset(SUB_VERSION);
        }
        let mut recon = match (&self.recon, key) {
            (Some(p), false) => p.clone(),
            _ => Picture::new(src.width, src.height),
        };
        let prev = if key { None } else { self.recon.clone() };
        let r = if key { Ref::Current } else { Ref::Previous };
        let permute: [usize; 64] = std::array::from_fn(|i| {
            let z = ZIGZAG[i] as usize;
            (z >> 3) | (z & 7) << 3
        });
        let index_to_pos = self.model.index_to_pos;
        let idct_selector = self.model.idct_selector;
        let (dz_dc, dz_ac) = if key { (0.5, 0.4) } else { (0.45, 0.3) };

        let mut ev = Events::new();
        let mut dcs = DcState::new(self.mb_width);
        for row in 0..self.mb_height {
            dcs.start_row();
            for col in 0..self.mb_width {
                for b in 0..6 {
                    let (plane, offset, stride) = src.block_at(row, col, b);
                    let mut px = [0i32; 64];
                    for y in 0..8 {
                        for x in 0..8 {
                            let s = src.planes[plane][offset + y * stride + x] as i32;
                            px[y * 8 + x] = match &prev {
                                Some(p) => s - p.planes[plane][offset + y * stride + x] as i32,
                                None => s - 128,
                            };
                        }
                    }
                    let f = dct::fdct(&px);
                    let mut scan = [0i32; 64];
                    for (k, s) in scan.iter_mut().enumerate() {
                        let v = f[permute[index_to_pos[k] as usize]];
                        let (dq, dz) = if k == 0 {
                            (dq_dc, dz_dc)
                        } else {
                            (dq_ac, dz_ac)
                        };
                        let a = (v.abs() / dq as f32 + dz).floor() as i32;
                        *s = if v < 0.0 { -a } else { a };
                    }
                    let pred = dcs.predict(b, r);
                    let dc = (scan[0] as i16).clamp(-2000, 2000);
                    scan[0] = dc.wrapping_sub(pred) as i32;
                    if scan[0].unsigned_abs() > 2047 + 67 {
                        // Out of range delta: keep the prediction.
                        scan[0] = 0;
                    }
                    let dc = (scan[0] as i16).wrapping_add(pred);
                    let ctx = dcs.dc_ctx(b);
                    let end = tokenize(&scan, (b > 3) as usize, ctx, &mut ev);
                    dcs.set_not_null(b, scan[0] != 0);
                    dcs.store(b, r, dc);

                    let mut block = [0i16; 64];
                    block[0] = (dc as i32 * dq_dc) as i16;
                    for k in 1..64 {
                        let v = scan[k].clamp(-2047 - 67, 2047 + 67);
                        block[permute[index_to_pos[k] as usize]] = (v * dq_ac) as i16;
                    }
                    let dst = &mut recon.planes[plane];
                    if key {
                        dct::idct(dst, offset, stride, &block, false);
                    } else if idct_selector[end.min(63)] > 1 {
                        dct::idct(dst, offset, stride, &block, true);
                    } else {
                        dct::idct_dc_add(dst, offset, stride, &block);
                    }
                }
                dcs.next_mb();
            }
        }

        // Partition 1: header bits, model updates, macroblock types.
        let mut c = RangeEncoder::new();
        if key {
            c.puts(0, 2);
        } else {
            c.put(false); // golden frame
        }
        c.put(false); // range-coded coefficients, not Huffman
        if !key {
            for _ in 0..3 {
                c.put_prob(174, false);
                c.put_prob(254, false);
            }
            self.model.derive_mb_types();
            for comp in 0..2 {
                c.put_prob(SIG_DCT_PCT[comp][0], false);
                c.put_prob(SIG_DCT_PCT[comp][1], false);
            }
            for comp in 0..2 {
                for node in 0..7 {
                    c.put_prob(PDV_PCT[comp][node], false);
                }
            }
            for comp in 0..2 {
                for node in 0..8 {
                    c.put_prob(FDV_PCT[comp][node], false);
                }
            }
        }
        self.write_coeff_models(&mut c, &ev, key);
        if !key {
            let p = self.model.mb_type[1][0][0];
            for _ in 0..self.mb_width * self.mb_height {
                c.put_prob(p, true);
            }
        }
        let part1 = c.finish();

        // Partition 2: coefficients with the final probabilities.
        let probs = self.slot_probs();
        let mut cc = RangeEncoder::new();
        for e in &ev.list {
            let slot = (e.0 & 0x7FFF) as usize;
            cc.put_prob(probs[slot], e.0 >> 15 != 0);
        }
        let part2 = cc.finish();

        let mut out = Vec::with_capacity(8 + part1.len() + part2.len());
        let separated = 1u8;
        if key {
            out.push((q as u8) << 1 | separated);
            out.push(SUB_VERSION << 3);
            let offset = 8 + part1.len();
            out.extend_from_slice(&(offset as u16).to_be_bytes());
            let (rows, cols) = (self.mb_height as u8, self.mb_width as u8);
            out.extend_from_slice(&[rows, cols, rows, cols]);
        } else {
            out.push(0x80 | (q as u8) << 1 | separated);
            let offset = 3 + part1.len();
            out.extend_from_slice(&(offset as u16).to_be_bytes());
        }
        out.extend_from_slice(&part1);
        out.extend_from_slice(&part2);
        self.recon = Some(recon);
        out
    }

    fn slot_probs(&self) -> Vec<u8> {
        let m = &self.model;
        let mut p = vec![128u8; SLOTS];
        for pt in 0..2 {
            for node in 0..11 {
                p[dccv(pt, node) as usize] = m.dccv[pt][node];
            }
            for ctx in 0..3 {
                for node in 0..5 {
                    p[dcct(pt, ctx, node) as usize] = m.dcct[pt][ctx][node];
                }
            }
            for ct in 0..3 {
                for cg in 0..6 {
                    for node in 0..11 {
                        p[ract(pt, ct, cg, node) as usize] = m.ract[pt][ct][cg][node];
                    }
                }
            }
        }
        for cg in 0..2 {
            for node in 0..14 {
                p[runv(cg, node) as usize] = m.runv[cg][node];
            }
        }
        for prob in 0..256 {
            p[fixed(prob as u8) as usize] = prob as u8;
        }
        p
    }

    /// Writes the coefficient model updates that pay for themselves and
    /// applies them (and the key-frame defaults) to the model, as the decoder does.
    fn write_coeff_models(&mut self, c: &mut RangeEncoder, ev: &Events, key: bool) {
        const UPDATE_BITS: u64 = 7 * 256;
        let candidates: Vec<u8> = std::iter::once(1).chain((1..128).map(|v| v * 2)).collect();
        let choose = |cost: &dyn Fn(u8) -> u64, current: u8, pct: u8| -> Option<u8> {
            let keep = cost(current) + bit_cost(pct, false) as u64;
            let (best, best_cost) = candidates
                .iter()
                .map(|&p| (p, cost(p)))
                .min_by_key(|&(_, c)| c)
                .expect("candidates");
            let update = best_cost + bit_cost(pct, true) as u64 + UPDATE_BITS;
            (update < keep).then_some(best)
        };
        let m = &mut self.model;
        let mut def = [128u8; 11];
        for pt in 0..2 {
            for node in 0..11 {
                let current = if key { def[node] } else { m.dccv[pt][node] };
                let cost = |p: u8| {
                    let mut total = ev.cost(dccv(pt, node), p);
                    if node < 5 {
                        for ctx in 0..3 {
                            total += ev.cost(dcct(pt, ctx, node), dcct_prob(p, ctx, node));
                        }
                    }
                    total
                };
                match choose(&cost, current, DCCV_PCT[pt][node]) {
                    Some(p) => {
                        c.put_prob(DCCV_PCT[pt][node], true);
                        c.puts_nn(p);
                        def[node] = p;
                        m.dccv[pt][node] = p;
                    }
                    None => {
                        c.put_prob(DCCV_PCT[pt][node], false);
                        m.dccv[pt][node] = current;
                    }
                }
            }
        }
        c.put(false); // no coefficient reordering
        for cg in 0..2 {
            for node in 0..14 {
                let cost = |p: u8| ev.cost(runv(cg, node), p);
                match choose(&cost, m.runv[cg][node], RUNV_PCT[cg][node]) {
                    Some(p) => {
                        c.put_prob(RUNV_PCT[cg][node], true);
                        c.puts_nn(p);
                        m.runv[cg][node] = p;
                    }
                    None => c.put_prob(RUNV_PCT[cg][node], false),
                }
            }
        }
        for ct in 0..3 {
            for pt in 0..2 {
                for cg in 0..6 {
                    for node in 0..11 {
                        let current = if key {
                            def[node]
                        } else {
                            m.ract[pt][ct][cg][node]
                        };
                        let cost = |p: u8| ev.cost(ract(pt, ct, cg, node), p);
                        let pct = RACT_PCT[ct][pt][cg][node];
                        match choose(&cost, current, pct) {
                            Some(p) => {
                                c.put_prob(pct, true);
                                c.puts_nn(p);
                                def[node] = p;
                                m.ract[pt][ct][cg][node] = p;
                            }
                            None => {
                                c.put_prob(pct, false);
                                m.ract[pt][ct][cg][node] = current;
                            }
                        }
                    }
                }
            }
        }
        m.derive_dcct();
    }
}

fn mean_abs_diff(a: &[u8], b: &[u8]) -> u32 {
    let sum: u64 = a
        .iter()
        .zip(b)
        .map(|(&x, &y)| (x as i32 - y as i32).unsigned_abs() as u64)
        .sum();
    (sum / a.len().max(1) as u64) as u32
}
