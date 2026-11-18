//! The deltas on the resources of the miniature game ([`crate::testing`]):
//! its SimGroups, a string table, the parts database and the world grid.

use super::simgroup::{Op, Piece, SimGroupDelta};
use super::*;
use crate::game::GameInstance;
use crate::resource::lh2::StringTable;
use crate::resource::partsdb::{Part, PartsDb};
use crate::resource::simgroup::{CLASS_ENTITY, Guid, Hash32, SimGroup, Value};
use crate::resource::wgi::{self, GridStream, NewCell, WorldGrid};
use crate::testing;

/// The type id and the data of a resource of the miniature game.
fn resource(stream: &str, index: usize) -> (u32, Vec<u8>) {
    let dir = tempfile::tempdir().unwrap();
    testing::write_game(dir.path()).unwrap();
    testing::write_town(dir.path()).unwrap();
    let game = GameInstance::open(dir.path()).unwrap();
    let stream = game.open_stream(game.find_stream(stream).unwrap()).unwrap();
    (
        stream.layout().resources[index].header.type_id,
        stream.resource_data(index).unwrap(),
    )
}

fn delta_text(delta: &Delta) -> String {
    String::from_utf8(delta.to_bytes()).unwrap()
}

/// The delta between two versions, checked the way a mod build uses it.
fn exact_delta(type_id: u32, base: &[u8], mine: &[u8]) -> Delta {
    let bytes = exact(type_id, base, mine)
        .unwrap()
        .expect("the change is a delta");
    let delta = Delta::parse(&bytes).unwrap();
    assert_eq!(delta.to_bytes(), bytes, "the document is canonical");
    let again = diff_resource(type_id, base, mine).unwrap().unwrap();
    assert_eq!(again.to_bytes(), bytes, "the same delta, the same bytes");
    let applied = apply_resource(type_id, &bytes, base).unwrap();
    assert_eq!(applied.data, mine);
    assert!(applied.skipped.is_empty() && applied.conflicts.is_empty());
    assert_eq!(applied.applied, delta.len());
    delta
}

// ------------------------------------------------------------- SimGroup

fn guid(n: u8) -> Guid {
    let mut bytes = *b"\0\0\0X\0\0\0\0DOCTORTK";
    bytes[0] = n;
    Guid(bytes)
}

fn edit(data: &[u8], change: impl FnOnce(&mut SimGroup)) -> Vec<u8> {
    let mut group = SimGroup::parse(data).unwrap();
    change(&mut group);
    group.encode()
}

fn object(group: &SimGroup, n: u8) -> usize {
    let guid = guid(n);
    group.objects.iter().position(|o| o.guid == guid).unwrap()
}

/// The group of the yard of the town with three more crates, `guid(1)` to
/// `guid(3)`, each at its own place and with its own name.
fn yard() -> (u32, Vec<u8>) {
    let (type_id, data) = resource(testing::YARD, testing::YARD_SIMGROUP);
    assert_eq!(kind_of(type_id), Some(Kind::SimGroup));
    let data = edit(&data, |group| {
        for n in 1..=3u8 {
            let copy = group.duplicate_object(0, guid(n)).unwrap();
            group
                .set_position(copy, [10.0 * f32::from(n), 0.0, 1.0])
                .unwrap();
            let name = Value::String(format!("crate_{n}"));
            group
                .set_class_attr(copy, CLASS_ENTITY, 2, Some(name))
                .unwrap();
        }
    });
    (type_id, data)
}

/// Everything a group says about its objects, whatever its layout.
fn view(data: &[u8]) -> Vec<String> {
    let group = SimGroup::parse(data).unwrap();
    assert_eq!(group.encode(), data, "the group is written as it is read");
    group
        .objects
        .iter()
        .map(|o| {
            let packets: Vec<_> = group
                .object_packets(o)
                .map(|p| {
                    let attrs: Vec<_> = p.attrs.iter().map(|a| (a.index, &a.value)).collect();
                    (p.class, p.attr_count, attrs)
                })
                .collect();
            format!(
                "{} {:08x} {} {} {:?} {packets:?}",
                o.guid, o.class, o.flags, o.flags2, o.links
            )
        })
        .collect()
}

/// One edit of every kind: a move, an attribute, a model, a removal and a
/// new object.
fn five_edits(group: &mut SimGroup) {
    let (one, two, three) = (object(group, 1), object(group, 2), object(group, 3));
    group.set_position(one, [1.0, 2.0, 3.0]).unwrap();
    group
        .set_class_attr(two, CLASS_ENTITY, 3, Some(Value::Int(1)))
        .unwrap();
    group.set_model(0, Some(0x1234_5678)).unwrap();
    group.remove_object(three).unwrap();
    let copy = group.duplicate_object(one, guid(9)).unwrap();
    group.set_position(copy, [7.0, 8.0, 9.0]).unwrap();
}

fn simgroup_delta(delta: Delta) -> SimGroupDelta {
    match delta {
        Delta::SimGroup(delta) => delta,
        other => panic!("a SimGroup delta expected, not {}", other.kind()),
    }
}

