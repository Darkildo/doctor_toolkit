//! RefPack, the LZ77 variant EA uses for compressed stream blocks.
//!
//! # Stream layout
//!
//! ```text
//! u8   flags      bit 0: a compressed-size field follows the signature
//!                 bit 7: size fields are 4 bytes wide instead of 3
//! u8   0xFB       signature
//! [u24|u32 BE]    compressed size, only if flags bit 0 is set
//! u24|u32 BE      decompressed size
//! ...             commands
//! ```
//!
//! # Commands
//!
//! | First byte | Bytes | Literals | Match length | Match offset |
//! |---|---|---|---|---|
//! | `0x00..=0x7F` | 2 | 0–3 | 3–10 | 1–1024 |
//! | `0x80..=0xBF` | 3 | 0–3 | 4–67 | 1–16384 |
//! | `0xC0..=0xDF` | 4 | 0–3 | 5–1028 | 1–131072 |
//! | `0xE0..=0xFB` | 1 | 4–112, multiple of 4 | — | — |
//! | `0xFC..=0xFF` | 1 | 0–3, then end of stream | — | — |
//!
//! Literals are copied before the match. Matches may overlap their own output.

/// Largest decompressed size a 3-byte size field can describe.
pub const MAX_SIZE_NARROW: usize = 0x00FF_FFFF;

const SIGNATURE: u8 = 0xFB;
const FLAG_COMPRESSED_SIZE: u8 = 0x01;
const FLAG_WIDE: u8 = 0x80;

const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 1028;
const MAX_OFFSET: usize = 131_072;
const MAX_LITERAL_RUN: usize = 112;

/// Errors produced by the RefPack codec.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RefpackError {
    /// The input ended in the middle of the header or of a command.
    #[error("input truncated")]
    Truncated,
    /// The second header byte is not `0xFB`.
    #[error("bad signature byte {0:#04x}")]
    BadSignature(u8),
    /// A match refers to data before the start of the output.
    #[error("match offset {offset} reaches before the start of the output ({produced} byte(s))")]
    BadOffset { offset: usize, produced: usize },
    /// The commands produce more data than the header declares.
    #[error("output exceeds the declared size of {0} byte(s)")]
    Overflow(usize),
    /// The stream ended before producing the declared amount of data.
    #[error("stream ended after {produced} of {declared} byte(s)")]
    Short { produced: usize, declared: usize },
    /// The declared size is above the caller's limit.
    #[error("declared size {declared} exceeds the limit of {limit} byte(s)")]
    TooLarge { declared: usize, limit: usize },
}

/// Parsed RefPack header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Raw flags byte.
    pub flags: u8,
    /// Optional compressed size stored in the header.
    pub compressed_size: Option<usize>,
    /// Number of bytes the stream decompresses to.
    pub decompressed_size: usize,
    /// Length of the header in bytes; commands start here.
    pub len: usize,
}

/// Parses the header at the start of `src`.
pub fn parse_header(src: &[u8]) -> Result<Header, RefpackError> {
    let (&flags, &signature) = match src {
        [flags, signature, ..] => (flags, signature),
        _ => return Err(RefpackError::Truncated),
    };
    if signature != SIGNATURE {
        return Err(RefpackError::BadSignature(signature));
    }
    let width = if flags & FLAG_WIDE != 0 { 4 } else { 3 };
    let read_size = |pos: usize| -> Result<usize, RefpackError> {
        let bytes = src.get(pos..pos + width).ok_or(RefpackError::Truncated)?;
        Ok(bytes.iter().fold(0usize, |acc, &b| (acc << 8) | b as usize))
    };
    let mut pos = 2;
    let compressed_size = if flags & FLAG_COMPRESSED_SIZE != 0 {
        let size = read_size(pos)?;
        pos += width;
        Some(size)
    } else {
        None
    };
    let decompressed_size = read_size(pos)?;
    pos += width;
    Ok(Header {
        flags,
        compressed_size,
        decompressed_size,
        len: pos,
    })
}

