//! Lua 5.0 compiler: source text to a function prototype.
//!
//! This is a port of the parser and code generator of Lua 5.0.3 (`lparser.c`,
//! `lcode.c`); it emits the same instructions, constants, line and local
//! variable information as `luac` 5.0 built with the same number type, so a
//! script compiled here is byte-identical to one compiled by the game's
//! developers. Function names follow the C sources to keep them comparable.
//!
//! Lua 5.0.3 is Copyright (C) 1994-2006 Tecgraf, PUC-Rio, and distributed under
//! the MIT license:
//!
//! > Permission is hereby granted, free of charge, to any person obtaining a copy
//! > of this software and associated documentation files (the "Software"), to
//! > deal in the Software without restriction, including without limitation the
//! > rights to use, copy, modify, merge, publish, distribute, sublicense, and/or
//! > sell copies of the Software, and to permit persons to whom the Software is
//! > furnished to do so, subject to the following conditions:
//! >
//! > The above copyright notice and this permission notice shall be included in
//! > all copies or substantial portions of the Software.
//! >
//! > THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! > IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! > FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! > AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! > LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! > OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! > SOFTWARE.

mod grammar;

use std::collections::HashMap;
use std::rc::Rc;

use super::CompileError;
use super::chunk::{Chunk, Constant, LocVar, Proto};
use super::lexer::{Lexer, Tok};
use super::opcodes::{
    FIELDS_PER_FLUSH, MAXARG_BX, MAXARG_C, MAXARG_SBX, MAXSTACK, NO_REG, Op, ins,
};

const NO_JUMP: i32 = -1;
const MULTRET: i32 = -1;
const MAXVARS: usize = 200;
const MAXUPVALUES: usize = 32;
const MAXPARAMS: usize = 100;
const MAXPARSERLEVEL: u32 = 200;
const MAXEXPWHILE: usize = 100;
const UNARY_PRIORITY: i32 = 8;

type R<T> = Result<T, CompileError>;