#[test]
fn simgroup_edits_of_every_kind_round_trip() {
    let (type_id, base) = yard();
    let mine = edit(&base, five_edits);
    let delta = exact_delta(type_id, &base, &mine);
    let text = delta_text(&delta);
    let delta = simgroup_delta(delta);
    assert!(matches!(&delta.ops[0], Op::Remove { object } if *object == guid(3)));
    assert!(matches!(&delta.ops[4], Op::Add { object, .. } if *object == guid(9)));
    let has = |wanted: fn(&Op) -> bool| delta.ops.iter().filter(|op| wanted(op)).count();
    assert_eq!(has(|op| matches!(op, Op::Transform { .. })), 1);
    assert_eq!(has(|op| matches!(op, Op::Model { .. })), 1);
    assert_eq!(has(|op| matches!(op, Op::Set { .. })), 1);
    assert_eq!(delta.ops.len(), 5);
    // Objects are named by GUID, attributes by class and index, and an
    // operation tells the value it replaced.
    for part in [
        format!(r#"{{"op":"remove","object":"{}"}}"#, guid(3)),
        format!(
            r#"{{"op":"set","object":"{}","attribute":"38523fc3.3","new":{{"int":1}}}}"#,
            guid(2)
        ),
        format!(
            r#""old":"{:08x}","model":"12345678""#,
            testing::CRATE_MODEL_ID
        ),
        r#""matrix":[[1.0,0.0,0.0,0.0],[0.0,1.0,0.0,0.0],[0.0,0.0,1.0,0.0],[1.0,2.0,3.0,1.0]]"#
            .to_owned(),
        r#"{"string":"crate_1"}"#.to_owned(),
    ] {
        assert!(text.contains(&part), "{part} in {text}");
    }
    assert!(text.starts_with(r#"{"format":1,"kind":"simgroup","ops":["#));

    // Nothing changed: a delta without operations, which changes nothing.
    let none = exact_delta(type_id, &base, &base);
    assert!(none.is_empty());
    assert_eq!(
        delta_text(&none),
        r#"{"format":1,"kind":"simgroup","ops":[]}"#
    );
}

#[test]
fn simgroup_layout_gives_the_bytes_of_edits_made_in_another_order() {
    let (type_id, base) = yard();
    // Crate 1 is moved, crate 2 is moved, crate 1 is moved again: its
    // matrix is written after that of crate 2.
    let mut mine = base.clone();
    for (n, x) in [(1, 5.0), (2, 6.0), (1, 7.0)] {
        mine = edit(&mine, |group| {
            let at = object(group, n);
            group.set_position(at, [x, 0.0, 0.0]).unwrap();
        });
    }
    let delta = simgroup_delta(exact_delta(type_id, &base, &mine));
    let layout = delta
        .layout
        .clone()
        .expect("the operations alone give another layout");
    assert_eq!(layout.base, crate::workspace::store::sha256_hex(&base));
    assert!(layout.packets.is_empty() && layout.strings.is_empty());
    assert!(layout.aux.iter().all(|p| matches!(p, Piece::Run(_))));
    assert!(layout.aux.len() >= 3, "{:?}", layout.aux);

    // Without the layout: the same group in other bytes.
    let plain = SimGroupDelta {
        layout: None,
        ..delta.clone()
    };
    let plain = simgroup::apply(&plain, &base).unwrap().data;
    assert_ne!(plain, mine);
    assert_eq!(view(&plain), view(&mine));

    // On another base the layout is not used, and the result is the edits.
    let other = edit(&base, |group| {
        let at = object(group, 3);
        group.set_position(at, [0.5, 0.5, 0.5]).unwrap();
    });
    let applied = simgroup::apply(&delta, &other).unwrap();
    let mut by_hand = other.clone();
    for (n, x) in [(1, 5.0), (2, 6.0), (1, 7.0)] {
        by_hand = edit(&by_hand, |group| {
            let at = object(group, n);
            group.set_position(at, [x, 0.0, 0.0]).unwrap();
        });
    }
    assert_eq!(view(&applied.data), view(&by_hand));

    // A layout that does not fit the group is an error, not a broken group.
    for broken in [
        simgroup::Layout {
            packets: vec![[0, 1]],
            ..layout.clone()
        },
        simgroup::Layout {
            packets: vec![[0, u32::MAX], [0, 4]],
            ..layout.clone()
        },
        simgroup::Layout {
            aux: vec![Piece::Run([u32::MAX, 16])],
            ..layout.clone()
        },
        simgroup::Layout {
            aux: vec![Piece::Literal("zz".into())],
            ..layout.clone()
        },
        simgroup::Layout {
            aux: vec![Piece::Run([0, 16])],
            ..layout.clone()
        },
        simgroup::Layout {
            strings: vec![Piece::Run([0, 99])],
            ..layout.clone()
        },
        simgroup::Layout {
            strings: vec![Piece::Literal("only this".into())],
            ..layout.clone()
        },
    ] {
        let delta = SimGroupDelta {
            layout: Some(broken),
            ..delta.clone()
        };
        let error = simgroup::apply(&delta, &base).unwrap_err().to_string();
        assert!(error.contains("layout"), "{error}");
    }
}

#[test]
fn simgroup_layout_names_which_of_two_equal_packets_an_object_uses() {
    let (type_id, base) = yard();
    // The files of the game do not always share equal packets. Here crates
    // 2 and 3 have the same name, each in a packet of its own.
    let mut slot = 0;
    let base = edit(&base, |group| {
        let (two, three) = (object(group, 2), object(group, 3));
        let name = Value::String("crate_2".into());
        group
            .set_class_attr(three, CLASS_ENTITY, 2, Some(name))
            .unwrap();
        slot = group.slot_of(three, CLASS_ENTITY).unwrap();
        let shared = group.objects[three].packets[slot];
        assert_eq!(group.objects[two].packets[slot], shared);
        group.packets.push(group.packets[shared].clone());
        group.objects[three].packets[slot] = group.packets.len() - 1;
    });
    // A duplicate of crate 3 shares the packet of crate 3; an object added
    // from its values takes the first equal packet, that of crate 2.
    let mine = edit(&base, |group| {
        let three = object(group, 3);
        group.duplicate_object(three, guid(9)).unwrap();
    });
    let delta = simgroup_delta(exact_delta(type_id, &base, &mine));
    let layout = delta.layout.clone().expect("the packets are told apart");
    let group = SimGroup::parse(&mine).unwrap();
    let theirs = group.objects[object(&group, 3)].packets[slot] as u32;
    assert_eq!(layout.users, [[4, slot as u32, theirs]]);
    assert!(layout.packets.is_empty() && layout.aux.is_empty() && layout.strings.is_empty());

    // An object does not get a packet with other values through a layout.
    let other = group.objects[object(&group, 1)].packets[slot] as u32;
    for users in [
        vec![[4, slot as u32, other]],
        vec![[4, slot as u32, 99]],
        vec![[4, 7, theirs]],
        vec![[99, 0, 0]],
    ] {
        let broken = SimGroupDelta {
            layout: Some(simgroup::Layout {
                users,
                ..layout.clone()
            }),
            ..delta.clone()
        };
        let error = simgroup::apply(&broken, &base).unwrap_err().to_string();
        assert!(error.contains("layout"), "{error}");
    }
    // Nor does a pointer get other data.
    let moved = edit(&base, |group| {
        let at = object(group, 1);
        group.set_position(at, [3.0, 2.0, 1.0]).unwrap();
    });
    let delta = simgroup_delta(exact_delta(type_id, &base, &moved));
    assert!(delta.layout.is_none());
    for layout in [
        simgroup::Layout {
            aux_pointers: vec![[0, 16]],
            ..layout.clone()
        },
        simgroup::Layout {
            aux_pointers: vec![[0, u32::MAX]],
            ..layout.clone()
        },
        simgroup::Layout {
            string_pointers: vec![[0, 3]],
            ..layout.clone()
        },
    ] {
        let broken = SimGroupDelta {
            layout: Some(simgroup::Layout {
                users: Vec::new(),
                ..layout
            }),
            ..delta.clone()
        };
        let error = simgroup::apply(&broken, &base).unwrap_err().to_string();
        assert!(error.contains("would get another"), "{error}");
    }
}

#[test]
fn simgroup_delta_applies_to_a_changed_base() {
    let (type_id, base) = yard();
    let mine = edit(&base, five_edits);
    let delta = exact_delta(type_id, &base, &mine).to_bytes();

    // Another mod added an object, moved the first one and named crate 2.
    let other = |group: &mut SimGroup| {
        let copy = group.duplicate_object(0, guid(20)).unwrap();
        group.set_position(copy, [-5.0, 0.0, -5.0]).unwrap();
        group.set_position(0, [100.0, 0.0, 100.0]).unwrap();
        let two = object(group, 2);
        let name = Value::String("renamed".into());
        group
            .set_class_attr(two, CLASS_ENTITY, 2, Some(name))
            .unwrap();
    };
    let changed = edit(&base, other);
    let applied = apply_resource(type_id, &delta, &changed).unwrap();
    assert!(applied.skipped.is_empty(), "{:?}", applied.skipped);
    assert!(applied.conflicts.is_empty(), "{:?}", applied.conflicts);
    assert_eq!(applied.applied, 5);
    let by_hand = edit(&changed, five_edits);
    assert_eq!(view(&applied.data), view(&by_hand));
    // Both mods are in: the other's object and name, the author's edits.
    let group = SimGroup::parse(&applied.data).unwrap();
    assert_eq!(group.objects.len(), 5);
    assert_eq!(group.objects[3].guid, guid(20));
    assert_eq!(group.objects[4].guid, guid(9));
    assert_eq!(name_of(&group, object(&group, 2)), "renamed");
    assert_eq!(group.model(&group.objects[0]), Some(0x1234_5678));
}

/// The name the tests give a crate (`Entity` attribute 2).
fn name_of(group: &SimGroup, object: usize) -> String {
    match group.attr(&group.objects[object], CLASS_ENTITY, 2) {
        Some(Value::String(name)) => name.clone(),
        other => panic!("no name: {other:?}"),
    }
}

#[test]
fn simgroup_operations_without_a_target_are_skipped() {
    let (type_id, base) = yard();
    let mine = edit(&base, five_edits);
    let delta = exact_delta(type_id, &base, &mine).to_bytes();

    // The base lost crate 1 (moved by the delta) and crate 3 (removed by it).
    let changed = edit(&base, |group| {
        let three = object(group, 3);
        group.remove_object(three).unwrap();
        let one = object(group, 1);
        group.remove_object(one).unwrap();
    });
    let applied = apply_resource(type_id, &delta, &changed).unwrap();
    assert_eq!(applied.applied, 3);
    assert_eq!(applied.skipped.len(), 2);
    let notes: Vec<String> = applied.skipped.iter().map(Note::to_string).collect();
    assert_eq!(
        notes[0],
        format!(
            "operation 1 (object {}): the object is not in the group",
            guid(3)
        )
    );
    assert!(
        notes[1].contains(&guid(1).to_string())
            && notes[1].contains("CSystemCommands.CMD_LoadMatrix")
            && notes[1].ends_with("the object is not in the group"),
        "{notes:?}"
    );
    // The rest is there: the attribute, the model, the new object.
    let group = SimGroup::parse(&applied.data).unwrap();
    assert_eq!(group.objects.len(), 3);
    let two = &group.objects[object(&group, 2)];
    assert_eq!(group.attr(two, CLASS_ENTITY, 3), Some(&Value::Int(1)));
    assert_eq!(group.model(&group.objects[0]), Some(0x1234_5678));
    let new = &group.objects[object(&group, 9)];
    assert_eq!(group.transform(new).unwrap()[3], [7.0, 8.0, 9.0, 1.0]);

    // A class the object does not have, an attribute the class does not
    // have and a value of another kind are skipped too.
    let hand = format!(
        r#"{{"format":1,"kind":"simgroup","ops":[
            {{"op":"set","object":"{g}","attribute":"11111111.0","new":{{"int":1}}}},
            {{"op":"set","object":"{g}","attribute":"38523fc3.9","new":{{"int":1}}}},
            {{"op":"set","object":"{g}","attribute":"38523fc3.2","new":{{"int":1}}}},
            {{"op":"set","object":"{g}","attribute":"38523fc3.3","new":{{"int":1}}}}
        ]}}"#,
        g = guid(2)
    );
    let applied = apply_resource(type_id, hand.as_bytes(), &base).unwrap();
    assert_eq!(applied.applied, 1);
    let reasons: Vec<&str> = applied.skipped.iter().map(|n| n.reason.as_str()).collect();
    assert_eq!(reasons[0], "the object has no packet of this class");
    assert!(reasons[1].contains("4 attribute(s)"), "{reasons:?}");
    assert!(reasons[2].contains("cannot hold"), "{reasons:?}");
    assert_eq!(applied.skipped[2].op, 2);

    // Nothing applies: the base comes back as it is.
    let nothing = format!(
        r#"{{"format":1,"kind":"simgroup","ops":[{{"op":"remove","object":"{}"}}]}}"#,
        guid(77)
    );
    let applied = apply_resource(type_id, nothing.as_bytes(), &base).unwrap();
    assert_eq!((applied.applied, applied.skipped.len()), (0, 1));
    assert_eq!(applied.data, base);
}

