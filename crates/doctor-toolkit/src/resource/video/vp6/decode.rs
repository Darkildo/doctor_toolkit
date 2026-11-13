//! The VP6 decoder: key frames and inter frames with motion vectors (one per
//! macroblock or one per luma block), the golden frame, bilinear and bicubic
//! interpolation, the loop filter of streams with a filter header, and both
//! coefficient codings (range coder and Huffman). It follows the reference
//! decoder (FFmpeg `vp56.c`, `vp6.c`) bit for bit, including how it conceals
//! damaged macroblocks.
//!
//! Pictures are in coded orientation: `vp60` streams store them bottom-up,
//! flip with [`Picture::flipped`] (or let the RGB conversion do it).

use super::dct;
use super::huffman::{self, Bits, Table};
use super::mc::{self, Filters, Mv};
use super::model::{DcState, Model, Ref};
use super::rac::RangeDecoder;
use super::tables::*;
use super::{FrameHeader, Picture};
use crate::{Error, Result};

/// Macroblock types, in the bitstream's numbering.
const MB_INTER_NOVEC_PF: u8 = 0;
const MB_INTRA: u8 = 1;
const MB_INTER_DELTA_PF: u8 = 2;
const MB_INTER_V1_PF: u8 = 3;
const MB_INTER_V2_PF: u8 = 4;
const MB_INTER_NOVEC_GF: u8 = 5;
const MB_INTER_DELTA_GF: u8 = 6;
const MB_INTER_4V: u8 = 7;
const MB_INTER_V1_GF: u8 = 8;
const MB_INTER_V2_GF: u8 = 9;

/// Reference picture of each macroblock type.
const REFERENCE: [Ref; 10] = [
    Ref::Previous,
    Ref::Current,
    Ref::Previous,
    Ref::Previous,
    Ref::Previous,
    Ref::Golden,
    Ref::Golden,
    Ref::Previous,
    Ref::Golden,
    Ref::Golden,
];

/// Coefficient position of each zigzag index, transposed for the IDCT.
const PERMUTE: [usize; 64] = {
    let mut p = [0usize; 64];
    let mut i = 0;
    while i < 64 {
        let z = ZIGZAG[i] as usize;
        p[i] = (z >> 3) | (z & 7) << 3;
        i += 1;
    }
    p
};

/// What the last frame used (for statistics and tests).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameStats {
    pub key: bool,
    /// Coefficients coded with Huffman codes (else the range coder).
    pub huffman: bool,
    /// The frame became the golden frame.
    pub golden: bool,
    /// Loop filter on (inter frames of streams with a filter header).
    pub deblock: bool,
    /// Interpolation filter mode (0 bilinear, 1 bicubic, 2 adaptive).
    pub filter_mode: u8,
    /// Macroblocks of each type (bitstream numbering: 0 no vector from the
    /// previous frame, 1 intra, 2 delta, 3/4 first/second neighbour vector,
    /// 5–6 and 8–9 the same from the golden frame, 7 four vectors).
    pub mb_types: [u32; 10],
}

#[derive(Clone, Copy, Default)]
struct Macroblock {
    kind: u8,
    mv: Mv,
}

/// Header fields that persist between frames.
#[derive(Clone, Copy)]
struct StreamState {
    sub_version: u8,
    filter_header: u8,
    deblock_filtering: bool,
    filter_mode: u8,
    sample_variance_threshold: i32,
    max_vector_length: i32,
    filter_selection: usize,
}

/// Huffman tables of the current frame.
struct HuffTables {
    dccv: [Table; 2],
    runv: [Table; 2],
    /// `[pt][ct][cg]` flattened.
    ract: Vec<Table>,
}

