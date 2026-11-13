//! `lua` resources: precompiled Lua 5.0 chunks (`luac` output).
//!
//! Every script of The Godfather II is Lua 5.0 bytecode, little-endian, with
//! 4-byte `int`, `size_t` and instructions and **4-byte `float` numbers**
//! (`lua_Number` is `float`).
//!
//! ```text
//! 0x00  "\x1bLua"
//! 0x04  u8  version 0x50
//! 0x05  u8  1 = little-endian
//! 0x06  u8  sizeof(int), sizeof(size_t), sizeof(Instruction) = 4, 4, 4
//! 0x09  u8  SIZE_OP, SIZE_A, SIZE_B, SIZE_C = 6, 8, 9, 9
//! 0x0D  u8  sizeof(lua_Number) = 4
//! 0x0E  lua_Number 3.14159265358979e7 (format check)
//!       main function: source name, line, upvalue/parameter counts, line info,
//!       locals, upvalue names, constants (nil, number, string), nested functions, code
//! ```
//!
//! [`Chunk`] reads and writes every field, [`compile`] turns Lua 5.0 source
//! into a chunk exactly like `luac` 5.0, and [`decompile`] turns a chunk back
//! into source that compiles to the same instructions.

mod chunk;
mod compiler;
mod decompiler;
mod lexer;
pub mod opcodes;
pub mod vm;

use std::fmt;

use serde::Serialize;

pub use chunk::{Chunk, Constant, LocVar, Proto};
pub use compiler::compile;
pub use decompiler::{Comparison, Decompiled, RoundTrip, compare, decompile, round_trip};
pub use lexer::is_reserved;

use crate::Result;

/// Type id of `lua` resources.
pub const TYPE_LUA: u32 = crate::hash::sdbm(b"lua");

/// `sizeof(lua_Number)` in the game's chunks.
pub const GAME_NUMBER_SIZE: u8 = 4;

/// A syntax error found while compiling Lua source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    /// 1-based source line.
    pub line: u32,
    pub message: String,
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for CompileError {}

impl From<CompileError> for crate::Error {
    fn from(e: CompileError) -> Self {
        crate::Error::InvalidArgument(format!("Lua syntax error at {e}"))
    }
}

/// Compiles source for the game: float numbers and the given chunk name.
pub fn compile_for_game(source: &[u8], chunk_name: &[u8]) -> Result<Vec<u8>, CompileError> {
    Ok(compile(source, chunk_name, GAME_NUMBER_SIZE)?.to_bytes())
}

/// Summary of a parsed chunk.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LuaChunk {
    pub version: u8,
    pub number_size: u8,
    /// Source file name recorded by the compiler (`x:\assets\...\name.lua`).
    pub source: String,
    /// Number of functions including the main chunk.
    pub functions: usize,
    pub instructions: usize,
    /// String constants of all functions, in load order.
    pub strings: Vec<String>,
}

impl LuaChunk {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let chunk = Chunk::parse(data)?;
        let mut strings = Vec::new();
        collect_strings(&chunk.main, &mut strings);
        Ok(Self {
            version: 0x50,
            number_size: chunk.number_size,
            source: chunk.source(),
            functions: chunk.main.function_count(),
            instructions: chunk.main.instruction_count(),
            strings,
        })
    }

    /// File name of the source without its directory, e.g. `music_system.lua`.
    pub fn file_name(&self) -> &str {
        file_name(&self.source)
    }
}

/// File name of a chunk source without its directory.
pub fn file_name(source: &str) -> &str {
    source.rsplit(['\\', '/']).next().unwrap_or_default()
}

fn collect_strings(f: &Proto, out: &mut Vec<String>) {
    for k in &f.constants {
        if let Constant::String(s) = k {
            out.push(String::from_utf8_lossy(s).into_owned());
        }
    }
    for p in &f.protos {
        collect_strings(p, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chunk with one string constant and an empty nested function.
    fn sample() -> Vec<u8> {
        let mut d = b"\x1bLua\x50\x01\x04\x04\x04\x06\x08\x09\x09\x04".to_vec();
        d.extend_from_slice(&(3.141_592_653_589_793e7_f64 as f32).to_le_bytes());
        let int = |d: &mut Vec<u8>, v: u32| d.extend_from_slice(&v.to_le_bytes());
        let string = |d: &mut Vec<u8>, s: &str| {
            d.extend_from_slice(&(s.len() as u32 + 1).to_le_bytes());
            d.extend_from_slice(s.as_bytes());
            d.push(0);
        };
        string(&mut d, "x:\\scripts\\test.lua");
        int(&mut d, 0);
        d.extend_from_slice(&[0, 0, 1, 2]);
        int(&mut d, 1); // one line
        int(&mut d, 1);
        int(&mut d, 0); // locals
        int(&mut d, 0); // upvalues
        int(&mut d, 2); // constants
        d.push(4);
        string(&mut d, "hello");
        d.push(3);
        d.extend_from_slice(&1.5f32.to_le_bytes());
        int(&mut d, 1); // nested function
        int(&mut d, 0); // inherits the source name
        int(&mut d, 1);
        d.extend_from_slice(&[0, 0, 0, 2]);
        for _ in 0..4 {
            int(&mut d, 0); // lines, locals, upvalues, constants
        }
        int(&mut d, 0); // functions
        int(&mut d, 1); // code
        int(&mut d, 0x1E);
        int(&mut d, 1); // code of the main function
        int(&mut d, 0x1E);
        d
    }

    #[test]
    fn parses_a_chunk() {
        let chunk = LuaChunk::parse(&sample()).unwrap();
        assert_eq!(chunk.source, "x:\\scripts\\test.lua");
        assert_eq!(chunk.file_name(), "test.lua");
        assert_eq!(chunk.functions, 2);
        assert_eq!(chunk.instructions, 2);
        assert_eq!(chunk.strings, vec!["hello".to_owned()]);
    }

    #[test]
    fn writes_a_chunk_back() {
        let data = sample();
        assert_eq!(Chunk::parse(&data).unwrap().to_bytes(), data);
    }

    #[test]
    fn rejects_bad_input() {
        let data = sample();
        assert!(LuaChunk::parse(&data[..data.len() - 1]).is_err());
        let mut longer = data.clone();
        longer.push(0);
        assert!(LuaChunk::parse(&longer).is_err());
        let mut v51 = data.clone();
        v51[4] = 0x51;
        assert!(LuaChunk::parse(&v51).is_err());
        assert!(LuaChunk::parse(b"\x1bLu").is_err());
    }
}
