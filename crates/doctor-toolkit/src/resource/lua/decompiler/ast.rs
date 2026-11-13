//! Syntax tree of decompiled Lua 5.0 and its printer.
//!
//! Statements carry the source line of their first instruction; the printer
//! puts them on that line when it can, so the output keeps the line numbers of
//! the original script (and recompiles to the same line information).

use std::fmt::Write as _;

use super::super::lexer::is_reserved;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinOp {
    fn text(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Pow => "^",
            BinOp::Concat => "..",
            BinOp::Eq => "==",
            BinOp::Ne => "~=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "and",
            BinOp::Or => "or",
        }
    }

    /// Left and right priority, as in `lparser.c`.
    fn priority(self) -> (u8, u8) {
        match self {
            BinOp::Add | BinOp::Sub => (6, 6),
            BinOp::Mul | BinOp::Div => (7, 7),
            BinOp::Pow => (10, 9),
            BinOp::Concat => (5, 4),
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => (3, 3),
            BinOp::And => (2, 2),
            BinOp::Or => (1, 1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Minus,
    Not,
}

const UNARY_PRIORITY: u8 = 8;

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Nil,
    True,
    False,
    Number(f64),
    String(Vec<u8>),
    /// A local, upvalue or global variable.
    Name(Vec<u8>),
    Index(Box<Expr>, Box<Expr>),
    Call(Box<CallExpr>),
    Function(Box<FuncExpr>),
    Table(Box<TableExpr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Un(UnOp, Box<Expr>),
    /// Parentheses that matter: a call truncated to one value.
    Paren(Box<Expr>),
    /// An expression that starts on a given source line (layout only).
    At(u32, Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CallExpr {
    pub func: Expr,
    /// `obj:name(...)`: `func` is the object.
    pub method: Option<Vec<u8>>,
    pub args: Vec<Expr>,
    /// Returns all its values (`C = 0` or several results).
    pub multi: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FuncExpr {
    pub params: Vec<Vec<u8>>,
    pub vararg: bool,
    pub body: Block,
    /// Line of the closing `end`.
    pub end_line: u32,
    /// Line where the function starts (`line_defined`).
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TableItem {
    Positional(Expr),
    Named(Vec<u8>, Expr),
    Keyed(Expr, Expr),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TableExpr {
    /// Items with the line of the instruction that stored them.
    pub items: Vec<(TableItem, u32)>,
    /// Line of the closing brace (the instruction after the last item).
    pub end_line: u32,
    /// Line of `NEWTABLE`: the line of the token before the opening brace.
    pub open_line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stat {
    Local {
        names: Vec<Vec<u8>>,
        values: Vec<Expr>,
    },
    LocalFunction {
        name: Vec<u8>,
        func: FuncExpr,
    },
    Assign {
        targets: Vec<Expr>,
        values: Vec<Expr>,
    },
    /// `function a.b:c() ... end`
    Function {
        target: Expr,
        method: bool,
        func: FuncExpr,
    },
    Call(CallExpr),
    Do(Block),
    While {
        cond: Expr,
        body: Block,
    },
    Repeat {
        body: Block,
        cond: Expr,
    },
    If {
        cond: Expr,
        then: Block,
        otherwise: Option<Block>,
    },
    NumFor {
        var: Vec<u8>,
        start: Expr,
        limit: Expr,
        step: Option<Expr>,
        body: Block,
    },
    GenFor {
        names: Vec<Vec<u8>>,
        exprs: Vec<Expr>,
        body: Block,
    },
    Return(Vec<Expr>),
    Break,
}

/// A statement with its source line.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub line: u32,
    pub stat: Stat,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Block {
    pub stats: Vec<Line>,
    /// Line of the token that closes the block (`end`, `else`, `until`), 0 if unknown.
    pub end_line: u32,
}

/// Valid Lua 5.0 identifier that is not a reserved word.
pub fn is_identifier(name: &[u8]) -> bool {
    !name.is_empty()
        && (name[0].is_ascii_alphabetic() || name[0] == b'_')
        && name.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
        && !is_reserved(name)
}

/// Lua source text of a number that reads back to the same `f32`/`f64`.
pub fn number_text(n: f64, float: bool) -> String {
    if n.is_infinite() {
        return if n > 0.0 { "1e999" } else { "-1e999" }.to_owned();
    }
    if n == n.trunc() && n.abs() < 1e15 {
        if n == 0.0 && n.is_sign_negative() {
            return "-0".to_owned();
        }
        return format!("{}", n as i64);
    }
    let reads_back = |s: &str| {
        s.parse::<f64>().ok().is_some_and(|v| {
            if float {
                (v as f32).to_bits() == (n as f32).to_bits()
            } else {
                v.to_bits() == n.to_bits()
            }
        })
    };
    if float {
        let short = format!("{}", n as f32);
        if reads_back(&short) {
            return short;
        }
    }
    format!("{n}")
}

fn string_literal(s: &[u8]) -> String {
    let raw_ok = std::str::from_utf8(s).is_ok();
    let mut out = String::from("\"");
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        match c {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            7 => out.push_str("\\a"),
            8 => out.push_str("\\b"),
            12 => out.push_str("\\f"),
            11 => out.push_str("\\v"),
            0x20..=0x7E => out.push(c as char),
            0x80.. if raw_ok => {
                let len = match c {
                    0xF0.. => 4,
                    0xE0.. => 3,
                    _ => 2,
                };
                out.push_str(std::str::from_utf8(&s[i..i + len]).unwrap_or("?"));
                i += len;
                continue;
            }
            _ => {
                let _ = write!(out, "\\{c:03}");
            }
        }
        i += 1;
    }
    out.push('"');
    out
}

fn name_text(n: &[u8]) -> String {
    String::from_utf8_lossy(n).into_owned()
}

/// Prints statements as Lua source.
pub struct Printer {
    out: String,
    line: u32,
    indent: usize,
    float: bool,
    /// Right after a keyword that opens a block (no statement yet).
    fresh: bool,
}

impl Printer {
    pub fn new(float: bool) -> Self {
        Self {
            out: String::new(),
            line: 1,
            indent: 0,
            float,
            fresh: false,
        }
    }

    pub fn finish(mut self) -> String {
        if !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        self.out
    }

    /// Starts a new line, moving down to `target` when it is below the current line.
    fn newline_to(&mut self, target: u32) {
        if self.out.is_empty() && self.line >= target {
            self.push_indent();
            return;
        }
        // No trailing spaces (the output is between tokens here).
        while self.out.ends_with(' ') {
            self.out.pop();
        }
        loop {
            self.out.push('\n');
            self.line += 1;
            if self.line >= target {
                break;
            }
        }
        self.push_indent();
    }

    fn push_indent(&mut self) {
        for _ in 0..self.indent {
            self.out.push_str("  ");
        }
    }

    /// Moves to `target` if it is below; otherwise stays on the current line.
    fn move_to(&mut self, target: u32) -> bool {
        if target > self.line {
            self.newline_to(target);
            true
        } else {
            false
        }
    }

    /// Prints a block after its opening keyword; true if it stayed on that line.
    pub fn block(&mut self, block: &Block) -> bool {
        let start = self.line;
        self.indent += 1;
        self.fresh = true;
        for s in &block.stats {
            self.stat(s);
        }
        self.indent -= 1;
        self.line == start
    }

    /// Top-level statements of a chunk.
    pub fn chunk(&mut self, block: &Block) {
        for s in &block.stats {
            self.stat(s);
        }
    }

    fn stat_start(&mut self, line: u32) {
        if self.out.is_empty() && line <= 1 {
            self.push_indent();
        } else if line > 0 && line == self.line {
            // Several statements on one line, as in the original.
            self.out.push_str(if self.fresh { " " } else { "; " });
        } else if !self.move_to(line) {
            self.newline_to(self.line + 1);
        }
        self.fresh = false;
    }

    /// Closes a block: on the same line when the block stayed on one line
    /// (and the closing line is not known to be below).
    fn close_block(&mut self, keyword: &str, line: u32, inline: bool) {
        if inline && line <= self.line {
            self.out.push(' ');
            self.out.push_str(keyword);
        } else {
            self.close(keyword, line);
        }
    }

    fn close(&mut self, keyword: &str, line: u32) {
        if line > self.line {
            self.newline_to(line);
        } else {
            self.newline_to(self.line + 1);
        }
        self.out.push_str(keyword);
    }

    fn body_end(&mut self, end_line: u32) {
        self.indent -= 1;
        if end_line == self.line && end_line > 0 {
            self.out.push_str(" end");
        } else {
            self.close("end", end_line);
        }
        self.indent += 1;
    }

    pub fn stat(&mut self, s: &Line) {
        self.stat_start(s.line);
        match &s.stat {
            Stat::Local { names, values } => {
                self.out.push_str("local ");
                self.names(names);
                if !values.is_empty() {
                    self.out.push_str(" = ");
                    self.exprs(values);
                }
            }
            Stat::LocalFunction { name, func } => {
                let _ = write!(self.out, "local function {}", name_text(name));
                self.func_body(func);
            }
            Stat::Assign { targets, values } => {
                self.exprs(targets);
                self.out.push_str(" = ");
                self.exprs(values);
            }
            Stat::Function {
                target,
                method,
                func,
            } => {
                self.out.push_str("function ");
                match (method, target) {
                    (true, Expr::Index(obj, key)) => {
                        self.expr(obj, 0);
                        if let Expr::String(k) = &**key {
                            let _ = write!(self.out, ":{}", name_text(k));
                        }
                    }
                    _ => self.expr(target, 0),
                }
                let mut f = func.clone();
                if *method {
                    f.params.remove(0);
                }
                self.func_body(&f);
            }
            Stat::Call(call) => self.call(call),
            Stat::Do(block) => {
                self.out.push_str("do");
                let inline = self.block(block);
                self.close_block("end", block.end_line, inline);
            }
            Stat::While { cond, body } => {
                self.out.push_str("while ");
                self.expr(cond, 0);
                self.out.push_str(" do");
                let inline = self.block(body);
                self.close_block("end", body.end_line, inline);
            }
            Stat::Repeat { body, cond } => {
                self.out.push_str("repeat");
                let inline = self.block(body);
                self.close_block("until ", body.end_line, inline);
                self.expr(cond, 0);
            }
            Stat::If {
                cond,
                then,
                otherwise,
            } => {
                self.out.push_str("if ");
                self.expr(cond, 0);
                self.out.push_str(" then");
                let mut inline = self.block(then);
                let mut otherwise = otherwise.as_ref();
                while let Some(block) = otherwise {
                    if let [
                        Line {
                            stat:
                                Stat::If {
                                    cond,
                                    then,
                                    otherwise: next,
                                },
                            line,
                        },
                    ] = block.stats.as_slice()
                    {
                        self.close("elseif ", *line);
                        self.expr(cond, 0);
                        self.out.push_str(" then");
                        inline = self.block(then);
                        otherwise = next.as_ref();
                    } else {
                        let first = block.stats.first().map_or(0, |s| s.line);
                        if first == self.line + 1 || (inline && first == self.line) {
                            // `end else` keeps the first statement on its line.
                            self.out.push_str(" else");
                        } else {
                            self.close("else", 0);
                        }
                        inline = self.block(block);
                        otherwise = None;
                    }
                }
                self.close_block("end", 0, inline);
            }
            Stat::NumFor {
                var,
                start,
                limit,
                step,
                body,
            } => {
                let _ = write!(self.out, "for {} = ", name_text(var));
                self.expr(start, 0);
                self.out.push_str(", ");
                self.expr(limit, 0);
                if let Some(step) = step {
                    self.out.push_str(", ");
                    self.expr(step, 0);
                }
                self.out.push_str(" do");
                let inline = self.block(body);
                self.close_block("end", body.end_line, inline);
            }
            Stat::GenFor { names, exprs, body } => {
                self.out.push_str("for ");
                self.names(names);
                self.out.push_str(" in ");
                self.exprs(exprs);
                self.out.push_str(" do");
                let inline = self.block(body);
                self.close_block("end", body.end_line, inline);
            }
            Stat::Return(values) => {
                self.out.push_str("return");
                if !values.is_empty() {
                    self.out.push(' ');
                    self.exprs(values);
                }
            }
            Stat::Break => self.out.push_str("break"),
        }
    }

    fn names(&mut self, names: &[Vec<u8>]) {
        for (i, n) in names.iter().enumerate() {
            if i > 0 {
                self.out.push_str(", ");
            }
            self.out.push_str(&name_text(n));
        }
    }

    fn exprs(&mut self, exprs: &[Expr]) {
        for (i, e) in exprs.iter().enumerate() {
            if i > 0 {
                self.out.push_str(", ");
            }
            self.expr(e, 0);
        }
    }

    fn func_body(&mut self, f: &FuncExpr) {
        self.out.push('(');
        self.names(&f.params);
        if f.vararg {
            if !f.params.is_empty() {
                self.out.push_str(", ");
            }
            self.out.push_str("...");
        }
        self.out.push(')');
        self.indent += 1;
        self.fresh = true;
        for s in &f.body.stats {
            self.stat(s);
        }
        self.body_end(f.end_line);
        self.indent -= 1;
    }

    fn call(&mut self, c: &CallExpr) {
        self.prefix(&c.func);
        if let Some(m) = &c.method {
            let _ = write!(self.out, ":{}", name_text(m));
        }
        self.out.push('(');
        self.exprs(&c.args);
        self.out.push(')');
    }

    /// An expression used as the object of an index or call.
    fn prefix(&mut self, e: &Expr) {
        match e {
            Expr::At(_, inner) => self.prefix(inner),
            Expr::Name(_) | Expr::Index(..) | Expr::Call(_) | Expr::Paren(_) => self.expr(e, 0),
            _ => {
                self.out.push('(');
                self.expr(e, 0);
                self.out.push(')');
            }
        }
    }

    /// Prints `e` where the parser reads it with `subexpr(limit)`.
    fn expr(&mut self, e: &Expr, limit: u8) {
        self.expr_in(e, limit, 0);
    }

    /// Prints `e` read by `subexpr(limit)` and followed by an operator whose
    /// left priority is `left_of` (0 if none): a binary operator needs
    /// parentheses unless its left priority is above `limit` and its right
    /// priority is at least `left_of`.
    fn expr_in(&mut self, e: &Expr, limit: u8, left_of: u8) {
        match e {
            Expr::Nil => self.out.push_str("nil"),
            Expr::True => self.out.push_str("true"),
            Expr::False => self.out.push_str("false"),
            Expr::Number(n) => {
                let text = number_text(*n, self.float);
                let negative = text.starts_with('-');
                if negative && left_of > UNARY_PRIORITY {
                    let _ = write!(self.out, "({text})");
                } else {
                    // `- -1` must not become a comment.
                    if negative && self.out.ends_with('-') {
                        self.out.push(' ');
                    }
                    self.out.push_str(&text);
                }
            }
            Expr::String(s) => self.out.push_str(&string_literal(s)),
            Expr::Name(n) => self.out.push_str(&name_text(n)),
            Expr::Index(obj, key) => {
                self.prefix(obj);
                match &**key {
                    Expr::String(k) if is_identifier(k) => {
                        let _ = write!(self.out, ".{}", name_text(k));
                    }
                    _ => {
                        self.out.push('[');
                        self.expr(key, 0);
                        self.out.push(']');
                    }
                }
            }
            Expr::Call(c) => self.call(c),
            Expr::Function(f) => {
                self.indent += 1;
                self.move_to(f.line);
                self.indent -= 1;
                self.out.push_str("function");
                self.func_body(f);
            }
            Expr::Table(t) => self.table(t),
            Expr::Paren(inner) => {
                self.out.push('(');
                self.expr(inner, 0);
                self.out.push(')');
            }
            Expr::At(line, inner) => {
                self.indent += 1;
                self.move_to(*line);
                self.indent -= 1;
                self.expr_in(inner, limit, left_of);
            }
            Expr::Un(op, operand) => {
                let parens = left_of > UNARY_PRIORITY;
                if parens {
                    self.out.push('(');
                }
                match op {
                    UnOp::Minus => {
                        if self.out.ends_with('-') {
                            self.out.push(' ');
                        }
                        self.out.push('-');
                    }
                    UnOp::Not => self.out.push_str("not "),
                }
                self.expr_in(operand, UNARY_PRIORITY, 0);
                if parens {
                    self.out.push(')');
                }
            }
            Expr::Bin(op, l, r) => {
                let (left, right) = op.priority();
                let parens = left <= limit || right < left_of;
                let (limit, left_of) = if parens { (0, 0) } else { (limit, left_of) };
                if parens {
                    self.out.push('(');
                }
                self.expr_in(l, limit, left);
                let _ = write!(self.out, " {} ", op.text());
                self.expr_in(r, right, left_of);
                if parens {
                    self.out.push(')');
                }
            }
        }
    }

    fn table(&mut self, t: &TableExpr) {
        if t.items.is_empty() {
            self.out.push_str("{}");
            return;
        }
        let multiline = t.items.iter().any(|(_, line)| *line > self.line) || t.end_line > self.line;
        // `NEWTABLE` of a nested table takes the line of the token before its
        // `{`: this brace or the comma before the item.
        let nested_open = |item: &TableItem| match item {
            TableItem::Positional(Expr::Table(u)) => u.open_line,
            _ => 0,
        };
        if multiline && let Some((first, _)) = t.items.first() {
            self.indent += 1;
            self.move_to(nested_open(first));
            self.indent -= 1;
        }
        self.out.push('{');
        self.indent += 1;
        for (i, (item, line)) in t.items.iter().enumerate() {
            if i > 0 {
                if multiline {
                    self.move_to(nested_open(item));
                }
                self.out.push(',');
            }
            if !(multiline && self.move_to(*line)) {
                self.out.push(' ');
            }
            match item {
                TableItem::Positional(e) => self.expr(e, 0),
                TableItem::Named(k, v) => {
                    let _ = write!(self.out, "{} = ", name_text(k));
                    self.expr(v, 0);
                }
                TableItem::Keyed(k, v) => {
                    self.out.push('[');
                    self.expr(k, 0);
                    self.out.push_str("] = ");
                    self.expr(v, 0);
                }
            }
        }
        self.indent -= 1;
        if !(multiline && self.move_to(t.end_line)) {
            self.out.push(' ');
        }
        self.out.push('}');
    }
}
