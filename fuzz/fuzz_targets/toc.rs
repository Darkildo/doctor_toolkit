//! Tables of contents (`.toc`): parse, write, parse again.
#![no_main]

use doctor_toolkit::toc::Toc;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(toc) = Toc::parse(data) else {
        return;
    };
    for index in 0..toc.files.len().min(64) {
        let _ = toc.file_name(index).name_lossy();
        let _ = toc.file_deps(index).count();
    }
    let _ = toc.is_sorted_by_hash();
    if let Ok(bytes) = toc.to_bytes() {
        let again = Toc::parse(&bytes).expect("a written TOC parses");
        assert_eq!(again, toc, "TOC round trip");
    }
    let mut sorted = toc.clone();
    sorted.sort_files_by_hash();
    let _ = sorted.to_bytes();
});
