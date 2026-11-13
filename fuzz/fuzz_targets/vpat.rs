//! Patches of the old toolkit (`.vpat`), with resources addressed by ids or by
//! type and every operation.
#![no_main]

use doctor_toolkit::mods::vpat::{BY_TYPE_IDS, FILE_FLAG_BY_TYPE, VpatOperation, VpatPatch};
use doctor_toolkit_fuzz::scratch_file;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let path = scratch_file("patch.vpat", data);
    let Ok(patch) = VpatPatch::open(&path) else {
        return;
    };
    let mut end = 0;
    for file in &patch.files {
        if file.resources.iter().any(|r| r.by_type) {
            assert_ne!(file.flags & FILE_FLAG_BY_TYPE, 0);
        }
        for resource in &file.resources {
            if resource.by_type {
                assert_eq!(resource.ids, BY_TYPE_IDS);
            }
            assert!(VpatOperation::from_u32(resource.operation as u32).is_some());
            let _ = resource.key(&file.stream).to_string();
            assert!(resource.offset >= end, "resources are in file order");
            end = resource.offset + u64::from(resource.size);
        }
    }
    assert!(end <= data.len() as u64);
    let _ = patch.resource_count();
    for file in patch.files.iter().take(4) {
        for resource in file.resources.iter().take(4) {
            let bytes = patch.read_data(resource).expect("a resource inside the file reads");
            assert_eq!(bytes.len(), resource.size as usize);
        }
    }
});
