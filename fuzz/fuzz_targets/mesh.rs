//! Models (`mesh` + `meshvolatile`): geometry and glTF export.
#![no_main]

use doctor_toolkit::export::gltf::{NoAssets, model_to_glb};
use doctor_toolkit::resource::mesh::Model;
use doctor_toolkit_fuzz::split2;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (mesh, volatile) = split2(data);
    let Ok(model) = Model::parse(mesh, volatile) else {
        return;
    };
    for record in model.records.iter().take(16) {
        let _ = model.triangles(record);
        let _ = model.vertices(record);
    }
    let _ = model_to_glb(&model, &mut NoAssets);
});