/// Compiles Lua 5.0 source into a chunk.
///
/// `chunk_name` becomes the source name of the main function; `number_size`
/// is `sizeof(lua_Number)`: 4 for The Godfather II, 8 for stock Lua.
pub fn compile(source: &[u8], chunk_name: &[u8], number_size: u8) -> R<Chunk> {
    let float = number_size == 4;
    let mut p = Parser {
        lex: Lexer::new(source, float),
        t: Tok::Eos,
        lookahead: None,
        lastline: 1,
        fs: Vec::new(),
        nestlevel: 0,
    };
    p.open_func(Some(chunk_name.to_vec()));
    p.next()?;
    p.chunk()?;
    if p.t != Tok::Eos {
        return Err(p.syntax_error("<eof> expected"));
    }
    let (main, _) = p.close_func()?;
    Ok(Chunk { number_size, main })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ExpKind {
    Void,
    Nil,
    True,
    False,
    K,
    Local,
    Upval,
    Global,
    Indexed,
    Jmp,
    Relocable,
    NonReloc,
    Call,
}

#[derive(Debug, Clone, Copy)]
struct ExpDesc {
    k: ExpKind,
    info: i32,
    aux: i32,
    t: i32,
    f: i32,
}

impl ExpDesc {
    fn new(k: ExpKind, info: i32) -> Self {
        Self {
            k,
            info,
            aux: 0,
            t: NO_JUMP,
            f: NO_JUMP,
        }
    }

    fn has_jumps(&self) -> bool {
        self.t != self.f
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BinOpr {
    Add,
    Sub,
    Mult,
    Div,
    Pow,
    Concat,
    Ne,
    Eq,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinOpr {
    fn from_token(t: &Tok) -> Option<Self> {
        Some(match t {
            Tok::Char(b'+') => Self::Add,
            Tok::Char(b'-') => Self::Sub,
            Tok::Char(b'*') => Self::Mult,
            Tok::Char(b'/') => Self::Div,
            Tok::Char(b'^') => Self::Pow,
            Tok::Concat => Self::Concat,
            Tok::Ne => Self::Ne,
            Tok::Eq => Self::Eq,
            Tok::Char(b'<') => Self::Lt,
            Tok::Le => Self::Le,
            Tok::Char(b'>') => Self::Gt,
            Tok::Ge => Self::Ge,
            Tok::And => Self::And,
            Tok::Or => Self::Or,
            _ => return None,
        })
    }

    /// Left and right priority.
    fn priority(self) -> (i32, i32) {
        match self {
            Self::Add | Self::Sub => (6, 6),
            Self::Mult | Self::Div => (7, 7),
            Self::Pow => (10, 9),
            Self::Concat => (5, 4),
            Self::Ne | Self::Eq | Self::Lt | Self::Le | Self::Gt | Self::Ge => (3, 3),
            Self::And => (2, 2),
            Self::Or => (1, 1),
        }
    }
}

/// Key of the constant table: numbers by value (`0` and `-0` are equal), strings by bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum KKey {
    Nil,
    Number(u64),
    String(Rc<[u8]>),
}

struct BlockCnt {
    breaklist: i32,
    nactvar: usize,
    upval: bool,
    isbreakable: bool,
}

struct FuncState {
    f: Proto,
    h: HashMap<KKey, usize>,
    bl: Vec<BlockCnt>,
    lasttarget: i32,
    jpc: i32,
    freereg: i32,
    nactvar: usize,
    upvalues: Vec<(ExpKind, i32)>,
    actvar: Vec<usize>,
}

impl FuncState {
    fn new(f: Proto) -> Self {
        Self {
            f,
            h: HashMap::new(),
            bl: Vec::new(),
            lasttarget: 0,
            jpc: NO_JUMP,
            freereg: 0,
            nactvar: 0,
            upvalues: Vec::new(),
            actvar: Vec::new(),
        }
    }

    fn pc(&self) -> i32 {
        self.f.code.len() as i32
    }

    fn local(&mut self, i: usize) -> &mut LocVar {
        let index = self.actvar[i];
        &mut self.f.locals[index]
    }
}

struct Parser<'a> {
    lex: Lexer<'a>,
    t: Tok,
    lookahead: Option<Tok>,
    lastline: u32,
    fs: Vec<FuncState>,
    nestlevel: u32,
}

impl Parser<'_> {
    // ----------------------------------------------------------------- lexer glue

    fn fs(&mut self) -> &mut FuncState {
        self.fs.last_mut().expect("an open function")
    }

    fn next(&mut self) -> R<()> {
        self.lastline = self.lex.line;
        self.t = match self.lookahead.take() {
            Some(t) => t,
            None => self.lex.next_token()?,
        };
        Ok(())
    }

    fn peek(&mut self) -> R<&Tok> {
        if self.lookahead.is_none() {
            self.lookahead = Some(self.lex.next_token()?);
        }
        Ok(self.lookahead.as_ref().expect("just read"))
    }

    fn syntax_error(&self, msg: &str) -> CompileError {
        let near = match &self.t {
            Tok::Number(_) | Tok::String(_) => self.lex.raw.clone(),
            t => t.describe(),
        };
        CompileError {
            line: self.lex.line,
            message: format!("{msg} near `{near}'"),
        }
    }

    fn error_expected(&self, t: &Tok) -> CompileError {
        self.syntax_error(&format!("`{}' expected", t.describe()))
    }

    fn check_limit(&self, value: usize, limit: usize, what: &str) -> R<()> {
        if value > limit {
            return Err(self.syntax_error(&format!("too many {what} (limit={limit})")));
        }
        Ok(())
    }

    fn testnext(&mut self, t: &Tok) -> R<bool> {
        if &self.t == t {
            self.next()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn check(&mut self, t: &Tok) -> R<()> {
        if !self.testnext(t)? {
            return Err(self.error_expected(t));
        }
        Ok(())
    }

    fn check_condition(&self, ok: bool, msg: &str) -> R<()> {
        if ok {
            Ok(())
        } else {
            Err(self.syntax_error(msg))
        }
    }

    fn check_match(&mut self, what: Tok, who: Tok, line: u32) -> R<()> {
        if !self.testnext(&what)? {
            if line == self.lex.line {
                return Err(self.error_expected(&what));
            }
            return Err(self.syntax_error(&format!(
                "`{}' expected (to close `{}' at line {line})",
                what.describe(),
                who.describe()
            )));
        }
        Ok(())
    }

    fn str_checkname(&mut self) -> R<Rc<[u8]>> {
        let Tok::Name(name) = &self.t else {
            return Err(self.syntax_error("<name> expected"));
        };
        let name = name.clone();
        self.next()?;
        Ok(name)
    }

    fn enter_level(&mut self) -> R<()> {
        self.nestlevel += 1;
        if self.nestlevel > MAXPARSERLEVEL {
            return Err(self.syntax_error("too many syntax levels"));
        }
        Ok(())
    }

    // ----------------------------------------------------------- code generator

    fn code(&mut self, i: u32, line: u32) -> R<i32> {
        self.dischargejpc()?;
        let fs = self.fs();
        fs.f.code.push(i);
        fs.f.line_info.push(line);
        Ok(fs.pc() - 1)
    }

    fn code_abc(&mut self, op: Op, a: i32, b: i32, c: i32) -> R<i32> {
        self.code(ins::abc(op, a as u32, b as u32, c as u32), self.lastline)
    }

    fn code_abx(&mut self, op: Op, a: i32, bx: u32) -> R<i32> {
        self.code(ins::abx(op, a as u32, bx), self.lastline)
    }

    fn code_asbx(&mut self, op: Op, a: i32, sbx: i32) -> R<i32> {
        self.code_abx(op, a, (sbx + MAXARG_SBX) as u32)
    }

    fn fixline(&mut self, line: u32) {
        if let Some(last) = self.fs().f.line_info.last_mut() {
            *last = line;
        }
    }

    fn instr(&mut self, pc: i32) -> &mut u32 {
        &mut self.fs().f.code[pc as usize]
    }

    fn code_nil(&mut self, from: i32, n: i32) -> R<()> {
        let fs = self.fs();
        if fs.pc() > fs.lasttarget {
            let previous = *fs.f.code.last().expect("pc > lasttarget >= 0");
            if ins::op(previous) == Some(Op::LoadNil) {
                let pfrom = ins::a(previous) as i32;
                let pto = ins::b(previous) as i32;
                if pfrom <= from && from <= pto + 1 {
                    if from + n - 1 > pto {
                        let last = fs.f.code.last_mut().expect("checked");
                        ins::set_b(last, (from + n - 1) as u32);
                    }
                    return Ok(());
                }
            }
        }
        self.code_abc(Op::LoadNil, from, from + n - 1, 0)?;
        Ok(())
    }

    fn jump(&mut self) -> R<i32> {
        let jpc = self.fs().jpc;
        self.fs().jpc = NO_JUMP;
        let j = self.code_asbx(Op::Jmp, 0, NO_JUMP)?;
        self.concat(j, jpc)
    }

    fn condjump(&mut self, op: Op, a: i32, b: i32, c: i32) -> R<i32> {
        self.code_abc(op, a, b, c)?;
        self.jump()
    }

    fn fixjump(&mut self, pc: i32, dest: i32) -> R<()> {
        let offset = dest - (pc + 1);
        if offset.abs() > MAXARG_SBX {
            return Err(self.syntax_error("control structure too long"));
        }
        ins::set_sbx(self.instr(pc), offset);
        Ok(())
    }

    fn getlabel(&mut self) -> i32 {
        let fs = self.fs();
        fs.lasttarget = fs.pc();
        fs.lasttarget
    }

    fn getjump(&mut self, pc: i32) -> i32 {
        let offset = ins::sbx(*self.instr(pc));
        if offset == NO_JUMP {
            NO_JUMP
        } else {
            pc + 1 + offset
        }
    }

    fn jumpcontrol(&mut self, pc: i32) -> i32 {
        if pc >= 1 && ins::op(*self.instr(pc - 1)).is_some_and(Op::is_test) {
            pc - 1
        } else {
            pc
        }
    }

    fn need_value(&mut self, mut list: i32, cond: u32) -> bool {
        while list != NO_JUMP {
            let control = self.jumpcontrol(list);
            let i = *self.instr(control);
            if ins::op(i) != Some(Op::Test) || ins::a(i) != NO_REG || ins::c(i) != cond {
                return true;
            }
            list = self.getjump(list);
        }
        false
    }

    fn patchtestreg(i: &mut u32, reg: u32) {
        let reg = if reg == NO_REG { ins::b(*i) } else { reg };
        ins::set_a(i, reg);
    }

    fn removevalues(&mut self, mut list: i32) {
        while list != NO_JUMP {
            let control = self.jumpcontrol(list);
            let i = self.instr(control);
            if ins::op(*i) == Some(Op::Test) {
                Self::patchtestreg(i, NO_REG);
            }
            list = self.getjump(list);
        }
    }

    fn patchlistaux(&mut self, mut list: i32, vtarget: i32, reg: u32, dtarget: i32) -> R<()> {
        while list != NO_JUMP {
            let next = self.getjump(list);
            let control = self.jumpcontrol(list);
            let i = self.instr(control);
            if ins::op(*i) == Some(Op::Test) && ins::a(*i) == NO_REG {
                Self::patchtestreg(i, reg);
                self.fixjump(list, vtarget)?;
            } else {
                self.fixjump(list, dtarget)?;
            }
            list = next;
        }
        Ok(())
    }

    fn dischargejpc(&mut self) -> R<()> {
        let (jpc, pc) = {
            let fs = self.fs();
            (fs.jpc, fs.pc())
        };
        self.patchlistaux(jpc, pc, NO_REG, pc)?;
        self.fs().jpc = NO_JUMP;
        Ok(())
    }

    fn patchlist(&mut self, list: i32, target: i32) -> R<()> {
        if target == self.fs().pc() {
            self.patchtohere(list)
        } else {
            self.patchlistaux(list, target, NO_REG, target)
        }
    }

    fn patchtohere(&mut self, list: i32) -> R<()> {
        self.getlabel();
        let jpc = self.fs().jpc;
        self.fs().jpc = self.concat(jpc, list)?;
        Ok(())
    }

    /// Appends list `l2` to list `l1` and returns the new head of `l1`.
    fn concat(&mut self, l1: i32, l2: i32) -> R<i32> {
        if l2 == NO_JUMP {
            return Ok(l1);
        }
        if l1 == NO_JUMP {
            return Ok(l2);
        }
        let mut list = l1;
        loop {
            let next = self.getjump(list);
            if next == NO_JUMP {
                break;
            }
            list = next;
        }
        self.fixjump(list, l2)?;
        Ok(l1)
    }

    fn checkstack(&mut self, n: i32) -> R<()> {
        let newstack = self.fs().freereg + n;
        if newstack > self.fs().f.max_stack as i32 {
            if newstack >= MAXSTACK as i32 {
                return Err(self.syntax_error("function or expression too complex"));
            }
            self.fs().f.max_stack = newstack as u8;
        }
        Ok(())
    }

    fn reserveregs(&mut self, n: i32) -> R<()> {
        self.checkstack(n)?;
        self.fs().freereg += n;
        Ok(())
    }

    fn freereg(&mut self, reg: i32) {
        let fs = self.fs();
        if reg >= fs.nactvar as i32 && reg < MAXSTACK as i32 {
            fs.freereg -= 1;
            debug_assert_eq!(reg, fs.freereg);
        }
    }

    fn freeexp(&mut self, e: &ExpDesc) {
        if e.k == ExpKind::NonReloc {
            self.freereg(e.info);
        }
    }

    fn addk(&mut self, key: KKey, value: Constant) -> R<i32> {
        if let Some(&index) = self.fs().h.get(&key) {
            return Ok(index as i32);
        }
        let n = self.fs().f.constants.len();
        if n >= MAXARG_BX as usize {
            return Err(self.syntax_error("constant table overflow"));
        }
        let fs = self.fs();
        fs.f.constants.push(value);
        fs.h.insert(key, n);
        Ok(n as i32)
    }

    fn string_k(&mut self, s: &Rc<[u8]>) -> R<i32> {
        self.addk(KKey::String(s.clone()), Constant::String(s.to_vec()))
    }

    fn number_k(&mut self, r: f64) -> R<i32> {
        self.addk(KKey::Number((r + 0.0).to_bits()), Constant::Number(r))
    }

    fn nil_k(&mut self) -> R<i32> {
        self.addk(KKey::Nil, Constant::Nil)
    }

    fn setcallreturns(&mut self, e: &mut ExpDesc, nresults: i32) {
        if e.k == ExpKind::Call {
            let i = self.instr(e.info);
            ins::set_c(i, (nresults + 1) as u32);
            if nresults == 1 {
                e.k = ExpKind::NonReloc;
                e.info = ins::a(*i) as i32;
            }
        }
    }

    fn dischargevars(&mut self, e: &mut ExpDesc) -> R<()> {
        match e.k {
            ExpKind::Local => e.k = ExpKind::NonReloc,
            ExpKind::Upval => {
                e.info = self.code_abc(Op::GetUpval, 0, e.info, 0)?;
                e.k = ExpKind::Relocable;
            }
            ExpKind::Global => {
                e.info = self.code_abx(Op::GetGlobal, 0, e.info as u32)?;
                e.k = ExpKind::Relocable;
            }
            ExpKind::Indexed => {
                self.freereg(e.aux);
                self.freereg(e.info);
                e.info = self.code_abc(Op::GetTable, 0, e.info, e.aux)?;
                e.k = ExpKind::Relocable;
            }
            ExpKind::Call => self.setcallreturns(e, 1),
            _ => {}
        }
        Ok(())
    }

    fn code_label(&mut self, a: i32, b: i32, jump: i32) -> R<i32> {
        self.getlabel();
        self.code_abc(Op::LoadBool, a, b, jump)
    }

    fn discharge2reg(&mut self, e: &mut ExpDesc, reg: i32) -> R<()> {
        self.dischargevars(e)?;
        match e.k {
            ExpKind::Nil => self.code_nil(reg, 1)?,
            ExpKind::False | ExpKind::True => {
                self.code_abc(Op::LoadBool, reg, (e.k == ExpKind::True) as i32, 0)?;
            }
            ExpKind::K => {
                self.code_abx(Op::LoadK, reg, e.info as u32)?;
            }
            ExpKind::Relocable => ins::set_a(self.instr(e.info), reg as u32),
            ExpKind::NonReloc => {
                if reg != e.info {
                    self.code_abc(Op::Move, reg, e.info, 0)?;
                }
            }
            _ => return Ok(()),
        }
        e.info = reg;
        e.k = ExpKind::NonReloc;
        Ok(())
    }

    fn discharge2anyreg(&mut self, e: &mut ExpDesc) -> R<()> {
        if e.k != ExpKind::NonReloc {
            self.reserveregs(1)?;
            let reg = self.fs().freereg - 1;
            self.discharge2reg(e, reg)?;
        }
        Ok(())
    }

    fn exp2reg(&mut self, e: &mut ExpDesc, reg: i32) -> R<()> {
        self.discharge2reg(e, reg)?;
        if e.k == ExpKind::Jmp {
            e.t = self.concat(e.t, e.info)?;
        }
        if e.has_jumps() {
            let mut p_f = NO_JUMP;
            let mut p_t = NO_JUMP;
            if self.need_value(e.t, 1) || self.need_value(e.f, 0) {
                let fj = if e.k == ExpKind::Jmp {
                    NO_JUMP
                } else {
                    self.jump()?
                };
                p_f = self.code_label(reg, 0, 1)?;
                p_t = self.code_label(reg, 1, 0)?;
                self.patchtohere(fj)?;
            }
            let final_ = self.getlabel();
            self.patchlistaux(e.f, final_, reg as u32, p_f)?;
            self.patchlistaux(e.t, final_, reg as u32, p_t)?;
        }
        e.f = NO_JUMP;
        e.t = NO_JUMP;
        e.info = reg;
        e.k = ExpKind::NonReloc;
        Ok(())
    }

    fn exp2nextreg(&mut self, e: &mut ExpDesc) -> R<()> {
        self.dischargevars(e)?;
        self.freeexp(e);
        self.reserveregs(1)?;
        let reg = self.fs().freereg - 1;
        self.exp2reg(e, reg)
    }

    fn exp2anyreg(&mut self, e: &mut ExpDesc) -> R<i32> {
        self.dischargevars(e)?;
        if e.k == ExpKind::NonReloc {
            if !e.has_jumps() {
                return Ok(e.info);
            }
            if e.info >= self.fs().nactvar as i32 {
                self.exp2reg(e, e.info)?;
                return Ok(e.info);
            }
        }
        self.exp2nextreg(e)?;
        Ok(e.info)
    }

    fn exp2val(&mut self, e: &mut ExpDesc) -> R<()> {
        if e.has_jumps() {
            self.exp2anyreg(e)?;
            Ok(())
        } else {
            self.dischargevars(e)
        }
    }

    fn exp2rk(&mut self, e: &mut ExpDesc) -> R<i32> {
        self.exp2val(e)?;
        match e.k {
            ExpKind::Nil => {
                if self.fs().f.constants.len() as u32 + MAXSTACK <= MAXARG_C {
                    e.info = self.nil_k()?;
                    e.k = ExpKind::K;
                    return Ok(e.info + MAXSTACK as i32);
                }
            }
            ExpKind::K if e.info as u32 + MAXSTACK <= MAXARG_C => {
                return Ok(e.info + MAXSTACK as i32);
            }
            _ => {}
        }
        self.exp2anyreg(e)
    }

    fn storevar(&mut self, var: &ExpDesc, ex: &mut ExpDesc) -> R<()> {
        match var.k {
            ExpKind::Local => {
                self.freeexp(ex);
                return self.exp2reg(ex, var.info);
            }
            ExpKind::Upval => {
                let e = self.exp2anyreg(ex)?;
                self.code_abc(Op::SetUpval, e, var.info, 0)?;
            }
            ExpKind::Global => {
                let e = self.exp2anyreg(ex)?;
                self.code_abx(Op::SetGlobal, e, var.info as u32)?;
            }
            ExpKind::Indexed => {
                let e = self.exp2rk(ex)?;
                self.code_abc(Op::SetTable, var.info, var.aux, e)?;
            }
            _ => unreachable!("invalid variable kind to store"),
        }
        self.freeexp(ex);
        Ok(())
    }

    fn code_self(&mut self, e: &mut ExpDesc, key: &mut ExpDesc) -> R<()> {
        self.exp2anyreg(e)?;
        self.freeexp(e);
        let func = self.fs().freereg;
        self.reserveregs(2)?;
        let k = self.exp2rk(key)?;
        self.code_abc(Op::SelfOp, func, e.info, k)?;
        self.freeexp(key);
        e.info = func;
        e.k = ExpKind::NonReloc;
        Ok(())
    }

    fn invertjump(&mut self, e: &ExpDesc) {
        let control = self.jumpcontrol(e.info);
        let i = self.instr(control);
        let a = ins::a(*i);
        ins::set_a(i, (a == 0) as u32);
    }

    fn jumponcond(&mut self, e: &mut ExpDesc, cond: i32) -> R<i32> {
        if e.k == ExpKind::Relocable {
            let ie = *self.instr(e.info);
            if ins::op(ie) == Some(Op::Not) {
                let fs = self.fs();
                fs.f.code.pop();
                fs.f.line_info.pop();
                let b = ins::b(ie) as i32;
                return self.condjump(Op::Test, b, b, (cond == 0) as i32);
            }
        }
        self.discharge2anyreg(e)?;
        self.freeexp(e);
        self.condjump(Op::Test, NO_REG as i32, e.info, cond)
    }

    fn goiftrue(&mut self, e: &mut ExpDesc) -> R<()> {
        self.dischargevars(e)?;
        let pc = match e.k {
            ExpKind::K | ExpKind::True => NO_JUMP,
            ExpKind::False => self.jump()?,
            ExpKind::Jmp => {
                self.invertjump(e);
                e.info
            }
            _ => self.jumponcond(e, 0)?,
        };
        e.f = self.concat(e.f, pc)?;
        Ok(())
    }

    fn goiffalse(&mut self, e: &mut ExpDesc) -> R<()> {
        self.dischargevars(e)?;
        let pc = match e.k {
            ExpKind::Nil | ExpKind::False => NO_JUMP,
            ExpKind::True => self.jump()?,
            ExpKind::Jmp => e.info,
            _ => self.jumponcond(e, 1)?,
        };
        e.t = self.concat(e.t, pc)?;
        Ok(())
    }

    fn codenot(&mut self, e: &mut ExpDesc) -> R<()> {
        self.dischargevars(e)?;
        match e.k {
            ExpKind::Nil | ExpKind::False => e.k = ExpKind::True,
            ExpKind::K | ExpKind::True => e.k = ExpKind::False,
            ExpKind::Jmp => self.invertjump(e),
            ExpKind::Relocable | ExpKind::NonReloc => {
                self.discharge2anyreg(e)?;
                self.freeexp(e);
                e.info = self.code_abc(Op::Not, 0, e.info, 0)?;
                e.k = ExpKind::Relocable;
            }
            _ => unreachable!("cannot negate this expression"),
        }
        std::mem::swap(&mut e.f, &mut e.t);
        self.removevalues(e.f);
        self.removevalues(e.t);
        Ok(())
    }

    fn indexed(&mut self, t: &mut ExpDesc, k: &mut ExpDesc) -> R<()> {
        t.aux = self.exp2rk(k)?;
        t.k = ExpKind::Indexed;
        Ok(())
    }

    fn prefix_minus(&mut self, e: &mut ExpDesc) -> R<()> {
        self.exp2val(e)?;
        if e.k == ExpKind::K
            && let Constant::Number(n) = self.fs().f.constants[e.info as usize]
        {
            e.info = self.number_k(-n)?;
        } else {
            self.exp2anyreg(e)?;
            self.freeexp(e);
            e.info = self.code_abc(Op::Unm, 0, e.info, 0)?;
            e.k = ExpKind::Relocable;
        }
        Ok(())
    }

    fn infix(&mut self, op: BinOpr, v: &mut ExpDesc) -> R<()> {
        match op {
            BinOpr::And => {
                self.goiftrue(v)?;
                self.patchtohere(v.t)?;
                v.t = NO_JUMP;
            }
            BinOpr::Or => {
                self.goiffalse(v)?;
                self.patchtohere(v.f)?;
                v.f = NO_JUMP;
            }
            BinOpr::Concat => self.exp2nextreg(v)?,
            _ => {
                self.exp2rk(v)?;
            }
        }
        Ok(())
    }

    fn codebinop(&mut self, res: &mut ExpDesc, op: BinOpr, o1: i32, o2: i32) -> R<()> {
        let arith = match op {
            BinOpr::Add => Some(Op::Add),
            BinOpr::Sub => Some(Op::Sub),
            BinOpr::Mult => Some(Op::Mul),
            BinOpr::Div => Some(Op::Div),
            BinOpr::Pow => Some(Op::Pow),
            _ => None,
        };
        if let Some(opc) = arith {
            res.info = self.code_abc(opc, 0, o1, o2)?;
            res.k = ExpKind::Relocable;
            return Ok(());
        }
        let (opc, cond, o1, o2) = match op {
            BinOpr::Ne => (Op::Eq, 0, o1, o2),
            BinOpr::Eq => (Op::Eq, 1, o1, o2),
            BinOpr::Lt => (Op::Lt, 1, o1, o2),
            BinOpr::Le => (Op::Le, 1, o1, o2),
            BinOpr::Gt => (Op::Lt, 1, o2, o1),
            BinOpr::Ge => (Op::Le, 1, o2, o1),
            _ => unreachable!("not a comparison"),
        };
        res.info = self.condjump(opc, cond, o1, o2)?;
        res.k = ExpKind::Jmp;
        Ok(())
    }

    fn posfix(&mut self, op: BinOpr, e1: &mut ExpDesc, e2: &mut ExpDesc) -> R<()> {
        match op {
            BinOpr::And => {
                self.dischargevars(e2)?;
                let f = self.concat(e1.f, e2.f)?;
                *e1 = ExpDesc { f, ..*e2 };
            }
            BinOpr::Or => {
                self.dischargevars(e2)?;
                let t = self.concat(e1.t, e2.t)?;
                *e1 = ExpDesc { t, ..*e2 };
            }
            BinOpr::Concat => {
                self.exp2val(e2)?;
                if e2.k == ExpKind::Relocable && ins::op(*self.instr(e2.info)) == Some(Op::Concat) {
                    self.freeexp(e1);
                    ins::set_b(self.instr(e2.info), e1.info as u32);
                    e1.k = e2.k;
                    e1.info = e2.info;
                } else {
                    self.exp2nextreg(e2)?;
                    self.freeexp(e2);
                    self.freeexp(e1);
                    e1.info = self.code_abc(Op::Concat, 0, e1.info, e2.info)?;
                    e1.k = ExpKind::Relocable;
                }
            }
            _ => {
                let o1 = self.exp2rk(e1)?;
                let o2 = self.exp2rk(e2)?;
                self.freeexp(e2);
                self.freeexp(e1);
                self.codebinop(e1, op, o1, o2)?;
            }
        }
        Ok(())
    }
}
