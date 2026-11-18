//! Delta documents: parse, write, parse again, and apply to a small
//! resource of the kind.
#![no_main]

use std::sync::OnceLock;

use doctor_toolkit::resource::delta::{Delta, Kind};
use doctor_toolkit::resource::lh2::StringTable;
use doctor_toolkit::resource::partsdb::PartsDb;
use doctor_toolkit::resource::simgroup::{
    CLASS_ENTITY, CLASS_SYSTEM_COMMANDS, Guid, SimGroup, Value,
};
use doctor_toolkit::resource::wgi::{self, GridStream, NewCell, WorldGrid};
use doctor_toolkit::workspace::store::sha256_hex;
use libfuzzer_sys::fuzz_target;

fn guid(n: u8) -> [u8; 16] {
    let mut bytes = *b"\0\0\0X\0\0\0\0DOCTORTK";
    bytes[0] = n;
    bytes
}

/// Three placed objects, `guid(1)` to `guid(3)`, with a model and a name.
fn group() -> Vec<u8> {
    let mut group = SimGroup::empty();
    for n in 1..=3u8 {
        let object = group
            .push_object(
                CLASS_ENTITY,
                Guid(guid(n)),
                &[(CLASS_SYSTEM_COMMANDS, 4), (CLASS_ENTITY, 5)],
            )
            .expect("a new object");
        group
            .set_position(object, [f32::from(n), 0.0, 2.0])
            .expect("a transform");
        group.set_model(object, Some(0x7001_0001)).expect("a model");
        let name = Value::String(format!("crate_{n}"));
        group
            .set_class_attr(object, CLASS_ENTITY, 2, Some(name))
            .expect("a name");
    }
    group.encode()
}

fn table() -> Vec<u8> {
    StringTable {
        version: 1,
        flags: 0x4000,
        languages: vec!["en".into(), "cs".into()],
        hashes: vec![1, 2],
        strings: vec![
            vec!["One".into(), "Two".into()],
            vec!["Jedna".into(), "Dva".into()],
        ],
    }
    .to_bytes()
    .expect("a table")
}

fn parts() -> Vec<u8> {
    let db: PartsDb = serde_json::from_str(
        r#"{"name":"db","kind":2,"slots":["head","a_hat"],
        "parts":[{"name":"head","slot":0,"models":["00000001","00000002","00000000","00000000","00000000"]},
                 {"name":"hat","slot":1,"models":["00000003","00000000","00000000","00000000","00000000"],"tags":["hatCulled"]}],
        "variants":[{"name":"head","parts":[0]},{"name":"hat","parts":[1]}],
        "presets":[],"blenders":[],
        "sets":[{"name":"db","choices":[{"slot":0,"variants":[0]},{"slot":1,"variants":[1]}]}],
        "groups":[{"name":"_all","sets":[0]}]}"#,
    )
    .expect("a database");
    db.encode().expect("a database")
}

fn grid() -> Vec<u8> {
    let mut grid = WorldGrid {
        streams: vec![
            GridStream {
                guid: Guid(guid(1)),
                needs: None,
            },
            GridStream {
                guid: Guid(guid(2)),
                needs: None,
            },
        ],
        cells: Vec::new(),
        pool: Vec::new(),
    };
    grid.streams[1].needs = grid.intern_list(&[0]);
    grid.add_cell(&NewCell {
        guid: guid(10),
        box_min: [0.0, -1.0, 0.0],
        box_max: [20.0, 10.0, 20.0],
        kind: wgi::KIND_WORLD,
        flags: 0,
        lod_mask: wgi::lod::HIGH,
        name: Some("Yard".into()),
        needs: Some(grid.streams[1].hash()),
    })
    .expect("a cell");
    grid.to_bytes().expect("a grid")
}

/// A resource of each kind for the operations to find their targets in.
fn base(kind: Kind) -> &'static [u8] {
    static BASES: OnceLock<[Vec<u8>; 4]> = OnceLock::new();
    let bases = BASES.get_or_init(|| [group(), table(), parts(), grid()]);
    match kind {
        Kind::SimGroup => &bases[0],
        Kind::Lh2 => &bases[1],
        Kind::PartsDb => &bases[2],
        Kind::Wgi => &bases[3],
    }
}

fuzz_target!(|data: &[u8]| {
    let Ok(mut delta) = Delta::parse(data) else {
        return;
    };
    let bytes = delta.to_bytes();
    let again = Delta::parse(&bytes).expect("a written delta parses");
    assert_eq!(again.to_bytes(), bytes, "delta round trip");
    assert_eq!(again.kind(), delta.kind());
    assert_eq!(again.len(), delta.len());

    let base = base(delta.kind());
    // A layout is used on the base it names: name this one, so the fuzzer
    // reaches the code that moves the data.
    if let Delta::SimGroup(group) = &mut delta
        && let Some(layout) = &mut group.layout
    {
        layout.base = sha256_hex(base);
    }
    let Ok(applied) = delta.apply(base) else {
        return;
    };
    assert_eq!(applied.applied + applied.skipped.len(), delta.len());
    // Whatever applied, the result is a resource of its kind.
    match delta.kind() {
        Kind::SimGroup => {
            SimGroup::parse(&applied.data).expect("the group parses");
        }
        Kind::Lh2 => {
            StringTable::parse(&applied.data).expect("the table parses");
        }
        Kind::PartsDb => {
            PartsDb::parse(&applied.data).expect("the database parses");
        }
        Kind::Wgi => {
            WorldGrid::parse(&applied.data).expect("the grid parses");
        }
    }
});
