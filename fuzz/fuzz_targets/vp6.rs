//! The full VP6 decoder on a stream of frames: the input is split into
//! frames (two-byte length prefixes); the first is made a key frame of at
//! most 8×8 macroblocks so every following inter frame is decoded with
//! motion compensation, filters, the golden frame and concealment.
#![no_main]

use doctor_toolkit::resource::video::vp6;
use doctor_toolkit_fuzz::split2;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut dec = vp6::Decoder::new();
    let mut rest = data;
    let mut first = true;
    let mut count = 0;
    while !rest.is_empty() && count < 16 {
        let (frame, tail) = split2(rest);
        rest = tail;
        count += 1;
        let mut frame = frame.to_vec();
        if first && frame.len() > 8 {
            // Key frame: keep the flags, bound the size.
            frame[0] &= 0x7F;
            frame[1] &= !1;
            let base = if frame[0] & 1 != 0 || frame[1] & 6 == 0 { 4 } else { 2 };
            frame[base] = frame[base] % 8 + 1;
            frame[base + 1] = frame[base + 1] % 8 + 1;
        }
        first = false;
        let _ = dec.decode(&frame);
        if let Some(p) = dec.picture() {
            let mut rgba = Vec::new();
            p.rgba_into(p.width, p.height, true, &mut rgba);
        }
    }
});
