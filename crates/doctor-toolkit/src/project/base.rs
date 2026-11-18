//! The base of a project (stage 108): the mods it is made on, named in
//! `project.toml`.
//!
//! ```toml
//! format = 2
//!
//! [base]
//! exclusive = true            # only these mods are in the game
//!
//! [[base.mod]]
//! id = "new-new-york"
//! version = "^1.1.0"          # the versions the project agrees to work with
//! built_on = "1.1.0"          # the version it was made on
//! sha256 = "…"                # the manifest of that version
//! ```
//!
//! The editor reads the files of the game, so what a project is made on is
//! what the mod manager has **written into the game** while the author
//! works. Before this stage that was implied: whatever happened to be
//! there. The base names it, and with it
//!
//! - the base is taken from the game when the author says so
//!   ([`Base::record`]), never by itself;
//! - the mods of the game are compared with it ([`Base::differences`]) when
//!   the project is opened, played and installed;
//! - the mods are brought to it ([`Base::bring`]): the arrangement before
//!   is saved as a set of the mod manager, the mods of the base are enabled
//!   in its order (with `exclusive` the others are switched off) and
//!   written into the game;
//! - the mod of the project depends on the mods of the base with the
//!   conditions written here (`Project::mod_info`), whether they are
//!   written into the game at that moment or not.
//!
//! The original of an edit in the project's workspace stays the unmodded
//! game: the tie to the base is made when the mod is built or installed —
//! its dependencies, and the data of the base it replaces
//! ([`crate::mods::add_made_on`]).
//!
//! `[dependencies] mods` of stage 36 names mods without versions; it keeps
//! working as a base of which only "installed and enabled" is known
//! ([`ProjectConfig::dependency_differences`]).

use serde::{Deserialize, Serialize};

use crate::game::GameInstance;
use crate::mods::ModManager;
use crate::mods::manager::{GameMods, ModApplyOptions, SetEntry};
use crate::mods::version::Condition;
use crate::workspace::{ApplyReport, SyncProgress};
use crate::{Error, Result};

use super::config::ProjectConfig;

/// `[base]` of `project.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Base {
    /// Only the mods of the base are in the game: any other enabled mod is
    /// a difference, and bringing the mods to the base switches it off.
    /// Without it other mods may be enabled next to the base.
    #[serde(default)]
    pub exclusive: bool,
    /// The mods, in load order (`[[base.mod]]`).
    #[serde(default, rename = "mod", skip_serializing_if = "Vec::is_empty")]
    pub mods: Vec<BaseMod>,
}

/// One mod of a [`Base`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseMod {
    pub id: String,
    /// The versions of the mod the project agrees to work with, a condition
    /// of [`crate::mods::version`]; the mod of the project depends on the
    /// mod with it. `^<built_on>` when the base is recorded.
    #[serde(default = "any_version")]
    pub version: String,
    /// The version the project was made on; empty in a base written by hand.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub built_on: String,
    /// SHA-256 of the manifest of that version
    /// ([`crate::mods::ModManifest::sha256`]): another one in the game means
    /// the base changed since the project was made on it. Empty: not known.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sha256: String,
}

fn any_version() -> String {
    "*".into()
}

/// The condition a base mod recorded at `version` gets: within its major
/// version (`^1.1.0`) — an update of the base that keeps the major version
/// is taken to keep what the project was made on; the author can write
/// another. Any version (`*`) for a version that is no version number.
pub fn default_condition(version: &str) -> String {
    let version = version.trim();
    match crate::mods::version::parse_version(version) {
        Ok(_) => Condition::parse(&format!("^{version}"))
            .map_or_else(|_| any_version(), |c| c.to_string()),
        Err(_) => any_version(),
    }
}

