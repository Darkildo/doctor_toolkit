//! A small Lua 5.0 virtual machine for chunks made by [`super::compile`].
//!
//! It runs the bytecode the game runs: every opcode of Lua 5.0, closures
//! with upvalues, `...` as the `arg` table, `pcall`/`error`, metatables with
//! `__index`, and a subset of the base, string, table and math libraries.
//! Numbers are rounded to `f32` after every operation when the chunk has
//! 4-byte numbers, as in the game.
//!
//! It is not a full Lua: no coroutines, no garbage collection (values are
//! reference counted; cycles leak until the VM is dropped), no metamethods
//! besides `__index`, patterns in `string.find`/`gsub` are matched only as
//! plain text plus the classes `%w`, `%d`, `%a`, `%s` and `.`. It exists to
//! test generated scripts against a mock of the game API
//! ([`crate::script::sim`]) and to simulate them in the editor.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use super::chunk::{Chunk, Constant, Proto};
use super::opcodes::{FIELDS_PER_FLUSH, MAXSTACK, Op, ins};

/// Calls deeper than this fail with "stack overflow".
const MAX_DEPTH: usize = 180;

/// A Lua value.
#[derive(Clone, Default)]
pub enum Value {
    #[default]
    Nil,
    Bool(bool),
    Number(f64),
    Str(Rc<[u8]>),
    Table(TableRef),
    Function(Function),
}

impl Value {
    pub fn str(s: &str) -> Value {
        Value::Str(Rc::from(s.as_bytes()))
    }

    pub fn is_nil(&self) -> bool {
        matches!(self, Value::Nil)
    }

    /// `false` and `nil` are false, everything else is true.
    pub fn truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }

    /// Name of the type as `type()` returns it.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::Str(_) => "string",
            Value::Table(_) => "table",
            Value::Function(_) => "function",
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            Value::Str(s) => parse_number(&String::from_utf8_lossy(s)),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<String> {
        match self {
            Value::Str(s) => Some(String::from_utf8_lossy(s).into_owned()),
            _ => None,
        }
    }

    pub fn as_table(&self) -> Option<&TableRef> {
        match self {
            Value::Table(t) => Some(t),
            _ => None,
        }
    }

    fn raw_eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Number(a), Value::Number(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Table(a), Value::Table(b)) => Rc::ptr_eq(&a.0, &b.0),
            (Value::Function(a), Value::Function(b)) => a.ptr() == b.ptr(),
            _ => false,
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.raw_eq(other)
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Nil => write!(f, "nil"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Number(n) => write!(f, "{}", format_g(*n, 14)),
            Value::Str(s) => write!(f, "{:?}", String::from_utf8_lossy(s)),
            Value::Table(t) => write!(f, "table: {:p}", Rc::as_ptr(&t.0)),
            Value::Function(func) => write!(f, "function: {:#x}", func.ptr()),
        }
    }
}

impl From<f64> for Value {
    fn from(n: f64) -> Self {
        Value::Number(n)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::str(s)
    }
}

/// A native function: gets the VM and the arguments, returns the results.
pub type NativeFn = dyn Fn(&mut Vm, Vec<Value>) -> Result<Vec<Value>, LuaError>;

/// A function value: a Lua closure or a native function.
#[derive(Clone)]
pub enum Function {
    Lua(Rc<Closure>),
    Native(Rc<NativeFn>),
}

impl Function {
    fn ptr(&self) -> usize {
        match self {
            Function::Lua(c) => Rc::as_ptr(c) as usize,
            Function::Native(n) => Rc::as_ptr(n) as *const u8 as usize,
        }
    }
}

/// A Lua function with its upvalues.
pub struct Closure {
    proto: Rc<Proto>,
    upvalues: Vec<UpvalRef>,
    source: Rc<str>,
}

type UpvalRef = Rc<RefCell<Upval>>;

enum Upval {
    /// Still a register: absolute stack index.
    Open(usize),
    Closed(Value),
}

/// A table: values by key in insertion order, and an optional metatable.
#[derive(Clone, Default)]
pub struct TableRef(pub Rc<RefCell<Table>>);

impl TableRef {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, key: &Value) -> Value {
        self.0.borrow().get(key)
    }

    pub fn get_str(&self, key: &str) -> Value {
        self.get(&Value::str(key))
    }

    pub fn set(&self, key: Value, value: Value) -> Result<(), LuaError> {
        self.0.borrow_mut().set(key, value)
    }

    pub fn set_str(&self, key: &str, value: Value) {
        // A string key is always valid.
        let _ = self.set(Value::str(key), value);
    }

    /// `#t` / `table.getn`: the field `n` when it is a number, otherwise
    /// the last index of the sequence `1..`.
    pub fn len(&self) -> usize {
        let t = self.0.borrow();
        if let Value::Number(n) = t.get(&Value::str("n")) {
            return n.max(0.0) as usize;
        }
        let mut n = 0;
        while !t.get(&Value::Number((n + 1) as f64)).is_nil() {
            n += 1;
        }
        n
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn metatable(&self) -> Option<TableRef> {
        self.0.borrow().meta.clone()
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Key {
    Bool(bool),
    Num(u64),
    Str(Rc<[u8]>),
    Ptr(usize),
}

/// Contents of a table.
#[derive(Default)]
pub struct Table {
    slots: Vec<(Value, Value)>,
    index: HashMap<Key, usize>,
    meta: Option<TableRef>,
}

impl Table {
    fn key(v: &Value) -> Option<Key> {
        Some(match v {
            Value::Nil => return None,
            Value::Bool(b) => Key::Bool(*b),
            Value::Number(n) => {
                if n.is_nan() {
                    return None;
                }
                Key::Num(if *n == 0.0 { 0 } else { n.to_bits() })
            }
            Value::Str(s) => Key::Str(s.clone()),
            Value::Table(t) => Key::Ptr(Rc::as_ptr(&t.0) as *const u8 as usize),
            Value::Function(f) => Key::Ptr(f.ptr()),
        })
    }

    pub fn get(&self, key: &Value) -> Value {
        Self::key(key)
            .and_then(|k| self.index.get(&k))
            .map(|&i| self.slots[i].1.clone())
            .unwrap_or_default()
    }

    pub fn set(&mut self, key: Value, value: Value) -> Result<(), LuaError> {
        let Some(k) = Self::key(&key) else {
            return Err(LuaError::msg(if key.is_nil() {
                "table index is nil"
            } else {
                "table index is NaN"
            }));
        };
        match self.index.get(&k) {
            Some(&i) => self.slots[i].1 = value,
            None if value.is_nil() => {}
            None => {
                self.index.insert(k, self.slots.len());
                self.slots.push((key, value));
            }
        }
        Ok(())
    }

    /// The pair after `key` (`next`), skipping removed (nil) entries.
    fn next(&self, key: &Value) -> Result<Option<(Value, Value)>, LuaError> {
        let start = if key.is_nil() {
            0
        } else {
            match Self::key(key).and_then(|k| self.index.get(&k)) {
                Some(&i) => i + 1,
                None => return Err(LuaError::msg("invalid key to `next'")),
            }
        };
        Ok(self.slots[start..]
            .iter()
            .find(|(_, v)| !v.is_nil())
            .cloned())
    }
}

/// An error raised by Lua code (`error(v)`) or by the VM.
#[derive(Clone, Debug)]
pub struct LuaError {
    /// The error value: the message with its position for runtime errors.
    pub value: Value,
    /// Source line of the instruction that failed, when known.
    pub line: Option<u32>,
    /// A limit of the VM (instructions, depth) was reached: `pcall` does not catch it.
    pub fatal: bool,
}

impl LuaError {
    pub fn msg(text: impl Into<String>) -> Self {
        Self {
            value: Value::str(&text.into()),
            line: None,
            fatal: false,
        }
    }

    fn fatal(text: impl Into<String>) -> Self {
        Self {
            fatal: true,
            ..Self::msg(text)
        }
    }

    /// Text of the error value.
    pub fn text(&self) -> String {
        match &self.value {
            Value::Str(s) => String::from_utf8_lossy(s).into_owned(),
            v => format!("{v:?}"),
        }
    }
}

impl fmt::Display for LuaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.text())
    }
}

