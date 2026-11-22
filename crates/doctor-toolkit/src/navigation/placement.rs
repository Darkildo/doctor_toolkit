//! Which streams hold a copy of a resource (`docs/analysis/resource-placement.md`).
//!
//! The game has no table "resource → stream": a stream can use a resource
//! only if the stream or one of its prerequisites in the tables of contents
//! holds it, or a stream its data loads (a `StreamSet`, the animations of an
//! `InterestingObject`). The game's build tools therefore put a copy of a
//! resource into every stream whose resources refer to it, unless a
//! prerequisite of that stream already provides it. On the whole game every
//! stream that refers to a resource reaches a copy this way, and every copy
//! lies in a stream that uses it or in a prerequisite of such streams.
//!
//! [`Placement`] computes this over a [`GameIndex`]: who uses a resource
//! (the streams of the records that refer to it, by the references of the
//! index and, for particle effects, by name), where its copies are, which
//! users reach none, and where new copies go ([`Placement::place`]: every
//! user that reaches no copy, except one under another such user).

use std::collections::{HashMap, HashSet};

use crate::game;
use crate::hash::sdbm;
use crate::resource::material_swap::TYPE_MATERIAL_SWAP_LIST;
use crate::resource::simgroup::{Attr, SimGroup, Value};
use crate::resource::texture::TYPE_TG4H;
use crate::workspace::key::normalize_stream;

use super::index::{Anchor, GameIndex, NameKind, RecordId, Target};

const TYPE_VFX: u32 = sdbm(b"vfx");

/// A resource without its stream: type and ids.
pub type Key = (u32, [u32; 4]);

/// Users and copies of every resource of a game, with what every stream can
/// reach. Stream numbers are those of the index.
#[derive(Debug, Default)]
pub struct Placement {
    /// Prerequisites of each stream, transitively.
    ancestors: Vec<Vec<usize>>,
    /// Streams named by the data of each stream.
    requests: Vec<Vec<usize>>,
    /// What each stream reaches besides itself: its prerequisites, the
    /// streams it or they load by their data, their prerequisites, and so on.
    reach: Vec<Vec<usize>>,
    /// Streams holding each key, in stream order.
    holders: HashMap<Key, Vec<usize>>,
    /// Streams whose records refer to each key, in stream order.
    users: HashMap<Key, Vec<usize>>,
}

