//! The node library: every node kind, its pins and its settings.
//!
//! Pins of most kinds are fixed; a few follow from the node's params:
//! `flow.sequence` (`count`), functions (`inputs`), `lua.call` (`args`,
//! `results`), `lua.code` (`inputs`, `outputs`) and the graph-variable nodes
//! (the variable's type). [`pins`] resolves them for a node of a graph.

use std::sync::OnceLock;

use serde_json::Value as Json;

use super::graph::{Node, PinType, ScriptGraph};

/// Group of nodes in the palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    Event,
    Flow,
    Value,
    Math,
    Logic,
    Text,
    Variable,
    Game,
    Group,
    Message,
    Timer,
    Audio,
    Lua,
    Debug,
    Comment,
}

impl Category {
    pub fn name(self) -> &'static str {
        match self {
            Category::Event => "Events",
            Category::Flow => "Flow",
            Category::Value => "Values",
            Category::Math => "Math",
            Category::Logic => "Logic",
            Category::Text => "Text",
            Category::Variable => "Variables",
            Category::Game => "Game",
            Category::Group => "Groups",
            Category::Message => "Messages",
            Category::Timer => "Timers",
            Category::Audio => "Audio",
            Category::Lua => "Lua",
            Category::Debug => "Debug",
            Category::Comment => "Comments",
        }
    }
}

/// Kind of a setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    /// Free text.
    Text,
    /// The name of a message of the bus: editors offer the names of the
    /// game (`game_types::MessageCatalog`), any other text is a new message.
    Message,
    /// A Lua identifier (timer, function and variable names).
    Name,
    Number,
    Bool,
    /// Lua code.
    Code,
    /// A list of pins: `[{"name": "a", "type": "number"}]`.
    Pins,
}

/// A setting of a node kind.
#[derive(Debug, Clone, Copy)]
pub struct ParamDef {
    pub name: &'static str,
    pub kind: ParamKind,
    pub label: &'static str,
    /// Default as JSON text.
    pub default: &'static str,
}

/// A pin of a node kind.
#[derive(Debug, Clone, PartialEq)]
pub struct Pin {
    pub name: String,
    pub ty: PinType,
    pub label: String,
}

/// A node kind.
#[derive(Debug, Clone)]
pub struct NodeDef {
    pub kind: &'static str,
    pub title: &'static str,
    pub category: Category,
    pub help: &'static str,
    /// Fixed input pins: `(name, type, label)`; inputs of type exec are entry points.
    pub inputs: &'static [(&'static str, PinType, &'static str)],
    pub outputs: &'static [(&'static str, PinType, &'static str)],
    pub params: &'static [ParamDef],
    /// Verified in the game (`docs/scripts.md`); `false` for API calls whose
    /// arguments are deduced from the game's scripts only.
    pub verified: bool,
}

impl NodeDef {
    /// No exec pins: the node is an expression evaluated where it is used.
    pub fn is_pure(&self) -> bool {
        !self
            .inputs
            .iter()
            .chain(self.outputs)
            .any(|p| p.1 == PinType::Exec)
            && !self.kind.starts_with("event.")
            && self.category != Category::Comment
    }

    pub fn is_event(&self) -> bool {
        self.kind.starts_with("event.")
    }

    pub fn param(&self, name: &str) -> Option<&ParamDef> {
        self.params.iter().find(|p| p.name == name)
    }
}

const fn p(
    name: &'static str,
    kind: ParamKind,
    label: &'static str,
    default: &'static str,
) -> ParamDef {
    ParamDef {
        name,
        kind,
        label,
        default,
    }
}

use Category as C;
use ParamKind as K;
use PinType::{Any, Bool, Exec, Group, Number, Position, String as Str};

const EXEC_IN: (&str, PinType, &str) = ("exec", Exec, "");
const THEN: (&str, PinType, &str) = ("then", Exec, "");

