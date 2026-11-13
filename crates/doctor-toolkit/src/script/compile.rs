//! Compiler: script graph → Lua 5.0 source → bytecode for the game.
//!
//! The source is meant to be read: a short runtime prelude, one function per
//! event, every node a commented statement (`-- [12] Send message`). Pure
//! nodes (math, comparisons, queries) become expressions where their
//! outputs are used; nodes with exec pins become statements, and their
//! outputs are kept in the table `S` (`S.n12_handle`) so later nodes and
//! later events can read them. Graph variables live in `V`.
//!
//! An exec input reached from more than one place, and the continuation of
//! a Delay, becomes a function in `K` that the places call. A Delay is a
//! one-shot game timer (`CreateTimer` calls a global function by name, so
//! every timer has a global callback `dts_<script>_<key>`).
//!
//! Every problem is reported against a node ([`Diagnostic`]); errors of the
//! Lua compiler (in `lua.code` nodes) are mapped back through the line table.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;

use serde::Serialize;
use serde_json::Value as Json;

use super::graph::{Link, Node, PinRef, PinType, ScriptGraph, is_identifier};
use super::library::{self, NodeDef, Pin};
use crate::resource::lua;

/// Severity of a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Warning,
    Error,
}

/// A problem found while compiling, attached to a node (and pin) when possible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub node: Option<u32>,
    pub pin: Option<String>,
    pub message: String,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sev = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        match (&self.node, &self.pin) {
            (Some(n), Some(p)) => write!(f, "{sev}: node {n}, pin {p}: {}", self.message),
            (Some(n), None) => write!(f, "{sev}: node {n}: {}", self.message),
            _ => write!(f, "{sev}: {}", self.message),
        }
    }
}

/// Result of [`compile`].
#[derive(Debug, Clone, Serialize)]
pub struct Compiled {
    /// Lua source (also when there are errors, as far as it could be made).
    pub source: String,
    /// The chunk for the game; `None` when there are errors.
    #[serde(skip)]
    pub chunk: Option<Vec<u8>>,
    pub diagnostics: Vec<Diagnostic>,
    /// For every line of `source` (index = line - 1), the node it comes from.
    #[serde(skip)]
    pub line_nodes: Vec<Option<u32>>,
}

impl Compiled {
    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
    }

    pub fn has_errors(&self) -> bool {
        self.errors().next().is_some()
    }

    /// Diagnostics of one node.
    pub fn for_node(&self, id: u32) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter().filter(move |d| d.node == Some(id))
    }

    /// The node that line `line` (1-based) of the source comes from.
    pub fn node_at_line(&self, line: u32) -> Option<u32> {
        self.line_nodes
            .get(line.checked_sub(1)? as usize)
            .copied()
            .flatten()
    }
}

/// Prefix of the global functions a script defines (timer callbacks).
pub fn global_prefix(script: &str) -> String {
    format!("dts_{script}_")
}

/// The runtime every script starts with. `@NAME@` is the script name,
/// `@TRACE@` is `true` or `false`.
const PRELUDE: &str = r#"local SCRIPT = "@NAME@.lua"
local G = "dts_@NAME@_"
local V, S, K, F, E = {}, {}, {}, {}, {}
local dt = { handlers = {}, timers = {}, receiver = nil, trace = @TRACE@ }

-- Runs one handler; an error stops only this handler.
local function dt_run(where, f, a, b)
  local ok, err = pcall(f, a, b)
  if not ok and dt.trace then
    loadfile("doctor_error_@NAME@_" .. where .. "_" .. string.gsub(tostring(err), "[^%w%.%-]", "_"))
  end
end

local function dt_on(message, f)
  local list = dt.handlers[message]
  if list == nil then
    list = {}
    dt.handlers[message] = list
  end
  table.insert(list, f)
end

local function dt_receive(self, message, dtype, data)
  local list = dt.handlers[message]
  if list then
    for i = 1, table.getn(list) do
      dt_run(tostring(message), list[i], data, dtype)
    end
  end
end

-- Starts (or restarts) the game timer `key`; it calls the global G .. key.
local function dt_timer(key, seconds, once)
  local t = dt.timers[key]
  if t == nil then
    t = CreateTimer(G .. key, once)
    dt.timers[key] = t
  end
  StartTimer(t, seconds * 1000)
end

local function dt_stop_timer(key)
  local t = dt.timers[key]
  if t then
    StopTimer(t)
  end
end

-- PlayEmx finds a sound by its file name, `.emx` included (stage 45: the
-- name alone finds nothing).
local function dt_emx(name)
  name = tostring(name)
  if string.find(string.lower(name), "%.emx$") then
    return name
  end
  return name .. ".emx"
end

local function dt_get(name)
  return VariableGet(FindVariable(name))
end

-- Game variables made here hold whole numbers: true and false become 1 and 0.
local function dt_set(name, value)
  if value == true then
    value = 1
  elseif value == false then
    value = 0
  end
  VariableSet(FindVariable(name), value)
end

local function dt_axis(p, i)
  local v = { GetPositionValues(p) }
  return v[i]
end

local function dt_dist(p, x, y, z)
  local px, py, pz = GetPositionValues(p)
  return math.sqrt((px - x) * (px - x) + (py - y) * (py - y) + (pz - z) * (pz - z))
end

local function dt_self(o)
  return o
end

local function dt_dist2(a, b)
  return dt_dist(a, GetPositionValues(b))
end
"#;

/// Compiles a graph. Never fails: problems are diagnostics.
pub fn compile(graph: &ScriptGraph) -> Compiled {
    let mut c = Compiler::new(graph);
    c.check();
    // Generated also with errors, for the preview, as far as it goes.
    c.generate();
    let source = c.out.text();
    let mut diagnostics = c.diags;
    let line_nodes = c.out.owners.clone();
    let mut chunk = None;
    if !diagnostics.iter().any(|d| d.severity == Severity::Error) {
        match lua::compile_for_game(source.as_bytes(), graph.chunk_name().as_bytes()) {
            Ok(bytes) => chunk = Some(bytes),
            Err(e) => {
                let node = line_nodes
                    .get(e.line.saturating_sub(1) as usize)
                    .copied()
                    .flatten();
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    node,
                    pin: None,
                    message: format!("Lua: {e}"),
                });
            }
        }
    }
    diagnostics.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.node.cmp(&b.node)));
    diagnostics.dedup();
    Compiled {
        source,
        chunk,
        diagnostics,
        line_nodes,
    }
}

