//! Live debugging in the editor (stage 35): the "Debug" part of the Link
//! panel (pause, step, breakpoints, free camera, textures), and the glue
//! that gives the game's messages to the logic tabs, its prints and Lua
//! errors to the script editor, and their requests to the link.

use std::sync::atomic::Ordering;

use doctor_toolkit::live::proto::journal::mask;
use doctor_toolkit::live::proto::{self, BreakOn, BreakRule, Control, MessageId, StopReason};
use doctor_toolkit::logic_graph::trace::TraceEvent;
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::RichText;

use super::BrowserApp;
use super::live::{Cmd, shared_debug};
use crate::level_view::LevelAction;
use crate::logic_live::{LinkView, LiveRequest};

impl BrowserApp {
    /// Every frame: messages to the logic tabs, prints and errors to the
    /// script editor, their requests to the link, the confirmation window.
    pub(super) fn live_debug_frame(&mut self, ctx: &egui::Context) {
        self.live_preset(ctx);
        self.live_do_preset();
        let live_on = self.logics.values().any(|e| e.live.on);
        let script_on = self.script_editor.open;
        let mut extra = 0;
        if live_on {
            extra |= mask::MESSAGE | mask::MESSAGE_COUNTS | mask::MESSAGE_SENDER;
        }
        if script_on {
            extra |= mask::OUTPUT | mask::LUA_ERROR;
        }
        self.play.live.extra_kinds.store(extra, Ordering::Relaxed);
        let (trace, script_events, link) = {
            let connected = self.play.live.connected();
            let mut s = shared_debug(&self.play.live);
            let link = LinkView {
                connected,
                can_break: connected && s.debug_ok,
                stop: s.debug.as_ref().and_then(|d| d.stop.clone()),
                rules: s
                    .debug
                    .as_ref()
                    .map(|d| d.rules.clone())
                    .unwrap_or_default(),
                lost: s.lost_total(),
            };
            (
                std::mem::take(&mut s.trace),
                std::mem::take(&mut s.script_events),
                link,
            )
        };
        let names = self.play.live.names_now();
        let events: Vec<TraceEvent> = trace
            .iter()
            .flat_map(TraceEvent::from_live)
            .map(|mut e| {
                if e.name.is_none() {
                    e.name = names
                        .as_ref()
                        .and_then(|n| n.message(e.hash))
                        .map(str::to_owned);
                }
                e
            })
            .collect();
        let mut requests = Vec::new();
        for editor in self.logics.values_mut() {
            editor.live.set_link(link.clone());
            if !events.is_empty() {
                editor.live_feed(&events);
            }
            requests.extend(editor.live.take_requests());
        }
        self.script_editor.live.set_link(link.clone());
        if !script_events.is_empty() {
            self.script_editor.live_feed(&script_events);
        }
        requests.extend(self.script_editor.live.take_requests());
        for r in requests {
            self.live_request(r, &link);
        }
        if !events.is_empty() && (live_on || script_on) {
            ctx.request_repaint();
        }
        self.live_confirm(ctx);
    }

    /// `DOCTOR_TOOLKIT_GUI_LIVE=PORT[:TOKEN]` connects once at start (for
    /// checks without input, with `DOCTOR_TOOLKIT_GUI_LOGIC=…;live`);
    /// `DOCTOR_TOOLKIT_GUI_LIVE_FOLLOW=1` makes level tabs follow the game's
    /// camera.
    fn live_preset(&mut self, ctx: &egui::Context) {
        static ONCE: std::sync::Once = std::sync::Once::new();
        if std::env::var_os("DOCTOR_TOOLKIT_GUI_LIVE").is_none() {
            return;
        }
        if std::env::var_os("DOCTOR_TOOLKIT_GUI_LIVE_FOLLOW").is_some() {
            for level in self.levels.values_mut() {
                level.follow_game(true);
            }
        }
        ONCE.call_once(|| {
            let spec = std::env::var("DOCTOR_TOOLKIT_GUI_LIVE").unwrap_or_default();
            let (port, token) = spec.split_once(':').unwrap_or((spec.as_str(), ""));
            if let Ok(port) = port.trim().parse() {
                self.play.live.connect(
                    doctor_toolkit::live::Endpoint {
                        port,
                        token: token.trim().to_owned(),
                    },
                    ctx,
                );
            }
        });
    }

    /// A request of a logic tab or the script editor.
    fn live_request(&mut self, request: LiveRequest, link: &LinkView) {
        let hold = self.play.live.hold_s;
        match request {
            LiveRequest::Break(rule) => {
                let mut rules = link.rules.clone();
                if !rules.contains(&rule) {
                    rules.push(rule);
                }
                self.play.live.send(Cmd::Break(rules, hold));
            }
            LiveRequest::Unbreak(on) => {
                let rules = link.rules.iter().filter(|r| r.on != on).cloned().collect();
                self.play.live.send(Cmd::Break(rules, hold));
            }
            LiveRequest::Control(c) => self.play.live.send(Cmd::Control(c)),
        }
    }

