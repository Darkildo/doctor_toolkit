//! `game_info` and `list_streams`.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Value, json};

use doctor_toolkit::logic_graph::scope;
use doctor_toolkit::resource::names::type_tags;

use super::{ListStreamsParams, Output, clamp};
use crate::state::{ToolResult, Toolkit};

/// What the game is, what the index holds, the workspace and the project.
pub fn game_info(kit: &Toolkit) -> ToolResult<Output> {
    let game = &kit.game;
    let (index, report) = kit.index_with_report();
    let mut types: HashMap<u32, usize> = HashMap::new();
    for id in index.ids() {
        *types.entry(index.record(id).header.type_id).or_default() += 1;
    }
    let mut types: Vec<(u32, usize)> = types.into_iter().collect();
    types.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let types: Vec<Value> = types
        .iter()
        .take(40)
        .map(|(t, n)| json!({ "type": type_tags().display(*t), "count": n }))
        .collect();
    let mut areas: BTreeMap<String, usize> = BTreeMap::new();
    for s in game.streams() {
        let name = s.stream_name();
        let area: Vec<&str> = name.split('/').take(2).collect();
        *areas.entry(area.join("/")).or_default() += 1;
    }
    let ws = kit.workspace()?;
    let workspace = match &ws {
        Some(ws) => json!({
            "dir": kit.workspace_dir,
            "exists": true,
            "changed_resources": ws.len(),
            "new_streams": ws.new_streams().count(),
            "file_changes": ws.file_changes().count(),
            "operations_in_effect": ws.head(),
        }),
        None => json!({ "dir": kit.workspace_dir, "exists": false }),
    };
    let project = kit
        .project_dir
        .as_ref()
        .map(|dir| match doctor_toolkit::project::Project::open(dir) {
            Ok(p) => json!({
                "dir": dir,
                "name": p.config.project.name,
                "id": p.config.project.id,
                "version": p.config.project.version,
            }),
            Err(e) => json!({ "dir": dir, "error": e.to_string() }),
        });
    Ok(Output::Json(json!({
        "game": {
            "root": game.root(),
            "profile": game.profile().name,
            "streams": game.streams().len(),
            "tables_of_contents": game.tocs().len(),
            "missions": scope::missions(game).len(),
        },
        "index": {
            "resources": index.len(),
            "streams": index.stream_count(),
            "unreadable_resources": index.record_errors(),
            "cache": kit.index_cache(),
            "streams_from_cache": report.reused,
            "streams_read": report.rebuilt,
        },
        "types": types,
        "areas": areas,
        "workspace": workspace,
        "project": project,
        "limits": { "max_response_chars": kit.max_chars },
        "read_only": true,
        "hints": [
            "search finds resources by name, hash, key, type (type:simgroup) or text (text:...).",
            "resource_info shows names, references and what a resource is used by; resource_json shows its contents.",
            "level_objects lists placed objects with positions; logic_graph follows the messages between objects.",
            "Resources doctor-toolkit://docs/formats/*.md describe the file formats.",
        ],
    })))
}

/// Streams by name with their sizes.
pub fn list_streams(kit: &Toolkit, p: ListStreamsParams) -> ToolResult<Output> {
    let index = kit.index();
    let prefix = p
        .prefix
        .as_deref()
        .map(|s| s.trim().replace('\\', "/").to_ascii_lowercase());
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for s in 0..index.stream_count() {
        counts.insert(index.stream_name(s), index.stream_records(s).count());
    }
    let all: Vec<Value> = kit
        .game
        .streams()
        .iter()
        .map(|s| (s.stream_name(), s))
        .filter(|(name, _)| {
            prefix
                .as_deref()
                .is_none_or(|p| name.to_ascii_lowercase().starts_with(p))
        })
        .map(|(name, s)| {
            json!({
                "stream": name,
                "bytes": s.len,
                "resources": counts.get(name.as_str()).copied().unwrap_or(0),
            })
        })
        .collect();
    let total = all.len();
    let offset = p.offset.unwrap_or(0);
    let (items, more) = super::super::output::window(all, offset, clamp(p.limit, 200, 1000));
    Ok(Output::Json(json!({
        "total": total,
        "offset": offset,
        "more": more,
        "streams": items,
    })))
}