macro_rules! def {
    ($kind:literal, $title:literal, $cat:expr, $help:literal,
     in [$($i:expr),* $(,)?], out [$($o:expr),* $(,)?], params [$($p:expr),* $(,)?]) => {
        NodeDef {
            kind: $kind,
            title: $title,
            category: $cat,
            help: $help,
            inputs: &[$($i),*],
            outputs: &[$($o),*],
            params: {
                const PARAMS: &[ParamDef] = &[$($p),*];
                PARAMS
            },
            verified: true,
        }
    };
}

macro_rules! unverified {
    ($d:expr) => {{
        let mut d = $d;
        d.verified = false;
        d
    }};
}

fn build() -> Vec<NodeDef> {
    vec![
        // ------------------------------------------------------------ events
        def!("event.load", "On start", C::Event,
            "Runs when the script starts: when the level loads, or on its start message.",
            in [], out [THEN], params []),
        def!("event.stop", "On stop", C::Event,
            "Runs when the script's stop message arrives (shutdown).",
            in [], out [THEN], params []),
        def!("event.message", "On message", C::Event,
            "Runs when a game message arrives. Data and type are what the sender attached.",
            in [], out [THEN, ("data", Any, "Data"), ("type", Str, "Data type")],
            params [p("message", K::Message, "Message", "\"\"")]),
        def!("event.timer", "On timer", C::Event,
        "Runs every `seconds` (or once). Start and Stop timer nodes control it by name.",
        in [], out [THEN],
        params [
            p("name", K::Name, "Timer name", "\"timer\""),
            p("seconds", K::Number, "Seconds", "1"),
            p("repeat", K::Bool, "Repeat", "true"),
            p("autostart", K::Bool, "Start with the script", "true"),
        ]),
        def!("event.function", "Function", C::Event,
            "A function of this script: Call function nodes run it with their inputs.",
            in [], out [THEN],
            params [p("name", K::Name, "Function name", "\"my_function\""), p("inputs", K::Pins, "Inputs", "[]")]),
        // -------------------------------------------------------------- flow
        def!("flow.branch", "Branch", C::Flow, "If the condition is true, runs True, else False.",
            in [EXEC_IN, ("condition", Bool, "Condition")],
            out [("true", Exec, "True"), ("false", Exec, "False")], params []),
        def!("flow.sequence", "Sequence", C::Flow, "Runs its outputs one after another.",
            in [EXEC_IN], out [], params [p("count", K::Number, "Outputs", "2")]),
        def!("flow.delay", "Delay", C::Flow,
            "Continues after `seconds` (a game timer). Triggered again while waiting, it starts the wait over.",
            in [EXEC_IN, ("seconds", Number, "Seconds")], out [THEN], params []),
        def!("flow.for", "For loop", C::Flow, "Runs Body for every whole number from First to Last, then Done.",
            in [EXEC_IN, ("first", Number, "First"), ("last", Number, "Last")],
            out [("body", Exec, "Body"), ("index", Number, "Index"), ("done", Exec, "Done")], params []),
        unverified!(def!("flow.for_each_member", "For each member", C::Flow,
            "Runs Body for every member of a group (`ForEachMember`), then Done.",
            in [EXEC_IN, ("group", Group, "Group")],
            out [("body", Exec, "Body"), ("member", Any, "Member"), ("done", Exec, "Done")], params [])),
        def!("flow.gate", "Gate", C::Flow,
            "Enter passes to Exit only while the gate is open; Open, Close and Toggle change it.",
            in [("enter", Exec, "Enter"), ("open", Exec, "Open"), ("close", Exec, "Close"), ("toggle", Exec, "Toggle")],
            out [("exit", Exec, "Exit")], params [p("start_open", K::Bool, "Starts open", "true")]),
        def!("flow.do_once", "Do once", C::Flow, "Passes the first time only, until Reset.",
            in [EXEC_IN, ("reset", Exec, "Reset")], out [THEN], params []),
        def!("flow.flip_flop", "Flip-flop", C::Flow, "Alternates between A and B.",
            in [EXEC_IN], out [("a", Exec, "A"), ("b", Exec, "B"), ("is_a", Bool, "Is A")], params []),
        def!("flow.call_function", "Call function", C::Flow, "Runs a Function of this script.",
            in [EXEC_IN], out [THEN], params [p("function", K::Name, "Function", "\"my_function\"")]),
        // ------------------------------------------------------------ values
        def!("value.number", "Number", C::Value, "A constant number.",
            in [], out [("value", Number, "")], params [p("value", K::Number, "Value", "0")]),
        def!("value.bool", "Boolean", C::Value, "A constant true or false.",
            in [], out [("value", Bool, "")], params [p("value", K::Bool, "Value", "false")]),
        def!("value.string", "Text", C::Value, "A constant text.",
            in [], out [("value", Str, "")], params [p("value", K::Text, "Value", "\"\"")]),
        // -------------------------------------------------------------- math
        def!("math.add", "Add", C::Math, "A + B", in [("a", Number, "A"), ("b", Number, "B")], out [("result", Number, "")], params []),
        def!("math.sub", "Subtract", C::Math, "A - B", in [("a", Number, "A"), ("b", Number, "B")], out [("result", Number, "")], params []),
        def!("math.mul", "Multiply", C::Math, "A × B", in [("a", Number, "A"), ("b", Number, "B")], out [("result", Number, "")], params []),
        def!("math.div", "Divide", C::Math, "A / B", in [("a", Number, "A"), ("b", Number, "B")], out [("result", Number, "")], params []),
        def!("math.mod", "Remainder", C::Math, "A mod B (math.mod)", in [("a", Number, "A"), ("b", Number, "B")], out [("result", Number, "")], params []),
        def!("math.min", "Min", C::Math, "The smaller of A and B", in [("a", Number, "A"), ("b", Number, "B")], out [("result", Number, "")], params []),
        def!("math.max", "Max", C::Math, "The larger of A and B", in [("a", Number, "A"), ("b", Number, "B")], out [("result", Number, "")], params []),
        def!("math.abs", "Absolute", C::Math, "|X|", in [("x", Number, "X")], out [("result", Number, "")], params []),
        def!("math.floor", "Floor", C::Math, "X rounded down", in [("x", Number, "X")], out [("result", Number, "")], params []),
        def!("math.negate", "Negate", C::Math, "-X", in [("x", Number, "X")], out [("result", Number, "")], params []),
        def!("math.clamp", "Clamp", C::Math, "X limited to Min…Max",
            in [("x", Number, "X"), ("min", Number, "Min"), ("max", Number, "Max")], out [("result", Number, "")], params []),
        def!("math.random", "Random integer", C::Math, "A whole number from Min to Max (math.random)",
            in [("min", Number, "Min"), ("max", Number, "Max")], out [("result", Number, "")], params []),
        // ------------------------------------------------------------- logic
        def!("compare.eq", "Equal", C::Logic, "A == B", in [("a", Any, "A"), ("b", Any, "B")], out [("result", Bool, "")], params []),
        def!("compare.ne", "Not equal", C::Logic, "A ~= B", in [("a", Any, "A"), ("b", Any, "B")], out [("result", Bool, "")], params []),
        def!("compare.lt", "Less", C::Logic, "A < B", in [("a", Number, "A"), ("b", Number, "B")], out [("result", Bool, "")], params []),
        def!("compare.le", "Less or equal", C::Logic, "A <= B", in [("a", Number, "A"), ("b", Number, "B")], out [("result", Bool, "")], params []),
        def!("compare.gt", "Greater", C::Logic, "A > B", in [("a", Number, "A"), ("b", Number, "B")], out [("result", Bool, "")], params []),
        def!("compare.ge", "Greater or equal", C::Logic, "A >= B", in [("a", Number, "A"), ("b", Number, "B")], out [("result", Bool, "")], params []),
        def!("logic.and", "And", C::Logic, "A and B", in [("a", Bool, "A"), ("b", Bool, "B")], out [("result", Bool, "")], params []),
        def!("logic.or", "Or", C::Logic, "A or B", in [("a", Bool, "A"), ("b", Bool, "B")], out [("result", Bool, "")], params []),
        def!("logic.not", "Not", C::Logic, "not X", in [("x", Bool, "X")], out [("result", Bool, "")], params []),
        // -------------------------------------------------------------- text
        def!("string.concat", "Join text", C::Text, "A followed by B",
            in [("a", Str, "A"), ("b", Str, "B")], out [("result", Str, "")], params []),
        def!("string.from", "To text", C::Text, "Any value as text (tostring)",
            in [("value", Any, "Value")], out [("result", Str, "")], params []),
        def!("string.number", "Format number", C::Text, "A number with a fixed count of decimals",
            in [("value", Number, "Value"), ("decimals", Number, "Decimals")], out [("result", Str, "")], params []),
        def!("string.length", "Text length", C::Text, "Number of bytes",
            in [("text", Str, "Text")], out [("result", Number, "")], params []),
        // --------------------------------------------------------- variables
        def!("var.get", "Get variable", C::Variable, "Value of a variable of this script.",
            in [], out [("value", Any, "")], params [p("variable", K::Name, "Variable", "\"\"")]),
        def!("var.set", "Set variable", C::Variable, "Sets a variable of this script.",
            in [EXEC_IN, ("value", Any, "Value")], out [THEN, ("value", Any, "")],
            params [p("variable", K::Name, "Variable", "\"\"")]),
        def!("engine.get", "Get game variable", C::Variable,
            "Value of a game variable (FindVariable; created if missing). Levels only: the main menu has no variables.",
            in [("name", Str, "Name")], out [("value", Any, "")], params []),
        def!("engine.set", "Set game variable", C::Variable,
            "Sets a game variable (VariableSet), seen by VariableSwitch, VariableWatcher and other objects.",
            in [EXEC_IN, ("name", Str, "Name"), ("value", Any, "Value")], out [THEN], params []),
        // -------------------------------------------------------------- game
        def!("game.sim_time", "Game time", C::Game, "Seconds of game time (GetSimTime() gives milliseconds).",
            in [], out [("seconds", Number, "")], params []),
        def!("game.player_position", "Player position", C::Game, "GetPlayerPosition()",
            in [], out [("position", Position, "")], params []),
        def!("game.position_xyz", "Break position", C::Game, "X, Y (up) and Z of a position (GetPositionValues)",
            in [("position", Position, "Position")], out [("x", Number, "X"), ("y", Number, "Y"), ("z", Number, "Z")], params []),
        def!("game.distance", "Distance", C::Game, "Distance between two positions in metres",
            in [("a", Position, "A"), ("b", Position, "B")], out [("result", Number, "")], params []),
        def!("game.distance_to", "Distance to point", C::Game, "Distance from a position to X, Y, Z",
            in [("position", Position, "Position"), ("x", Number, "X"), ("y", Number, "Y"), ("z", Number, "Z")],
            out [("result", Number, "")], params []),
        unverified!(
            def!("game.foot_police", "Foot police intensity", C::Game, "GetFootPoliceIntensity()",
            in [], out [("value", Number, "")], params [])
        ),
        unverified!(
            def!("game.car_police", "Car police intensity", C::Game, "GetCarPoliceIntensity()",
            in [], out [("value", Number, "")], params [])
        ),
        unverified!(
            def!("game.vehicle_speed", "Vehicle speed", C::Game, "GetVehicleSpeed(): speed of the player's vehicle",
            in [], out [("value", Number, "")], params [])
        ),
        unverified!(
            def!("game.vehicle_type", "Vehicle type", C::Game, "GetVehicleType()",
            in [], out [("value", Any, "")], params [])
        ),
        def!("game.platform", "Platform", C::Game, "GetPlatform()", in [], out [("value", Any, "")], params []),
        // ------------------------------------------------------------ groups
        unverified!(
            def!("group.find", "Find group", C::Group, "GroupManager:FindGroup(name)",
            in [("name", Str, "Name")], out [("group", Group, "")], params [])
        ),
        unverified!(
            def!("group.register", "Create group", C::Group, "GroupManager:RegisterGroup(name)",
            in [EXEC_IN, ("name", Str, "Name")], out [THEN, ("group", Group, "Group")], params [])
        ),
        unverified!(
            def!("group.add", "Add to group", C::Group, "group:AddMember(member)",
            in [EXEC_IN, ("group", Group, "Group"), ("member", Any, "Member")], out [THEN], params [])
        ),
        unverified!(
            def!("group.remove", "Remove from group", C::Group, "group:RemoveMember(member)",
            in [EXEC_IN, ("group", Group, "Group"), ("member", Any, "Member")], out [THEN], params [])
        ),
        unverified!(
            def!("group.clear", "Clear group", C::Group, "group:Clear()",
            in [EXEC_IN, ("group", Group, "Group")], out [THEN], params [])
        ),
        unverified!(
            def!("group.count", "Member count", C::Group, "group:GetMemberCount()",
            in [("group", Group, "Group")], out [("result", Number, "")], params [])
        ),
        unverified!(
            def!("group.contains", "Is member", C::Group, "group:IsMember(member)",
            in [("group", Group, "Group"), ("member", Any, "Member")], out [("result", Bool, "")], params [])
        ),
        // ---------------------------------------------------------- messages
        def!("action.send_message", "Send message", C::Message,
            "Sends a game message (SendMessage): triggers, MultiManagers, other scripts listening to it.",
            in [EXEC_IN, ("message", Str, "Message")], out [THEN], params []),
        unverified!(
            def!("action.send_message_data", "Send message with data", C::Message,
            "SendMessage(message, type, data)",
            in [EXEC_IN, ("message", Str, "Message"), ("type", Str, "Data type"), ("data", Any, "Data")], out [THEN], params [])
        ),
        unverified!(def!("action.query", "Ask the game", C::Message,
            "SendMessageEx(message, data): messages answered by the game, e.g. iMsgLuaGetPlayerVenue",
            in [EXEC_IN, ("message", Str, "Message"), ("data", Any, "Data")],
            out [THEN, ("r1", Any, "Result 1"), ("r2", Any, "Result 2"), ("r3", Any, "Result 3")], params [])),
        // ------------------------------------------------------------ timers
        def!("timer.start", "Start timer", C::Timer, "Starts (or restarts) an On timer event of this script.",
            in [EXEC_IN], out [THEN], params [p("timer", K::Name, "Timer", "\"timer\"")]),
        def!("timer.stop", "Stop timer", C::Timer, "Stops an On timer event of this script.",
            in [EXEC_IN], out [THEN], params [p("timer", K::Name, "Timer", "\"timer\"")]),
        // ------------------------------------------------------------- audio
        unverified!(
            def!("audio.play_emx", "Play sound", C::Audio, "PlayEmx(name, volume): a sound emitter (.emx)",
            in [EXEC_IN, ("sound", Str, "Sound (.emx)"), ("volume", Number, "Volume")],
            out [THEN, ("handle", Any, "Handle")], params [])
        ),
        unverified!(
            def!("audio.stop_emx", "Stop sound", C::Audio, "StopEmx(handle)",
            in [EXEC_IN, ("handle", Any, "Handle")], out [THEN], params [])
        ),
        unverified!(
            def!("audio.emx_volume", "Sound volume", C::Audio, "SetEmxVolume(handle, volume)",
            in [EXEC_IN, ("handle", Any, "Handle"), ("volume", Number, "Volume")], out [THEN], params [])
        ),
        unverified!(
            def!("audio.play_stream", "Play stream", C::Audio, "PlayEarsAudioStream(name, volume): music or speech (.stx)",
            in [EXEC_IN, ("stream", Str, "Stream (.stx)"), ("volume", Number, "Volume")],
            out [THEN, ("handle", Any, "Handle")], params [])
        ),
        unverified!(
            def!("audio.stop_stream", "Stop stream", C::Audio, "StopEarsAudioStream(handle)",
            in [EXEC_IN, ("handle", Any, "Handle")], out [THEN], params [])
        ),
        // --------------------------------------------------------------- lua
        def!("lua.call", "Call game function", C::Lua,
        "Calls any global function of the game's Lua (see docs/formats/mission-logic.md §2.3).",
        in [EXEC_IN], out [THEN],
        params [
            p("function", K::Text, "Function", "\"\""),
            p("args", K::Number, "Arguments", "0"),
            p("results", K::Number, "Results", "0"),
        ]),
        def!("lua.code", "Lua code", C::Lua,
            "Your own Lua 5.0 statements. Inputs are read as `{name}`, outputs assigned as `{name} = …`.",
            in [EXEC_IN], out [THEN],
            params [p("code", K::Code, "Code", "\"\""), p("inputs", K::Pins, "Inputs", "[]"), p("outputs", K::Pins, "Outputs", "[]")]),
        // ------------------------------------------------------------- debug
        def!("debug.probe", "File probe", C::Debug,
            "loadfile(\"doctor_probe_<tag>.lua\"): the game looks for that file, which a file trace shows (WINEDEBUG=+file, strace).",
            in [EXEC_IN, ("tag", Str, "Tag")], out [THEN], params []),
        def!("comment", "Comment", C::Comment, "A note on the canvas.",
            in [], out [], params [p("text", K::Text, "Text", "\"\""), p("width", K::Number, "Width", "300"), p("height", K::Number, "Height", "120")]),
    ]
}

