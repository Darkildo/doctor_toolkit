//! DDS files imported as textures.
#![no_main]

use doctor_toolkit::resource::{dds, texture};
use doctor_toolkit_fuzz::tg4_header;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if dds::read(data).is_err() {
        return;
    }
    for original in [
        tg4_header("DXT1c", 64, 64, 7),
        tg4_header("DXT5_NM", 32, 32, 6),
        tg4_header("RGBA8", 16, 16, 1),
    ] {
        if let Ok(texture) = texture::from_dds(&original, data) {
            let _ = texture.to_dds();
            let _ = texture.rgba(0);
        }
    }
});