/// What differs between the base of a project and the mods of the game
/// ([`Base::differences`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseDifference {
    /// A mod of the base is not installed.
    NotInstalled { id: String },
    /// A mod of the base is installed, but switched off or damaged.
    Disabled { id: String },
    /// A mod of the base is enabled, but the files of the game do not hold
    /// it (yet): the editor shows the game without it.
    NotWritten { id: String },
    /// The installed mod is of a version the project does not agree to.
    Version {
        id: String,
        /// The condition of the base.
        wanted: String,
        found: String,
    },
    /// The installed mod has another manifest than the one the project was
    /// made on, in a version the project agrees to: the base changed since.
    /// A warning — the edits of the project may be made on data the mod no
    /// longer has (rebasing, stage 111, starts here).
    Changed {
        id: String,
        built_on: String,
        found: String,
    },
    /// The enabled mods of the base are loaded in another order.
    Order {
        base: Vec<String>,
        game: Vec<String>,
    },
    /// With `exclusive`: a mod that is not of the base is enabled.
    Extra { id: String, written: bool },
    /// With `exclusive`: the project's own mod is written into the game, so
    /// the editor shows the game with the state of the project that was
    /// installed, not without the project.
    OwnMod { id: String },
    /// With `exclusive`: the files of the game still hold `resources`
    /// resource(s) of mods that are switched off or removed.
    Leftover { resources: usize },
}

impl BaseDifference {
    /// Whether [`Base::bring`] takes the difference away. The others need
    /// the author: a mod to install, another version of it, or the base
    /// recorded anew.
    pub fn brought(&self) -> bool {
        !matches!(
            self,
            Self::NotInstalled { .. } | Self::Version { .. } | Self::Changed { .. }
        )
    }

    /// Whether a mod of the project cannot be applied while the difference
    /// is there: a dependency that is missing, off or of another version
    /// (`crate::mods::manager::Problem`).
    pub fn blocks_the_mod(&self) -> bool {
        matches!(
            self,
            Self::NotInstalled { .. } | Self::Disabled { .. } | Self::Version { .. }
        )
    }

    /// The difference in words.
    pub fn describe(&self) -> String {
        match self {
            Self::NotInstalled { id } => format!("the base mod `{id}` is not installed"),
            Self::Disabled { id } => format!("the base mod `{id}` is switched off"),
            Self::NotWritten { id } => {
                format!("the base mod `{id}` is enabled but not written into the game")
            }
            Self::Version { id, wanted, found } => format!(
                "the base mod `{id}` is of version {found}; the project works with {wanted}"
            ),
            Self::Changed {
                id,
                built_on,
                found,
            } => format!(
                "the base mod `{id}` changed since the project was made on it (made on \
                 {}, the game has another build of {found})",
                if built_on.is_empty() {
                    "another build"
                } else {
                    built_on
                }
            ),
            Self::Order { base, game } => format!(
                "the base mods are loaded in the order {}; the base has {}",
                game.join(", "),
                base.join(", ")
            ),
            Self::Extra { id, written: true } => {
                format!("the mod `{id}` is in the game but not in the base")
            }
            Self::Extra { id, written: false } => {
                format!("the mod `{id}` is enabled but not in the base")
            }
            Self::OwnMod { id } => format!(
                "the project's own mod `{id}` is written into the game: the editor shows the \
                 game with its installed state"
            ),
            Self::Leftover { resources } => format!(
                "the game files still hold {resources} resource(s) of mods that are switched \
                 off or removed"
            ),
        }
    }
}

/// What [`Base::record`] took from the game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub base: Base,
    /// Enabled mods the files of the game do not hold: they are left out of
    /// the base, because the editor does not show the game with them.
    pub not_written: Vec<String>,
}

/// What [`Base::bring`] did.
#[derive(Debug, Clone)]
pub struct Brought {
    /// The set of the mod manager that holds the arrangement before, to go
    /// back to (use the set, then write the mods); `None` when the
    /// arrangement did not change — the mods were only written.
    pub set: Option<String>,
    /// The set was saved now; else the arrangement before was a saved set
    /// already.
    pub saved: bool,
    pub report: ApplyReport,
}

