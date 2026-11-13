//! PNG files imported as textures.
#![no_main]

use doctor_toolkit::resource::{pixels, texture};
use doctor_toolkit_fuzz::tg4_header;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok((width, height, rgba)) = pixels::decode_png(data) else {
        return;
    };
    assert_eq!(rgba.len(), width * height * 4, "RGBA buffer size");
    for original in [tg4_header("DXT1c", 64, 64, 7), tg4_header("DXT5_NM", 32, 32, 6)] {
        if let Ok(texture) = texture::from_png(&original, data) {
            assert_eq!(
                texture.data.len(),
                texture.header.expected_data_size().unwrap(),
                "encoded size"
            );
        }
    }
});
