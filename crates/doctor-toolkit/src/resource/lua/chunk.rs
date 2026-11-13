//! Full model of a Lua 5.0 binary chunk: read and write every field.
//!
//! The layout is the one of `ldump.c` / `lundump.c` in Lua 5.0. Only
//! little-endian chunks with 4-byte `int`, `size_t` and instructions are
//! accepted; numbers are 4-byte floats (The Godfather II) or 8-byte doubles.

use crate::io::{self, Endian};
use crate::{Error, Result};

pub(crate) const SIGNATURE: &[u8; 4] = b"\x1bLua";
const VERSION: u8 = 0x50;
const MAX_DEPTH: usize = 200;
/// Value stored after the header to check the number format.
const TEST_NUMBER: f64 = 3.141_592_653_589_793e7;

/// Constant of a function.
#[derive(Debug, Clone, PartialEq)]
pub enum Constant {
    Nil,
    /// A number; for float chunks the value is exactly representable as `f32`.
    Number(f64),
    String(Vec<u8>),
}

/// Debug information of a local variable: its name and the instructions where it is active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocVar {
    pub name: Vec<u8>,
    /// First instruction where the variable is active.
    pub start_pc: u32,
    /// First instruction where the variable is dead.
    pub end_pc: u32,
}

/// A function prototype.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Proto {
    /// Source name; `None` in nested functions that inherit the parent's name.
    pub source: Option<Vec<u8>>,
    pub line_defined: u32,
    pub num_upvalues: u8,
    pub num_params: u8,
    pub is_vararg: bool,
    pub max_stack: u8,
    /// Source line of every instruction (empty when stripped).
    pub line_info: Vec<u32>,
    pub locals: Vec<LocVar>,
    /// Upvalue names (empty when stripped).
    pub upvalue_names: Vec<Vec<u8>>,
    pub constants: Vec<Constant>,
    pub protos: Vec<Proto>,
    pub code: Vec<u32>,
}

/// A whole chunk: the number format and the main function.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    /// `sizeof(lua_Number)`: 4 (float) or 8 (double).
    pub number_size: u8,
    pub main: Proto,
}

impl Proto {
    /// Number of functions including this one.
    pub fn function_count(&self) -> usize {
        1 + self.protos.iter().map(Proto::function_count).sum::<usize>()
    }

    /// Number of instructions in this function and every nested one.
    pub fn instruction_count(&self) -> usize {
        self.code.len()
            + self
                .protos
                .iter()
                .map(Proto::instruction_count)
                .sum::<usize>()
    }
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
    number_size: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8> {
        let b = io::slice(self.data, self.at, 1)?[0];
        self.at += 1;
        Ok(b)
    }

    fn int(&mut self) -> Result<u32> {
        let v = io::read_u32(self.data, self.at, Endian::Little)?;
        self.at += 4;
        Ok(v)
    }

    /// A count of items at least `min_size` bytes each, checked against the remaining data.
    fn count(&mut self, min_size: usize) -> Result<usize> {
        let n = self.int()? as usize;
        if n.saturating_mul(min_size) > self.data.len().saturating_sub(self.at) {
            return Err(Error::malformed("lua", self.at - 4, format!("count {n}")));
        }
        Ok(n)
    }

    fn number(&mut self) -> Result<f64> {
        let bytes = io::slice(self.data, self.at, self.number_size)?;
        self.at += self.number_size;
        Ok(match self.number_size {
            4 => f32::from_le_bytes(bytes.try_into().expect("4 bytes")) as f64,
            _ => f64::from_le_bytes(bytes.try_into().expect("8 bytes")),
        })
    }

    fn string(&mut self) -> Result<Option<Vec<u8>>> {
        let len = self.int()? as usize;
        if len == 0 {
            return Ok(None);
        }
        let bytes = io::slice(self.data, self.at, len)?;
        let at = self.at;
        self.at += len;
        match bytes.split_last() {
            Some((0, text)) => Ok(Some(text.to_vec())),
            _ => Err(Error::malformed(
                "lua",
                at,
                "string without a terminating NUL",
            )),
        }
    }

    fn name(&mut self) -> Result<Vec<u8>> {
        Ok(self.string()?.unwrap_or_default())
    }

    fn function(&mut self, depth: usize) -> Result<Proto> {
        if depth > MAX_DEPTH {
            return Err(Error::malformed(
                "lua",
                self.at,
                "functions nested too deeply",
            ));
        }
        let source = self.string()?;
        let line_defined = self.int()?;
        let num_upvalues = self.byte()?;
        let num_params = self.byte()?;
        let is_vararg = self.byte()? != 0;
        let max_stack = self.byte()?;
        let lines = self.count(4)?;
        let line_info = (0..lines).map(|_| self.int()).collect::<Result<_>>()?;
        let locals = (0..self.count(12)?)
            .map(|_| {
                Ok(LocVar {
                    name: self.name()?,
                    start_pc: self.int()?,
                    end_pc: self.int()?,
                })
            })
            .collect::<Result<_>>()?;
        let upvalue_names = (0..self.count(4)?)
            .map(|_| self.name())
            .collect::<Result<_>>()?;
        let mut constants = Vec::new();
        for _ in 0..self.count(1)? {
            constants.push(match self.byte()? {
                0 => Constant::Nil,
                3 => Constant::Number(self.number()?),
                4 => Constant::String(self.name()?),
                other => {
                    return Err(Error::malformed(
                        "lua",
                        self.at - 1,
                        format!("constant type {other}"),
                    ));
                }
            });
        }
        let protos = (0..self.count(20)?)
            .map(|_| self.function(depth + 1))
            .collect::<Result<_>>()?;
        let code = (0..self.count(4)?)
            .map(|_| self.int())
            .collect::<Result<_>>()?;
        Ok(Proto {
            source,
            line_defined,
            num_upvalues,
            num_params,
            is_vararg,
            max_stack,
            line_info,
            locals,
            upvalue_names,
            constants,
            protos,
            code,
        })
    }
}

