//! The state of every mod in plain words for the cards (stage 64): ready,
//! in conflict with another mod, not fitting this version of the game, not
//! written into the game yet, off. Stage 71 adds the states of format
//! version 3: another mod needed, a newer mod manager needed, another
//! edition of the game, a mod it does not work with.

use egui::Color32;

use doctor_toolkit::mods::ModManager;
use doctor_toolkit::mods::manager::{Clash, InstalledMod, Problem, Subject};
use doctor_toolkit::mods::native::{NativeBlock, NativeState};
use doctor_toolkit::mods::version::Condition;

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

/// Why a mod another one needs is not there as needed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NeedKind {
    /// Not installed.
    Missing,
    /// Installed, switched off.
    Off,
    /// Installed in this version, which is not one the mod allows.
    Version(String),
    /// Loaded after the mod that needs it.
    Later,
}

/// A mod another one needs (`depends`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Need {
    /// Name of the needed mod, or its id when it is not installed.
    pub name: String,
    /// Condition on its version, `*` for any.
    pub versions: String,
    pub kind: NeedKind,
}

impl Need {
    /// `New York ≥1.1` or `New York`.
    pub fn label(&self) -> String {
        match Condition::parse(&self.versions) {
            Ok(c) if !c.is_any() => format!("{} {}", self.name, versions_text(&c)),
            _ => self.name.clone(),
        }
    }

    /// A sentence: what is wrong and what to do.
    fn hint(&self) -> String {
        let need = self.label();
        match &self.kind {
            NeedKind::Missing => trf("mods.need_missing", &[("need", &need)]),
            NeedKind::Off => trf("mods.need_off", &[("need", &need)]),
            NeedKind::Version(found) => {
                trf("mods.need_version", &[("need", &need), ("found", found)])
            }
            NeedKind::Later => trf("mods.need_later", &[("name", &self.name)]),
        }
    }
}

/// A condition for people: `>=1.1 <2` as `≥1.1 <2`.
pub fn versions_text(condition: &Condition) -> String {
    condition.to_string().replace(">=", "≥").replace("<=", "≤")
}

