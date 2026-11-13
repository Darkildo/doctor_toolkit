//! Model files: FBX (binary, ASCII), OBJ, USD (text, crate), USDZ, glTF.
//! The first byte picks the reader; a scene that reads is written in every
//! format and read back.
#![no_main]

use doctor_toolkit::scene::{self, ModelFormat, SideFiles};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&selector, bytes)) = data.split_first() else {
        return;
    };
    let none = SideFiles::new(None);
    let read = match selector % 5 {
        0 => scene::fbx::read(bytes, &none),
        1 => scene::obj::read(bytes, &none, "fuzz"),
        2 => scene::usd::read(bytes, &none),
        3 => scene::usd::read_usdz(bytes),
        _ => doctor_toolkit::import::gltf::read(bytes, None),
    };
    let Ok(read) = read else { return };
    if read.validate().is_err() {
        // Readers must give consistent scenes.
        panic!("inconsistent scene");
    }
    // Small scenes only: writing is not what is fuzzed here.
    let vertices: usize = read.parts.iter().map(|p| p.geometry.positions.len()).sum();
    if vertices > 10_000 || read.clips.iter().map(|c| c.frames() * c.tracks.len()).sum::<usize>() > 100_000 {
        return;
    }
    for format in ModelFormat::WRITABLE {
        let Ok(files) = scene::write(&read, format, "model") else {
            continue;
        };
        let memory = files.iter().map(|f| (f.name.clone(), f.bytes.clone())).collect();
        let side = SideFiles::in_memory(memory);
        let bytes = &files[0].bytes;
        let back = match format {
            ModelFormat::Glb | ModelFormat::Gltf => doctor_toolkit::import::gltf::read(bytes, None),
            ModelFormat::Fbx => scene::fbx::read(bytes, &side),
            ModelFormat::Obj => scene::obj::read(bytes, &side, "model"),
            ModelFormat::Usdz => scene::usd::read_usdz(bytes),
            _ => scene::usd::read(bytes, &side),
        };
        if let Ok(back) = back {
            assert!(back.validate().is_ok(), "{format}: written scene reads inconsistent");
        }
    }
});
