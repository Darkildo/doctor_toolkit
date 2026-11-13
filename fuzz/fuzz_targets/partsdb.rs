//! `partsdb` resources decoded to JSON.
#![no_main]

use doctor_toolkit::resource::logic::{self, TYPE_PARTSDB};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = logic::to_json(TYPE_PARTSDB, data);
});
