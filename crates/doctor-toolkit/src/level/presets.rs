//! Presets of zones (stage 113): what to load to see a level, computed from
//! the zones of the game and never from a hand-written list.
//!
//! A city is hundreds of streams, and they are not of one kind: the world at
//! three levels of detail, interiors, and the streams the logic of the game
//! turns on (missions, hits, favours, compounds, checkpoints). Loading all
//! of them at once shows three copies of every district and both states of
//! every mission. A preset names the part that makes one picture:
//!
//! - **the whole level, near detail** ([`PresetKind::Level`]): the world as
//!   the game shows it around the player, with its interiors;
//! - **a district** ([`PresetKind::District`], `<level>/local/<name>`): the
//!   same of one district;
//! - **missions** and **checkpoints** ([`PresetKind::Missions`],
//!   [`PresetKind::Checkpoints`]): the streams the logic turns on, apart.
//!
//! What a zone is ([`Role`]) comes from the **world grid** of its level
//! (`docs/formats/wgi.md`): a world cell with the `high` bit is near
//! detail, with `medium`, `low` or `map` a coarser copy, with `interior` an
//! interior; a cell of another kind is turned on by a `ZoneStreamEnable`
//! object. The names of the folders (`…/high/…`, `…/medium/…`) say nearly
//! the same and are used only where the grid has no cell: they call the hit
//! and compound streams that lie in `high` folders near detail, call the
//! favour streams that lie in `medium` folders far detail, and are not kept
//! by the multiplayer maps (`terrain/zone01_high`, `sky_high`).
//!
//! [`ZoneSet`] is a set saved under a name by the user (a project's
//! `[[zones]]`, the editor's settings). [`resolve`] turns the words a person
//! types — zones, folders, `preset:<id>`, `set:<name>` — into streams, and
//! [`resolve_saved`] the entries of a saved choice.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use super::catalog::{Catalog, Zone};
use crate::game::GameInstance;
use crate::resource::wgi::{self, WorldGrid};

/// What the world grids of a level say about one stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridCell {
    /// [`wgi::KIND_WORLD`] or a kind a `ZoneStreamEnable` object turns on.
    pub kind: u8,
    pub flags: u8,
    /// Bits of [`wgi::lod`].
    pub lod_mask: u32,
}

/// The cells of the world grids of every level, by stream name.
pub type GridCells = HashMap<String, GridCell>;

/// Reads the world grids of the levels of `catalog` from the files of
/// `game`. A level whose grid cannot be read has no cells here: its zones
/// are told apart by the names of their folders.
pub fn grid_cells(game: &GameInstance, catalog: &Catalog) -> GridCells {
    let mut names: HashMap<u32, String> = HashMap::new();
    for index in 0..game.tocs().len() {
        let Ok(table) = game.load_toc(index) else {
            continue;
        };
        for name in &table.names {
            // `*_global` streams have no GUID: their hash is 0.
            if name.hash != 0 {
                names.entry(name.hash).or_insert_with(|| {
                    String::from_utf8_lossy(&crate::toc::normalize_name(&name.name)).into_owned()
                });
            }
        }
    }
    let mut cells = GridCells::new();
    for level in catalog.levels() {
        let Some(stream) = game.find_stream(level) else {
            continue;
        };
        let Ok(file) = game.open_stream(stream) else {
            continue;
        };
        for (resource, entry) in file.layout().resources.iter().enumerate() {
            if entry.header.type_id != wgi::TYPE_WGI {
                continue;
            }
            let Ok(grid) = file
                .resource_data(resource)
                .and_then(|data| WorldGrid::parse(&data))
            else {
                continue;
            };
            for cell in &grid.cells {
                if let Some(name) = names.get(&cell.stream_hash.0) {
                    cells.insert(
                        name.clone(),
                        GridCell {
                            kind: cell.kind,
                            flags: cell.flags,
                            lod_mask: cell.lod_mask,
                        },
                    );
                }
            }
        }
    }
    cells
}

/// What a zone is to the world of its level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Loaded with the level whatever the player does: its root stream and
    /// the streams other zones hang on (`gf2/miami/local`).
    Base,
    /// The world at its finest level of detail, and what the game keeps
    /// loaded at every distance (the ocean, the sky).
    Near,
    /// The inside of a building.
    Interior,
    /// A coarser copy of the world: medium and low detail, the map of the
    /// Don's View.
    Far,
    /// Turned on by the logic of the game: missions, hits, favours,
    /// compounds, and the streams that hang on a zone.
    Mission,
    /// A mission stream of `<level>/global/checkpoints`.
    Checkpoint,
    /// Nothing a preset loads.
    Other,
}

