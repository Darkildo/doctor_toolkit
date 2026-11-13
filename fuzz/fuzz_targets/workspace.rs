//! Workspace files: `manifest.json`, `log.json` and `applied.json`,
//! separated by `\0--\0` in the input.
#![no_main]

use doctor_toolkit::workspace::Workspace;
use doctor_toolkit_fuzz::{scratch_dir, split_parts};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let dir = scratch_dir().join("workspace");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let parts = split_parts(data, 3);
    for (name, part) in ["manifest.json", "log.json", "applied.json"].iter().zip(&parts) {
        std::fs::write(dir.join(name), part).unwrap();
    }
    let Ok(mut workspace) = Workspace::open(&dir) else {
        return;
    };
    let _ = workspace.changes().count();
    let _ = workspace.operations().len();
    let _ = workspace.diff();
    let _ = workspace.undo();
    let _ = workspace.redo();
});