    /// Camera and teleport from a level tab: writes, asked first (the free
    /// camera goes off without asking).
    pub(super) fn live_level_action(&mut self, action: LevelAction) {
        match action {
            LevelAction::LiveTeleport { position, yaw } => {
                self.play.live.confirm = Some(Cmd::Teleport(position, Some(yaw)));
            }
            LevelAction::LiveCamera(Some(c)) => {
                self.play.live.confirm = Some(Cmd::Camera(Some(c)));
            }
            LevelAction::LiveCamera(None) => self.play.live.send(Cmd::Camera(None)),
            _ => {}
        }
    }

    /// "Send edited textures": the workspace's edited textures go to the
    /// running game without a restart.
    pub(super) fn live_textures_button(&mut self, ui: &mut egui::Ui, enabled: bool) {
        let debug_ok = shared_debug(&self.play.live).debug_ok;
        if !debug_ok {
            return;
        }
        let r = ui
            .add_enabled(enabled, egui::Button::new(tr("debug.textures")))
            .on_hover_text(tr("debug.textures_hint"));
        if r.clicked() {
            let Some(ws) = self.workspace.as_ref() else {
                self.notify(super::Level::Warning, tr("debug.no_textures").to_owned());
                return;
            };
            match doctor_toolkit::live::hot_textures(ws, None) {
                Ok(list) if list.is_empty() => {
                    self.notify(super::Level::Warning, tr("debug.no_textures").to_owned());
                }
                Ok(list) => self.play.live.confirm = Some(Cmd::Textures(list)),
                Err(e) => self.notify(super::Level::Warning, e.to_string()),
            }
        }
    }