/// Lines of Lua with the node each comes from.
#[derive(Default)]
struct Out {
    lines: Vec<String>,
    owners: Vec<Option<u32>>,
}

impl Out {
    fn push(&mut self, indent: usize, owner: Option<u32>, text: &str) {
        for line in text.split('\n') {
            self.lines.push(if line.is_empty() {
                String::new()
            } else {
                format!("{}{line}", "  ".repeat(indent))
            });
            self.owners.push(owner);
        }
    }

    fn blank(&mut self) {
        if self.lines.last().is_some_and(|l| !l.is_empty()) {
            self.lines.push(String::new());
            self.owners.push(None);
        }
    }

    fn text(&self) -> String {
        let mut s = self.lines.join("\n");
        s.push('\n');
        s
    }
}

struct NodeInfo<'a> {
    node: &'a Node,
    def: &'static NodeDef,
    inputs: Vec<Pin>,
    outputs: Vec<Pin>,
}

impl NodeInfo<'_> {
    fn input(&self, name: &str) -> Option<&Pin> {
        self.inputs.iter().find(|p| p.name == name)
    }

    fn output(&self, name: &str) -> Option<&Pin> {
        self.outputs.iter().find(|p| p.name == name)
    }
}

struct Compiler<'a> {
    graph: &'a ScriptGraph,
    nodes: BTreeMap<u32, NodeInfo<'a>>,
    /// Valid links by target input.
    data_in: HashMap<PinRef, PinRef>,
    /// Valid exec links by source output.
    exec_out: HashMap<PinRef, PinRef>,
    /// Number of valid exec links into each input.
    exec_in_count: HashMap<PinRef, usize>,
    diags: Vec<Diagnostic>,
    out: Out,
    timers: BTreeMap<String, u32>,
    functions: BTreeMap<String, u32>,
    /// Pure nodes being expanded (cycle guard).
    expanding: HashSet<u32>,
}

fn err(node: Option<u32>, pin: Option<&str>, message: impl Into<String>) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        node,
        pin: pin.map(str::to_owned),
        message: message.into(),
    }
}

fn warn(node: Option<u32>, pin: Option<&str>, message: impl Into<String>) -> Diagnostic {
    Diagnostic {
        severity: Severity::Warning,
        ..err(node, pin, message)
    }
}

/// A Lua string literal.
pub fn lua_string(s: &str) -> String {
    let mut out = String::from("\"");
    for b in s.bytes() {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7E => out.push(b as char),
            _ => {
                let _ = write!(out, "\\{b}");
            }
        }
    }
    out.push('"');
    out
}

/// A Lua number literal.
pub fn lua_number(n: f64) -> String {
    if !n.is_finite() {
        return "0".into();
    }
    let s = lua::opcodes::number_text(n);
    if n < 0.0 { format!("({s})") } else { s }
}

/// A JSON value as a Lua literal of type `ty`.
fn literal(value: &Json, ty: PinType) -> Option<String> {
    Some(match (value, ty) {
        (Json::Null, _) => "nil".into(),
        (Json::Bool(b), PinType::Bool | PinType::Any) => b.to_string(),
        (Json::Number(n), PinType::Number | PinType::Any) => lua_number(n.as_f64()?),
        (Json::Number(n), PinType::String) => lua_string(&lua::opcodes::number_text(n.as_f64()?)),
        (Json::String(s), PinType::String | PinType::Any) => lua_string(s),
        (Json::String(s), PinType::Number) => lua_number(s.trim().parse().ok()?),
        (Json::Bool(b), PinType::String) => lua_string(&b.to_string()),
        _ => return None,
    })
}

fn type_default(ty: PinType) -> &'static str {
    match ty {
        PinType::Number => "0",
        PinType::Bool => "false",
        PinType::String => "\"\"",
        _ => "nil",
    }
}

/// Name of a field of `S` for an output.
fn slot(id: u32, pin: &str) -> String {
    format!("S.n{id}_{pin}")
}

impl<'a> Compiler<'a> {
    fn new(graph: &'a ScriptGraph) -> Self {
        Self {
            graph,
            nodes: BTreeMap::new(),
            data_in: HashMap::new(),
            exec_out: HashMap::new(),
            exec_in_count: HashMap::new(),
            diags: Vec::new(),
            out: Out::default(),
            timers: BTreeMap::new(),
            functions: BTreeMap::new(),
            expanding: HashSet::new(),
        }
    }

    fn param(&self, id: u32, name: &str) -> Json {
        let n = &self.nodes[&id];
        library::param(n.node, n.def, name)
    }

    fn param_str(&self, id: u32, name: &str) -> String {
        self.param(id, name).as_str().unwrap_or_default().to_owned()
    }

    // ------------------------------------------------------------- checks

