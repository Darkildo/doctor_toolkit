//! Probability models of a VP6 stream and the per-frame state shared by the
//! decoder and the encoder (DC prediction, contexts).

use super::tables::*;

/// Adaptive probabilities. Reset on every key frame, updated by frame headers.
#[derive(Debug, Clone)]
pub struct Model {
    pub dccv: [[u8; 11]; 2],
    pub dcct: [[[u8; 5]; 3]; 2],
    /// `[pt][ct][cg][node]`.
    pub ract: [[[[u8; 11]; 6]; 3]; 2],
    pub runv: [[u8; 14]; 2],
    pub reorder: [u8; 64],
    pub index_to_pos: [u8; 64],
    pub idct_selector: [u8; 64],
    pub mb_types_stats: [[[u8; 2]; 10]; 3],
    pub mb_type: [[[u8; 10]; 10]; 3],
    /// Motion vector models per component (x, y): long-vector flag, sign,
    /// short-delta tree, long-delta bits.
    pub vector_dct: [u8; 2],
    pub vector_sig: [u8; 2],
    pub vector_pdv: [[u8; 7]; 2],
    pub vector_fdv: [[u8; 8]; 2],
}

impl Default for Model {
    fn default() -> Self {
        let mut m = Self {
            dccv: [[128; 11]; 2],
            dcct: [[[128; 5]; 3]; 2],
            ract: [[[[128; 11]; 6]; 3]; 2],
            runv: DEF_RUNV,
            reorder: DEF_COEFF_REORDER,
            index_to_pos: [0; 64],
            idct_selector: [0; 64],
            mb_types_stats: DEF_MB_TYPES_STATS,
            mb_type: [[[0; 10]; 10]; 3],
            vector_dct: DEF_VECTOR_DCT,
            vector_sig: DEF_VECTOR_SIG,
            vector_pdv: DEF_PDV,
            vector_fdv: DEF_FDV,
        };
        m.order_init(7);
        m
    }
}

impl Model {
    /// The defaults set at a key frame.
    pub fn reset(&mut self, sub_version: u8) {
        self.mb_types_stats = DEF_MB_TYPES_STATS;
        self.runv = DEF_RUNV;
        self.reorder = DEF_COEFF_REORDER;
        self.vector_dct = DEF_VECTOR_DCT;
        self.vector_sig = DEF_VECTOR_SIG;
        self.vector_pdv = DEF_PDV;
        self.vector_fdv = DEF_FDV;
        self.order_init(sub_version);
    }

    pub fn order_init(&mut self, sub_version: u8) {
        self.index_to_pos[0] = 0;
        let mut idx = 1;
        for i in 0..16 {
            for pos in 1..64 {
                if self.reorder[pos] == i {
                    self.index_to_pos[idx] = pos as u8;
                    idx += 1;
                }
            }
        }
        let mut max = 0u8;
        for idx in 0..64 {
            max = max.max(self.index_to_pos[idx]);
            self.idct_selector[idx] = max + (sub_version > 6) as u8;
        }
    }

    /// Recomputes the DC context models from `dccv`.
    pub fn derive_dcct(&mut self) {
        for pt in 0..2 {
            for ctx in 0..3 {
                for node in 0..5 {
                    self.dcct[pt][ctx][node] = dcct_prob(self.dccv[pt][node], ctx, node);
                }
            }
        }
    }

    /// Macroblock type probabilities from `mb_types_stats`.
    pub fn derive_mb_types(&mut self) {
        for ctx in 0..3 {
            let stats = &self.mb_types_stats[ctx];
            let mut p = [0i32; 10];
            for t in 0..10 {
                p[t] = 100 * stats[t][1] as i32;
            }
            for t in 0..10 {
                let s0 = stats[t][0] as i32;
                let s1 = stats[t][1] as i32;
                let out = &mut self.mb_type[ctx][t];
                out[0] = (255 - (255 * s0) / (1 + s0 + s1)) as u8;
                p[t] = 0;
                let p02 = p[0] + p[2];
                let p34 = p[3] + p[4];
                let p0234 = p02 + p34;
                let p17 = p[1] + p[7];
                let p56 = p[5] + p[6];
                let p89 = p[8] + p[9];
                let p5689 = p56 + p89;
                let p156789 = p17 + p5689;
                out[1] = (1 + 255 * p0234 / (1 + p0234 + p156789)) as u8;
                out[2] = (1 + 255 * p02 / (1 + p0234)) as u8;
                out[3] = (1 + 255 * p17 / (1 + p156789)) as u8;
                out[4] = (1 + 255 * p[0] / (1 + p02)) as u8;
                out[5] = (1 + 255 * p[3] / (1 + p34)) as u8;
                out[6] = (1 + 255 * p[1] / (1 + p17)) as u8;
                out[7] = (1 + 255 * p56 / (1 + p5689)) as u8;
                out[8] = (1 + 255 * p[5] / (1 + p56)) as u8;
                out[9] = (1 + 255 * p[8] / (1 + p89)) as u8;
                p[t] = 100 * s1;
            }
        }
    }
}

