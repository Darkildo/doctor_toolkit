//! "World and state" of the Link panel (stage 48): the time of day and how
//! fast it goes, the weather, streams loaded one at a time, the player's
//! money, crew and mission, the variables of the story. Every change asks
//! first (the confirmation window of the panel).

use doctor_toolkit::live::proto::{self, MoneyChange, StreamAction, TimeChange, state};
use doctor_toolkit::live::{self, Client};
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::RichText;

use super::BrowserApp;
use super::live::{Cmd, shared_debug};

/// What the panel last read.
#[derive(Debug, Clone, Default)]
pub(crate) struct WorldState {
    pub time: Option<proto::TimeOfDay>,
    pub weather: Option<proto::Weather>,
    pub money: Option<proto::Money>,
    pub family: Option<proto::Family>,
    pub mission: Option<proto::Mission>,
    pub streams: Option<proto::StreamQueue>,
    pub variables: Vec<proto::Variable>,
}

impl WorldState {
    /// New values of variables replace those read before.
    pub(crate) fn merge_variables(&mut self, now: Vec<proto::Variable>) {
        for v in now {
            match self.variables.iter_mut().find(|x| x.hash == v.hash) {
                Some(x) => *x = v,
                None => self.variables.push(v),
            }
        }
    }
}

/// Reads all of it (on the link's thread).
pub(super) fn read(client: &mut Client, vars: Vec<u32>) -> Result<WorldState, live::LinkError> {
    Ok(WorldState {
        time: client.time_of_day()?,
        weather: Some(client.weather()?),
        money: client.money()?,
        family: Some(client.family()?),
        mission: Some(client.mission()?),
        streams: Some(client.stream_queue()?),
        variables: if vars.is_empty() {
            Vec::new()
        } else {
            client.variables(vars)?
        },
    })
}

/// The question before a change of the clock.
pub(super) fn confirm_time(c: &TimeChange) -> String {
    let time = c
        .time
        .map(|[h, m, s]| format!("{h:02}:{m:02}:{s:02}"))
        .unwrap_or_else(|| tr("world.keep").to_owned());
    let rate = match c.rate {
        Some(r) if r < 0.0 => tr("world.level_rate").to_owned(),
        Some(r) => r.to_string(),
        None => tr("world.keep").to_owned(),
    };
    trf("world.confirm_time", &[("time", &time), ("rate", &rate)])
}

/// Fields of the section.
#[derive(Debug, Clone)]
pub(super) struct WorldFields {
    time: String,
    rate: String,
    money: String,
    stream: String,
    force: bool,
    variable: String,
    value: String,
}

impl Default for WorldFields {
    fn default() -> Self {
        Self {
            time: "22:00".into(),
            rate: "60".into(),
            money: "1000".into(),
            stream: String::new(),
            force: false,
            variable: String::new(),
            value: "1".into(),
        }
    }
}

fn hash_of(text: &str) -> u32 {
    match proto::MessageId::parse(text) {
        proto::MessageId::Hash(h) => h,
        proto::MessageId::Name(n) => proto::name_hash(&n),
    }
}

/// `HH:MM` or `HH:MM:SS`.
pub(crate) fn parse_time(text: &str) -> Option<[u8; 3]> {
    let parts: Vec<u8> = text
        .trim()
        .split(':')
        .map(|p| p.parse::<u8>().ok())
        .collect::<Option<_>>()?;
    let t = match parts.as_slice() {
        [h, m] => [*h, *m, 0],
        [h, m, s] => [*h, *m, *s],
        _ => return None,
    };
    (t[0] < 24 && t[1] < 60 && t[2] < 60).then_some(t)
}