impl Placement {
    /// Builds the table from `index` and the prerequisites of the streams
    /// (`GameInstance::stream_parents`).
    pub fn new(index: &GameIndex, parents: &HashMap<String, Vec<String>>) -> Self {
        let streams = index.stream_count();
        let number: HashMap<&str, usize> =
            (0..streams).map(|s| (index.stream_name(s), s)).collect();
        let ancestors: Vec<Vec<usize>> = (0..streams)
            .map(|s| {
                game::ancestors(parents, index.stream_name(s))
                    .iter()
                    .filter_map(|a| number.get(a.as_str()).copied())
                    .collect()
            })
            .collect();
        let requests = requests(index, &number);
        let reach: Vec<Vec<usize>> = (0..streams)
            .map(|s| {
                let mut out: Vec<usize> = Vec::new();
                let mut todo = vec![s];
                while let Some(c) = todo.pop() {
                    for &t in ancestors[c].iter().chain(&requests[c]) {
                        if t != s && !out.contains(&t) {
                            out.push(t);
                            todo.push(t);
                        }
                    }
                }
                out.sort_unstable();
                out
            })
            .collect();

        let key_of = |id: RecordId| -> Option<Key> {
            let header = &index.record(id).header;
            (header.ids != [0; 4]).then_some((header.type_id, header.ids))
        };
        let mut holders: HashMap<Key, Vec<usize>> = HashMap::new();
        // Effects name their textures and the effects they start.
        let mut by_name: HashMap<String, Vec<Key>> = HashMap::new();
        for stream in 0..streams {
            for id in index.stream_records(stream) {
                let Some(key) = key_of(id) else { continue };
                let list = holders.entry(key).or_default();
                if list.last() != Some(&stream) {
                    list.push(stream);
                }
                if list.len() == 1
                    && matches!(key.0, TYPE_VFX | TYPE_TG4H)
                    && let Some(name) = index.record(id).name()
                {
                    let names = by_name.entry(name.to_ascii_lowercase()).or_default();
                    if !names.contains(&key) {
                        names.push(key);
                    }
                }
            }
        }

        // A swap list has the GUID of the SimGroup object it applies to.
        let swap_lists: HashMap<[u8; 16], Key> = holders
            .keys()
            .filter(|(type_id, _)| *type_id == TYPE_MATERIAL_SWAP_LIST)
            .map(|&(type_id, ids)| {
                let mut guid = [0u8; 16];
                for (k, word) in ids.iter().enumerate() {
                    guid[4 * k..4 * k + 4].copy_from_slice(&word.to_le_bytes());
                }
                (guid, (type_id, ids))
            })
            .collect();
        let mut users: HashMap<Key, Vec<usize>> = HashMap::new();
        let mut targets: Vec<Key> = Vec::new();
        for (stream, reached) in reach.iter().enumerate() {
            let reaches = |key: &Key| {
                holders.get(key).is_some_and(|list| {
                    list.iter()
                        .any(|&h| h == stream || reached.binary_search(&h).is_ok())
                })
            };
            let mut add = |candidates: &mut Vec<Key>, own: Option<Key>| {
                candidates.retain(|k| Some(*k) != own);
                candidates.sort_unstable();
                candidates.dedup();
                // Of several candidates (a name hash shared by materials,
                // rigs with the same bones), the ones the stream can reach.
                if candidates.iter().any(reaches) {
                    candidates.retain(reaches);
                }
                for key in candidates.drain(..) {
                    let list = users.entry(key).or_default();
                    if list.last() != Some(&stream) {
                        list.push(stream);
                    }
                }
            };
            for id in index.stream_records(stream) {
                let own = key_of(id);
                for anchor in &index.record(id).anchors {
                    if let Anchor::Guid(guid) = anchor
                        && let Some(&key) = swap_lists.get(guid)
                        && Some(key) != own
                    {
                        targets.push(key);
                        add(&mut targets, own);
                    }
                }
                for (_, found) in index.uses(id) {
                    targets.extend(found.iter().filter_map(|&t| key_of(t)));
                    add(&mut targets, own);
                }
                let record = index.record(id);
                if record.header.type_id == TYPE_VFX {
                    for (kind, text) in &record.names {
                        if *kind != NameKind::Text {
                            continue;
                        }
                        // Names stand after `#` padding in some chunks.
                        let name = text
                            .trim_start_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '_'));
                        if let Some(keys) = by_name.get(&name.to_ascii_lowercase()) {
                            targets.extend(keys.iter().copied());
                            add(&mut targets, own);
                        }
                    }
                }
            }
        }
        Self {
            ancestors,
            requests,
            reach,
            holders,
            users,
        }
    }

    /// Prerequisites of stream `stream`, transitively.
    pub fn ancestors(&self, stream: usize) -> &[usize] {
        &self.ancestors[stream]
    }

    /// Streams the data of stream `stream` loads: GUIDs of its SimGroup that
    /// are no object or graph but the hash of a stream in the tables of
    /// contents (`InterestingObject.8`, `StreamSet`, `Animated.4`, …).
    pub fn requests(&self, stream: usize) -> &[usize] {
        &self.requests[stream]
    }

    /// Whether stream `stream` reaches stream `other`: through its
    /// prerequisites and the streams it or they load by data.
    pub fn reaches(&self, stream: usize, other: usize) -> bool {
        stream == other || self.reach[stream].binary_search(&other).is_ok()
    }

    /// Streams that hold the resource.
    pub fn holders(&self, type_id: u32, ids: [u32; 4]) -> &[usize] {
        self.holders.get(&(type_id, ids)).map_or(&[], Vec::as_slice)
    }

    /// Streams with a resource that refers to the resource.
    pub fn users(&self, type_id: u32, ids: [u32; 4]) -> &[usize] {
        self.users.get(&(type_id, ids)).map_or(&[], Vec::as_slice)
    }

    /// Every resource of the game with its holders.
    pub fn keys(&self) -> impl Iterator<Item = (&Key, &[usize])> {
        self.holders.iter().map(|(k, v)| (k, v.as_slice()))
    }

    /// The users of the resource that reach none of its copies.
    pub fn unreached(&self, type_id: u32, ids: [u32; 4]) -> Vec<usize> {
        let holders = self.holders(type_id, ids);
        self.users(type_id, ids)
            .iter()
            .copied()
            .filter(|&u| !holders.iter().any(|&h| self.reaches(u, h)))
            .collect()
    }

    /// The streams that need a new copy of the resource: [`place`] over its
    /// [`unreached`](Self::unreached) users. Empty for every resource of
    /// the unmodified game except the few listed in the document.
    pub fn copy_streams(&self, type_id: u32, ids: [u32; 4]) -> Vec<usize> {
        self.place(&self.unreached(type_id, ids))
    }

    /// The streams that get a copy of a resource that is in no stream yet
    /// and is used by the streams `users`: [`place`] with the prerequisites
    /// of the game.
    pub fn place(&self, users: &[usize]) -> Vec<usize> {
        place(&self.ancestors, users)
    }
}

