//! Deployment tests on a miniature game.

use super::compile::compile;
use super::deploy;
use super::graph::{Node, ScriptGraph};
use crate::container::ustream::{FileHeader, Storage, StreamLayout, StreamWriter};
use crate::game::GameInstance;
use crate::level::{Location, edit::current_group};
use crate::resource::lua::{Chunk, TYPE_LUA};
use crate::resource::simgroup::{Hash32, Packet, SimGroup, Value};
use crate::workspace::tests::{header, toc_bytes};
use crate::workspace::{ApplyOptions, Workspace};

fn node(kind: &str) -> Node {
    Node::new(0, kind, [0.0, 0.0])
}

/// A game with `gf2/globals_global` (a SimGroup with a `LuaAudioScript`
/// template) and `gf2/zone` (a SimGroup with one object).
fn script_game() -> (tempfile::TempDir, GameInstance) {
    let sample = crate::resource::simgroup::tests::sample();
    let mut template = SimGroup::parse(&sample).unwrap();
    template.packets.push(Packet {
        offset: 0,
        class: Hash32(deploy::CLASS_LUA_AUDIO_SCRIPT),
        class_name: None,
        attr_count: 4,
        attrs: Vec::new(),
    });
    template.objects[0].class = deploy::CLASS_LUA_AUDIO_SCRIPT;
    template.objects[0].packets = vec![template.packets.len() - 1];
    template.objects[0].components = None;
    template.relayout = true;
    let template = template.encode();
    let stream = |group: &[u8]| {
        let mut w = StreamWriter::new(&FileHeader::GF2_PC, crate::io::Endian::Little);
        w.push_resource(&header(b"lh2", [0x77; 4]), &[1, 2, 3], Storage::Raw)
            .unwrap();
        w.push_resource(&header(b"simgroup", [0; 4]), group, Storage::RefPack)
            .unwrap();
        w.finish()
    };
    let (globals, zone) = (stream(&template), stream(&sample));
    let size = |d: &[u8]| StreamLayout::parse(d).unwrap().total_data_size();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("gf2")).unwrap();
    std::fs::write(root.join("godfather2.exe"), b"MZ fake").unwrap();
    std::fs::write(root.join("gf2/globals_global.str"), &globals).unwrap();
    std::fs::write(root.join("gf2/zone.str"), &zone).unwrap();
    let table = crate::toc::Toc::parse(&toc_bytes(&[
        ("gf2/globals_global", size(&globals)),
        ("gf2/zone", size(&zone)),
    ]))
    .unwrap();
    std::fs::write(root.join("gf2/all.toc"), table.to_bytes().unwrap()).unwrap();
    let game = GameInstance::open(root).unwrap();
    (dir, game)
}

