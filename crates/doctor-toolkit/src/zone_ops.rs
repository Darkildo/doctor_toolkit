//! Operations that make a new zone, one function per action with its
//! parameters as a structure: a new stream, a SimGroup in it, its cell of
//! the world grid, resources cloned or added into it, a model imported into
//! it. The editor's forms and its New zone wizard, the command line and the
//! hands-free presets call these, so that an action means the same on every
//! surface (stage 103).
//!
//! Every function records one undoable operation of the workspace. `game`
//! may be the staged game ([`Workspace::stage`]): then a stream the
//! workspace creates is addressed like any other (`<stream>#<resource>`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::container::ustream::{ResourceHeader, Storage};
use crate::create::{self, CloneOptions, IdForm, Remap};
use crate::game::{GameAssets, GameInstance, ResourceIndex};
use crate::import::model;
use crate::level::edit::{self, GroupAt, NewGroup};
use crate::level::world_grid::{self, ZoneCell, ZoneCellAdded};
use crate::level::{Location, TYPE_SIMGROUP};
use crate::resource::hkx::collision::CollisionKind;
use crate::resource::simgroup::Guid;
use crate::resource::wgi;
use crate::workspace::key::normalize_stream;
use crate::workspace::{
    NewResourceEdit, NewStream, ResourceEdit, ResourceKey, StorageChoice, Workspace,
};
use crate::{Error, Result};

fn bad(text: impl Into<String>) -> Error {
    Error::InvalidArgument(text.into())
}

/// A stream by name: of the game (staged streams included) or new in the
/// workspace. Returns the normalised name.
pub fn any_stream(game: &GameInstance, ws: &Workspace, name: &str) -> Result<String> {
    let name = normalize_stream(name.trim());
    if name.is_empty() || name.contains("..") {
        return Err(bad(format!("`{name}` is not a stream name")));
    }
    if game.find_stream(&name).is_some() || ws.new_stream(&name).is_some() {
        return Ok(name);
    }
    Err(bad(format!(
        "no stream `{name}` in the game or among the new streams of the workspace"
    )))
}

/// A resource written as `<stream>#<index>`.
pub fn parse_location(game: &GameInstance, text: &str) -> Result<Location> {
    let text = text.trim();
    let (stream, resource) = text
        .rsplit_once('#')
        .ok_or_else(|| bad(format!("`{text}`: <stream>#<resource index> expected")))?;
    let resource = resource
        .trim()
        .parse()
        .map_err(|_| bad(format!("`{resource}` is not a resource index")))?;
    let name = normalize_stream(stream.trim());
    let stream = game
        .find_stream(&name)
        .ok_or_else(|| bad(format!("stream `{name}` not found")))?;
    Ok(Location { stream, resource })
}

// ------------------------------------------------------------ streams

/// How the game comes to load a new stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamLoad {
    /// When game data asks for its hash: a cell of the world grid
    /// ([`add_zone_cell`]), a `StreamSet`.
    #[default]
    OnDemand,
    /// A parented stream of its parent, disabled until an
    /// `EnableDisableStreams` object enables its hash.
    WithParent,
    /// A parented stream opened with every load of its parent.
    AutoLoad,
}

impl StreamLoad {
    pub const ALL: [Self; 3] = [Self::OnDemand, Self::WithParent, Self::AutoLoad];

    pub fn slug(self) -> &'static str {
        match self {
            Self::OnDemand => "on_demand",
            Self::WithParent => "with_parent",
            Self::AutoLoad => "auto_load",
        }
    }

    pub fn from_slug(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.slug() == text)
    }
}

/// [`add_stream`]: a new stream.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NewStreamParams {
    /// Name, e.g. `gf2/doctor/yard` (file `gf2/doctor/yard.str`).
    pub name: String,
    /// The stream it depends on, loaded before it.
    pub parent: String,
    #[serde(default)]
    pub load: StreamLoad,
    /// Tables of contents that list it (`toc` or `toc=parent`); every table
    /// that lists the parent when empty.
    #[serde(default)]
    pub tocs: Vec<String>,
    /// 32 hexadecimal digits; derived from the name when `None`.
    #[serde(default)]
    pub guid: Option<String>,
}

