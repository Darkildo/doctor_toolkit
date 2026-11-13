//! Reconstruction of statements and expressions from Lua 5.0 bytecode.
//!
//! The code generator of Lua 5.0 is simple and deterministic, so the
//! decompiler follows its patterns: registers above the active locals hold
//! pending expressions until an instruction consumes them; a store or call
//! with no pending registers left ends a statement; local variables (from the
//! debug information) start where their declaration ends. Control structures
//! are recognised by their jump shapes, and boolean expressions are rebuilt
//! from the jump targets of their comparisons. Where several sources give the
//! same code, any of them is fine; the round trip check compares the result.

use super::ast::is_identifier;
use super::ast::{BinOp, Block, CallExpr, Expr, FuncExpr, Line, Stat, TableExpr, TableItem, UnOp};
use crate::resource::lua::chunk::{Constant, Proto};
use crate::resource::lua::opcodes::{FIELDS_PER_FLUSH, MAXSTACK, Op, ins};

pub type Res<T> = Result<T, String>;

/// A table constructor being filled.
#[derive(Debug, Clone)]
struct TableBuild {
    /// Items with the pc that completed them and their line.
    items: Vec<(TableItem, usize, u32)>,
    /// `C` of `NEWTABLE`: log2 of the number of `name = value` items, plus one.
    c: u32,
    end_line: u32,
    /// The `NEWTABLE` instruction.
    start: usize,
}

#[derive(Debug, Clone)]
enum Kind {
    Value,
    /// `SELF`: the method name; `expr` is the object.
    Method(Vec<u8>),
    /// `SELF`: the hidden self argument.
    SelfArg,
    /// First register of a call with several results (`count` registers).
    MultiHead,
    /// Further registers of such a call.
    MultiPart,
    Table(Box<TableBuild>),
    /// A closure; true if it captures the register it is stored in.
    Closure(bool, usize),
}

#[derive(Debug, Clone)]
struct Pend {
    expr: Expr,
    kind: Kind,
    /// Instruction that completed the value.
    pc: usize,
}

#[derive(Debug, Clone)]
struct State {
    regs: Vec<Option<Pend>>,
    stats: Vec<Line>,
    /// Stores of a multiple assignment, in instruction order.
    assign: Vec<(Expr, Expr)>,
    /// Smallest line of the instructions of the statement in progress.
    line: u32,
}

impl State {
    fn new(size: usize) -> Self {
        Self {
            regs: vec![None; size + 3],
            stats: Vec::new(),
            assign: Vec::new(),
            line: u32::MAX,
        }
    }

    fn pending(&self) -> bool {
        self.regs.iter().any(Option::is_some)
    }

    fn idle(&self) -> bool {
        !self.pending() && self.assign.is_empty()
    }

    fn push(&mut self, stat: Stat) {
        let line = if self.line == u32::MAX { 0 } else { self.line };
        self.stats.push(Line { line, stat });
        self.line = u32::MAX;
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Ctx {
    /// Target of `break` in the innermost loop.
    break_to: Option<usize>,
    /// Body of a `repeat`: loop start and exit.
    until: Option<(usize, usize)>,
    /// The parent already declared the locals that start at this pc.
    declared: Option<usize>,
}

/// One comparison or test of a condition, with its jump.
#[derive(Debug, Clone)]
struct Atom {
    /// First instruction of the operand code.
    start: usize,
    test: usize,
    target: usize,
    cond: Expr,
    /// The jump is taken when `cond` has this truth value.
    jump_when: bool,
    /// `TEST` that copies its operand into a register (`A`), or `None`.
    copy_to: Option<u32>,
    is_test: bool,
    test_c: u32,
    /// First source line of the operand code.
    line: u32,
}

/// The atoms of a condition with their jump classification.
struct Graph<'x> {
    atoms: &'x [Atom],
    /// Where control goes after atom `k - 1` falls through.
    fall: &'x dyn Fn(usize) -> Exit,
    /// Where the jump of an atom leads.
    class: &'x dyn Fn(&Atom) -> Exit,
}

/// Where a jump of a condition leads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Exit {
    True,
    False,
    At(usize),
}

pub struct FnDec<'a> {
    p: &'a Proto,
    float: bool,
    loc_reg: Vec<u32>,
    implicit: Vec<bool>,
    /// First pc after the code of the body (the final `RETURN`).
    end: usize,
}

fn op(i: u32) -> Option<Op> {
    ins::op(i)
}

impl<'a> FnDec<'a> {
    pub fn new(p: &'a Proto, float: bool) -> Res<Self> {
        if p.code.is_empty() {
            return Err("function without code".into());
        }
        if p.locals.is_empty() && p.num_params > 0 {
            return Err("the chunk has no debug information (stripped)".into());
        }
        // Registers follow the stack of active variables of the compiler: a
        // variable is pushed when declared and popped when its scope ends. A
        // variable whose scope is empty at the very end (all scopes close
        // there together) does not pop the ones below it.
        let mut loc_reg = Vec::with_capacity(p.locals.len());
        let mut stack: Vec<usize> = Vec::new();
        for (i, v) in p.locals.iter().enumerate() {
            while let Some(&top) = stack.last() {
                let w = &p.locals[top];
                let ended =
                    w.end_pc < v.start_pc || (w.end_pc == v.start_pc && v.end_pc > v.start_pc);
                if !ended {
                    break;
                }
                stack.pop();
            }
            loc_reg.push(stack.len() as u32);
            stack.push(i);
        }
        let mut implicit = vec![false; p.locals.len()];
        let params = p.num_params as usize + p.is_vararg as usize;
        for flag in implicit.iter_mut().take(params) {
            *flag = true;
        }
        for (pc, &i) in p.code.iter().enumerate() {
            let (body, base, count) = match op(i) {
                Some(Op::ForLoop) => {
                    let body = (pc as i64 + 1 + ins::sbx(i) as i64) as usize;
                    (body, ins::a(i), 3)
                }
                Some(Op::TForPrep) => {
                    let target = (pc as i64 + 1 + ins::sbx(i) as i64) as usize;
                    let c = p.code.get(target).map_or(0, |&t| ins::c(t));
                    (pc + 1, ins::a(i), c + 3)
                }
                _ => continue,
            };
            for (k, v) in p.locals.iter().enumerate() {
                if v.start_pc as usize == body && (base..base + count).contains(&loc_reg[k]) {
                    implicit[k] = true;
                }
            }
        }
        let last = p.code.len() - 1;
        let end = if op(p.code[last]) == Some(Op::Return) {
            last
        } else {
            p.code.len()
        };
        Ok(Self {
            p,
            float,
            loc_reg,
            implicit,
            end,
        })
    }

    /// Decompiles the function body.
    pub fn function(&self) -> Res<FuncExpr> {
        let p = self.p;
        let params = p
            .locals
            .iter()
            .take(p.num_params as usize)
            .map(|v| v.name.clone())
            .collect();
        let mut body = self.block(0, self.end, Ctx::default()).map_err(|e| {
            if e.starts_with("function at line") {
                e
            } else {
                format!("function at line {}: {e}", p.line_defined)
            }
        })?;
        body.end_line = p.line_info.last().copied().unwrap_or(0);
        Ok(FuncExpr {
            params,
            vararg: p.is_vararg,
            body,
            end_line: p.line_info.last().copied().unwrap_or(0),
            line: p.line_defined,
        })
    }

    // ------------------------------------------------------------------ helpers

    fn code(&self, pc: usize) -> Option<(Op, u32)> {
        let i = *self.p.code.get(pc)?;
        Some((op(i)?, i))
    }

    fn line(&self, pc: usize) -> u32 {
        self.p.line_info.get(pc).copied().unwrap_or(0)
    }

