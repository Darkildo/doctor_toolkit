//! Lua 5.0 bytecode (`lua`).
#![no_main]

use doctor_toolkit::resource::lua::LuaChunk;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(chunk) = LuaChunk::parse(data) {
        let _ = chunk.file_name();
    }
});
