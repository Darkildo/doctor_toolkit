//! Havok packfiles (`hkx`).
#![no_main]

use doctor_toolkit::resource::hkx::geometry::physics_mesh;
use doctor_toolkit::resource::hkx::graph::Graph;
use doctor_toolkit::resource::hkx::physics::{Physics, ShapeKind};
use doctor_toolkit::resource::hkx::Packfile;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(packfile) = Packfile::parse(data) else {
        return;
    };
    let _ = serde_json::to_vec(&packfile);
    let _ = packfile.to_bytes();
    if let Ok(graph) = Graph::from_packfile(&packfile) {
        let _ = graph.layout();
    }
    if let Ok(physics) = Physics::from_packfile(&packfile) {
        let _ = physics_mesh(&physics);
        for body in physics.bodies() {
            if let ShapeKind::Mopp { code, .. } = &body.shape.kind {
                let _ = code.keys();
                let _ = code.query([0.0; 3], [1.0; 3]);
            }
        }
        let mut copy = packfile.clone();
        let _ = physics.store(&mut copy);
    }
});
