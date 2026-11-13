//! `graph` navigation graphs: parsing, the JSON view, writing and edits.
#![no_main]

use doctor_toolkit::resource::graph::{Graph, Neighbourhood};
use doctor_toolkit::resource::logic::{self, TYPE_GRAPH};
use libfuzzer_sys::fuzz_target;

/// What is written parses back and writes the same bytes.
fn stable(graph: &Graph) {
    if let Ok(bytes) = graph.to_bytes() {
        let again = Graph::parse(&bytes).expect("a written graph parses");
        assert_eq!(again.to_bytes().ok(), Some(bytes), "writing is not stable");
    }
}

fuzz_target!(|data: &[u8]| {
    let _ = logic::to_json(TYPE_GRAPH, data);
    let Ok(mut graph) = Graph::parse(data) else {
        return;
    };
    let _ = graph.info();
    stable(&graph);
    // Path tables grow with the square of the nodes: edits on small graphs.
    if graph.nodes.len() > 64 {
        return;
    }
    let _ = graph.repair_derived();
    stable(&graph);
    if let Some(p) = graph.nodes.first().map(|n| n.position) {
        let _ = graph.move_node(0, [p[0] + 1.0, p[1], p[2]]);
        let _ = graph.attach_point(p, 0.5, Neighbourhood::default());
        let last = graph.nodes.len() - 1;
        let _ = graph.add_link(0, last, 0);
        let _ = graph.remove_link(0, last);
        let _ = graph.remove_node(0);
        stable(&graph);
    }
});