/// Records a new stream ([`create::new_stream`], [`Workspace::add_stream`]).
pub fn add_stream(
    game: &GameInstance,
    ws: &mut Workspace,
    p: &NewStreamParams,
) -> Result<NewStream> {
    let name = normalize_stream(p.name.trim());
    if name.is_empty() {
        return Err(bad("a new stream needs a name, e.g. gf2/doctor/yard"));
    }
    let parent = normalize_stream(p.parent.trim());
    if game.find_stream(&parent).is_none() {
        return Err(bad(format!(
            "parent stream `{parent}` is not a stream of the game"
        )));
    }
    let mut stream = create::new_stream(
        game,
        ws,
        &name,
        &parent,
        p.load != StreamLoad::OnDemand,
        &p.tocs,
        p.guid.as_deref().filter(|g| !g.trim().is_empty()),
    )?;
    stream.auto_load = p.load == StreamLoad::AutoLoad;
    ws.add_stream(game, stream.clone())?;
    Ok(stream)
}

/// Removes a stream the workspace created with everything added to it.
/// Returns how many resources went with it.
pub fn remove_stream(ws: &mut Workspace, name: &str) -> Result<usize> {
    let name = normalize_stream(name.trim());
    let resources = ws.new_stream_resources(&name).len();
    if !ws.remove_stream(&name)? {
        return Err(bad(format!("{name} is not a stream the workspace created")));
    }
    Ok(resources)
}

// ------------------------------------------------------------ SimGroups

/// [`add_simgroup`]: a new SimGroup.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NewGroupParams {
    /// The stream it goes into: of the game or new.
    pub stream: String,
    /// Copy the objects of this SimGroup (`<stream>#<resource>`, or the key
    /// of a SimGroup the workspace added); empty when `None`.
    #[serde(default)]
    pub copy_of: Option<String>,
    /// With `copy_of`: only these objects.
    #[serde(default)]
    pub objects: Option<Vec<usize>>,
}

/// A SimGroup written as `<stream>#<resource>` or as a workspace key.
pub fn parse_group(game: &GameInstance, text: &str) -> Result<GroupAt> {
    if text.contains('#') {
        return Ok(GroupAt::Game(parse_location(game, text)?));
    }
    let key: ResourceKey = text.trim().parse()?;
    // A key of a stream the game has (staged or applied) is a location.
    if let Some(stream) = game.find_stream(&key.stream)
        && let Some(resource) = key.find_in(game.open_stream(stream)?.layout())
    {
        return Ok(GroupAt::Game(Location { stream, resource }));
    }
    Ok(GroupAt::Added(key))
}

/// Records a new SimGroup ([`edit::add_group`]).
pub fn add_simgroup(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: &mut Workspace,
    p: &NewGroupParams,
    taken: &dyn Fn(&Guid) -> bool,
) -> Result<NewGroup> {
    if p.objects.is_some() && p.copy_of.is_none() {
        return Err(bad("a list of objects goes with a SimGroup to copy"));
    }
    let stream = any_stream(game, ws, &p.stream)?;
    let from = p
        .copy_of
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .map(|t| parse_group(game, t))
        .transpose()?;
    edit::add_group(
        game,
        index,
        ws,
        &stream,
        from.as_ref().map(|g| (g, p.objects.as_deref())),
        taken,
    )
}

// ------------------------------------------------------------ world grid