/// The path components of `name` after its level (the first two).
fn after_level(name: &str) -> Vec<&str> {
    name.split('/').skip(2).collect()
}

/// The role of every zone of `catalog`, by stream name.
pub fn roles(catalog: &Catalog, cells: &GridCells) -> HashMap<String, Role> {
    let names: BTreeSet<&str> = catalog.zones.iter().map(|z| z.name.as_str()).collect();
    catalog
        .zones
        .iter()
        .map(|zone| (zone.name.clone(), role(zone, cells, &names)))
        .collect()
}

fn role(zone: &Zone, cells: &GridCells, zones: &BTreeSet<&str>) -> Role {
    let rest = after_level(&zone.name);
    let in_global = |folder: &str| rest.first() == Some(&"global") && rest.get(1) == Some(&folder);
    if let Some(cell) = cells.get(&zone.name) {
        let interior = cell.lod_mask & wgi::lod::INTERIOR != 0;
        if cell.kind != wgi::KIND_WORLD {
            return if in_global("checkpoints") {
                Role::Checkpoint
            } else if interior {
                Role::Interior
            } else {
                Role::Mission
            };
        }
        let far = wgi::lod::MEDIUM | wgi::lod::LOW | wgi::lod::MAP;
        return if interior {
            Role::Interior
        } else if cell.lod_mask & wgi::lod::HIGH != 0 || cell.flags & wgi::FLAG_ALWAYS != 0 {
            Role::Near
        } else if cell.lod_mask & far != 0 {
            Role::Far
        } else {
            Role::Near
        };
    }
    // No cell: by the folders.
    if rest.first() == Some(&"global") {
        return if in_global("checkpoints") {
            Role::Checkpoint
        } else if in_global("objectives") {
            Role::Mission
        } else {
            Role::Other
        };
    }
    // A stream below a stream of the grid is loaded by the scripts of that
    // zone (a cut-scene, an event), not by the grid.
    let mut parent = zone.name.as_str();
    while let Some((dir, _)) = parent.rsplit_once('/') {
        if cells.contains_key(dir) {
            return Role::Mission;
        }
        parent = dir;
    }
    if rest.iter().any(|c| matches!(*c, "medium" | "low")) {
        return Role::Far;
    }
    if rest.iter().any(|c| c.starts_with("interior")) {
        return Role::Interior;
    }
    // The root of the level and the streams other zones hang on.
    let below = format!("{}/", zone.name);
    let has_children = zones
        .range(below.as_str()..)
        .next()
        .is_some_and(|next| next.starts_with(&below));
    if rest.is_empty() || has_children {
        Role::Base
    } else {
        Role::Near
    }
}

/// What a preset is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PresetKind {
    /// The whole level at near detail.
    Level,
    /// One district at near detail.
    District,
    /// The streams the logic of the game turns on.
    Missions,
    /// The streams of `<level>/global/checkpoints`.
    Checkpoints,
}

/// A set of streams computed from the game, with what it costs to load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Preset {
    /// `gf2/miami`, `gf2/miami/bonita`, `gf2/miami:missions`,
    /// `gf2/miami:checkpoints`.
    pub id: String,
    pub kind: PresetKind,
    /// The level (`gf2/miami`).
    pub level: String,
    /// Name of the district, for [`PresetKind::District`].
    pub district: Option<String>,
    /// A short name in English, for lists.
    pub title: String,
    /// What the set holds, in English.
    pub description: String,
    /// Stream names, sorted.
    pub streams: Vec<String>,
    /// Decompressed size of the streams.
    pub bytes: u64,
}

impl Preset {
    fn new(
        id: String,
        kind: PresetKind,
        level: &str,
        district: Option<&str>,
        zones: &[&Zone],
    ) -> Self {
        let (title, description) = match (kind, district) {
            (PresetKind::Level, _) => (
                format!("{level}: whole level, near detail"),
                "the world at its finest level of detail with interiors; no coarser copies, \
                 no missions"
                    .to_owned(),
            ),
            (PresetKind::District, name) => (
                format!("{level}: district {}", name.unwrap_or_default()),
                "the district at its finest level of detail with interiors".to_owned(),
            ),
            (PresetKind::Missions, _) => (
                format!("{level}: missions"),
                "streams the logic of the game turns on: objectives, hits, favours, compounds"
                    .to_owned(),
            ),
            (PresetKind::Checkpoints, _) => (
                format!("{level}: checkpoints"),
                "the mission streams of global/checkpoints".to_owned(),
            ),
        };
        Self {
            id,
            kind,
            level: level.to_owned(),
            district: district.map(str::to_owned),
            title,
            description,
            streams: zones.iter().map(|z| z.name.clone()).collect(),
            bytes: zones.iter().map(|z| z.bytes).sum(),
        }
    }