#[test]
fn deploy_update_undo_apply_and_remove() {
    let (dir, game) = script_game();
    let index = game.build_index(|_| {}).0;
    let zone = game.find_stream("gf2/zone").unwrap();
    let mut ws = Workspace::for_game(&game, Some(&dir.path().join("ws"))).unwrap();
    let group_at = |resource| Location {
        stream: zone,
        resource,
    };

    let mut g = ScriptGraph::new("hello");
    let ev = g.add(node("event.load"));
    let send = g.add(node("action.send_message").input("message", "doctor_hello"));
    g.connect((ev, "then"), (send, "exec"));
    let chunk = compile(&g).chunk.unwrap();

    let d = deploy::deploy(&game, &index, &mut ws, zone, &g, &chunk).unwrap();
    assert!(!d.updated && d.changed);
    assert_eq!(
        d.object, 1,
        "a copy of the template after the zone's object"
    );
    assert_eq!(d.key.type_id, TYPE_LUA);
    let found = deploy::find(&game, &ws, zone, "hello.lua")
        .unwrap()
        .unwrap();
    assert!(found.added);
    assert_eq!(found.objects, [1]);
    assert_eq!(found.start.as_ref().unwrap(), &g.start);
    let group = current_group(&game, &ws, group_at(1)).unwrap();
    let o = &group.objects[1];
    let c = deploy::CLASS_LUA_AUDIO_SCRIPT;
    assert_eq!(group.attr(o, c, 0), Some(&Value::Int(1)));
    assert_eq!(group.attr(o, c, 3), Some(&Value::Guid(found.guid())));

    // Deploy again with a start message: the same resource and object change.
    g.start.on_load = false;
    g.start.message = Some("doctor_go".into());
    let send2 = g.add(node("action.send_message").input("message", "again"));
    g.connect((send, "then"), (send2, "exec"));
    let chunk2 = compile(&g).chunk.unwrap();
    let d2 = deploy::deploy(&game, &index, &mut ws, zone, &g, &chunk2).unwrap();
    assert!(d2.updated && d2.changed);
    assert_eq!((&d2.key, d2.object), (&d.key, d.object));
    let found = deploy::find(&game, &ws, zone, "hello.lua")
        .unwrap()
        .unwrap();
    let start = found.start.unwrap();
    assert_eq!(start.message.as_deref(), Some("doctor_go"));
    assert!(!start.on_load);
    assert_eq!(ws.data(&d.key).unwrap().unwrap(), chunk2);
    // The same again changes nothing.
    let same = deploy::deploy(&game, &index, &mut ws, zone, &g, &chunk2).unwrap();
    assert!(!same.changed);

    // Undo goes back to the first deployment; apply writes it before the SimGroup.
    ws.undo().unwrap();
    assert_eq!(ws.data(&d.key).unwrap().unwrap(), chunk);
    ws.apply(&game, ApplyOptions::default()).unwrap();
    let file = game.open_stream(zone).unwrap();
    let types: Vec<u32> = file
        .layout()
        .resources
        .iter()
        .map(|r| r.header.type_id)
        .collect();
    assert_eq!(types[1], TYPE_LUA, "the script goes before the SimGroup");
    let applied = file.resource_data(1).unwrap();
    assert_eq!(Chunk::parse(&applied).unwrap().source(), "hello.lua");
    // Found again after apply, and updated in place.
    let d3 = deploy::deploy(&game, &index, &mut ws, zone, &g, &chunk2).unwrap();
    assert!(d3.updated);
    assert_eq!(d3.key, d.key);

    // The graph is kept next to the workspace.
    let path = deploy::save_graph(&ws, &g).unwrap();
    assert_eq!(deploy::saved_graphs(&ws), [("hello".to_owned(), path)]);
    assert_eq!(deploy::list(&game, &ws, zone).unwrap().len(), 1);

    // Remove takes out the object and the resource.
    assert!(deploy::remove(&game, &mut ws, zone, "hello.lua").unwrap());
    assert!(
        deploy::find(&game, &ws, zone, "hello.lua")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        current_group(&game, &ws, group_at(2))
            .unwrap()
            .objects
            .len(),
        1
    );
    assert!(!deploy::remove(&game, &mut ws, zone, "hello.lua").unwrap());
    ws.apply(&game, ApplyOptions::default()).unwrap();
    let file = game.open_stream(zone).unwrap();
    assert!(
        file.layout()
            .resources
            .iter()
            .all(|r| r.header.type_id != TYPE_LUA)
    );

    // A script that never starts is refused; a stream without a SimGroup too.
    g.start.message = None;
    assert!(deploy::deploy(&game, &index, &mut ws, zone, &g, &chunk2).is_err());
}

#[test]
fn deploying_into_the_template_stream_duplicates_the_template() {
    let (dir, game) = script_game();
    let index = game.build_index(|_| {}).0;
    let globals = game.find_stream("gf2/globals_global").unwrap();
    let mut ws = Workspace::for_game(&game, Some(&dir.path().join("ws"))).unwrap();
    let mut g = ScriptGraph::new("always");
    g.start.stop_message = Some("doctor_stop".into());
    g.add(node("event.load"));
    let chunk = compile(&g).chunk.unwrap();
    let d = deploy::deploy(&game, &index, &mut ws, globals, &g, &chunk).unwrap();
    assert_eq!(d.object, 1);
    let found = deploy::find(&game, &ws, globals, "always.lua")
        .unwrap()
        .unwrap();
    assert_eq!(
        found.start.unwrap().stop_message.as_deref(),
        Some("doctor_stop")
    );
    // The template itself is not changed.
    let group = current_group(
        &game,
        &ws,
        Location {
            stream: globals,
            resource: 1,
        },
    )
    .unwrap();
    assert_eq!(
        group.attr(&group.objects[0], deploy::CLASS_LUA_AUDIO_SCRIPT, 3),
        None
    );
}