#[test]
fn simgroup_conflicts_and_taken_guids_are_told() {
    let (type_id, base) = yard();
    let mine = edit(&base, five_edits);
    let delta = exact_delta(type_id, &base, &mine).to_bytes();

    // Something else set the attribute the delta sets, to another value.
    let changed = edit(&base, |group| {
        let two = object(group, 2);
        group
            .set_class_attr(two, CLASS_ENTITY, 3, Some(Value::Int(5)))
            .unwrap();
    });
    let applied = apply_resource(type_id, &delta, &changed).unwrap();
    assert!(applied.skipped.is_empty());
    assert_eq!(applied.conflicts.len(), 1);
    let note = applied.conflicts[0].to_string();
    assert!(
        note.contains(&guid(2).to_string())
            && note.contains("Entity.CMD_SetInteriorLit")
            && note.contains("the value was 5, not default"),
        "{note}"
    );
    // The delta wins.
    assert_eq!(view(&applied.data), view(&mine));

    // The same value set by both is not a conflict.
    let agreed = edit(&base, |group| {
        let two = object(group, 2);
        group
            .set_class_attr(two, CLASS_ENTITY, 3, Some(Value::Int(1)))
            .unwrap();
    });
    let applied = apply_resource(type_id, &delta, &agreed).unwrap();
    assert!(applied.conflicts.is_empty());

    // The new object is already there: an error, whatever it holds.
    let error = apply_resource(type_id, &delta, &mine).unwrap_err();
    assert!(
        error.to_string().contains(&format!(
            "an object with GUID {} is already in the group",
            guid(9)
        )),
        "{error}"
    );
}