/// What a card says about its mod, the most important first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModState {
    /// The mod's files are damaged: it can only be removed.
    Damaged(String),
    /// Switched off.
    Off,
    /// Needs a newer mod manager: the condition.
    NeedsUpdate(String),
    /// Carries executable code nobody answered about yet: nothing
    /// executable is installed; `required` — and nothing else of the mod.
    CodeWaiting { required: bool },
    /// Does not work without its code, and the code cannot be installed.
    CodeBlocked(NativeBlock),
    /// Needs other mods that are not there as needed.
    NeedsMods(Vec<Need>),
    /// Not tested on this edition of the game: the editions it is tested
    /// on and this game's, if known.
    WrongEdition {
        tested: Vec<String>,
        game: Option<String>,
    },
    /// Some of its resources or streams do not fit this game (another
    /// version or edition, or files changed by another program).
    Misfit(usize),
    /// Does not work with these enabled mods: (name, reason of the author).
    Incompatible(Vec<(String, String)>),
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
        let name = |id: &str| {
            manager
                .get(id)
                .map_or(id, |m| m.manifest.name.as_str())
                .to_owned()
        };
        let mine = || overview.issues.iter().filter(|i| i.mod_id == m.id);
        if let Some(required) = mine().find_map(|i| match &i.problem {
            Problem::ManagerTooOld { required } => Some(required.clone()),
            _ => None,
        }) {
            return Self::NeedsUpdate(required);
        }
        if let Some((block, required)) = mine().find_map(|i| match &i.problem {
            Problem::Native { block, required } => Some((block.clone(), *required)),
            _ => None,
        }) {
            if block == NativeBlock::Unasked {
                return Self::CodeWaiting { required };
            }
            if i_is_fatal(&block, required) {
                return Self::CodeBlocked(block);
            }
        }
        let needs: Vec<Need> = mine()
            .filter_map(|i| {
                let (id, versions, kind) = match &i.problem {
                    Problem::DependencyMissing { id, versions } => {
                        (id, versions.clone(), NeedKind::Missing)
                    }
                    Problem::DependencyDisabled { id, versions } => {
                        (id, versions.clone(), NeedKind::Off)
                    }
                    Problem::DependencyVersion {
                        id,
                        versions,
                        found,
                    } => (id, versions.clone(), NeedKind::Version(found.clone())),
                    Problem::DependencyLater { id } => (id, "*".to_owned(), NeedKind::Later),
                    _ => return None,
                };
                Some(Need {
                    name: name(id),
                    versions,
                    kind,
                })
            })
            .collect();
        if !needs.is_empty() {
            return Self::NeedsMods(needs);
        }
        if let Some((tested, game)) = mine().find_map(|i| match &i.problem {
            Problem::Edition { game, tested } => Some((tested.clone(), game.clone())),
            _ => None,
        }) {
            return Self::WrongEdition { tested, game };
        }
        let misfits = mine()
            .filter(|i| !i.problem.is_notice() && i.subject != Subject::Mod)
            .count();
        if misfits > 0 {
            return Self::Misfit(misfits);
        }
        let mut incompatible: Vec<(String, String)> = Vec::new();
        for c in &overview.clashes {
            if let Clash::Incompatible {
                declared_by,
                other,
                reason,
                ..
            } = c
            {
                let them = if *declared_by == m.id {
                    other
                } else if *other == m.id {
                    declared_by
                } else {
                    continue;
                };
                incompatible.push((name(them), reason.clone()));
            }
        }
        if !incompatible.is_empty() {
            return Self::Incompatible(incompatible);
        }
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
        let code_waits = overview
            .code
            .iter()
            .any(|c| c.question.id == m.id && c.state == Some(NativeState::NotWritten));
        if !written(m, overview) || code_waits {
            return Self::NotWritten;
        }
        Self::Ready
    }

    /// The words of the state.
    pub fn text(&self) -> String {
        match self {
            Self::Damaged(_) => tr("mods.state_damaged").to_owned(),
            Self::Off => tr("mods.state_off").to_owned(),
            Self::NeedsUpdate(_) => tr("mods.state_needs_update").to_owned(),
            Self::CodeWaiting { .. } => tr("mods.state_code_waiting").to_owned(),
            Self::CodeBlocked(_) => tr("mods.state_code_blocked").to_owned(),
            Self::NeedsMods(needs) => {
                let first = &needs[0];
                let mut text = match &first.kind {
                    NeedKind::Version(_) => {
                        trf("mods.state_needs_version", &[("need", &first.label())])
                    }
                    NeedKind::Later => trf("mods.state_needs_order", &[("name", &first.name)]),
                    _ => trf("mods.state_needs_mod", &[("need", &first.label())]),
                };
                if needs.len() > 1 {
                    text += &trf("mods.state_needs_more", &[("count", &(needs.len() - 1))]);
                }
                text
            }
            Self::WrongEdition { .. } | Self::Misfit(_) => tr("mods.state_misfit").to_owned(),
            Self::Incompatible(list) => trf(
                "mods.state_incompatible",
                &[(
                    "mods",
                    &list
                        .iter()
                        .map(|(n, _)| n.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                )],
            ),
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
            Self::NeedsUpdate(required) => trf(
                "mods.state_needs_update_hint",
                &[
                    ("required", required),
                    ("version", &doctor_toolkit::mods::version::manager_version()),
                ],
            ),
            Self::CodeWaiting { required: true } => {
                tr("mods.state_code_waiting_required_hint").to_owned()
            }
            Self::CodeWaiting { required: false } => tr("mods.state_code_waiting_hint").to_owned(),
            Self::CodeBlocked(block) => trf(
                "mods.state_code_blocked_hint",
                &[("why", &crate::native::block_words(block))],
            ),
            Self::NeedsMods(needs) => needs.iter().map(Need::hint).collect::<Vec<_>>().join("\n"),
            Self::WrongEdition { tested, game } => trf(
                "mods.state_edition_hint",
                &[
                    ("tested", &tested.join(", ")),
                    (
                        "game",
                        &game
                            .clone()
                            .unwrap_or_else(|| tr("mods.edition_unknown").to_owned()),
                    ),
                ],
            ),
            Self::Misfit(count) => trf("mods.state_misfit_hint", &[("count", count)]),
            Self::Incompatible(list) => {
                let mut lines: Vec<String> = list
                    .iter()
                    .filter(|(_, reason)| !reason.is_empty())
                    .map(|(name, reason)| format!("{name}: {reason}"))
                    .collect();
                lines.push(tr("mods.state_incompatible_hint").to_owned());
                lines.join("\n")
            }
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
            Self::Damaged(_)
            | Self::Misfit(_)
            | Self::Clash(_)
            | Self::NeedsUpdate(_)
            | Self::NeedsMods(_)
            | Self::CodeBlocked(_)
            | Self::WrongEdition { .. }
            | Self::Incompatible(_) => crate::BAD,
            Self::Conflict(_) | Self::CodeWaiting { .. } => crate::WARN,
            Self::NotWritten => crate::theme::FOCUS,
            Self::Ready => crate::GOOD,
            Self::Off => Color32::GRAY,
        }
    }
}

/// Whether a mod whose code is blocked like this cannot be written at all.
fn i_is_fatal(block: &NativeBlock, required: bool) -> bool {
    doctor_toolkit::mods::native::is_fatal(block, required)
}

/// The enabled mods that change resources of `m` with other data.
pub fn rivals(m: &InstalledMod, manager: &ModManager, overview: &Overview) -> Vec<Rival> {
    let position = |id: &str| manager.mods().iter().position(|x| x.id == id);
    let mine = position(&m.id);
    let mut out: Vec<Rival> = Vec::new();
    // A patch on top of other mods wins on purpose: not a conflict to warn of.
    for conflict in overview
        .conflicts
        .iter()
        .filter(|c| !c.same_data && !c.patch)
    {
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