impl Base {
    /// The mod `id` of the base.
    pub fn get(&self, id: &str) -> Option<&BaseMod> {
        self.mods.iter().find(|m| m.id == id)
    }

    /// Checks the base of the project `own_id`: ids, conditions and hashes
    /// as they must be, no mod twice, not the project itself.
    pub fn validate(&self, own_id: &str) -> Result<()> {
        let bad = |why: String| Err(Error::InvalidArgument(format!("[base]: {why}")));
        let mut seen = std::collections::BTreeSet::new();
        for m in &self.mods {
            if crate::mods::validate_id(&m.id).is_err() {
                return bad(format!("`{}` is not a mod id", m.id.escape_debug()));
            }
            if m.id == own_id {
                return bad(format!("the project `{own_id}` cannot be its own base"));
            }
            if !seen.insert(m.id.as_str()) {
                return bad(format!("the mod `{}` is listed twice", m.id));
            }
            if let Err(error) = Condition::parse(&m.version) {
                return bad(format!("mod `{}`: {}", m.id, super::plain(&error)));
            }
            if !m.sha256.is_empty() && !crate::mods::is_sha256(&m.sha256) {
                return bad(format!("mod `{}`: `sha256` is not a SHA-256", m.id));
            }
            if m.built_on.len() > 64 || m.built_on.chars().any(char::is_control) {
                return bad(format!("mod `{}`: `built_on` is not a version", m.id));
            }
        }
        Ok(())
    }

    /// The base the game has now: the enabled mods the mod manager has
    /// written into it, in load order, each with its version and the hash
    /// of its manifest, without the project's own mod `own_id`. A mod of
    /// `previous` (the base recorded before) keeps the condition its author
    /// wrote when the version in the game still satisfies it, and
    /// `exclusive` stays as it was; a base recorded first is exclusive: the
    /// game holds these mods and no others.
    pub fn record(game: &GameMods, own_id: &str, previous: Option<&Base>) -> Recorded {
        let mods = game
            .written()
            .filter(|m| m.id != own_id)
            .map(|m| {
                let kept = previous
                    .and_then(|b| b.get(&m.id))
                    .filter(|b| {
                        Condition::parse(&b.version).is_ok_and(|c| c.matches_text(&m.version))
                    })
                    .map(|b| b.version.clone());
                BaseMod {
                    id: m.id.clone(),
                    version: kept.unwrap_or_else(|| default_condition(&m.version)),
                    built_on: m.version.clone(),
                    sha256: m.sha256.clone(),
                }
            })
            .collect();
        Recorded {
            base: Base {
                exclusive: previous.is_none_or(|b| b.exclusive),
                mods,
            },
            not_written: game
                .mods
                .iter()
                .filter(|m| m.enabled && !m.written && m.id != own_id)
                .map(|m| m.id.clone())
                .collect(),
        }
    }