    fn check(&mut self) {
        let g = self.graph;
        if !is_identifier(&g.name) {
            self.diags.push(err(
                None,
                None,
                format!(
                    "script name {:?} must be letters, digits and _ (not starting with a digit)",
                    g.name
                ),
            ));
        }
        let mut seen = HashSet::new();
        for v in &g.variables {
            if !is_identifier(&v.name) {
                self.diags
                    .push(err(None, None, format!("bad variable name {:?}", v.name)));
            } else if !seen.insert(v.name.as_str()) {
                self.diags.push(err(
                    None,
                    None,
                    format!("variable {} is declared twice", v.name),
                ));
            }
            if v.ty == PinType::Exec {
                self.diags.push(err(
                    None,
                    None,
                    format!("variable {} cannot be exec", v.name),
                ));
            } else if literal(&v.default, v.ty).is_none() {
                self.diags.push(err(
                    None,
                    None,
                    format!(
                        "variable {}: default {} is not a {}",
                        v.name,
                        v.default,
                        v.ty.name()
                    ),
                ));
            }
        }
        let mut ids = HashSet::new();
        for node in &g.nodes {
            if !ids.insert(node.id) {
                self.diags
                    .push(err(Some(node.id), None, "two nodes have this id"));
                continue;
            }
            let Some(def) = library::get(&node.kind) else {
                self.diags.push(err(
                    Some(node.id),
                    None,
                    format!("unknown node kind {:?}", node.kind),
                ));
                continue;
            };
            let (inputs, outputs) = library::pins(g, node).unwrap_or_default();
            self.nodes.insert(
                node.id,
                NodeInfo {
                    node,
                    def,
                    inputs,
                    outputs,
                },
            );
        }
        self.check_names();
        self.check_links();
        self.check_nodes();
        self.check_exec_cycles();
        self.check_reachability();
    }

    fn check_names(&mut self) {
        let ids: Vec<u32> = self.nodes.keys().copied().collect();
        for id in ids {
            let kind = self.nodes[&id].def.kind;
            let (table, what) = match kind {
                "event.timer" => (&mut self.timers, "timer"),
                "event.function" => (&mut self.functions, "function"),
                _ => continue,
            };
            let n = &self.nodes[&id];
            let name = library::param(n.node, n.def, "name")
                .as_str()
                .unwrap_or_default()
                .to_owned();
            if !is_identifier(&name) {
                self.diags
                    .push(err(Some(id), None, format!("bad {what} name {name:?}")));
            } else if table.insert(name.clone(), id).is_some() {
                self.diags
                    .push(err(Some(id), None, format!("two {what}s are named {name}")));
            }
        }
    }

    fn check_links(&mut self) {
        let mut data_links: HashMap<PinRef, Vec<&Link>> = HashMap::new();
        let mut exec_links: HashMap<PinRef, Vec<&Link>> = HashMap::new();
        for link in &self.graph.links {
            let (Some(from), Some(to)) = (self.nodes.get(&link.from.0), self.nodes.get(&link.to.0))
            else {
                let missing = if self.nodes.contains_key(&link.from.0) {
                    link.to.0
                } else {
                    link.from.0
                };
                self.diags.push(err(
                    self.nodes.contains_key(&link.from.0).then_some(link.from.0),
                    None,
                    format!("a link goes to node {missing}, which does not exist"),
                ));
                continue;
            };
            let Some(out) = from.output(&link.from.1) else {
                self.diags.push(err(
                    Some(link.from.0),
                    Some(&link.from.1),
                    format!("{} has no output {:?}", from.def.title, link.from.1),
                ));
                continue;
            };
            let Some(inp) = to.input(&link.to.1) else {
                self.diags.push(err(
                    Some(link.to.0),
                    Some(&link.to.1),
                    format!("{} has no input {:?}", to.def.title, link.to.1),
                ));
                continue;
            };
            if !out.ty.connects_to(inp.ty) {
                self.diags.push(err(
                    Some(link.to.0),
                    Some(&link.to.1),
                    format!(
                        "a {} output cannot feed a {} input",
                        out.ty.name(),
                        inp.ty.name()
                    ),
                ));
                continue;
            }
            if out.ty == PinType::Exec {
                exec_links.entry(link.from.clone()).or_default().push(link);
            } else {
                data_links.entry(link.to.clone()).or_default().push(link);
            }
        }
        for (from, links) in exec_links {
            if links.len() > 1 {
                self.diags.push(err(
                    Some(from.0),
                    Some(&from.1),
                    "an exec output can go to one place only (use a Sequence)",
                ));
            }
            let to = links[0].to.clone();
            *self.exec_in_count.entry(to.clone()).or_default() += 1;
            self.exec_out.insert(from, to);
        }
        for (to, links) in data_links {
            if links.len() > 1 {
                self.diags.push(err(
                    Some(to.0),
                    Some(&to.1),
                    "an input can have one link only",
                ));
            }
            self.data_in.insert(to, links[0].from.clone());
        }
    }

    fn check_nodes(&mut self) {
        let ids: Vec<u32> = self.nodes.keys().copied().collect();
        for id in ids {
            let n = &self.nodes[&id];
            let kind = n.def.kind;
            match kind {
                "event.message" => {
                    let m = self.param_str(id, "message");
                    if m.trim().is_empty() {
                        self.diags
                            .push(err(Some(id), None, "the message name is empty"));
                    }
                }
                "event.timer" => {
                    if self.param(id, "seconds").as_f64().unwrap_or(0.0) <= 0.0 {
                        self.diags.push(err(
                            Some(id),
                            None,
                            "the timer needs a positive number of seconds",
                        ));
                    }
                }
                "timer.start" | "timer.stop" => {
                    let t = self.param_str(id, "timer");
                    if !self.timers.contains_key(&t) {
                        self.diags.push(err(
                            Some(id),
                            None,
                            format!("there is no On timer named {t:?}"),
                        ));
                    }
                }
                "flow.call_function" => {
                    let f = self.param_str(id, "function");
                    if !self.functions.contains_key(&f) {
                        self.diags.push(err(
                            Some(id),
                            None,
                            format!("there is no Function named {f:?}"),
                        ));
                    }
                }
                "var.get" | "var.set" => {
                    let v = self.param_str(id, "variable");
                    if self.graph.variable(&v).is_none() {
                        self.diags.push(err(
                            Some(id),
                            None,
                            format!("there is no variable named {v:?}"),
                        ));
                    }
                }
                "lua.call" => {
                    let f = self.param_str(id, "function");
                    let ok = !f.is_empty() && f.split('.').all(is_identifier);
                    if !ok {
                        self.diags.push(err(
                            Some(id),
                            None,
                            format!("{f:?} is not a function name (name or name.name)"),
                        ));
                    }
                }
                "lua.code" | "event.function" => {
                    let lists: &[&str] = if kind == "lua.code" {
                        &["inputs", "outputs"]
                    } else {
                        &["inputs"]
                    };
                    let mut names = HashSet::new();
                    for list in lists {
                        for pin in library::pin_list(&self.param(id, list)) {
                            if !is_identifier(&pin.name)
                                || ["exec", "then"].contains(&pin.name.as_str())
                            {
                                self.diags.push(err(
                                    Some(id),
                                    None,
                                    format!("bad pin name {:?}", pin.name),
                                ));
                            } else if !names.insert(pin.name.clone()) {
                                self.diags.push(err(
                                    Some(id),
                                    None,
                                    format!("two pins are named {}", pin.name),
                                ));
                            }
                        }
                    }
                }
                _ => {}
            }
            let n = &self.nodes[&id];
            for (pin, value) in &n.node.inputs {
                match n.input(pin) {
                    None => self.diags.push(warn(
                        Some(id),
                        Some(pin),
                        "value for an input the node does not have",
                    )),
                    Some(p) if literal(value, p.ty).is_none() => self.diags.push(err(
                        Some(id),
                        Some(pin),
                        format!("{value} is not a {} value", p.ty.name()),
                    )),
                    _ => {}
                }
            }
            for p in &n.inputs {
                let key = PinRef::new(id, &p.name);
                if matches!(p.ty, PinType::Position | PinType::Group)
                    && !self.data_in.contains_key(&key)
                    && !n.node.inputs.contains_key(&p.name)
                {
                    self.diags.push(err(
                        Some(id),
                        Some(&p.name),
                        format!("the {} input needs a link", p.ty.name()),
                    ));
                }
            }
        }
    }

