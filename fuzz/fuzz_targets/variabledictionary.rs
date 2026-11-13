//! `variabledictionary` resources decoded to JSON.
#![no_main]

use doctor_toolkit::resource::logic::{self, TYPE_VARIABLE_DICTIONARY};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = logic::to_json(TYPE_VARIABLE_DICTIONARY, data);
});