/// Decoder state across frames.
pub struct Decoder {
    model: Model,
    stream: StreamState,
    mb_width: usize,
    mb_height: usize,
    /// The last decoded picture (the reference of `Ref::Previous`).
    previous: Option<Picture>,
    golden: Option<Picture>,
    /// The picture being decoded; recycled.
    current: Option<Picture>,
    macroblocks: Vec<Macroblock>,
    /// Coefficients; kept between macroblocks as the reference decoder does.
    blocks: [[i16; 64]; 6],
    selector: [u8; 6],
    mv: [Mv; 6],
    candidates: [Mv; 2],
    candidate_pos: usize,
    mb_type: u8,
    nb_null: [[u32; 2]; 2],
    dequant_ac: i32,
    have_undamaged_frame: bool,
    damaged: bool,
    stats: FrameStats,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-frame values the macroblock loop needs.
struct Frame {
    key: bool,
    dequant_dc: i32,
    dequant_ac: i32,
    filters: Filters,
}

/// Where the coefficients come from.
enum Coeffs<'a> {
    /// The first partition's range coder.
    Shared,
    Range(RangeDecoder<'a>),
    Huffman(Bits<'a>, Box<HuffTables>),
}

impl Decoder {
    pub fn new() -> Self {
        Self {
            // The DC context models start at zero, as in the reference
            // decoder (they matter only for the quirk below).
            model: Model {
                dcct: [[[0; 5]; 3]; 2],
                ..Model::default()
            },
            stream: StreamState {
                sub_version: 0,
                filter_header: 0,
                deblock_filtering: false,
                filter_mode: 0,
                sample_variance_threshold: 0,
                max_vector_length: 0,
                filter_selection: 0,
            },
            mb_width: 0,
            mb_height: 0,
            previous: None,
            golden: None,
            current: None,
            macroblocks: Vec::new(),
            blocks: [[0; 64]; 6],
            selector: [0; 6],
            mv: [Mv::default(); 6],
            candidates: [Mv::default(); 2],
            candidate_pos: 0,
            mb_type: MB_INTER_NOVEC_PF,
            nb_null: [[0; 2]; 2],
            dequant_ac: 0,
            have_undamaged_frame: false,
            damaged: false,
            stats: FrameStats::default(),
        }
    }

    /// The last decoded picture (also after a frame whose damaged macroblocks
    /// were concealed).
    pub fn picture(&self) -> Option<&Picture> {
        self.previous.as_ref()
    }

    /// Whether the last frame had damaged macroblocks (concealed by copying
    /// the previous picture).
    pub fn last_damaged(&self) -> bool {
        self.damaged
    }

    /// What the last decoded frame used.
    pub fn stats(&self) -> &FrameStats {
        &self.stats
    }

    /// Forgets the reference pictures: the next frame must be a key frame.
    pub fn reset(&mut self) {
        self.previous = None;
        self.golden = None;
        self.have_undamaged_frame = false;
    }

    /// Decodes one frame and returns the coded picture (bottom-up for `vp60`
    /// streams: flip it with [`Picture::flipped`]).
    ///
    /// A frame whose macroblock data is damaged is an error; if an earlier
    /// frame decoded cleanly, the damaged part is concealed (as the reference
    /// decoder does) and decoding continues with the next frame from the
    /// concealed picture ([`Self::picture`]).
    pub fn decode(&mut self, data: &[u8]) -> Result<&Picture> {
        self.damaged = false;
        match self.decode_frame(data) {
            Ok(()) => Ok(self.previous.as_ref().expect("decoded picture")),
            Err(e) => Err(e),
        }
    }

    fn decode_frame(&mut self, data: &[u8]) -> Result<()> {
        let h = FrameHeader::parse_in(data, self.stream.filter_header)?;
        let mut stream = self.stream;
        if h.key {
            if h.sub_version > 8 {
                return Err(unsupported("VP6 sub-version above 8"));
            }
            if h.interlaced {
                return Err(unsupported("interlaced VP6"));
            }
        } else if stream.sub_version == 0 || self.previous.is_none() || self.golden.is_none() {
            return Err(Error::malformed(
                "VP6",
                0,
                "inter frame without a decoded key frame",
            ));
        }

        let mut c = RangeDecoder::new(&data[h.rac_start..]);
        let mut golden_frame = false;
        let mut parse_filter_info = false;
        let mut vrt_shift = 0;
        if h.key {
            c.gets(2);
            stream.sub_version = h.sub_version;
            stream.filter_header = h.filter_header;
            parse_filter_info = h.filter_header != 0;
            if h.sub_version < 8 {
                vrt_shift = 5;
            }
        } else {
            golden_frame = c.get();
            if stream.filter_header != 0 {
                stream.deblock_filtering = c.get();
                if stream.deblock_filtering {
                    c.get();
                }
                if stream.sub_version > 7 {
                    parse_filter_info = c.get();
                }
            }
        }
        if parse_filter_info {
            if c.get() {
                stream.filter_mode = 2;
                stream.sample_variance_threshold = (c.gets(5) as i32) << vrt_shift;
                stream.max_vector_length = 2 << c.gets(3);
            } else if c.get() {
                stream.filter_mode = 1;
            } else {
                stream.filter_mode = 0;
            }
            stream.filter_selection = if stream.sub_version > 7 {
                c.gets(4) as usize
            } else {
                16
            };
        }
        let use_huffman = c.get();
        let coeff_data = match h.coeff_offset {
            Some(off) if off < data.len() => Some(&data[off..]),
            Some(_) => return Err(Error::malformed("VP6", 1, "coefficient offset")),
            None => None,
        };

        if h.key {
            let (cols, rows) = (h.mb_cols as usize, h.mb_rows as usize);
            if cols != self.mb_width || rows != self.mb_height {
                self.mb_width = cols;
                self.mb_height = rows;
                self.previous = None;
                self.golden = None;
                self.current = None;
                self.have_undamaged_frame = false;
                self.macroblocks = vec![Macroblock::default(); cols * rows];
            }
        }
        self.stream = stream;

        let quant = h.quantizer as usize;
        let frame = Frame {
            key: h.key,
            dequant_dc: (DC_DEQUANT[quant] as i32) << 2,
            dequant_ac: (AC_DEQUANT[quant] as i32) << 2,
            filters: Filters {
                mode: stream.filter_mode,
                sample_variance_threshold: stream.sample_variance_threshold,
                max_vector_length: stream.max_vector_length,
                selection: stream.filter_selection,
                deblock: (stream.deblock_filtering && !h.key)
                    .then_some(FILTER_THRESHOLD[quant] as i32),
            },
        };

        self.dequant_ac = frame.dequant_ac;
        // Models.
        if h.key {
            self.model.reset(stream.sub_version);
            for mb in &mut self.macroblocks {
                mb.kind = MB_INTRA;
            }
        } else {
            self.parse_mb_type_models(&mut c);
            self.parse_vector_models(&mut c);
            self.mb_type = MB_INTER_NOVEC_PF;
        }
        self.parse_coeff_models(&mut c, h.key, use_huffman);
        let mut coeffs = match (coeff_data, use_huffman) {
            (Some(d), true) => {
                self.nb_null = [[0; 2]; 2];
                Coeffs::Huffman(Bits::new(d), Box::new(self.huffman_tables()))
            }
            (Some(d), false) => Coeffs::Range(RangeDecoder::new(d)),
            // Huffman codes need their own partition: read as range-coded, as
            // the reference decoder does.
            (None, _) => Coeffs::Shared,
        };

        let (w, hgt) = (self.mb_width * 16, self.mb_height * 16);
        let mut cur = match self.current.take() {
            Some(p) if p.width == w && p.height == hgt => p,
            _ => Picture::new(w, hgt),
        };

        let mut dcs = DcState::new(self.mb_width);
        let mut damaged = false;
        let mut mb_types = [0u32; 10];
        for row in 0..self.mb_height {
            dcs.start_row();
            for col in 0..self.mb_width {
                let kind = if damaged {
                    self.conceal_mv(row, col, h.key)
                } else {
                    let kind = if h.key {
                        MB_INTRA
                    } else {
                        self.decode_mv(&mut c, row, col)
                    };
                    let parsed = match &mut coeffs {
                        Coeffs::Shared => self.parse_coeff(&mut c, &mut dcs),
                        Coeffs::Range(cc) => self.parse_coeff(cc, &mut dcs),
                        Coeffs::Huffman(bits, tables) => self.parse_coeff_huffman(bits, tables),
                    };
                    if let Err(e) = parsed {
                        if !self.have_undamaged_frame {
                            // Nothing to conceal from: drop the frame.
                            self.current = Some(cur);
                            return Err(e);
                        }
                        damaged = true;
                        if h.key {
                            MB_INTRA
                        } else {
                            self.conceal_mv(row, col, false)
                        }
                    } else {
                        kind
                    }
                };
                mb_types[kind as usize % 10] += 1;
                self.render_mb(&mut cur, &mut dcs, &frame, row, col, kind);
                dcs.next_mb();
            }
        }
        if !damaged {
            self.have_undamaged_frame = true;
        }
        self.damaged = damaged;
        self.stats = FrameStats {
            key: h.key,
            huffman: matches!(coeffs, Coeffs::Huffman(..)),
            golden: h.key || golden_frame,
            deblock: frame.filters.deblock.is_some(),
            filter_mode: frame.filters.mode,
            mb_types,
        };

        if h.key || golden_frame {
            match &mut self.golden {
                Some(g) if g.width == cur.width && g.height == cur.height => {
                    for p in 0..3 {
                        g.planes[p].copy_from_slice(&cur.planes[p]);
                    }
                }
                g => *g = Some(cur.clone()),
            }
        }
        self.current = self.previous.replace(cur);
        if damaged {
            return Err(Error::malformed(
                "VP6",
                0,
                "damaged macroblocks (concealed)",
            ));
        }
        Ok(())
    }

    fn huffman_tables(&self) -> HuffTables {
        let m = &self.model;
        let mut ract = Vec::with_capacity(36);
        for pt in 0..2 {
            for ct in 0..3 {
                for cg in 0..6 {
                    ract.push(Table::build(&m.ract[pt][ct][cg], &HUFF_COEFF_MAP, 12));
                }
            }
        }
        HuffTables {
            dccv: [0, 1].map(|pt| Table::build(&m.dccv[pt], &HUFF_COEFF_MAP, 12)),
            runv: [0, 1].map(|pt| Table::build(&m.runv[pt], &HUFF_RUN_MAP, 9)),
            ract,
        }
    }

    fn parse_mb_type_models(&mut self, c: &mut RangeDecoder<'_>) {
        for ctx in 0..3 {
            if c.get_prob(174) {
                let idx = c.gets(4) as usize;
                self.model.mb_types_stats[ctx] = PRE_DEF_MB_TYPE_STATS[idx][ctx];
            }
            if c.get_prob(254) {
                for t in 0..10 {
                    for i in 0..2 {
                        if c.get_prob(205) {
                            let sign = c.get();
                            let mut delta = c.tree(&PMBTM_TREE, &MB_TYPE_MODEL_MODEL) as i32;
                            if delta == 0 {
                                delta = 4 * c.gets(7) as i32;
                            }
                            let s = &mut self.model.mb_types_stats[ctx][t][i];
                            let d = if sign { -delta } else { delta };
                            *s = (*s as i32).wrapping_add(d) as u8;
                        }
                    }
                }
            }
        }
        self.model.derive_mb_types();
    }

    fn parse_vector_models(&mut self, c: &mut RangeDecoder<'_>) {
        let m = &mut self.model;
        for comp in 0..2 {
            if c.get_prob(SIG_DCT_PCT[comp][0]) {
                m.vector_dct[comp] = c.gets_nn();
            }
            if c.get_prob(SIG_DCT_PCT[comp][1]) {
                m.vector_sig[comp] = c.gets_nn();
            }
        }
        for comp in 0..2 {
            for node in 0..7 {
                if c.get_prob(PDV_PCT[comp][node]) {
                    m.vector_pdv[comp][node] = c.gets_nn();
                }
            }
        }
        for comp in 0..2 {
            for node in 0..8 {
                if c.get_prob(FDV_PCT[comp][node]) {
                    m.vector_fdv[comp][node] = c.gets_nn();
                }
            }
        }
    }

    /// Coefficient model updates. With Huffman coding the DC context models
    /// are not derived: a frame that asks for Huffman codes without a second
    /// partition is range-coded with the old ones (as the reference does).
    fn parse_coeff_models(&mut self, c: &mut RangeDecoder<'_>, key: bool, use_huffman: bool) {
        let sub_version = self.stream.sub_version;
        let m = &mut self.model;
        let mut def = [128u8; 11];
        for pt in 0..2 {
            for node in 0..11 {
                if c.get_prob(DCCV_PCT[pt][node]) {
                    def[node] = c.gets_nn();
                    m.dccv[pt][node] = def[node];
                } else if key {
                    m.dccv[pt][node] = def[node];
                }
            }
        }
        if c.get() {
            for pos in 1..64 {
                if c.get_prob(COEFF_REORDER_PCT[pos]) {
                    m.reorder[pos] = c.gets(4) as u8;
                }
            }
            m.order_init(sub_version);
        }
        for cg in 0..2 {
            for node in 0..14 {
                if c.get_prob(RUNV_PCT[cg][node]) {
                    m.runv[cg][node] = c.gets_nn();
                }
            }
        }
        for ct in 0..3 {
            for pt in 0..2 {
                for cg in 0..6 {
                    for node in 0..11 {
                        if c.get_prob(RACT_PCT[ct][pt][cg][node]) {
                            def[node] = c.gets_nn();
                            m.ract[pt][ct][cg][node] = def[node];
                        } else if key {
                            m.ract[pt][ct][cg][node] = def[node];
                        }
                    }
                }
            }
        }
        if !use_huffman {
            m.derive_dcct();
        }
    }

    /// Motion vector candidates from decoded neighbours referencing `r`:
    /// returns the context of the macroblock type (0: two, 1: none, 2: one).
    fn vector_predictors(&mut self, row: usize, col: usize, r: Ref) -> usize {
        let mut count = 0usize;
        let mut vect = [Mv::default(); 2];
        let mut two = false;
        for (pos, d) in CANDIDATE_PREDICTOR_POS.iter().enumerate() {
            let x = col as isize + d[0] as isize;
            let y = row as isize + d[1] as isize;
            if x < 0 || y < 0 || x >= self.mb_width as isize || y >= self.mb_height as isize {
                continue;
            }
            let mb = self.macroblocks[y as usize * self.mb_width + x as usize];
            if REFERENCE[mb.kind as usize] != r {
                continue;
            }
            if mb.mv == vect[0] || mb.mv == Mv::default() {
                continue;
            }
            vect[count] = mb.mv;
            count += 1;
            if count > 1 {
                two = true;
                break;
            }
            self.candidate_pos = pos;
        }
        self.candidates = vect;
        if two { 0 } else { count + 1 }
    }

    fn vector_adjustment(&self, c: &mut RangeDecoder<'_>) -> Mv {
        let m = &self.model;
        let mut v = if self.candidate_pos < 2 {
            self.candidates[0]
        } else {
            Mv::default()
        };
        for comp in 0..2 {
            let mut delta: i32;
            if c.get_prob(m.vector_dct[comp]) {
                delta = 0;
                for j in [0usize, 1, 2, 7, 6, 5, 4] {
                    delta |= (c.get_prob(m.vector_fdv[comp][j]) as i32) << j;
                }
                if delta & 0xF0 != 0 {
                    delta |= (c.get_prob(m.vector_fdv[comp][3]) as i32) << 3;
                } else {
                    delta |= 8;
                }
            } else {
                delta = c.tree(&PVA_TREE, &m.vector_pdv[comp]) as i32;
            }
            if delta != 0 && c.get_prob(m.vector_sig[comp]) {
                delta = -delta;
            }
            if comp == 0 {
                v.x = v.x.wrapping_add(delta as i16);
            } else {
                v.y = v.y.wrapping_add(delta as i16);
            }
        }
        v
    }

    fn decode_mv(&mut self, c: &mut RangeDecoder<'_>, row: usize, col: usize) -> u8 {
        let ctx = self.vector_predictors(row, col, Ref::Previous);
        let probs = &self.model.mb_type[ctx][self.mb_type as usize];
        let kind = if c.get_prob(probs[0]) {
            self.mb_type
        } else {
            c.tree(&PMBT_TREE, probs)
        };
        self.mb_type = kind;
        let index = row * self.mb_width + col;
        self.macroblocks[index].kind = kind;
        let mv = match kind {
            MB_INTER_V1_PF => self.candidates[0],
            MB_INTER_V2_PF => self.candidates[1],
            MB_INTER_V1_GF => {
                self.vector_predictors(row, col, Ref::Golden);
                self.candidates[0]
            }
            MB_INTER_V2_GF => {
                self.vector_predictors(row, col, Ref::Golden);
                self.candidates[1]
            }
            MB_INTER_DELTA_PF => self.vector_adjustment(c),
            MB_INTER_DELTA_GF => {
                self.vector_predictors(row, col, Ref::Golden);
                self.vector_adjustment(c)
            }
            MB_INTER_4V => {
                self.decode_4mv(c, index);
                return kind;
            }
            _ => Mv::default(),
        };
        self.macroblocks[index].mv = mv;
        self.mv = [mv; 6];
        kind
    }

    fn decode_4mv(&mut self, c: &mut RangeDecoder<'_>, index: usize) {
        let mut types = [0u8; 4];
        for t in &mut types {
            *t = c.gets(2) as u8;
            if *t != 0 {
                *t += 1;
            }
        }
        let (mut sx, mut sy) = (0i32, 0i32);
        for b in 0..4 {
            self.mv[b] = match types[b] {
                MB_INTER_DELTA_PF => self.vector_adjustment(c),
                MB_INTER_V1_PF => self.candidates[0],
                MB_INTER_V2_PF => self.candidates[1],
                _ => Mv::default(),
            };
            sx += self.mv[b].x as i32;
            sy += self.mv[b].y as i32;
        }
        self.macroblocks[index].mv = self.mv[3];
        let round = |a: i32| if a > 0 { (a + 2) >> 2 } else { (a + 1) >> 2 };
        let chroma = Mv {
            x: round(sx) as i16,
            y: round(sy) as i16,
        };
        self.mv[4] = chroma;
        self.mv[5] = chroma;
    }

    fn conceal_mv(&mut self, row: usize, col: usize, key: bool) -> u8 {
        if key {
            return MB_INTRA;
        }
        self.mb_type = MB_INTER_NOVEC_PF;
        let mb = &mut self.macroblocks[row * self.mb_width + col];
        mb.kind = MB_INTER_NOVEC_PF;
        mb.mv = Mv::default();
        self.mv = [Mv::default(); 6];
        MB_INTER_NOVEC_PF
    }

    fn parse_coeff(&mut self, c: &mut RangeDecoder<'_>, dcs: &mut DcState) -> Result<()> {
        if c.is_end() {
            return Err(Error::malformed("VP6", 0, "end of coefficient data"));
        }
        let model = &self.model;
        for b in 0..6 {
            let pt = (b > 3) as usize;
            let mut ct = 1usize;
            let ctx = dcs.dc_ctx(b);
            let mut model1: &[u8; 11] = &model.dccv[pt];
            let mut model2: &[u8] = &model.dcct[pt][ctx];
            let mut idx = 0usize;
            let block = &mut self.blocks[b];
            loop {
                let mut run = 1usize;
                if (idx > 1 && ct == 0) || c.get_prob(model2[0]) {
                    let coeff: i32 = if c.get_prob(model2[2]) {
                        let v = if c.get_prob(model2[3]) {
                            let cat = c.tree(&PC_TREE, model1) as usize;
                            let mut v = COEFF_BIAS[cat + 5] as i32;
                            for i in (0..=COEFF_BIT_LENGTH[cat] as usize).rev() {
                                v += (c.get_prob(COEFF_PARSE_TABLE[cat][i]) as i32) << i;
                            }
                            v
                        } else if c.get_prob(model2[4]) {
                            3 + c.get_prob(model1[5]) as i32
                        } else {
                            2
                        };
                        ct = 2;
                        v
                    } else {
                        ct = 1;
                        1
                    };
                    let mut v = if c.get() { -coeff } else { coeff };
                    if idx > 0 {
                        v *= self.dequant_ac;
                    }
                    block[PERMUTE[model.index_to_pos[idx] as usize]] = v as i16;
                } else {
                    ct = 0;
                    if idx > 0 {
                        if !c.get_prob(model2[1]) {
                            break;
                        }
                        let model3 = &model.runv[(idx >= 6) as usize];
                        run = c.tree(&PCR_TREE, model3) as usize;
                        if run == 0 {
                            run = 9;
                            for i in 0..6 {
                                run += (c.get_prob(model3[i + 8]) as usize) << i;
                            }
                        }
                    }
                }
                idx += run;
                if idx >= 64 {
                    break;
                }
                let cg = COEFF_GROUPS[idx] as usize;
                model1 = &model.ract[pt][ct][cg];
                model2 = model1;
            }
            dcs.set_not_null(b, block[0] != 0);
            self.selector[b] = model.idct_selector[idx.min(63)];
        }
        Ok(())
    }

    fn parse_coeff_huffman(&mut self, bits: &mut Bits<'_>, t: &HuffTables) -> Result<()> {
        let model = &self.model;
        for b in 0..6 {
            let pt = (b > 3) as usize;
            let mut ct = 0usize;
            let mut idx = 0usize;
            let mut table = &t.dccv[pt];
            let block = &mut self.blocks[b];
            loop {
                let mut run = 1usize;
                if idx < 2 && self.nb_null[idx][pt] > 0 {
                    self.nb_null[idx][pt] -= 1;
                    if idx > 0 {
                        break;
                    }
                } else {
                    if bits.left() <= 0 {
                        return Err(huffman::exhausted(bits));
                    }
                    let coeff = table.read(bits);
                    if coeff == 0 {
                        if idx > 0 {
                            run += t.runv[(idx >= 6) as usize].read(bits) as usize;
                            if run >= 9 {
                                run += bits.bits(6) as usize;
                            }
                        } else {
                            self.nb_null[0][pt] = nb_null(bits);
                        }
                        ct = 0;
                    } else if coeff == 11 {
                        if idx == 1 {
                            self.nb_null[1][pt] = nb_null(bits);
                        }
                        break;
                    } else {
                        let coeff = coeff as usize;
                        let mut v = COEFF_BIAS[coeff.min(10)] as i32;
                        if coeff > 4 {
                            v += bits.bits(if coeff <= 9 { coeff as u32 - 4 } else { 11 }) as i32;
                        }
                        ct = 1 + (v > 1) as usize;
                        if bits.bit() != 0 {
                            v = -v;
                        }
                        if idx > 0 {
                            v *= self.dequant_ac;
                        }
                        block[PERMUTE[model.index_to_pos[idx] as usize]] = v as i16;
                    }
                }
                idx += run;
                if idx >= 64 {
                    break;
                }
                let cg = (COEFF_GROUPS[idx] as usize).min(3);
                table = &t.ract[pt * 18 + ct * 6 + cg];
            }
            self.selector[b] = model.idct_selector[idx.min(63)];
        }
        Ok(())
    }

    fn render_mb(
        &mut self,
        cur: &mut Picture,
        dcs: &mut DcState,
        frame: &Frame,
        row: usize,
        col: usize,
        kind: u8,
    ) {
        let r = REFERENCE[kind as usize];
        for b in 0..6 {
            let pred = dcs.predict(b, r);
            let dc = self.blocks[b][0].wrapping_add(pred);
            dcs.store(b, r, dc);
            self.blocks[b][0] = (dc as i32).wrapping_mul(frame.dequant_dc) as i16;
        }
        let reference = match r {
            Ref::Golden => self.golden.as_ref(),
            _ => self.previous.as_ref(),
        };
        for b in 0..6 {
            let (plane, offset, stride) = cur.block_at(row, col, b);
            let block = &mut self.blocks[b];
            let selector = self.selector[b];
            if kind == MB_INTRA || frame.key {
                if selector > 10 || selector == 1 {
                    dct::idct(&mut cur.planes[plane], offset, stride, block, false);
                } else {
                    dct::idct10(&mut cur.planes[plane], offset, stride, block, false);
                }
                *block = [0; 64];
                continue;
            }
            let Some(src) = reference else {
                // Unreachable: inter frames need decoded references.
                continue;
            };
            let dst = &mut cur.planes[plane];
            if kind == MB_INTER_NOVEC_PF || kind == MB_INTER_NOVEC_GF {
                let s = &src.planes[plane];
                for y in 0..8 {
                    let o = offset + y * stride;
                    dst[o..o + 8].copy_from_slice(&s[o..o + 8]);
                }
            } else {
                let (pw, ph) = if plane == 0 {
                    (cur.width, cur.height)
                } else {
                    (cur.width / 2, cur.height / 2)
                };
                let (bx, by) = if b < 4 {
                    (col * 16 + (b & 1) * 8, row * 16 + (b >> 1) * 8)
                } else {
                    (col * 8, row * 8)
                };
                mc::predict(
                    dst,
                    offset,
                    stride,
                    &src.planes[plane],
                    pw,
                    ph,
                    bx,
                    by,
                    self.mv[b],
                    b < 4,
                    &frame.filters,
                );
            }
            if selector > 10 {
                dct::idct(dst, offset, stride, block, true);
                *block = [0; 64];
            } else if selector > 1 {
                dct::idct10(dst, offset, stride, block, true);
                *block = [0; 64];
            } else {
                dct::idct_dc_add(dst, offset, stride, block);
                block[0] = 0;
            }
        }
    }
}

/// Runs of blocks with a zero DC or first AC coefficient (Huffman coding).
fn nb_null(bits: &mut Bits<'_>) -> u32 {
    let mut v = bits.bits(2);
    if v == 2 {
        v += bits.bits(2);
    } else if v == 3 {
        let e = bits.bit() << 2;
        v = 6 + e + bits.bits(2 + e);
    }
    v
}

fn unsupported(what: &str) -> Error {
    Error::InvalidArgument(format!("VP6 decoder: {what} not supported"))
}