impl std::error::Error for LuaError {}

/// The virtual machine: one global table, one value stack.
pub struct Vm {
    stack: Vec<Value>,
    globals: TableRef,
    open: Vec<UpvalRef>,
    depth: usize,
    /// Round numbers to `f32` (the game's `lua_Number`).
    pub float: bool,
    /// Instructions executed so far.
    pub steps: u64,
    /// Fail with a fatal error after this many instructions.
    pub step_limit: u64,
    /// Lines passed to `print`.
    pub printed: Vec<String>,
    /// Significant digits of numbers turned into text (`LUA_NUMBER_FMT`
    /// `%.14g`; a float build of Lua may use fewer).
    pub number_digits: usize,
    rng: u64,
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Vm {
    /// A VM with the libraries open, float numbers, a limit of ten million instructions.
    pub fn new() -> Self {
        let mut vm = Self {
            stack: Vec::new(),
            globals: TableRef::new(),
            open: Vec::new(),
            depth: 0,
            float: true,
            steps: 0,
            step_limit: 10_000_000,
            printed: Vec::new(),
            number_digits: 14,
            rng: 0x2545_F491_4F6C_DD1D,
        };
        stdlib::open(&mut vm);
        vm
    }

    pub fn globals(&self) -> &TableRef {
        &self.globals
    }

    pub fn get_global(&self, name: &str) -> Value {
        self.globals.get_str(name)
    }

    pub fn set_global(&mut self, name: &str, value: Value) {
        self.globals.set_str(name, value);
    }

    /// Registers a native function as a global.
    pub fn register(
        &mut self,
        name: &str,
        f: impl Fn(&mut Vm, Vec<Value>) -> Result<Vec<Value>, LuaError> + 'static,
    ) {
        self.set_global(name, native(f));
    }

    /// Runs the main function of a chunk.
    pub fn run_chunk(&mut self, chunk: &Chunk) -> Result<Vec<Value>, LuaError> {
        self.float = chunk.number_size == 4;
        let source: Rc<str> = Rc::from(chunk.source().as_str());
        let f = Value::Function(Function::Lua(Rc::new(Closure {
            proto: Rc::new(chunk.main.clone()),
            upvalues: Vec::new(),
            source,
        })));
        self.call(&f, Vec::new())
    }

    /// Calls a function value.
    pub fn call(&mut self, f: &Value, args: Vec<Value>) -> Result<Vec<Value>, LuaError> {
        let Value::Function(func) = f else {
            return Err(LuaError::msg(format!(
                "attempt to call a {} value",
                f.type_name()
            )));
        };
        if self.depth >= MAX_DEPTH {
            return Err(LuaError::fatal("stack overflow"));
        }
        self.depth += 1;
        let r = match func {
            Function::Native(n) => {
                let n = n.clone();
                n(self, args)
            }
            Function::Lua(c) => {
                let c = c.clone();
                self.call_lua(&c, args)
            }
        };
        self.depth -= 1;
        r
    }

    /// Calls the global function `name`.
    pub fn call_global(&mut self, name: &str, args: Vec<Value>) -> Result<Vec<Value>, LuaError> {
        let f = self.get_global(name);
        self.call(&f, args)
    }

    fn num(&self, x: f64) -> f64 {
        if self.float { x as f32 as f64 } else { x }
    }

    /// Text of a value as `tostring` gives it.
    pub fn tostring(&self, v: &Value) -> String {
        match v {
            Value::Nil => "nil".into(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => format_g(*n, self.number_digits),
            Value::Str(s) => String::from_utf8_lossy(s).into_owned(),
            Value::Table(t) => format!("table: {:p}", Rc::as_ptr(&t.0)),
            Value::Function(f) => format!("function: {:#010x}", f.ptr() & 0xFFFF_FFFF),
        }
    }

    /// A pseudo-random number in `[0, 1)`, deterministic for tests.
    pub fn random(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 11) as f64 / (1u64 << 53) as f64
    }

    fn call_lua(&mut self, c: &Rc<Closure>, mut args: Vec<Value>) -> Result<Vec<Value>, LuaError> {
        let p = &c.proto;
        let base = self.stack.len();
        let nparams = p.num_params as usize;
        let extra = if args.len() > nparams {
            args.split_off(nparams)
        } else {
            Vec::new()
        };
        args.resize(nparams, Value::Nil);
        self.stack.extend(args);
        if p.is_vararg {
            let t = TableRef::new();
            let n = extra.len();
            for (i, v) in extra.into_iter().enumerate() {
                t.set(Value::Number((i + 1) as f64), v)?;
            }
            t.set_str("n", Value::Number(n as f64));
            self.stack.push(Value::Table(t));
        }
        let size = base + p.max_stack as usize + 1;
        if self.stack.len() < size {
            self.stack.resize(size, Value::Nil);
        }
        let r = self.execute(c, base);
        self.close_upvalues(base);
        self.stack.truncate(base);
        r
    }

    fn close_upvalues(&mut self, level: usize) {
        let stack = &self.stack;
        self.open.retain(|u| {
            let mut uv = u.borrow_mut();
            match *uv {
                Upval::Open(i) if i >= level => {
                    *uv = Upval::Closed(stack.get(i).cloned().unwrap_or_default());
                    false
                }
                _ => true,
            }
        });
    }

    fn find_upvalue(&mut self, index: usize) -> UpvalRef {
        for u in &self.open {
            if matches!(*u.borrow(), Upval::Open(i) if i == index) {
                return u.clone();
            }
        }
        let u = Rc::new(RefCell::new(Upval::Open(index)));
        self.open.push(u.clone());
        u
    }

    fn upvalue(&self, u: &UpvalRef) -> Value {
        match &*u.borrow() {
            Upval::Open(i) => self.stack[*i].clone(),
            Upval::Closed(v) => v.clone(),
        }
    }

    fn set_upvalue(&mut self, u: &UpvalRef, v: Value) {
        let mut uv = u.borrow_mut();
        match &mut *uv {
            Upval::Open(i) => self.stack[*i] = v,
            Upval::Closed(c) => *c = v,
        }
    }

    /// `t[k]` with `__index`.
    pub fn index(&mut self, t: &Value, k: &Value) -> Result<Value, LuaError> {
        let mut cur = t.clone();
        for _ in 0..100 {
            let Value::Table(tab) = &cur else {
                return Err(LuaError::msg(format!(
                    "attempt to index a {} value",
                    cur.type_name()
                )));
            };
            let v = tab.get(k);
            if !v.is_nil() {
                return Ok(v);
            }
            let Some(meta) = tab.metatable() else {
                return Ok(Value::Nil);
            };
            match meta.get_str("__index") {
                Value::Nil => return Ok(Value::Nil),
                f @ Value::Function(_) => {
                    let r = self.call(&f, vec![cur.clone(), k.clone()])?;
                    return Ok(r.into_iter().next().unwrap_or_default());
                }
                next => cur = next,
            }
        }
        Err(LuaError::msg("loop in __index"))
    }

    fn set_index(&mut self, t: &Value, k: Value, v: Value) -> Result<(), LuaError> {
        match t {
            Value::Table(tab) => tab.set(k, v),
            _ => Err(LuaError::msg(format!(
                "attempt to index a {} value",
                t.type_name()
            ))),
        }
    }

    fn arith(&self, op: Op, a: &Value, b: &Value) -> Result<Value, LuaError> {
        let (Some(x), Some(y)) = (a.as_number(), b.as_number()) else {
            let bad = if a.as_number().is_none() { a } else { b };
            return Err(LuaError::msg(format!(
                "attempt to perform arithmetic on a {} value",
                bad.type_name()
            )));
        };
        let r = match op {
            Op::Add => x + y,
            Op::Sub => x - y,
            Op::Mul => x * y,
            Op::Div => x / y,
            _ => x.powf(y),
        };
        Ok(Value::Number(self.num(r)))
    }

    fn less(&self, a: &Value, b: &Value, or_equal: bool) -> Result<bool, LuaError> {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => Ok(if or_equal { x <= y } else { x < y }),
            (Value::Str(x), Value::Str(y)) => Ok(if or_equal { x <= y } else { x < y }),
            _ => Err(LuaError::msg(format!(
                "attempt to compare {} with {}",
                a.type_name(),
                b.type_name()
            ))),
        }
    }