/// Decompresses a RefPack stream.
///
/// Returns the decompressed data and the number of input bytes consumed.
/// `limit` bounds the declared output size so that a corrupt header cannot
/// trigger a huge allocation.
pub fn decompress_with_limit(src: &[u8], limit: usize) -> Result<(Vec<u8>, usize), RefpackError> {
    let header = parse_header(src)?;
    let declared = header.decompressed_size;
    if declared > limit {
        return Err(RefpackError::TooLarge { declared, limit });
    }
    let mut out: Vec<u8> = Vec::with_capacity(declared);
    let mut pos = header.len;

    loop {
        let op = take(src, &mut pos, 1)?[0];
        let (literals, match_len, match_offset) = match op {
            0x00..=0x7F => {
                let b1 = take(src, &mut pos, 1)?[0] as usize;
                let op = op as usize;
                (op & 3, ((op >> 2) & 7) + 3, (((op & 0x60) << 3) | b1) + 1)
            }
            0x80..=0xBF => {
                let b = take(src, &mut pos, 2)?;
                let (b1, b2) = (b[0] as usize, b[1] as usize);
                (
                    b1 >> 6,
                    (op as usize & 0x3F) + 4,
                    (((b1 & 0x3F) << 8) | b2) + 1,
                )
            }
            0xC0..=0xDF => {
                let b = take(src, &mut pos, 3)?;
                let (b1, b2, b3) = (b[0] as usize, b[1] as usize, b[2] as usize);
                let op = op as usize;
                (
                    op & 3,
                    ((op & 0x0C) << 6) + b3 + 5,
                    (((op & 0x10) << 12) | (b1 << 8) | b2) + 1,
                )
            }
            0xE0..=0xFB => {
                let n = ((op as usize & 0x1F) + 1) * 4;
                let bytes = take(src, &mut pos, n)?;
                if out.len() + n > declared {
                    return Err(RefpackError::Overflow(declared));
                }
                out.extend_from_slice(bytes);
                continue;
            }
            0xFC..=0xFF => {
                let n = op as usize & 3;
                let bytes = take(src, &mut pos, n)?;
                if out.len() + n > declared {
                    return Err(RefpackError::Overflow(declared));
                }
                out.extend_from_slice(bytes);
                break;
            }
        };

        let bytes = take(src, &mut pos, literals)?;
        if out.len() + literals + match_len > declared {
            return Err(RefpackError::Overflow(declared));
        }
        out.extend_from_slice(bytes);
        if match_offset > out.len() {
            return Err(RefpackError::BadOffset {
                offset: match_offset,
                produced: out.len(),
            });
        }
        let start = out.len() - match_offset;
        if match_offset >= match_len {
            out.extend_from_within(start..start + match_len);
        } else {
            // Overlapping match: the source runs into bytes produced by this very copy.
            for i in 0..match_len {
                let byte = out[start + i];
                out.push(byte);
            }
        }
    }

    if out.len() != declared {
        return Err(RefpackError::Short {
            produced: out.len(),
            declared,
        });
    }
    Ok((out, pos))
}

/// Returns the next `n` input bytes and advances `pos`.
fn take<'a>(src: &'a [u8], pos: &mut usize, n: usize) -> Result<&'a [u8], RefpackError> {
    let bytes = src.get(*pos..*pos + n).ok_or(RefpackError::Truncated)?;
    *pos += n;
    Ok(bytes)
}

/// Decompresses a RefPack stream with the default 16 MiB size limit of the 3-byte header.
pub fn decompress(src: &[u8]) -> Result<Vec<u8>, RefpackError> {
    decompress_with_limit(src, MAX_SIZE_NARROW).map(|(out, _)| out)
}

