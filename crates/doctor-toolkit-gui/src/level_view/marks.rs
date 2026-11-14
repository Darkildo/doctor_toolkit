//! Which objects of a SimGroup the workspace added or changed: the "new"
//! and "edited" marks of the Outliner and Details (stage 53).
//!
//! Objects are matched by GUID, so a removed object does not mark the ones
//! after it as changed (their index moved, not their contents).

use std::collections::HashMap;

use doctor_toolkit::resource::simgroup::{Guid, Object, SimGroup};

/// How an object differs from the game's version of its SimGroup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mark {
    /// Not in the game's version: added or duplicated in the workspace.
    New,
    /// In the game's version with other contents (moved, attributes, model).
    Edited,
}

/// The mark of every object of `current` against `original`.
pub fn group_marks(original: &SimGroup, current: &SimGroup) -> Vec<Option<Mark>> {
    let by_guid: HashMap<Guid, &Object> = original.objects.iter().map(|o| (o.guid, o)).collect();
    current
        .objects
        .iter()
        .map(|o| match by_guid.get(&o.guid) {
            None => Some(Mark::New),
            Some(before) if contents(original, before) != contents(current, o) => {
                Some(Mark::Edited)
            }
            Some(_) => None,
        })
        .collect()
}

/// What an object is made of, as comparable text: class, links, components
/// and the attributes of its packets (offsets left out). Text compares
/// floats by their printed value, so a NaN equals itself.
fn contents(group: &SimGroup, object: &Object) -> String {
    let packets: Vec<String> = group
        .object_packets(object)
        .map(|p| format!("{:?}{:?}", p.class, p.attrs))
        .collect();
    let components = object
        .components
        .map(|c| format!("{:?}", group.packets[c].attrs));
    format!(
        "{:08x}{:?}{:?}{:?}",
        object.class, object.links, components, packets
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_toolkit::resource::simgroup::{ATTR_LOAD_MATRIX, CLASS_SYSTEM_COMMANDS, Value};

    /// An object GUID (not the GUID of a resource id: not uniform).
    fn guid(n: u8) -> Guid {
        let mut g = [n; 16];
        g[15] = 0x77;
        Guid(g)
    }

    fn group(objects: u8) -> SimGroup {
        let mut g = SimGroup::empty();
        for n in 0..objects {
            let o = g
                .push_object(0x1234, guid(n + 1), &[(CLASS_SYSTEM_COMMANDS, 2)])
                .unwrap();
            let mut m = doctor_toolkit::level::IDENTITY;
            m[3][0] = f32::from(n);
            g.set_class_attr(
                o,
                CLASS_SYSTEM_COMMANDS,
                ATTR_LOAD_MATRIX,
                Some(Value::Matrix(m)),
            )
            .unwrap();
        }
        SimGroup::parse(&g.encode()).unwrap()
    }

    #[test]
    fn new_and_edited_objects_are_found_by_guid() {
        let original = group(3);
        assert_eq!(group_marks(&original, &original), [None, None, None]);
        let mut current = original.clone();
        // Object 0 removed: the others moved up but did not change.
        current.remove_object(0).unwrap();
        let mut m = doctor_toolkit::level::IDENTITY;
        m[3][1] = 5.0;
        current.set_transform(1, m).unwrap();
        let copy = current.duplicate_object(0, guid(9)).unwrap();
        let marks = group_marks(&original, &current);
        assert_eq!(marks, [None, Some(Mark::Edited), Some(Mark::New)]);
        assert_eq!(copy, 2);
    }
}
