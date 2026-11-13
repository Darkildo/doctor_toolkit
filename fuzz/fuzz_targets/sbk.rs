//! `sbk` sound bank metadata: parsing, validation, writing and edits.
#![no_main]

use doctor_toolkit::resource::sbk::SoundBank;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(mut bank) = SoundBank::parse(data) else {
        return;
    };
    let _ = bank.validate();
    for i in 0..bank.emitters.len().min(64) {
        let _ = bank.instrument_of(i);
        let _ = bank.emitter_label(i);
    }
    // Writing is canonical: what is written parses back and writes the same.
    let bytes = bank.to_bytes();
    let again = SoundBank::parse(&bytes).expect("a written bank parses");
    assert_eq!(again.to_bytes(), bytes, "writing is not stable");
    // Edits keep the bank writable.
    if !bank.emitters.is_empty() {
        let _ = bank.set_pitch(0, 1.5);
        let _ = bank.set_velocity(0, 100);
        let sample = bank.samples.first().map(|s| s.bytes.clone());
        if let Some(sample) = sample {
            let _ = bank.add_variation(0, sample.clone());
            let _ = bank.add_emitter("fuzz", 0, vec![sample]);
        }
        let bytes = bank.to_bytes();
        let again = SoundBank::parse(&bytes).expect("an edited bank parses");
        assert_eq!(again.to_bytes(), bytes, "writing an edited bank is not stable");
    }
});
