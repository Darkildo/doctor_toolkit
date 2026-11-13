//! String tables (`lh2`): parse, write, parse again.
#![no_main]

use doctor_toolkit::resource::lh2::StringTable;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(table) = StringTable::parse(data) else {
        return;
    };
    if let Some(&hash) = table.hashes.first() {
        let _ = table.find(hash);
    }
    if let Ok(bytes) = table.to_bytes() {
        let again = StringTable::parse(&bytes).expect("a written table parses");
        assert_eq!(again, table, "lh2 round trip");
    }
});