/// Compresses `src` into a RefPack stream with the `10 FB` header used by the game.
///
/// The encoder is a greedy hash-chain matcher. It is deterministic and always
/// produces a stream that [`decompress`] turns back into `src`; it does not try
/// to reproduce EA's encoder byte for byte.
///
/// Fails with [`RefpackError::TooLarge`] when `src` does not fit the 3-byte size field.
pub fn compress(src: &[u8]) -> Result<Vec<u8>, RefpackError> {
    if src.len() > MAX_SIZE_NARROW {
        return Err(RefpackError::TooLarge {
            declared: src.len(),
            limit: MAX_SIZE_NARROW,
        });
    }
    let n = src.len();
    let mut out = Vec::with_capacity(n / 2 + 16);
    out.push(0x10);
    out.push(SIGNATURE);
    out.extend_from_slice(&(n as u32).to_be_bytes()[1..]);

    const HASH_BITS: u32 = 16;
    const MAX_CHAIN: usize = 48;
    const NONE: u32 = u32::MAX;
    let hash3 = |i: usize| -> usize {
        let v = (src[i] as u32) | ((src[i + 1] as u32) << 8) | ((src[i + 2] as u32) << 16);
        (v.wrapping_mul(0x9E37_79B1) >> (32 - HASH_BITS)) as usize
    };
    let mut head = vec![NONE; 1 << HASH_BITS];
    let mut prev = vec![NONE; n];

    let mut literal_start = 0usize; // first literal not yet written
    let mut i = 0usize;
    while i < n {
        let mut best_len = 0usize;
        let mut best_offset = 0usize;
        if i + MIN_MATCH <= n {
            let h = hash3(i);
            let mut candidate = head[h];
            let max_len = (n - i).min(MAX_MATCH);
            let mut chain = 0;
            while candidate != NONE && chain < MAX_CHAIN {
                let c = candidate as usize;
                let offset = i - c;
                if offset > MAX_OFFSET {
                    break;
                }
                if src[c + best_len.min(max_len - 1)] == src[i + best_len.min(max_len - 1)] {
                    let len = src[c..c + max_len]
                        .iter()
                        .zip(&src[i..i + max_len])
                        .take_while(|(a, b)| a == b)
                        .count();
                    if len > best_len && is_encodable(len, offset) {
                        best_len = len;
                        best_offset = offset;
                        if len == max_len {
                            break;
                        }
                    }
                }
                candidate = prev[c];
                chain += 1;
            }
            prev[i] = head[h];
            head[h] = i as u32;
        }

        if best_len == 0 {
            i += 1;
            continue;
        }

        // Flush pending literals so that at most three remain for the match command.
        let mut pending = i - literal_start;
        while pending > 3 {
            let run = (pending & !3).min(MAX_LITERAL_RUN);
            out.push(0xE0 | (run / 4 - 1) as u8);
            out.extend_from_slice(&src[literal_start..literal_start + run]);
            literal_start += run;
            pending -= run;
        }
        write_match(&mut out, pending, best_len, best_offset);
        out.extend_from_slice(&src[literal_start..literal_start + pending]);

        // Index the positions covered by the match so later data can refer to them.
        let end = i + best_len;
        let mut j = i + 1;
        while j < end && j + MIN_MATCH <= n {
            let h = hash3(j);
            prev[j] = head[h];
            head[h] = j as u32;
            j += 1;
        }
        i = end;
        literal_start = end;
    }

    let mut pending = n - literal_start;
    while pending > 3 {
        let run = (pending & !3).min(MAX_LITERAL_RUN);
        out.push(0xE0 | (run / 4 - 1) as u8);
        out.extend_from_slice(&src[literal_start..literal_start + run]);
        literal_start += run;
        pending -= run;
    }
    out.push(0xFC | pending as u8);
    out.extend_from_slice(&src[literal_start..]);
    Ok(out)
}

/// Whether some command can express a match of `len` bytes at `offset`.
fn is_encodable(len: usize, offset: usize) -> bool {
    (len >= 3 && offset <= 1024) || (len >= 4 && offset <= 16_384) || len >= 5
}