    fn concat_piece(&self, v: &Value) -> Result<Vec<u8>, LuaError> {
        match v {
            Value::Str(s) => Ok(s.to_vec()),
            Value::Number(n) => Ok(format_g(*n, self.number_digits).into_bytes()),
            _ => Err(LuaError::msg(format!(
                "attempt to concatenate a {} value",
                v.type_name()
            ))),
        }
    }

    fn execute(&mut self, c: &Rc<Closure>, base: usize) -> Result<Vec<Value>, LuaError> {
        let p = c.proto.clone();
        let mut pc = 0usize;
        let mut top = base;
        loop {
            let Some(&i) = p.code.get(pc) else {
                return Ok(Vec::new());
            };
            let line = p.line_info.get(pc).copied();
            pc += 1;
            self.steps += 1;
            if self.steps > self.step_limit {
                return Err(LuaError::fatal("instruction limit reached"));
            }
            match self.step(c, &p, base, i, &mut pc, &mut top) {
                Ok(Some(ret)) => return Ok(ret),
                Ok(None) => {}
                Err(mut e) => {
                    if e.line.is_none() {
                        e.line = line;
                        if let (Some(l), Value::Str(s)) = (line, &e.value)
                            && !e.fatal
                        {
                            let text = format!(
                                "{}:{l}: {}",
                                c.source.rsplit(['\\', '/']).next().unwrap_or(""),
                                String::from_utf8_lossy(s)
                            );
                            e.value = Value::str(&text);
                        }
                    }
                    return Err(e);
                }
            }
        }
    }

    fn rk(&self, p: &Proto, base: usize, x: u32) -> Value {
        if x >= MAXSTACK {
            constant(&p.constants[(x - MAXSTACK) as usize])
        } else {
            self.stack[base + x as usize].clone()
        }
    }

    fn ensure(&mut self, size: usize) {
        if self.stack.len() < size {
            self.stack.resize(size, Value::Nil);
        }
    }

