use std::collections::HashSet;

use egui::{Pos2, Rect};

use super::*;

fn chain(n: u64) -> Graph {
    let mut g = Graph::default();
    for i in 0..n {
        g.nodes.push(
            Node::new(NodeId(i), format!("node {i}"))
                .input(Pin::new("in", 0))
                .output(Pin::new("out", 0)),
        );
    }
    for i in 1..n {
        g.wires.push(Wire::new((NodeId(i - 1), 0), (NodeId(i), 0)));
    }
    g
}

#[test]
fn layered_layout_puts_a_chain_left_to_right_without_overlaps() {
    let mut g = chain(5);
    // A cycle back to the start must not hang or stack the nodes.
    g.wires.push(Wire::new((NodeId(4), 0), (NodeId(0), 0)));
    // A loner.
    g.nodes.push(Node::new(NodeId(99), "alone"));
    let pos = layered(&g, None, &LayoutOptions::default());
    assert_eq!(pos.len(), 6);
    for i in 1..5 {
        assert!(pos[&NodeId(i)].x > pos[&NodeId(i - 1)].x, "layer order");
    }
    for n in &mut g.nodes {
        n.pos = pos[&n.id];
    }
    let rects: Vec<Rect> = g.nodes.iter().map(Node::rect).collect();
    for (i, a) in rects.iter().enumerate() {
        for b in &rects[i + 1..] {
            assert!(!a.intersects(*b), "{a:?} overlaps {b:?}");
        }
    }
    // The loner goes below the component.
    assert!(pos[&NodeId(99)].y > rects[0].max.y);
}

#[test]
fn layout_of_a_subset_ignores_the_rest() {
    let g = chain(6);
    let only: HashSet<NodeId> = [NodeId(1), NodeId(2)].into_iter().collect();
    let pos = layered(&g, Some(&only), &LayoutOptions::default());
    assert_eq!(pos.len(), 2);
}

#[test]
fn branches_are_ordered_to_avoid_crossings() {
    // 0 → 1, 0 → 2; 1 → 3, 2 → 4: the children keep their parents' order.
    let mut g = chain(1);
    for i in 1..5 {
        g.nodes.push(
            Node::new(NodeId(i), "n")
                .input(Pin::new("in", 0))
                .output(Pin::new("out", 0)),
        );
    }
    for (a, b) in [(0, 1), (0, 2), (1, 3), (2, 4)] {
        g.wires.push(Wire::new((NodeId(a), 0), (NodeId(b), 0)));
    }
    let pos = layered(&g, None, &LayoutOptions::default());
    let above = pos[&NodeId(1)].y < pos[&NodeId(2)].y;
    assert_eq!(above, pos[&NodeId(3)].y < pos[&NodeId(4)].y);
}

#[test]
fn store_round_trips_positions_and_comments() {
    let mut g = chain(3);
    g.nodes[1].pos = Pos2::new(120.0, -40.0);
    g.comments
        .push(g.comment_around(&[NodeId(0), NodeId(1)], "start").unwrap());
    let key = |id: NodeId| (id.0 != 2).then(|| format!("k{}", id.0));
    let mut store = LayoutStore::default();
    store.capture(&g, &key);
    let dir = std::env::temp_dir().join(format!("dtk-node-graph-{}", std::process::id()));
    let path = dir.join("layout.json");
    store.save(&path).unwrap();
    let loaded = LayoutStore::load(&path);
    assert_eq!(loaded, store);
    let mut fresh = chain(3);
    let missing = loaded.restore(&mut fresh, &key);
    assert_eq!(missing, vec![NodeId(2)]);
    assert_eq!(fresh.nodes[1].pos, Pos2::new(120.0, -40.0));
    assert_eq!(fresh.comments.len(), 1);
    assert_eq!(fresh.comments[0].text, "start");
    std::fs::remove_dir_all(dir).ok();
    // A broken file is an empty store.
    assert_eq!(
        LayoutStore::load(std::path::Path::new("/nonexistent/x.json")),
        LayoutStore::default()
    );
}

#[test]
fn node_sizes_grow_with_pins_and_text() {
    let small = Node::new(NodeId(0), "a");
    let big = Node::new(NodeId(1), "a much longer title of a node")
        .input(Pin::new("one", 0))
        .input(Pin::new("two", 0))
        .output(Pin::new("out", 0));
    assert!(big.size().x > small.size().x);
    assert!(big.size().y > small.size().y);
    assert!(big.pin_pos(true, 0).x == big.rect().max.x);
    assert!(big.pin_pos(false, 1).y > big.pin_pos(false, 0).y);
}

#[test]
fn wire_hit_test_uses_the_curve() {
    let pts = [
        Pos2::new(0.0, 0.0),
        Pos2::new(10.0, 0.0),
        Pos2::new(10.0, 10.0),
    ];
    assert!(view::polyline_distance(&pts, Pos2::new(5.0, 1.0)) < 1.01);
    assert!(view::polyline_distance(&pts, Pos2::new(0.0, 10.0)) > 7.0);
}

/// Draws the view over a 800×600 screen with `events`; returns its events.
fn run_view(
    ctx: &egui::Context,
    view: &mut GraphView,
    graph: &mut Graph,
    mut events: Vec<egui::Event>,
) -> Vec<GraphEvent> {
    // The keys' modifiers held for the frame, as a window reports them.
    let modifiers = events
        .iter()
        .find_map(|e| match e {
            egui::Event::Key { modifiers, .. } => Some(*modifiers),
            _ => None,
        })
        .unwrap_or_default();
    events.insert(0, egui::Event::ModifiersChanged(modifiers));
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
        events,
        ..Default::default()
    };
    let mut out = Vec::new();
    let mut output = ctx.run_ui(input, |ui| {
        out = view.show(ui, graph, &mut |_, _| {}).events;
    });
    output.textures_delta.clear();
    out
}

fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

#[test]
fn ctrl_d_duplicates_and_escape_deselects_as_in_the_level_view() {
    // Stage 56: the same keys in every editor.
    let ctx = egui::Context::default();
    let mut view = GraphView::default();
    let mut g = chain(2);
    let pointer = egui::Event::PointerMoved(Pos2::new(400.0, 300.0));
    run_view(&ctx, &mut view, &mut g, vec![pointer.clone()]);
    view.set_selection([NodeId(1)]);
    let events = run_view(
        &ctx,
        &mut view,
        &mut g,
        vec![pointer.clone(), key(egui::Key::D, egui::Modifiers::COMMAND)],
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, GraphEvent::Copy(ids) if ids == &vec![NodeId(1)]))
    );
    let at = g.node(NodeId(1)).unwrap().pos + egui::Vec2::splat(40.0);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, GraphEvent::Paste(p) if *p == at)),
        "pasted beside the node"
    );
    let events = run_view(
        &ctx,
        &mut view,
        &mut g,
        vec![pointer, key(egui::Key::Escape, egui::Modifiers::NONE)],
    );
    assert!(view.selection().is_empty());
    assert!(
        events
            .iter()
            .any(|e| matches!(e, GraphEvent::Selection(ids) if ids.is_empty()))
    );
}
