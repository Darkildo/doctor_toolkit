//! `BIGF` archives (`.viv`).
#![no_main]

use doctor_toolkit::container::big::{self, BigArchive};
use doctor_toolkit_fuzz::scratch_file;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = big::parse_table(data, data.len() as u64);
    let path = scratch_file("archive.viv", data);
    if let Ok(archive) = BigArchive::open(&path) {
        for entry in archive.entries.iter().take(16) {
            let data = archive.read(entry).expect("an entry inside the file reads");
            assert_eq!(data.len() as u64, entry.size);
        }
    }
});