    #[allow(clippy::too_many_lines)]
    fn step(
        &mut self,
        c: &Rc<Closure>,
        p: &Proto,
        base: usize,
        i: u32,
        pc: &mut usize,
        top: &mut usize,
    ) -> Result<Option<Vec<Value>>, LuaError> {
        let op = ins::op(i).ok_or_else(|| LuaError::fatal("bad opcode"))?;
        let a = ins::a(i) as usize;
        let ra = base + a;
        let b = ins::b(i);
        let cc = ins::c(i);
        match op {
            Op::Move => self.stack[ra] = self.stack[base + b as usize].clone(),
            Op::LoadK => self.stack[ra] = constant(&p.constants[ins::bx(i) as usize]),
            Op::LoadBool => {
                self.stack[ra] = Value::Bool(b != 0);
                if cc != 0 {
                    *pc += 1;
                }
            }
            Op::LoadNil => {
                for r in a..=b as usize {
                    self.stack[base + r] = Value::Nil;
                }
            }
            Op::GetUpval => self.stack[ra] = self.upvalue(&c.upvalues[b as usize]),
            Op::GetGlobal => {
                let k = constant(&p.constants[ins::bx(i) as usize]);
                let g = Value::Table(self.globals.clone());
                self.stack[ra] = self.index(&g, &k)?;
            }
            Op::GetTable => {
                let t = self.stack[base + b as usize].clone();
                let k = self.rk(p, base, cc);
                self.stack[ra] = self.index(&t, &k)?;
            }
            Op::SetGlobal => {
                let k = constant(&p.constants[ins::bx(i) as usize]);
                self.globals.set(k, self.stack[ra].clone())?;
            }
            Op::SetUpval => {
                let v = self.stack[ra].clone();
                self.set_upvalue(&c.upvalues[b as usize], v);
            }
            Op::SetTable => {
                let t = self.stack[ra].clone();
                let k = self.rk(p, base, b);
                let v = self.rk(p, base, cc);
                self.set_index(&t, k, v)?;
            }
            Op::NewTable => self.stack[ra] = Value::Table(TableRef::new()),
            Op::SelfOp => {
                let t = self.stack[base + b as usize].clone();
                let k = self.rk(p, base, cc);
                self.stack[ra + 1] = t.clone();
                self.stack[ra] = self.index(&t, &k)?;
            }
            Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Pow => {
                let x = self.rk(p, base, b);
                let y = self.rk(p, base, cc);
                self.stack[ra] = self.arith(op, &x, &y)?;
            }
            Op::Unm => {
                let x = self.stack[base + b as usize].clone();
                self.stack[ra] = self.arith(Op::Sub, &Value::Number(0.0), &x)?;
            }
            Op::Not => self.stack[ra] = Value::Bool(!self.stack[base + b as usize].truthy()),
            Op::Concat => {
                let mut out = Vec::new();
                for r in b..=cc {
                    out.extend(self.concat_piece(&self.stack[base + r as usize])?);
                }
                self.stack[ra] = Value::Str(Rc::from(out));
            }
            Op::Jmp => jump(pc, ins::sbx(i)),
            Op::Eq | Op::Lt | Op::Le => {
                let x = self.rk(p, base, b);
                let y = self.rk(p, base, cc);
                let r = match op {
                    Op::Eq => x.raw_eq(&y),
                    Op::Lt => self.less(&x, &y, false)?,
                    _ => self.less(&x, &y, true)?,
                };
                if r != (a != 0) {
                    *pc += 1;
                }
            }
            Op::Test => {
                let v = self.stack[base + b as usize].clone();
                if !v.truthy() == (cc != 0) {
                    *pc += 1;
                } else {
                    if a != super::opcodes::NO_REG as usize {
                        self.stack[ra] = v;
                    }
                    let next = p.code[*pc];
                    *pc += 1;
                    jump(pc, ins::sbx(next));
                }
            }
            Op::Call | Op::TailCall => {
                let f = self.stack[ra].clone();
                let end = if b == 0 { *top } else { ra + b as usize };
                let args = self.stack[ra + 1..end].to_vec();
                let results = self.call(&f, args)?;
                if op == Op::TailCall {
                    return Ok(Some(results));
                }
                if cc == 0 {
                    let n = results.len();
                    self.ensure(ra + n + 1);
                    for (k, v) in results.into_iter().enumerate() {
                        self.stack[ra + k] = v;
                    }
                    *top = ra + n;
                } else {
                    let want = cc as usize - 1;
                    let mut it = results.into_iter();
                    for k in 0..want {
                        self.stack[ra + k] = it.next().unwrap_or_default();
                    }
                }
            }
            Op::Return => {
                let end = if b == 0 { *top } else { ra + b as usize - 1 };
                return Ok(Some(self.stack[ra..end.max(ra)].to_vec()));
            }
            Op::ForLoop => {
                let (Some(idx), Some(limit), Some(step)) = (
                    self.stack[ra].as_number(),
                    self.stack[ra + 1].as_number(),
                    self.stack[ra + 2].as_number(),
                ) else {
                    return Err(LuaError::msg("`for' values must be numbers"));
                };
                let idx = self.num(idx + step);
                if if step > 0.0 {
                    idx <= limit
                } else {
                    idx >= limit
                } {
                    jump(pc, ins::sbx(i));
                    self.stack[ra] = Value::Number(idx);
                }
            }
            Op::TForLoop => {
                let f = self.stack[ra].clone();
                let args = vec![self.stack[ra + 1].clone(), self.stack[ra + 2].clone()];
                let mut it = self.call(&f, args)?.into_iter();
                for k in 0..=cc as usize {
                    self.stack[ra + 2 + k] = it.next().unwrap_or_default();
                }
                if self.stack[ra + 2].is_nil() {
                    *pc += 1;
                } else {
                    let next = p.code[*pc];
                    *pc += 1;
                    jump(pc, ins::sbx(next));
                }
            }
            Op::TForPrep => {
                if let Value::Table(_) = &self.stack[ra] {
                    self.stack[ra + 1] = self.stack[ra].clone();
                    self.stack[ra] = self.get_global("next");
                }
                jump(pc, ins::sbx(i));
            }
            Op::SetList | Op::SetListO => {
                let Value::Table(t) = self.stack[ra].clone() else {
                    return Err(LuaError::msg("SETLIST on a non-table"));
                };
                let bx = ins::bx(i);
                let n = if op == Op::SetList {
                    (bx % FIELDS_PER_FLUSH) as usize + 1
                } else {
                    top.saturating_sub(ra + 1)
                };
                let first = (bx - bx % FIELDS_PER_FLUSH) as usize;
                for k in 1..=n {
                    t.set(
                        Value::Number((first + k) as f64),
                        self.stack[ra + k].clone(),
                    )?;
                }
            }
            Op::Close => self.close_upvalues(ra),
            Op::Closure => {
                let proto = Rc::new(p.protos[ins::bx(i) as usize].clone());
                let mut upvalues = Vec::with_capacity(proto.num_upvalues as usize);
                for _ in 0..proto.num_upvalues {
                    let pseudo = p.code[*pc];
                    *pc += 1;
                    let ub = ins::b(pseudo) as usize;
                    if ins::op(pseudo) == Some(Op::GetUpval) {
                        upvalues.push(c.upvalues[ub].clone());
                    } else {
                        upvalues.push(self.find_upvalue(base + ub));
                    }
                }
                self.stack[ra] = Value::Function(Function::Lua(Rc::new(Closure {
                    proto,
                    upvalues,
                    source: c.source.clone(),
                })));
            }
        }
        Ok(None)
    }
}

fn jump(pc: &mut usize, offset: i32) {
    *pc = (*pc as i64 + offset as i64) as usize;
}

fn constant(k: &Constant) -> Value {
    match k {
        Constant::Nil => Value::Nil,
        Constant::Number(n) => Value::Number(*n),
        Constant::String(s) => Value::Str(Rc::from(s.as_slice())),
    }
}

/// A native function value.
pub fn native(f: impl Fn(&mut Vm, Vec<Value>) -> Result<Vec<Value>, LuaError> + 'static) -> Value {
    Value::Function(Function::Native(Rc::new(f)))
}

