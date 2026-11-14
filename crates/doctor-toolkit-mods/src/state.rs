//! The state of every mod in plain words for the cards (stage 64): ready,
//! in conflict with another mod, not fitting this version of the game, not
//! written into the game yet, off.

use egui::Color32;

use doctor_toolkit::mods::ModManager;
use doctor_toolkit::mods::manager::InstalledMod;

use crate::Overview;
use crate::i18n::{tr, trf};

/// Another mod that changes the same resources as this one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rival {
    /// Name of the other mod.
    pub name: String,
    /// The other mod is later in the load order: its data is used.
    pub wins: bool,
    /// Resources both change.
    pub resources: usize,
}

/// What a card says about its mod, the most important first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModState {
    /// The mod's files are damaged: it can only be removed.
    Damaged(String),
    /// Switched off.
    Off,
    /// Some of its resources or streams do not fit this game (another
    /// version or edition, or files changed by another program).
    Misfit(usize),
    /// Cannot be written together with these mods (names).
    Clash(Vec<String>),
    /// Changes the same resources as these mods; the later one is used.
    Conflict(Vec<Rival>),
    /// On, but the game does not have it yet.
    NotWritten,
    /// On and in the game.
    Ready,
}

impl ModState {
    /// The state of mod `m` of `manager`, compared with the game in `overview`.
    pub fn of(m: &InstalledMod, manager: &ModManager, overview: &Overview) -> Self {
        if let Some(problem) = &m.problem {
            return Self::Damaged(problem.clone());
        }
        if !m.enabled {
            return Self::Off;
        }
        let misfits = overview
            .issues
            .iter()
            .filter(|i| i.mod_id == m.id && !i.problem.is_notice())
            .count();
        if misfits > 0 {
            return Self::Misfit(misfits);
        }
        let name = |id: &str| {
            manager
                .get(id)
                .map_or(id, |m| m.manifest.name.as_str())
                .to_owned()
        };
        let mut clash: Vec<String> = Vec::new();
        for c in overview.clashes.iter().filter(|c| c.is_fatal()) {
            let mods = c.mods();
            if mods.contains(&m.id.as_str()) {
                for other in mods.into_iter().filter(|o| *o != m.id) {
                    let other = name(other);
                    if !clash.contains(&other) {
                        clash.push(other);
                    }
                }
            }
        }
        if !clash.is_empty() {
            return Self::Clash(clash);
        }
        let rivals = rivals(m, manager, overview);
        if !rivals.is_empty() {
            return Self::Conflict(rivals);
        }
        if !written(m, overview) {
            return Self::NotWritten;
        }
        Self::Ready
    }

    /// The words of the state.
    pub fn text(&self) -> String {
        match self {
            Self::Damaged(_) => tr("mods.state_damaged").to_owned(),
            Self::Off => tr("mods.state_off").to_owned(),
            Self::Misfit(_) => tr("mods.state_misfit").to_owned(),
            Self::Clash(names) => trf("mods.state_clash", &[("mods", &names.join(", "))]),
            Self::Conflict(rivals) => trf(
                "mods.state_conflict",
                &[(
                    "mods",
                    &rivals
                        .iter()
                        .map(|r| r.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                )],
            ),
            Self::NotWritten => tr("mods.state_not_written").to_owned(),
            Self::Ready => tr("mods.state_ready").to_owned(),
        }
    }

    /// A sentence that explains the state and what to do.
    pub fn hint(&self) -> String {
        match self {
            Self::Damaged(problem) => trf("mods.state_damaged_hint", &[("problem", problem)]),
            Self::Off => tr("mods.state_off_hint").to_owned(),
            Self::Misfit(count) => trf("mods.state_misfit_hint", &[("count", count)]),
            Self::Clash(_) => tr("mods.state_clash_hint").to_owned(),
            Self::Conflict(rivals) => rivals
                .iter()
                .map(|r| {
                    let key = if r.wins {
                        "mods.rival_wins"
                    } else {
                        "mods.rival_loses"
                    };
                    trf(key, &[("name", &r.name), ("count", &r.resources)])
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Self::NotWritten => tr("mods.state_not_written_hint").to_owned(),
            Self::Ready => tr("mods.state_ready_hint").to_owned(),
        }
    }

    pub fn color(&self) -> Color32 {
        match self {
            Self::Damaged(_) | Self::Misfit(_) | Self::Clash(_) => crate::BAD,
            Self::Conflict(_) => crate::WARN,
            Self::NotWritten => crate::theme::FOCUS,
            Self::Ready => crate::GOOD,
            Self::Off => Color32::GRAY,
        }
    }
}

/// The enabled mods that change resources of `m` with other data.
pub fn rivals(m: &InstalledMod, manager: &ModManager, overview: &Overview) -> Vec<Rival> {
    let position = |id: &str| manager.mods().iter().position(|x| x.id == id);
    let mine = position(&m.id);
    let mut out: Vec<Rival> = Vec::new();
    for conflict in overview.conflicts.iter().filter(|c| !c.same_data) {
        if !conflict.mods.contains(&m.id) {
            continue;
        }
        for other in conflict.mods.iter().filter(|o| **o != m.id) {
            let name = manager
                .get(other)
                .map_or(other.as_str(), |o| o.manifest.name.as_str());
            match out.iter_mut().find(|r| r.name == name) {
                Some(rival) => rival.resources += 1,
                None => out.push(Rival {
                    name: name.to_owned(),
                    wins: position(other) > mine,
                    resources: 1,
                }),
            }
        }
    }
    out
}

/// Whether the game has every stream of `m` as the mods want it.
fn written(m: &InstalledMod, overview: &Overview) -> bool {
    let Some(status) = &overview.status else {
        return true;
    };
    m.manifest.streams().iter().all(|name| {
        status
            .streams
            .iter()
            .filter(|s| s.stream == *name)
            .all(|s| s.up_to_date)
    })
}