/// [`add_zone_cell`]: a cell of the world grid of a level for a stream.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ZoneCellParams {
    /// The stream that holds the world grid, e.g. `gf2/cuba`.
    pub level: String,
    /// The stream the cell loads.
    pub stream: String,
    pub min: [f32; 3],
    pub max: [f32; 3],
    /// `high` (default), `medium`, `low` or `map`.
    #[serde(default)]
    pub lod: Option<String>,
    /// The name a `ZoneStreamEnable` object carries.
    #[serde(default)]
    pub name: Option<String>,
    /// Loaded only inside a `ZoneStreamEnable` box with `name`.
    #[serde(default)]
    pub on_enable: bool,
    /// GUID of the stream when it is not new in the workspace.
    #[serde(default)]
    pub guid: Option<String>,
}

/// The level-of-detail mask of a name.
pub fn parse_lod(text: Option<&str>) -> Result<u32> {
    Ok(
        match text
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or("high")
        {
            "high" => wgi::lod::HIGH,
            "medium" => wgi::lod::MEDIUM,
            "low" => wgi::lod::LOW,
            "map" => wgi::lod::MAP,
            other => {
                return Err(bad(format!(
                    "the level of detail is high, medium, low or map, not {other}"
                )));
            }
        },
    )
}

/// Records a cell of the world grid ([`world_grid::add_zone_cell`]).
pub fn add_zone_cell(
    game: &GameInstance,
    ws: &mut Workspace,
    p: &ZoneCellParams,
) -> Result<ZoneCellAdded> {
    let name = p.name.clone().filter(|n| !n.trim().is_empty());
    if p.on_enable && name.is_none() {
        return Err(bad(
            "a cell enabled by a ZoneStreamEnable object needs the name the object carries",
        ));
    }
    for axis in 0..3 {
        if !(p.min[axis].is_finite() && p.max[axis].is_finite()) || p.min[axis] >= p.max[axis] {
            return Err(bad(
                "the box of the cell needs its minimum below its maximum on every axis",
            ));
        }
    }
    let level = normalize_stream(p.level.trim());
    world_grid::add_zone_cell(
        game,
        ws,
        &level,
        &ZoneCell {
            stream: p.stream.trim().to_owned(),
            guid: p.guid.clone().filter(|g| !g.trim().is_empty()),
            box_min: p.min,
            box_max: p.max,
            lod_mask: parse_lod(p.lod.as_deref())?,
            kind: if p.on_enable { 5 } else { wgi::KIND_WORLD },
            flags: 0,
            name,
        },
    )
}

/// One cell of a world grid, for lists.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CellInfo {
    /// Resource index of the `wgi` in the level's stream.
    pub resource: usize,
    pub cell: usize,
    pub hash: u32,
    /// The stream by its hash; the hash in hexadecimal when unknown.
    pub stream: String,
    /// The stream is new in the workspace.
    pub new: bool,
    pub centre: [f32; 3],
    pub radius: f32,
    pub kind: u8,
    pub lod_mask: u32,
}

pub use crate::level::world_grid::stream_names;

/// The cells of the world grids of `level` as the workspace has them.
pub fn zone_cells(game: &GameInstance, ws: &Workspace, level: &str) -> Result<Vec<CellInfo>> {
    let level = normalize_stream(level.trim());
    let names = stream_names(game, ws)?;
    let mut out = Vec::new();
    for g in world_grid::level_grids(game, ws, &level)? {
        for (cell, c) in g.grid.cells.iter().enumerate() {
            let hash = c.stream_hash.0;
            let stream = names
                .get(&hash)
                .cloned()
                .unwrap_or_else(|| format!("{hash:08x}"));
            out.push(CellInfo {
                resource: g.resource,
                cell,
                hash,
                new: ws.new_stream(&stream).is_some(),
                stream,
                centre: c.centre,
                radius: c.radius,
                kind: c.kind,
                lod_mask: c.lod_mask,
            });
        }
    }
    Ok(out)
}

/// Removes the cell of `stream` (a name, or a hash in hexadecimal) from the
/// world grids of `level` ([`world_grid::remove_stream_cell`]).
pub fn remove_zone_cell(
    game: &GameInstance,
    ws: &mut Workspace,
    level: &str,
    stream: &str,
) -> Result<u32> {
    let level = normalize_stream(level.trim());
    world_grid::remove_stream_cell(game, ws, &level, stream)
}