/// Number from text as `tonumber` reads it (decimal, `0x` hexadecimal).
pub fn parse_number(s: &str) -> Option<f64> {
    let t = s.trim();
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return u64::from_str_radix(h, 16).ok().map(|v| v as f64);
    }
    if t.is_empty() || t.contains(|c: char| c.is_ascii_alphabetic() && c != 'e' && c != 'E') {
        return None;
    }
    t.parse::<f64>().ok()
}

/// `printf("%.<precision>g", x)`.
pub fn format_g(x: f64, precision: usize) -> String {
    if x.is_nan() {
        return if x.is_sign_negative() { "-nan" } else { "nan" }.into();
    }
    if x.is_infinite() {
        return if x < 0.0 { "-inf" } else { "inf" }.into();
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0" } else { "0" }.into();
    }
    let p = precision.max(1);
    let sci = format!("{:.*e}", p - 1, x);
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if exp < -4 || exp >= p as i32 {
        let m = strip_zeros(mantissa);
        format!("{m}e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs())
    } else {
        let decimals = (p as i32 - 1 - exp).max(0) as usize;
        strip_zeros(&format!("{x:.decimals$}")).to_owned()
    }
}

fn strip_zeros(s: &str) -> &str {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        s
    }
}

/// The libraries: base, string, table and math (the parts scripts use).
mod stdlib {
    use super::*;

    fn arg(args: &[Value], i: usize) -> Value {
        args.get(i).cloned().unwrap_or_default()
    }

    fn num_arg(args: &[Value], i: usize, f: &str) -> Result<f64, LuaError> {
        arg(args, i).as_number().ok_or_else(|| {
            LuaError::msg(format!(
                "bad argument #{} to `{f}' (number expected)",
                i + 1
            ))
        })
    }

    fn str_arg(vm: &Vm, args: &[Value], i: usize, f: &str) -> Result<String, LuaError> {
        match arg(args, i) {
            v @ (Value::Str(_) | Value::Number(_)) => Ok(vm.tostring(&v)),
            _ => Err(LuaError::msg(format!(
                "bad argument #{} to `{f}' (string expected)",
                i + 1
            ))),
        }
    }

    fn table_arg(args: &[Value], i: usize, f: &str) -> Result<TableRef, LuaError> {
        arg(args, i).as_table().cloned().ok_or_else(|| {
            LuaError::msg(format!("bad argument #{} to `{f}' (table expected)", i + 1))
        })
    }

    fn one(v: Value) -> Result<Vec<Value>, LuaError> {
        Ok(vec![v])
    }