#[test]
fn simgroup_changes_that_are_not_operations_keep_the_resource() {
    let (type_id, base) = yard();
    let changes: [fn(&mut SimGroup); 5] = [
        |group| group.objects[1].flags2 ^= 1,
        |group| group.objects.swap(0, 1),
        |group| group.group_hash ^= 1,
        |group| {
            // A new object before the others.
            group.duplicate_object(0, guid(9)).unwrap();
            let new = group.objects.pop().unwrap();
            group.objects.insert(0, new);
        },
        |group| {
            // A link is not an attribute.
            group.objects[2].links.push(Guid::default());
            group.objects[2].link_refs.push(0);
            group.set_link(2, 0, Guid::from_resource_id(7)).unwrap();
        },
    ];
    for (n, change) in changes.into_iter().enumerate() {
        let mine = edit(&base, change);
        assert_ne!(mine, base, "change {n}");
        assert!(
            diff_resource(type_id, &base, &mine).unwrap().is_none(),
            "change {n}"
        );
        assert!(exact(type_id, &base, &mine).unwrap().is_none());
    }
    assert!(diff_resource(type_id, b"junk", &base).is_err());
    assert!(diff_resource(type_id, &base, b"junk").is_err());
}

#[test]
fn simgroup_documents_are_checked() {
    let (type_id, base) = yard();
    let add = |object: &str, attrs: &str| {
        format!(
            r#"{{"format":1,"kind":"simgroup","ops":[{{"op":"add","object":"{object}",
            "class":"38523fc3","flags":3,"flags2":1,
            "packets":[{{"class":"38523fc3","count":5,"attrs":[{attrs}]}}]}}]}}"#
        )
    };
    let good = add(
        &guid(40).to_string(),
        r#"{"index":2,"value":{"string":"made by hand"}},{"index":4,"value":{"int":2}}"#,
    );
    let applied = apply_resource(type_id, good.as_bytes(), &base).unwrap();
    let group = SimGroup::parse(&applied.data).unwrap();
    assert_eq!(name_of(&group, object(&group, 40)), "made by hand");
    assert_eq!(view(&applied.data).len(), 5);

    for (bad, why) in [
        (
            add(
                &guid(40).to_string(),
                r#"{"index":4,"value":{"int":2}},{"index":2,"value":{"int":1}}"#,
            ),
            "ascending",
        ),
        (
            add(&guid(40).to_string(), r#"{"index":5,"value":{"int":2}}"#),
            "ascending",
        ),
        (
            add(&guid(40).to_string(), r#"{"index":1,"value":{"int":0}}"#),
            "default",
        ),
        (
            add(
                &guid(40).to_string(),
                r#"{"index":1,"value":{"string":"a\u0000b"}}"#,
            ),
            "NUL",
        ),
        (
            add("07000000070000000700000007000000", ""),
            "not an object GUID",
        ),
        (add(&guid(40).to_string(), r#"{"index":1}"#), "value"),
        (
            good.replace(r#""flags2":1"#, r#""flags2":1,"more":0"#),
            "more",
        ),
        (
            good.replace(r#""op":"add""#, r#""op":"replace""#),
            "replace",
        ),
        (
            good.replace(r#""class":"38523fc3","flags""#, r#""class":"xyz","flags""#),
            "hexadecimal",
        ),
    ] {
        let error = apply_resource(type_id, bad.as_bytes(), &base)
            .unwrap_err()
            .to_string();
        assert!(error.contains(why), "{why}: {error}");
    }
    let bad_attribute = format!(
        r#"{{"format":1,"kind":"simgroup","ops":[{{"op":"set","object":"{}","attribute":"Entity.4"}}]}}"#,
        guid(1)
    );
    assert!(Delta::parse(bad_attribute.as_bytes()).is_err());
}

// --------------------------------------------------------------- strings

fn strings() -> (u32, Vec<u8>) {
    let (type_id, data) = resource(testing::LEVEL, testing::STRINGS);
    assert_eq!(kind_of(type_id), Some(Kind::Lh2));
    (type_id, data)
}

fn edit_table(data: &[u8], change: impl FnOnce(&mut StringTable)) -> Vec<u8> {
    let mut table = StringTable::parse(data).unwrap();
    change(&mut table);
    table.to_bytes().unwrap()
}

#[test]
fn string_edits_round_trip() {
    let (type_id, base) = strings();
    let mine = edit_table(&base, |table| {
        table.strings[0][1] = "Mondo".into();
        table.insert(7, &["Seven".into()]).unwrap();
    });
    let delta = exact_delta(type_id, &base, &mine);
    // The whole document, as it is stored: a table without language codes
    // has the column `0`.
    assert_eq!(
        delta_text(&delta),
        concat!(
            r#"{"format":1,"kind":"lh2","ops":["#,
            r#"{"op":"set","hash":"00000002","language":"0","old":"World","text":"Mondo"},"#,
            r#"{"op":"add","hash":"00000007","texts":[{"language":"0","text":"Seven"}]}]}"#
        )
    );
    assert!(exact_delta(type_id, &base, &base).is_empty());

    // A removed string and a changed header are not operations.
    let mut table = StringTable::parse(&base).unwrap();
    table.hashes.remove(0);
    table.strings[0].remove(0);
    let removed = table.to_bytes().unwrap();
    assert!(diff_resource(type_id, &base, &removed).unwrap().is_none());
    let flagged = edit_table(&base, |table| table.flags = 0);
    assert!(diff_resource(type_id, &base, &flagged).unwrap().is_none());
    let named = edit_table(&base, |table| table.languages = vec!["en".into()]);
    assert!(diff_resource(type_id, &base, &named).unwrap().is_none());
}

#[test]
fn string_delta_applies_to_a_changed_base() {
    let (type_id, base) = strings();
    let mine = edit_table(&base, |table| {
        table.strings[0][1] = "Mondo".into();
        table.insert(7, &["Seven".into()]).unwrap();
    });
    let delta = exact_delta(type_id, &base, &mine).to_bytes();

    // Another mod changed the other string and added its own.
    let changed = edit_table(&base, |table| {
        table.strings[0][0] = "Hallo".into();
        table.insert(9, &["Nine".into()]).unwrap();
        table.insert(3, &["Three".into()]).unwrap();
    });
    let applied = apply_resource(type_id, &delta, &changed).unwrap();
    assert!(applied.skipped.is_empty() && applied.conflicts.is_empty());
    let table = StringTable::parse(&applied.data).unwrap();
    assert_eq!(table.hashes, [1, 2, 3, 7, 9]);
    assert_eq!(
        table.strings[0],
        ["Hallo", "Mondo", "Three", "Seven", "Nine"]
    );
    let by_hand = edit_table(&changed, |table| {
        table.strings[0][1] = "Mondo".into();
        table.insert(7, &["Seven".into()]).unwrap();
    });
    assert_eq!(applied.data, by_hand);

    // The string was changed by something else: the delta wins, and says so.
    let other = edit_table(&base, |table| table.strings[0][1] = "Welt".into());
    let applied = apply_resource(type_id, &delta, &other).unwrap();
    assert_eq!(applied.data, mine);
    assert_eq!(applied.conflicts.len(), 1);
    assert_eq!(
        applied.conflicts[0].to_string(),
        "operation 1 (string 00000002, language 0): the text was \"Welt\", not \"World\" as \
         when the delta was made"
    );

    // The string is gone: skipped, the new string is added.
    let mut table = StringTable::parse(&base).unwrap();
    table.hashes.remove(1);
    table.strings[0].remove(1);
    let applied = apply_resource(type_id, &delta, &table.to_bytes().unwrap()).unwrap();
    assert_eq!(applied.applied, 1);
    assert_eq!(
        applied.skipped[0].to_string(),
        "operation 1 (string 00000002, language 0): the table has no such string"
    );
    assert_eq!(
        StringTable::parse(&applied.data).unwrap().hashes,
        [1, 7],
        "the rest applies"
    );

    // The hash of the new string is taken: an error.
    let error = apply_resource(type_id, &delta, &mine).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("the table already has an entry 00000007"),
        "{error}"
    );
}

#[test]
fn string_columns_are_languages() {
    let type_id = Kind::Lh2.type_id();
    let base = StringTable {
        version: 1,
        flags: 0x4000,
        languages: vec!["en".into(), "cs".into()],
        hashes: vec![1, 2],
        strings: vec![
            vec!["One".into(), "Two".into()],
            vec!["Jedna".into(), "Dva".into()],
        ],
    };
    let mut mine = base.clone();
    mine.strings[1][0] = "Jednička".into();
    mine.insert(5, &["Five".into(), "Pět".into()]).unwrap();
    let (base, mine) = (base.to_bytes().unwrap(), mine.to_bytes().unwrap());
    let delta = exact_delta(type_id, &base, &mine);
    let text = delta_text(&delta);
    assert!(
        text.contains(r#""language":"cs","old":"Jedna","text":"Jednička""#),
        "{text}"
    );

    // A table with the languages in another order and one more: the texts
    // follow the codes, and the language the delta has no text for gets the
    // first one.
    let other = StringTable {
        version: 1,
        flags: 0x4000,
        languages: vec!["cs".into(), "de".into(), "en".into()],
        hashes: vec![1],
        strings: vec![
            vec!["Jedna".into()],
            vec!["Eins".into()],
            vec!["One".into()],
        ],
    };
    let applied = apply_resource(type_id, &delta.to_bytes(), &other.to_bytes().unwrap()).unwrap();
    let table = StringTable::parse(&applied.data).unwrap();
    assert_eq!(table.texts(1).unwrap(), ["Jednička", "Eins", "One"]);
    assert_eq!(table.texts(5).unwrap(), ["Pět", "Five", "Five"]);

    // A language the table does not have.
    let hand = br#"{"format":1,"kind":"lh2","ops":[
        {"op":"set","hash":"00000001","language":"fr","text":"Un"}]}"#;
    let applied = apply_resource(type_id, hand, &base).unwrap();
    assert_eq!(applied.skipped[0].reason, "the table has no such language");
    assert_eq!(applied.data, base);
    // A text with a NUL cannot be written.
    let hand = br#"{"format":1,"kind":"lh2","ops":[
        {"op":"set","hash":"00000001","language":"en","text":"a\u0000b"}]}"#;
    assert!(apply_resource(type_id, hand, &base).is_err());
}

// ---------------------------------------------------------------- parts

fn parts() -> (u32, Vec<u8>) {
    let (type_id, data) = resource(testing::LEVEL, testing::PARTSDB);
    assert_eq!(kind_of(type_id), Some(Kind::PartsDb));
    (type_id, data)
}

fn edit_parts(data: &[u8], change: impl FnOnce(&mut PartsDb)) -> Vec<u8> {
    let mut db = PartsDb::parse(data).unwrap();
    change(&mut db);
    db.encode().unwrap()
}

fn new_part(name: &str, slot: usize, model: u32) -> Part {
    Part {
        name: name.into(),
        slot: Some(slot),
        models: [Hash32(model), Hash32(0), Hash32(0), Hash32(0), Hash32(0)],
        label: String::new(),
        tags: vec!["hatCulled".into()],
    }
}

/// A new hat: a part, a variant of it and the variant allowed in the set.
fn add_cap(db: &mut PartsDb) {
    let part = db.add_part(new_part("cap", 1, 0x77)).unwrap();
    let variant = db.add_variant("cap", vec![part as u16]).unwrap();
    assert!(db.allow_variant(0, 1, variant, false).unwrap());
}

#[test]
fn parts_edits_round_trip() {
    let (type_id, base) = parts();
    let mine = edit_parts(&base, add_cap);
    let delta = exact_delta(type_id, &base, &mine);
    assert_eq!(
        delta_text(&delta),
        concat!(
            r#"{"format":1,"kind":"partsdb","ops":["#,
            r#"{"op":"add_part","name":"cap","slot":"a_hat","#,
            r#""models":["00000077","00000000","00000000","00000000","00000000"],"#,
            r#""tags":["hatCulled"]},"#,
            r#"{"op":"add_variant","name":"cap","parts":["cap"]},"#,
            r#"{"op":"allow","set":"db","slot":"a_hat","variant":"cap"}]}"#
        )
    );

    // The variant as the only choice of a slot.
    let only = edit_parts(&base, |db| {
        assert!(db.allow_variant(0, 1, 1, true).unwrap());
    });
    let delta = exact_delta(type_id, &base, &only);
    assert!(delta_text(&delta).ends_with(r#""variant":"hat","only":true}]}"#));

    // Anything else keeps the database whole.
    let changes: [fn(&mut PartsDb); 4] = [
        |db| db.parts[0].models[1] = Hash32(1),
        |db| db.slots.push("a_glasses".into()),
        |db| {
            db.clone_set(0, "db2").unwrap();
        },
        |db| db.sets[0].choices[1].variants.reverse(),
    ];
    for (n, change) in changes.into_iter().enumerate() {
        let mine = edit_parts(&base, change);
        assert!(
            diff_resource(type_id, &base, &mine).unwrap().is_none(),
            "change {n}"
        );
    }
}

#[test]
fn parts_delta_applies_to_a_changed_base() {
    let (type_id, base) = parts();
    let mine = edit_parts(&base, add_cap);
    let delta = exact_delta(type_id, &base, &mine).to_bytes();

    // Another mod added its own part and variant first: the numbers of the
    // author's part and variant are other ones, the names find them.
    let other = |db: &mut PartsDb| {
        let part = db.add_part(new_part("beret", 1, 0x88)).unwrap();
        let variant = db.add_variant("beret", vec![part as u16]).unwrap();
        db.allow_variant(0, 1, variant, false).unwrap();
    };
    let changed = edit_parts(&base, other);
    let applied = apply_resource(type_id, &delta, &changed).unwrap();
    assert!(applied.skipped.is_empty() && applied.conflicts.is_empty());
    assert_eq!(applied.data, edit_parts(&changed, add_cap));
    let db = PartsDb::parse(&applied.data).unwrap();
    let cap = db.variant_index("cap").unwrap();
    assert_eq!(
        db.variants[cap].parts,
        [db.part_index("cap").unwrap() as u16]
    );
    assert_eq!(
        db.sets[0].choices[1].variants,
        [2, 1, db.variant_index("beret").unwrap(), cap]
    );

    // The set and the slot are gone: what needs them is skipped.
    let changed = edit_parts(&base, |db| {
        db.sets[0].name = "other".into();
    });
    let applied = apply_resource(type_id, &delta, &changed).unwrap();
    assert_eq!(applied.applied, 2);
    assert_eq!(
        applied.skipped[0].to_string(),
        "operation 3 (variant `cap` in slot `a_hat` of set `db`): the database has no set `db`"
    );
    let hand = br#"{"format":1,"kind":"partsdb","ops":[
        {"op":"add_part","name":"visor","slot":"a_visor","models":["00000001","00000000","00000000","00000000","00000000"]},
        {"op":"add_variant","name":"visor","parts":["visor"]},
        {"op":"allow","set":"db","slot":"a_hat","variant":"visor"}]}"#;
    let applied = apply_resource(type_id, hand, &base).unwrap();
    let reasons: Vec<&str> = applied.skipped.iter().map(|n| n.reason.as_str()).collect();
    assert_eq!(
        reasons,
        [
            "the database has no slot `a_visor`",
            "the database has no part `visor`",
            "the database has no variant `visor`"
        ]
    );
    assert_eq!(applied.data, base);

    // The names are taken: an error.
    let error = apply_resource(type_id, &delta, &mine).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("the database already has a part named `cap`"),
        "{error}"
    );
}

// ----------------------------------------------------------- world grid

fn stream_guid(seed: u8) -> [u8; 16] {
    let mut guid = *b"\0\0\0X\0\0\0\0DOCTORTK";
    guid[1] = seed;
    guid
}

/// The world grid of the town with a stream table of two streams, the
/// second needing the first.
fn grid() -> (u32, Vec<u8>) {
    let (type_id, data) = resource(testing::TOWN, 0);
    assert_eq!(kind_of(type_id), Some(Kind::Wgi));
    let data = edit_grid(&data, |grid| {
        grid.streams = vec![
            GridStream {
                guid: Guid(stream_guid(1)),
                needs: None,
            },
            GridStream {
                guid: Guid(stream_guid(2)),
                needs: None,
            },
        ];
        grid.streams[1].needs = grid.intern_list(&[0]);
    });
    (type_id, data)
}

fn edit_grid(data: &[u8], change: impl FnOnce(&mut WorldGrid)) -> Vec<u8> {
    let mut grid = WorldGrid::parse(data).unwrap();
    change(&mut grid);
    grid.to_bytes().unwrap()
}

fn zone(seed: u8, needs: Option<u8>) -> NewCell {
    NewCell {
        guid: stream_guid(seed),
        box_min: [f32::from(seed), 0.0, -4.0],
        box_max: [f32::from(seed) + 20.0, 6.0, 4.0],
        kind: wgi::KIND_WORLD,
        flags: 0,
        lod_mask: wgi::lod::HIGH,
        name: Some(format!("Zone_{seed}")),
        needs: needs.map(|parent| crate::toc::guid_hash(&stream_guid(parent))),
    }
}

#[test]
fn grid_edits_round_trip() {
    let (type_id, base) = grid();
    let yard = WorldGrid::parse(&base).unwrap().cells[0].guid;
    // A zone that needs the second stream, a zone that needs nothing.
    let mine = edit_grid(&base, |grid| {
        grid.add_cell(&zone(10, Some(2))).unwrap();
        grid.add_cell(&zone(11, None)).unwrap();
    });
    let delta = exact_delta(type_id, &base, &mine);
    let text = delta_text(&delta);
    assert!(
        text.starts_with(&format!(
            r#"{{"format":1,"kind":"wgi","ops":[{{"op":"add","stream":"{}","centre":[20.0,3.0,0.0],"#,
            Guid(stream_guid(10))
        )),
        "{text}"
    );
    // The stream the cell needs is named by GUID.
    assert!(
        text.contains(&format!(r#""needs":["{}"]"#, Guid(stream_guid(2)))),
        "{text}"
    );
    assert_eq!(delta.len(), 2);

    // The cell of the yard removed, and removed and added in one delta.
    let removed = edit_grid(&base, |grid| {
        grid.remove_cell(0).unwrap();
        grid.trim_pool();
    });
    let delta = exact_delta(type_id, &base, &removed);
    assert_eq!(
        delta_text(&delta),
        format!(r#"{{"format":1,"kind":"wgi","ops":[{{"op":"remove","stream":"{yard}"}}]}}"#)
    );
    let both = edit_grid(&mine, |grid| {
        grid.remove_cell(0).unwrap();
        grid.trim_pool();
    });
    assert_eq!(exact_delta(type_id, &base, &both).len(), 3);
    // Back from the two zones to the base: the pool word of the first goes.
    assert_eq!(exact_delta(type_id, &mine, &base).len(), 2);

    // A changed stream table, a changed cell and a cell in a ring with
    // another keep the grid whole.
    let changes: [fn(&mut WorldGrid); 3] = [
        |grid| grid.streams[0].guid = Guid(stream_guid(9)),
        |grid| grid.cells[0].lod_mask = wgi::lod::LOW,
        |grid| {
            grid.add_cell(&zone(10, None)).unwrap();
            grid.cells[0].ring_next = 1;
            grid.cells[1].ring_next = 0;
        },
    ];
    for (n, change) in changes.into_iter().enumerate() {
        let mine = edit_grid(&base, change);
        assert!(
            diff_resource(type_id, &base, &mine).unwrap().is_none(),
            "change {n}"
        );
    }
}

#[test]
fn grid_delta_applies_to_a_changed_base() {
    let (type_id, base) = grid();
    let yard = WorldGrid::parse(&base).unwrap().cells[0].guid;
    let add_zones = |grid: &mut WorldGrid| {
        grid.add_cell(&zone(10, Some(2))).unwrap();
        grid.add_cell(&zone(11, None)).unwrap();
    };
    let mine = edit_grid(&base, add_zones);
    let delta = exact_delta(type_id, &base, &mine).to_bytes();

    // Another mod added its zone to the same city, and a stream to the
    // table before the others.
    let changed = edit_grid(&base, |grid| {
        grid.add_cell(&zone(30, Some(1))).unwrap();
    });
    let applied = apply_resource(type_id, &delta, &changed).unwrap();
    assert!(applied.skipped.is_empty() && applied.conflicts.is_empty());
    assert_eq!(applied.data, edit_grid(&changed, add_zones));
    let both = WorldGrid::parse(&applied.data).unwrap();
    let guids: Vec<Guid> = both.cells.iter().map(|c| c.guid).collect();
    assert_eq!(
        guids,
        [
            yard,
            Guid(stream_guid(30)),
            Guid(stream_guid(10)),
            Guid(stream_guid(11))
        ]
    );
    // Each zone needs its own parent.
    assert_eq!(both.list(&both.cells[1].needs), [0]);
    assert_eq!(both.list(&both.cells[2].needs), [1]);
    assert!(
        both.cells
            .iter()
            .enumerate()
            .all(|(i, c)| c.ring_next as usize == i)
    );

    // The stream table lost the parent: that zone is skipped, the other is in.
    let changed = edit_grid(&base, |grid| {
        grid.streams[1].guid = Guid(stream_guid(3));
    });
    let applied = apply_resource(type_id, &delta, &changed).unwrap();
    assert_eq!(applied.applied, 1);
    assert_eq!(
        applied.skipped[0].to_string(),
        format!(
            "operation 1 (cell of stream {}): the cell needs stream {}, which is not in the \
             stream table of the grid",
            Guid(stream_guid(10)),
            Guid(stream_guid(2))
        )
    );
    assert_eq!(WorldGrid::parse(&applied.data).unwrap().cells.len(), 2);

    // A cell to remove that is not there.
    let removed = edit_grid(&base, |grid| {
        grid.remove_cell(0).unwrap();
    });
    let remove = exact_delta(type_id, &base, &removed).to_bytes();
    let applied = apply_resource(type_id, &remove, &removed).unwrap();
    assert_eq!(applied.skipped[0].reason, "the grid has no such cell");
    assert_eq!(applied.data, removed);

    // The stream has a cell already: an error.
    let error = apply_resource(type_id, &delta, &mine).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("the grid already has a cell of stream"),
        "{error}"
    );
    // The fields the game does not read have their size.
    let hand = String::from_utf8(delta).unwrap().replace(
        r#""kind":0,"flags":0"#,
        r#""kind":0,"flags":0,"unused_24":"01""#,
    );
    assert!(apply_resource(type_id, hand.as_bytes(), &base).is_err());
}
