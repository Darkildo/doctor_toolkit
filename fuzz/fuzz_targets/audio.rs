//! Sound: `sbk` banks, `.snu` files, SNR/SNS and EA-XAS decoding.
#![no_main]

use doctor_toolkit::resource::audio::{self, Sample};
use libfuzzer_sys::fuzz_target;

fn decode(sample: &Sample<'_>) {
    if let Ok(pcm) = sample.decode() {
        let _ = audio::wav(&pcm, u16::from(sample.header.channels), sample.header.sample_rate);
    }
}

fuzz_target!(|data: &[u8]| {
    if let Ok(sounds) = audio::parse_sbk(data) {
        for sound in sounds.iter().take(8) {
            decode(&sound.sample);
            let _ = audio::to_snu(data, &sound.sample);
        }
    }
    if let Ok(sample) = Sample::parse_snu(data) {
        decode(&sample);
    }
    if let Ok(sample) = Sample::parse_inline(data, 0) {
        decode(&sample);
    }
});