    pub fn open(vm: &mut Vm) {
        vm.register("print", |vm, args| {
            let line: Vec<String> = args.iter().map(|v| vm.tostring(v)).collect();
            vm.printed.push(line.join("\t"));
            Ok(Vec::new())
        });
        vm.register("type", |_, args| {
            if args.is_empty() {
                return Err(LuaError::msg("bad argument #1 to `type' (value expected)"));
            }
            one(Value::str(arg(&args, 0).type_name()))
        });
        vm.register("tostring", |vm, args| {
            one(Value::str(&vm.tostring(&arg(&args, 0))))
        });
        vm.register("tonumber", |_, args| {
            let v = arg(&args, 0);
            Ok(vec![match arg(&args, 1).as_number() {
                Some(base) if base != 10.0 => v
                    .as_str()
                    .and_then(|s| u64::from_str_radix(s.trim(), base as u32).ok())
                    .map_or(Value::Nil, |n| Value::Number(n as f64)),
                _ => v.as_number().map_or(Value::Nil, Value::Number),
            }])
        });
        vm.register("next", |_, args| {
            let t = table_arg(&args, 0, "next")?;
            let r = t.0.borrow().next(&arg(&args, 1))?;
            Ok(match r {
                Some((k, v)) => vec![k, v],
                None => vec![Value::Nil],
            })
        });
        vm.register("pairs", |vm, args| {
            let t = table_arg(&args, 0, "pairs")?;
            Ok(vec![vm.get_global("next"), Value::Table(t), Value::Nil])
        });
        let ipairs_step = native(|_, args| {
            let t = table_arg(&args, 0, "ipairs")?;
            let i = arg(&args, 1).as_number().unwrap_or(0.0) + 1.0;
            let v = t.get(&Value::Number(i));
            Ok(if v.is_nil() {
                vec![Value::Nil]
            } else {
                vec![Value::Number(i), v]
            })
        });
        vm.register("ipairs", move |_, args| {
            let t = table_arg(&args, 0, "ipairs")?;
            Ok(vec![
                ipairs_step.clone(),
                Value::Table(t),
                Value::Number(0.0),
            ])
        });
        vm.register("unpack", |_, args| {
            let t = table_arg(&args, 0, "unpack")?;
            Ok((1..=t.len())
                .map(|i| t.get(&Value::Number(i as f64)))
                .collect())
        });
        vm.register("pcall", |vm, mut args| {
            if args.is_empty() {
                return Err(LuaError::msg("bad argument #1 to `pcall' (value expected)"));
            }
            let f = args.remove(0);
            match vm.call(&f, args) {
                Ok(mut r) => {
                    r.insert(0, Value::Bool(true));
                    Ok(r)
                }
                Err(e) if e.fatal => Err(e),
                Err(e) => Ok(vec![Value::Bool(false), e.value]),
            }
        });
        vm.register("error", |_, args| {
            Err(LuaError {
                value: arg(&args, 0),
                line: None,
                fatal: false,
            })
        });
        vm.register("assert", |_, args| {
            if arg(&args, 0).truthy() {
                Ok(args)
            } else {
                Err(LuaError::msg(
                    arg(&args, 1)
                        .as_str()
                        .unwrap_or_else(|| "assertion failed!".into()),
                ))
            }
        });
        vm.register("setmetatable", |_, args| {
            let t = table_arg(&args, 0, "setmetatable")?;
            t.0.borrow_mut().meta = arg(&args, 1).as_table().cloned();
            one(Value::Table(t))
        });
        vm.register("getmetatable", |_, args| {
            Ok(vec![
                arg(&args, 0)
                    .as_table()
                    .and_then(TableRef::metatable)
                    .map_or(Value::Nil, Value::Table),
            ])
        });
        vm.register("rawget", |_, args| {
            one(table_arg(&args, 0, "rawget")?.get(&arg(&args, 1)))
        });
        vm.register("rawset", |_, args| {
            let t = table_arg(&args, 0, "rawset")?;
            t.set(arg(&args, 1), arg(&args, 2))?;
            one(Value::Table(t))
        });
        vm.register("loadfile", |_, _| {
            Ok(vec![Value::Nil, Value::str("cannot read file")])
        });

        let string = TableRef::new();
        let s = |f: fn(&mut Vm, Vec<Value>) -> Result<Vec<Value>, LuaError>| native(f);
        string.set_str(
            "len",
            s(|vm, a| one(Value::Number(str_arg(vm, &a, 0, "len")?.len() as f64))),
        );
        string.set_str(
            "lower",
            s(|vm, a| one(Value::str(&str_arg(vm, &a, 0, "lower")?.to_lowercase()))),
        );
        string.set_str(
            "upper",
            s(|vm, a| one(Value::str(&str_arg(vm, &a, 0, "upper")?.to_uppercase()))),
        );
        string.set_str(
            "rep",
            s(|vm, a| {
                let t = str_arg(vm, &a, 0, "rep")?;
                let n = num_arg(&a, 1, "rep")?.max(0.0) as usize;
                one(Value::str(&t.repeat(n.min(1 << 20))))
            }),
        );
        string.set_str(
            "sub",
            s(|vm, a| {
                let t = str_arg(vm, &a, 0, "sub")?.into_bytes();
                let len = t.len() as i64;
                let fix = |i: i64| if i < 0 { len + i + 1 } else { i };
                let i = fix(num_arg(&a, 1, "sub")? as i64).max(1);
                let j = fix(arg(&a, 2).as_number().map_or(-1, |n| n as i64)).min(len);
                Ok(vec![if i > j {
                    Value::str("")
                } else {
                    Value::Str(Rc::from(&t[(i - 1) as usize..j as usize]))
                }])
            }),
        );
        string.set_str(
            "format",
            s(|vm, a| {
                let f = str_arg(vm, &a, 0, "format")?;
                one(Value::str(&format(vm, &f, &a[1..])?))
            }),
        );
        string.set_str(
            "find",
            s(|vm, a| {
                let t = str_arg(vm, &a, 0, "find")?;
                let pat = str_arg(vm, &a, 1, "find")?;
                let init = arg(&a, 2).as_number().map_or(1, |n| n.max(1.0) as usize);
                let from = (init - 1).min(t.len());
                Ok(match pattern::find(t.as_bytes(), pat.as_bytes(), from) {
                    Some((s, e)) => vec![Value::Number((s + 1) as f64), Value::Number(e as f64)],
                    None => vec![Value::Nil],
                })
            }),
        );
        string.set_str(
            "gsub",
            s(|vm, a| {
                let t = str_arg(vm, &a, 0, "gsub")?;
                let pat = str_arg(vm, &a, 1, "gsub")?;
                let rep = str_arg(vm, &a, 2, "gsub")?;
                let (out, n) = pattern::gsub(t.as_bytes(), pat.as_bytes(), rep.as_bytes());
                Ok(vec![Value::Str(Rc::from(out)), Value::Number(n as f64)])
            }),
        );
        vm.set_global("string", Value::Table(string));

        let table = TableRef::new();
        table.set_str(
            "getn",
            s(|_, a| one(Value::Number(table_arg(&a, 0, "getn")?.len() as f64))),
        );
        table.set_str(
            "insert",
            s(|_, a| {
                let t = table_arg(&a, 0, "insert")?;
                let n = t.len();
                if a.len() >= 3 {
                    let pos = num_arg(&a, 1, "insert")? as usize;
                    for i in (pos..=n).rev() {
                        let v = t.get(&Value::Number(i as f64));
                        t.set(Value::Number((i + 1) as f64), v)?;
                    }
                    t.set(Value::Number(pos as f64), arg(&a, 2))?;
                } else {
                    t.set(Value::Number((n + 1) as f64), arg(&a, 1))?;
                }
                if let Value::Number(_) = t.get_str("n") {
                    t.set_str("n", Value::Number((n + 1) as f64));
                }
                Ok(Vec::new())
            }),
        );
        table.set_str(
            "remove",
            s(|_, a| {
                let t = table_arg(&a, 0, "remove")?;
                let n = t.len();
                if n == 0 {
                    return Ok(vec![Value::Nil]);
                }
                let pos = arg(&a, 1).as_number().map_or(n, |p| p as usize);
                let v = t.get(&Value::Number(pos as f64));
                for i in pos..n {
                    let next = t.get(&Value::Number((i + 1) as f64));
                    t.set(Value::Number(i as f64), next)?;
                }
                t.set(Value::Number(n as f64), Value::Nil)?;
                if let Value::Number(_) = t.get_str("n") {
                    t.set_str("n", Value::Number((n - 1) as f64));
                }
                one(v)
            }),
        );
        table.set_str(
            "concat",
            s(|vm, a| {
                let t = table_arg(&a, 0, "concat")?;
                let sep = arg(&a, 1).as_str().unwrap_or_default();
                let parts: Vec<String> = (1..=t.len())
                    .map(|i| vm.tostring(&t.get(&Value::Number(i as f64))))
                    .collect();
                one(Value::str(&parts.join(&sep)))
            }),
        );
        vm.set_global("table", Value::Table(table));

        let math = TableRef::new();
        let m1 = |name: &'static str, f: fn(f64) -> f64| {
            native(move |vm, a| {
                let x = num_arg(&a, 0, name)?;
                one(Value::Number(vm.num(f(x))))
            })
        };
        math.set_str("floor", m1("floor", f64::floor));
        math.set_str("ceil", m1("ceil", f64::ceil));
        math.set_str("abs", m1("abs", f64::abs));
        math.set_str("sqrt", m1("sqrt", f64::sqrt));
        math.set_str("sin", m1("sin", |x| x.to_radians().sin()));
        math.set_str("cos", m1("cos", |x| x.to_radians().cos()));
        math.set_str("rad", m1("rad", f64::to_radians));
        math.set_str("deg", m1("deg", f64::to_degrees));
        math.set_str("pi", Value::Number(std::f64::consts::PI as f32 as f64));
        math.set_str(
            "mod",
            s(|vm, a| {
                let x = num_arg(&a, 0, "mod")?;
                let y = num_arg(&a, 1, "mod")?;
                one(Value::Number(vm.num(x % y)))
            }),
        );
        math.set_str(
            "pow",
            s(|vm, a| {
                let x = num_arg(&a, 0, "pow")?;
                let y = num_arg(&a, 1, "pow")?;
                one(Value::Number(vm.num(x.powf(y))))
            }),
        );
        math.set_str(
            "min",
            s(|_, a| {
                let mut m = num_arg(&a, 0, "min")?;
                for i in 1..a.len() {
                    m = m.min(num_arg(&a, i, "min")?);
                }
                one(Value::Number(m))
            }),
        );
        math.set_str(
            "max",
            s(|_, a| {
                let mut m = num_arg(&a, 0, "max")?;
                for i in 1..a.len() {
                    m = m.max(num_arg(&a, i, "max")?);
                }
                one(Value::Number(m))
            }),
        );
        math.set_str(
            "random",
            s(|vm, a| {
                let r = vm.random();
                let v = match (arg(&a, 0).as_number(), arg(&a, 1).as_number()) {
                    (None, _) => r,
                    (Some(m), None) => (r * m).floor() + 1.0,
                    (Some(l), Some(u)) => (r * (u - l + 1.0)).floor() + l,
                };
                one(Value::Number(vm.num(v)))
            }),
        );
        vm.set_global("math", Value::Table(math));
    }

    /// `string.format` with `%d %i %u %c %x %X %o %e %E %f %g %G %q %s %%`.
    fn format(vm: &Vm, f: &str, args: &[Value]) -> Result<String, LuaError> {
        let mut out = String::new();
        let mut it = f.chars().peekable();
        let mut n = 0;
        while let Some(ch) = it.next() {
            if ch != '%' {
                out.push(ch);
                continue;
            }
            if it.peek() == Some(&'%') {
                it.next();
                out.push('%');
                continue;
            }
            let mut flags = String::new();
            while let Some(&c) = it.peek() {
                if "-+ #0".contains(c) {
                    flags.push(c);
                    it.next();
                } else {
                    break;
                }
            }
            let mut width = String::new();
            while let Some(&c) = it.peek().filter(|c| c.is_ascii_digit()) {
                width.push(c);
                it.next();
            }
            let mut precision: Option<usize> = None;
            if it.peek() == Some(&'.') {
                it.next();
                let mut p = String::new();
                while let Some(&c) = it.peek().filter(|c| c.is_ascii_digit()) {
                    p.push(c);
                    it.next();
                }
                precision = Some(p.parse().unwrap_or(0));
            }
            let conv = it
                .next()
                .ok_or_else(|| LuaError::msg("invalid format (ends with `%')"))?;
            let a = args.get(n).cloned().unwrap_or_default();
            n += 1;
            let num = || {
                a.as_number().ok_or_else(|| {
                    LuaError::msg(format!(
                        "bad argument #{} to `format' (number expected)",
                        n + 1
                    ))
                })
            };
            let mut body = match conv {
                'd' | 'i' => format!("{}", num()?.trunc() as i64),
                'u' => format!("{}", num()?.trunc() as i64 as u32),
                'c' => char::from(num()? as u8).to_string(),
                'x' => format!("{:x}", num()? as i64 as u32),
                'X' => format!("{:X}", num()? as i64 as u32),
                'o' => format!("{:o}", num()? as i64 as u32),
                'f' => format!("{:.*}", precision.unwrap_or(6), num()?),
                'e' | 'E' => {
                    let s = format!("{:.*e}", precision.unwrap_or(6), num()?);
                    let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
                    let e: i32 = e.parse().unwrap_or(0);
                    let t = format!("{m}e{}{:02}", if e < 0 { '-' } else { '+' }, e.abs());
                    if conv == 'E' { t.to_uppercase() } else { t }
                }
                'g' | 'G' => {
                    let t = format_g(num()?, precision.unwrap_or(6));
                    if conv == 'G' { t.to_uppercase() } else { t }
                }
                'q' => format!("{:?}", vm.tostring(&a)),
                's' => {
                    let t = vm.tostring(&a);
                    match precision {
                        Some(p) => t.chars().take(p).collect(),
                        None => t,
                    }
                }
                other => {
                    return Err(LuaError::msg(format!(
                        "invalid option `%{other}' to `format'"
                    )));
                }
            };
            if flags.contains('+') && "dife".contains(conv) && !body.starts_with('-') {
                body.insert(0, '+');
            }
            let w: usize = width.parse().unwrap_or(0);
            if body.chars().count() < w {
                let pad = w - body.chars().count();
                if flags.contains('-') {
                    body.push_str(&" ".repeat(pad));
                } else if flags.contains('0') && conv != 's' {
                    let sign = body.starts_with(['-', '+']);
                    let at = usize::from(sign);
                    body.insert_str(at, &"0".repeat(pad));
                } else {
                    body.insert_str(0, &" ".repeat(pad));
                }
            }
            out.push_str(&body);
        }
        Ok(out)
    }
}