    fn jump_target(&self, pc: usize) -> Option<usize> {
        let (o, i) = self.code(pc)?;
        if !matches!(o, Op::Jmp | Op::ForLoop | Op::TForPrep) {
            return None;
        }
        usize::try_from(pc as i64 + 1 + ins::sbx(i) as i64).ok()
    }

    fn is_test(&self, pc: usize) -> bool {
        self.code(pc).is_some_and(|(o, _)| o.is_test())
            && self.code(pc + 1).is_some_and(|(o, _)| o == Op::Jmp)
    }

    /// First source line of the code in `from..=to`: the smallest line of its
    /// instructions and of the functions defined there.
    fn span_line(&self, from: usize, to: usize) -> u32 {
        let mut line = u32::MAX;
        for pc in from..=to.min(self.p.code.len().saturating_sub(1)) {
            line = line.min(self.line(pc));
            if let Some((Op::Closure, i)) = self.code(pc)
                && let Some(p) = self.p.protos.get(ins::bx(i) as usize)
            {
                line = line.min(p.line_defined);
            }
        }
        if line == u32::MAX { 0 } else { line }
    }

    fn finish_table(&self, t: TableBuild) -> TableExpr {
        finish_table_items(t, |from, to| self.span_line(from, to))
    }

    /// `LOADBOOL r 0 1; LOADBOOL r 1 0` at `pc`: the value of a comparison.
    fn is_bool_pair(&self, pc: usize) -> bool {
        match (self.code(pc), self.code(pc + 1)) {
            (Some((Op::LoadBool, f)), Some((Op::LoadBool, t))) => {
                ins::b(f) == 0
                    && ins::c(f) == 1
                    && ins::b(t) == 1
                    && ins::c(t) == 0
                    && ins::a(f) == ins::a(t)
            }
            _ => false,
        }
    }

    /// A jump to `actual` that the compiler meant for `expected`: the same
    /// position, or `expected` holds a jump to `actual` and `luaK_jump`
    /// joined the two.
    fn same_target(&self, actual: usize, expected: usize) -> bool {
        actual == expected || self.plain_jump(expected) == Some(actual)
    }

    /// An unconditional jump (not the jump of a comparison).
    fn plain_jump(&self, pc: usize) -> Option<usize> {
        if self.code(pc)?.0 != Op::Jmp || (pc > 0 && self.code(pc - 1)?.0.is_test()) {
            return None;
        }
        self.jump_target(pc)
    }

    fn active_local(&self, reg: u32, pc: usize) -> Option<usize> {
        self.p
            .locals
            .iter()
            .enumerate()
            .rev()
            .find(|(k, v)| {
                self.loc_reg[*k] == reg && v.start_pc as usize <= pc && pc < v.end_pc as usize
            })
            .map(|(k, _)| k)
    }

    fn konst(&self, k: u32) -> Res<Expr> {
        Ok(match self.p.constants.get(k as usize) {
            Some(Constant::Nil) => Expr::Nil,
            Some(Constant::Number(n)) => Expr::Number(*n),
            Some(Constant::String(s)) => Expr::String(s.clone()),
            None => return Err(format!("constant {k} out of range")),
        })
    }

    fn global_name(&self, k: u32) -> Res<Expr> {
        match self.konst(k)? {
            Expr::String(s) => Ok(Expr::Name(s)),
            _ => Err(format!("global name constant {k} is not a string")),
        }
    }

    fn upvalue(&self, b: u32) -> Res<Expr> {
        self.p
            .upvalue_names
            .get(b as usize)
            .map(|n| Expr::Name(n.clone()))
            .ok_or_else(|| format!("upvalue {b} has no name"))
    }

    fn take(&self, st: &mut State, reg: u32, pc: usize) -> Res<Pend> {
        st.regs
            .get_mut(reg as usize)
            .and_then(Option::take)
            .ok_or_else(|| format!("pc {}: register {reg} has no value", pc + 1))
    }

    /// Reads a register as a single value (consuming a pending value).
    fn read(&self, st: &mut State, reg: u32, pc: usize) -> Res<Expr> {
        if let Some(k) = self.active_local(reg, pc) {
            return Ok(Expr::Name(self.p.locals[k].name.clone()));
        }
        let pend = self.take(st, reg, pc)?;
        self.finish_value(pend, pc)
    }

    /// Reads the value of a store, which may be a result of a multi-value call
    /// (`a, b = f()` stores each result).
    fn read_stored(&self, st: &mut State, reg: u32, pc: usize) -> Res<Expr> {
        if self.active_local(reg, pc).is_none()
            && let Some(Some(pend)) = st.regs.get(reg as usize)
            && matches!(pend.kind, Kind::MultiHead | Kind::MultiPart)
        {
            let pend = self.take(st, reg, pc)?;
            return Ok(match pend.kind {
                Kind::MultiHead => pend.expr,
                _ => Expr::Nil.multi_part(),
            });
        }
        self.read(st, reg, pc)
    }

    fn finish_value(&self, pend: Pend, pc: usize) -> Res<Expr> {
        match pend.kind {
            Kind::Value | Kind::Closure(..) => Ok(pend.expr),
            Kind::Table(mut t) => {
                // A store right after the constructor has the line of its `}`.
                if t.end_line == 0
                    && let Some((Op::SetGlobal | Op::SetUpval | Op::SetTable | Op::Move, _)) =
                        self.code(pc)
                {
                    t.end_line = self.line(pc);
                }
                Ok(Expr::Table(Box::new(self.finish_table(*t))))
            }
            other => Err(format!("pc {}: unexpected {other:?} value", pc + 1)),
        }
    }

    fn rk(&self, st: &mut State, x: u32, pc: usize) -> Res<Expr> {
        if x >= MAXSTACK {
            self.konst(x - MAXSTACK)
        } else {
            self.read(st, x, pc)
        }
    }

    fn write(&self, st: &mut State, reg: u32, pc: usize, expr: Expr, kind: Kind) -> Res<()> {
        if let Some(k) = self.active_local(reg, pc) {
            let value = self.finish_value(Pend { expr, kind, pc }, pc)?;
            let name = Expr::Name(self.p.locals[k].name.clone());
            return self.store(st, name, value, pc);
        }
        let slot = st
            .regs
            .get_mut(reg as usize)
            .ok_or_else(|| format!("pc {}: register {reg} out of range", pc + 1))?;
        *slot = Some(Pend { expr, kind, pc });
        Ok(())
    }

    fn store(&self, st: &mut State, target: Expr, value: Expr, pc: usize) -> Res<()> {
        st.assign.push((target, value));
        if !st.pending() {
            self.flush_assign(st, pc);
        }
        Ok(())
    }

    fn flush_assign(&self, st: &mut State, pc: usize) {
        let mut stores = std::mem::take(&mut st.assign);
        stores.reverse();
        let (targets, values): (Vec<Expr>, Vec<Expr>) = stores.into_iter().unzip();
        let values = collapse_multi(values);
        if let ([target], [Expr::Function(func)]) = (targets.as_slice(), values.as_slice())
            && is_function_name(target)
            && self.line(pc) == func.line
        {
            let method = matches!(target, Expr::Index(_, k) if matches!(&**k, Expr::String(_)))
                && func.params.first().is_some_and(|p| p == b"self");
            st.push(Stat::Function {
                target: target.clone(),
                method,
                func: (**func).clone(),
            });
            return;
        }
        st.push(Stat::Assign { targets, values });
    }

