//! Lua 5.0 instruction set (`lopcodes.h`) and a `luac -l` style listing.

use std::fmt::Write as _;

use super::chunk::{Constant, Proto};

/// Opcodes of Lua 5.0, in encoding order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Op {
    Move,
    LoadK,
    LoadBool,
    LoadNil,
    GetUpval,
    GetGlobal,
    GetTable,
    SetGlobal,
    SetUpval,
    SetTable,
    NewTable,
    SelfOp,
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Unm,
    Not,
    Concat,
    Jmp,
    Eq,
    Lt,
    Le,
    Test,
    Call,
    TailCall,
    Return,
    ForLoop,
    TForLoop,
    TForPrep,
    SetList,
    SetListO,
    Close,
    Closure,
}

const OPS: [Op; 35] = [
    Op::Move,
    Op::LoadK,
    Op::LoadBool,
    Op::LoadNil,
    Op::GetUpval,
    Op::GetGlobal,
    Op::GetTable,
    Op::SetGlobal,
    Op::SetUpval,
    Op::SetTable,
    Op::NewTable,
    Op::SelfOp,
    Op::Add,
    Op::Sub,
    Op::Mul,
    Op::Div,
    Op::Pow,
    Op::Unm,
    Op::Not,
    Op::Concat,
    Op::Jmp,
    Op::Eq,
    Op::Lt,
    Op::Le,
    Op::Test,
    Op::Call,
    Op::TailCall,
    Op::Return,
    Op::ForLoop,
    Op::TForLoop,
    Op::TForPrep,
    Op::SetList,
    Op::SetListO,
    Op::Close,
    Op::Closure,
];

/// Registers below this value are registers; `RK` operands at or above it are constants.
pub const MAXSTACK: u32 = 250;
pub const MAXARG_BX: u32 = (1 << 18) - 1;
pub const MAXARG_SBX: i32 = (MAXARG_BX >> 1) as i32;
pub const MAXARG_A: u32 = 255;
pub const MAXARG_C: u32 = 511;
/// `NO_REG` of the code generator: a `TEST` that does not produce a value.
pub const NO_REG: u32 = MAXARG_A;
/// List items stored by one `SETLIST`.
pub const FIELDS_PER_FLUSH: u32 = 32;

/// Instruction formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Abc,
    ABx,
    AsBx,
}

impl Op {
    pub fn from_u32(i: u32) -> Option<Op> {
        OPS.get((i & 0x3F) as usize).copied()
    }

    pub fn name(self) -> &'static str {
        [
            "MOVE",
            "LOADK",
            "LOADBOOL",
            "LOADNIL",
            "GETUPVAL",
            "GETGLOBAL",
            "GETTABLE",
            "SETGLOBAL",
            "SETUPVAL",
            "SETTABLE",
            "NEWTABLE",
            "SELF",
            "ADD",
            "SUB",
            "MUL",
            "DIV",
            "POW",
            "UNM",
            "NOT",
            "CONCAT",
            "JMP",
            "EQ",
            "LT",
            "LE",
            "TEST",
            "CALL",
            "TAILCALL",
            "RETURN",
            "FORLOOP",
            "TFORLOOP",
            "TFORPREP",
            "SETLIST",
            "SETLISTO",
            "CLOSE",
            "CLOSURE",
        ][self as usize]
    }

    pub fn mode(self) -> Mode {
        match self {
            Op::LoadK | Op::GetGlobal | Op::SetGlobal | Op::SetList | Op::SetListO => Mode::ABx,
            Op::Closure => Mode::ABx,
            Op::Jmp | Op::ForLoop | Op::TForPrep => Mode::AsBx,
            _ => Mode::Abc,
        }
    }

    /// Comparison or test: the next instruction is a jump.
    pub fn is_test(self) -> bool {
        matches!(self, Op::Eq | Op::Lt | Op::Le | Op::Test)
    }
}

/// Field accessors and constructors of 32-bit instructions.
pub mod ins {
    use super::{MAXARG_SBX, Op};