/// The small subset of Lua patterns the VM understands: literal characters,
/// `.`, the classes `%a %d %l %s %u %w %p` and their complements, escaped
/// punctuation, sets `[...]` of those, and the anchors `^`/`$`. No captures
/// and no repetition.
mod pattern {
    fn class(c: u8, k: u8) -> bool {
        let r = match k.to_ascii_lowercase() {
            b'a' => c.is_ascii_alphabetic(),
            b'd' => c.is_ascii_digit(),
            b'l' => c.is_ascii_lowercase(),
            b's' => c.is_ascii_whitespace(),
            b'u' => c.is_ascii_uppercase(),
            b'w' => c.is_ascii_alphanumeric(),
            b'p' => c.is_ascii_punctuation(),
            _ => return c == k,
        };
        if k.is_ascii_uppercase() { !r } else { r }
    }

    /// One pattern item: returns whether `c` matches and the length of the item.
    fn item(c: u8, p: &[u8]) -> (bool, usize) {
        match p[0] {
            b'.' => (true, 1),
            b'%' if p.len() > 1 => (class(c, p[1]), 2),
            b'[' => {
                let mut i = 1;
                let negate = p.get(1) == Some(&b'^');
                if negate {
                    i += 1;
                }
                let mut hit = false;
                while i < p.len() && p[i] != b']' {
                    if p[i] == b'%' && i + 1 < p.len() {
                        hit |= class(c, p[i + 1]);
                        i += 2;
                    } else if i + 2 < p.len() && p[i + 1] == b'-' && p[i + 2] != b']' {
                        hit |= (p[i]..=p[i + 2]).contains(&c);
                        i += 3;
                    } else {
                        hit |= p[i] == c;
                        i += 1;
                    }
                }
                (hit != negate, (i + 1).min(p.len()))
            }
            x => (x == c, 1),
        }
    }

    fn match_at(s: &[u8], mut at: usize, p: &[u8]) -> Option<usize> {
        let mut i = 0;
        while i < p.len() {
            if p[i] == b'$' && i + 1 == p.len() {
                return (at == s.len()).then_some(at);
            }
            let c = *s.get(at)?;
            let (ok, len) = item(c, &p[i..]);
            if !ok {
                return None;
            }
            i += len;
            at += 1;
        }
        Some(at)
    }