/// Value of a data input that has neither a link nor a value, when it is
/// not the type's zero (`0`, `false`, `""`, nil).
pub fn input_default(kind: &str, pin: &str) -> Option<Json> {
    Some(match (kind, pin) {
        ("flow.delay", "seconds") => Json::from(1),
        ("flow.for", "first" | "last") => Json::from(1),
        ("math.random", "min") => Json::from(1),
        ("math.random", "max") => Json::from(100),
        ("math.div" | "math.mod", "b") => Json::from(1),
        ("math.clamp", "max") => Json::from(1),
        ("audio.play_emx" | "audio.play_stream" | "audio.emx_volume", "volume") => Json::from(1),
        _ => return None,
    })
}

/// Every node kind.
pub fn all() -> &'static [NodeDef] {
    static DEFS: OnceLock<Vec<NodeDef>> = OnceLock::new();
    DEFS.get_or_init(build)
}

/// The kind `kind`.
pub fn get(kind: &str) -> Option<&'static NodeDef> {
    all().iter().find(|d| d.kind == kind)
}

/// Value of a param of a node, or the kind's default.
pub fn param(node: &Node, def: &NodeDef, name: &str) -> Json {
    node.params.get(name).cloned().unwrap_or_else(|| {
        def.param(name)
            .and_then(|p| serde_json::from_str(p.default).ok())
            .unwrap_or(Json::Null)
    })
}