/// The streams that get a copy of a resource used by the streams `users`:
/// every user except one with a prerequisite (`ancestors[user]`, transitive)
/// that is a user too. Sorted, without repeats.
pub fn place(ancestors: &[Vec<usize>], users: &[usize]) -> Vec<usize> {
    place_by(users, |u| ancestors[*u].as_slice())
}

/// [`place`] over streams of any kind (names, for streams a workspace
/// creates): `ancestors(user)` gives the prerequisites of a user,
/// transitively. Sorted, without repeats.
pub fn place_by<'a, T, F>(users: &[T], ancestors: F) -> Vec<T>
where
    T: Clone + Ord + 'a,
    F: Fn(&T) -> &'a [T],
{
    let mut out: Vec<T> = users
        .iter()
        .filter(|&u| !ancestors(u).iter().any(|a| users.contains(a)))
        .cloned()
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Streams named by the data of every stream: GUID references that resolve
/// to no record but name a stream ([`StreamHashes::stream`]).
fn requests(index: &GameIndex, number: &HashMap<&str, usize>) -> Vec<Vec<usize>> {
    let hashes = StreamHashes::new(index.toc_names(), |name| number.contains_key(name));
    (0..index.stream_count())
        .map(|stream| {
            let mut out: Vec<usize> = Vec::new();
            for id in index.stream_records(stream) {
                for (r, found) in index.uses(id) {
                    if let (Target::Guid(guid), true) = (&r.target, found.is_empty())
                        && let Some(&t) = hashes.stream(guid).and_then(|name| number.get(name))
                        && t != stream
                        && !out.contains(&t)
                    {
                        out.push(t);
                    }
                }
            }
            out.sort_unstable();
            out
        })
        .collect()
}

/// The streams of the tables of contents by the hash of their GUID
/// (`toc::guid_hash`): what a GUID in the data of a SimGroup that is no
/// object stands for when it names a stream (`InterestingObject.8`,
/// `StreamSet`, `Animated.4`, `WorldGraphManualLoadSet`, `Door.23`). The
/// game loads such a stream for the stream whose data names it, so what it
/// holds is reachable from there.
#[derive(Debug, Clone, Default)]
pub struct StreamHashes {
    by_hash: HashMap<u32, String>,
}

impl StreamHashes {
    /// From `(hash, stream path)` pairs of the tables of contents
    /// ([`GameIndex::toc_names`]), keeping the names that are streams
    /// (`is_stream`, given the name as [`normalize_stream`] spells it): the
    /// tables also name streams that are no file of the game, some with the
    /// hash of one that is.
    pub fn new(toc_names: &[(u32, String)], is_stream: impl Fn(&str) -> bool) -> Self {
        Self {
            by_hash: toc_names
                .iter()
                .map(|(hash, path)| (*hash, normalize_stream(path)))
                .filter(|(_, name)| is_stream(name))
                .collect(),
        }
    }

    /// From the tables of contents of `game` and its streams, without an
    /// index.
    pub fn of_game(game: &game::GameInstance) -> Self {
        let streams: HashSet<String> = game.streams().iter().map(|f| f.stream_name()).collect();
        Self::new(&super::index::toc_names(game), |name| {
            streams.contains(name)
        })
    }

    /// The stream (as [`normalize_stream`] spells it) whose hash is the fold
    /// of `guid`.
    pub fn stream(&self, guid: &[u8; 16]) -> Option<&str> {
        self.by_hash
            .get(&crate::toc::guid_hash(guid))
            .map(String::as_str)
    }

    /// The streams the data of SimGroup `group` names
    /// ([`guid_reference`]s that are the hash of a stream), without repeats.
    /// A GUID of an object of another stream that folds to the hash of a
    /// stream would count too; on the game files the streams are those of
    /// [`Placement::requests`].
    pub fn requested_by(&self, group: &SimGroup) -> Vec<&str> {
        let own: HashSet<[u8; 16]> = group.objects.iter().map(|o| o.guid.0).collect();
        let mut out: Vec<&str> = Vec::new();
        for attr in group.packets.iter().flat_map(|p| &p.attrs) {
            if let Some(name) = guid_reference(attr, &own).and_then(|g| self.stream(&g))
                && !out.contains(&name)
            {
                out.push(name);
            }
        }
        out
    }
}

/// The GUID an attribute of a SimGroup refers to by GUID: a GUID value that
/// names no resource, is not nil and is no object of the group itself
/// (`own`). What the index reads as a [`Target::Guid`] reference: an object
/// of another group, a graph, or a stream ([`StreamHashes`]).
pub fn guid_reference(attr: &Attr, own: &HashSet<[u8; 16]>) -> Option<[u8; 16]> {
    match &attr.value {
        Value::Guid(guid)
            if attr.resource.is_none() && !guid.is_nil() && !own.contains(&guid.0) =>
        {
            Some(guid.0)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0 global, 1 level under it, 2 and 3 zones of the level, 4 another
    /// root.
    fn ancestors() -> Vec<Vec<usize>> {
        vec![vec![], vec![0], vec![1, 0], vec![1, 0], vec![]]
    }

    #[test]
    fn every_user_gets_a_copy() {
        assert_eq!(place(&ancestors(), &[2, 3]), [2, 3]);
        assert_eq!(place(&ancestors(), &[3, 4, 3]), [3, 4]);
        assert!(place(&ancestors(), &[]).is_empty());
    }

    #[test]
    fn a_user_under_another_user_gets_none() {
        assert_eq!(place(&ancestors(), &[2, 1]), [1]);
        assert_eq!(place(&ancestors(), &[3, 0, 2, 4]), [0, 4]);
    }

    #[test]
    fn place_works_on_names_too() {
        let ancestors: HashMap<&str, Vec<&str>> = HashMap::from([
            ("gf2/miami", vec!["gf2/globals_global"]),
            ("gf2/miami/zone", vec!["gf2/miami", "gf2/globals_global"]),
            ("gf2/doctor/new", vec!["gf2/miami", "gf2/globals_global"]),
            ("gf2/cuba", vec![]),
        ]);
        assert_eq!(
            place_by(&["gf2/miami/zone", "gf2/cuba", "gf2/doctor/new"], |u| {
                ancestors[u].as_slice()
            }),
            ["gf2/cuba", "gf2/doctor/new", "gf2/miami/zone"]
        );
        assert_eq!(
            place_by(&["gf2/miami/zone", "gf2/miami", "gf2/doctor/new"], |u| {
                ancestors[u].as_slice()
            }),
            ["gf2/miami"]
        );
    }

    #[test]
    fn a_guid_names_the_stream_whose_hash_it_folds_to() {
        // GUID of the shared "globals" stream (`toc::guid_hash`).
        let guid: [u8; 16] = [
            0xa3, 0x01, 0xc9, 0x53, 0x74, 0x50, 0x55, 0x8d, b'G', b'R', b'A', b'N', b'D', b'Q',
            b'U', b'I',
        ];
        let names = [
            (0x43BA_ECA2, "GF2\\Globals_Global.str".to_owned()),
            (0x43BA_ECA2, "gf2\\no_such_file".to_owned()),
        ];
        let hashes = StreamHashes::new(&names, |name| name == "gf2/globals_global");
        assert_eq!(hashes.stream(&guid), Some("gf2/globals_global"));
        assert_eq!(hashes.stream(&[0; 16]), None);
    }

    #[test]
    fn copies_go_to_the_users_that_reach_none() {
        let key: Key = (TYPE_VFX, [1, 2, 3, 4]);
        let reach = ancestors()
            .into_iter()
            .map(|mut a| {
                a.sort_unstable();
                a
            })
            .collect();
        let placement = Placement {
            ancestors: ancestors(),
            requests: vec![Vec::new(); 5],
            reach,
            holders: HashMap::from([(key, vec![2])]),
            users: HashMap::from([(key, vec![2, 3, 4])]),
        };
        assert_eq!(placement.unreached(key.0, key.1), [3, 4]);
        assert_eq!(placement.copy_streams(key.0, key.1), [3, 4]);
        assert!(placement.reaches(3, 0));
        assert!(!placement.reaches(0, 3));
        // A copy in the level serves both zones.
        let mut placement = placement;
        placement.holders.insert(key, vec![1]);
        assert_eq!(placement.copy_streams(key.0, key.1), [4]);
        assert_eq!(placement.place(&[1, 2, 4]), [1, 4]);
    }
}