    /// Number of zones.
    pub fn zones(&self) -> usize {
        self.streams.len()
    }

    /// Size in megabytes (10⁶ bytes, as the zone list shows sizes).
    pub fn megabytes(&self) -> f64 {
        self.bytes as f64 / 1e6
    }
}

/// The presets of one level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LevelPresets {
    pub level: String,
    /// The roles of its zones come from its world grid; `false`: from the
    /// names of the folders.
    pub from_grid: bool,
    pub whole: Preset,
    /// By name.
    pub districts: Vec<Preset>,
    pub missions: Option<Preset>,
    pub checkpoints: Option<Preset>,
}

impl LevelPresets {
    /// Every preset of the level: the whole level, missions, checkpoints,
    /// districts.
    pub fn iter(&self) -> impl Iterator<Item = &Preset> {
        std::iter::once(&self.whole)
            .chain(&self.missions)
            .chain(&self.checkpoints)
            .chain(&self.districts)
    }

    pub fn district(&self, name: &str) -> Option<&Preset> {
        self.districts
            .iter()
            .find(|p| p.district.as_deref() == Some(name))
    }

    /// The folder of a district (`gf2/miami/local/bonita`).
    pub fn district_dir(&self, name: &str) -> String {
        format!("{}/local/{name}", self.level)
    }

    /// The mission streams, of the whole level or of one district only.
    pub fn missions_of(&self, district: Option<&str>) -> Vec<&str> {
        let Some(missions) = &self.missions else {
            return Vec::new();
        };
        let dir = district.map(|d| format!("{}/", self.district_dir(d)));
        missions
            .streams
            .iter()
            .filter(|s| dir.as_ref().is_none_or(|d| s.starts_with(d.as_str())))
            .map(String::as_str)
            .collect()
    }

    /// The streams of a choice made in the editor: the whole level or one
    /// district, with the missions (of the district, when one is given) and
    /// the checkpoints (of the whole level only). `None`: no such district.
    pub fn choice(
        &self,
        district: Option<&str>,
        missions: bool,
        checkpoints: bool,
    ) -> Option<Vec<&str>> {
        let near = match district {
            Some(name) => self.district(name)?,
            None => &self.whole,
        };
        let mut streams: Vec<&str> = near.streams.iter().map(String::as_str).collect();
        if missions {
            streams.extend(self.missions_of(district));
        }
        if checkpoints
            && district.is_none()
            && let Some(preset) = &self.checkpoints
        {
            streams.extend(preset.streams.iter().map(String::as_str));
        }
        Some(streams)
    }
}

/// The presets of every level of a game.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Presets {
    /// In the order of [`Catalog::levels`].
    pub levels: Vec<LevelPresets>,
}

impl Presets {
    /// The presets of the levels of `catalog`, the roles of the zones taken
    /// from `cells` ([`grid_cells`]) and, for a zone without a cell, from
    /// the names of its folders.
    pub fn new(catalog: &Catalog, cells: &GridCells) -> Self {
        let roles = roles(catalog, cells);
        let mut by_level: BTreeMap<&str, Vec<&Zone>> = BTreeMap::new();
        for zone in &catalog.zones {
            by_level.entry(zone.level()).or_default().push(zone);
        }
        let levels = catalog
            .levels()
            .into_iter()
            .map(|level| {
                let mut zones = by_level.remove(level).unwrap_or_default();
                zones.sort_by(|a, b| a.name.cmp(&b.name));
                let with = |wanted: &[Role]| -> Vec<&Zone> {
                    zones
                        .iter()
                        .copied()
                        .filter(|z| wanted.contains(&roles[&z.name]))
                        .collect()
                };
                let shown = [Role::Base, Role::Near, Role::Interior];
                let mut districts: BTreeMap<&str, Vec<&Zone>> = BTreeMap::new();
                for zone in with(&shown) {
                    if let ["local", district, _, ..] = after_level(&zone.name)[..] {
                        districts.entry(district).or_default().push(zone);
                    }
                }
                let optional = |kind: PresetKind, word: &str, role: Role| {
                    let zones = with(&[role]);
                    (!zones.is_empty())
                        .then(|| Preset::new(format!("{level}:{word}"), kind, level, None, &zones))
                };
                LevelPresets {
                    level: level.to_owned(),
                    from_grid: zones.iter().any(|z| cells.contains_key(&z.name)),
                    whole: Preset::new(
                        level.to_owned(),
                        PresetKind::Level,
                        level,
                        None,
                        &with(&shown),
                    ),
                    districts: districts
                        .into_iter()
                        .map(|(name, zones)| {
                            Preset::new(
                                format!("{level}/{name}"),
                                PresetKind::District,
                                level,
                                Some(name),
                                &zones,
                            )
                        })
                        .collect(),
                    missions: optional(PresetKind::Missions, "missions", Role::Mission),
                    checkpoints: optional(PresetKind::Checkpoints, "checkpoints", Role::Checkpoint),
                }
            })
            .collect();
        Self { levels }
    }

