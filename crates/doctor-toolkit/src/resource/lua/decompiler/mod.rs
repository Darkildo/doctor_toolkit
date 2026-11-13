//! Lua 5.0 decompiler: bytecode back to source.
//!
//! The output keeps the line numbers of the original script where it can and
//! compiles (with [`compile`](super::compile)) to the same instructions; the
//! scripts of The Godfather II carry full debug information (local names,
//! upvalue names, lines), which the decompiler relies on.

mod ast;
mod build;

use std::fmt;

use super::chunk::{Chunk, Constant, Proto};
use super::compiler::compile;

/// Result of decompiling a chunk.
#[derive(Debug, Clone)]
pub struct Decompiled {
    pub source: String,
    /// Functions that could not be decompiled (their place holds a comment).
    pub errors: Vec<String>,
}

/// How a recompiled chunk relates to the original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Comparison {
    /// The bytes are identical.
    Identical,
    /// Same instructions, constants and functions; debug information
    /// (lines, local variable ranges) differs.
    SameCode,
    /// The code differs; the text names the first difference.
    Different(String),
}

impl fmt::Display for Comparison {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Comparison::Identical => f.write_str("byte-identical"),
            Comparison::SameCode => f.write_str("same instructions (debug info differs)"),
            Comparison::Different(what) => write!(f, "different: {what}"),
        }
    }
}

/// Decompiles a chunk to Lua source.
pub fn decompile(chunk: &Chunk) -> Decompiled {
    let float = chunk.number_size == 4;
    let mut errors = Vec::new();
    let mut printer = ast::Printer::new(float);
    match build::FnDec::new(&chunk.main, float).and_then(|d| d.function()) {
        Ok(f) => printer.chunk(&f.body),
        Err(e) => errors.push(format!("main function: {e}")),
    }
    let mut source = printer.finish();
    // The first statement is usually below line 1 (the original had comments
    // there): name the script on the free first line.
    if source.starts_with('\n') {
        let name = String::from_utf8_lossy(chunk.main.source.as_deref().unwrap_or_default())
            .replace(['\n', '\r'], " ");
        source.insert_str(0, &format!("-- {name} (decompiled by doctor_toolkit)"));
    }
    if !errors.is_empty() {
        let mut text = String::new();
        for e in &errors {
            text.push_str(&format!("-- decompiler error: {e}\n"));
        }
        text.push_str("--[[ listing:\n");
        text.push_str(&super::opcodes::listing(&chunk.main).replace("]]", "] ]"));
        text.push_str("]]\n");
        source = text;
    }
    Decompiled { source, errors }
}

/// Compares a recompiled chunk with the original.
pub fn compare(original: &Chunk, recompiled: &Chunk) -> Comparison {
    if original.to_bytes() == recompiled.to_bytes() {
        return Comparison::Identical;
    }
    if original.number_size != recompiled.number_size {
        return Comparison::Different("number size".into());
    }
    match compare_code(&original.main, &recompiled.main, "main") {
        Some(diff) => Comparison::Different(diff),
        None => Comparison::SameCode,
    }
}

fn compare_code(a: &Proto, b: &Proto, path: &str) -> Option<String> {
    if a.code != b.code {
        let pc = a
            .code
            .iter()
            .zip(&b.code)
            .position(|(x, y)| x != y)
            .unwrap_or(a.code.len().min(b.code.len()));
        return Some(format!(
            "{path}: code differs at instruction {} ({} vs {} instructions)",
            pc + 1,
            a.code.len(),
            b.code.len()
        ));
    }
    if !same_constants(&a.constants, &b.constants) {
        return Some(format!("{path}: constants differ"));
    }
    if (a.num_params, a.is_vararg, a.num_upvalues, a.max_stack)
        != (b.num_params, b.is_vararg, b.num_upvalues, b.max_stack)
    {
        return Some(format!("{path}: function header differs"));
    }
    if a.protos.len() != b.protos.len() {
        return Some(format!("{path}: number of functions differs"));
    }
    a.protos
        .iter()
        .zip(&b.protos)
        .enumerate()
        .find_map(|(i, (x, y))| compare_code(x, y, &format!("{path}.{i}")))
}

fn same_constants(a: &[Constant], b: &[Constant]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| match (x, y) {
            (Constant::Number(x), Constant::Number(y)) => x.to_bits() == y.to_bits(),
            _ => x == y,
        })
}

/// Result of decompiling a chunk and compiling the source again.
#[derive(Debug, Clone)]
pub struct RoundTrip {
    pub decompiled: Decompiled,
    /// The recompiled chunk, or the syntax error of the decompiled source.
    pub recompiled: Result<Chunk, super::CompileError>,
    pub comparison: Comparison,
}

/// Decompiles `chunk`, compiles the source with the same chunk name and number
/// size, and compares the result with the original.
pub fn round_trip(chunk: &Chunk) -> RoundTrip {
    let decompiled = decompile(chunk);
    let name = chunk.main.source.clone().unwrap_or_default();
    let recompiled = compile(decompiled.source.as_bytes(), &name, chunk.number_size);
    let comparison = match &recompiled {
        Ok(c) => compare(chunk, c),
        Err(e) => Comparison::Different(format!("decompiled source does not compile: {e}")),
    };
    RoundTrip {
        decompiled,
        recompiled,
        comparison,
    }
}
