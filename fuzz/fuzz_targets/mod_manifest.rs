//! `mod.json` manifests, with the conditions on versions of format 3 and
//! the resources stored as deltas of format 6.
#![no_main]

use doctor_toolkit::mods::{DELTA_VERSION, ModManifest};
use doctor_toolkit::resource::delta;
use doctor_toolkit::mods::version::{Condition, parse_version};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(manifest) = ModManifest::parse(data) else {
        return;
    };
    let json = manifest.to_json().expect("a valid manifest serialises");
    let again = ModManifest::parse(&json).expect("a written manifest parses");
    assert_eq!(again.to_json().unwrap(), json, "mod.json round trip");
    assert_eq!(again, manifest, "mod.json round trip");
    let _ = manifest.warnings();
    // Format 6: a resource that is a delta is one of a kind its type has,
    // never an added one, and no program that reads only up to format 5
    // accepts the manifest (it would write the document as the resource).
    assert!(manifest.required_version() <= manifest.format_version);
    for resource in manifest.deltas() {
        let named = resource.delta.expect("a delta");
        assert_eq!(delta::kind_of(resource.key.type_id), Some(named.kind));
        assert!((1..=delta::FORMAT).contains(&named.format));
        assert!(!resource.is_added());
    }
    if manifest.deltas().next().is_some() {
        assert_eq!(manifest.required_version(), DELTA_VERSION);
        assert!(ModManifest::parse_up_to(data, DELTA_VERSION - 1).is_err());
        assert!(ModManifest::parse_up_to(&json, DELTA_VERSION - 1).is_err());
    }
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
