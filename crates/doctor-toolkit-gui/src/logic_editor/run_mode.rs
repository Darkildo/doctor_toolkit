//! «Run» mode of the logic tab (stage 38): the logic of the scope runs
//! without the game ([`doctor_toolkit::logic_graph::run`]); messages that
//! went through are lit on the graph, the panel below steps the run, takes
//! inputs, breakpoints and manual reactions, and shows the journal.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use doctor_toolkit::hash::string_id;
use doctor_toolkit::logic_graph::NodeSource;
use doctor_toolkit::logic_graph::run::{
    self, Breakpoint, Comparison, Input, ManualOut, ManualReaction, Run, RunOptions, Stop, Took,
    Until, What,
};
use doctor_toolkit::workspace::Workspace;
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::{Color32, RichText};

use super::LogicEditor;
use crate::node_graph::{Badge, Graph, NodeId};

/// Messages sent in the last command.
const LIVE_COLOR: Color32 = Color32::from_rgb(120, 235, 120);
/// Messages sent earlier in the run.
const PASSED_COLOR: Color32 = Color32::from_rgb(70, 150, 90);
/// Black boxes that got a message.
const UNKNOWN_COLOR: Color32 = Color32::from_rgb(235, 170, 60);

/// State of the run mode.
pub(super) struct RunMode {
    pub on: bool,
    run: Option<Run>,
    /// Inputs at the start (`load; wait:2; enter:…`).
    start: String,
    /// An input given now.
    now: String,
    until: String,
    seed: u32,
    lua: bool,
    break_message: String,
    react_on: String,
    react_send: String,
    react_delay: f32,
    file: String,
    compare_file: String,
    comparison: Option<Comparison>,
    status: String,
    /// Journal entries before the last command.
    mark: usize,
    /// Select the node of the last delivery after each command.
    follow: bool,
    /// A command to run once the run is ready (the environment preset).
    auto: Option<Until>,
}

impl Default for RunMode {
    fn default() -> Self {
        Self {
            on: false,
            run: None,
            start: "load".into(),
            now: String::new(),
            until: "60".into(),
            seed: 1,
            lua: true,
            break_message: String::new(),
            react_on: String::new(),
            react_send: String::new(),
            react_delay: 0.0,
            file: String::new(),
            compare_file: String::new(),
            comparison: None,
            status: String::new(),
            mark: 0,
            follow: true,
            auto: None,
        }
    }
}

enum Cmd {
    Restart,
    Go(Until),
    Input(String),
    BreakMessage(String),
    BreakObject(usize),
    React,
    Export,
    Compare,
    Select(usize),
}

impl LogicEditor {
    /// Switches the run mode on with these start inputs and runs it to
    /// `until` (the environment preset `run=…;until=…`).
    pub fn preset_run(&mut self, inputs: &str, until: Option<f64>) {
        self.runm.on = true;
        if !inputs.is_empty() {
            self.runm.start = inputs.to_owned();
        }
        self.runm.auto = Some(until.map_or(Until::Idle, Until::Time));
    }

    /// The scope changed or the workspace did: the run starts again on use.
    pub(super) fn run_reset(&mut self) {
        self.runm.run = None;
        self.runm.comparison = None;
        self.runm.mark = 0;
    }

    fn run_start(&mut self, ws: Option<&Workspace>) {
        let (Some(game), Some(index), Some(spec)) = (&self.game, &self.index, &self.spec) else {
            return;
        };
        match run::load(game, index, ws, spec) {
            Ok((input, _errors)) => {
                let mut r = Run::new(
                    input,
                    RunOptions {
                        seed: self.runm.seed,
                        lua: self.runm.lua,
                        ..RunOptions::default()
                    },
                );
                match run::parse_inputs(std::slice::from_ref(&self.runm.start)) {
                    Ok(list) => {
                        for (t, i) in list {
                            r.schedule(t, i);
                        }
                    }
                    Err(e) => self.runm.status = e.to_string(),
                }
                let black = r.actors().iter().filter(|a| a.model.is_black_box()).count();
                self.runm.status = trf(
                    "logic_run.started",
                    &[("objects", &r.actors().len()), ("black", &black)],
                );
                self.runm.run = Some(r);
                self.runm.mark = 0;
                self.runm.comparison = None;
                if self.runm.file.is_empty()
                    && let Some(dir) = &self.workspace_dir
                {
                    self.runm.file = dir
                        .join("logic_runs")
                        .join(format!("{}.json", super::file_key(&self.scope_text)))
                        .display()
                        .to_string();
                }
            }
            Err(e) => self.runm.status = e.to_string(),
        }
    }