    /// Exec successors of a node for cycle detection: through links, and
    /// from a Call function to the Function. A Delay's continuation runs
    /// later, so it does not count.
    fn exec_successors(&self, id: u32) -> Vec<u32> {
        let n = &self.nodes[&id];
        let mut next = Vec::new();
        for p in &n.outputs {
            if p.ty != PinType::Exec || (n.def.kind == "flow.delay" && p.name == "then") {
                continue;
            }
            if let Some(to) = self.exec_out.get(&PinRef::new(id, &p.name)) {
                next.push(to.0);
            }
        }
        if n.def.kind == "flow.call_function"
            && let Some(&f) = self.functions.get(&self.param_str(id, "function"))
        {
            next.push(f);
        }
        next
    }

    fn check_exec_cycles(&mut self) {
        // Iterative DFS with colours.
        let mut colour: HashMap<u32, u8> = HashMap::new();
        let mut reported = HashSet::new();
        for &start in self.nodes.keys() {
            if colour.contains_key(&start) {
                continue;
            }
            let mut stack = vec![(start, 0usize)];
            colour.insert(start, 1);
            while let Some(&mut (id, ref mut i)) = stack.last_mut() {
                let succ = self.exec_successors(id);
                if *i < succ.len() {
                    let s = succ[*i];
                    *i += 1;
                    match colour.get(&s) {
                        Some(1) => {
                            if reported.insert(s) {
                                self.diags.push(err(
                                    Some(s),
                                    None,
                                    "this node runs itself again without waiting: put a Delay in the loop",
                                ));
                            }
                        }
                        Some(_) => {}
                        None => {
                            colour.insert(s, 1);
                            stack.push((s, 0));
                        }
                    }
                } else {
                    colour.insert(id, 2);
                    stack.pop();
                }
            }
        }
    }

    fn check_reachability(&mut self) {
        let mut reached = HashSet::new();
        let mut todo: Vec<u32> = self
            .nodes
            .values()
            .filter(|n| n.def.is_event())
            .map(|n| n.node.id)
            .collect();
        while let Some(id) = todo.pop() {
            if !reached.insert(id) {
                continue;
            }
            let n = &self.nodes[&id];
            for p in &n.outputs {
                if p.ty == PinType::Exec
                    && let Some(to) = self.exec_out.get(&PinRef::new(id, &p.name))
                {
                    todo.push(to.0);
                }
            }
        }
        for n in self.nodes.values() {
            let has_exec = n.inputs.iter().any(|p| p.ty == PinType::Exec);
            if has_exec && !reached.contains(&n.node.id) {
                self.diags.push(warn(
                    Some(n.node.id),
                    None,
                    "never runs: no event leads here",
                ));
            }
        }
    }

    // --------------------------------------------------------- expressions

    /// Lua expression for input `pin` of node `id`.
    fn input(&mut self, id: u32, pin: &str) -> String {
        let n = &self.nodes[&id];
        let Some(p) = n.input(pin) else {
            return "nil".into();
        };
        let ty = p.ty;
        if let Some(from) = self.data_in.get(&PinRef::new(id, pin)).cloned() {
            let from_ty = self.nodes[&from.0]
                .output(&from.1)
                .map_or(PinType::Any, |p| p.ty);
            let e = self.output(from.0, &from.1);
            return match (from_ty, ty) {
                (PinType::Number | PinType::Bool | PinType::Any, PinType::String) => {
                    format!("tostring({e})")
                }
                _ => e,
            };
        }
        match n.node.inputs.get(pin) {
            Some(v) => literal(v, ty).unwrap_or_else(|| "nil".into()),
            None => library::input_default(n.def.kind, pin)
                .and_then(|v| literal(&v, ty))
                .unwrap_or_else(|| type_default(ty).into()),
        }
    }

    /// Lua expression for output `pin` of node `id`.
    fn output(&mut self, id: u32, pin: &str) -> String {
        let n = &self.nodes[&id];
        if !n.def.is_pure() {
            return match n.def.kind {
                "var.set" => format!("V.{}", self.param_str(id, "variable")),
                _ => slot(id, pin),
            };
        }
        if !self.expanding.insert(id) {
            self.diags
                .push(err(Some(id), None, "data links go round in a circle"));
            return "nil".into();
        }
        let e = self.pure(id, pin);
        self.expanding.remove(&id);
        e
    }

