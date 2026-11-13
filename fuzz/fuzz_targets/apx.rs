//! Flash interfaces (`apx`): blocks, embedded textures, writing.
#![no_main]

use doctor_toolkit::resource::apx::AptFile;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(apt) = AptFile::parse(data) else {
        return;
    };
    let _ = apt.title();
    if let Ok(textures) = apt.textures() {
        for texture in textures.iter().take(4) {
            let _ = texture.to_dds();
            let _ = texture.rgba(0);
        }
    }
    if let Ok(bytes) = apt.to_bytes() {
        let again = AptFile::parse(&bytes).expect("a written apx parses");
        assert_eq!(again.to_bytes().ok(), Some(bytes), "apx round trip");
    }
});
