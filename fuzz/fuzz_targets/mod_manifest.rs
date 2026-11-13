//! `mod.json` manifests.
#![no_main]

use doctor_toolkit::mods::ModManifest;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(manifest) = ModManifest::parse(data) else {
        return;
    };
    let json = manifest.to_json().expect("a valid manifest serialises");
    let again = ModManifest::parse(&json).expect("a written manifest parses");
    assert_eq!(again.to_json().unwrap(), json, "mod.json round trip");
});