impl BrowserApp {
    /// The "World and state" part of the Link panel.
    pub(super) fn live_state_panel(&mut self, ui: &mut egui::Ui, writes: bool, busy: bool) {
        let (state_ok, w) = {
            let s = shared_debug(&self.play.live);
            (s.state_ok, s.world_state.clone())
        };
        if !state_ok {
            ui.label(RichText::new(tr("world.old_module")).weak().small());
            return;
        }
        let names = self.play.live.names_now();
        let mut fields = std::mem::take(&mut self.play.live.world);
        let mut ask: Option<Cmd> = None;
        let mut read: Option<Cmd> = None;
        egui::CollapsingHeader::new(RichText::new(tr("world.title")).strong())
            .id_salt("live-world")
            .default_open(false)
            .show(ui, |ui| {
                let f = &mut fields;
                let var_hashes: Vec<u32> = w.variables.iter().map(|v| v.name_hash).collect();
                if ui
                    .add_enabled(!busy, egui::Button::new(tr("world.read")))
                    .on_hover_text(tr("world.read_hint"))
                    .clicked()
                {
                    let mut want = var_hashes.clone();
                    if !f.variable.trim().is_empty() {
                        want.push(hash_of(&f.variable));
                    }
                    read = Some(Cmd::State(want));
                }

                // Time of day.
                ui.label(RichText::new(tr("world.time")).strong());
                match &w.time {
                    Some(t) => ui.label(trf(
                        "world.time_now",
                        &[
                            (
                                "time",
                                &format!("{:02}:{:02}:{:02}", t.hour, t.minute, t.second),
                            ),
                            ("rate", &t.rate),
                            ("level", &t.default_rate),
                        ],
                    )),
                    None => ui.label(RichText::new(tr("world.unknown")).weak()),
                };
                ui.add_enabled_ui(writes && !busy, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.time).desired_width(70.0));
                        let t = parse_time(&f.time);
                        if ui
                            .add_enabled(t.is_some(), egui::Button::new(tr("world.set_time")))
                            .clicked()
                        {
                            ask = Some(Cmd::SetTime(TimeChange {
                                time: t,
                                rate: None,
                            }));
                        }
                        ui.label(tr("world.rate"));
                        ui.add(egui::TextEdit::singleline(&mut f.rate).desired_width(50.0));
                        let r = f.rate.trim().parse::<f32>().ok().filter(|r| *r >= 0.0);
                        if ui
                            .add_enabled(r.is_some(), egui::Button::new(tr("world.set_rate")))
                            .on_hover_text(tr("world.rate_hint"))
                            .clicked()
                        {
                            ask = Some(Cmd::SetTime(TimeChange {
                                time: None,
                                rate: r,
                            }));
                        }
                        if ui.button(tr("world.level_rate")).clicked() {
                            ask = Some(Cmd::SetTime(TimeChange {
                                time: None,
                                rate: Some(-1.0),
                            }));
                        }
                    });
                });
                // Weather.
                if let Some(weather) = &w.weather {
                    ui.label(match weather.rain {
                        Some(r) if r.flags & 1 != 0 => tr("world.rain_on").to_owned(),
                        Some(_) => tr("world.rain_off").to_owned(),
                        None => tr("world.no_rain").to_owned(),
                    })
                    .on_hover_text(tr("world.sky_hint"));
                }
                ui.separator();

                // Money and crew.
                ui.label(RichText::new(tr("world.money")).strong());
                match &w.money {
                    Some(m) => ui.label(format!("${}", m.cash)),
                    None => ui.label(RichText::new(tr("world.unknown")).weak()),
                };
                ui.add_enabled_ui(writes && !busy, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.money).desired_width(90.0));
                        let v = f.money.trim().parse::<f32>().ok().filter(|v| v.is_finite());
                        if ui
                            .add_enabled(v.is_some(), egui::Button::new(tr("world.set_money")))
                            .clicked()
                        {
                            ask = v.map(|v| Cmd::SetMoney(MoneyChange::Set(v)));
                        }
                        if ui
                            .add_enabled(v.is_some(), egui::Button::new(tr("world.add_money")))
                            .clicked()
                        {
                            ask = v.map(|v| Cmd::SetMoney(MoneyChange::Add(v)));
                        }
                    });
                });
                if let Some(fam) = &w.family {
                    ui.label(RichText::new(tr("world.crew")).strong());
                    for s in &fam.crew {
                        let who = match &s.member {
                            Some(g) => {
                                let name = names
                                    .as_ref()
                                    .and_then(|n| n.object(g))
                                    .map(|o| o.name.clone())
                                    .filter(|n| !n.is_empty())
                                    .unwrap_or_else(|| proto::guid_hex(g)[..8].to_owned());
                                let sp = state::specialty_names(s.specialties).join(", ");
                                format!("{name} — {} {sp}", state::rank_name(s.member_rank))
                            }
                            None if s.open() => tr("world.slot_free").to_owned(),
                            None => tr("world.slot_locked").to_owned(),
                        };
                        ui.label(
                            RichText::new(format!(
                                "{} {}: {who}",
                                s.slot + 1,
                                state::rank_name(s.rank)
                            ))
                            .small(),
                        );
                    }
                }
                if let Some(m) = &w.mission {
                    ui.label(match &m.current {
                        Some(c) => trf(
                            "world.checkpoint",
                            &[("number", &c.number), ("chapter", &c.chapter)],
                        ),
                        None => tr("world.no_checkpoint").to_owned(),
                    })
                    .on_hover_text(trf(
                        "world.checkpoints_known",
                        &[("count", &m.checkpoints.len())],
                    ));
                }
                ui.separator();

                // Streams.
                ui.label(RichText::new(tr("world.streams")).strong());
                ui.add_enabled_ui(writes && !busy, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut f.stream)
                                .hint_text("gf2/cuba/local/yuri/high/filler_056_01")
                                .desired_width(240.0),
                        );
                        let ok = !f.stream.trim().is_empty();
                        if ui
                            .add_enabled(ok, egui::Button::new(tr("world.load")))
                            .on_hover_text(tr("world.load_hint"))
                            .clicked()
                        {
                            ask = Some(Cmd::Stream(
                                f.stream.trim().to_owned(),
                                StreamAction::Load,
                                false,
                            ));
                        }
                        if ui
                            .add_enabled(ok, egui::Button::new(tr("world.unload")))
                            .clicked()
                        {
                            ask = Some(Cmd::Stream(
                                f.stream.trim().to_owned(),
                                StreamAction::Unload,
                                f.force,
                            ));
                        }
                        ui.checkbox(&mut f.force, tr("world.force"))
                            .on_hover_text(tr("world.force_hint"));
                    });
                });
                if let Some(q) = &w.streams {
                    if q.busy {
                        ui.label(RichText::new(tr("world.streamer_busy")).weak().small());
                    }
                    for r in q.requests.iter().rev().take(6) {
                        let what = match r.action {
                            StreamAction::Load => tr("world.load"),
                            StreamAction::Unload => tr("world.unload"),
                        };
                        let how = if r.done {
                            tr("world.done")
                        } else if r.expired {
                            tr("world.expired")
                        } else {
                            tr("world.in_progress")
                        };
                        ui.label(RichText::new(format!("{what} {} — {how}", r.name)).small());
                    }
                }
                ui.separator();

                // Variables.
                ui.label(RichText::new(tr("world.variables")).strong());
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut f.variable)
                            .hint_text(tr("world.variable_hint"))
                            .desired_width(200.0),
                    );
                    let ok = !f.variable.trim().is_empty();
                    if ui
                        .add_enabled(ok && !busy, egui::Button::new(tr("world.read")))
                        .clicked()
                    {
                        let mut want = var_hashes.clone();
                        want.push(hash_of(&f.variable));
                        read = Some(Cmd::State(want));
                    }
                    ui.add(egui::TextEdit::singleline(&mut f.value).desired_width(50.0));
                    let v = f.value.trim().parse::<i32>().ok();
                    if ui
                        .add_enabled(
                            writes && !busy && ok && v.is_some(),
                            egui::Button::new(tr("world.set_variable")),
                        )
                        .clicked()
                    {
                        ask = v.map(|v| {
                            Cmd::SetVariable(hash_of(&f.variable), v, f.variable.trim().to_owned())
                        });
                    }
                });
                for v in &w.variables {
                    let name = names
                        .as_ref()
                        .and_then(|n| n.variable(v.name_hash))
                        .map(|x| x.name.clone())
                        .unwrap_or_else(|| format!("{:#010x}", v.name_hash));
                    ui.label(
                        RichText::new(format!("{name} = {}", v.value))
                            .monospace()
                            .small(),
                    );
                }
            });
        self.play.live.world = fields;
        if let Some(cmd) = read {
            self.play.live.send(cmd);
        }
        if let Some(cmd) = ask {
            self.play.live.confirm = Some(cmd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_of_the_fields() {
        assert_eq!(parse_time("22:00"), Some([22, 0, 0]));
        assert_eq!(parse_time(" 6:05:09 "), Some([6, 5, 9]));
        assert_eq!(parse_time("24:00"), None);
        assert_eq!(parse_time("x"), None);
        assert_eq!(hash_of("0x10"), 0x10);
        let mut w = WorldState::default();
        let v = |hash, value| proto::Variable {
            hash,
            name_hash: hash + 1,
            value,
            kind: 0,
        };
        w.merge_variables(vec![v(1, 0), v(2, 0)]);
        w.merge_variables(vec![v(1, 5)]);
        assert_eq!(w.variables.len(), 2);
        assert_eq!(w.variables[0].value, 5);
    }
}
