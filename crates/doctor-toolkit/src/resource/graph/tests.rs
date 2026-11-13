use super::*;

fn guid(k: u8) -> Guid {
    let mut g = [k; 16];
    g[8..].copy_from_slice(b"CREATOR1");
    Guid(g)
}

/// A square of four nodes linked both ways around, with a property on a
/// link and a link into a neighbouring graph.
fn square() -> Graph {
    let mut g = Graph::new(guid(1), graph_flags::PED);
    for p in [
        [0.0, 0.0, 0.0],
        [10.0, 0.0, 0.0],
        [10.0, 0.0, 10.0],
        [0.0, 0.0, 10.0],
    ] {
        g.add_node(p, 0.5).unwrap();
    }
    for k in 0..4 {
        g.add_link(k, (k + 1) % 4, 0).unwrap();
        g.add_link((k + 1) % 4, k, 0).unwrap();
    }
    g.property_sets.push(PropertySet {
        fields: vec![
            PropertyField {
                name: Hash32(0x22C6CC5),
                kind: PropertyKind::Guid,
                offset: 4,
            },
            PropertyField {
                name: Hash32(0x1),
                kind: PropertyKind::Float,
                offset: 0,
            },
        ],
    });
    g.property_instances.push(PropertyInstance {
        set: 0,
        values: vec![PropertyValue::Guid(guid(7)), PropertyValue::Float(2.5)],
    });
    g.nodes[0].links[0].properties = Some(0);
    g.neighbours.push(guid(2));
    g.nodes[2].neighbour_links.push(NeighbourLink {
        neighbour: 0,
        node: 3,
        length: 4.0,
        flags: 0,
        properties: None,
    });
    g
}

#[test]
fn a_built_graph_writes_and_parses_back() {
    let g = square();
    assert!(g.problems().is_empty(), "{:?}", g.problems());
    let bytes = g.to_bytes().unwrap();
    assert_eq!(bytes.len() % 2, 0);
    assert_eq!(
        u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
        VERSION
    );
    let back = Graph::parse(&bytes).unwrap();
    assert_eq!(back, g);
    assert_eq!(back.to_bytes().unwrap(), bytes);
    // The JSON form gives back the same graph.
    let json = serde_json::to_string_pretty(&g).unwrap();
    let from_json: Graph = serde_json::from_str(&json).unwrap();
    assert_eq!(from_json.to_bytes().unwrap(), bytes);
    // Links of a node, lengths, paths.
    assert_eq!(g.link_count(), 8);
    assert_eq!(g.nodes[0].links[0].length, 10.0);
    let t = g.precomputed_paths.as_ref().unwrap();
    assert!(paths_valid(&g, t));
    // 0 → 2 goes through 1 (first link of node 0) or 3 (second), both 20.
    assert!(matches!(t.get(4, 0, 2), 1 | 2));
    assert_eq!(t.get(4, 0, 0), 0);
    let info = g.info();
    assert_eq!((info.nodes, info.links, info.one_way_links), (4, 8, 0));
    assert_eq!(info.flags, "ped");
}

#[test]
fn moving_a_node_updates_lengths_boxes_and_paths() {
    let mut g = square();
    g.move_node(1, [10.0, 0.0, -10.0]).unwrap();
    assert!(g.stale_lengths().is_empty());
    assert!((g.nodes[0].links[0].length - 200f32.sqrt()).abs() < 1e-5);
    let back = Graph::parse(&g.to_bytes().unwrap()).unwrap();
    let RTree::Shape(_) = back.rtree else {
        panic!("a written tree reads back as a shape");
    };
    assert!(paths_valid(&back, back.precomputed_paths.as_ref().unwrap()));
}

#[test]
fn removing_a_node_renumbers_links_and_neighbours() {
    let mut g = square();
    g.visibility = Visibility::Pairs {
        hidden: vec![[0, 3], [1, 2]],
    };
    let map = g.remove_node(1).unwrap();
    assert_eq!(map, vec![Some(0), None, Some(1), Some(2)]);
    assert_eq!(g.nodes.len(), 3);
    assert!(g.links().all(|(_, l)| (l.to as usize) < 3));
    assert_eq!(
        g.visibility,
        Visibility::Pairs {
            hidden: vec![[0, 2]]
        }
    );
    // Another graph that links into this one follows the renumbering.
    let mut other = Graph::new(guid(2), graph_flags::PED);
    other.add_node([0.0; 3], 0.5).unwrap();
    other.neighbours.push(guid(1));
    for node in [1, 3] {
        other.nodes[0].neighbour_links.push(NeighbourLink {
            neighbour: 0,
            node,
            length: 1.0,
            flags: 0,
            properties: None,
        });
    }
    assert_eq!(other.remap_links_into(guid(1), &map), 2);
    assert_eq!(other.nodes[0].neighbour_links.len(), 1);
    assert_eq!(other.nodes[0].neighbour_links[0].node, 2);
    Graph::parse(&g.to_bytes().unwrap()).unwrap();
}

