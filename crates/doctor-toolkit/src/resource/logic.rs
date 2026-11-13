//! Game-logic resources (`simgroup`, `metamodel`, ...) decoded to JSON.
//!
//! One entry point for the command line and the GUI: [`to_json`] picks the
//! parser by type id.

use crate::hash::sdbm;
use crate::resource::graph::Graph;
use crate::resource::metamodel::MetaModel;
use crate::resource::mobface::{MobFace, TYPE_MOBFACE};
use crate::resource::partsdb::PartsDb;
use crate::resource::simgroup::SimGroup;
use crate::resource::variabledictionary::VariableDictionary;
use crate::{Error, Result};

/// Type tags with a JSON decoder, in the order they were implemented.
pub const TAGS: [&str; 6] = [
    "simgroup",
    "metamodel",
    "variabledictionary",
    "partsdb",
    "graph",
    "mobface",
];

pub const TYPE_SIMGROUP: u32 = sdbm(b"simgroup");
pub const TYPE_METAMODEL: u32 = sdbm(b"metamodel");
pub const TYPE_VARIABLE_DICTIONARY: u32 = sdbm(b"variabledictionary");
pub const TYPE_PARTSDB: u32 = sdbm(b"partsdb");
pub const TYPE_GRAPH: u32 = sdbm(b"graph");

/// Whether [`to_json`] can decode resources of `type_id`.
pub fn is_supported(type_id: u32) -> bool {
    [
        TYPE_SIMGROUP,
        TYPE_METAMODEL,
        TYPE_VARIABLE_DICTIONARY,
        TYPE_PARTSDB,
        TYPE_GRAPH,
        TYPE_MOBFACE,
    ]
    .contains(&type_id)
}

/// Decodes a resource of type `type_id` into JSON; `None` for other types.
pub fn to_json(type_id: u32, data: &[u8]) -> Option<Result<serde_json::Value>> {
    fn json<T: serde::Serialize>(value: Result<T>) -> Result<serde_json::Value> {
        serde_json::to_value(value?).map_err(|e| Error::InvalidArgument(e.to_string()))
    }
    Some(match type_id {
        TYPE_SIMGROUP => json(SimGroup::parse(data)),
        TYPE_METAMODEL => json(MetaModel::parse(data)),
        TYPE_VARIABLE_DICTIONARY => json(VariableDictionary::parse(data)),
        TYPE_PARTSDB => json(PartsDb::parse(data)),
        TYPE_GRAPH => json(Graph::parse(data)),
        TYPE_MOBFACE => json(MobFace::parse(data)),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::names::type_tags;

    #[test]
    fn type_ids_match_the_tags() {
        for tag in TAGS {
            let id = type_tags().resolve(tag).unwrap();
            assert!(is_supported(id), "{tag}");
        }
        assert!(to_json(sdbm(b"mesh"), &[]).is_none());
        assert!(to_json(TYPE_SIMGROUP, &[]).unwrap().is_err());
    }
}
