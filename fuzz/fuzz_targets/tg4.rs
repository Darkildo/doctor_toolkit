//! TG4 texture headers (`tg4h`, `tx4h` sets) and decoding of texture data.
#![no_main]

use doctor_toolkit::resource::texture::Texture;
use doctor_toolkit::resource::tg4::{self, Tg4Header};
use doctor_toolkit_fuzz::split2;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (head, pixels) = split2(data);
    if let Ok(set) = tg4::parse_set(head) {
        let _ = tg4::set_to_bytes(&set);
    }
    let Ok(header) = Tg4Header::parse(head) else {
        return;
    };
    let _ = header.kind();
    let _ = header.is_srgb();
    let _ = header.mip_count();
    let _ = header.layer_count();
    let _ = header.surfaces();
    if let Ok(bytes) = header.to_bytes() {
        let again = Tg4Header::parse(&bytes).expect("a written header parses");
        assert_eq!(again, header, "tg4h round trip");
    }
    // As `Texture::load` does: the data must have the declared size.
    if pixels.len() == header.data_size as usize {
        let texture = Texture {
            header,
            data: pixels.to_vec(),
        };
        let _ = texture.to_dds();
        for layer in 0..texture.header.layer_count().min(6) {
            let _ = texture.rgba(layer);
        }
    }
});
