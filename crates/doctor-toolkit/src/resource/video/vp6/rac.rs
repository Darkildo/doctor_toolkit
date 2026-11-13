//! The VP5/VP6/VP8 boolean range coder: decoder and encoder.

use super::tables::Tree;

/// Range decoder over one partition.
pub struct RangeDecoder<'a> {
    high: u32,
    bits: i32,
    code_word: u32,
    data: &'a [u8],
    pos: usize,
    end_reached: u32,
}

impl<'a> RangeDecoder<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        let byte = |i: usize| data.get(i).copied().unwrap_or(0) as u32;
        Self {
            high: 255,
            bits: -16,
            code_word: byte(0) << 16 | byte(1) << 8 | byte(2),
            data,
            pos: 3,
            end_reached: 0,
        }
    }

    /// True once the decoder has read well past the end of its data.
    pub fn is_end(&mut self) -> bool {
        if self.pos >= self.data.len() && self.bits >= 0 {
            self.end_reached += 1;
        }
        self.end_reached > 10
    }

    fn renorm(&mut self) -> u32 {
        let shift = self.high.leading_zeros().saturating_sub(24);
        self.high <<= shift;
        let mut code_word = self.code_word << shift;
        self.bits += shift as i32;
        if self.bits >= 0 && self.pos < self.data.len() {
            let hi = self.data[self.pos] as u32;
            let lo = self.data.get(self.pos + 1).copied().unwrap_or(0) as u32;
            code_word |= (hi << 8 | lo) << self.bits;
            self.pos += 2;
            self.bits -= 16;
        }
        code_word
    }

    pub fn get_prob(&mut self, prob: u8) -> bool {
        let code_word = self.renorm();
        let low = 1 + (((self.high - 1) * prob as u32) >> 8);
        let low_shift = low << 16;
        let bit = code_word >= low_shift;
        if bit {
            self.high -= low;
            self.code_word = code_word - low_shift;
        } else {
            self.high = low;
            self.code_word = code_word;
        }
        bit
    }

    pub fn get(&mut self) -> bool {
        self.get_prob(128)
    }

    /// `bits` equiprobable bits, most significant first.
    pub fn gets(&mut self, bits: u32) -> u32 {
        (0..bits).fold(0, |v, _| v << 1 | self.get() as u32)
    }

    /// A 7-bit probability (stored halved; 0 means 1).
    pub fn gets_nn(&mut self) -> u8 {
        let v = (self.gets(7) << 1) as u8;
        v.max(1)
    }

    pub fn tree(&mut self, tree: &[Tree], probs: &[u8]) -> u8 {
        let mut i = 0usize;
        while tree[i].val > 0 {
            if self.get_prob(probs[tree[i].prob as usize]) {
                i += tree[i].val as usize;
            } else {
                i += 1;
            }
        }
        (-tree[i].val) as u8
    }
}

/// Range encoder (RFC 6386 §7.3), producing data [`RangeDecoder`] reads back.
pub struct RangeEncoder {
    out: Vec<u8>,
    range: u32,
    bottom: u32,
    bit_count: i32,
}

impl Default for RangeEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl RangeEncoder {
    pub fn new() -> Self {
        Self {
            out: Vec::new(),
            range: 255,
            bottom: 0,
            bit_count: 24,
        }
    }

    fn carry(&mut self) {
        for byte in self.out.iter_mut().rev() {
            if *byte == 255 {
                *byte = 0;
            } else {
                *byte += 1;
                return;
            }
        }
    }

    pub fn put_prob(&mut self, prob: u8, bit: bool) {
        let split = 1 + (((self.range - 1) * prob as u32) >> 8);
        if bit {
            self.bottom = self.bottom.wrapping_add(split);
            self.range -= split;
        } else {
            self.range = split;
        }
        while self.range < 128 {
            self.range <<= 1;
            if self.bottom & (1 << 31) != 0 {
                self.carry();
            }
            self.bottom <<= 1;
            self.bit_count -= 1;
            if self.bit_count == 0 {
                self.out.push((self.bottom >> 24) as u8);
                self.bottom &= (1 << 24) - 1;
                self.bit_count = 8;
            }
        }
    }

