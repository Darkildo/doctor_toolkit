//! A copy of a place of the game somewhere else: every resource and object of
//! a few streams (a venue with its interior, say), moved by an offset, in new
//! streams of their own (docs/levels.md §7.1, game-bench.md §56).
//!
//! - Each source stream becomes a new stream with a cell of the world grid
//!   like the source's (kind, flags, level of detail), moved; an interior
//!   cell named after the venue (`venue_026`) is named after the new venue,
//!   and a `ZoneStreamEnable` box of that name is added: the game does not
//!   turn on the interior cell of a new venue itself.
//! - The resources are cloned under new ids ([`create::clone_resources`]):
//!   models, collision, materials and textures of the copy are its own, and
//!   the references between them follow the clones.
//! - The objects of each SimGroup are copied with new GUIDs (links between
//!   them follow), moved by the offset, and pointed at the cloned resources.
//! - Objects of other streams (`also`: the shell of a venue in the `medium`
//!   zone of its district) go with their models into a stream of their own
//!   with a cell of the source's level of detail; a `Building` goes into the
//!   first stream, with the interior.
//! - Messages sent and heard inside the copied groups get a prefix, so that
//!   the copy and the original do not hear each other; messages of the rest
//!   of the game (music, engine) are kept.
//! - With a venue number, the copy is a new venue: `Enum<venue>` attributes
//!   get the new number, the venue table gets a record made after the old
//!   one and moved (the game places the collision of a venue's interior by
//!   it), and the copied Building the GUID of that record.
//! - Navigation graphs are moved with their nodes (links into the graphs of
//!   the original place are dropped); the points of `covernodetable` are
//!   moved and get new GUIDs.

use std::collections::{BTreeSet, HashMap};

use crate::container::ustream::ResourceHeader;
use crate::create::{self, CloneOptions, Remap};
use crate::game::StreamFile;
use crate::game::{GameInstance, ResourceIndex};
use crate::game_types::{SemanticType, attr_type};
use crate::level::world_grid::{self, ZoneCell};
use crate::logic_graph::schema::LogicSchema;
use crate::resource::graph::{Graph, TYPE_GRAPH};
use crate::resource::points::TYPE_POINTS;
use crate::resource::simgroup::{Guid, SimGroup, Value};
use crate::resource::wgi;
use crate::workspace::key::normalize_stream;
use crate::workspace::{NewResourceEdit, StorageChoice, Workspace};
use crate::zone_ops::{self, NewStreamParams};
use crate::{Error, Result};

use super::TYPE_SIMGROUP;
use super::edit::new_guid;

/// What to copy and where.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneCopy {
    /// Streams to copy, e.g. `gf2/queens/local/hank/high/venue_026`.
    pub from: Vec<String>,
    /// The new stream.
    pub to: String,
    /// The stream the new one depends on (`gf2/queens/local`).
    pub parent: String,
    /// The level root stream whose world grid gets a cell (`gf2/queens`).
    pub level: String,
    pub offset: [f32; 3],
    /// Venue number of the original and of the copy.
    pub venue: Option<(i32, i32)>,
    /// Prefix of the messages of the copy; none: kept.
    pub prefix: Option<String>,
    /// Objects of other streams to copy along, `stream#resource:object`:
    /// a venue's shell, which the game keeps in the `medium` zone of its
    /// district (`ZoneRenderLOD` with the venue's number). Their models are
    /// cloned with every resource of their stream they refer to.
    pub also: Vec<String>,
    /// Owner of the new venue (sdbm of the family name) [default: that of
    /// the original].
    pub owner: Option<u32>,
}

/// What [`copy_zone`] made.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ZoneCopied {
    pub stream: String,
    pub resources: usize,
    pub groups: usize,
    pub objects: usize,
    pub moved: usize,
    pub venues: usize,
    /// Messages renamed (old name, new name).
    pub messages: Vec<(String, String)>,
    pub graphs: usize,
    pub points: usize,
    pub cell: Option<([f32; 3], [f32; 3])>,
    /// GUID of the record of the new venue (`bricknmortarres`).
    pub venue_guid: Option<String>,
}

