//! Huffman-coded VP6 coefficients: code tables built from the probability
//! models (as the reference decoder builds them) and an MSB-first bit reader.

use crate::Error;

/// MSB-first bit reader; reads past the end give zero bits, as in the
/// reference decoder (callers check [`Bits::left`] where it does).
pub struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Bits not yet read (negative after reading past the end).
    pub fn left(&self) -> i64 {
        self.data.len() as i64 * 8 - self.pos as i64
    }

    /// The next 32 bits without consuming them.
    #[inline]
    fn peek32(&self) -> u32 {
        let byte = self.pos >> 3;
        let mut v = 0u64;
        if let Some(s) = self.data.get(byte..byte + 8) {
            v = u64::from_be_bytes(s.try_into().expect("8 bytes"));
        } else {
            for i in 0..8 {
                v = v << 8 | self.data.get(byte + i).copied().unwrap_or(0) as u64;
            }
        }
        ((v << (self.pos & 7)) >> 32) as u32
    }

    #[inline]
    fn skip(&mut self, n: u32) {
        self.pos = self.pos.saturating_add(n as usize);
    }

    /// `n` bits (0..=24), most significant first.
    #[inline]
    pub fn bits(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let v = self.peek32() >> (32 - n);
        self.skip(n);
        v
    }

    #[inline]
    pub fn bit(&mut self) -> u32 {
        self.bits(1)
    }
}

/// A prefix code as a lookup table indexed by the next `width` bits.
#[derive(Clone, Default)]
pub struct Table {
    /// `symbol << 8 | length`.
    lut: Vec<u16>,
    width: u32,
}

impl Table {
    /// Builds the code of a `size`-symbol tree from its node probabilities
    /// (`model`) and the node-to-symbol map, like `vp6_build_huff_tree`.
    pub fn build(model: &[u8], map: &[u8], size: usize) -> Self {
        #[derive(Clone, Copy, Default)]
        struct Node {
            count: u32,
            sym: i32,
            n0: i32,
        }
        let mut nodes = [Node::default(); 24];
        let size = size.min(12);
        nodes[size].count = 256;
        for i in 0..size - 1 {
            let t = nodes[size + i].count;
            let a = (t * model[i] as u32) >> 8;
            let b = (t * (255 - model[i] as u32)) >> 8;
            nodes[map[2 * i] as usize].count = a + (a == 0) as u32;
            nodes[map[2 * i + 1] as usize].count = b + (b == 0) as u32;
        }
        for (i, n) in nodes.iter_mut().take(size).enumerate() {
            n.sym = i as i32;
            n.n0 = -2;
        }
        // Ascending count, descending symbol.
        nodes[..size].sort_by(|a, b| a.count.cmp(&b.count).then(b.sym.cmp(&a.sym)));
        let mut cur = size;
        nodes[2 * size - 1].count = 0;
        let mut i = 0;
        while i < 2 * size - 1 {
            let count = nodes[i].count + nodes[i + 1].count;
            let mut j = cur;
            while j > i + 2 {
                if count > nodes[j - 1].count {
                    break;
                }
                nodes[j] = nodes[j - 1];
                j -= 1;
            }
            nodes[j] = Node {
                count,
                sym: -1,
                n0: i as i32,
            };
            cur += 1;
            i += 2;
        }
        // Codes by walking the tree from the root.
        let mut codes: Vec<(u32, u32, u8)> = Vec::with_capacity(size);
        let mut stack = vec![(2 * size - 2, 0u32, 0u32)];
        while let Some((n, code, len)) = stack.pop() {
            let node = nodes[n];
            if node.sym != -1 || node.count == 0 || len >= 16 {
                codes.push((code, len, node.sym.max(0) as u8));
            } else {
                let child = node.n0.max(0) as usize;
                stack.push((child + 1, code << 1 | 1, len + 1));
                stack.push((child, code << 1, len + 1));
            }
        }
        let width = codes.iter().map(|c| c.1).max().unwrap_or(0).max(1);
        let mut lut = vec![0u16; 1 << width];
        for &(code, len, sym) in &codes {
            let shift = width - len;
            let first = (code << shift) as usize;
            let entry = (sym as u16) << 8 | len as u16;
            for e in &mut lut[first..first + (1 << shift)] {
                *e = entry;
            }
        }
        Self { lut, width }
    }

    #[inline]
    pub fn read(&self, bits: &mut Bits<'_>) -> u8 {
        let index = bits.peek32() >> (32 - self.width);
        let entry = self.lut.get(index as usize).copied().unwrap_or(0);
        bits.skip((entry & 0xFF) as u32);
        (entry >> 8) as u8
    }
}

/// Error for coefficient data that ran out.
pub fn exhausted(bits: &Bits<'_>) -> Error {
    Error::malformed("VP6", bits.pos >> 3, "end of Huffman data")
}

#[cfg(test)]
mod tests {
    use super::super::tables::{HUFF_COEFF_MAP, HUFF_RUN_MAP};
    use super::*;

    /// The code is complete (a full binary tree) and prefix-free for any model.
    #[test]
    fn codes_are_complete() {
        let mut state = 7u32;
        for _ in 0..500 {
            let mut model = [0u8; 14];
            for m in &mut model {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12345);
                *m = (state >> 16) as u8;
            }
            for (map, size) in [(&HUFF_COEFF_MAP[..], 12), (&HUFF_RUN_MAP[..], 9)] {
                let t = Table::build(&model, map, size);
                // Kraft sum of a complete code is exactly 1: every entry filled.
                let mut seen = vec![false; size];
                for &e in &t.lut {
                    assert!(e & 0xFF > 0);
                    seen[(e >> 8) as usize] = true;
                }
                assert!(seen.iter().all(|&s| s), "every symbol has a code");
            }
        }
    }

    #[test]
    fn bit_reader() {
        let data = [0b1010_0000u8, 0xFF];
        let mut b = Bits::new(&data);
        assert_eq!(b.bit(), 1);
        assert_eq!(b.bits(3), 0b010);
        assert_eq!(b.bits(8), 0b0000_1111);
        assert_eq!(b.left(), 4);
        assert_eq!(b.bits(8), 0b1111_0000);
        assert_eq!(b.left(), -4);
    }
}
