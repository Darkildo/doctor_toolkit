//! Rigs (`rcb`): skeletons and bind poses.
#![no_main]

use doctor_toolkit::resource::rcb::Rig;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(rig) = Rig::parse(data) else {
        return;
    };
    for skeleton in &rig.skeletons {
        let _ = skeleton.bind_globals();
        let _ = skeleton.bind_locals();
    }
    let _ = serde_json::to_vec(&rig);
});