fn bad(what: impl Into<String>) -> Error {
    Error::InvalidArgument(what.into())
}

fn moved(m: [[f32; 4]; 4], offset: [f32; 3]) -> [[f32; 4]; 4] {
    let mut m = m;
    for (axis, d) in offset.iter().enumerate() {
        m[3][axis] += d;
    }
    m
}

/// Message attributes of a group: (object, slot, index, name, sent).
fn message_attrs(g: &SimGroup) -> Vec<(usize, usize, u16, String, bool)> {
    let schema = LogicSchema::gf2();
    let mut out = Vec::new();
    for (o, object) in g.objects.iter().enumerate() {
        for (slot, &p) in object.packets.iter().enumerate() {
            let packet = &g.packets[p];
            for a in &packet.attrs {
                let Value::String(name) = &a.value else {
                    continue;
                };
                if name.is_empty() {
                    continue;
                }
                match schema.semantic(packet.class.0, a.index) {
                    Some((SemanticType::MsgIn, _)) => {
                        out.push((o, slot, a.index, name.clone(), false));
                    }
                    Some((SemanticType::MsgOut, _)) => {
                        out.push((o, slot, a.index, name.clone(), true));
                    }
                    _ => {}
                }
            }
        }
    }
    out
}

/// A copy of the objects of `source` with new GUIDs; GUID attributes and
/// links between the copies follow them.
fn copy_group(
    source: &SimGroup,
    group_hash: u32,
    seed: &str,
    taken: &dyn Fn(&Guid) -> bool,
) -> Result<SimGroup> {
    let mut g = SimGroup::empty();
    g.group_hash = group_hash;
    g.version = source.version;
    g.flags = source.flags;
    let mut map: HashMap<Guid, Guid> = HashMap::new();
    for o in 0..source.objects.len() {
        let new = new_guid(&g, &format!("{seed}/{o}"), taken);
        g.copy_object_from(source, o, new)?;
        map.insert(source.objects[o].guid, new);
    }
    for o in 0..g.objects.len() {
        let mut sets = Vec::new();
        for (slot, &p) in g.objects[o].packets.iter().enumerate() {
            for a in &g.packets[p].attrs {
                if let Value::Guid(old) = a.value
                    && let Some(new) = map.get(&old)
                {
                    sets.push((slot, a.index, *new));
                }
            }
        }
        for (slot, index, new) in sets {
            g.set_attr(o, slot, index, Some(Value::Guid(new)))?;
        }
        let links: Vec<(usize, Guid)> = g.objects[o]
            .links
            .iter()
            .enumerate()
            .filter_map(|(i, l)| map.get(l).map(|n| (i, *n)))
            .collect();
        for (i, new) in links {
            g.set_link(o, i, new)?;
        }
    }
    Ok(g)
}

/// Moves a navigation graph by `offset`, gives it the GUID of its new
/// header and drops its links into other graphs (they lead to the place
/// the copy was made from).
fn move_graph(data: &[u8], header: &ResourceHeader, offset: [f32; 3]) -> Result<Vec<u8>> {
    let mut graph = Graph::parse(data)?;
    graph.guid = Guid(create::guid_bytes(&header.ids));
    for node in &mut graph.nodes {
        for (axis, d) in offset.iter().enumerate() {
            node.position[axis] += d;
        }
        node.neighbour_links.clear();
    }
    graph.neighbours.clear();
    graph.external_neighbours.clear();
    graph.rebuild_rtree();
    graph.to_bytes()
}