    /// Values of registers `from..from+n` for an expression list.
    fn take_list(&self, st: &mut State, from: u32, n: u32, pc: usize) -> Res<Vec<Expr>> {
        let mut out = Vec::new();
        for r in from..from + n {
            if self.active_local(r, pc).is_some() {
                out.push(self.read(st, r, pc)?);
                continue;
            }
            let pend = self.take(st, r, pc)?;
            match pend.kind {
                Kind::MultiHead => out.push(pend.expr),
                Kind::MultiPart => out.push(Expr::Nil.multi_part()),
                _ => out.push(self.finish_value(pend, pc)?),
            }
        }
        Ok(collapse_multi(out))
    }

    /// Values from `from` up to the open call that ends them.
    fn take_open_list(&self, st: &mut State, from: u32, pc: usize) -> Res<Vec<Expr>> {
        let mut n = 0;
        while st
            .regs
            .get((from + n) as usize)
            .is_some_and(Option::is_some)
        {
            n += 1;
        }
        let values = self.take_list(st, from, n, pc)?;
        match values.last() {
            Some(Expr::Call(c)) if c.multi => Ok(values),
            _ => Err(format!("pc {}: open value list without a call", pc + 1)),
        }
    }

    // --------------------------------------------------------------- expressions

    /// Executes one straight-line instruction (or a whole value expression
    /// with jumps) and returns the next pc.
    fn exec(&self, st: &mut State, pc: usize) -> Res<usize> {
        let (o, i) = self
            .code(pc)
            .ok_or_else(|| format!("pc {}: bad opcode", pc + 1))?;
        let (a, b, c) = (ins::a(i), ins::b(i), ins::c(i));
        st.line = st.line.min(self.line(pc));
        match o {
            Op::Move => {
                let v = self.read_any(st, b, pc)?;
                self.write(st, a, pc, v.0, v.1)?;
            }
            Op::LoadK => {
                let k = self.konst(ins::bx(i))?;
                self.write(st, a, pc, k, Kind::Value)?;
            }
            Op::LoadBool => {
                if c != 0 {
                    return Err(format!("pc {}: LOADBOOL outside a comparison", pc + 1));
                }
                let v = if b != 0 { Expr::True } else { Expr::False };
                self.write(st, a, pc, v, Kind::Value)?;
            }
            Op::LoadNil => {
                for r in a..=b {
                    self.write(st, r, pc, Expr::Nil, Kind::Value)?;
                }
            }
            Op::GetUpval => {
                let v = self.upvalue(b)?;
                self.write(st, a, pc, v, Kind::Value)?;
            }
            Op::GetGlobal => {
                let v = self.global_name(ins::bx(i))?;
                self.write(st, a, pc, v, Kind::Value)?;
            }
            Op::GetTable => {
                let key = self.rk(st, c, pc)?;
                let obj = self.read(st, b, pc)?;
                self.write(
                    st,
                    a,
                    pc,
                    Expr::Index(Box::new(obj), Box::new(key)),
                    Kind::Value,
                )?;
            }
            Op::SetGlobal => {
                let v = self.read_stored(st, a, pc)?;
                let name = self.global_name(ins::bx(i))?;
                self.store(st, name, v, pc)?;
            }
            Op::SetUpval => {
                let v = self.read_stored(st, a, pc)?;
                let name = self.upvalue(b)?;
                self.store(st, name, v, pc)?;
            }
            Op::SetTable => {
                let is_table = self.active_local(a, pc).is_none()
                    && matches!(
                        st.regs.get(a as usize),
                        Some(Some(Pend {
                            kind: Kind::Table(_),
                            ..
                        }))
                    );
                let value = if c >= MAXSTACK {
                    self.konst(c - MAXSTACK)?
                } else {
                    self.read_stored(st, c, pc)?
                };
                let key = self.rk(st, b, pc)?;
                if is_table {
                    let line = self.line(pc);
                    if let Some(Some(Pend {
                        kind: Kind::Table(t),
                        ..
                    })) = st.regs.get_mut(a as usize)
                    {
                        let item = match key {
                            Expr::String(k) if is_identifier(&k) => TableItem::Named(k, value),
                            key => TableItem::Keyed(key, value),
                        };
                        t.items.push((item, pc, line));
                    }
                } else {
                    let obj = self.read(st, a, pc)?;
                    self.store(st, Expr::Index(Box::new(obj), Box::new(key)), value, pc)?;
                }
            }
            Op::NewTable => {
                let t = TableBuild {
                    items: Vec::new(),
                    c,
                    end_line: 0,
                    start: pc,
                };
                self.write(st, a, pc, Expr::Nil, Kind::Table(Box::new(t)))?;
            }
            Op::SelfOp => {
                let key = self.rk(st, c, pc)?;
                let obj = self.read(st, b, pc)?;
                let Expr::String(name) = key else {
                    return Err(format!("pc {}: SELF with a non-string key", pc + 1));
                };
                st.regs[a as usize] = Some(Pend {
                    expr: obj,
                    kind: Kind::Method(name),
                    pc,
                });
                st.regs[a as usize + 1] = Some(Pend {
                    expr: Expr::Nil,
                    kind: Kind::SelfArg,
                    pc,
                });
            }
            Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Pow => {
                let r = self.rk(st, c, pc)?;
                let l = self.rk(st, b, pc)?;
                let op = match o {
                    Op::Add => BinOp::Add,
                    Op::Sub => BinOp::Sub,
                    Op::Mul => BinOp::Mul,
                    Op::Div => BinOp::Div,
                    _ => BinOp::Pow,
                };
                self.write(
                    st,
                    a,
                    pc,
                    Expr::Bin(op, Box::new(l), Box::new(r)),
                    Kind::Value,
                )?;
            }
            Op::Unm | Op::Not => {
                let v = self.read(st, b, pc)?;
                let op = if o == Op::Unm { UnOp::Minus } else { UnOp::Not };
                self.write(st, a, pc, Expr::Un(op, Box::new(v)), Kind::Value)?;
            }
            Op::Concat => {
                let mut parts = Vec::new();
                for r in (b..=c).rev() {
                    parts.push(self.read(st, r, pc)?);
                }
                let mut e = parts.remove(0);
                for p in parts {
                    e = Expr::Bin(BinOp::Concat, Box::new(p), Box::new(e));
                }
                self.write(st, a, pc, e, Kind::Value)?;
            }
            Op::Call | Op::TailCall => {
                let call = self.call(st, a, b, c, pc)?;
                if o == Op::TailCall {
                    st.push(Stat::Return(vec![Expr::Call(Box::new(CallExpr {
                        multi: true,
                        ..call
                    }))]));
                    // The `RETURN A 0` that follows is part of the tail call.
                    if self.code(pc + 1).is_some_and(|(o, _)| o == Op::Return) {
                        return Ok(pc + 2);
                    }
                    return Ok(pc + 1);
                }
                match c {
                    1 => {
                        if st.pending() {
                            return Err(format!(
                                "pc {}: call statement inside an expression",
                                pc + 1
                            ));
                        }
                        st.push(Stat::Call(call));
                    }
                    2 => self.write(st, a, pc, Expr::Call(Box::new(call)), Kind::Value)?,
                    _ => {
                        let multi = Expr::Call(Box::new(CallExpr {
                            multi: true,
                            ..call
                        }));
                        st.regs[a as usize] = Some(Pend {
                            expr: multi,
                            kind: Kind::MultiHead,
                            pc,
                        });
                        for r in 1..c.saturating_sub(1) {
                            st.regs[(a + r) as usize] = Some(Pend {
                                expr: Expr::Nil,
                                kind: Kind::MultiPart,
                                pc,
                            });
                        }
                    }
                }
            }
            Op::Return => {
                let mut values = if b == 0 {
                    self.take_open_list(st, a, pc)?
                } else {
                    self.take_list(st, a, b - 1, pc)?
                };
                if b != 0 {
                    paren_last_call(&mut values);
                }
                if st.pending() {
                    return Err(format!("pc {}: values left before return", pc + 1));
                }
                st.push(Stat::Return(values));
            }
            Op::SetList | Op::SetListO => {
                let bx = ins::bx(i);
                let values = if o == Op::SetListO {
                    let mut n = 0;
                    while st
                        .regs
                        .get((a + 1 + n) as usize)
                        .is_some_and(Option::is_some)
                    {
                        n += 1;
                    }
                    self.positional(st, a + 1, n, pc)?
                } else {
                    let n = bx % FIELDS_PER_FLUSH + 1;
                    self.positional(st, a + 1, n, pc)?
                };
                let line = self.line(pc);
                match st.regs.get_mut(a as usize) {
                    Some(Some(Pend {
                        kind: Kind::Table(t),
                        ..
                    })) => {
                        for (e, at, l) in values {
                            t.items.push((TableItem::Positional(e), at, l));
                        }
                        t.end_line = line;
                    }
                    _ => return Err(format!("pc {}: SETLIST without a table", pc + 1)),
                }
            }
            Op::Closure => {
                let proto =
                    self.p.protos.get(ins::bx(i) as usize).ok_or_else(|| {
                        format!("pc {}: closure {} out of range", pc + 1, ins::bx(i))
                    })?;
                let nups = proto.num_upvalues as usize;
                let mut own = false;
                for k in 0..nups {
                    match self.code(pc + 1 + k) {
                        Some((Op::Move, u)) => own |= ins::b(u) == a,
                        Some((Op::GetUpval, _)) => {}
                        _ => return Err(format!("pc {}: closure without upvalue moves", pc + 1)),
                    }
                }
                let func = FnDec::new(proto, self.float)?.function()?;
                st.line = st.line.min(proto.line_defined);
                self.write(
                    st,
                    a,
                    pc,
                    Expr::Function(Box::new(func)),
                    Kind::Closure(own, ins::bx(i) as usize),
                )?;
                return Ok(pc + 1 + nups);
            }
            Op::Close => {}
            Op::Eq | Op::Lt | Op::Le | Op::Test => {
                return self.value_region(st, pc);
            }
            Op::Jmp | Op::ForLoop | Op::TForLoop | Op::TForPrep => {
                return Err(format!("pc {}: unexpected {}", pc + 1, o.name()));
            }
        }
        Ok(pc + 1)
    }

