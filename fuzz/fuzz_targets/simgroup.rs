//! SimGroup: parse, JSON, encode, parse again.
#![no_main]

use doctor_toolkit::resource::logic::{self, TYPE_SIMGROUP};
use doctor_toolkit::resource::simgroup::SimGroup;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(group) = SimGroup::parse(data) else {
        return;
    };
    for object in group.objects.iter().take(32) {
        let _ = group.transform(object);
        let _ = group.model(object);
        let _ = group.object_name(object);
        let _ = group.object_packets(object).count();
    }
    let _ = logic::to_json(TYPE_SIMGROUP, data);
    let bytes = group.encode();
    let again = SimGroup::parse(&bytes).expect("an encoded group parses");
    assert_eq!(again.encode(), bytes, "SimGroup round trip");
});