// ------------------------------------------------------------ resources

/// [`clone_resources`]: clones of resources under new ids.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CloneParams {
    /// The stream that holds the resources.
    pub stream: String,
    /// Resource indices; companions (texture header and data, mesh and
    /// `meshvolatile`) are cloned with them.
    pub indices: Vec<usize>,
    /// The stream the clones go into; the same stream when `None`.
    #[serde(default)]
    pub to: Option<String>,
    /// Text the new ids are derived from; the target stream when `None`.
    #[serde(default)]
    pub seed: Option<String>,
    /// Keep the references between the clones pointing at the originals.
    #[serde(default)]
    pub keep_references: bool,
    /// Resources of the source stream to point at the clones.
    #[serde(default)]
    pub retarget: Vec<usize>,
}

/// What [`clone_resources`] made.
#[derive(Debug, Clone, PartialEq)]
pub struct Cloned {
    pub source: String,
    pub target: String,
    /// (source, clone); a clone without a source has `None`.
    pub clones: Vec<(Option<ResourceKey>, ResourceKey)>,
    pub remap: Remap,
    /// Retargeted resources with the number of references changed.
    pub retargeted: Vec<(ResourceKey, usize)>,
    pub label: String,
}

/// Where clones of resources of game stream `source` go in `target`.
///
/// A stream the workspace creates holds its resources in the order they
/// were added (SimGroups last), so no place is asked for: this is checked
/// first, because once applied or staged the stream is among the game's as
/// well. In the same stream the clones follow their sources; in another
/// game stream they go before its first SimGroup, so that what a SimGroup
/// refers to comes first. The place is a resource of the original stream:
/// the game file may hold resources the workspace added and applied, and
/// those cannot be places.
pub fn place_for_clones(
    game: &GameInstance,
    ws: &Workspace,
    source: usize,
    target: &str,
    last_source: usize,
) -> Result<Option<ResourceKey>> {
    use crate::container::ustream::StreamLayout;
    let target = normalize_stream(target);
    if ws.new_stream(&target).is_some() {
        return Ok(None);
    }
    let target_index = game
        .find_stream(&target)
        .ok_or_else(|| bad(format!("stream {target} is neither in the game nor new")))?;
    let original = {
        let path = game.original_path(&game.streams()[target_index]);
        let data = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
        StreamLayout::parse(&data)?
    };
    let is_original = |header: &ResourceHeader| {
        ResourceKey::new(&target, header)
            .find_in(&original)
            .is_some()
    };
    let stream = game.open_stream(target_index)?;
    let resources = &stream.layout().resources;
    let found = if target_index == source {
        resources
            .iter()
            .skip(last_source + 1)
            .find(|r| is_original(&r.header))
    } else {
        resources
            .iter()
            .filter(|r| r.header.type_id == TYPE_SIMGROUP)
            .find(|r| is_original(&r.header))
    };
    Ok(found.map(|r| ResourceKey::new(&target, &r.header)))
}