    /// Like `read` but keeps the kind of a pending closure or table (for `MOVE`).
    fn read_any(&self, st: &mut State, reg: u32, pc: usize) -> Res<(Expr, Kind)> {
        if self.active_local(reg, pc).is_some() {
            return Ok((self.read(st, reg, pc)?, Kind::Value));
        }
        let pend = self.take(st, reg, pc)?;
        match pend.kind {
            Kind::Closure(..) => Ok((pend.expr, Kind::Value)),
            Kind::MultiHead => Ok((pend.expr, Kind::Value)),
            Kind::MultiPart => Ok((Expr::Nil.multi_part(), Kind::Value)),
            _ => Ok((self.finish_value(pend, pc)?, Kind::Value)),
        }
    }

    fn positional(
        &self,
        st: &mut State,
        from: u32,
        n: u32,
        pc: usize,
    ) -> Res<Vec<(Expr, usize, u32)>> {
        let mut out = Vec::new();
        for r in from..from + n {
            let pend = self.take(st, r, pc)?;
            let at = pend.pc;
            let line = self.line(at);
            let e = match pend.kind {
                Kind::MultiHead => pend.expr,
                _ => self.finish_value(pend, pc)?,
            };
            out.push((e, at, line));
        }
        if let Some((last @ Expr::Call(_), ..)) = out.last_mut()
            && let Expr::Call(c) = last
            && !c.multi
            && ins::op(self.p.code[pc]) == Some(Op::SetList)
            && self.is_last_flush(pc)
        {
            *last = Expr::Paren(Box::new(last.clone()));
        }
        Ok(out)
    }

    /// A `SETLIST` that is the last flush of its constructor (not followed by more items).
    fn is_last_flush(&self, pc: usize) -> bool {
        let bx = ins::bx(self.p.code[pc]);
        !(bx + 1).is_multiple_of(FIELDS_PER_FLUSH) || {
            // A full flush is last unless more items follow; they would end in
            // another SETLIST on the same table register.
            let a = ins::a(self.p.code[pc]);
            !self.p.code[pc + 1..].iter().any(|&i| {
                matches!(op(i), Some(Op::SetList | Op::SetListO))
                    && ins::a(i) == a
                    && ins::bx(i) > bx
            })
        }
    }

    fn call(&self, st: &mut State, a: u32, b: u32, _c: u32, pc: usize) -> Res<CallExpr> {
        let head = self.take(st, a, pc)?;
        let (func, method, first) = match head.kind {
            Kind::Method(name) => {
                self.take(st, a + 1, pc)?;
                (head.expr, Some(name), a + 2)
            }
            _ => (self.finish_value(head, pc)?, None, a + 1),
        };
        let args = if b == 0 {
            self.take_open_list(st, first, pc)?
        } else {
            let mut args = self.take_list(st, first, (a + b).saturating_sub(first), pc)?;
            paren_last_call(&mut args);
            args
        };
        Ok(CallExpr {
            func,
            method,
            args,
            multi: false,
        })
    }

    // ------------------------------------------------------------ conditions

    /// The condition of a comparison or test at `pc` (operands consumed from `st`).
    fn atom(&self, st: &mut State, pc: usize, start: usize) -> Res<Atom> {
        let (o, i) = self.code(pc).ok_or("bad test")?;
        let (a, b, c) = (ins::a(i), ins::b(i), ins::c(i));
        let target = self
            .jump_target(pc + 1)
            .ok_or_else(|| format!("pc {}: test without a jump", pc + 1))?;
        st.line = st.line.min(self.line(pc));
        let (cond, jump_when, copy_to) = match o {
            Op::Test => {
                let v = self.read(st, b, pc)?;
                (v, c != 0, (a != b).then_some(a))
            }
            _ => {
                let r = self.rk(st, c, pc)?;
                let l = self.rk(st, b, pc)?;
                let swapped = match (b >= MAXSTACK, c >= MAXSTACK) {
                    (true, false) => true,
                    (false, true) => false,
                    (true, true) => b > c,
                    (false, false) => b > c && self.active_local(b, pc).is_none(),
                };
                let (op, l, r) = match (o, swapped) {
                    (Op::Eq, false) => (BinOp::Eq, l, r),
                    (Op::Eq, true) => (BinOp::Eq, r, l),
                    (Op::Lt, false) => (BinOp::Lt, l, r),
                    (Op::Lt, true) => (BinOp::Gt, r, l),
                    (Op::Le, false) => (BinOp::Le, l, r),
                    _ => (BinOp::Ge, r, l),
                };
                (Expr::Bin(op, Box::new(l), Box::new(r)), a != 0, None)
            }
        };
        Ok(Atom {
            start,
            test: pc,
            target,
            cond,
            jump_when,
            copy_to,
            is_test: o == Op::Test,
            test_c: c,
            line: self.span_line(start, pc),
        })
    }