    fn pure(&mut self, id: u32, pin: &str) -> String {
        let kind = self.nodes[&id].def.kind;
        let mut i = |name: &str| self.input(id, name);
        let bin = |op: &str, a: String, b: String| format!("({a} {op} {b})");
        match kind {
            "value.number" => {
                literal(&self.param(id, "value"), PinType::Number).unwrap_or("0".into())
            }
            "value.bool" => {
                literal(&self.param(id, "value"), PinType::Bool).unwrap_or("false".into())
            }
            "value.string" => {
                literal(&self.param(id, "value"), PinType::String).unwrap_or("\"\"".into())
            }
            "math.add" => bin("+", i("a"), i("b")),
            "math.sub" => bin("-", i("a"), i("b")),
            "math.mul" => bin("*", i("a"), i("b")),
            "math.div" => bin("/", i("a"), i("b")),
            "math.mod" => format!("math.mod({}, {})", i("a"), i("b")),
            "math.min" => format!("math.min({}, {})", i("a"), i("b")),
            "math.max" => format!("math.max({}, {})", i("a"), i("b")),
            "math.abs" => format!("math.abs({})", i("x")),
            "math.floor" => format!("math.floor({})", i("x")),
            "math.negate" => format!("(-{})", i("x")),
            "math.clamp" => {
                let (x, lo, hi) = (i("x"), i("min"), i("max"));
                format!("math.max({lo}, math.min({hi}, {x}))")
            }
            "math.random" => format!("math.random({}, {})", i("min"), i("max")),
            "compare.eq" => bin("==", i("a"), i("b")),
            "compare.ne" => bin("~=", i("a"), i("b")),
            "compare.lt" => bin("<", i("a"), i("b")),
            "compare.le" => bin("<=", i("a"), i("b")),
            "compare.gt" => bin(">", i("a"), i("b")),
            "compare.ge" => bin(">=", i("a"), i("b")),
            "logic.and" => bin("and", i("a"), i("b")),
            "logic.or" => bin("or", i("a"), i("b")),
            "logic.not" => format!("(not {})", i("x")),
            "string.concat" => bin("..", i("a"), i("b")),
            "string.from" => format!("tostring({})", i("value")),
            "string.number" => {
                let (v, d) = (i("value"), i("decimals"));
                match d.parse::<f64>() {
                    Ok(n) => format!(
                        "string.format({}, {v})",
                        lua_string(&format!("%.{}f", n.max(0.0) as u32))
                    ),
                    Err(_) => format!("string.format(\"%.\" .. math.floor({d}) .. \"f\", {v})"),
                }
            }
            "string.length" => format!("string.len({})", i("text")),
            "var.get" => format!("V.{}", self.param_str(id, "variable")),
            "engine.get" => format!("dt_get({})", i("name")),
            "game.sim_time" => "(GetSimTime() / 1000)".into(),
            "game.player_position" => "GetPlayerPosition()".into(),
            "game.position_xyz" => {
                let axis = match pin {
                    "x" => 1,
                    "y" => 2,
                    _ => 3,
                };
                format!("dt_axis({}, {axis})", i("position"))
            }
            "game.distance" => format!("dt_dist2({}, {})", i("a"), i("b")),
            "game.distance_to" => {
                let (p, x, y, z) = (i("position"), i("x"), i("y"), i("z"));
                format!("dt_dist({p}, {x}, {y}, {z})")
            }
            "game.foot_police" => "GetFootPoliceIntensity()".into(),
            "game.car_police" => "GetCarPoliceIntensity()".into(),
            "game.vehicle_speed" => "GetVehicleSpeed()".into(),
            "game.vehicle_type" => "GetVehicleType()".into(),
            "game.platform" => "GetPlatform()".into(),
            "group.find" => format!("GroupManager:FindGroup({})", i("name")),
            "group.count" => format!("({}):GetMemberCount()", i("group")),
            "group.contains" => {
                let (g, m) = (i("group"), i("member"));
                format!("({g}):IsMember({m})")
            }
            _ => "nil".into(),
        }
    }

    // ---------------------------------------------------------- statements

    /// Emits what happens when exec input `pin` of node `id` is entered.
    fn enter(&mut self, indent: usize, id: u32, pin: &str, inside: Option<&PinRef>) {
        let key = PinRef::new(id, pin);
        let shared = self.exec_in_count.get(&key).copied().unwrap_or(0) > 1;
        if shared && inside != Some(&key) {
            self.out.push(indent, Some(id), &format!("K.n{id}_{pin}()"));
            return;
        }
        self.statement(indent, id, pin);
    }

    /// Emits the chain that follows exec output `pin` of node `id`.
    fn follow(&mut self, indent: usize, id: u32, pin: &str) {
        if let Some(to) = self.exec_out.get(&PinRef::new(id, pin)).cloned() {
            self.enter(indent, to.0, &to.1, None);
        }
    }

    fn comment(&mut self, indent: usize, id: u32, extra: &str) {
        let title = self.nodes[&id].def.title;
        let line = if extra.is_empty() {
            format!("-- [{id}] {title}")
        } else {
            format!("-- [{id}] {title}: {extra}")
        };
        self.out.push(indent, Some(id), &line);
    }

    fn line(&mut self, indent: usize, id: u32, text: &str) {
        self.out.push(indent, Some(id), text);
    }

