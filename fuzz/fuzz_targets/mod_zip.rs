//! Mod packages (ZIP files): open, verify, read the preview.
#![no_main]

use doctor_toolkit::mods::package::ModArchive;
use doctor_toolkit_fuzz::scratch_file;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let path = scratch_file("mod.zip", data);
    let Ok(mut archive) = ModArchive::open(&path) else {
        return;
    };
    let _ = archive.warnings().len();
    let _ = archive.read_preview();
    let _ = archive.verify();
});