    pub fn put(&mut self, bit: bool) {
        self.put_prob(128, bit);
    }

    pub fn puts(&mut self, value: u32, bits: u32) {
        for i in (0..bits).rev() {
            self.put(value >> i & 1 != 0);
        }
    }

    /// Writes a probability as the decoder's `gets_nn` reads it (must be even or 1).
    pub fn puts_nn(&mut self, prob: u8) {
        self.puts((prob >> 1) as u32, 7);
    }

    /// Writes `sym` with `tree`; the symbol must be a leaf of the tree.
    pub fn tree(&mut self, tree: &[Tree], probs: &[u8], sym: u8) {
        for (prob, bit) in tree_path(tree, sym) {
            self.put_prob(probs[prob as usize], bit);
        }
    }

    pub fn finish(mut self) -> Vec<u8> {
        // Pad so the decoder never runs dry, then flush the remaining bits.
        for _ in 0..32 {
            self.put(false);
        }
        self.out
    }

    /// Bytes produced so far (excluding the bits still in the coder).
    pub fn len(&self) -> usize {
        self.out.len()
    }

    pub fn is_empty(&self) -> bool {
        self.out.is_empty()
    }
}

/// The (probability index, bit) decisions that lead to `sym` in `tree`.
pub fn tree_path(tree: &[Tree], sym: u8) -> Vec<(u8, bool)> {
    fn walk(tree: &[Tree], i: usize, sym: u8, path: &mut Vec<(u8, bool)>) -> bool {
        let node = tree[i];
        if node.val <= 0 {
            return (-node.val) as u8 == sym;
        }
        path.push((node.prob, false));
        if walk(tree, i + 1, sym, path) {
            return true;
        }
        path.pop();
        path.push((node.prob, true));
        if walk(tree, i + node.val as usize, sym, path) {
            return true;
        }
        path.pop();
        false
    }
    let mut path = Vec::new();
    assert!(
        walk(tree, 0, sym, &mut path),
        "symbol {sym} is not in the tree"
    );
    path
}

/// Cost in 1/256 bit units of coding `bit` with probability `prob` (of a zero).
pub fn bit_cost(prob: u8, bit: bool) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (p, v) in t.iter_mut().enumerate() {
            let p = (p.max(1)) as f64 / 256.0;
            *v = (-p.log2() * 256.0).round() as u32;
        }
        t
    });
    let p = if bit {
        256 - prob as usize
    } else {
        prob as usize
    };
    table[p.min(255)]
}

#[cfg(test)]
mod tests {
    use super::super::tables::{PC_TREE, PCR_TREE};
    use super::*;

    #[test]
    fn round_trip_bits_and_trees() {
        let mut state = 12345u32;
        let mut next = || {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12345);
            state >> 16
        };
        let mut items = Vec::new();
        for _ in 0..20000 {
            let r = next();
            items.push(((r % 255 + 1) as u8, next() % 7 < (r % 255) / 40));
        }
        let mut enc = RangeEncoder::new();
        for &(p, b) in &items {
            enc.put_prob(p, b);
        }
        enc.puts(0x55, 7);
        let probs = [
            100u8, 150, 30, 200, 90, 128, 140, 160, 180, 20, 240, 250, 10, 77,
        ];
        for sym in 0..9 {
            enc.tree(&PCR_TREE, &probs, sym);
        }
        for sym in 0..6 {
            enc.tree(&PC_TREE, &probs, sym);
        }
        let data = enc.finish();
        let mut dec = RangeDecoder::new(&data);
        for &(p, b) in &items {
            assert_eq!(dec.get_prob(p), b);
        }
        assert_eq!(dec.gets(7), 0x55);
        for sym in 0..9 {
            assert_eq!(dec.tree(&PCR_TREE, &probs), sym);
        }
        for sym in 0..6 {
            assert_eq!(dec.tree(&PC_TREE, &probs), sym);
        }
    }
}
