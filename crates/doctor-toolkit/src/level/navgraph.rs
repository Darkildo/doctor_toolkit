//! Navigation graphs (`graph`) of a level and their edits through the
//! workspace.
//!
//! A level shows the graphs of its chosen streams. Removing a node renumbers
//! the nodes after it, and other graphs may link into them (neighbour
//! links); [`save_graph`] follows the renumbering in every graph of the game
//! that names the edited one, so the links keep pointing at the same places.

use crate::Result;
use crate::game::{GameInstance, StreamFile};
use crate::resource::graph::{Graph, TYPE_GRAPH};
use crate::resource::simgroup::Guid;
use crate::workspace::Workspace;

/// A graph of the game as the workspace has it.
#[derive(Debug, Clone)]
pub struct GraphRef {
    pub stream: usize,
    pub resource: usize,
    pub graph: Graph,
}

/// The graphs of some streams (with workspace edits). Graphs that do not
/// parse are skipped and reported in the second list.
pub fn graphs_of_streams(
    game: &GameInstance,
    ws: Option<&Workspace>,
    streams: &[usize],
) -> Result<(Vec<GraphRef>, Vec<String>)> {
    let mut out = Vec::new();
    let mut failed = Vec::new();
    for &s in streams {
        let Some(info) = game.streams().get(s) else {
            continue;
        };
        let file = StreamFile::open(&info.path)?;
        for (r, res) in file.layout().resources.iter().enumerate() {
            if res.header.type_id != TYPE_GRAPH {
                continue;
            }
            let data = match ws {
                Some(ws) => ws.current_data(game, s, r)?,
                None => file.resource_data(r)?,
            };
            match Graph::parse(&data) {
                Ok(graph) => out.push(GraphRef {
                    stream: s,
                    resource: r,
                    graph,
                }),
                Err(e) => failed.push(format!("{} #{r}: {e}", info.rel_path)),
            }
        }
    }
    Ok((out, failed))
}

/// Every graph of the game that has a neighbour slot for `guid`.
pub fn graphs_linking_into(
    game: &GameInstance,
    ws: &Workspace,
    guid: Guid,
) -> Result<Vec<GraphRef>> {
    let all: Vec<usize> = (0..game.streams().len()).collect();
    let (graphs, _) = graphs_of_streams(game, Some(ws), &all)?;
    Ok(graphs
        .into_iter()
        .filter(|g| g.graph.neighbours.contains(&guid))
        .collect())
}

/// Records an edited graph in the workspace. With `renumbered` (the map
/// [`Graph::remove_node`] returns), neighbour links of other graphs into
/// it follow; returns how many such links changed.
pub fn save_graph(
    game: &GameInstance,
    ws: &mut Workspace,
    edited: &GraphRef,
    renumbered: Option<&[Option<u16>]>,
    label: &str,
) -> Result<usize> {
    let bytes = edited.graph.to_bytes()?;
    let mut changed = 0;
    if let Some(map) = renumbered {
        for mut other in graphs_linking_into(game, ws, edited.graph.guid)? {
            if (other.stream, other.resource) == (edited.stream, edited.resource) {
                continue;
            }
            let n = other.graph.remap_links_into(edited.graph.guid, map);
            if n > 0 {
                changed += n;
                let data = other.graph.to_bytes()?;
                ws.set_resource(game, other.stream, other.resource, data, label)?;
            }
        }
    }
    ws.set_resource(game, edited.stream, edited.resource, bytes, label)?;
    Ok(changed)
}
