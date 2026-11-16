//! `mod.json` manifests, with the conditions on versions of format 3.
#![no_main]

use doctor_toolkit::mods::ModManifest;
use doctor_toolkit::mods::version::{Condition, parse_version};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(manifest) = ModManifest::parse(data) else {
        return;
    };
    let json = manifest.to_json().expect("a valid manifest serialises");
    let again = ModManifest::parse(&json).expect("a written manifest parses");
    assert_eq!(again.to_json().unwrap(), json, "mod.json round trip");
    let _ = manifest.warnings();
    if manifest.format_version >= 3 {
        let version = parse_version(&manifest.version).expect("a checked version");
        let conditions = manifest
            .depends
            .values()
            .chain(manifest.recommends.values())
            .chain(manifest.incompatible.values().map(|i| &i.versions))
            .chain(manifest.requires_manager.iter());
        for text in conditions {
            let condition = Condition::parse(text).expect("a checked condition");
            let written = Condition::parse(&condition.to_string()).expect("written back");
            assert_eq!(written, condition, "condition round trip: {text}");
            assert_eq!(written.matches(&version), condition.matches(&version));
        }
    }
});