    /// The "Debug" part of the Link panel.
    pub(super) fn live_debug_panel(&mut self, ui: &mut egui::Ui) {
        let (debug_ok, debug) = {
            let s = shared_debug(&self.play.live);
            (s.debug_ok, s.debug.clone())
        };
        if !debug_ok {
            ui.label(RichText::new(tr("debug.old_module")).weak().small());
            return;
        }
        let names = self.play.live.names_now();
        let message_name = |h: u32| {
            names
                .as_ref()
                .and_then(|n| n.message(h))
                .map_or_else(|| format!("{h:#010x}"), str::to_owned)
        };
        let object_name = |g: &proto::Guid| match names.as_ref().and_then(|n| n.object(g)) {
            Some(o) if !o.name.is_empty() => o.name.clone(),
            Some(o) => format!("{} {}", o.class, &proto::guid_hex(g)[..8]),
            None => proto::guid_hex(g)[..8].to_owned(),
        };
        let d = debug.unwrap_or_default();
        egui::CollapsingHeader::new(RichText::new(tr("debug.title")).strong())
            .id_salt("live-debug")
            .default_open(true)
            .show(ui, |ui| {
                // Where the game stands.
                match &d.stop {
                    Some(stop) => {
                        let why = match &stop.reason {
                            StopReason::Pause => tr("debug.stop_pause").to_owned(),
                            StopReason::Step => tr("debug.stop_step").to_owned(),
                            StopReason::Message {
                                hash, receivers, ..
                            } => {
                                let to: Vec<String> = receivers.iter().map(object_name).collect();
                                trf(
                                    "debug.stop_message_to",
                                    &[("message", &message_name(*hash)), ("to", &to.join(", "))],
                                )
                            }
                            StopReason::Output { text, script, line } => trf(
                                "debug.stop_print_at",
                                &[("text", text), ("script", script), ("line", line)],
                            ),
                            StopReason::LuaError { message } => {
                                trf("debug.stop_lua", &[("message", message)])
                            }
                        };
                        ui.colored_label(doctor_toolkit_mods::BAD, RichText::new(why).strong());
                        ui.label(
                            RichText::new(trf(
                                "debug.stop_where",
                                &[
                                    ("place", &crate::logic_live::place_name(stop.place)),
                                    ("frame", &stop.frame),
                                    (
                                        "seconds",
                                        &format!("{:.0}", f64::from(stop.resume_in_ms) / 1000.0),
                                    ),
                                ],
                            ))
                            .small(),
                        );
                        if !stop.place.allows_writes() {
                            ui.label(RichText::new(tr("debug.reads_only")).weak().small());
                        }
                    }
                    None if d.pending => {
                        ui.label(tr("debug.pending"));
                    }
                    None => {
                        ui.label(RichText::new(tr("debug.running")).weak());
                    }
                }
                ui.horizontal(|ui| {
                    if d.stop.is_some() {
                        if ui.button(tr("debug.continue")).clicked() {
                            self.play.live.send(Cmd::Control(Control::Continue));
                        }
                        if ui
                            .button(tr("debug.step"))
                            .on_hover_text(tr("debug.step_hint"))
                            .clicked()
                        {
                            self.play.live.send(Cmd::Control(Control::Step));
                        }
                    } else if ui
                        .button(tr("debug.pause"))
                        .on_hover_text(tr("debug.pause_hint"))
                        .clicked()
                    {
                        self.play.live.send(Cmd::Control(Control::Pause));
                    }
                    ui.label(tr("debug.hold"));
                    ui.add(
                        egui::DragValue::new(&mut self.play.live.hold_s)
                            .range(5..=proto::debug::HOLD_MAX_S)
                            .suffix(" s"),
                    )
                    .on_hover_text(tr("debug.hold_hint"));
                });
                // Breakpoints.
                ui.label(RichText::new(trf("debug.rules", &[("count", &d.rules.len())])).strong());
                let mut remove = None;
                for (i, r) in d.rules.iter().enumerate() {
                    let text = match &r.on {
                        BreakOn::Message { hash, receiver } => match receiver {
                            Some(g) => trf(
                                "debug.rule_message_to",
                                &[("message", &message_name(*hash)), ("to", &object_name(g))],
                            ),
                            None => trf("debug.rule_message", &[("message", &message_name(*hash))]),
                        },
                        BreakOn::Output { text } => trf("debug.rule_print", &[("text", text)]),
                        BreakOn::LuaError => tr("debug.rule_lua").to_owned(),
                    };
                    ui.horizontal(|ui| {
                        if ui.small_button("✕").clicked() {
                            remove = Some(i);
                        }
                        ui.label(RichText::new(text).monospace().small());
                    });
                }
                let hold = self.play.live.hold_s;
                if let Some(i) = remove {
                    let mut rules = d.rules.clone();
                    rules.remove(i);
                    self.play.live.send(Cmd::Break(rules, hold));
                }
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.play.live.break_message)
                            .hint_text(tr("debug.break_message_hint"))
                            .desired_width(150.0),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.play.live.break_target)
                            .hint_text(tr("live.target_all"))
                            .desired_width(150.0),
                    );
                    let target_text = self.play.live.break_target.trim().to_owned();
                    let target = (!target_text.is_empty()).then(|| proto::parse_guid(&target_text));
                    let ok = !self.play.live.break_message.trim().is_empty()
                        && !matches!(target, Some(None));
                    if ui
                        .add_enabled(ok, egui::Button::new(tr("debug.add_break")))
                        .clicked()
                    {
                        let mut rules = d.rules.clone();
                        rules.push(BreakRule::message(
                            MessageId::parse(&self.play.live.break_message).hash(),
                            target.flatten(),
                        ));
                        self.play.live.send(Cmd::Break(rules, hold));
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.play.live.break_print)
                            .hint_text(tr("debug.break_print_hint"))
                            .desired_width(150.0),
                    );
                    if ui
                        .add_enabled(
                            !self.play.live.break_print.trim().is_empty(),
                            egui::Button::new(tr("debug.add_break")),
                        )
                        .clicked()
                    {
                        let mut rules = d.rules.clone();
                        rules.push(BreakRule {
                            on: BreakOn::Output {
                                text: self.play.live.break_print.trim().to_owned(),
                            },
                            once: false,
                        });
                        self.play.live.send(Cmd::Break(rules, hold));
                    }
                    let lua = BreakOn::LuaError;
                    let mut on = d.rules.iter().any(|r| r.on == lua);
                    if ui.checkbox(&mut on, tr("debug.rule_lua")).changed() {
                        let mut rules: Vec<BreakRule> =
                            d.rules.iter().filter(|r| r.on != lua).cloned().collect();
                        if on {
                            rules.push(BreakRule {
                                on: lua,
                                once: false,
                            });
                        }
                        self.play.live.send(Cmd::Break(rules, hold));
                    }
                    if !d.rules.is_empty() && ui.button(tr("debug.clear_breaks")).clicked() {
                        self.play.live.send(Cmd::Break(Vec::new(), hold));
                    }
                });
                if d.camera.is_some() {
                    ui.horizontal(|ui| {
                        ui.label(tr("debug.free_camera_on"));
                        if ui.button(tr("debug.camera_release")).clicked() {
                            self.play.live.send(Cmd::Camera(None));
                        }
                    });
                }
                ui.label(
                    RichText::new(trf("debug.hits", &[("count", &d.hits)]))
                        .weak()
                        .small(),
                );
            });
        ui.separator();
    }
}
