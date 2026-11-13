//! UStream (`.str`) layout, resource data and rewriting.
#![no_main]

use doctor_toolkit::container::ustream::{self, Replacement, Storage, StreamLayout};
use doctor_toolkit::resource::texture::{self, Texture};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(layout) = StreamLayout::parse(data) else {
        return;
    };
    let _ = layout.total_data_size();
    for index in 0..layout.resources.len().min(64) {
        let _ = layout.stored_size(index);
        let _ = layout.is_compressed(index);
        let _ = layout.resource_data_with_excess(data, index);
    }
    for pair in texture::pairs(&layout).into_iter().take(8) {
        if let Ok(texture) = Texture::load(data, &layout, pair) {
            let _ = texture.to_dds();
            let _ = texture.rgba(0);
        }
    }
    let _ = ustream::relayout(data, &layout);
    if !layout.resources.is_empty() {
        let last = layout.resources.len() - 1;
        let replacements = [
            Replacement {
                index: last,
                data: b"replacement data",
                storage: Storage::RefPack,
            },
            Replacement {
                index: 0,
                data: &data[..data.len().min(300)],
                storage: Storage::Raw,
            },
        ];
        let replacements = if last == 0 {
            &replacements[1..]
        } else {
            &replacements[..]
        };
        if let Ok(rebuilt) = ustream::rebuild(data, &layout, replacements) {
            let again = StreamLayout::parse(&rebuilt).expect("a rebuilt stream parses");
            assert_eq!(again.resources.len(), layout.resources.len());
        }
    }
});