    #[allow(clippy::too_many_lines)]
    fn statement(&mut self, ind: usize, id: u32, pin: &str) {
        let kind = self.nodes[&id].def.kind;
        match (kind, pin) {
            ("flow.branch", _) => {
                self.comment(ind, id, "");
                let c = self.input(id, "condition");
                self.line(ind, id, &format!("if {c} then"));
                self.follow(ind + 1, id, "true");
                if self.exec_out.contains_key(&PinRef::new(id, "false")) {
                    self.line(ind, id, "else");
                    self.follow(ind + 1, id, "false");
                }
                self.line(ind, id, "end");
            }
            ("flow.sequence", _) => {
                self.comment(ind, id, "");
                let outs: Vec<String> = self.nodes[&id]
                    .outputs
                    .iter()
                    .map(|p| p.name.clone())
                    .collect();
                for o in outs {
                    self.follow(ind, id, &o);
                }
            }
            ("flow.delay", _) => {
                let s = self.input(id, "seconds");
                self.comment(ind, id, "");
                self.line(ind, id, &format!("dt_timer(\"d{id}\", {s}, true)"));
            }
            ("flow.for", _) => {
                self.comment(ind, id, "");
                let (a, b) = (self.input(id, "first"), self.input(id, "last"));
                self.line(ind, id, &format!("for i{id} = {a}, {b} do"));
                self.line(ind + 1, id, &format!("{} = i{id}", slot(id, "index")));
                self.follow(ind + 1, id, "body");
                self.line(ind, id, "end");
                self.follow(ind, id, "done");
            }
            ("flow.for_each_member", _) => {
                self.comment(ind, id, "");
                let g = self.input(id, "group");
                let g = receiver(&g);
                self.line(ind, id, &format!("{g}:ForEachMember(function(member)"));
                self.line(ind + 1, id, &format!("{} = member", slot(id, "member")));
                self.follow(ind + 1, id, "body");
                self.line(ind, id, "end)");
                self.follow(ind, id, "done");
            }
            ("flow.gate", "enter") => {
                self.comment(ind, id, "enter");
                self.line(ind, id, &format!("if {} then", slot(id, "open")));
                self.follow(ind + 1, id, "exit");
                self.line(ind, id, "end");
            }
            ("flow.gate", "open" | "close" | "toggle") => {
                self.comment(ind, id, pin);
                let s = slot(id, "open");
                let v = match pin {
                    "open" => "true".to_owned(),
                    "close" => "false".to_owned(),
                    _ => format!("not {s}"),
                };
                self.line(ind, id, &format!("{s} = {v}"));
            }
            ("flow.do_once", "reset") => {
                self.comment(ind, id, "reset");
                self.line(ind, id, &format!("{} = false", slot(id, "done")));
            }
            ("flow.do_once", _) => {
                self.comment(ind, id, "");
                let s = slot(id, "done");
                self.line(ind, id, &format!("if not {s} then"));
                self.line(ind + 1, id, &format!("{s} = true"));
                self.follow(ind + 1, id, "then");
                self.line(ind, id, "end");
            }
            ("flow.flip_flop", _) => {
                self.comment(ind, id, "");
                let s = slot(id, "is_a");
                self.line(ind, id, &format!("{s} = not {s}"));
                self.line(ind, id, &format!("if {s} then"));
                self.follow(ind + 1, id, "a");
                if self.exec_out.contains_key(&PinRef::new(id, "b")) {
                    self.line(ind, id, "else");
                    self.follow(ind + 1, id, "b");
                }
                self.line(ind, id, "end");
            }
            ("flow.call_function", _) => {
                let name = self.param_str(id, "function");
                self.comment(ind, id, &name);
                let args: Vec<String> = self.nodes[&id]
                    .inputs
                    .iter()
                    .filter(|p| p.ty != PinType::Exec)
                    .map(|p| p.name.clone())
                    .collect();
                let args: Vec<String> = args.iter().map(|a| self.input(id, a)).collect();
                self.line(ind, id, &format!("F.{name}({})", args.join(", ")));
                self.follow(ind, id, "then");
            }
            ("var.set", _) => {
                let name = self.param_str(id, "variable");
                self.comment(ind, id, &name);
                let v = self.input(id, "value");
                self.line(ind, id, &format!("V.{name} = {v}"));
                self.follow(ind, id, "then");
            }
            ("engine.set", _) => {
                self.comment(ind, id, "");
                let (n, v) = (self.input(id, "name"), self.input(id, "value"));
                self.line(ind, id, &format!("dt_set({n}, {v})"));
                self.follow(ind, id, "then");
            }
            ("action.send_message", _) => {
                self.comment(ind, id, "");
                let m = self.input(id, "message");
                self.line(ind, id, &format!("SendMessage({m})"));
                self.follow(ind, id, "then");
            }
            ("action.send_message_data", _) => {
                self.comment(ind, id, "");
                let (m, t, d) = (
                    self.input(id, "message"),
                    self.input(id, "type"),
                    self.input(id, "data"),
                );
                self.line(ind, id, &format!("SendMessage({m}, {t}, {d})"));
                self.follow(ind, id, "then");
            }
            ("action.query", _) => {
                self.comment(ind, id, "");
                let (m, d) = (self.input(id, "message"), self.input(id, "data"));
                let outs = ["r1", "r2", "r3"].map(|p| slot(id, p)).join(", ");
                self.line(ind, id, &format!("{outs} = SendMessageEx({m}, {d})"));
                self.follow(ind, id, "then");
            }
            ("timer.start" | "timer.stop", _) => {
                let name = self.param_str(id, "timer");
                self.comment(ind, id, &name);
                if let Some(&t) = self.timers.get(&name) {
                    if kind == "timer.start" {
                        let secs = lua_number(self.param(t, "seconds").as_f64().unwrap_or(1.0));
                        let once = !self.param(t, "repeat").as_bool().unwrap_or(true);
                        self.line(ind, id, &format!("dt_timer(\"t{t}\", {secs}, {once})"));
                    } else {
                        self.line(ind, id, &format!("dt_stop_timer(\"t{t}\")"));
                    }
                }
                self.follow(ind, id, "then");
            }
            ("audio.play_emx" | "audio.play_stream", _) => {
                self.comment(ind, id, "");
                let f = if kind == "audio.play_emx" {
                    "PlayEmx"
                } else {
                    "PlayEarsAudioStream"
                };
                let name = self.input(
                    id,
                    if kind == "audio.play_emx" {
                        "sound"
                    } else {
                        "stream"
                    },
                );
                let vol = self.input(id, "volume");
                let name = if kind == "audio.play_emx" {
                    format!("dt_emx({name})")
                } else {
                    name
                };
                self.line(
                    ind,
                    id,
                    &format!("{} = {f}({name}, {vol})", slot(id, "handle")),
                );
                self.follow(ind, id, "then");
            }
            ("audio.stop_emx" | "audio.stop_stream", _) => {
                self.comment(ind, id, "");
                let f = if kind == "audio.stop_emx" {
                    "StopEmx"
                } else {
                    "StopEarsAudioStream"
                };
                let h = self.input(id, "handle");
                self.line(ind, id, &format!("{f}({h})"));
                self.follow(ind, id, "then");
            }
            ("audio.emx_volume", _) => {
                self.comment(ind, id, "");
                let (h, v) = (self.input(id, "handle"), self.input(id, "volume"));
                self.line(ind, id, &format!("SetEmxVolume({h}, {v})"));
                self.follow(ind, id, "then");
            }
            ("group.register", _) => {
                self.comment(ind, id, "");
                let n = self.input(id, "name");
                self.line(
                    ind,
                    id,
                    &format!("{} = GroupManager:RegisterGroup({n})", slot(id, "group")),
                );
                self.follow(ind, id, "then");
            }
            ("group.add" | "group.remove", _) => {
                self.comment(ind, id, "");
                let (g, m) = (self.input(id, "group"), self.input(id, "member"));
                let f = if kind == "group.add" {
                    "AddMember"
                } else {
                    "RemoveMember"
                };
                self.line(ind, id, &format!("{}:{f}({m})", receiver(&g)));
                self.follow(ind, id, "then");
            }
            ("group.clear", _) => {
                self.comment(ind, id, "");
                let g = self.input(id, "group");
                self.line(ind, id, &format!("{}:Clear()", receiver(&g)));
                self.follow(ind, id, "then");
            }
            ("lua.call", _) => {
                let f = self.param_str(id, "function");
                self.comment(ind, id, &f);
                let n = &self.nodes[&id];
                let args: Vec<String> = n
                    .inputs
                    .iter()
                    .filter(|p| p.ty != PinType::Exec)
                    .map(|p| p.name.clone())
                    .collect();
                let results: Vec<String> = n
                    .outputs
                    .iter()
                    .filter(|p| p.ty != PinType::Exec)
                    .map(|p| slot(id, &p.name))
                    .collect();
                let args: Vec<String> = args.iter().map(|a| self.input(id, a)).collect();
                let call = format!("{f}({})", args.join(", "));
                if results.is_empty() {
                    self.line(ind, id, &call);
                } else {
                    self.line(ind, id, &format!("{} = {call}", results.join(", ")));
                }
                self.follow(ind, id, "then");
            }
            ("lua.code", _) => {
                self.comment(ind, id, "");
                let code = self.param_str(id, "code");
                let mut map = HashMap::new();
                for p in library::pin_list(&self.param(id, "inputs")) {
                    let e = self.input(id, &p.name);
                    map.insert(p.name.clone(), format!("({e})"));
                }
                for p in library::pin_list(&self.param(id, "outputs")) {
                    map.insert(p.name.clone(), slot(id, &p.name));
                }
                let text = substitute(&code, &map);
                self.line(ind, id, "do");
                self.line(ind + 1, id, &text);
                self.line(ind, id, "end");
                self.follow(ind, id, "then");
            }
            ("debug.probe", _) => {
                self.comment(ind, id, "");
                let tag = self.input(id, "tag");
                let name = match self.nodes[&id]
                    .node
                    .inputs
                    .get("tag")
                    .and_then(Json::as_str)
                {
                    Some(t) if !self.data_in.contains_key(&PinRef::new(id, "tag")) => {
                        lua_string(&format!("doctor_probe_{t}.lua"))
                    }
                    _ => format!("\"doctor_probe_\" .. {tag} .. \".lua\""),
                };
                self.line(ind, id, &format!("loadfile({name})"));
                self.follow(ind, id, "then");
            }
            _ => {
                self.comment(ind, id, "(nothing to do)");
            }
        }
    }