pub fn dcct_prob(dccv: u8, ctx: usize, node: usize) -> u8 {
    let [a, b] = DCCV_LC[ctx][node];
    (((dccv as i32 * a + 128) >> 8) + b).clamp(1, 255) as u8
}

const DEF_VECTOR_DCT: [u8; 2] = [0xA2, 0xA4];
const DEF_VECTOR_SIG: [u8; 2] = [0x80, 0x80];

/// Reference frame of a DC neighbour or a macroblock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ref {
    None,
    Current,
    Previous,
    Golden,
}

#[derive(Debug, Clone, Copy)]
pub struct RefDc {
    pub ref_frame: Ref,
    pub dc: i16,
    pub not_null_dc: bool,
}

const EMPTY: RefDc = RefDc {
    ref_frame: Ref::None,
    dc: 0,
    not_null_dc: false,
};

/// DC prediction and coefficient-context state of one frame, walked in
/// macroblock order exactly as the decoder does.
pub struct DcState {
    mb_width: usize,
    above: Vec<RefDc>,
    left: [RefDc; 4],
    above_idx: [usize; 6],
    /// `[plane][ref]` with ref 0 current, 1 previous, 2 golden.
    prev_dc: [[i16; 3]; 3],
}

impl DcState {
    pub fn new(mb_width: usize) -> Self {
        let mut above = vec![EMPTY; 4 * mb_width + 6];
        above[2 * mb_width + 2].ref_frame = Ref::Current;
        above[3 * mb_width + 4].ref_frame = Ref::Current;
        Self {
            mb_width,
            above,
            left: [EMPTY; 4],
            above_idx: [0; 6],
            prev_dc: [[0, 0, 0], [128, 0, 0], [128, 0, 0]],
        }
    }

    pub fn start_row(&mut self) {
        self.left = [EMPTY; 4];
        let w = self.mb_width;
        self.above_idx = [1, 2, 1, 2, 2 * w + 3, 3 * w + 5];
    }

    pub fn next_mb(&mut self) {
        for i in 0..4 {
            self.above_idx[i] += 2;
        }
        self.above_idx[4] += 1;
        self.above_idx[5] += 1;
    }

    /// Context (0..2) for the DC token of block `b`.
    pub fn dc_ctx(&self, b: usize) -> usize {
        self.left[B6TO4[b]].not_null_dc as usize
            + self.above[self.above_idx[b]].not_null_dc as usize
    }

    pub fn set_not_null(&mut self, b: usize, not_null: bool) {
        self.left[B6TO4[b]].not_null_dc = not_null;
        let i = self.above_idx[b];
        self.above[i].not_null_dc = not_null;
    }

    /// Predicted DC of block `b` for a macroblock referencing `r`.
    pub fn predict(&self, b: usize, r: Ref) -> i16 {
        let ab = &self.above[self.above_idx[b]];
        let lb = &self.left[B6TO4[b]];
        let mut count = 0;
        let mut dc = 0i32;
        if lb.ref_frame == r {
            dc += lb.dc as i32;
            count += 1;
        }
        if ab.ref_frame == r {
            dc += ab.dc as i32;
            count += 1;
        }
        match count {
            0 => self.prev_dc[B2P[b]][ref_index(r)],
            2 => (dc / 2) as i16,
            _ => dc as i16,
        }
    }

    /// Records the reconstructed (quantized) DC of block `b`.
    pub fn store(&mut self, b: usize, r: Ref, dc: i16) {
        self.prev_dc[B2P[b]][ref_index(r)] = dc;
        let i = self.above_idx[b];
        self.above[i].dc = dc;
        self.above[i].ref_frame = r;
        let l = &mut self.left[B6TO4[b]];
        l.dc = dc;
        l.ref_frame = r;
    }
}

fn ref_index(r: Ref) -> usize {
    match r {
        Ref::Previous => 1,
        Ref::Golden => 2,
        _ => 0,
    }
}
