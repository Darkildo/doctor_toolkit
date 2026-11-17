//! What the mouse does in the level view, said as data (stage 104): several
//! objects moved, copied or removed in one operation; an object put on the
//! ground; the point on the ground a ray hits; a walk from A to B over the
//! collision. The collision world is the library's (`level::walk`), the
//! same one the level view of the editor builds: End, a dropped model and
//! the Walk panel there are these queries.

use std::sync::Arc;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use doctor_toolkit::level::edit::GroupAt;
use doctor_toolkit::level::walk::build::Area;
use doctor_toolkit::level::walk::physics::BodySize;
use doctor_toolkit::level::walk::{REACH, Scene, SceneOptions, ground_point};
use doctor_toolkit::level::{RowMatrix, TYPE_SIMGROUP, many};
use doctor_toolkit::resource::simgroup::Guid;
use doctor_toolkit::resource::simgroup_edit::Transform;
use doctor_toolkit::workspace::Workspace;

use super::create::last_label;
use super::edit::{
    DuplicateObjectParams, MoveObjectParams, RemoveObjectParams, finish, group, object, object_text,
};
use super::{Output, object_id};
use crate::state::{ToolError, ToolResult, Toolkit, fail};

/// Streams a collision world may be built from at once.
const MAX_STREAMS: usize = 60;
/// Collision farther than this from the place of interest is left out.
const AREA: f32 = 300.0;