    /// Scans conditions from `from`: operand code, test, jump; repeatedly.
    fn scan_atoms(&self, from: usize, limit: usize) -> Vec<Atom> {
        let mut atoms = Vec::new();
        let mut start = from;
        let mut st = State::new(self.p.max_stack as usize);
        let mut pc = from;
        while pc < limit {
            if pc != start && self.starts_locals(pc) {
                break;
            }
            if self.is_test(pc) {
                let mut trial = st.clone();
                if let Ok(next) = self.value_region(&mut trial, pc)
                    && !atoms
                        .iter()
                        .any(|a: &Atom| a.target > pc && a.target < next)
                {
                    // A value with jumps inside an operand.
                    st = trial;

                    pc = next;
                    continue;
                }
                match self.atom(&mut st, pc, start) {
                    Ok(atom) if !st.pending() && st.stats.is_empty() => atoms.push(atom),
                    _ => break,
                }
                pc += 2;
                start = pc;
                continue;
            }
            match self.code(pc) {
                Some((Op::Jmp | Op::ForLoop | Op::TForLoop | Op::TForPrep | Op::Return, _)) => {
                    break;
                }
                Some((Op::TailCall, _)) | None => break,
                _ => {}
            }
            match self.exec(&mut st, pc) {
                Ok(next) if st.stats.is_empty() && st.assign.is_empty() => pc = next,
                _ => break,
            }
        }
        atoms
    }

    /// Builds the boolean expression of atoms `lo..hi` whose true and false
    /// exits are `t` and `f`; `fall` is where the last atom falls through.
    fn build(
        &self,
        g: &Graph<'_>,
        lo: usize,
        hi: usize,
        t: Exit,
        f: Exit,
        tail: Option<&Expr>,
    ) -> Option<Expr> {
        let items = hi - lo + tail.is_some() as usize;
        if items == 0 {
            return None;
        }
        if hi == lo {
            return tail.cloned();
        }
        if items == 1 {
            let a = &g.atoms[lo];
            let fall_to = (g.fall)(lo + 1);
            let jump_to = (g.class)(a);
            let positive = if fall_to == t && jump_to == f {
                !a.jump_when
            } else if fall_to == f && jump_to == t {
                a.jump_when
            } else {
                return None;
            };
            let cond = if positive {
                a.cond.clone()
            } else {
                negate(a.cond.clone())
            };
            return Some(Expr::At(a.line, Box::new(cond)));
        }
        for k in (lo + 1..=hi).rev() {
            if k == hi && tail.is_none() {
                continue;
            }
            let split = (g.fall)(k);
            if let Some(l) = self.build(g, lo, k, split, f, None)
                && let Some(r) = self.build(g, k, hi, t, f, tail)
            {
                return Some(Expr::Bin(BinOp::And, Box::new(l), Box::new(r)));
            }
            if let Some(l) = self.build(g, lo, k, t, split, None)
                && let Some(r) = self.build(g, k, hi, t, f, tail)
            {
                return Some(Expr::Bin(BinOp::Or, Box::new(l), Box::new(r)));
            }
        }
        None
    }

    /// A condition from `atoms[..n]` whose last atom falls through to the
    /// `fall_exit` and whose exits are the positions `t_at` and `f_at`.
    fn condition(
        &self,
        atoms: &[Atom],
        n: usize,
        t_at: usize,
        f_at: usize,
        last_falls_true: bool,
    ) -> Option<Expr> {
        let atoms = &atoms[..n];
        let after = atoms.last()?.test + 2;
        let fall = |k: usize| {
            if k >= n {
                if last_falls_true {
                    Exit::True
                } else {
                    Exit::False
                }
            } else {
                Exit::At(atoms[k].start)
            }
        };
        let class = |a: &Atom| {
            if a.target == f_at && (a.target != after || !last_falls_true || f_at == t_at) {
                Exit::False
            } else if a.target == t_at {
                Exit::True
            } else if a.target == after {
                if last_falls_true {
                    Exit::True
                } else {
                    Exit::False
                }
            } else {
                // A jump joined (threaded) with the jump at an exit goes where it goes.
                if self.plain_jump(f_at) == Some(a.target) {
                    Exit::False
                } else if self.plain_jump(t_at) == Some(a.target) {
                    Exit::True
                } else {
                    Exit::At(a.target)
                }
            }
        };
        // Internal targets must be the start of a later atom.
        for a in atoms {
            if let Exit::At(p) = class(a)
                && !atoms.iter().any(|b| b.start == p && b.start > a.test)
            {
                return None;
            }
        }
        let g = Graph {
            atoms,
            fall: &fall,
            class: &class,
        };
        self.build(&g, 0, n, Exit::True, Exit::False, None)
            .map(simplify)
    }

    /// A value expression with jumps (`a and b`, `x == y`, `t or {}`) that
    /// starts with the test at `pc`; on success its value is written to its
    /// register and the pc after it is returned.
    fn value_region(&self, st: &mut State, pc: usize) -> Res<usize> {
        let first_target = self.jump_target(pc + 1).ok_or("test without a jump")?;
        if first_target <= pc + 1 {
            return Err("backward jump".into());
        }
        let mut end = first_target;
        let mut q = pc + 2;
        while q < end {
            if let Some(t) = self.jump_target(q) {
                if self.code(q).is_some_and(|(o, _)| o != Op::Jmp) || t <= q {
                    return Err("loop inside a value".into());
                }
                end = end.max(t);
            }
            q += 1;
        }
        // Comparisons jump to `LOADBOOL r 1 0`, the last instruction.
        if self.is_bool_pair(end.wrapping_sub(1)) {
            end += 1;
        }
        let (loads, tail_end) = match (
            self.code(end.wrapping_sub(2)),
            self.code(end.wrapping_sub(1)),
        ) {
            (Some((Op::LoadBool, f)), Some((Op::LoadBool, t)))
                if end >= pc + 4
                    && ins::b(f) == 0
                    && ins::c(f) == 1
                    && ins::b(t) == 1
                    && ins::c(t) == 0
                    && ins::a(f) == ins::a(t) =>
            {
                let reg = ins::a(f);
                let tail_end = if self.plain_jump(end - 3) == Some(end) {
                    end - 3
                } else {
                    end - 2
                };
                (Some((reg, end - 2, end - 1)), tail_end)
            }
            _ => (None, end),
        };
        // Atoms and the tail value.
        let mut atoms = vec![];
        let mut work = st.clone();
        atoms.push(self.atom(&mut work, pc, pc)?);
        let mut at = pc + 2;
        let mut tail = None;
        let mut sub = State::new(self.p.max_stack as usize);
        let mut sub_start = at;
        while at < tail_end {
            if self.is_test(at) {
                let mut trial = sub.clone();
                if let Ok(next) = self.value_region(&mut trial, at)
                    && next <= tail_end
                    // Earlier atoms cannot jump into the middle of an operand.
                    && !atoms.iter().any(|a: &Atom| a.target > at && a.target < next)
                {
                    sub = trial;
                    at = next;
                    continue;
                }
                let atom = self.atom(&mut sub, at, sub_start)?;
                if sub.pending() || !sub.stats.is_empty() || !sub.assign.is_empty() {
                    return Err("pending values around a condition".into());
                }
                atoms.push(atom);
                at += 2;
                sub_start = at;
                continue;
            }
            if self.starts_locals(at) && at != pc + 2 {
                return Err("local declaration inside a value".into());
            }
            at = self.exec(&mut sub, at)?;
            if !sub.stats.is_empty() || !sub.assign.is_empty() {
                return Err("statement inside a value".into());
            }
        }
        if at != tail_end {
            return Err("value overruns its end".into());
        }
        let reg = match loads {
            Some((reg, ..)) => reg,
            None => atoms
                .iter()
                .find(|a| a.target == end)
                .and_then(|a| a.copy_to.or(a.is_test.then(|| ins::b(self.p.code[a.test]))))
                .ok_or("no register for the value")?,
        };
        let has_tail = sub_start < tail_end || loads.is_none();
        if has_tail {
            let pending: Vec<u32> = (0..sub.regs.len() as u32)
                .filter(|&r| sub.regs[r as usize].is_some())
                .collect();
            if pending != [reg] {
                return Err(format!("value tail leaves registers {pending:?}"));
            }
            let value = self.read(&mut sub, reg, tail_end.saturating_sub(1))?;
            tail = Some(Expr::At(
                self.span_line(sub_start, tail_end.saturating_sub(1)),
                Box::new(value),
            ));
        } else if sub.pending() {
            return Err("pending values in a comparison value".into());
        }
        // Classify jumps: value tests to `end` carry the operand.
        let (pf, pt) = match loads {
            Some((_, pf, pt)) => (Some(pf), Some(pt)),
            None => (None, None),
        };
        let tail_start = sub_start;
        let n = atoms.len();
        let fall = |k: usize| {
            if k >= n {
                if tail.is_some() {
                    Exit::At(tail_start)
                } else {
                    Exit::False
                }
            } else {
                Exit::At(atoms[k].start)
            }
        };
        let class = |a: &Atom| {
            if a.target == end {
                if a.is_test && a.test_c != 0 {
                    Exit::True
                } else {
                    Exit::False
                }
            } else if Some(a.target) == pt {
                Exit::True
            } else if Some(a.target) == pf {
                Exit::False
            } else {
                Exit::At(a.target)
            }
        };
        for a in &atoms {
            if a.target == end
                && !(a.is_test && a.copy_to.is_some_and(|r| r == reg)
                    || a.is_test && ins::b(self.p.code[a.test]) == reg)
            {
                return Err("jump to the end of a value from a comparison".into());
            }
        }
        let g = Graph {
            atoms: &atoms,
            fall: &fall,
            class: &class,
        };
        let expr = self
            .build(&g, 0, n, Exit::True, Exit::False, tail.as_ref())
            .ok_or("cannot rebuild the boolean expression")?;
        *st = work;
        st.line = st.line.min(sub.line);
        let last = end - 1;
        self.write(st, reg, last, simplify(expr), Kind::Value)?;
        Ok(end)
    }