/// Moves the points of a `covernodetable` by `offset` and gives them new
/// GUIDs (the last eight bytes, the author's machine, are kept).
fn move_points(data: &mut [u8], offset: [f32; 3], salt: u32) -> Result<usize> {
    let table = crate::resource::points::PointTable::parse(data)?;
    for (k, point) in table.points.iter().enumerate() {
        let at = point.offset;
        for (base, count) in [(0x34usize, 1usize), (0x58, 2)] {
            for v in 0..count {
                for (axis, d) in offset.iter().enumerate() {
                    let o = at + base + 12 * v + 4 * axis;
                    let x = f32::from_le_bytes(data[o..o + 4].try_into().expect("4 bytes")) + d;
                    data[o..o + 4].copy_from_slice(&x.to_le_bytes());
                }
            }
        }
        let word = crate::hash::sdbm(format!("{salt:08x}/{k}").as_bytes());
        data[at..at + 4].copy_from_slice(&word.to_le_bytes());
    }
    Ok(table.points.len())
}

/// `stream#resource:object` of [`ZoneCopy::also`].
fn parse_object(text: &str) -> Result<(String, usize, usize)> {
    let bad_form = || bad(format!("{text}: stream#resource:object expected"));
    let (stream, rest) = text.trim().split_once('#').ok_or_else(bad_form)?;
    let (resource, object) = rest.split_once(':').ok_or_else(bad_form)?;
    Ok((
        normalize_stream(stream),
        resource.trim().parse().map_err(|_| bad_form())?,
        object.trim().parse().map_err(|_| bad_form())?,
    ))
}

/// The resources of `stream` a model needs, the model first: what its
/// references reach inside the stream (meshes, collision, nested models,
/// materials, textures). What the stream does not have stays shared.
fn model_closure(stream: &StreamFile, model: u32) -> Result<Vec<usize>> {
    use crate::create::Target;
    let resources = &stream.layout().resources;
    let mut material_at: HashMap<u32, usize> = HashMap::new();
    for (i, r) in resources.iter().enumerate() {
        if r.header.type_id == create::TYPE_MATERIAL {
            let data = stream.resource_data(i)?;
            if data.len() >= 12 {
                let hash = u32::from_le_bytes(data[8..12].try_into().expect("4 bytes"));
                material_at.insert(hash, i);
            }
        }
    }
    let start = resources
        .iter()
        .position(|r| r.header.ids == [model; 4])
        .ok_or_else(|| {
            bad(format!(
                "model {model:08x} is not in the stream of its object"
            ))
        })?;
    let mut out = vec![start];
    let mut k = 0;
    while k < out.len() {
        let i = out[k];
        k += 1;
        let header = &resources[i].header;
        let data = stream.resource_data(i)?;
        let mut targets = create::references(header.type_id, &data)?;
        // `create::references` leaves out the references of type `hka`; a
        // venue's shell names its collision so, and the `hkx` of the stream
        // with that id is it (`7608f847` of hank/medium/zone4 → hkx
        // `17efee6c`): without it the copy crashes the game (0x008CAE17).
        if header.type_id == create::TYPE_METAMODEL {
            for r in crate::resource::metamodel::resource_refs(&data)? {
                targets.push(Target::Guid(r.guid()));
            }
        }
        for target in targets {
            let found: Vec<usize> = match target {
                Target::Guid(g) => resources
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.header.ids == g)
                    .map(|(j, _)| j)
                    .collect(),
                Target::Material(h) => material_at.get(&h).copied().into_iter().collect(),
                Target::Texture(h) => resources
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| {
                        [
                            crate::resource::texture::TYPE_TG4H,
                            crate::resource::texture::TYPE_TG4D,
                        ]
                        .contains(&r.header.type_id)
                            && r.header.ids[3] == h
                    })
                    .map(|(j, _)| j)
                    .collect(),
            };
            for j in found {
                if !out.contains(&j) {
                    out.push(j);
                }
            }
        }
    }
    Ok(out)
}

/// A new stream of the copy; its table-of-contents hash.
fn make_stream(game: &GameInstance, ws: &mut Workspace, name: &str, parent: &str) -> Result<u32> {
    Ok(zone_ops::add_stream(
        game,
        ws,
        &NewStreamParams {
            name: name.to_owned(),
            parent: parent.to_owned(),
            load: Default::default(),
            tocs: Vec::new(),
            guid: None,
        },
    )?
    .hash)
}