// ------------------------------------------------------------ parameters

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct GroundParams {
    /// Zones whose collision counts: streams with SimGroups
    /// (`gf2/miami/local/bonita/high/terrain06`) or prefixes of such
    /// streams (`gf2/miami/local/bonita/high`). The `medium` zones of
    /// their districts, where the game keeps the terrain collision, are
    /// taken along.
    pub streams: Vec<String>,
    /// A point `[x, y, z]` (Y is up): the ground straight below it (or,
    /// when nothing is below, the one it is under).
    #[serde(default)]
    pub point: Option<[f32; 3]>,
    /// Or a ray, as from a camera through the pointer: its origin.
    #[serde(default)]
    pub ray_origin: Option<[f32; 3]>,
    /// The direction of the ray (any length).
    #[serde(default)]
    pub ray_direction: Option<[f32; 3]>,
    /// Read the game files only, without the edits of the workspace.
    #[serde(default)]
    pub original: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct WalkRouteParams {
    /// Zones whose collision counts: streams with SimGroups or prefixes of
    /// such streams; the `medium` zones of their districts (the terrain)
    /// are taken along.
    pub streams: Vec<String>,
    /// Where the player starts `[x, y, z]`: put on the ground below first.
    pub from: [f32; 3],
    /// Where the player walks to, in a straight line.
    pub to: [f32; 3],
    /// Height of the player, metres (default 1.8).
    #[serde(default)]
    pub height: Option<f32>,
    /// Radius of the player's capsule, metres (default 0.35).
    #[serde(default)]
    pub radius: Option<f32>,
    /// Highest step climbed without a jump, metres (default 0.45).
    #[serde(default)]
    pub step: Option<f32>,
    /// Read the game files only, without the edits of the workspace.
    #[serde(default)]
    pub original: Option<bool>,
}

// ------------------------------------------------------------ the world

/// Streams with SimGroups named by `texts`: exact names or prefixes.
fn zone_streams(kit: &Toolkit, texts: &[String]) -> ToolResult<Vec<usize>> {
    let mut out: Vec<usize> = Vec::new();
    for text in texts {
        let want = text.trim().replace('\\', "/").to_ascii_lowercase();
        let want = want.trim_end_matches(".str").trim_end_matches('/');
        if want.is_empty() || want.contains("..") {
            return fail(format!("`{text}` is not a stream or a prefix of streams"));
        }
        let exact = kit.game.find_stream(want);
        let before = out.len();
        for e in &kit.resources().entries {
            if e.header.type_id != TYPE_SIMGROUP {
                continue;
            }
            let s = e.stream as usize;
            let matches = match exact {
                Some(x) => s == x,
                None => kit.game.streams()[s]
                    .stream_name()
                    .to_ascii_lowercase()
                    .starts_with(&format!("{want}/")),
            };
            if matches && !out.contains(&s) {
                out.push(s);
            }
        }
        if out.len() == before && exact.is_none() {
            return fail(format!(
                "no stream with SimGroups at `{text}`; list_streams lists streams"
            ));
        }
    }
    if out.len() > MAX_STREAMS {
        return fail(format!(
            "{} streams: narrow the zones to at most {MAX_STREAMS}",
            out.len()
        ));
    }
    Ok(out)
}

fn load_scene(
    kit: &Toolkit,
    ws: Option<&Workspace>,
    streams: &[usize],
    centre: [f32; 3],
    reach: f32,
) -> ToolResult<Scene> {
    if streams.is_empty() {
        return fail("no zone to take the collision from: name the streams whose collision counts");
    }
    let game: &Arc<_> = &kit.game;
    Ok(Scene::load(
        game,
        kit.resources(),
        ws,
        streams,
        &SceneOptions {
            ground: true,
            area: Some(Area {
                centre,
                radius: AREA + reach,
            }),
        },
    )?)
}

/// The object of a level as the tools name it, with its class.
fn owner_json(kit: &Toolkit, scene: &Scene, layer: usize, at: (usize, usize)) -> Value {
    let level = if layer == 0 {
        Some(&scene.level)
    } else {
        scene.ground.as_ref()
    };
    let Some(group) = level.and_then(|l| l.groups.get(at.0)) else {
        return Value::Null;
    };
    let o = group.group.objects.get(at.1);
    json!({
        "object": object_id(kit, group.location, at.1),
        "class": o.and_then(|o| o.class_name()),
        "name": o.and_then(|o| group.group.object_name(o)),
    })
}

fn scene_stats(scene: &Scene) -> Value {
    json!({
        "collisions_placed": scene.stats.built,
        "colliders": scene.stats.colliders,
        "ground_zones": scene.ground.as_ref().map_or(0, |g| g.streams.len()),
        "not_readable": scene.stats.failed,
    })
}

// ------------------------------------------------------------ ground

pub fn ground(kit: &Toolkit, p: GroundParams) -> ToolResult<Output> {
    let streams = zone_streams(kit, &p.streams)?;
    let ws = kit.workspace_unless(p.original.unwrap_or(false))?;
    match (p.point, p.ray_origin, p.ray_direction) {
        (Some(point), None, None) => {
            let scene = load_scene(kit, ws.as_ref(), &streams, point, 0.0)?;
            let below = scene
                .world
                .below([point[0], point[1] + 0.05, point[2]], 2000.0);
            let (toi, owner, from_above) = match below {
                Some((toi, owner)) => (toi - 0.05, owner, false),
                None => match scene
                    .world
                    .below([point[0], point[1] + 1000.0, point[2]], 2000.0)
                {
                    Some((toi, owner)) => (toi - 1000.0, owner, true),
                    None => {
                        return Ok(Output::Json(json!({
                            "ground": null,
                            "note": "no collision below or above the point in these zones",
                            "world": scene_stats(&scene),
                        })));
                    }
                },
            };
            Ok(Output::Json(json!({
                "ground": [point[0], point[1] - toi, point[2]],
                "height_above_ground": toi,
                "point_is_below_the_ground": from_above,
                "on": owner.map(|o| owner_json(kit, &scene, o.layer, o.object)),
                "world": scene_stats(&scene),
            })))
        }
        (None, Some(origin), Some(direction)) => {
            let len = direction.iter().map(|c| c * c).sum::<f32>().sqrt();
            if len < 1e-6 {
                return fail("`ray_direction` has no length");
            }
            let dir = direction.map(|c| c / len);
            // The world is built around the middle of the reach worth looking at.
            let reach = 400.0;
            let centre = [
                origin[0] + dir[0] * reach / 2.0,
                origin[1] + dir[1] * reach / 2.0,
                origin[2] + dir[2] * reach / 2.0,
            ];
            let scene = load_scene(kit, ws.as_ref(), &streams, centre, reach / 2.0)?;
            let hit = scene.world.cast(origin, dir, REACH);
            let ground = ground_point(&scene.world, origin, dir, REACH);
            Ok(Output::Json(json!({
                "ground": ground,
                "hit": hit.map(|(toi, normal)| json!({
                    "point": [
                        origin[0] + dir[0] * toi,
                        origin[1] + dir[1] * toi,
                        origin[2] + dir[2] * toi,
                    ],
                    "distance": toi,
                    "normal": normal,
                })),
                "note": if ground.is_some() {
                    "`ground` is where an object dropped along this ray stands: the hit \
                     itself when it faces up, else the ground in front of the wall or under the roof"
                } else {
                    "the ray hits no collision in these zones"
                },
                "world": scene_stats(&scene),
            })))
        }
        _ => fail("give `point`, or `ray_origin` with `ray_direction`"),
    }
}

pub fn walk_route(kit: &Toolkit, p: WalkRouteParams) -> ToolResult<Output> {
    let streams = zone_streams(kit, &p.streams)?;
    let ws = kit.workspace_unless(p.original.unwrap_or(false))?;
    let centre = [
        (p.from[0] + p.to[0]) / 2.0,
        (p.from[1] + p.to[1]) / 2.0,
        (p.from[2] + p.to[2]) / 2.0,
    ];
    let length = ((p.to[0] - p.from[0]).powi(2) + (p.to[2] - p.from[2]).powi(2)).sqrt();
    if length > 2000.0 {
        return fail("the route is longer than 2 km: check it in parts");
    }
    let scene = load_scene(kit, ws.as_ref(), &streams, centre, length / 2.0)?;
    let mut body = BodySize::default();
    if let Some(h) = p.height {
        body.eye *= h / body.height;
        body.height = h;
    }
    if let Some(r) = p.radius {
        body.radius = r;
    }
    if let Some(s) = p.step {
        body.step = s;
    }
    if !(body.height > 2.0 * body.radius && body.radius > 0.0 && body.step >= 0.0) {
        return fail("the height must be more than twice the radius, and both positive");
    }
    if scene.stats.colliders == 0 {
        return fail(format!(
            "these zones have no collision to walk on ({} placed objects with collision); name \
             the zones of the place (`ground` shows what is under a point)",
            scene.stats.placements
        ));
    }
    let report = scene.walk(&body, p.from, p.to);
    // At most 60 points of the path: enough to see where it went.
    let every = report.path.len().div_ceil(60).max(1);
    let path: Vec<[f32; 3]> = report
        .path
        .iter()
        .enumerate()
        .filter(|(i, _)| i % every == 0 || *i + 1 == report.path.len())
        .map(|(_, p)| *p)
        .collect();
    Ok(Output::Json(json!({
        "outcome": report.outcome,
        "reached": report.outcome == "reached",
        "end": report.end,
        "target_on_the_ground": report.target,
        "end_above_target": report.end[1] - report.target[1],
        "walked_metres": report.walked,
        "straight_metres": length,
        "seconds": report.seconds,
        "blocked_by": report
            .blocked_by
            .map(|o| owner_json(kit, &scene, 0, o)),
        "path": path,
        "meaning": match report.outcome {
            "reached" => "the player walks from A to B in a straight line (B is reached on \
                          the plan: `end_above_target` says how far above or below it the feet are)",
            "stuck" => "the player pushed on at `end` for a second without getting nearer: \
                        a wall, a step too high or a slope too steep is in the way",
            "fell" => "the player fell below all collision at `end`: there is no ground there",
            _ => "the walk took three times the time of the straight line: the way is far from straight",
        },
        "body": { "height": body.height, "radius": body.radius, "step": body.step },
        "world": scene_stats(&scene),
    })))
}

// ------------------------------------------------------------ objects

/// The objects of a call: `object`, or the list `objects`.
fn named(one: &Option<String>, many: &[String]) -> ToolResult<Vec<String>> {
    match (one, many.is_empty()) {
        (Some(_), false) => fail("give `object` or `objects`, not both"),
        (Some(one), true) => Ok(vec![one.clone()]),
        (None, false) => Ok(many.to_vec()),
        (None, true) => fail("give `object` (one) or `objects` (a list)"),
    }
}

/// The streams whose collision counts for objects: those of the game
/// SimGroups among them, and `zones`.
fn object_streams(kit: &Toolkit, groups: &[GroupAt], zones: &[String]) -> ToolResult<Vec<usize>> {
    let mut streams = zone_streams(kit, zones)?;
    for at in groups {
        if let GroupAt::Game(l) = at
            && !streams.contains(&l.stream)
        {
            streams.push(l.stream);
        }
    }
    if streams.is_empty() {
        return fail(
            "`on_ground` needs the zones whose collision counts: the objects are in SimGroups \
             the workspace added, so name the streams in `zones`",
        );
    }
    Ok(streams)
}

/// How far `position` is above the ground of the zones (negative: below).
pub(crate) fn drop_new(
    kit: &Toolkit,
    ws: &Workspace,
    group: &GroupAt,
    zones: &[String],
    position: [f32; 3],
) -> ToolResult<f32> {
    let streams = object_streams(kit, std::slice::from_ref(group), zones)?;
    let scene = load_scene(kit, Some(ws), &streams, position, 0.0)?;
    scene.drop_height(None, position, position).ok_or_else(|| {
        ToolError(format!(
            "`on_ground`: no collision below or above {position:?} in the zones; `ground` shows \
             what a zone has, `zones` names more of them"
        ))
    })
}

pub fn move_objects(kit: &Toolkit, p: MoveObjectParams) -> ToolResult<Output> {
    let texts = named(&p.object, &p.objects)?;
    let on_ground = p.on_ground.unwrap_or(false);
    if p.position.is_none()
        && p.offset.is_none()
        && p.rotation.is_none()
        && p.scale.is_none()
        && !on_ground
    {
        return fail("give `position`, `offset`, `rotation`, `scale` or `on_ground`");
    }
    if p.position.is_some() && p.offset.is_some() {
        return fail("give `position` or `offset`, not both");
    }
    if p.position.is_some() && texts.len() > 1 {
        return fail("`position` puts one object: move several with `offset`");
    }
    let mut access = kit.write_workspace(&format!("move_object: {}", texts.join(", ")))?;
    let ws = &mut access.ws;
    // Every object and its new matrix, before anything is recorded.
    let mut planned: Vec<(GroupAt, usize, RowMatrix, RowMatrix)> = Vec::new();
    for text in &texts {
        let (at, o) = object(kit, ws, text)?;
        let g = group(kit, ws, &at)?;
        let item = g.objects.get(o).ok_or_else(|| {
            ToolError(format!(
                "{text}: the SimGroup has {} object(s)",
                g.objects.len()
            ))
        })?;
        let current = g.transform(item).ok_or_else(|| {
            ToolError(format!(
                "{text} has no transform (CMD_LoadMatrix): it is not placed in the world"
            ))
        })?;
        let mut m = current;
        if p.rotation.is_some() || p.scale.is_some() {
            let mut t = Transform::from_matrix(&current);
            if let Some(r) = p.rotation {
                t.rotation = r;
            }
            if let Some(s) = p.scale {
                t.scale = s;
            }
            m = t.to_matrix();
            m[3] = current[3];
        }
        if let Some(pos) = p.position {
            m[3][..3].copy_from_slice(&pos);
        }
        if let Some(d) = p.offset {
            for (i, v) in d.iter().enumerate() {
                m[3][i] += v;
            }
        }
        if planned.iter().any(|(a, b, _, _)| *a == at && *b == o) {
            return fail(format!("{text} is named twice"));
        }
        planned.push((at, o, current, m));
    }
    let mut no_ground: Vec<String> = Vec::new();
    let mut world = Value::Null;
    if on_ground {
        let groups: Vec<GroupAt> = planned.iter().map(|(at, ..)| at.clone()).collect();
        let streams = object_streams(kit, &groups, &p.zones)?;
        let mut centre = [0.0f32; 3];
        for (_, _, _, m) in &planned {
            for k in 0..3 {
                centre[k] += m[3][k] / planned.len() as f32;
            }
        }
        let reach = planned
            .iter()
            .map(|(_, _, _, m)| {
                ((m[3][0] - centre[0]).powi(2) + (m[3][2] - centre[2]).powi(2)).sqrt()
            })
            .fold(0.0f32, f32::max);
        let scene = load_scene(kit, Some(ws), &streams, centre, reach)?;
        world = scene_stats(&scene);
        for (i, (at, o, current, m)) in planned.iter_mut().enumerate() {
            let in_level = match at {
                GroupAt::Game(l) => scene.object(*l, *o),
                GroupAt::Added(_) => None,
            };
            let origin = [current[3][0], current[3][1], current[3][2]];
            let target = [m[3][0], m[3][1], m[3][2]];
            match scene.drop_height(in_level, origin, target) {
                Some(dy) => m[3][1] += dy,
                None => no_ground.push(texts[i].clone()),
            }
        }
        if no_ground.len() == planned.len() {
            return fail(format!(
                "`on_ground`: no collision below or above {} in the zones ({} collisions placed); \
                 `zones` names more streams, `ground` shows what is under a point",
                no_ground.join(", "),
                scene.stats.built
            ));
        }
    }
    let since = ws.head();
    let moves: Vec<(GroupAt, usize, RowMatrix)> = planned
        .iter()
        .map(|(at, o, _, m)| (at.clone(), *o, *m))
        .collect();
    many::transform_many(&kit.game, ws, &moves)?;
    let label = last_label(ws);
    let rows: Vec<Value> = planned
        .iter()
        .zip(&texts)
        .map(|((_, _, old, new), text)| {
            json!({
                "object": text,
                "old_position": &old[3][..3],
                "new_position": &new[3][..3],
            })
        })
        .collect();
    finish(
        ws,
        since,
        &label,
        json!({
            "moved": rows,
            "no_ground_under": no_ground,
            "world": world,
        }),
    )
}

pub fn duplicate_objects(kit: &Toolkit, p: DuplicateObjectParams) -> ToolResult<Output> {
    let texts = named(&p.object, &p.objects)?;
    let taken = kit.object_guids();
    let mut access = kit.write_workspace(&format!("duplicate_object: {}", texts.join(", ")))?;
    let ws = &mut access.ws;
    let mut objects = Vec::new();
    for text in &texts {
        let (at, o) = object(kit, ws, text)?;
        group(kit, ws, &at)?;
        objects.push((at, o));
    }
    let since = ws.head();
    let copies = many::duplicate_many(
        &kit.game,
        ws,
        &objects,
        p.offset.unwrap_or_default(),
        &|g: &Guid| taken.contains(g),
    )?;
    let label = last_label(ws);
    let rows: Vec<Value> = copies
        .iter()
        .zip(&texts)
        .map(|((at, copy, guid), text)| {
            json!({
                "object": text,
                "copy": object_text(kit, at, *copy),
                "guid": guid.to_string(),
            })
        })
        .collect();
    finish(ws, since, &label, json!({ "copies": rows }))
}

pub fn remove_objects(kit: &Toolkit, p: RemoveObjectParams) -> ToolResult<Output> {
    let texts = named(&p.object, &p.objects)?;
    let mut access = kit.write_workspace(&format!("remove_object: {}", texts.join(", ")))?;
    let ws = &mut access.ws;
    let mut objects = Vec::new();
    for text in &texts {
        let (at, o) = object(kit, ws, text)?;
        let g = group(kit, ws, &at)?;
        if o >= g.objects.len() {
            return fail(format!(
                "{text}: the SimGroup has {} object(s)",
                g.objects.len()
            ));
        }
        objects.push((at, o));
    }
    let since = ws.head();
    let dangling = many::remove_many(&kit.game, ws, &objects)?;
    let label = last_label(ws);
    finish(
        ws,
        since,
        &label,
        json!({
            "removed": texts,
            "dangling_references": dangling,
            "note": "the objects after a removed one in its SimGroup moved up: read \
                     level_objects again before naming objects of these SimGroups",
        }),
    )
}