    // ---------------------------------------------------------------- statements

    fn starts_locals(&self, pc: usize) -> bool {
        self.p
            .locals
            .iter()
            .enumerate()
            .any(|(k, v)| v.start_pc as usize == pc && !self.implicit[k])
    }

    /// Declares the explicit locals that start at `pc`; returns their scope end.
    fn declare_locals(&self, st: &mut State, pc: usize) -> Res<Option<usize>> {
        let mut vars: Vec<usize> = (0..self.p.locals.len())
            .filter(|&k| self.p.locals[k].start_pc as usize == pc && !self.implicit[k])
            .collect();
        if vars.is_empty() {
            return Ok(None);
        }
        vars.sort_by_key(|&k| self.loc_reg[k]);
        if !st.assign.is_empty() {
            return Err(format!(
                "pc {}: assignment pending at a local declaration",
                pc + 1
            ));
        }
        let first = self.loc_reg[vars[0]];
        let names: Vec<Vec<u8>> = vars
            .iter()
            .map(|&k| self.p.locals[k].name.clone())
            .collect();
        let scope_end = vars.iter().map(|&k| self.p.locals[k].end_pc as usize).max();
        if names.len() == 1
            && let Some(Some(Pend {
                kind: Kind::Closure(own, index),
                expr: Expr::Function(func),
                ..
            })) = st.regs.get(first as usize)
            && (*own || !uses_global(&self.p.protos[*index], &names[0]))
        {
            let func = (**func).clone();
            st.regs[first as usize] = None;
            if st.pending() {
                return Err(format!("pc {}: values left at a local function", pc + 1));
            }
            st.push(Stat::LocalFunction {
                name: names[0].clone(),
                func,
            });
            return Ok(scope_end);
        }
        let mut values = Vec::new();
        for (i, _) in vars.iter().enumerate() {
            let r = first + i as u32;
            let pend = self.take(st, r, pc.saturating_sub(1))?;
            values.push(match pend.kind {
                Kind::MultiHead => pend.expr,
                Kind::MultiPart => Expr::Nil.multi_part(),
                _ => self.finish_value(pend, pc)?,
            });
        }
        // More expressions than names (`local a = 1, 2`): the extra values
        // are computed into the registers above and dropped.
        let mut r = first + vars.len() as u32;
        while st.regs.get(r as usize).is_some_and(Option::is_some) {
            let pend = self.take(st, r, pc)?;
            values.push(self.finish_value(pend, pc)?);
            r += 1;
        }
        if st.pending() {
            return Err(format!("pc {}: values left at a local declaration", pc + 1));
        }
        let mut values = collapse_multi(values);
        if values.iter().all(|v| *v == Expr::Nil) && values.len() <= names.len() {
            values.clear();
        }
        st.push(Stat::Local { names, values });
        Ok(scope_end)
    }

    /// Decompiles `start..end` into a block.
    fn block(&self, start: usize, end: usize, ctx: Ctx) -> Res<Block> {
        let (block, _) = self.block_until(start, end, ctx)?;
        Ok(block)
    }

    /// Decompiles a block; in a `repeat` body also returns the `until` condition.
    fn block_until(&self, start: usize, end: usize, ctx: Ctx) -> Res<(Block, Option<Expr>)> {
        let mut st = State::new(self.p.max_stack as usize);
        let mut pc = start;
        let mut stmt_start = start;
        let mut until = None;
        let skip = ctx.declared;
        let ctx = Ctx {
            declared: None,
            ..ctx
        };
        let inner = Ctx { until: None, ..ctx };
        while pc < end {
            let declared = if skip == Some(pc) {
                None
            } else {
                self.declare_locals(&mut st, pc)?
            };
            if let Some(scope_end) = declared
                && scope_end < end
                // A `CLOSE` at the end of the block belongs to the block itself.
                && !(scope_end..end).all(|q| self.code(q).is_some_and(|(o, _)| o == Op::Close))
                && scope_end >= pc
                && !self.is_until(ctx, scope_end)
            {
                // The locals live in a `do ... end` block.
                let decl = st.stats.pop().expect("just declared");
                let body_ctx = Ctx {
                    declared: Some(pc),
                    ..inner
                };
                let mut body = self.block(pc, scope_end, body_ctx)?;
                body.stats.insert(0, decl);
                body.end_line = self.line(scope_end);
                st.line = st.line.min(body.stats[0].line);
                st.push(Stat::Do(body));
                pc = scope_end;
                stmt_start = pc;
                continue;
            }
            if st.idle() {
                stmt_start = pc;
                st.line = u32::MAX;
                if let Some((loop_start, exit)) = ctx.until
                    && let Some(cond) = self.until_condition(pc, loop_start, exit)
                {
                    until = Some(cond);
                    pc = exit;
                    break;
                }
                if let Some(next) = self.control(&mut st, pc, end, ctx)? {
                    pc = next;
                    continue;
                }
            }
            if self.is_test(pc) {
                let mut trial = st.clone();
                if let Ok(next) = self.value_region(&mut trial, pc) {
                    st = trial;
                    pc = next;
                    continue;
                }
                pc = self.if_statement(&mut st, pc, stmt_start, end, ctx)?;
                continue;
            }
            if let Some(next) = self.for_loop(&mut st, pc, end, ctx)? {
                pc = next;
                continue;
            }
            pc = self.exec(&mut st, pc)?;
        }
        if pc > end {
            return Err(format!("block overruns its end at pc {}", end + 1));
        }
        self.declare_locals(&mut st, end)?;
        if !st.idle() {
            return Err(format!("pc {}: values left at the end of a block", end + 1));
        }
        let mut stats = st.stats;
        wrap_early_exits(&mut stats);
        Ok((
            Block {
                stats,
                end_line: self.line(end),
            },
            until,
        ))
    }

