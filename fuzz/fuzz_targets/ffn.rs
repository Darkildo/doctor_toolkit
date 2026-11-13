//! Fonts (`ffn`).
#![no_main]

use doctor_toolkit::resource::ffn::Font;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(font) = Font::parse(data) else {
        return;
    };
    for glyph in font.glyphs.iter().take(64) {
        let _ = glyph.character();
    }
    let _ = font.covers('A');
    let _ = serde_json::to_vec(&font);
});
