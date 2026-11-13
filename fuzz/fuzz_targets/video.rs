//! Movies: EA container, EA-XA audio, the VP6 decoder, and the WAV,
//! AIFF, AVI, FLV and Y4M readers used by movie import.
#![no_main]

use doctor_toolkit::resource::video::{self, Movie, input, vp6};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(movie) = Movie::parse(data) {
        let _ = movie.info();
        let _ = movie.decode_audio();
        // A few frames only: each can be a 4080×4080 picture.
        let mut dec = vp6::Decoder::new();
        for (_, frame) in movie.frames().take(3) {
            let _ = dec.decode(frame);
        }
    }
    let _ = video::info(data);
    let _ = vp6::FrameHeader::parse(data);
    let _ = vp6::Decoder::new().decode(data);
    let _ = input::read_wav(data);
    let _ = input::read_aiff(data);
    let _ = input::read_avi_vp6(data);
    let _ = input::read_flv_vp6(data);
    if let Ok(mut y4m) = input::Y4mReader::new(data) {
        let _ = y4m.next_frame();
    }
});