    /// The presets of `game`, by its world grids.
    pub fn of_game(game: &GameInstance, catalog: &Catalog) -> Self {
        Self::new(catalog, &grid_cells(game, catalog))
    }

    /// Every preset, level by level.
    pub fn iter(&self) -> impl Iterator<Item = &Preset> {
        self.levels.iter().flat_map(LevelPresets::iter)
    }

    /// The presets of a level given by its name (`gf2/miami`) or, when no
    /// other level ends the same way, by its last component (`miami`,
    /// `mp_mall`).
    pub fn level(&self, name: &str) -> Option<&LevelPresets> {
        if let Some(found) = self.levels.iter().find(|l| l.level == name) {
            return Some(found);
        }
        let mut short = self
            .levels
            .iter()
            .filter(|l| l.level.rsplit('/').next() == Some(name));
        let found = short.next()?;
        short.next().is_none().then_some(found)
    }

    /// The streams a preset id names: `<level>` (the whole level at near
    /// detail), `<level>/<district>`, and either followed by `:missions`
    /// (the missions of the level or of the district alone) or, for a
    /// level, `:checkpoints`. The level as [`Self::level`] takes it.
    pub fn resolve(&self, id: &str) -> Option<Vec<&str>> {
        let (place, part) = match id.rsplit_once(':') {
            Some((place, part)) => (place, Some(part)),
            None => (id, None),
        };
        let (level, district) = match self.level(place) {
            Some(level) => (level, None),
            None => {
                let (level, district) = place.rsplit_once('/')?;
                let level = self.level(level)?;
                level.district(district)?;
                (level, Some(district))
            }
        };
        match (part, district) {
            (None, _) => level.choice(district, false, false),
            (Some("missions"), _) => Some(level.missions_of(district)),
            (Some("checkpoints"), None) => Some(
                level
                    .checkpoints
                    .iter()
                    .flat_map(|p| p.streams.iter().map(String::as_str))
                    .collect(),
            ),
            _ => None,
        }
    }
}

/// A set of zones saved under a name: `[[zones]]` of a project's
/// `project.toml`, or of the editor's settings of a game. Streams are named,
/// never numbered, so a set outlives streams added to the game. An entry is
/// read as [`resolve_saved`] reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoneSet {
    pub name: String,
    #[serde(default)]
    pub streams: Vec<String>,
}

/// What a list of words names in a game ([`resolve`], [`resolve_saved`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolved {
    /// Stream indices of the zones.
    pub streams: BTreeSet<usize>,
    /// Typed words that name nothing: no such zone, folder, preset or set.
    pub unknown: Vec<String>,
    /// Entries of a saved choice that the game does not have (any more):
    /// they are skipped.
    pub gone: Vec<String>,
}

/// Stream indices of the zones of `catalog` by name.
fn zone_streams(catalog: &Catalog) -> HashMap<&str, usize> {
    catalog
        .zones
        .iter()
        .map(|z| (z.name.as_str(), z.stream))
        .collect()
}