/// Checks the 14-byte header and returns `sizeof(lua_Number)`.
pub(crate) fn parse_header(data: &[u8]) -> Result<u8> {
    if io::slice(data, 0, 4)? != SIGNATURE {
        return Err(Error::BadMagic {
            what: "lua",
            found: io::read_u32(data, 0, Endian::Big)?,
        });
    }
    let header = io::slice(data, 4, 10)?;
    if header[0] != VERSION {
        return Err(Error::UnsupportedVersion {
            what: "lua",
            version: header[0] as u32,
        });
    }
    if header[1] != 1 || header[2..5] != [4, 4, 4] {
        return Err(Error::malformed(
            "lua",
            5,
            "not a 32-bit little-endian chunk",
        ));
    }
    if header[5..9] != [6, 8, 9, 9] {
        return Err(Error::malformed("lua", 9, "instruction layout"));
    }
    let number_size = header[9];
    if number_size != 4 && number_size != 8 {
        return Err(Error::malformed("lua", 13, "number size"));
    }
    Ok(number_size)
}

impl Chunk {
    /// Parses a chunk; the data must end with the main function.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let number_size = parse_header(data)?;
        let mut reader = Reader {
            data,
            at: 14,
            number_size: number_size as usize,
        };
        let test = reader.number()?;
        let expected = match number_size {
            4 => TEST_NUMBER as f32 as f64,
            _ => TEST_NUMBER,
        };
        if test as i64 != expected as i64 {
            return Err(Error::malformed("lua", 14, "number format check failed"));
        }
        let main = reader.function(0)?;
        if reader.at != data.len() {
            return Err(Error::malformed(
                "lua",
                reader.at,
                "data after the main function",
            ));
        }
        Ok(Self { number_size, main })
    }

    /// Writes the chunk in the format `parse` reads.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer {
            out: Vec::new(),
            number_size: self.number_size,
        };
        w.out.extend_from_slice(SIGNATURE);
        w.out
            .extend_from_slice(&[VERSION, 1, 4, 4, 4, 6, 8, 9, 9, self.number_size]);
        w.number(TEST_NUMBER);
        w.function(&self.main);
        w.out
    }

    /// Source name of the main function as text.
    pub fn source(&self) -> String {
        String::from_utf8_lossy(self.main.source.as_deref().unwrap_or_default()).into_owned()
    }
}

struct Writer {
    out: Vec<u8>,
    number_size: u8,
}

impl Writer {
    fn int(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    fn len(&mut self, n: usize) {
        self.int(u32::try_from(n).expect("Lua chunks are smaller than 4 GiB"));
    }

    fn number(&mut self, v: f64) {
        if self.number_size == 4 {
            self.out.extend_from_slice(&(v as f32).to_le_bytes());
        } else {
            self.out.extend_from_slice(&v.to_le_bytes());
        }
    }

    fn string(&mut self, s: Option<&[u8]>) {
        match s {
            None => self.int(0),
            Some(s) => {
                self.len(s.len() + 1);
                self.out.extend_from_slice(s);
                self.out.push(0);
            }
        }
    }

    fn function(&mut self, f: &Proto) {
        self.string(f.source.as_deref());
        self.int(f.line_defined);
        self.out
            .extend_from_slice(&[f.num_upvalues, f.num_params, f.is_vararg as u8, f.max_stack]);
        self.len(f.line_info.len());
        for &line in &f.line_info {
            self.int(line);
        }
        self.len(f.locals.len());
        for local in &f.locals {
            self.string(Some(&local.name));
            self.int(local.start_pc);
            self.int(local.end_pc);
        }
        self.len(f.upvalue_names.len());
        for name in &f.upvalue_names {
            self.string(Some(name));
        }
        self.len(f.constants.len());
        for k in &f.constants {
            match k {
                Constant::Nil => self.out.push(0),
                Constant::Number(n) => {
                    self.out.push(3);
                    self.number(*n);
                }
                Constant::String(s) => {
                    self.out.push(4);
                    self.string(Some(s));
                }
            }
        }
        self.len(f.protos.len());
        for p in &f.protos {
            self.function(p);
        }
        self.len(f.code.len());
        for &i in &f.code {
            self.int(i);
        }
    }
}