/// Records clones of resources of a stream, in the same or another stream,
/// and points the `retarget` resources of the source stream at them: one
/// undoable operation.
pub fn clone_resources(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: &mut Workspace,
    p: &CloneParams,
) -> Result<Cloned> {
    if p.indices.is_empty() {
        return Err(bad("no resources to clone"));
    }
    let source = any_stream(game, ws, &p.stream)?;
    let source_index = game.find_stream(&source).ok_or_else(|| {
        bad(format!(
            "{source} is new and not staged: its resources have no indices yet"
        ))
    })?;
    let target = match p.to.as_deref().filter(|t| !t.trim().is_empty()) {
        Some(to) => any_stream(game, ws, to)?,
        None => source.clone(),
    };
    let file = game.open_stream(source_index)?;
    let mut used = ws.used_ids(index);
    let options = CloneOptions {
        seed: p
            .seed
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| target.clone()),
        rewrite_references: !p.keep_references,
        storage: None,
    };
    let (clones, remap) = create::clone_resources(&file, &p.indices, &mut used, &options)?;
    let last = clones.iter().filter_map(|c| c.source).max().unwrap_or(0);
    let before = place_for_clones(game, ws, source_index, &target, last)?;
    let additions: Vec<NewResourceEdit> = clones
        .iter()
        .map(|c| NewResourceEdit {
            stream: target.clone(),
            header: c.header,
            data: c.data.clone(),
            storage: match c.storage {
                Storage::Raw => StorageChoice::Raw,
                Storage::RefPack => StorageChoice::Refpack,
            },
            before: before.clone(),
        })
        .collect();
    let mut retargeted = Vec::new();
    let mut edits = Vec::new();
    for &r in &p.retarget {
        let resource = file
            .layout()
            .resources
            .get(r)
            .ok_or_else(|| bad(format!("{source} has no resource {r} to retarget")))?;
        let key = ResourceKey::new(&source, &resource.header);
        let mut data = ws.current_data(game, source_index, r)?;
        let changed = create::rewrite_references(resource.header.type_id, &mut data, &remap)?;
        retargeted.push((key, changed));
        if changed > 0 {
            edits.push(ResourceEdit {
                index: r,
                data,
                storage: StorageChoice::Keep,
            });
        }
    }
    let list = clones
        .iter()
        .map(|c| {
            (
                c.source
                    .map(|i| ResourceKey::new(&source, &file.layout().resources[i].header)),
                ResourceKey::new(&target, &c.header),
            )
        })
        .collect();
    drop(file);
    let label = format!(
        "Clone {} resource(s) of {source} into {target}",
        additions.len()
    );
    let edits = if edits.is_empty() {
        Vec::new()
    } else {
        vec![(source_index, edits)]
    };
    ws.change_resources(game, &label, &edits, &additions)?;
    Ok(Cloned {
        source,
        target,
        clones: list,
        remap,
        retargeted,
        label,
    })
}

/// [`add_resource`]: a new resource from a file of raw data.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AddResourceParams {
    /// The stream it goes into: of the game or new.
    pub stream: String,
    /// Resource type: a tag such as `lh2` or a hexadecimal id.
    #[serde(rename = "type")]
    pub type_tag: String,
    /// File with the raw data.
    pub file: PathBuf,
    /// One hexadecimal id or four separated by dots; a new unused id when
    /// `None`.
    #[serde(default)]
    pub ids: Option<String>,
    /// In a game stream: before this resource index; at the end when `None`.
    #[serde(default)]
    pub before: Option<usize>,
    /// Memory policy name or hash; the one of the type when `None`.
    #[serde(default)]
    pub policy: Option<String>,
    #[serde(default)]
    pub alignment: Option<u32>,
    /// Store RefPack-compressed.
    #[serde(default)]
    pub compress: bool,
}

