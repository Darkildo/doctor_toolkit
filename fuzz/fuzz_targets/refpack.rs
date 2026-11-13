//! RefPack decompression; compression round trip.
#![no_main]

use doctor_toolkit::compress::refpack;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = refpack::parse_header(data);
    let _ = refpack::decompress(data);
    let _ = refpack::decompress_with_limit(data, 1 << 16);
    if let Ok(packed) = refpack::compress(data) {
        let unpacked = refpack::decompress(&packed).expect("compressed data decompresses");
        assert_eq!(unpacked, data, "RefPack round trip");
    }
});