    /// What differs between the base and the mods of the game, for the
    /// project `own_id`; empty when the game holds the base.
    pub fn differences(&self, game: &GameMods, own_id: &str) -> Vec<BaseDifference> {
        let mut out = Vec::new();
        for wanted in &self.mods {
            let id = wanted.id.clone();
            let Some(m) = game.get(&wanted.id) else {
                out.push(BaseDifference::NotInstalled { id });
                continue;
            };
            if !m.enabled {
                out.push(BaseDifference::Disabled { id: id.clone() });
            } else if !m.written {
                out.push(BaseDifference::NotWritten { id: id.clone() });
            }
            if m.damaged {
                // Its manifest cannot be read: nothing more is known.
                continue;
            }
            let fits = Condition::parse(&wanted.version).is_ok_and(|c| c.matches_text(&m.version));
            if !fits {
                out.push(BaseDifference::Version {
                    id,
                    wanted: wanted.version.clone(),
                    found: m.version.clone(),
                });
            } else if !wanted.sha256.is_empty() && wanted.sha256 != m.sha256 {
                out.push(BaseDifference::Changed {
                    id,
                    built_on: wanted.built_on.clone(),
                    found: m.version.clone(),
                });
            }
        }
        // The order of the mods of the base that are enabled, against theirs
        // in the base.
        let enabled = |id: &str| game.get(id).is_some_and(|m| m.enabled);
        let in_base: Vec<String> = self
            .mods
            .iter()
            .filter(|m| enabled(&m.id))
            .map(|m| m.id.clone())
            .collect();
        let in_game: Vec<String> = game
            .mods
            .iter()
            .filter(|m| m.enabled && self.get(&m.id).is_some())
            .map(|m| m.id.clone())
            .collect();
        if in_base != in_game {
            out.push(BaseDifference::Order {
                base: in_base,
                game: in_game,
            });
        }
        if self.exclusive {
            for m in game.mods.iter().filter(|m| m.enabled) {
                if self.get(&m.id).is_some() {
                    continue;
                }
                if m.id != own_id {
                    out.push(BaseDifference::Extra {
                        id: m.id.clone(),
                        written: m.written,
                    });
                } else if m.written {
                    // Enabled only, it is replaced by the project as it is
                    // now for every launch and is in no file the editor reads.
                    out.push(BaseDifference::OwnMod { id: m.id.clone() });
                }
            }
            if game.leftover > 0 {
                out.push(BaseDifference::Leftover {
                    resources: game.leftover,
                });
            }
        }
        out
    }

    /// The arrangement of the installed mods `now` with the base in it: the
    /// mods of the base enabled and in its order. With `exclusive` they go
    /// first and every other mod is switched off; without, the other mods
    /// stay as and where they are, and the mods of the base take the places
    /// the mods of the base had.
    pub fn arrangement(&self, now: &[SetEntry]) -> Vec<SetEntry> {
        let installed = |id: &str| now.iter().any(|e| e.id == id);
        let mut base = self
            .mods
            .iter()
            .filter(|m| installed(&m.id))
            .map(|m| SetEntry {
                id: m.id.clone(),
                enabled: true,
            });
        if self.exclusive {
            let rest = now
                .iter()
                .filter(|e| self.get(&e.id).is_none())
                .map(|e| SetEntry {
                    id: e.id.clone(),
                    enabled: false,
                });
            return base.chain(rest).collect();
        }
        now.iter()
            .map(|e| match self.get(&e.id) {
                Some(_) => base.next().expect("as many places as mods of the base"),
                None => e.clone(),
            })
            .collect()
    }