/// The zones of a saved choice (a [`ZoneSet`], what the editor loaded
/// last). An entry is the name of a zone and means that zone alone — also
/// when other zones lie below it, as below the root stream of a level; an
/// entry no zone has is a folder of zones (`gf2/miami/local/bonita/high`,
/// for sets written by hand), and `preset:<id>` is a preset
/// ([`Presets::resolve`]). Entries that name nothing are skipped and listed
/// in [`Resolved::gone`].
pub fn resolve_saved<'a>(
    entries: impl IntoIterator<Item = &'a str>,
    catalog: &Catalog,
    presets: &Presets,
) -> Resolved {
    let zones = zone_streams(catalog);
    let mut out = Resolved::default();
    for entry in entries {
        let entry = entry.trim().trim_end_matches('/');
        // A preset that is there and holds nothing (the missions of a
        // district without any) is not gone.
        let found: Option<Vec<usize>> = match entry.strip_prefix("preset:") {
            Some(id) => presets.resolve(id).map(|streams| {
                streams
                    .into_iter()
                    .filter_map(|name| zones.get(name).copied())
                    .collect()
            }),
            None => match zones.get(entry) {
                Some(&stream) => Some(vec![stream]),
                None => {
                    let below: Vec<usize> = catalog.under(entry).map(|z| z.stream).collect();
                    (!below.is_empty()).then_some(below)
                }
            },
        };
        match found {
            Some(found) => out.streams.extend(found),
            None => out.gone.push(entry.to_owned()),
        }
    }
    out
}

