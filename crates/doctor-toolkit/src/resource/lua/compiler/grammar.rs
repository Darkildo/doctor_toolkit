//! Grammar rules of the compiler (`lparser.c`, from "GRAMMAR RULES" on).

use std::rc::Rc;

use super::{
    BinOpr, ExpDesc, ExpKind, MAXARG_BX, MAXPARAMS, MAXUPVALUES, MAXVARS, MULTRET, NO_JUMP, NO_REG,
    Parser, R, UNARY_PRIORITY,
};
use super::{BlockCnt, FIELDS_PER_FLUSH, FuncState, MAXEXPWHILE};
use crate::resource::lua::chunk::{LocVar, Proto};
use crate::resource::lua::lexer::Tok;
use crate::resource::lua::opcodes::{Op, ins};

/// Left-hand side of an assignment (`struct LHS_assign`).
type Lhs = Vec<ExpDesc>;

/// State of a table constructor (`struct ConsControl`).
struct ConsControl {
    v: ExpDesc,
    t: ExpDesc,
    nh: usize,
    na: usize,
    tostore: u32,
}

impl Parser<'_> {
    // ------------------------------------------------------- variables, functions

    fn registerlocalvar(&mut self, name: &Rc<[u8]>) -> usize {
        let fs = self.fs();
        fs.f.locals.push(LocVar {
            name: name.to_vec(),
            start_pc: 0,
            end_pc: 0,
        });
        fs.f.locals.len() - 1
    }

    fn new_localvar(&mut self, name: &Rc<[u8]>, n: usize) -> R<()> {
        let nactvar = self.fs().nactvar;
        self.check_limit(nactvar + n + 1, MAXVARS, "local variables")?;
        let index = self.registerlocalvar(name);
        let fs = self.fs();
        if fs.actvar.len() <= nactvar + n {
            fs.actvar.resize(nactvar + n + 1, 0);
        }
        fs.actvar[nactvar + n] = index;
        Ok(())
    }

    fn new_localvarstr(&mut self, name: &str, n: usize) -> R<()> {
        self.new_localvar(&Rc::from(name.as_bytes()), n)
    }

    fn adjustlocalvars(&mut self, nvars: usize) {
        let fs = self.fs();
        fs.nactvar += nvars;
        let pc = fs.pc() as u32;
        for i in fs.nactvar - nvars..fs.nactvar {
            fs.local(i).start_pc = pc;
        }
    }

    fn removevars(&mut self, tolevel: usize) {
        let fs = self.fs();
        let pc = fs.pc() as u32;
        while fs.nactvar > tolevel {
            fs.nactvar -= 1;
            let i = fs.nactvar;
            fs.local(i).end_pc = pc;
        }
    }

    fn create_local(&mut self, name: &str) -> R<()> {
        self.new_localvarstr(name, 0)?;
        self.adjustlocalvars(1);
        Ok(())
    }

    fn indexupvalue(&mut self, level: usize, name: &Rc<[u8]>, v: &ExpDesc) -> R<i32> {
        let fs = &self.fs[level];
        if let Some(i) = fs
            .upvalues
            .iter()
            .position(|&(k, info)| k == v.k && info == v.info)
        {
            return Ok(i as i32);
        }
        if fs.upvalues.len() + 1 > MAXUPVALUES {
            return Err(self.syntax_error(&format!("too many upvalues (limit={MAXUPVALUES})")));
        }
        let fs = &mut self.fs[level];
        fs.f.upvalue_names.push(name.to_vec());
        fs.upvalues.push((v.k, v.info));
        fs.f.num_upvalues = fs.upvalues.len() as u8;
        Ok(fs.upvalues.len() as i32 - 1)
    }

    fn searchvar(&self, level: usize, name: &[u8]) -> Option<usize> {
        let fs = &self.fs[level];
        (0..fs.nactvar)
            .rev()
            .find(|&i| *fs.f.locals[fs.actvar[i]].name == *name)
    }

    fn markupval(&mut self, level: usize, v: usize) {
        if let Some(bl) = self.fs[level]
            .bl
            .iter_mut()
            .rev()
            .find(|bl| bl.nactvar <= v)
        {
            bl.upval = true;
        }
    }

    fn singlevaraux(&mut self, level: Option<usize>, name: &Rc<[u8]>, base: bool) -> R<ExpDesc> {
        let Some(level) = level else {
            return Ok(ExpDesc::new(ExpKind::Global, NO_REG as i32));
        };
        if let Some(v) = self.searchvar(level, name) {
            if !base {
                self.markupval(level, v);
            }
            return Ok(ExpDesc::new(ExpKind::Local, v as i32));
        }
        let mut var = self.singlevaraux(level.checked_sub(1), name, false)?;
        if var.k == ExpKind::Global {
            if base {
                var.info = self.string_k(name)?;
            }
        } else {
            var.info = self.indexupvalue(level, name, &var)?;
            var.k = ExpKind::Upval;
        }
        Ok(var)
    }

    fn singlevar(&mut self) -> R<ExpDesc> {
        let name = self.str_checkname()?;
        let level = self.fs.len() - 1;
        self.singlevaraux(Some(level), &name, true)
    }

    fn adjust_assign(&mut self, nvars: i32, nexps: i32, e: &mut ExpDesc) -> R<()> {
        let mut extra = nvars - nexps;
        if e.k == ExpKind::Call {
            extra += 1;
            if extra <= 0 {
                extra = 0;
            } else {
                self.reserveregs(extra - 1)?;
            }
            self.setcallreturns(e, extra);
        } else {
            if e.k != ExpKind::Void {
                self.exp2nextreg(e)?;
            }
            if extra > 0 {
                let reg = self.fs().freereg;
                self.reserveregs(extra)?;
                self.code_nil(reg, extra)?;
            }
        }
        Ok(())
    }

    fn code_params(&mut self, nparams: usize, dots: bool) -> R<()> {
        self.adjustlocalvars(nparams);
        let nactvar = self.fs().nactvar;
        self.check_limit(nactvar, MAXPARAMS, "parameters")?;
        let fs = self.fs();
        fs.f.num_params = nactvar as u8;
        fs.f.is_vararg = dots;
        if dots {
            self.create_local("arg")?;
        }
        let n = self.fs().nactvar as i32;
        self.reserveregs(n)
    }

    fn enterblock(&mut self, isbreakable: bool) {
        let fs = self.fs();
        let nactvar = fs.nactvar;
        fs.bl.push(BlockCnt {
            breaklist: NO_JUMP,
            nactvar,
            upval: false,
            isbreakable,
        });
    }

    fn leaveblock(&mut self) -> R<()> {
        let bl = self.fs().bl.pop().expect("an open block");
        self.removevars(bl.nactvar);
        if bl.upval {
            self.code_abc(Op::Close, bl.nactvar as i32, 0, 0)?;
        }
        let fs = self.fs();
        fs.freereg = fs.nactvar as i32;
        self.patchtohere(bl.breaklist)
    }

    pub(super) fn open_func(&mut self, source: Option<Vec<u8>>) {
        self.fs.push(FuncState::new(Proto {
            source,
            max_stack: 2,
            ..Proto::default()
        }));
    }

    pub(super) fn close_func(&mut self) -> R<(Proto, Vec<(ExpKind, i32)>)> {
        self.removevars(0);
        self.code_abc(Op::Return, 0, 1, 0)?;
        let fs = self.fs.pop().expect("an open function");
        Ok((fs.f, fs.upvalues))
    }

    fn pushclosure(&mut self, f: Proto, upvalues: &[(ExpKind, i32)]) -> R<ExpDesc> {
        let np = self.fs().f.protos.len();
        if np >= MAXARG_BX as usize {
            return Err(self.syntax_error("constant table overflow"));
        }
        self.fs().f.protos.push(f);
        let pc = self.code_abx(Op::Closure, 0, np as u32)?;
        for &(k, info) in upvalues {
            let op = if k == ExpKind::Local {
                Op::Move
            } else {
                Op::GetUpval
            };
            self.code_abc(op, 0, info, 0)?;
        }
        Ok(ExpDesc::new(ExpKind::Relocable, pc))
    }

    // ---------------------------------------------------------------- expressions

    fn field(&mut self, v: &mut ExpDesc) -> R<()> {
        self.exp2anyreg(v)?;
        self.next()?;
        let mut key = self.checkname()?;
        self.indexed(v, &mut key)
    }

    fn index(&mut self) -> R<ExpDesc> {
        self.next()?;
        let mut v = self.expr()?;
        self.exp2val(&mut v)?;
        self.check(&Tok::Char(b']'))?;
        Ok(v)
    }

    fn checkname(&mut self) -> R<ExpDesc> {
        let name = self.str_checkname()?;
        Ok(ExpDesc::new(ExpKind::K, self.string_k(&name)?))
    }

    fn recfield(&mut self, cc: &mut ConsControl) -> R<()> {
        let reg = self.fs().freereg;
        let mut key = if matches!(self.t, Tok::Name(_)) {
            cc.nh += 1;
            self.checkname()?
        } else {
            self.index()?
        };
        self.check(&Tok::Char(b'='))?;
        self.exp2rk(&mut key)?;
        let mut val = self.expr()?;
        let k = self.exp2rk(&mut key)?;
        let v = self.exp2rk(&mut val)?;
        self.code_abc(Op::SetTable, cc.t.info, k, v)?;
        self.fs().freereg = reg;
        Ok(())
    }

    fn closelistfield(&mut self, cc: &mut ConsControl) -> R<()> {
        if cc.v.k == ExpKind::Void {
            return Ok(());
        }
        self.exp2nextreg(&mut cc.v)?;
        cc.v.k = ExpKind::Void;
        if cc.tostore == FIELDS_PER_FLUSH {
            self.code_abx(Op::SetList, cc.t.info, cc.na as u32 - 1)?;
            cc.tostore = 0;
            self.fs().freereg = cc.t.info + 1;
        }
        Ok(())
    }

    fn lastlistfield(&mut self, cc: &mut ConsControl) -> R<()> {
        if cc.tostore == 0 {
            return Ok(());
        }
        if cc.v.k == ExpKind::Call {
            self.setcallreturns(&mut cc.v, MULTRET);
            self.code_abx(Op::SetListO, cc.t.info, cc.na as u32 - 1)?;
        } else {
            if cc.v.k != ExpKind::Void {
                self.exp2nextreg(&mut cc.v)?;
            }
            self.code_abx(Op::SetList, cc.t.info, cc.na as u32 - 1)?;
        }
        self.fs().freereg = cc.t.info + 1;
        Ok(())
    }

    fn listfield(&mut self, cc: &mut ConsControl) -> R<()> {
        cc.v = self.expr()?;
        self.check_limit(cc.na, MAXARG_BX as usize, "items in a constructor")?;
        cc.na += 1;
        cc.tostore += 1;
        Ok(())
    }

    fn constructor(&mut self) -> R<ExpDesc> {
        let line = self.lex.line;
        let pc = self.code_abc(Op::NewTable, 0, 0, 0)?;
        let mut t = ExpDesc::new(ExpKind::Relocable, pc);
        self.exp2nextreg(&mut t)?;
        let mut cc = ConsControl {
            v: ExpDesc::new(ExpKind::Void, 0),
            t,
            nh: 0,
            na: 0,
            tostore: 0,
        };
        self.check(&Tok::Char(b'{'))?;
        loop {
            self.testnext(&Tok::Char(b';'))?;
            if self.t == Tok::Char(b'}') {
                break;
            }
            self.closelistfield(&mut cc)?;
            match self.t {
                Tok::Name(_) => {
                    if *self.peek()? != Tok::Char(b'=') {
                        self.listfield(&mut cc)?;
                    } else {
                        self.recfield(&mut cc)?;
                    }
                }
                Tok::Char(b'[') => self.recfield(&mut cc)?,
                _ => self.listfield(&mut cc)?,
            }
            if !(self.testnext(&Tok::Char(b','))? || self.testnext(&Tok::Char(b';'))?) {
                break;
            }
        }
        self.check_match(Tok::Char(b'}'), Tok::Char(b'{'), line)?;
        self.lastlistfield(&mut cc)?;
        let i = self.instr(pc);
        ins::set_b(i, int2fb(cc.na as u32));
        ins::set_c(i, (log2(cc.nh as u32) + 1) as u32);
        Ok(t)
    }

    fn parlist(&mut self) -> R<()> {
        let mut nparams = 0;
        let mut dots = false;
        if self.t != Tok::Char(b')') {
            loop {
                match &self.t {
                    Tok::Dots => {
                        dots = true;
                        self.next()?;
                    }
                    Tok::Name(_) => {
                        let name = self.str_checkname()?;
                        self.new_localvar(&name, nparams)?;
                        nparams += 1;
                    }
                    _ => return Err(self.syntax_error("<name> or `...' expected")),
                }
                if dots || !self.testnext(&Tok::Char(b','))? {
                    break;
                }
            }
        }
        self.code_params(nparams, dots)
    }

    fn body(&mut self, needself: bool, line: u32) -> R<ExpDesc> {
        self.open_func(None);
        self.fs().f.line_defined = line;
        self.check(&Tok::Char(b'('))?;
        if needself {
            self.create_local("self")?;
        }
        self.parlist()?;
        self.check(&Tok::Char(b')'))?;
        self.chunk()?;
        self.check_match(Tok::End, Tok::Function, line)?;
        let (f, upvalues) = self.close_func()?;
        self.pushclosure(f, &upvalues)
    }

    /// `explist1`: returns the number of expressions and the last one.
    fn explist1(&mut self) -> R<(i32, ExpDesc)> {
        let mut n = 1;
        let mut v = self.expr()?;
        while self.testnext(&Tok::Char(b','))? {
            self.exp2nextreg(&mut v)?;
            v = self.expr()?;
            n += 1;
        }
        Ok((n, v))
    }

    fn funcargs(&mut self, f: &mut ExpDesc) -> R<()> {
        let line = self.lex.line;
        let mut args = match &self.t {
            Tok::Char(b'(') => {
                if line != self.lastline {
                    return Err(
                        self.syntax_error("ambiguous syntax (function call x new statement)")
                    );
                }
                self.next()?;
                let args = if self.t == Tok::Char(b')') {
                    ExpDesc::new(ExpKind::Void, 0)
                } else {
                    let (_, mut args) = self.explist1()?;
                    self.setcallreturns(&mut args, MULTRET);
                    args
                };
                self.check_match(Tok::Char(b')'), Tok::Char(b'('), line)?;
                args
            }
            Tok::Char(b'{') => self.constructor()?,
            Tok::String(s) => {
                let s = s.clone();
                let args = ExpDesc::new(ExpKind::K, self.string_k(&s)?);
                self.next()?;
                args
            }
            _ => return Err(self.syntax_error("function arguments expected")),
        };
        let base = f.info;
        let nparams = if args.k == ExpKind::Call {
            MULTRET
        } else {
            if args.k != ExpKind::Void {
                self.exp2nextreg(&mut args)?;
            }
            self.fs().freereg - (base + 1)
        };
        *f = ExpDesc::new(
            ExpKind::Call,
            self.code_abc(Op::Call, base, nparams + 1, 2)?,
        );
        self.fixline(line);
        self.fs().freereg = base + 1;
        Ok(())
    }

    fn prefixexp(&mut self) -> R<ExpDesc> {
        match self.t {
            Tok::Char(b'(') => {
                let line = self.lex.line;
                self.next()?;
                let mut v = self.expr()?;
                self.check_match(Tok::Char(b')'), Tok::Char(b'('), line)?;
                self.dischargevars(&mut v)?;
                Ok(v)
            }
            Tok::Name(_) => self.singlevar(),
            _ => Err(self.syntax_error("unexpected symbol")),
        }
    }

    fn primaryexp(&mut self) -> R<ExpDesc> {
        let mut v = self.prefixexp()?;
        loop {
            match self.t {
                Tok::Char(b'.') => self.field(&mut v)?,
                Tok::Char(b'[') => {
                    self.exp2anyreg(&mut v)?;
                    let mut key = self.index()?;
                    self.indexed(&mut v, &mut key)?;
                }
                Tok::Char(b':') => {
                    self.next()?;
                    let mut key = self.checkname()?;
                    self.code_self(&mut v, &mut key)?;
                    self.funcargs(&mut v)?;
                }
                Tok::Char(b'(') | Tok::String(_) | Tok::Char(b'{') => {
                    self.exp2nextreg(&mut v)?;
                    self.funcargs(&mut v)?;
                }
                _ => return Ok(v),
            }
        }
    }

    fn simpleexp(&mut self) -> R<ExpDesc> {
        let v = match &self.t {
            Tok::Number(n) => {
                let n = *n;
                ExpDesc::new(ExpKind::K, self.number_k(n)?)
            }
            Tok::String(s) => {
                let s = s.clone();
                ExpDesc::new(ExpKind::K, self.string_k(&s)?)
            }
            Tok::Nil => ExpDesc::new(ExpKind::Nil, 0),
            Tok::True => ExpDesc::new(ExpKind::True, 0),
            Tok::False => ExpDesc::new(ExpKind::False, 0),
            Tok::Char(b'{') => return self.constructor(),
            Tok::Function => {
                self.next()?;
                let line = self.lex.line;
                return self.body(false, line);
            }
            _ => return self.primaryexp(),
        };
        self.next()?;
        Ok(v)
    }

    fn subexpr(&mut self, v: &mut ExpDesc, limit: i32) -> R<Option<BinOpr>> {
        self.enter_level()?;
        let unary = match self.t {
            Tok::Not => Some(true),
            Tok::Char(b'-') => Some(false),
            _ => None,
        };
        if let Some(not) = unary {
            self.next()?;
            self.subexpr(v, UNARY_PRIORITY)?;
            if not {
                self.codenot(v)?;
            } else {
                self.prefix_minus(v)?;
            }
        } else {
            *v = self.simpleexp()?;
        }
        let mut op = BinOpr::from_token(&self.t);
        while let Some(o) = op {
            if o.priority().0 <= limit {
                break;
            }
            self.next()?;
            self.infix(o, v)?;
            let mut v2 = ExpDesc::new(ExpKind::Void, 0);
            let nextop = self.subexpr(&mut v2, o.priority().1)?;
            self.posfix(o, v, &mut v2)?;
            op = nextop;
        }
        self.nestlevel -= 1;
        Ok(op)
    }

    fn expr(&mut self) -> R<ExpDesc> {
        let mut v = ExpDesc::new(ExpKind::Void, 0);
        self.subexpr(&mut v, -1)?;
        Ok(v)
    }

    // ------------------------------------------------------------------ statements

    fn block_follow(&self) -> bool {
        matches!(
            self.t,
            Tok::Else | Tok::Elseif | Tok::End | Tok::Until | Tok::Eos
        )
    }

    fn block(&mut self) -> R<()> {
        self.enterblock(false);
        self.chunk()?;
        self.leaveblock()
    }

    fn check_conflict(&mut self, lhs: &mut Lhs, v: &ExpDesc) -> R<()> {
        let extra = self.fs().freereg;
        let mut conflict = false;
        for lh in lhs.iter_mut() {
            if lh.k == ExpKind::Indexed {
                if lh.info == v.info {
                    conflict = true;
                    lh.info = extra;
                }
                if lh.aux == v.info {
                    conflict = true;
                    lh.aux = extra;
                }
            }
        }
        if conflict {
            self.code_abc(Op::Move, extra, v.info, 0)?;
            self.reserveregs(1)?;
        }
        Ok(())
    }

    fn assignment(&mut self, lhs: &mut Lhs) -> R<()> {
        let nvars = lhs.len();
        let k = lhs[nvars - 1].k;
        self.check_condition(
            (ExpKind::Local..=ExpKind::Indexed).contains(&k),
            "syntax error",
        )?;
        if self.testnext(&Tok::Char(b','))? {
            let nv = self.primaryexp()?;
            if nv.k == ExpKind::Local {
                self.check_conflict(lhs, &nv)?;
            }
            lhs.push(nv);
            self.assignment(lhs)?;
            lhs.pop();
        } else {
            self.check(&Tok::Char(b'='))?;
            let (nexps, mut e) = self.explist1()?;
            if nexps != nvars as i32 {
                self.adjust_assign(nvars as i32, nexps, &mut e)?;
                if nexps > nvars as i32 {
                    self.fs().freereg -= nexps - nvars as i32;
                }
            } else {
                self.setcallreturns(&mut e, 1);
                let var = lhs[nvars - 1];
                return self.storevar(&var, &mut e);
            }
        }
        let mut e = ExpDesc::new(ExpKind::NonReloc, self.fs().freereg - 1);
        let var = lhs[nvars - 1];
        self.storevar(&var, &mut e)
    }

    fn cond(&mut self) -> R<ExpDesc> {
        let mut v = self.expr()?;
        if v.k == ExpKind::Nil {
            v.k = ExpKind::False;
        }
        self.goiftrue(&mut v)?;
        self.patchtohere(v.t)?;
        Ok(v)
    }

    fn whilestat(&mut self, line: u32) -> R<()> {
        self.next()?;
        let whileinit = self.jump()?;
        let expinit = self.getlabel();
        let mut v = self.expr()?;
        if v.k == ExpKind::K {
            v.k = ExpKind::True;
        }
        let lineexp = self.lex.line;
        self.goiffalse(&mut v)?;
        let jpc = self.fs().jpc;
        v.f = self.concat(v.f, jpc)?;
        self.fs().jpc = NO_JUMP;
        let sizeexp = (self.fs().pc() - expinit) as usize;
        if sizeexp > MAXEXPWHILE {
            return Err(self.syntax_error("`while' condition too complex"));
        }
        let fs = self.fs();
        let codeexp = fs.f.code.split_off(expinit as usize);
        fs.f.line_info.truncate(expinit as usize);
        self.enterblock(true);
        self.check(&Tok::Do)?;
        let blockinit = self.getlabel();
        self.block()?;
        self.patchtohere(whileinit)?;
        let shift = self.fs().pc() - expinit;
        if v.t != NO_JUMP {
            v.t += shift;
        }
        if v.f != NO_JUMP {
            v.f += shift;
        }
        for i in codeexp {
            self.code(i, lineexp)?;
        }
        self.check_match(Tok::End, Tok::While, line)?;
        self.leaveblock()?;
        self.patchlist(v.t, blockinit)?;
        self.patchtohere(v.f)
    }

    fn repeatstat(&mut self, line: u32) -> R<()> {
        let repeat_init = self.getlabel();
        self.enterblock(true);
        self.next()?;
        self.block()?;
        self.check_match(Tok::Until, Tok::Repeat, line)?;
        let v = self.cond()?;
        self.patchlist(v.f, repeat_init)?;
        self.leaveblock()
    }

    fn exp1(&mut self) -> R<()> {
        let mut e = self.expr()?;
        self.exp2nextreg(&mut e)
    }

    fn forbody(&mut self, base: i32, line: u32, nvars: usize, isnum: bool) -> R<()> {
        self.adjustlocalvars(nvars);
        self.check(&Tok::Do)?;
        self.enterblock(true);
        let prep = self.getlabel();
        self.block()?;
        self.patchtohere(prep - 1)?;
        let endfor = if isnum {
            self.code_asbx(Op::ForLoop, base, NO_JUMP)?
        } else {
            self.code_abc(Op::TForLoop, base, 0, nvars as i32 - 3)?
        };
        self.fixline(line);
        let list = if isnum { endfor } else { self.jump()? };
        self.patchlist(list, prep)?;
        self.leaveblock()
    }

    fn fornum(&mut self, varname: &Rc<[u8]>, line: u32) -> R<()> {
        let base = self.fs().freereg;
        self.new_localvar(varname, 0)?;
        self.new_localvarstr("(for limit)", 1)?;
        self.new_localvarstr("(for step)", 2)?;
        self.check(&Tok::Char(b'='))?;
        self.exp1()?;
        self.check(&Tok::Char(b','))?;
        self.exp1()?;
        if self.testnext(&Tok::Char(b','))? {
            self.exp1()?;
        } else {
            let freereg = self.fs().freereg;
            let k = self.number_k(1.0)?;
            self.code_abx(Op::LoadK, freereg, k as u32)?;
            self.reserveregs(1)?;
        }
        let freereg = self.fs().freereg;
        self.code_abc(Op::Sub, freereg - 3, freereg - 3, freereg - 1)?;
        self.jump()?;
        self.forbody(base, line, 3, true)
    }

    fn forlist(&mut self, indexname: &Rc<[u8]>) -> R<()> {
        let base = self.fs().freereg;
        self.new_localvarstr("(for generator)", 0)?;
        self.new_localvarstr("(for state)", 1)?;
        self.new_localvar(indexname, 2)?;
        let mut nvars = 3;
        while self.testnext(&Tok::Char(b','))? {
            let name = self.str_checkname()?;
            self.new_localvar(&name, nvars)?;
            nvars += 1;
        }
        self.check(&Tok::In)?;
        let line = self.lex.line;
        let (nexps, mut e) = self.explist1()?;
        self.adjust_assign(nvars as i32, nexps, &mut e)?;
        self.checkstack(3)?;
        self.code_asbx(Op::TForPrep, base, NO_JUMP)?;
        self.forbody(base, line, nvars, false)
    }

    fn forstat(&mut self, line: u32) -> R<()> {
        self.enterblock(false);
        self.next()?;
        let varname = self.str_checkname()?;
        match self.t {
            Tok::Char(b'=') => self.fornum(&varname, line)?,
            Tok::Char(b',') | Tok::In => self.forlist(&varname)?,
            _ => return Err(self.syntax_error("`=' or `in' expected")),
        }
        self.check_match(Tok::End, Tok::For, line)?;
        self.leaveblock()
    }

    fn test_then_block(&mut self) -> R<ExpDesc> {
        self.next()?;
        let v = self.cond()?;
        self.check(&Tok::Then)?;
        self.block()?;
        Ok(v)
    }

    fn ifstat(&mut self, line: u32) -> R<()> {
        let mut escapelist = NO_JUMP;
        let mut v = self.test_then_block()?;
        while self.t == Tok::Elseif {
            let j = self.jump()?;
            escapelist = self.concat(escapelist, j)?;
            self.patchtohere(v.f)?;
            v = self.test_then_block()?;
        }
        if self.t == Tok::Else {
            let j = self.jump()?;
            escapelist = self.concat(escapelist, j)?;
            self.patchtohere(v.f)?;
            self.next()?;
            self.block()?;
        } else {
            escapelist = self.concat(escapelist, v.f)?;
        }
        self.patchtohere(escapelist)?;
        self.check_match(Tok::End, Tok::If, line)
    }

    fn localfunc(&mut self) -> R<()> {
        let name = self.str_checkname()?;
        self.new_localvar(&name, 0)?;
        let v = ExpDesc::new(ExpKind::Local, self.fs().freereg);
        self.reserveregs(1)?;
        self.adjustlocalvars(1);
        let line = self.lex.line;
        let mut b = self.body(false, line)?;
        self.storevar(&v, &mut b)?;
        let fs = self.fs();
        let pc = fs.pc() as u32;
        let last = fs.nactvar - 1;
        fs.local(last).start_pc = pc;
        Ok(())
    }

    fn localstat(&mut self) -> R<()> {
        let mut nvars = 0;
        loop {
            let name = self.str_checkname()?;
            self.new_localvar(&name, nvars)?;
            nvars += 1;
            if !self.testnext(&Tok::Char(b','))? {
                break;
            }
        }
        let (nexps, mut e) = if self.testnext(&Tok::Char(b'='))? {
            self.explist1()?
        } else {
            (0, ExpDesc::new(ExpKind::Void, 0))
        };
        self.adjust_assign(nvars as i32, nexps, &mut e)?;
        self.adjustlocalvars(nvars);
        Ok(())
    }

    fn funcname(&mut self) -> R<(bool, ExpDesc)> {
        let mut v = self.singlevar()?;
        while self.t == Tok::Char(b'.') {
            self.field(&mut v)?;
        }
        let needself = self.t == Tok::Char(b':');
        if needself {
            self.field(&mut v)?;
        }
        Ok((needself, v))
    }

    fn funcstat(&mut self, line: u32) -> R<()> {
        self.next()?;
        let (needself, v) = self.funcname()?;
        let mut b = self.body(needself, line)?;
        self.storevar(&v, &mut b)?;
        self.fixline(line);
        Ok(())
    }

    fn exprstat(&mut self) -> R<()> {
        let mut v = self.primaryexp()?;
        if v.k == ExpKind::Call {
            self.setcallreturns(&mut v, 0);
            Ok(())
        } else {
            let mut lhs = vec![v];
            self.assignment(&mut lhs)
        }
    }

    fn retstat(&mut self) -> R<()> {
        self.next()?;
        let (first, nret) = if self.block_follow() || self.t == Tok::Char(b';') {
            (0, 0)
        } else {
            let (nret, mut e) = self.explist1()?;
            if e.k == ExpKind::Call {
                self.setcallreturns(&mut e, MULTRET);
                if nret == 1 {
                    ins::set_op(self.instr(e.info), Op::TailCall);
                }
                (self.fs().nactvar as i32, MULTRET)
            } else if nret == 1 {
                (self.exp2anyreg(&mut e)?, 1)
            } else {
                self.exp2nextreg(&mut e)?;
                (self.fs().nactvar as i32, nret)
            }
        };
        self.code_abc(Op::Return, first, nret + 1, 0)?;
        Ok(())
    }

    fn breakstat(&mut self) -> R<()> {
        self.next()?;
        let fs = self.fs();
        let mut upval = false;
        let mut found = None;
        for (index, bl) in fs.bl.iter().enumerate().rev() {
            if bl.isbreakable {
                found = Some(index);
                break;
            }
            upval |= bl.upval;
        }
        let Some(index) = found else {
            return Err(self.syntax_error("no loop to break"));
        };
        let nactvar = fs.bl[index].nactvar;
        if upval {
            self.code_abc(Op::Close, nactvar as i32, 0, 0)?;
        }
        let j = self.jump()?;
        let breaklist = self.fs().bl[index].breaklist;
        self.fs().bl[index].breaklist = self.concat(breaklist, j)?;
        Ok(())
    }

    /// Returns true for statements that must be the last of a block.
    fn statement(&mut self) -> R<bool> {
        let line = self.lex.line;
        match self.t {
            Tok::If => self.ifstat(line)?,
            Tok::While => self.whilestat(line)?,
            Tok::Do => {
                self.next()?;
                self.block()?;
                self.check_match(Tok::End, Tok::Do, line)?;
            }
            Tok::For => self.forstat(line)?,
            Tok::Repeat => self.repeatstat(line)?,
            Tok::Function => self.funcstat(line)?,
            Tok::Local => {
                self.next()?;
                if self.testnext(&Tok::Function)? {
                    self.localfunc()?;
                } else {
                    self.localstat()?;
                }
            }
            Tok::Return => {
                self.retstat()?;
                return Ok(true);
            }
            Tok::Break => {
                self.breakstat()?;
                return Ok(true);
            }
            _ => self.exprstat()?,
        }
        Ok(false)
    }

    pub(super) fn chunk(&mut self) -> R<()> {
        self.enter_level()?;
        let mut islast = false;
        while !islast && !self.block_follow() {
            islast = self.statement()?;
            self.testnext(&Tok::Char(b';'))?;
            let fs = self.fs();
            fs.freereg = fs.nactvar as i32;
        }
        self.nestlevel -= 1;
        Ok(())
    }
}

/// `luaO_int2fb`: converts an integer to a "floating point byte" (`eeeeexxx`).
fn int2fb(mut x: u32) -> u32 {
    let mut m = 0;
    while x >= 8 {
        x = (x + 1) >> 1;
        m += 1;
    }
    (m << 3) | x
}

/// `luaO_log2`: floor of log2, -1 for zero.
fn log2(x: u32) -> i32 {
    if x == 0 {
        -1
    } else {
        31 - x.leading_zeros() as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers_match_lua() {
        assert_eq!(int2fb(0), 0);
        assert_eq!(int2fb(7), 7);
        assert_eq!(int2fb(8), 12);
        assert_eq!(int2fb(33), 29);
        assert_eq!(log2(0), -1);
        assert_eq!(log2(1), 0);
        assert_eq!(log2(5), 2);
    }
}
