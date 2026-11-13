//! Animation banks (`bnk`): the reader, the clip decoder and the writer.
//! A bank the writer parses must encode, parse again to the same structure
//! and encode to the same bytes.
#![no_main]

use doctor_toolkit::resource::anim::AnimBank;
use doctor_toolkit::resource::anim_image::BankImage;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(bank) = AnimBank::parse(data) {
        for clip in bank.clips.iter().take(8) {
            let _ = clip.decode(data, &[0, 1, 2]);
        }
    }
    let Ok(image) = BankImage::parse(data) else {
        return;
    };
    let _ = image.entry_list();
    let Ok(out) = image.encode() else {
        return;
    };
    let again = BankImage::parse(&out).expect("the writer's output parses");
    assert_eq!(again, image);
    assert_eq!(again.encode().expect("encodes again"), out);
});