    fn is_until(&self, ctx: Ctx, pc: usize) -> bool {
        ctx.until
            .is_some_and(|(s, x)| self.until_condition(pc, s, x).is_some())
    }

    fn until_condition(&self, pc: usize, loop_start: usize, exit: usize) -> Option<Expr> {
        if self.plain_jump(pc) == Some(loop_start) && pc + 1 == exit {
            return Some(Expr::False);
        }
        let atoms = self.scan_atoms(pc, exit);
        let n = atoms.iter().position(|a| a.test + 2 == exit)? + 1;
        if atoms[n - 1].target != loop_start {
            return None;
        }
        self.condition(&atoms, n, exit, loop_start, true)
    }

    /// Statements that start with a jump or a loop: `break`, `while`, `repeat`.
    fn control(&self, st: &mut State, pc: usize, end: usize, ctx: Ctx) -> Res<Option<usize>> {
        // repeat ... until: a later condition jumps back here.
        if ctx.until.is_none_or(|(s, _)| s != pc) {
            let back = (pc + 1..end)
                .filter(|&j| {
                    self.code(j).is_some_and(|(o, _)| o == Op::Jmp)
                        && self.jump_target(j) == Some(pc)
                        && (self.code(j - 1).is_some_and(|(o, _)| o.is_test())
                            || self.plain_jump(j).is_some())
                })
                .max();
            if let Some(j) = back {
                let exit = j + 1;
                let body_ctx = Ctx {
                    break_to: Some(exit),
                    until: Some((pc, exit)),
                    declared: Some(pc),
                };
                let (body, cond) = self.block_until(pc, exit, body_ctx)?;
                let cond = cond.ok_or_else(|| format!("pc {}: repeat without until", pc + 1))?;
                st.line = self.line(pc);
                st.push(Stat::Repeat { body, cond });
                return Ok(Some(exit));
            }
        }
        let Some(target) = self.plain_jump(pc) else {
            return Ok(None);
        };
        if ctx.break_to.is_some_and(|b| self.same_target(target, b)) {
            st.line = self.line(pc);
            st.push(Stat::Break);
            return Ok(Some(pc + 1));
        }
        if target > pc && target <= end {
            // while: the condition after the body jumps back to its start.
            let body_start = pc + 1;
            let (cond, exit) = if self.plain_jump(target) == Some(body_start) {
                (Some(Expr::True), target + 1)
            } else {
                let atoms = self.scan_atoms(target, end);
                let mut found = (None, 0);
                for n in (1..=atoms.len()).rev() {
                    if atoms[n - 1].target != body_start {
                        continue;
                    }
                    let exit = atoms[n - 1].test + 2;
                    if let Some(c) = self.condition(&atoms, n, body_start, exit, false) {
                        found = (Some(c), exit);
                        break;
                    }
                }
                found
            };
            if let Some(cond) = cond
                && exit <= end
            {
                let body = self.block(
                    body_start,
                    target,
                    Ctx {
                        break_to: Some(exit),
                        until: None,
                        declared: None,
                    },
                )?;
                st.line = self.line(pc);
                st.push(Stat::While { cond, body });
                return Ok(Some(exit));
            }
            // `if false then ... end`: a jump over a block.
            let then = self.block(body_start, target, Ctx { until: None, ..ctx })?;
            st.line = self.line(pc);
            st.push(Stat::If {
                cond: Expr::False,
                then,
                otherwise: None,
            });
            return Ok(Some(target));
        }
        Err(format!("pc {}: unexpected jump to {}", pc + 1, target + 1))
    }

    /// `for` loops, recognised at their preparation instructions.
    fn for_loop(&self, st: &mut State, pc: usize, end: usize, ctx: Ctx) -> Res<Option<usize>> {
        let Some((o, i)) = self.code(pc) else {
            return Ok(None);
        };
        let a = ins::a(i);
        let _ = ctx;
        if o == Op::Sub && ins::b(i) == a && ins::c(i) == a + 2 {
            let Some(forloop) = self.plain_jump(pc + 1) else {
                return Ok(None);
            };
            let ok = self
                .code(forloop)
                .is_some_and(|(o, f)| o == Op::ForLoop && ins::a(f) == a)
                && self.jump_target(forloop) == Some(pc + 2);
            if !ok || forloop >= end {
                return Ok(None);
            }
            st.line = st.line.min(self.line(pc));
            let step = self.read(st, a + 2, pc)?;
            let limit = self.read(st, a + 1, pc)?;
            let start = self.read(st, a, pc)?;
            if st.pending() || !st.assign.is_empty() {
                return Err(format!("pc {}: values left at a for loop", pc + 1));
            }
            let var = self
                .local_at(a, pc + 2)
                .ok_or_else(|| format!("pc {}: for loop without a variable", pc + 1))?;
            let step = (step != Expr::Number(1.0)).then_some(step);
            let body = self.block(
                pc + 2,
                forloop,
                Ctx {
                    break_to: Some(forloop + 1),
                    until: None,
                    declared: None,
                },
            )?;
            st.line = self.line(forloop);
            st.push(Stat::NumFor {
                var,
                start,
                limit,
                step,
                body,
            });
            return Ok(Some(forloop + 1));
        }
        if o == Op::TForPrep {
            let tfor = self.jump_target(pc).ok_or("bad TFORPREP")?;
            let (to, ti) = self.code(tfor).ok_or("TFORPREP out of range")?;
            if to != Op::TForLoop || self.jump_target(tfor + 1) != Some(pc + 1) || tfor + 2 > end {
                return Err(format!("pc {}: malformed generic for", pc + 1));
            }
            let nvars = ins::c(ti) + 3;
            let mut exprs = self.take_list(st, a, nvars, pc)?;
            while exprs.len() > 1
                && exprs.last() == Some(&Expr::Nil)
                && !matches!(exprs[exprs.len() - 2], Expr::Call(_))
            {
                exprs.pop();
            }
            if st.pending() || !st.assign.is_empty() {
                return Err(format!("pc {}: values left at a for loop", pc + 1));
            }
            let mut names = Vec::new();
            for r in a + 2..a + nvars {
                names.push(
                    self.local_at(r, pc + 1)
                        .ok_or_else(|| format!("pc {}: for loop without variables", pc + 1))?,
                );
            }
            let body = self.block(
                pc + 1,
                tfor,
                Ctx {
                    break_to: Some(tfor + 2),
                    until: None,
                    declared: None,
                },
            )?;
            st.line = self.line(tfor);
            st.push(Stat::GenFor { names, exprs, body });
            return Ok(Some(tfor + 2));
        }
        Ok(None)
    }

    fn local_at(&self, reg: u32, pc: usize) -> Option<Vec<u8>> {
        self.active_local(reg, pc)
            .map(|k| self.p.locals[k].name.clone())
    }