/// The cell a stream of the copy gets.
#[derive(Debug, Clone)]
struct CellSpec {
    stream: String,
    kind: u8,
    flags: u8,
    lod: u32,
    name: Option<String>,
    /// The `wgi` resource of the source's cell.
    grid: Option<usize>,
    /// Moved box of the source's cell; none: the box of the whole copy.
    bounds: Option<([f32; 3], [f32; 3])>,
}

/// Copies the streams `p.from` (and the objects `p.also`) into new streams
/// named after `p.to` (see the module). Several operations of the
/// workspace: the streams, the venue record, the resources, the cells.
pub fn copy_zone(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: &mut Workspace,
    p: &ZoneCopy,
    taken: &dyn Fn(&Guid) -> bool,
) -> Result<ZoneCopied> {
    if p.from.is_empty() {
        return Err(bad("no stream to copy"));
    }
    let to = normalize_stream(p.to.trim());
    let mut cells: Vec<CellSpec> = Vec::new();
    // the name of the interior cell of a venue (`venue_026`), for the copy
    let venue_cell = p.venue.map(|(old, new)| {
        (
            crate::hash::sdbm(format!("venue_{old:03}").as_bytes()),
            format!("venue_{new:03}"),
        )
    });
    let mut report = ZoneCopied {
        stream: to.clone(),
        ..Default::default()
    };
    let mut used = ws.used_ids(index);
    let mut additions: Vec<NewResourceEdit> = Vec::new();
    // (stream, SimGroup): the copied groups, and one group for the objects of
    // each other stream of `also`, in a stream of its own
    let mut groups: Vec<(String, SimGroup)> = Vec::new();
    // other streams: (new stream, level-of-detail mask of the source's cell)
    let mut extra_streams: Vec<(String, u32)> = Vec::new();
    let mut boxes: Vec<([f32; 3], [f32; 3])> = Vec::new();
    let mut remap = Remap::default();

    for (i, from) in p.from.iter().enumerate() {
        let from = normalize_stream(from.trim());
        let source = game
            .find_stream(&from)
            .ok_or_else(|| bad(format!("no stream {from} in the game")))?;
        // each source in a stream of its own, with a cell like the source's
        let target = if i == 0 {
            to.clone()
        } else {
            format!("{to}_{}", cells.len())
        };
        let target_hash = make_stream(game, ws, &target, &p.parent)?;
        let hash = world_grid::stream_hash(game, ws, &from)?;
        let mut spec = CellSpec {
            stream: target.clone(),
            kind: wgi::KIND_WORLD,
            flags: 0,
            lod: wgi::lod::HIGH,
            name: None,
            grid: None,
            bounds: None,
        };
        for grid in world_grid::level_grids(game, ws, &p.level)? {
            if let Some(cell) = grid.grid.find_cell(hash) {
                let c = &grid.grid.cells[cell];
                let lo: [f32; 3] = std::array::from_fn(|a| c.box_min[a] + p.offset[a]);
                let hi: [f32; 3] = std::array::from_fn(|a| c.box_max[a] + p.offset[a]);
                boxes.push((lo, hi));
                spec.kind = c.kind;
                spec.flags = c.flags;
                spec.lod = c.lod_mask;
                spec.grid = Some(grid.resource);
                spec.bounds = Some((lo, hi));
                spec.name = match &venue_cell {
                    Some((old, new)) if c.name_hash.0 == *old => Some(new.clone()),
                    _ => None,
                };
                if c.name_hash.0 != 0 && spec.name.is_none() {
                    // a name the copy cannot carry: loaded by distance instead
                    spec.kind = wgi::KIND_WORLD;
                }
            }
        }
        cells.push(spec);
        let file = game.open_stream(source)?;
        let resources = &file.layout().resources;
        let indices: Vec<usize> = resources
            .iter()
            .enumerate()
            .filter(|(_, r)| r.header.type_id != TYPE_SIMGROUP)
            .map(|(i, _)| i)
            .collect();
        if !indices.is_empty() {
            let options = CloneOptions {
                seed: format!("{to}/{from}"),
                rewrite_references: true,
                storage: None,
            };
            let (clones, map) = create::clone_resources(&file, &indices, &mut used, &options)?;
            remap.guids.extend(map.guids);
            remap.names.extend(map.names);
            for c in clones {
                let mut data = c.data;
                match c.header.type_id {
                    TYPE_GRAPH => {
                        data = move_graph(&data, &c.header, p.offset)?;
                        report.graphs += 1;
                    }
                    TYPE_POINTS => {
                        report.points += move_points(&mut data, p.offset, c.header.ids[0])?;
                    }
                    _ => {}
                }
                additions.push(NewResourceEdit {
                    stream: target.clone(),
                    header: c.header,
                    data,
                    storage: StorageChoice::Refpack,
                    before: None,
                });
            }
        }
        for (r, entry) in resources.iter().enumerate() {
            if entry.header.type_id != TYPE_SIMGROUP {
                continue;
            }
            let data = ws.current_data(game, source, r)?;
            let original = SimGroup::parse(&data)?;
            let seed = format!("{to}/{from}#{r}");
            let copy = copy_group(&original, target_hash, &seed, &|g| {
                taken(g)
                    || groups
                        .iter()
                        .any(|(_, h)| h.objects.iter().any(|o| o.guid == *g))
            })?;
            report.objects += copy.objects.len();
            groups.push((target.clone(), copy));
        }
    }

    // Objects of other streams (a venue's shell) and their models. Each
    // source stream gets a new stream of its own with a cell of the same
    // level of detail: a model made for the `medium` grid in a `high` cell
    // crashes the game (0x008CAE17, game-bench.md §56).
    let mut sources: Vec<String> = Vec::new();
    for (n, text) in p.also.iter().enumerate() {
        let (stream_name, resource, object) = parse_object(text)?;
        let source = game
            .find_stream(&stream_name)
            .ok_or_else(|| bad(format!("no stream {stream_name} in the game")))?;
        // A venue's Building goes into the first stream of the copy, with the
        // venue's interior: from another stream the interior has no collision
        // (game-bench.md §56).
        {
            let data = ws.current_data(game, source, resource)?;
            let group = SimGroup::parse(&data)?;
            let o = group.objects.get(object).ok_or_else(|| {
                bad(format!(
                    "{text}: the SimGroup has {} object(s)",
                    group.objects.len()
                ))
            })?;
            if o.class == crate::hash::sdbm(b"building") {
                let (_, g) = groups
                    .iter_mut()
                    .find(|(s, _)| *s == to)
                    .ok_or_else(|| bad("a Building goes into a copied SimGroup: copy one"))?;
                let guid = new_guid(g, &format!("{to}/also/{n}"), taken);
                g.copy_object_from(&group, object, guid)?;
                report.objects += 1;
                continue;
            }
        }
        let k = match sources.iter().position(|s| *s == stream_name) {
            Some(k) => k,
            None => {
                let hash = world_grid::stream_hash(game, ws, &stream_name)?;
                let lod = world_grid::level_grids(game, ws, &p.level)?
                    .iter()
                    .find_map(|g| g.grid.find_cell(hash).map(|c| g.grid.cells[c].lod_mask))
                    .unwrap_or(wgi::lod::HIGH);
                let name = format!("{to}_{}", cells.len());
                let extra = make_stream(game, ws, &name, &p.parent)?;
                let mut g = SimGroup::empty();
                g.group_hash = extra;
                groups.push((name.clone(), g));
                cells.push(CellSpec {
                    stream: name.clone(),
                    kind: wgi::KIND_WORLD,
                    flags: 0,
                    lod,
                    name: None,
                    grid: None,
                    bounds: None,
                });
                extra_streams.push((name, lod));
                sources.push(stream_name.clone());
                sources.len() - 1
            }
        };
        let target = extra_streams[k].0.clone();
        let file = game.open_stream(source)?;
        let data = ws.current_data(game, source, resource)?;
        let group = SimGroup::parse(&data)?;
        let o = group.objects.get(object).ok_or_else(|| {
            bad(format!(
                "{text}: the SimGroup has {} object(s)",
                group.objects.len()
            ))
        })?;
        if let Some(model) = group.model(o) {
            let indices = model_closure(&file, model)?;
            let options = CloneOptions {
                seed: format!("{target}/{text}"),
                rewrite_references: true,
                storage: None,
            };
            let (clones, map) = create::clone_resources(&file, &indices, &mut used, &options)?;
            remap.guids.extend(map.guids);
            remap.names.extend(map.names);
            for c in clones {
                additions.push(NewResourceEdit {
                    stream: target.clone(),
                    header: c.header,
                    data: c.data,
                    storage: StorageChoice::Refpack,
                    before: None,
                });
            }
        }
        let (_, g) = groups
            .iter_mut()
            .find(|(s, _)| *s == target)
            .expect("the group of the stream was made above");
        if g.version == 0 {
            g.version = group.version;
            g.flags = group.flags;
        }
        let guid = new_guid(g, &format!("{target}/also/{n}"), taken);
        g.copy_object_from(&group, object, guid)?;
        report.objects += 1;
    }

    // Messages inside the copy: sent and heard by its own objects.
    let mut sent = BTreeSet::new();
    let mut heard = BTreeSet::new();
    for (_, g) in &groups {
        for (_, _, _, name, out) in message_attrs(g) {
            if out {
                sent.insert(name);
            } else {
                heard.insert(name);
            }
        }
    }
    let inner: BTreeSet<String> = sent.intersection(&heard).cloned().collect();
    if let Some(prefix) = &p.prefix {
        report.messages = inner
            .iter()
            .map(|n| (n.clone(), format!("{prefix}{n}")))
            .collect();
    }

    // The record of the new venue, moved with the copy: the game places the
    // collision of a venue's interior by it (game-bench.md §56).
    let mut venue_guid = None;
    if let Some((old, new)) = p.venue {
        let city = crate::level::venues::city_venues(game, ws, &p.level)?;
        let original = city
            .table
            .find(old)
            .ok_or_else(|| bad(format!("{} has no venue {old}", p.level)))?
            .position;
        let made = crate::level::venues::add_venue(
            game,
            ws,
            &p.level,
            &crate::level::venues::NewVenue {
                id: new,
                like: old,
                owner: p.owner,
                position: Some(std::array::from_fn(|a| original[a] + p.offset[a])),
                guid: None,
            },
        )?;
        venue_guid = Some(Guid(create::parse_guid(&made.guid)?));
        report.venue_guid = Some(made.guid);
    }

    // An interior cell of the copy (`venue_NNN`, kind 5) is loaded while the
    // player is inside a ZoneStreamEnable box of that name (as the bakery's,
    // venue_115): the game does not turn on the cell of a new venue itself.
    for spec in &cells {
        let (Some(name), Some((lo, hi))) = (&spec.name, spec.bounds) else {
            continue;
        };
        if spec.kind != 5 {
            continue;
        }
        let class = crate::hash::sdbm(b"zonestreamenable");
        let layout = LogicSchema::gf2()
            .layout(class)
            .ok_or_else(|| bad("the schema has no layout of ZoneStreamEnable"))?;
        let Some((_, g)) = groups.iter_mut().find(|(s, _)| *s == to) else {
            continue;
        };
        let guid = new_guid(g, &format!("{to}/enable/{name}"), taken);
        let o = g.push_object(class, guid, layout)?;
        // the box is the scale of the matrix, around its translation
        let mut m = [[0.0f32; 4]; 4];
        for axis in 0..3 {
            m[axis][axis] = (hi[axis] - lo[axis]) + 4.0;
            m[3][axis] = (lo[axis] + hi[axis]) / 2.0;
        }
        m[3][3] = 1.0;
        // set before the copy is moved: the box is already where the copy is
        let back: [[f32; 4]; 4] = moved(m, [-p.offset[0], -p.offset[1], -p.offset[2]]);
        g.set_transform(o, back)?;
        g.set_class_attr(o, class, 0, Some(Value::String(name.clone())))?;
        report.objects += 1;
    }

    for (k, (stream_of, g)) in groups.iter_mut().enumerate() {
        for o in 0..g.objects.len() {
            if let Some(m) = g.transform(&g.objects[o])
                && g.set_transform(o, moved(m, p.offset))?
            {
                report.moved += 1;
            }
        }
        if let Some(prefix) = &p.prefix {
            for (o, slot, index, name, _) in message_attrs(g) {
                if inner.contains(&name) {
                    g.set_attr(
                        o,
                        slot,
                        index,
                        Some(Value::String(format!("{prefix}{name}"))),
                    )?;
                }
            }
        }
        if let Some((old, new)) = p.venue {
            let mut sets = Vec::new();
            for (o, object) in g.objects.iter().enumerate() {
                for (slot, &q) in object.packets.iter().enumerate() {
                    let packet = &g.packets[q];
                    for a in &packet.attrs {
                        let venue = attr_type(packet.class.0, a.index)
                            .is_some_and(|t| t.ty == SemanticType::Enum("venue"));
                        if venue && a.value == Value::Int(old) {
                            sets.push((o, slot, a.index));
                        }
                    }
                }
            }
            for (o, slot, index) in sets {
                if g.set_attr(o, slot, index, Some(Value::Int(new)))? {
                    report.venues += 1;
                }
            }
        }
        // The Building of the new venue gets the GUID of its record.
        let mut sets = Vec::new();
        for (o, object) in g.objects.iter().enumerate() {
            for (slot, &q) in object.packets.iter().enumerate() {
                let packet = &g.packets[q];
                if let (Some(guid), Some((_, new))) = (venue_guid, p.venue)
                    && packet.class_name == Some("Building")
                    && packet
                        .attrs
                        .iter()
                        .any(|a| a.index == 0 && a.value == Value::Int(new))
                {
                    sets.push((o, slot, 5u16, Value::Guid(guid)));
                }
            }
        }
        for (o, slot, index, value) in sets {
            g.set_attr(o, slot, index, Some(value))?;
        }
        let mut data = g.encode();
        create::rewrite_references(TYPE_SIMGROUP, &mut data, &remap)?;
        let ids = used.new_ids(
            create::IdForm::for_type(TYPE_SIMGROUP),
            TYPE_SIMGROUP,
            &[0; 4],
            &format!("{to}:simgroup:{k}"),
        );
        let defaults = create::type_defaults(TYPE_SIMGROUP);
        additions.push(NewResourceEdit {
            stream: stream_of.clone(),
            header: ResourceHeader {
                mem_policy: defaults.mem_policy,
                alignment: defaults.alignment,
                type_id: TYPE_SIMGROUP,
                ids,
                data_size: 0,
            },
            data,
            storage: StorageChoice::Refpack,
            before: None,
        });
    }
    report.groups = groups.len();
    report.resources = additions.len();
    ws.add_resources(
        game,
        &format!("Copy {} into {to}", p.from.join(", ")),
        &additions,
    )?;

    if let Some(first) = boxes.first() {
        let mut lo = first.0;
        let mut hi = first.1;
        for (a, b) in &boxes[1..] {
            for axis in 0..3 {
                lo[axis] = lo[axis].min(a[axis]);
                hi[axis] = hi[axis].max(b[axis]);
            }
        }
        for spec in &cells {
            let (box_min, box_max) = spec.bounds.unwrap_or((lo, hi));
            world_grid::add_zone_cell_in(
                game,
                ws,
                &normalize_stream(&p.level),
                &ZoneCell {
                    stream: spec.stream.clone(),
                    guid: None,
                    box_min,
                    box_max,
                    lod_mask: spec.lod,
                    kind: spec.kind,
                    flags: spec.flags,
                    name: spec.name.clone(),
                },
                spec.grid,
            )?;
        }
        report.cell = Some((lo, hi));
    }
    Ok(report)
}