    pub fn op(i: u32) -> Option<Op> {
        Op::from_u32(i)
    }
    pub fn a(i: u32) -> u32 {
        i >> 24
    }
    pub fn b(i: u32) -> u32 {
        (i >> 15) & 0x1FF
    }
    pub fn c(i: u32) -> u32 {
        (i >> 6) & 0x1FF
    }
    pub fn bx(i: u32) -> u32 {
        (i >> 6) & 0x3FFFF
    }
    pub fn sbx(i: u32) -> i32 {
        bx(i) as i32 - MAXARG_SBX
    }
    pub fn abc(op: Op, a: u32, b: u32, c: u32) -> u32 {
        op as u32 | (a << 24) | ((b & 0x1FF) << 15) | ((c & 0x1FF) << 6)
    }
    pub fn abx(op: Op, a: u32, bx: u32) -> u32 {
        op as u32 | (a << 24) | ((bx & 0x3FFFF) << 6)
    }
    pub fn set_a(i: &mut u32, v: u32) {
        *i = (*i & 0x00FF_FFFF) | ((v & 0xFF) << 24);
    }
    pub fn set_b(i: &mut u32, v: u32) {
        *i = (*i & !(0x1FF << 15)) | ((v & 0x1FF) << 15);
    }
    pub fn set_c(i: &mut u32, v: u32) {
        *i = (*i & !(0x1FF << 6)) | ((v & 0x1FF) << 6);
    }
    pub fn set_bx(i: &mut u32, v: u32) {
        *i = (*i & 0xFF00_003F) | ((v & 0x3FFFF) << 6);
    }
    pub fn set_sbx(i: &mut u32, v: i32) {
        set_bx(i, (v + MAXARG_SBX) as u32);
    }
    pub fn set_op(i: &mut u32, op: Op) {
        *i = (*i & !0x3F) | op as u32;
    }
}

/// Lua-style text of a number constant (`%.14g`-like, shortest form that reads back).
pub fn number_text(n: f64) -> String {
    if n == n.trunc() && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Text of a constant for listings and comments.
pub fn constant_text(k: &Constant) -> String {
    match k {
        Constant::Nil => "nil".to_owned(),
        Constant::Number(n) => number_text(*n),
        Constant::String(s) => format!("{:?}", String::from_utf8_lossy(s)),
    }
}

/// Disassembly of a function and its nested functions, one instruction per line.
pub fn listing(f: &Proto) -> String {
    let mut out = String::new();
    list_function(f, "main", &mut out);
    out
}

fn list_function(f: &Proto, path: &str, out: &mut String) {
    let _ = writeln!(
        out,
        "function {path} (line {}): {} params{}, {} upvalues, {} stack, {} locals, {} constants, {} functions, {} instructions",
        f.line_defined,
        f.num_params,
        if f.is_vararg { " + vararg" } else { "" },
        f.num_upvalues,
        f.max_stack,
        f.locals.len(),
        f.constants.len(),
        f.protos.len(),
        f.code.len(),
    );
    let rk = |x: u32| -> String {
        if x >= MAXSTACK {
            f.constants
                .get((x - MAXSTACK) as usize)
                .map(constant_text)
                .unwrap_or_else(|| format!("K{}", x - MAXSTACK))
        } else {
            format!("R{x}")
        }
    };
    let kst = |x: u32| -> String {
        f.constants
            .get(x as usize)
            .map(constant_text)
            .unwrap_or_else(|| format!("K{x}"))
    };
    for (pc, &i) in f.code.iter().enumerate() {
        let line = f
            .line_info
            .get(pc)
            .map(|l| l.to_string())
            .unwrap_or_default();
        let Some(op) = ins::op(i) else {
            let _ = writeln!(out, "  {:>4} [{line:>4}] ??? {i:#010x}", pc + 1);
            continue;
        };
        let (a, b, c) = (ins::a(i), ins::b(i), ins::c(i));
        let args = match op.mode() {
            Mode::Abc => format!("{a} {b} {c}"),
            Mode::ABx => format!("{a} {}", ins::bx(i)),
            Mode::AsBx => format!("{a} {}", ins::sbx(i)),
        };
        let note = match op {
            Op::LoadK | Op::GetGlobal | Op::SetGlobal => kst(ins::bx(i)),
            Op::GetTable | Op::SelfOp => rk(c),
            Op::SetTable => format!("{} {}", rk(b), rk(c)),
            Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Pow | Op::Eq | Op::Lt | Op::Le => {
                format!("{} {}", rk(b), rk(c))
            }
            Op::GetUpval | Op::SetUpval => f
                .upvalue_names
                .get(b as usize)
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .unwrap_or_default(),
            Op::Jmp | Op::ForLoop | Op::TForPrep => {
                format!("to {}", pc as i64 + 2 + ins::sbx(i) as i64)
            }
            Op::Closure => format!("function {path}.{}", ins::bx(i)),
            _ => String::new(),
        };
        let _ = writeln!(
            out,
            "  {:>4} [{line:>4}] {:<9} {args:<12}{}{note}",
            pc + 1,
            op.name(),
            if note.is_empty() { "" } else { " ; " }
        );
    }
    for v in &f.locals {
        let _ = writeln!(
            out,
            "  local {} [{}, {})",
            String::from_utf8_lossy(&v.name),
            v.start_pc + 1,
            v.end_pc + 1
        );
    }
    for (n, p) in f.protos.iter().enumerate() {
        out.push('\n');
        list_function(p, &format!("{path}.{n}"), out);
    }
}