#[test]
fn attach_point_links_the_nearest_nodes_both_ways() {
    let mut g = square();
    let (k, near) = g
        .attach_point([1.0, 0.0, 1.0], 0.5, Neighbourhood::default())
        .unwrap();
    assert_eq!(k, 4);
    assert_eq!(near[0], 0);
    assert_eq!(near.len(), 2);
    assert!(g.nodes[0].links.iter().any(|l| l.to == 4));
    let t = g.precomputed_paths.as_ref().unwrap();
    assert!(paths_valid(&g, t));
    assert_ne!(t.get(5, 4, 2), 0, "the new node reaches the far corner");
    let far = Neighbourhood {
        max_distance: 1.0,
        ..Neighbourhood::default()
    };
    assert!(g.attach_point([100.0, 0.0, 100.0], 0.5, far).is_err());
}

#[test]
fn edits_refuse_what_the_file_cannot_hold() {
    let mut g = square();
    assert!(g.add_link(0, 0, 0).is_err());
    assert!(g.add_link(0, 1, 0).is_err(), "duplicate");
    assert!(g.add_link(0, 9, 0).is_err());
    assert!(g.remove_link(0, 2).is_err());
    assert!(g.move_node(0, [f32::NAN, 0.0, 0.0]).is_err());
    g.nodes[0].links[0].to = 40;
    assert!(g.to_bytes().is_err());
}

#[test]
fn visibility_bits_follow_the_game_formula() {
    // Pair (a, b) of n nodes: a·(2n − a − 1)/2 + b − a − 1 (0x005FC8C0).
    assert_eq!(visibility_bit(5, 0, 1), 0);
    assert_eq!(visibility_bit(5, 0, 4), 3);
    assert_eq!(visibility_bit(5, 1, 2), 4);
    assert_eq!(visibility_bit(5, 3, 4), 9);
    assert_eq!(visibility_bit(5, 4, 3), 9);
    let mut g = square();
    g.visibility = Visibility::Pairs {
        hidden: vec![[1, 2]],
    };
    let bytes = g.to_bytes().unwrap();
    let back = Graph::parse(&bytes).unwrap();
    assert_eq!(back.visibility, g.visibility);
}

#[test]
fn rejects_bad_input_without_panicking() {
    let d = square().to_bytes().unwrap();
    assert!(Graph::parse(&d[..0x80]).is_err());
    let mut bad = d.clone();
    bad[8] = 17;
    assert!(Graph::parse(&bad).is_err());
    let mut huge = d.clone();
    huge[0x0C..0x10].copy_from_slice(&[0xFF; 4]);
    assert!(Graph::parse(&huge).is_err());
    for i in 0..d.len() {
        for x in [0x41, 0xFF, 0x01] {
            let mut c = d.clone();
            c[i] ^= x;
            if let Ok(g) = Graph::parse(&c)
                && let Ok(bytes) = g.to_bytes()
            {
                let again = Graph::parse(&bytes).expect("a written graph parses");
                assert_eq!(again.to_bytes().unwrap(), bytes);
            }
        }
    }
}

#[test]
fn an_empty_graph_has_the_layout_of_the_game() {
    // 41 graphs of the game have no nodes: header, then 4 zero bytes before
    // the empty visibility list, and the path table pointer (if any) after.
    let g = Graph::new(guid(3), graph_flags::GROUND_PATHING);
    let bytes = g.to_bytes().unwrap();
    assert_eq!(bytes.len(), HEADER_SIZE + 8);
    let w = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    assert_eq!(w(0x68), HEADER_SIZE as u32);
    assert_eq!(w(0x70), HEADER_SIZE as u32 + 4);
    assert_eq!(w(0x40), HEADER_SIZE as u32 + 8);
    assert_eq!(Graph::parse(&bytes).unwrap(), g);
}