/// The zones a list of typed words names (the command line,
/// `DOCTOR_TOOLKIT_GUI_LEVEL`). A word is a zone with every zone below it or
/// a folder of zones (`gf2/miami/local/bonita/high`; `gf2/miami` is the
/// whole level at every level of detail), `preset:<id>`
/// ([`Presets::resolve`]) or `set:<name>` (one of `sets`,
/// [`resolve_saved`]).
pub fn resolve<'a>(
    words: impl IntoIterator<Item = &'a str>,
    catalog: &Catalog,
    presets: &Presets,
    sets: &[ZoneSet],
) -> Resolved {
    let zones = zone_streams(catalog);
    let mut out = Resolved::default();
    for word in words {
        let word = word.trim();
        if word.is_empty() {
            continue;
        }
        if let Some(id) = word.strip_prefix("preset:") {
            match presets.resolve(id) {
                Some(streams) => out.streams.extend(
                    streams
                        .into_iter()
                        .filter_map(|name| zones.get(name).copied()),
                ),
                None => out.unknown.push(word.to_owned()),
            }
        } else if let Some(name) = word.strip_prefix("set:") {
            match sets.iter().find(|s| s.name == name) {
                Some(set) => {
                    let saved =
                        resolve_saved(set.streams.iter().map(String::as_str), catalog, presets);
                    out.streams.extend(saved.streams);
                    out.gone.extend(saved.gone);
                }
                None => out.unknown.push(word.to_owned()),
            }
        } else {
            let found: Vec<usize> = catalog
                .under(word.trim_end_matches('/'))
                .map(|z| z.stream)
                .collect();
            if found.is_empty() {
                out.unknown.push(word.to_owned());
            }
            out.streams.extend(found);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog(names: &[&str]) -> Catalog {
        let mut zones: Vec<Zone> = names
            .iter()
            .enumerate()
            .map(|(stream, name)| Zone {
                stream,
                name: (*name).to_owned(),
                simgroups: 1,
                meshes: 0,
                metamodels: 0,
                bytes: 1_000_000,
            })
            .collect();
        zones.sort_by(|a, b| a.name.cmp(&b.name));
        Catalog { zones }
    }

    fn cell(kind: u8, flags: u8, lod_mask: u32) -> GridCell {
        GridCell {
            kind,
            flags,
            lod_mask,
        }
    }

    const CITY: &[&str] = &[
        "gf2/miami",
        "gf2/miami/local",
        "gf2/miami/local/bonita/high/terrain06",
        "gf2/miami/local/bonita/high/terrain07",
        "gf2/miami/local/bonita/high/hit34stream",
        "gf2/miami/local/bonita/medium/zone1",
        "gf2/miami/local/bonita/low/zone1",
        "gf2/miami/local/bonita/interior/venue_115",
        "gf2/miami/local/alice/high/terrain01",
        "gf2/miami/local/alice/high/terrain01/design/trinity1",
        "gf2/miami/local/alice/medium/poi_p01_giverzonestream/poi_p01_giver_zonestream",
        "gf2/miami/local/ocean/ocean",
        "gf2/miami/local/sky/sky",
        "gf2/miami/global/dons_view/map_stream",
        "gf2/miami/global/objectives/c29/r07_common_folderstream",
        "gf2/queens",
        "gf2/queens/local/ham/high/zone1/terrain02",
        "gf2/queens/global/checkpoints/c17/r04_common_folderstream",
        "mp_mall/mp_mall",
        "mp_mall/mp_mall/terrain/high",
        "mp_mall/mp_mall/terrain/high/side1/terrain1",
        "mp_mall/mp_mall/terrain/medium",
        "mp_mall/mp_mall/terrain/sky_high",
        "mp_mall/mp_mall_global",
        "gf2/frontend",
    ];

    /// The cells of [`CITY`] as the game has them: world cells by level of
    /// detail, the ocean and the sky always loaded, a hit in a `high` folder
    /// and a favour in a `medium` folder turned on by logic.
    fn city_cells() -> GridCells {
        use wgi::lod::*;
        [
            ("gf2/miami/local/bonita/high/terrain06", cell(0, 0, HIGH)),
            ("gf2/miami/local/bonita/high/terrain07", cell(0, 2, HIGH)),
            ("gf2/miami/local/bonita/high/hit34stream", cell(5, 1, 0)),
            ("gf2/miami/local/bonita/medium/zone1", cell(0, 0, MEDIUM)),
            ("gf2/miami/local/bonita/low/zone1", cell(0, 0, LOW)),
            (
                "gf2/miami/local/bonita/interior/venue_115",
                cell(5, 0, INTERIOR),
            ),
            ("gf2/miami/local/alice/high/terrain01", cell(0, 0, HIGH)),
            (
                "gf2/miami/local/alice/medium/poi_p01_giverzonestream/poi_p01_giver_zonestream",
                cell(2, 1, 0),
            ),
            ("gf2/miami/local/ocean/ocean", cell(0, 1, HIGH)),
            ("gf2/miami/local/sky/sky", cell(0, 1, LOW)),
            ("gf2/miami/global/dons_view/map_stream", cell(0, 0, MAP)),
            (
                "gf2/queens/local/ham/high/zone1/terrain02",
                cell(0, 0, HIGH),
            ),
            (
                "gf2/queens/global/checkpoints/c17/r04_common_folderstream",
                cell(2, 1, 0),
            ),
            (
                "mp_mall/mp_mall/terrain/high/side1/terrain1",
                cell(0, 0, HIGH),
            ),
            ("mp_mall/mp_mall/terrain/medium", cell(0, 0, MEDIUM)),
            ("mp_mall/mp_mall/terrain/sky_high", cell(0, 1, HIGH)),
        ]
        .into_iter()
        .map(|(name, cell)| (name.to_owned(), cell))
        .collect()
    }

    #[test]
    fn roles_come_from_the_world_grid() {
        let catalog = catalog(CITY);
        let roles = roles(&catalog, &city_cells());
        let role = |name: &str| roles[name];
        assert_eq!(role("gf2/miami"), Role::Base);
        assert_eq!(role("gf2/miami/local"), Role::Base);
        assert_eq!(role("gf2/miami/local/bonita/high/terrain06"), Role::Near);
        // In a `high` folder, yet turned on by a mission.
        assert_eq!(
            role("gf2/miami/local/bonita/high/hit34stream"),
            Role::Mission
        );
        // In a `medium` folder, yet not a coarser copy of the district.
        assert_eq!(
            role("gf2/miami/local/alice/medium/poi_p01_giverzonestream/poi_p01_giver_zonestream"),
            Role::Mission
        );
        assert_eq!(role("gf2/miami/local/bonita/medium/zone1"), Role::Far);
        assert_eq!(role("gf2/miami/local/bonita/low/zone1"), Role::Far);
        assert_eq!(
            role("gf2/miami/local/bonita/interior/venue_115"),
            Role::Interior
        );
        // Below a zone of the grid, without a cell of its own.
        assert_eq!(
            role("gf2/miami/local/alice/high/terrain01/design/trinity1"),
            Role::Mission
        );
        // Always loaded: near at every distance, whatever the mask.
        assert_eq!(role("gf2/miami/local/ocean/ocean"), Role::Near);
        assert_eq!(role("gf2/miami/local/sky/sky"), Role::Near);
        assert_eq!(role("gf2/miami/global/dons_view/map_stream"), Role::Far);
        assert_eq!(
            role("gf2/miami/global/objectives/c29/r07_common_folderstream"),
            Role::Mission
        );
        assert_eq!(
            role("gf2/queens/global/checkpoints/c17/r04_common_folderstream"),
            Role::Checkpoint
        );
        // A multiplayer map does not keep to the folder names.
        assert_eq!(role("mp_mall/mp_mall/terrain/high"), Role::Base);
        assert_eq!(role("mp_mall/mp_mall/terrain/medium"), Role::Far);
        assert_eq!(role("mp_mall/mp_mall/terrain/sky_high"), Role::Near);
    }

    #[test]
    fn folders_tell_the_roles_without_a_grid() {
        let catalog = catalog(CITY);
        let roles = roles(&catalog, &GridCells::new());
        let role = |name: &str| roles[name];
        assert_eq!(role("gf2/miami"), Role::Base);
        assert_eq!(role("gf2/miami/local"), Role::Base);
        assert_eq!(role("gf2/miami/local/bonita/high/terrain06"), Role::Near);
        assert_eq!(role("gf2/miami/local/bonita/medium/zone1"), Role::Far);
        assert_eq!(role("gf2/miami/local/bonita/low/zone1"), Role::Far);
        assert_eq!(
            role("gf2/miami/local/bonita/interior/venue_115"),
            Role::Interior
        );
        assert_eq!(
            role("gf2/miami/global/objectives/c29/r07_common_folderstream"),
            Role::Mission
        );
        assert_eq!(
            role("gf2/queens/global/checkpoints/c17/r04_common_folderstream"),
            Role::Checkpoint
        );
        assert_eq!(role("gf2/miami/global/dons_view/map_stream"), Role::Other);
        // What the folders cannot tell: the hit is near detail to them.
        assert_eq!(role("gf2/miami/local/bonita/high/hit34stream"), Role::Near);
        assert_eq!(role("gf2/frontend"), Role::Base);
    }

    #[test]
    fn levels_districts_and_missions_apart() {
        let catalog = catalog(CITY);
        let presets = Presets::new(&catalog, &city_cells());
        let levels: Vec<&str> = presets.levels.iter().map(|l| l.level.as_str()).collect();
        assert_eq!(levels, catalog.levels());
        let miami = presets.level("gf2/miami").unwrap();
        assert!(miami.from_grid);
        assert_eq!(
            miami.whole.streams,
            [
                "gf2/miami",
                "gf2/miami/local",
                "gf2/miami/local/alice/high/terrain01",
                "gf2/miami/local/bonita/high/terrain06",
                "gf2/miami/local/bonita/high/terrain07",
                "gf2/miami/local/bonita/interior/venue_115",
                "gf2/miami/local/ocean/ocean",
                "gf2/miami/local/sky/sky",
            ]
        );
        assert_eq!(miami.whole.zones(), 8);
        assert_eq!(miami.whole.bytes, 8_000_000);
        assert_eq!(miami.whole.megabytes(), 8.0);
        let districts: Vec<&str> = miami.districts.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            districts,
            [
                "gf2/miami/alice",
                "gf2/miami/bonita",
                "gf2/miami/ocean",
                "gf2/miami/sky"
            ]
        );
        assert_eq!(
            miami.district("bonita").unwrap().streams,
            [
                "gf2/miami/local/bonita/high/terrain06",
                "gf2/miami/local/bonita/high/terrain07",
                "gf2/miami/local/bonita/interior/venue_115",
            ]
        );
        let missions = miami.missions.as_ref().unwrap();
        assert_eq!(missions.id, "gf2/miami:missions");
        assert_eq!(
            missions.streams,
            [
                "gf2/miami/global/objectives/c29/r07_common_folderstream",
                "gf2/miami/local/alice/high/terrain01/design/trinity1",
                "gf2/miami/local/alice/medium/poi_p01_giverzonestream/poi_p01_giver_zonestream",
                "gf2/miami/local/bonita/high/hit34stream",
            ]
        );
        assert!(miami.checkpoints.is_none());
        assert_eq!(
            miami.missions_of(Some("bonita")),
            ["gf2/miami/local/bonita/high/hit34stream"]
        );
        // The choice of the editor: a district with its missions; the
        // checkpoints are of the whole level only.
        assert_eq!(miami.choice(Some("bonita"), true, true).unwrap().len(), 4);
        assert_eq!(miami.choice(None, true, false).unwrap().len(), 12);
        assert!(miami.choice(Some("nowhere"), false, false).is_none());

        let queens = presets.level("queens").unwrap();
        assert!(queens.missions.is_none());
        assert_eq!(
            queens.checkpoints.as_ref().unwrap().streams,
            ["gf2/queens/global/checkpoints/c17/r04_common_folderstream"]
        );
        assert_eq!(queens.choice(None, false, true).unwrap().len(), 3);

        let mall = presets.level("mp_mall").unwrap();
        assert_eq!(mall.level, "mp_mall/mp_mall");
        assert!(mall.districts.is_empty());
        assert_eq!(
            mall.whole.streams,
            [
                "mp_mall/mp_mall",
                "mp_mall/mp_mall/terrain/high",
                "mp_mall/mp_mall/terrain/high/side1/terrain1",
                "mp_mall/mp_mall/terrain/sky_high",
            ]
        );
        // A level of one zone has its preset too.
        assert_eq!(
            presets.level("gf2/frontend").unwrap().whole.streams,
            ["gf2/frontend"]
        );
        assert_eq!(
            presets.iter().count(),
            presets
                .levels
                .iter()
                .map(|l| 1
                    + l.districts.len()
                    + usize::from(l.missions.is_some())
                    + usize::from(l.checkpoints.is_some()))
                .sum::<usize>()
        );
    }

    #[test]
    fn preset_ids_and_words() {
        let catalog = catalog(CITY);
        let presets = Presets::new(&catalog, &city_cells());
        assert_eq!(presets.resolve("gf2/miami").unwrap().len(), 8);
        assert_eq!(presets.resolve("miami").unwrap().len(), 8);
        assert_eq!(presets.resolve("miami/bonita").unwrap().len(), 3);
        assert_eq!(presets.resolve("gf2/miami/bonita").unwrap().len(), 3);
        assert_eq!(presets.resolve("miami:missions").unwrap().len(), 4);
        assert_eq!(presets.resolve("miami/bonita:missions").unwrap().len(), 1);
        assert_eq!(presets.resolve("queens:checkpoints").unwrap().len(), 1);
        assert_eq!(presets.resolve("miami:checkpoints").unwrap().len(), 0);
        assert!(presets.resolve("miami/bonita:checkpoints").is_none());
        assert!(presets.resolve("miami/nowhere").is_none());
        assert!(presets.resolve("havana").is_none());
        assert!(presets.resolve("miami:everything").is_none());
        // Every listed preset is found by its id.
        for preset in presets.iter() {
            let found = presets.resolve(&preset.id).expect(&preset.id);
            assert_eq!(found, preset.streams, "{}", preset.id);
        }

        let sets = [ZoneSet {
            name: "my yard".into(),
            streams: vec![
                "gf2/miami/local/bonita/high/terrain06".into(),
                // A folder, written by hand.
                "gf2/miami/local/alice/medium".into(),
                // A zone with a zone below it: that zone alone.
                "gf2/miami/local/alice/high/terrain01".into(),
                "gf2/miami/local/bonita/high/removed".into(),
            ],
        }];
        let stream = |name: &str| catalog.under(name).next().unwrap().stream;
        let streams =
            |names: &[&str]| -> BTreeSet<usize> { names.iter().map(|name| stream(name)).collect() };
        let found = resolve(
            ["preset:miami/bonita", "set:my yard", "gf2/queens/local"],
            &catalog,
            &presets,
            &sets,
        );
        assert!(found.unknown.is_empty(), "{:?}", found.unknown);
        assert_eq!(found.gone, ["gf2/miami/local/bonita/high/removed"]);
        assert_eq!(
            found.streams,
            streams(&[
                "gf2/miami/local/bonita/high/terrain06",
                "gf2/miami/local/bonita/high/terrain07",
                "gf2/miami/local/bonita/interior/venue_115",
                "gf2/miami/local/alice/medium/poi_p01_giverzonestream/poi_p01_giver_zonestream",
                "gf2/miami/local/alice/high/terrain01",
                "gf2/queens/local/ham/high/zone1/terrain02",
            ])
        );
        // A typed word is a zone with everything below it: the root stream
        // of a level is the whole level. A preset and a saved entry name
        // the root stream alone.
        let typed = resolve(["gf2/miami"], &catalog, &presets, &[]);
        assert_eq!(typed.streams.len(), 15);
        let preset = resolve(["preset:gf2/miami"], &catalog, &presets, &[]);
        assert_eq!(preset.streams.len(), 8);
        assert!(preset.streams.contains(&stream("gf2/miami")));
        // A preset without streams (Miami has no checkpoints) is not gone.
        let saved = resolve_saved(
            [
                "gf2/miami",
                "preset:miami/bonita:missions",
                "preset:miami:checkpoints",
                "preset:gone",
                "gf2/miami/local/gone",
            ],
            &catalog,
            &presets,
        );
        assert_eq!(
            saved.streams,
            streams(&["gf2/miami", "gf2/miami/local/bonita/high/hit34stream"])
        );
        assert_eq!(saved.gone, ["preset:gone", "gf2/miami/local/gone"]);
        assert!(saved.unknown.is_empty());
        let wrong = resolve(
            ["preset:nowhere", "set:none", "gf2/none", ""],
            &catalog,
            &presets,
            &sets,
        );
        assert!(wrong.streams.is_empty());
        assert_eq!(wrong.unknown, ["preset:nowhere", "set:none", "gf2/none"]);
    }
}
