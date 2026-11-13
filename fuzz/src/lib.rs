//! Helpers shared by the fuzz targets.

use std::path::PathBuf;

use doctor_toolkit::resource::tg4::{PARAM_DATA_SIZE, PARAM_FORMAT, ParamValue, Tg4Header};

/// Splits `data` in two: the first two bytes (little endian) give the length
/// of the first part, clamped to what is available.
pub fn split2(data: &[u8]) -> (&[u8], &[u8]) {
    if data.len() < 2 {
        return (&[], data);
    }
    let len = usize::from(u16::from_le_bytes([data[0], data[1]]));
    let rest = &data[2..];
    rest.split_at(len.min(rest.len()))
}

/// Splits `data` into up to `n` parts separated by the byte string `\0--\0`.
pub fn split_parts(data: &[u8], n: usize) -> Vec<&[u8]> {
    const SEP: &[u8] = b"\0--\0";
    let mut parts = Vec::with_capacity(n);
    let mut rest = data;
    while parts.len() + 1 < n {
        match rest.windows(SEP.len()).position(|w| w == SEP) {
            Some(at) => {
                parts.push(&rest[..at]);
                rest = &rest[at + SEP.len()..];
            }
            None => break,
        }
    }
    parts.push(rest);
    parts
}

/// A scratch directory private to this process, under `$TMPDIR`.
pub fn scratch_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("doctor-toolkit-fuzz-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create the scratch directory");
    dir
}

/// Writes `data` to the file `name` of the scratch directory and returns its path.
pub fn scratch_file(name: &str, data: &[u8]) -> PathBuf {
    let path = scratch_dir().join(name);
    std::fs::write(&path, data).expect("write a scratch file");
    path
}

/// A flat texture header like the ones in the game.
pub fn tg4_header(format: &str, width: u16, height: u16, mips: u8) -> Tg4Header {
    let mut header = Tg4Header {
        version: 3,
        budget_hash: 0,
        package: "fuzz".into(),
        name_hash: 7,
        name: "fuzz_texture".into(),
        data_size: 0,
        width,
        height,
        padding: 0,
        surface_count: mips,
        params: Vec::new(),
    };
    header.set_param(PARAM_FORMAT, ParamValue::Str(format.into()));
    header.data_size = header.expected_data_size().expect("known format") as u32;
    header.set_param(PARAM_DATA_SIZE, ParamValue::Int(header.data_size));
    header
}