/// Writes the shortest command for the given match. The literals follow the command.
fn write_match(out: &mut Vec<u8>, literals: usize, len: usize, offset: usize) {
    debug_assert!(literals <= 3 && is_encodable(len, offset) && offset <= MAX_OFFSET);
    let o = offset - 1;
    if offset <= 1024 && len <= 10 {
        out.push((((o >> 8) << 5) | ((len - 3) << 2) | literals) as u8);
        out.push(o as u8);
    } else if offset <= 16_384 && (4..=67).contains(&len) {
        out.push((0x80 | (len - 4)) as u8);
        out.push(((literals << 6) | (o >> 8)) as u8);
        out.push(o as u8);
    } else {
        let l = len - 5;
        out.push((0xC0 | ((o >> 16) << 4) | ((l >> 8) << 2) | literals) as u8);
        out.push((o >> 8) as u8);
        out.push(o as u8);
        out.push(l as u8);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn decodes_each_command_class() {
        // "abcabcabc": 3 literals, then a short match of 6 at offset 3 (overlapping).
        let stream = [0x10, 0xFB, 0, 0, 9, 0xFC | 3, b'a', b'b', b'c'];
        assert_eq!(
            decompress(&stream),
            Err(RefpackError::Short {
                produced: 3,
                declared: 9
            })
        );

        let stream = [0x10, 0xFB, 0, 0, 9, 0x0F, 0x02, b'a', b'b', b'c', 0xFC];
        assert_eq!(decompress(&stream).unwrap(), b"abcabcabc");

        // Literal run of 4, then a medium match of 4 at offset 4.
        let stream = [
            0x10, 0xFB, 0, 0, 8, 0xE0, b'w', b'x', b'y', b'z', 0x80, 0x00, 0x03, 0xFC,
        ];
        assert_eq!(decompress(&stream).unwrap(), b"wxyzwxyz");

        // Literal run of 4, then a long match of 5 at offset 1 (run of one byte).
        let stream = [
            0x10, 0xFB, 0, 0, 9, 0xE0, b'q', b'q', b'q', b'q', 0xC0, 0, 0, 0, 0xFC,
        ];
        assert_eq!(decompress(&stream).unwrap(), b"qqqqqqqqq");
    }

    #[test]
    fn header_with_compressed_size_field() {
        let stream = [0x11, 0xFB, 0, 0, 99, 0, 0, 2, 0xFE, b'h', b'i'];
        let header = parse_header(&stream).unwrap();
        assert_eq!(header.compressed_size, Some(99));
        assert_eq!(header.decompressed_size, 2);
        assert_eq!(header.len, 8);
        assert_eq!(decompress(&stream).unwrap(), b"hi");
    }

    #[test]
    fn rejects_malformed_streams() {
        assert_eq!(decompress(&[]), Err(RefpackError::Truncated));
        assert_eq!(decompress(&[0x10]), Err(RefpackError::Truncated));
        assert_eq!(
            decompress(&[0x10, 0x00, 0, 0, 0]),
            Err(RefpackError::BadSignature(0))
        );
        assert_eq!(
            decompress(&[0x10, 0xFB, 0, 0]),
            Err(RefpackError::Truncated)
        );
        // Match before any output exists.
        assert_eq!(
            decompress(&[0x10, 0xFB, 0, 0, 3, 0x00, 0x00, 0xFC]),
            Err(RefpackError::BadOffset {
                offset: 1,
                produced: 0
            })
        );
        // More output than declared.
        assert_eq!(
            decompress(&[0x10, 0xFB, 0, 0, 1, 0xFE, b'a', b'b']),
            Err(RefpackError::Overflow(1))
        );
        // Declared size above the limit.
        assert_eq!(
            decompress_with_limit(&[0x10, 0xFB, 0xFF, 0xFF, 0xFF, 0xFC], 16),
            Err(RefpackError::TooLarge {
                declared: 0xFF_FFFF,
                limit: 16
            })
        );
    }

    #[test]
    fn reports_consumed_length() {
        let mut stream = vec![0x10, 0xFB, 0, 0, 2, 0xFE, b'o', b'k'];
        let len = stream.len();
        stream.extend_from_slice(&[0xAA, 0xBB]); // padding after the stream
        let (out, used) = decompress_with_limit(&stream, 64).unwrap();
        assert_eq!(out, b"ok");
        assert_eq!(used, len);
    }

    #[test]
    fn compresses_repetitive_data_well() {
        let data: Vec<u8> = (0..20_000u32).map(|i| (i % 7) as u8).collect();
        let packed = compress(&data).unwrap();
        assert!(
            packed.len() < data.len() / 20,
            "packed to {} bytes",
            packed.len()
        );
        assert_eq!(decompress(&packed).unwrap(), data);
    }

    #[test]
    fn compress_handles_edge_sizes() {
        for len in [
            0usize, 1, 2, 3, 4, 5, 111, 112, 113, 115, 116, 1027, 1028, 1029, 1030,
        ] {
            let data: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
            assert_eq!(
                decompress(&compress(&data).unwrap()).unwrap(),
                data,
                "len {len}"
            );
            let zeros = vec![0u8; len];
            assert_eq!(
                decompress(&compress(&zeros).unwrap()).unwrap(),
                zeros,
                "zeros {len}"
            );
        }
    }

    #[test]
    fn compress_uses_far_offsets() {
        // Two identical random-looking blocks 100 000 bytes apart need the long command.
        let mut state = 0x1234_5678u32;
        let block: Vec<u8> = (0..2000)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect();
        let mut data = block.clone();
        data.extend((0..98_000).map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        }));
        data.extend_from_slice(&block);
        let packed = compress(&data).unwrap();
        // Literal runs cost one byte per 112, so the saving is the match minus that overhead.
        assert!(
            packed.len() < data.len() - 1000,
            "packed to {} bytes",
            packed.len()
        );
        assert_eq!(decompress(&packed).unwrap(), data);
    }

    /// Byte strings with long repeats, the interesting case for an LZ codec.
    fn compressible() -> impl Strategy<Value = Vec<u8>> {
        prop::collection::vec(
            (prop::collection::vec(any::<u8>(), 1..24), 1usize..40),
            0..40,
        )
        .prop_map(|parts| {
            parts
                .into_iter()
                .flat_map(|(chunk, times)| chunk.repeat(times))
                .collect()
        })
    }

    proptest! {
        #[test]
        fn roundtrip_random(data in prop::collection::vec(any::<u8>(), 0..4096)) {
            prop_assert_eq!(decompress(&compress(&data).unwrap()).unwrap(), data);
        }

        #[test]
        fn roundtrip_compressible(data in compressible()) {
            prop_assert_eq!(decompress(&compress(&data).unwrap()).unwrap(), data);
        }

        #[test]
        fn decompress_never_panics(data in prop::collection::vec(any::<u8>(), 0..512)) {
            let _ = decompress(&data);
            let mut framed = vec![0x10, 0xFB, 0x00, 0x10, 0x00];
            framed.extend_from_slice(&data);
            let _ = decompress(&framed);
        }

        /// Our decoder agrees with the independent `refpack` crate on streams we produce.
        #[test]
        fn reference_crate_decodes_our_output(data in compressible()) {
            let packed = compress(&data).unwrap();
            let reference = refpack::easy_decompress::<refpack::format::SimEA>(&packed);
            prop_assert_eq!(reference.ok(), Some(data));
        }

        /// Our decoder reads streams produced by the independent `refpack` crate.
        #[test]
        fn decodes_reference_crate_output(data in compressible()) {
            // The reference crate refuses empty input.
            prop_assume!(!data.is_empty());
            let packed = refpack::easy_compress::<refpack::format::SimEA>(&data, Default::default()).unwrap();
            prop_assert_eq!(decompress(&packed).unwrap(), data);
        }
    }
}