/// Records a new resource read from a file. Returns its key.
pub fn add_resource(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: &mut Workspace,
    p: &AddResourceParams,
) -> Result<ResourceKey> {
    let data = std::fs::read(&p.file).map_err(|e| Error::io(&p.file, e))?;
    let type_id = create::parse_type(p.type_tag.trim())?;
    let stream = any_stream(game, ws, &p.stream)?;
    let ids = match p.ids.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        Some(text) => create::parse_ids(text)?,
        None => {
            let mut used = ws.used_ids(index);
            let seed = format!("{stream}:{type_id:08x}:{}", p.file.display());
            used.new_ids(IdForm::for_type(type_id), type_id, &[0; 4], &seed)
        }
    };
    let defaults = create::type_defaults(type_id);
    let mem_policy = match p.policy.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        Some(text) => create::parse_mem_policy(text)?,
        None => defaults.mem_policy,
    };
    let before = match p.before {
        None => None,
        Some(_) if ws.new_stream(&stream).is_some() => {
            return Err(bad(format!(
                "{stream} is new: its resources are written in the order they were added, \
                 a place cannot be asked for"
            )));
        }
        Some(at) => {
            let s = game
                .find_stream(&stream)
                .ok_or_else(|| bad(format!("stream `{stream}` not found")))?;
            let file = game.open_stream(s)?;
            let r = file
                .layout()
                .resources
                .get(at)
                .ok_or_else(|| bad(format!("{stream} has no resource {at}")))?;
            Some(ResourceKey::new(&stream, &r.header))
        }
    };
    let header = ResourceHeader {
        mem_policy,
        alignment: p.alignment.unwrap_or(defaults.alignment),
        type_id,
        ids,
        data_size: 0,
    };
    let key = ResourceKey::new(&stream, &header);
    let label = format!("Add {key} from {}", p.file.display());
    ws.add_resources(
        game,
        &label,
        &[NewResourceEdit {
            stream,
            header,
            data,
            storage: if p.compress {
                StorageChoice::Refpack
            } else {
                StorageChoice::Raw
            },
            before,
        }],
    )?;
    Ok(key)
}

// ------------------------------------------------------------ models

/// [`plan_model_import`]: a model file as a new model set or over a model.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ImportModelParams {
    /// The model file: glTF, FBX, OBJ or USD.
    pub file: PathBuf,
    /// The model it replaces, or the template of a new set:
    /// `<stream>#<resource>` of a mesh or a metamodel.
    pub template: String,
    /// A new model set with this name; the template is replaced when `None`.
    #[serde(default)]
    pub name: Option<String>,
    /// With `name`: the stream the new set goes into (of the game or new);
    /// the template's stream when `None`.
    #[serde(default)]
    pub stream: Option<String>,
    /// New collision built from the imported geometry: `mesh`, `hull` or
    /// `box`.
    #[serde(default)]
    pub collision: Option<String>,
    /// With `name`: resources of the template's stream to point at the set.
    #[serde(default)]
    pub retarget: Vec<usize>,
    /// Vertex format of every draw record.
    #[serde(default)]
    pub format: Option<u8>,
}

/// The collision kind of a name; `None` for nothing or `none`.
pub fn parse_collision(text: Option<&str>) -> Result<Option<CollisionKind>> {
    match text
        .map(str::trim)
        .filter(|t| !t.is_empty() && *t != "none")
    {
        None => Ok(None),
        Some(kind) => kind
            .parse()
            .map(Some)
            .map_err(|e| bad(format!("collision: {e} (mesh, hull or box)"))),
    }
}

/// Works out a model import ([`model::plan`]) into the stream the
/// parameters name ([`model::ImportPlan::put_into`]). Nothing is recorded:
/// [`model::ImportPlan::commit`] does that.
pub fn plan_model_import(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: &Workspace,
    assets: &mut GameAssets,
    scene: &crate::scene::Scene,
    p: &ImportModelParams,
) -> Result<model::ImportPlan> {
    let at = parse_location(game, &p.template)?;
    let name = p.name.clone().filter(|n| !n.trim().is_empty());
    let into = p
        .stream
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(|s| any_stream(game, ws, s))
        .transpose()?;
    if into.is_some() && name.is_none() {
        return Err(bad(
            "only a new model set goes into another stream: give it a name",
        ));
    }
    let mut plan = model::plan(
        game,
        ws,
        index,
        assets,
        at.stream,
        at.resource,
        scene,
        &model::ImportOptions {
            new_name: name,
            retarget: p.retarget.clone(),
            format: p.format,
            collision: parse_collision(p.collision.as_deref())?,
            this_stream_only: false,
            keep_copies: Vec::new(),
        },
    )?;
    if let Some(into) = &into {
        plan.put_into(game, ws, into)?;
    }
    Ok(plan)
}