    // ---------------------------------------------------------- generation

    fn generate(&mut self) {
        let g = self.graph;
        let name = if is_identifier(&g.name) {
            g.name.as_str()
        } else {
            "script"
        };
        self.out.push(
            0,
            None,
            &format!(
                "-- {name}.lua: made by doctor_toolkit from the visual script {name:?}.\n\
                 -- Edit the graph, not this file: it is generated again."
            ),
        );
        if !g.description.is_empty() {
            for l in g.description.lines() {
                self.out.push(0, None, &format!("-- {l}"));
            }
        }
        self.out.blank();
        let prelude = PRELUDE
            .replace("@NAME@", name)
            .replace("@TRACE@", if g.trace_errors { "true" } else { "false" });
        self.out.push(0, None, prelude.trim_end());
        self.out.blank();

        // Variables and node state.
        if !g.variables.is_empty() {
            self.out.push(0, None, "-- Variables");
            for v in &g.variables {
                let value = literal(&v.default, v.ty).unwrap_or_else(|| "nil".into());
                self.out.push(0, None, &format!("V.{} = {value}", v.name));
            }
            self.out.blank();
        }
        let ids: Vec<u32> = self.nodes.keys().copied().collect();
        let mut state = Vec::new();
        for &id in &ids {
            match self.nodes[&id].def.kind {
                "flow.gate" => {
                    let open = self.param(id, "start_open").as_bool().unwrap_or(true);
                    state.push((id, format!("{} = {open}", slot(id, "open"))));
                }
                "flow.do_once" => state.push((id, format!("{} = false", slot(id, "done")))),
                "flow.flip_flop" => state.push((id, format!("{} = false", slot(id, "is_a")))),
                _ => {}
            }
        }
        if !state.is_empty() {
            self.out.push(0, None, "-- Node state");
            for (id, line) in state {
                self.out.push(0, Some(id), &line);
            }
            self.out.blank();
        }

        // Functions of the script.
        for (fname, &id) in &self.functions.clone() {
            let params: Vec<String> = library::pin_list(&self.param(id, "inputs"))
                .into_iter()
                .map(|p| p.name)
                .collect();
            self.comment(0, id, fname);
            self.line(
                0,
                id,
                &format!("F.{fname} = function({})", params.join(", ")),
            );
            for p in &params {
                self.line(1, id, &format!("{} = {p}", slot(id, p)));
            }
            self.follow(1, id, "then");
            self.line(0, id, "end");
            self.out.blank();
        }

        // Shared entry points and Delay continuations.
        let shared: BTreeSet<PinRef> = self
            .exec_in_count
            .iter()
            .filter(|(_, n)| **n > 1)
            .map(|(k, _)| k.clone())
            .collect();
        for key in &shared {
            if !self.nodes.contains_key(&key.0) {
                continue;
            }
            self.line(0, key.0, &format!("K.n{}_{} = function()", key.0, key.1));
            self.enter(1, key.0, &key.1, Some(key));
            self.line(0, key.0, "end");
            self.out.blank();
        }
        let delays: Vec<u32> = ids
            .iter()
            .copied()
            .filter(|id| self.nodes[id].def.kind == "flow.delay")
            .collect();
        for &id in &delays {
            self.line(
                0,
                id,
                &format!("-- [{id}] Delay: what runs when the wait is over"),
            );
            self.line(0, id, &format!("K.d{id} = function()"));
            self.follow(1, id, "then");
            self.line(0, id, "end");
            self.line(0, id, &format!("{}d{id} = function()", global_prefix(name)));
            self.line(1, id, &format!("dt_run(\"d{id}\", K.d{id})"));
            self.line(0, id, "end");
            self.out.blank();
        }

        // Events.
        let mut messages: Vec<(String, u32)> = Vec::new();
        let mut loads = Vec::new();
        let mut stops = Vec::new();
        let mut autostart = Vec::new();
        for &id in &ids {
            let kind = self.nodes[&id].def.kind;
            match kind {
                "event.load" | "event.stop" => {
                    self.comment(0, id, "");
                    self.line(0, id, &format!("E.n{id} = function()"));
                    self.follow(1, id, "then");
                    self.line(0, id, "end");
                    if kind == "event.load" {
                        loads.push(id)
                    } else {
                        stops.push(id)
                    }
                }
                "event.message" => {
                    let m = self.param_str(id, "message");
                    self.comment(0, id, &m);
                    self.line(0, id, &format!("E.n{id} = function(data, dtype)"));
                    self.line(1, id, &format!("{} = data", slot(id, "data")));
                    self.line(1, id, &format!("{} = dtype", slot(id, "type")));
                    self.follow(1, id, "then");
                    self.line(0, id, "end");
                    messages.push((m, id));
                }
                "event.timer" => {
                    let t = self.param_str(id, "name");
                    self.comment(0, id, &t);
                    self.line(0, id, &format!("E.n{id} = function()"));
                    self.follow(1, id, "then");
                    self.line(0, id, "end");
                    self.line(0, id, &format!("{}t{id} = function()", global_prefix(name)));
                    self.line(1, id, &format!("dt_run(\"t{id}\", E.n{id})"));
                    self.line(0, id, "end");
                    if self.param(id, "autostart").as_bool().unwrap_or(true) {
                        autostart.push(id);
                    }
                }
                _ => continue,
            }
            self.out.blank();
        }

        // Start and stop.
        if !loads.is_empty() {
            self.out.push(
                0,
                None,
                "-- The On start events, at the first game tick after the start",
            );
            self.out.push(
                0,
                None,
                &format!("{}start = function()", global_prefix(name)),
            );
            for &id in &loads {
                self.out
                    .push(1, Some(id), &format!("dt_run(\"n{id}\", E.n{id})"));
            }
            self.out.push(0, None, "end");
            self.out.blank();
        }
        self.out
            .push(0, None, "RegisterFunction(SCRIPT, \"startup\", function()");
        for (m, id) in &messages {
            self.out
                .push(1, Some(*id), &format!("dt_on({}, E.n{id})", lua_string(m)));
        }
        if !messages.is_empty() {
            self.out
                .push(1, None, "dt.receiver = NewMessageReceiver(dt_receive)");
            self.out
                .push(1, None, "for message in pairs(dt.handlers) do");
            self.out.push(2, None, "dt.receiver:AddMessage(message)");
            self.out.push(1, None, "end");
        }
        for id in autostart {
            let secs = lua_number(self.param(id, "seconds").as_f64().unwrap_or(1.0));
            let once = !self.param(id, "repeat").as_bool().unwrap_or(true);
            self.out
                .push(1, Some(id), &format!("dt_timer(\"t{id}\", {secs}, {once})"));
        }
        if !loads.is_empty() {
            // On start waits for the first game tick: a script that starts
            // when the level loads runs while its SimGroup is still being
            // read, and messages to the objects after it would be lost.
            self.out.push(1, None, "dt_timer(\"start\", 0.001, true)");
        }
        self.out.push(0, None, "end)");
        self.out.blank();
        self.out
            .push(0, None, "RegisterFunction(SCRIPT, \"shutdown\", function()");
        for id in stops {
            self.out
                .push(1, Some(id), &format!("dt_run(\"n{id}\", E.n{id})"));
        }
        self.out.push(1, None, "for key, t in pairs(dt.timers) do");
        self.out.push(2, None, "StopTimer(t)");
        self.out.push(1, None, "end");
        self.out.push(1, None, "if dt.receiver then");
        self.out.push(2, None, "dt.receiver:Delete()");
        self.out.push(2, None, "dt.receiver = nil");
        self.out.push(1, None, "end");
        self.out.push(0, None, "end)");
    }
}

/// An expression to call a method on at the start of a statement: Lua 5.0
/// reads a line starting with `(` as a call of the previous line, so
/// anything but a name goes through `dt_self`.
fn receiver(e: &str) -> String {
    if e.split('.').all(is_identifier) {
        e.to_owned()
    } else {
        format!("dt_self({e})")
    }
}

/// Replaces `{name}` by `map[name]`; other braces stay.
fn substitute(code: &str, map: &HashMap<String, String>) -> String {
    let mut out = String::new();
    let mut rest = code;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if map.contains_key(after[..close].trim()) => {
                out.push_str(&map[after[..close].trim()]);
                rest = &after[close + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}
