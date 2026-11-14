//! `level_objects`: placed objects of SimGroups with positions and models.

use serde_json::{Value, json};

use doctor_toolkit::level::{Location, TYPE_SIMGROUP};
use doctor_toolkit::logic_graph::{LogicSchema, scope};
use doctor_toolkit::resource::simgroup::SimGroup;

use super::{LevelObjectsParams, Output, clamp, object_id};
use crate::state::{ToolResult, Toolkit, fail};

/// Streams a request may load at once.
const MAX_STREAMS: usize = 400;

pub fn level_objects(kit: &Toolkit, p: LevelObjectsParams) -> ToolResult<Output> {
    let resources = kit.resources();
    let want = p.stream.trim().replace('\\', "/").to_ascii_lowercase();
    let want = want.trim_end_matches(".str").trim_end_matches('/');
    if want.is_empty() || want.contains("..") {
        return fail("give a stream or a prefix of streams, e.g. gf2/miami/local/bonita");
    }
    let exact = kit.game.find_stream(want);
    let mut groups: Vec<Location> = Vec::new();
    let mut streams = std::collections::BTreeSet::new();
    for e in &resources.entries {
        if e.header.type_id != TYPE_SIMGROUP {
            continue;
        }
        let s = e.stream as usize;
        let matches = match exact {
            Some(x) => s == x,
            None => {
                let name = kit.game.streams()[s].stream_name().to_ascii_lowercase();
                name == want || name.starts_with(&format!("{want}/"))
            }
        };
        if matches {
            streams.insert(s);
            groups.push(Location {
                stream: s,
                resource: e.resource as usize,
            });
        }
    }
    let ws = kit.workspace_unless(p.original.unwrap_or(false))?;
    // SimGroups the workspace added and that are not in the game files yet
    // (in streams it creates, or in game streams before `apply`).
    let mut added: Vec<(String, SimGroup)> = Vec::new();
    for c in ws.iter().flat_map(|w| w.changes()) {
        let name = c.key.stream.to_ascii_lowercase();
        if c.is_added()
            && c.key.type_id == TYPE_SIMGROUP
            && (name == want || name.starts_with(&format!("{want}/")))
            && let Some(ws) = &ws
            && let Ok(Some(data)) = ws.data(&c.key)
            && let Ok(g) = SimGroup::parse(&data)
        {
            added.push((c.key.to_string(), g));
        }
    }
    if groups.is_empty() && added.is_empty() {
        return fail(format!(
            "no SimGroups in `{}` or below it; list_streams and search type:simgroup find them",
            p.stream
        ));
    }
    if streams.len() > MAX_STREAMS {
        return fail(format!(
            "`{}` covers {} streams with SimGroups; narrow it to at most {MAX_STREAMS}",
            p.stream,
            streams.len()
        ));
    }
    let (loaded, errors) = scope::load(&kit.game, ws.as_ref(), &groups);
    let schema = LogicSchema::gf2();
    let filter = p.filter.as_deref().map(str::to_lowercase);
    let radius = p.radius.unwrap_or(30.0);
    let mut found: Vec<(f32, Value)> = Vec::new();
    let all = loaded
        .iter()
        .map(|g| (&g.group, None, Some(g.location)))
        .chain(added.iter().map(|(key, g)| (g, Some(key), None)));
    for (group, key, location) in all {
        for (i, o) in group.objects.iter().enumerate() {
            let class = o
                .class_name()
                .or_else(|| schema.class_name(o.class))
                .map_or_else(|| format!("{:08x}", o.class), str::to_owned);
            let name = group.object_name(o).unwrap_or_default();
            if let Some(f) = &filter
                && !class.to_lowercase().contains(f)
                && !name.to_lowercase().contains(f)
            {
                continue;
            }
            let position = group.transform(o).map(|m| [m[3][0], m[3][1], m[3][2]]);
            let distance = match (p.near, position) {
                (Some(at), Some(pos)) => {
                    let d = (0..3).map(|k| (pos[k] - at[k]).powi(2)).sum::<f32>().sqrt();
                    if d > radius {
                        continue;
                    }
                    d
                }
                (Some(_), None) => continue,
                (None, _) => 0.0,
            };
            let round = |v: f32| (f64::from(v) * 100.0).round() / 100.0;
            let mut item = json!({
                "object": match (key, location) {
                    (Some(key), _) => format!("{key}:{i}"),
                    (None, Some(at)) => object_id(kit, at, i),
                    (None, None) => unreachable!("a SimGroup is in the game or added"),
                },
                "class": class,
                "name": name,
                "guid": o.guid.to_string(),
                "position": position.map(|p| p.map(round)),
                "model": group.model(o).map(super::hex),
            });
            if p.near.is_some() {
                item["distance"] = json!(round(distance));
            }
            found.push((distance, item));
        }
    }
    if p.near.is_some() {
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    let total = found.len();
    let offset = p.offset.unwrap_or(0);
    let items: Vec<Value> = found
        .into_iter()
        .skip(offset)
        .take(clamp(p.limit, 100, 500))
        .map(|(_, v)| v)
        .collect();
    let simgroups: Vec<Value> = loaded
        .iter()
        .take(50)
        .map(|g| {
            json!({
                "stream": g.stream,
                "resource": g.location.resource,
                "objects": g.group.objects.len(),
            })
        })
        .chain(added.iter().map(|(key, g)| {
            json!({
                "key": key,
                "objects": g.objects.len(),
                "added_by_workspace": true,
            })
        }))
        .collect();
    Ok(Output::Json(json!({
        "streams": streams.len(),
        "simgroups": { "total": loaded.len() + added.len(), "items": simgroups },
        "unreadable_simgroups": errors.len(),
        "total": total,
        "offset": offset,
        "more": offset + items.len() < total,
        "objects": items,
        "note": "position is the translation of CMD_LoadMatrix (Y up); model is the metamodel id; \
                 object ids work with logic_graph, the SimGroup itself with resource_json",
    })))
}