/// Pins of a list param (`inputs`, `outputs`); bad entries are skipped.
pub fn pin_list(value: &Json) -> Vec<Pin> {
    value
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    let name = e.get("name")?.as_str()?.to_owned();
                    let ty = e
                        .get("type")
                        .and_then(Json::as_str)
                        .and_then(PinType::parse)
                        .filter(|t| *t != PinType::Exec)
                        .unwrap_or(PinType::Any);
                    Some(Pin {
                        label: name.clone(),
                        name,
                        ty,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn fixed(list: &[(&str, PinType, &str)]) -> Vec<Pin> {
    list.iter()
        .map(|(n, t, l)| Pin {
            name: (*n).to_owned(),
            ty: *t,
            label: (*l).to_owned(),
        })
        .collect()
}

fn count(node: &Node, def: &NodeDef, name: &str, max: usize) -> usize {
    param(node, def, name)
        .as_f64()
        .map_or(0, |n| n.max(0.0) as usize)
        .min(max)
}

/// Input and output pins of a node of graph `graph`; `None` for an unknown kind.
pub fn pins(graph: &ScriptGraph, node: &Node) -> Option<(Vec<Pin>, Vec<Pin>)> {
    let def = get(&node.kind)?;
    let mut inputs = fixed(def.inputs);
    let mut outputs = fixed(def.outputs);
    let var_type = |name: &Json| {
        name.as_str()
            .and_then(|n| graph.variable(n))
            .map_or(PinType::Any, |v| v.ty)
    };
    match def.kind {
        "flow.sequence" => {
            for i in 0..count(node, def, "count", 16).max(1) {
                outputs.push(Pin {
                    name: format!("then{i}"),
                    ty: PinType::Exec,
                    label: format!("Then {i}"),
                });
            }
        }
        "event.function" => outputs.extend(pin_list(&param(node, def, "inputs"))),
        "flow.call_function" => {
            let name = param(node, def, "function");
            let entry = graph.nodes.iter().find(|n| {
                n.kind == "event.function"
                    && n.params.get("name").and_then(Json::as_str) == name.as_str()
            });
            if let Some(f) = entry
                && let Some(fdef) = get("event.function")
            {
                inputs.extend(pin_list(&param(f, fdef, "inputs")));
            }
        }
        "lua.call" => {
            for i in 1..=count(node, def, "args", 8) {
                inputs.push(Pin {
                    name: format!("arg{i}"),
                    ty: PinType::Any,
                    label: format!("Argument {i}"),
                });
            }
            for i in 1..=count(node, def, "results", 4) {
                outputs.push(Pin {
                    name: format!("result{i}"),
                    ty: PinType::Any,
                    label: format!("Result {i}"),
                });
            }
        }
        "lua.code" => {
            inputs.extend(pin_list(&param(node, def, "inputs")));
            outputs.extend(pin_list(&param(node, def, "outputs")));
        }
        "var.get" | "var.set" => {
            let ty = var_type(&param(node, def, "variable"));
            for pin in inputs.iter_mut().chain(outputs.iter_mut()) {
                if pin.name == "value" {
                    pin.ty = ty;
                }
            }
        }
        _ => {}
    }
    Some((inputs, outputs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_unique_and_well_formed() {
        let mut kinds = std::collections::HashSet::new();
        for d in all() {
            assert!(kinds.insert(d.kind), "duplicate kind {}", d.kind);
            for p in d.params {
                assert!(
                    serde_json::from_str::<Json>(p.default).is_ok(),
                    "{}: default of {}",
                    d.kind,
                    p.name
                );
            }
            for list in [d.inputs, d.outputs] {
                let names: Vec<_> = list.iter().map(|p| p.0).collect();
                let unique: std::collections::HashSet<_> = names.iter().collect();
                assert_eq!(unique.len(), names.len(), "{}: pin names", d.kind);
            }
        }
        assert!(get("math.add").unwrap().is_pure());
        assert!(!get("action.send_message").unwrap().is_pure());
        assert!(!get("event.load").unwrap().is_pure());
    }

    #[test]
    fn every_kind_is_in_the_node_reference() {
        let doc = include_str!("../../../../docs/scripts.md");
        for d in all() {
            assert!(
                doc.contains(&format!("`{}`", d.kind)),
                "docs/scripts.md misses {}",
                d.kind
            );
        }
    }

    #[test]
    fn dynamic_pins_follow_params() {
        let mut g = ScriptGraph::new("t");
        g.variables.push(super::super::graph::Variable {
            name: "n".into(),
            ty: PinType::Number,
            default: Json::from(0),
        });
        let seq = Node::new(1, "flow.sequence", [0.0; 2]).param("count", 3);
        let (_, out) = pins(&g, &seq).unwrap();
        assert_eq!(out.len(), 3);
        let get_n = Node::new(2, "var.get", [0.0; 2]).param("variable", "n");
        assert_eq!(pins(&g, &get_n).unwrap().1[0].ty, PinType::Number);
        let f = Node::new(3, "event.function", [0.0; 2])
            .param("name", "f")
            .param(
                "inputs",
                serde_json::json!([{"name": "x", "type": "number"}]),
            );
        g.nodes.push(f);
        let call = Node::new(4, "flow.call_function", [0.0; 2]).param("function", "f");
        let (ins, _) = pins(&g, &call).unwrap();
        assert_eq!(
            ins.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["exec", "x"]
        );
        let lua = Node::new(5, "lua.call", [0.0; 2])
            .param("args", 2)
            .param("results", 1);
        let (ins, outs) = pins(&g, &lua).unwrap();
        assert_eq!((ins.len(), outs.len()), (3, 2));
    }
}