    pub fn find(s: &[u8], p: &[u8], from: usize) -> Option<(usize, usize)> {
        let (anchored, p) = match p.first() {
            Some(b'^') => (true, &p[1..]),
            _ => (false, p),
        };
        let last = if anchored { from } else { s.len() };
        (from..=last).find_map(|start| match_at(s, start, p).map(|end| (start, end)))
    }

    pub fn gsub(s: &[u8], p: &[u8], rep: &[u8]) -> (Vec<u8>, usize) {
        let mut out = Vec::new();
        let mut at = 0;
        let mut n = 0;
        let anchored = p.first() == Some(&b'^');
        while at <= s.len() {
            match find(s, p, at).filter(|&(st, _)| st == at || !anchored) {
                Some((st, end)) => {
                    out.extend_from_slice(&s[at..st]);
                    let mut k = 0;
                    while k < rep.len() {
                        if rep[k] == b'%' && k + 1 < rep.len() {
                            if rep[k + 1] == b'0' {
                                out.extend_from_slice(&s[st..end]);
                            } else {
                                out.push(rep[k + 1]);
                            }
                            k += 2;
                        } else {
                            out.push(rep[k]);
                            k += 1;
                        }
                    }
                    n += 1;
                    if end == st {
                        if st < s.len() {
                            out.push(s[st]);
                        }
                        at = st + 1;
                    } else {
                        at = end;
                    }
                    if anchored {
                        out.extend_from_slice(&s[at.min(s.len())..]);
                        return (out, n);
                    }
                }
                None => {
                    out.extend_from_slice(&s[at..]);
                    break;
                }
            }
        }
        (out, n)
    }
}

#[cfg(test)]
mod tests {
    use super::super::compile;
    use super::*;

    fn run(src: &str) -> Vm {
        let chunk = compile(src.as_bytes(), b"test.lua", 4).expect("compiles");
        let mut vm = Vm::new();
        vm.run_chunk(&chunk).expect("runs");
        vm
    }

    fn printed(src: &str) -> Vec<String> {
        run(src).printed
    }

    #[test]
    fn arithmetic_strings_and_floats() {
        assert_eq!(
            printed("print(1 + 2 * 3, 10 / 4, 2 ^ 10, -(3), 7 - 10)"),
            ["7\t2.5\t1024\t-3\t-3"]
        );
        // 0.1 is a float in the game: tostring shows the double of the float.
        assert_eq!(printed("print(0.1)"), ["0.10000000149012"]);
        assert_eq!(printed("print('a' .. 1 .. 'b', '10' + 1)"), ["a1b\t11"]);
        assert_eq!(
            printed("print(1e20, 123456789)"),
            ["1.0000000200409e+20\t123456792"]
        );
    }

    #[test]
    fn control_flow_and_loops() {
        let src = r#"
            local s = 0
            for i = 1, 10 do if i ~= 5 then s = s + i end end
            local n = 0
            while n < 3 do n = n + 1 end
            repeat n = n - 1 until n == 0
            local t = { 10, 20, 30, x = 1 }
            local sum = 0
            for _, v in ipairs(t) do sum = sum + v end
            local keys = 0
            for k, v in pairs(t) do keys = keys + 1 end
            for i = 3, 1, -1 do s = s + i end
            print(s, n, sum, keys, table.getn(t), a and b or 'none', not nil)
        "#;
        assert_eq!(printed(src), ["56\t0\t60\t4\t3\tnone\ttrue"]);
    }

    #[test]
    fn closures_upvalues_varargs_and_pcall() {
        let src = r#"
            local function counter()
              local n = 0
              return function() n = n + 1; return n end
            end
            local c1, c2 = counter(), counter()
            c1(); c1()
            local function sum(...)
              local s = 0
              for i = 1, arg.n do s = s + arg[i] end
              return s, arg.n
            end
            local fns = {}
            for i = 1, 3 do local j = i; fns[i] = function() return j end end
            local ok, err = pcall(function() error("boom") end)
            local ok2, err2 = pcall(function() local x = nil; return x.y end)
            print(c1(), c2(), sum(1, 2, 3), fns[1]() + fns[3](), ok, err)
            print(ok2, err2)
        "#;
        let p = printed(src);
        assert_eq!(p[0], "3\t1\t6\t4\tfalse\ttest.lua:15: boom");
        assert_eq!(p[1], "false\ttest.lua:16: attempt to index a nil value");
    }

    #[test]
    fn methods_metatables_and_strings() {
        let src = r#"
            local Meta = { __index = function(t, k) return k .. "!" end }
            local Obj = { v = 5 }
            function Obj:get() return self.v end
            local o = setmetatable({}, Meta)
            print(Obj:get(), o.hi, string.format("%d|%5.2f|%s|%x|%-3s|%03d", 3.9, 1.005, "s", 255, "a", 7))
            print(string.sub("hello", 2, -2), string.len("abc"), string.upper("x"),
                  string.find("a.b", ".", 1), string.gsub("a b-c", "[^%w%.%-]", "_"))
        "#;
        let p = printed(src);
        assert_eq!(p[0], "5\thi!\t3| 1.00|s|ff|a  |007");
        assert_eq!(p[1], "ell\t3\tX\t1\ta_b-c\t1");
    }

    #[test]
    fn limits_stop_runaway_code() {
        let chunk = compile(b"while true do end", b"t.lua", 4).unwrap();
        let mut vm = Vm::new();
        vm.step_limit = 1000;
        let e = vm.run_chunk(&chunk).unwrap_err();
        assert!(e.fatal);
        let chunk = compile(b"local function f() return 1 + f() end f()", b"t.lua", 4).unwrap();
        let e = Vm::new().run_chunk(&chunk).unwrap_err();
        assert!(e.text().contains("stack overflow"), "{e}");
    }

    #[test]
    fn natives_are_called_with_their_arguments() {
        let chunk = compile(b"local a, b = Host(1, 'x') return a + b", b"t.lua", 4).unwrap();
        let mut vm = Vm::new();
        vm.register("Host", |_, args| {
            assert_eq!(args, vec![Value::Number(1.0), Value::str("x")]);
            Ok(vec![Value::Number(2.0), Value::Number(3.0)])
        });
        assert_eq!(vm.run_chunk(&chunk).unwrap(), vec![Value::Number(5.0)]);
    }

    #[test]
    fn formats_like_printf_g() {
        assert_eq!(format_g(100.0, 14), "100");
        assert_eq!(format_g(0.0001, 14), "0.0001");
        assert_eq!(format_g(0.00001, 14), "1e-05");
        assert_eq!(format_g(1.5, 14), "1.5");
        assert_eq!(format_g(-2.25, 6), "-2.25");
        assert_eq!(format_g(1e15, 14), "1e+15");
    }
}
