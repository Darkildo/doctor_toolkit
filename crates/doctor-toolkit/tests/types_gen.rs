//! The files generated from `data/gf2/` (JSON Schema, the Python module of
//! the value lists and its stub) are up to date: `doctor-toolkit types-gen`
//! writes them.

use std::path::Path;

use doctor_toolkit::game_types::export;

#[test]
fn the_generated_files_are_up_to_date() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let stale = export::write(&root, true).unwrap();
    assert!(
        stale.is_empty(),
        "run `doctor-toolkit types-gen`: out of date {stale:?}"
    );
    for (rel, text) in export::files() {
        assert!(root.join(rel).is_file(), "{rel}");
        assert!(!text.is_empty());
    }
}
