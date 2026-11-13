//! Materials.
#![no_main]

use doctor_toolkit::resource::material::Material;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(material) = Material::parse(data) {
        let _ = material.textures();
    }
});