    /// An `if` statement whose condition ends with the test at `pc`.
    fn if_statement(
        &self,
        st: &mut State,
        pc: usize,
        stmt_start: usize,
        end: usize,
        ctx: Ctx,
    ) -> Res<usize> {
        let first = self.atom(st, pc, stmt_start)?;
        if st.pending() || !st.assign.is_empty() {
            return Err(format!(
                "pc {}: comparison inside an unsupported expression",
                pc + 1
            ));
        }
        let line = st.line;
        let mut atoms = vec![first];
        atoms.extend(self.scan_atoms(pc + 2, end));
        let inner = Ctx { until: None, ..ctx };
        let mut error = String::from("no condition fits the jumps");
        for n in (1..=atoms.len()).rev() {
            let then_start = atoms[n - 1].test + 2;
            let false_at = atoms[n - 1].target;
            let Some(cond) = self.condition(&atoms, n, then_start, false_at, true) else {
                continue;
            };
            match self.if_parts(cond, then_start, false_at, end, inner) {
                Ok((stat, next)) => {
                    st.line = line;
                    st.push(stat);
                    return Ok(next);
                }
                Err(e) => error = e,
            }
        }
        Err(format!(
            "pc {}: cannot rebuild the if statement ({error})",
            pc + 1
        ))
    }

    /// Candidate positions for a jump target that may have been threaded
    /// through unconditional jumps (`luaK_jump` joins pending jumps).
    fn unthreaded(&self, target: usize, lo: usize, end: usize) -> Vec<usize> {
        let mut out = Vec::new();
        if (lo..=end).contains(&target) {
            out.push(target);
        }
        for q in (lo..=end).rev() {
            if q != target && self.plain_jump(q) == Some(target) {
                out.push(q);
            }
        }
        out
    }

    fn if_parts(
        &self,
        cond: Expr,
        then_start: usize,
        false_at: usize,
        end: usize,
        ctx: Ctx,
    ) -> Res<(Stat, usize)> {
        let mut errors = Vec::new();
        for f in self.unthreaded(false_at, then_start, end) {
            // if ... else: an escape jump right before the else part.
            if f > then_start
                && let Some(escape) = self.plain_jump(f - 1)
                && escape >= f
                && !ctx.break_to.is_some_and(|b| self.same_target(escape, b))
            {
                for e in self.unthreaded(escape, f, end) {
                    let then = match self.block(then_start, f - 1, ctx) {
                        Ok(then) => then,
                        Err(e) => {
                            errors.push(e);
                            break;
                        }
                    };
                    match self.block(f, e, ctx) {
                        Ok(otherwise) => {
                            return Ok((
                                Stat::If {
                                    cond,
                                    then,
                                    otherwise: Some(otherwise),
                                },
                                e,
                            ));
                        }
                        Err(e) => errors.push(e),
                    }
                }
            }
            match self.block(then_start, f, ctx) {
                Ok(then) => {
                    return Ok((
                        Stat::If {
                            cond,
                            then,
                            otherwise: None,
                        },
                        f,
                    ));
                }
                Err(e) => errors.push(e),
            }
        }
        Err(errors
            .into_iter()
            .next()
            .unwrap_or_else(|| format!("no end for the jump to {}", false_at + 1)))
    }
}

impl Expr {
    /// Marker for the second and later results of a multi-value call.
    fn multi_part(self) -> Expr {
        Expr::Un(UnOp::Not, Box::new(Expr::Name(b"\0multi".to_vec())))
    }

    fn is_multi_part(&self) -> bool {
        matches!(self, Expr::Un(UnOp::Not, inner) if matches!(&**inner, Expr::Name(n) if n == b"\0multi"))
    }
}

/// Removes the markers of multi-value calls: the call stands for all of them.
fn collapse_multi(values: Vec<Expr>) -> Vec<Expr> {
    values.into_iter().filter(|v| !v.is_multi_part()).collect()
}

fn paren_last_call(values: &mut [Expr]) {
    if let Some(last) = values.last_mut()
        && let Expr::Call(c) = last
        && !c.multi
    {
        *last = Expr::Paren(Box::new(last.clone()));
    }
}

fn negate(e: Expr) -> Expr {
    match e {
        Expr::At(line, e) => Expr::At(line, Box::new(negate(*e))),
        Expr::Bin(BinOp::Eq, l, r) => Expr::Bin(BinOp::Ne, l, r),
        Expr::Bin(BinOp::Ne, l, r) => Expr::Bin(BinOp::Eq, l, r),
        Expr::Un(UnOp::Not, inner) if !inner.is_multi_part() => *inner,
        e => Expr::Un(UnOp::Not, Box::new(e)),
    }
}

/// Small rewrites that keep the code: `not (a == b)` → `a ~= b`.
fn simplify(e: Expr) -> Expr {
    match e {
        Expr::Un(UnOp::Not, inner) => match *inner {
            Expr::At(line, e) => Expr::At(line, Box::new(simplify(Expr::Un(UnOp::Not, e)))),
            Expr::Bin(BinOp::Eq, l, r) => Expr::Bin(BinOp::Ne, l, r),
            Expr::Bin(BinOp::Ne, l, r) => Expr::Bin(BinOp::Eq, l, r),
            other => Expr::Un(UnOp::Not, Box::new(simplify(other))),
        },
        Expr::Bin(op, l, r) => Expr::Bin(op, Box::new(simplify(*l)), Box::new(simplify(*r))),
        Expr::At(line, e) => Expr::At(line, Box::new(simplify(*e))),
        e => e,
    }
}

fn finish_table_items(mut t: TableBuild, line_of: impl Fn(usize, usize) -> u32) -> TableExpr {
    t.items.sort_by_key(|(_, pc, _)| *pc);
    // `NEWTABLE C` counts the `name = value` items: write the rest as `["name"] = value`.
    let named = t
        .items
        .iter()
        .filter(|(i, ..)| matches!(i, TableItem::Named(..)))
        .count();
    let fits = |n: usize| {
        (if n == 0 {
            0
        } else {
            32 - (n as u32).leading_zeros()
        }) == t.c
    };
    if !fits(named) {
        let mut keep = named;
        while keep > 0 && !fits(keep) {
            keep -= 1;
        }
        let mut seen = 0;
        for (item, ..) in &mut t.items {
            if let TableItem::Named(k, v) = item {
                seen += 1;
                if seen > keep {
                    *item = TableItem::Keyed(Expr::String(k.clone()), v.clone());
                }
            }
        }
    }
    // An item starts at the first line of the code after the previous item;
    // the `NEWTABLE` of a nested table has the line of the token before it.
    let mut prev = t.start;
    let mut items = Vec::with_capacity(t.items.len());
    for (item, pc, _) in t.items {
        let skip = matches!(item, TableItem::Positional(Expr::Table(_))) as usize;
        items.push((item, line_of(prev + 1 + skip, pc)));
        prev = pc;
    }
    TableExpr {
        items,
        end_line: t.end_line,
        open_line: line_of(t.start, t.start),
    }
}

fn is_function_name(e: &Expr) -> bool {
    match e {
        Expr::Name(_) => true,
        Expr::Index(obj, key) => {
            matches!(&**key, Expr::String(k) if is_identifier(k)) && is_function_name(obj)
        }
        _ => false,
    }
}

/// True if the function `p` (or a function inside it) reads or writes the global `name`.
fn uses_global(p: &Proto, name: &[u8]) -> bool {
    p.code.iter().any(|&i| {
        matches!(op(i), Some(Op::GetGlobal | Op::SetGlobal))
            && matches!(p.constants.get(ins::bx(i) as usize), Some(Constant::String(s)) if s == name)
    }) || p.protos.iter().any(|q| uses_global(q, name))
}

/// `return` and `break` must end a block: wrap earlier ones in `do ... end`.
fn wrap_early_exits(stats: &mut [Line]) {
    let n = stats.len();
    for (k, s) in stats.iter_mut().enumerate() {
        if k + 1 < n && matches!(s.stat, Stat::Return(_) | Stat::Break) {
            let inner = std::mem::replace(&mut s.stat, Stat::Break);
            s.stat = Stat::Do(Block {
                stats: vec![Line {
                    line: s.line,
                    stat: inner,
                }],
                end_line: s.line,
            });
        }
    }
}
