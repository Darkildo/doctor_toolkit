//! A run described as data: what `logic-run` of the command line, the tool
//! `logic_run` of the MCP server and the run mode of the editor start from.

use serde::Deserialize;

use crate::game::{GameInstance, ResourceIndex};
use crate::hash::string_id;
use crate::workspace::Workspace;
use crate::{Error, Result};

use super::super::ScopeSpec;
use super::{Breakpoint, ManualReaction, Run, RunOptions, Stop, Until, load, parse_inputs};

/// What to run and how.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RunSpec {
    /// The logic to run: `mission:<name>`, `checkpoint:<number>`,
    /// `stream:<stream>`, `group:<stream>#<resource>`, `level:<stream>` or
    /// `game`.
    pub scope: String,
    /// Inputs, in order: `load`, `send:MSG`, `enter:OBJ`, `exit:OBJ`,
    /// `var:NAME=N`, `checkpoint:N`, `wait:SECONDS`, `at:T:<input>`.
    /// Without any, the run starts with `load`.
    #[serde(default)]
    pub events: Vec<String>,
    /// Stop at this game time, seconds [default: when nothing is left to do].
    #[serde(default)]
    pub until: Option<f64>,
    /// Stop after this many steps (default 20000).
    #[serde(default)]
    pub steps: Option<usize>,
    /// Stop before these messages are sent.
    #[serde(default)]
    pub break_message: Vec<String>,
    /// Stop before these objects get a message: GUID, name or `#<actor>`.
    #[serde(default)]
    pub break_object: Vec<String>,
    /// Reactions of black-box objects (classes whose code is not modelled):
    /// when `object` gets `on` (any message when left out) it sends `send`.
    #[serde(default)]
    pub reactions: Vec<ManualReaction>,
    /// Seed of `rand()` for `TriggerRandom` (default 1).
    #[serde(default)]
    pub seed: Option<u32>,
    /// Do not run Lua scripts.
    #[serde(default)]
    pub no_lua: bool,
}

impl RunSpec {
    /// A run of `scope` with the defaults.
    pub fn new(scope: &str) -> Self {
        Self {
            scope: scope.to_owned(),
            events: Vec::new(),
            until: None,
            steps: None,
            break_message: Vec::new(),
            break_object: Vec::new(),
            reactions: Vec::new(),
            seed: None,
            no_lua: false,
        }
    }

    /// Loads the scope and makes the run with its reactions, breakpoints
    /// and inputs scheduled; nothing has run yet. The second value lists
    /// what could not be read or found (groups that fail to parse,
    /// reactions without an object).
    pub fn prepare(
        &self,
        game: &GameInstance,
        index: &ResourceIndex,
        ws: Option<&Workspace>,
    ) -> Result<(Run, Vec<String>)> {
        let spec = ScopeSpec::parse(game, &self.scope)?;
        let (input, errors) = load(game, index, ws, &spec)?;
        let mut warnings: Vec<String> = errors
            .iter()
            .map(|(l, e)| {
                format!(
                    "{} #{}: {e}",
                    game.streams()[l.stream].stream_name(),
                    l.resource
                )
            })
            .collect();
        let mut run = Run::new(
            input,
            RunOptions {
                seed: self.seed.unwrap_or(1),
                lua: !self.no_lua,
                ..RunOptions::default()
            },
        );
        for reaction in &self.reactions {
            if run.add_reaction(reaction).is_empty() {
                warnings.push(format!(
                    "reaction for {:?}: no such object",
                    reaction.object
                ));
            }
        }
        for message in &self.break_message {
            run.toggle_breakpoint(Breakpoint::Message(string_id(message)));
        }
        for object in &self.break_object {
            let found = run.find(object);
            if found.is_empty() {
                return Err(Error::InvalidArgument(format!(
                    "break_object {object:?}: no such object"
                )));
            }
            for actor in found {
                run.toggle_breakpoint(Breakpoint::Object(actor));
            }
        }
        let load_only = ["load".to_owned()];
        let events: &[String] = if self.events.is_empty() {
            &load_only
        } else {
            &self.events
        };
        for (time, input) in parse_inputs(events)? {
            run.schedule(time, input);
        }
        Ok((run, warnings))
    }

    /// Runs a prepared run to the end the spec asks for.
    pub fn run(&self, run: &mut Run) -> Stop {
        run.run(
            self.until.map_or(Until::Idle, Until::Time),
            self.steps.unwrap_or(20_000),
        )
    }
}

impl Stop {
    /// Why the run stopped, in words.
    pub fn text(&self) -> String {
        match self {
            Self::Steps => "step count".into(),
            Self::Message => "message".into(),
            Self::Time => "time reached".into(),
            Self::Idle => "nothing left to do".into(),
            Self::Breakpoint(what) => format!("breakpoint: {what}"),
            Self::Limit => "step limit".into(),
        }
    }
}