    /// The panel under the graph.
    pub(super) fn run_panel(&mut self, ui: &mut egui::Ui, ws: Option<&Workspace>) {
        if self.runm.run.is_none() && self.graph.is_some() {
            self.run_start(ws);
            self.rebuild_shown_keep_positions();
        }
        if self.runm.run.is_some()
            && let Some(until) = self.runm.auto.take()
        {
            self.run_cmd(Cmd::Go(until), ws);
        }
        let mut cmd: Option<Cmd> = None;
        let rm = &mut self.runm;
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(tr("logic_run.restart"))
                .on_hover_text(tr("logic_run.restart_hint"))
                .clicked()
            {
                cmd = Some(Cmd::Restart);
            }
            if ui
                .button(tr("logic_run.step"))
                .on_hover_text(tr("logic_run.step_hint"))
                .clicked()
            {
                cmd = Some(Cmd::Go(Until::Steps(1)));
            }
            if ui
                .button(tr("logic_run.next_message"))
                .on_hover_text(tr("logic_run.next_message_hint"))
                .clicked()
            {
                cmd = Some(Cmd::Go(Until::Message));
            }
            ui.add(
                egui::TextEdit::singleline(&mut rm.until)
                    .desired_width(50.0)
                    .hint_text("s"),
            );
            if ui
                .button(tr("logic_run.to_time"))
                .on_hover_text(tr("logic_run.to_time_hint"))
                .clicked()
            {
                match rm.until.trim().replace(',', ".").parse::<f64>() {
                    Ok(t) => cmd = Some(Cmd::Go(Until::Time(t))),
                    Err(_) => rm.status = tr("logic_run.bad_time").to_owned(),
                }
            }
            if ui
                .button(tr("logic_run.continue"))
                .on_hover_text(tr("logic_run.continue_hint"))
                .clicked()
            {
                cmd = Some(Cmd::Go(Until::Idle));
            }
            ui.checkbox(&mut rm.follow, tr("logic_run.follow"))
                .on_hover_text(tr("logic_run.follow_hint"));
            if let Some(r) = &rm.run {
                ui.separator();
                ui.label(trf(
                    "logic_run.clock",
                    &[
                        ("time", &format!("{:.3}", r.time())),
                        ("steps", &r.steps()),
                        ("queued", &r.pending().len()),
                    ],
                ));
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(tr("logic_run.start_inputs"));
            ui.add(
                egui::TextEdit::singleline(&mut rm.start)
                    .desired_width(260.0)
                    .hint_text("load; wait:2; enter:…"),
            )
            .on_hover_text(tr("logic_run.inputs_hint"));
            ui.add(egui::DragValue::new(&mut rm.seed).prefix("seed "))
                .on_hover_text(tr("logic_run.seed_hint"));
            ui.checkbox(&mut rm.lua, "Lua");
            ui.separator();
            ui.label(tr("logic_run.input_now"));
            let edit = ui.add(
                egui::TextEdit::singleline(&mut rm.now)
                    .desired_width(220.0)
                    .hint_text("send:iMsgNewCheckpointLoaded"),
            );
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button(tr("logic_run.give")).clicked() || enter) && !rm.now.trim().is_empty() {
                cmd = Some(Cmd::Input(rm.now.clone()));
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(tr("logic_run.breakpoints"));
            ui.add(
                egui::TextEdit::singleline(&mut rm.break_message)
                    .desired_width(180.0)
                    .hint_text(tr("logic_run.break_message")),
            );
            if ui.button("+").clicked() && !rm.break_message.trim().is_empty() {
                cmd = Some(Cmd::BreakMessage(rm.break_message.trim().to_owned()));
            }
            if let Some(r) = &rm.run {
                let mut list: Vec<(String, Breakpoint)> = r
                    .breakpoints()
                    .iter()
                    .map(|b| {
                        let text = match b {
                            Breakpoint::Message(h) => r
                                .message_name(*h)
                                .map_or_else(|| format!("{h:08x}"), str::to_owned),
                            Breakpoint::Object(i) => r.actors()[*i].label(),
                        };
                        (text, b.clone())
                    })
                    .collect();
                list.sort_by(|a, b| a.0.cmp(&b.0));
                for (text, b) in list {
                    if ui
                        .small_button(format!("{text} x"))
                        .on_hover_text(tr("logic_run.break_remove"))
                        .clicked()
                    {
                        cmd = Some(match b {
                            Breakpoint::Message(_) => Cmd::BreakMessage(text),
                            Breakpoint::Object(i) => Cmd::BreakObject(i),
                        });
                    }
                }
            }
        });
        // The selected node: breakpoint and manual reaction.
        let selected_actor = self.selected.and_then(|n| {
            let (group, object) = self.graph.as_ref()?.nodes.get(n)?.object()?;
            self.runm.run.as_ref()?.actor_of(group, object)
        });
        let rm = &mut self.runm;
        if let (Some(i), Some(r)) = (selected_actor, &rm.run) {
            let a = &r.actors()[i];
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("{} · {}", a.label(), a.model.kind())).strong());
                let on = r.breakpoints().contains(&Breakpoint::Object(i));
                if ui
                    .selectable_label(on, tr("logic_run.break_object"))
                    .clicked()
                {
                    cmd = Some(Cmd::BreakObject(i));
                }
                ui.separator();
                ui.label(if a.model.is_black_box() {
                    tr("logic_run.react_black")
                } else {
                    tr("logic_run.react_extra")
                });
                ui.add(
                    egui::TextEdit::singleline(&mut rm.react_on)
                        .desired_width(140.0)
                        .hint_text(tr("logic_run.react_on")),
                );
                ui.label("->");
                ui.add(
                    egui::TextEdit::singleline(&mut rm.react_send)
                        .desired_width(140.0)
                        .hint_text(tr("logic_run.react_send")),
                );
                ui.add(
                    egui::DragValue::new(&mut rm.react_delay)
                        .speed(0.1)
                        .range(0.0..=3600.0)
                        .suffix(" s"),
                );
                if ui.button(tr("logic_run.react_add")).clicked()
                    && !rm.react_send.trim().is_empty()
                {
                    cmd = Some(Cmd::React);
                }
                if !a.manual.is_empty() {
                    ui.label(trf("logic_run.react_count", &[("n", &a.manual.len())]));
                }
            });
        }
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut rm.file)
                    .desired_width(280.0)
                    .hint_text("journal.json"),
            );
            if ui
                .button(tr("logic_run.export"))
                .on_hover_text(tr("logic_run.export_hint"))
                .clicked()
            {
                cmd = Some(Cmd::Export);
            }
            ui.separator();
            ui.add(
                egui::TextEdit::singleline(&mut rm.compare_file)
                    .desired_width(280.0)
                    .hint_text(tr("logic_run.compare_file")),
            );
            if ui
                .button(tr("logic_run.compare"))
                .on_hover_text(tr("logic_run.compare_hint"))
                .clicked()
            {
                cmd = Some(Cmd::Compare);
            }
        });
        if !rm.status.is_empty() {
            ui.label(RichText::new(&rm.status).small());
        }
        if let Some(c) = &rm.comparison {
            ui.label(
                RichText::new(trf(
                    "logic_run.compared",
                    &[
                        ("both", &c.matched.len()),
                        ("run", &c.only_run.len()),
                        ("game", &c.only_game.len()),
                        ("drift", &format!("{:.3}", c.worst_drift())),
                    ],
                ))
                .small(),
            )
            .on_hover_text(
                c.only_run
                    .iter()
                    .map(|(_, m)| format!("run: {m}"))
                    .chain(c.only_game.iter().map(|(_, m)| format!("game: {m}")))
                    .take(40)
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        // Journal, variables, queue.
        ui.columns(2, |cols| {
            if let Some(r) = &rm.run {
                let mark = rm.mark;
                egui::ScrollArea::vertical()
                    .id_salt("run-journal")
                    .auto_shrink(false)
                    .stick_to_bottom(true)
                    .show(&mut cols[0], |ui| {
                        let journal = r.journal();
                        let first = journal.len().saturating_sub(2000);
                        for (k, e) in journal.iter().enumerate().skip(first) {
                            let line = e.line().replace('→', "->").replace('←', "<-");
                            let mut text = RichText::new(line).monospace().small();
                            text = match &e.what {
                                What::Deliver {
                                    took: Took::Unknown,
                                    ..
                                } => text.color(UNKNOWN_COLOR),
                                What::Warning { .. } => text.color(Color32::YELLOW),
                                _ if k >= mark => text.color(LIVE_COLOR),
                                _ => text,
                            };
                            let actor = match &e.what {
                                What::Send { from, .. } => from.actor,
                                What::Deliver { to, .. } => to.actor,
                                What::Schedule { from, .. } => from.actor,
                                What::Variable { by, .. } => by.actor,
                                _ => None,
                            };
                            let resp = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
                            if let Some(i) = actor
                                && resp.clicked()
                            {
                                cmd = Some(Cmd::Select(i));
                            }
                        }
                    });
                egui::ScrollArea::vertical()
                    .id_salt("run-state")
                    .auto_shrink(false)
                    .show(&mut cols[1], |ui| {
                        ui.label(RichText::new(tr("logic_run.queue")).strong());
                        for (t, what) in r.pending().iter().take(50) {
                            ui.label(
                                RichText::new(format!("{t:>9.3}  {}", what.replace('→', "->")))
                                    .monospace()
                                    .small(),
                            );
                        }
                        ui.label(RichText::new(tr("logic_run.variables")).strong());
                        for (name, value, known) in r.variables() {
                            let text = format!("{name} = {value}{}", if known { "" } else { " ?" });
                            ui.label(RichText::new(text).monospace().small());
                        }
                        if let Some(cp) = r.checkpoint() {
                            ui.label(format!("checkpoint = {cp}"));
                        }
                    });
            }
        });
        if let Some(c) = cmd {
            self.run_cmd(c, ws);
        }
    }

    fn run_cmd(&mut self, cmd: Cmd, ws: Option<&Workspace>) {
        match cmd {
            Cmd::Restart => {
                let breaks: Vec<Breakpoint> = self
                    .runm
                    .run
                    .as_ref()
                    .map(|r| r.breakpoints().iter().cloned().collect())
                    .unwrap_or_default();
                self.run_start(ws);
                if let Some(r) = &mut self.runm.run {
                    for b in breaks {
                        r.toggle_breakpoint(b);
                    }
                }
            }
            Cmd::Go(until) => {
                let Some(r) = &mut self.runm.run else { return };
                self.runm.mark = r.journal().len();
                let stop = r.run(until, 20_000);
                self.runm.status = match stop {
                    Stop::Breakpoint(what) => trf("logic_run.stopped_break", &[("what", &what)]),
                    Stop::Idle => tr("logic_run.stopped_idle").to_owned(),
                    Stop::Limit => tr("logic_run.stopped_limit").to_owned(),
                    _ => String::new(),
                };
                if self.runm.follow
                    && let Some(i) =
                        r.journal()[self.runm.mark..]
                            .iter()
                            .rev()
                            .find_map(|e| match &e.what {
                                What::Deliver { to, .. } => to.actor,
                                _ => None,
                            })
                {
                    self.run_select(i);
                }
            }
            Cmd::Input(text) => match Input::parse(&text) {
                Ok(input) => {
                    if let Some(r) = &mut self.runm.run {
                        let t = r.time();
                        r.schedule(t, input);
                        self.runm.now.clear();
                        self.runm.status = tr("logic_run.input_queued").to_owned();
                    }
                }
                Err(e) => self.runm.status = e.to_string(),
            },
            Cmd::BreakMessage(m) => {
                if let Some(r) = &mut self.runm.run {
                    r.toggle_breakpoint(Breakpoint::Message(string_id(&m)));
                    self.runm.break_message.clear();
                }
            }
            Cmd::BreakObject(i) => {
                if let Some(r) = &mut self.runm.run {
                    r.toggle_breakpoint(Breakpoint::Object(i));
                }
            }
            Cmd::React => {
                let selected = self.selected.and_then(|n| {
                    let (group, object) = self.graph.as_ref()?.nodes.get(n)?.object()?;
                    self.runm.run.as_ref()?.actor_of(group, object)
                });
                if let (Some(i), Some(r)) = (selected, &mut self.runm.run) {
                    let on = self.runm.react_on.trim();
                    r.add_reaction(&ManualReaction {
                        object: format!("#{i}"),
                        on: (!on.is_empty()).then(|| on.to_owned()),
                        send: vec![ManualOut {
                            message: self.runm.react_send.trim().to_owned(),
                            delay: f64::from(self.runm.react_delay),
                        }],
                    });
                    self.runm.react_send.clear();
                }
            }
            Cmd::Export => {
                let Some(r) = &self.runm.run else { return };
                let path = PathBuf::from(self.runm.file.trim());
                let result = path
                    .parent()
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .and_then(|()| {
                        std::fs::write(
                            &path,
                            serde_json::to_string_pretty(r.journal()).unwrap_or_default(),
                        )
                    });
                self.runm.status = match result {
                    Ok(()) => trf("logic_run.exported", &[("path", &path.display())]),
                    Err(e) => e.to_string(),
                };
            }
            Cmd::Compare => {
                let Some(r) = &self.runm.run else { return };
                let path = PathBuf::from(self.runm.compare_file.trim());
                match std::fs::read_to_string(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|t| run::parse_game_journal(&t).map_err(|e| e.to_string()))
                {
                    Ok(game) => {
                        self.runm.comparison =
                            Some(run::compare(r.journal(), &game, &r.scope_messages()));
                        self.runm.status.clear();
                    }
                    Err(e) => self.runm.status = e,
                }
            }
            Cmd::Select(i) => self.run_select(i),
        }
        self.rebuild_shown_keep_positions();
    }

    fn run_select(&mut self, actor: usize) {
        let Some(a) = self.runm.run.as_ref().and_then(|r| r.actors().get(actor)) else {
            return;
        };
        let (location, object) = (a.location, a.object);
        if let Some(n) = self
            .graph
            .as_ref()
            .and_then(|g| g.node_of(location, object))
        {
            self.selected = Some(n);
            self.view.set_selection([NodeId(n as u64)]);
            self.view.frame_selection();
        }
    }

    /// Lights the graph: wires of messages sent (bright: in the last
    /// command), nodes that got messages (a badge with the count; black
    /// boxes in amber), breakpoints.
    pub(super) fn run_highlight(&self, shown: &mut Graph) {
        if !self.runm.on {
            return;
        }
        let (Some(r), Some(graph)) = (&self.runm.run, &self.graph) else {
            return;
        };
        let node_of_actor = |i: usize| {
            let a = r.actors().get(i)?;
            graph.node_of(a.location, a.object)
        };
        // (sender node or None for the engine and inputs, hash) → live.
        let mut sent: HashMap<(Option<usize>, u32), bool> = HashMap::new();
        let mut hashes: HashMap<u32, bool> = HashMap::new();
        let mut got: HashMap<usize, (usize, bool, bool)> = HashMap::new();
        for (k, e) in r.journal().iter().enumerate() {
            let live = k >= self.runm.mark;
            match &e.what {
                What::Send { from, hash, .. } => {
                    let h = u32::from_str_radix(hash, 16).unwrap_or(0);
                    let key = (from.actor.and_then(node_of_actor), h);
                    let v = sent.entry(key).or_insert(false);
                    *v |= live;
                    let v = hashes.entry(h).or_insert(false);
                    *v |= live;
                }
                What::Deliver { to, took, .. } => {
                    if let Some(n) = to.actor.and_then(node_of_actor) {
                        let g = got.entry(n).or_insert((0, false, false));
                        g.0 += 1;
                        g.1 |= live;
                        g.2 |= *took == Took::Unknown;
                    }
                }
                _ => {}
            }
        }
        for (w, wire) in shown.wires.iter_mut().enumerate() {
            let Some(gw) = self
                .wire_of
                .get(w)
                .copied()
                .flatten()
                .and_then(|i| graph.wires.get(i))
            else {
                continue;
            };
            let Some(h) = gw.message else { continue };
            let from_virtual =
                !matches!(graph.nodes[gw.from.node].source, NodeSource::Object { .. });
            let live = if from_virtual {
                hashes.get(&h).copied()
            } else {
                sent.get(&(Some(gw.from.node), h)).copied()
            };
            if let Some(live) = live {
                wire.color = if live { LIVE_COLOR } else { PASSED_COLOR };
            }
        }
        let breaks: HashSet<usize> = r
            .breakpoints()
            .iter()
            .filter_map(|b| match b {
                Breakpoint::Object(i) => node_of_actor(*i),
                Breakpoint::Message(_) => None,
            })
            .collect();
        for node in &mut shown.nodes {
            let n = node.id.0 as usize;
            if let Some((count, live, unknown)) = got.get(&n) {
                node.badges.push(Badge {
                    text: format!("»{count}"),
                    color: if *unknown {
                        UNKNOWN_COLOR
                    } else if *live {
                        LIVE_COLOR
                    } else {
                        PASSED_COLOR
                    },
                    tooltip: if *unknown {
                        tr("logic_run.badge_black").to_owned()
                    } else {
                        tr("logic_run.badge_got").to_owned()
                    },
                });
            }
            if breaks.contains(&n) {
                node.badges.push(Badge {
                    text: "BP".into(),
                    color: Color32::from_rgb(230, 70, 70),
                    tooltip: tr("logic_run.badge_break").to_owned(),
                });
            }
        }
    }
}