    /// Brings the mods of `game` to the base: saves the arrangement they
    /// have as a set of the mod manager named after `project` (unless it is
    /// a saved set already), enables the mods of the base in its order —
    /// with `exclusive` the others are switched off — and writes the mods
    /// into the game. Going back is using that set and writing the mods.
    ///
    /// Refused, with nothing changed, when a mod of the base is not
    /// installed or is damaged: the mod manager cannot bring a mod it does
    /// not have. When the write is refused or fails, the arrangement before
    /// comes back and the set saved for it is removed.
    pub fn bring(
        &self,
        game: &GameInstance,
        manager: &mut ModManager,
        project: &str,
        options: ModApplyOptions,
        progress: &mut dyn FnMut(SyncProgress<'_>),
    ) -> Result<Brought> {
        manager.reload_if_changed()?;
        let unusable: Vec<String> = self
            .mods
            .iter()
            .filter(|m| manager.get(&m.id).is_none_or(|i| i.problem.is_some()))
            .map(|m| match (manager.get(&m.id), m.built_on.is_empty()) {
                (Some(_), _) => format!("`{}` is damaged", m.id),
                (None, true) => format!("`{}` is not installed", m.id),
                (None, false) => format!("`{}` {} is not installed", m.id, m.built_on),
            })
            .collect();
        if !unusable.is_empty() {
            return Err(Error::mod_error(format!(
                "the mods cannot be brought to the base of the project: {}; install {} first",
                unusable.join(", "),
                if unusable.len() == 1 { "it" } else { "them" },
            )));
        }
        let before = manager.arrangement();
        let wanted = self.arrangement(&before);
        let (set, saved) = if wanted == before {
            (None, false)
        } else {
            match manager.current_set() {
                Some(name) => (Some(name), false),
                None => {
                    let name = manager.free_set_name(&format!("Before {project}"))?;
                    manager.save_set(&name)?;
                    (Some(name), true)
                }
            }
        };
        match manager.arrange_and_apply(game, &wanted, options, progress) {
            Ok((_, report)) => Ok(Brought { set, saved, report }),
            Err(error) => {
                if let (Some(name), true) = (&set, saved) {
                    let _ = manager.delete_set(name);
                }
                Err(error)
            }
        }
    }
}

impl ProjectConfig {
    /// The mods the mod of the project depends on, each with the condition
    /// on its version: the mods of the base with theirs, and the mods of
    /// `[dependencies] mods` that the base does not name, of any version.
    pub fn mod_dependencies(&self) -> std::collections::BTreeMap<String, String> {
        let mut out: std::collections::BTreeMap<String, String> = self
            .dependencies
            .mods
            .iter()
            .map(|id| (id.clone(), any_version()))
            .collect();
        for m in self.base.iter().flat_map(|b| &b.mods) {
            out.insert(m.id.clone(), m.version.clone());
        }
        out
    }

    /// What differs between what the project is made on and the mods of the
    /// game: the base ([`Base::differences`]), and the mods of
    /// `[dependencies] mods` the base does not name, of which only
    /// "installed and enabled" is asked — a project of stage 36 is built
    /// into the workspace of the game, which is not written into it
    /// together with mods.
    pub fn base_differences(&self, game: &GameMods) -> Vec<BaseDifference> {
        let mut out = self
            .base
            .as_ref()
            .map(|b| b.differences(game, &self.project.id))
            .unwrap_or_default();
        out.extend(self.dependency_differences(game));
        out
    }

    /// [`Self::base_differences`] of the mods of `[dependencies] mods` only.
    pub fn dependency_differences(&self, game: &GameMods) -> Vec<BaseDifference> {
        self.dependencies
            .mods
            .iter()
            .filter(|id| self.base.as_ref().is_none_or(|b| b.get(id).is_none()))
            .filter_map(|id| match game.get(id) {
                None => Some(BaseDifference::NotInstalled { id: id.clone() }),
                Some(m) if !m.enabled => Some(BaseDifference::Disabled { id: id.clone() }),
                Some(_) => None,
            })
            .collect()
    }

    /// [`Self::base_differences`] against the mods of `game` as its mod
    /// manager and its files have them now.
    pub fn check_base(&self, game: &GameInstance) -> Result<Vec<BaseDifference>> {
        if self.base.is_none() && self.dependencies.mods.is_empty() {
            return Ok(Vec::new());
        }
        let in_game = ModManager::open(game)?.in_game(game)?;
        Ok(self.base_differences(&in_game))
    }

    /// Takes the base of the project in `root` from the mods written into
    /// `game` ([`Base::record`]) and writes it into its `project.toml`
    /// ([`Self::write_base`]).
    pub fn record_base(root: &std::path::Path, game: &GameInstance) -> Result<Recorded> {
        let config = Self::read(root)?;
        let in_game = ModManager::open(game)?.in_game(game)?;
        let recorded = Base::record(&in_game, &config.project.id, config.base.as_ref());
        Self::write_base(root, Some(&recorded.base))?;
        Ok(recorded)
    }
}

#[cfg(test)]
mod tests;
