//! Every SimGroup object the game has (stage 49): not only those with a
//! world matrix near the player ([`crate::Request::Objects`]) but logic,
//! triggers and relays too, from the SimManager's table of groups.

use crate::Guid;

/// Which objects [`crate::Request::World`] returns. Empty fields do not
/// filter.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorldFilter {
    /// Only objects of exactly this class (hash of the name,
    /// [`crate::name_hash`]).
    pub class: Option<u32>,
    /// Only these GUIDs (a search by name maps names to GUIDs on the
    /// client's side, from the SimGroups of its workspace).
    pub guids: Vec<Guid>,
    /// Only objects of this group (the stream that made them).
    pub group: Option<u32>,
    /// `Some(false)`: only objects without a world matrix (logic);
    /// `Some(true)`: only those with one.
    pub positioned: Option<bool>,
}

/// One object of the game.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldObject {
    pub guid: Guid,
    /// Hash of the most derived registered class; 0 when unknown.
    pub class: u32,
    /// The group: id of the stream that made the object (`+0x1C`; the
    /// stream record's `+0x1C`, `0x00446E30`).
    pub group: u32,
    /// Translation of the world matrix, for objects that have one.
    pub position: Option<[f32; 3]>,
    /// Flags word of the object (`+0x14`) as it is: the SimManager sets
    /// `0x20000000` when it registers the object (`0x004467C0`), the
    /// constructor of `Base` `0x08000000` (`0x0046C590`).
    pub flags: u32,
    /// Whether its message listener is switched on (bit 0 of `+0x40`): the
    /// bus calls it without `force` only then (`0x00408A7B`).
    pub listening: bool,
}

/// Answer to [`crate::Request::World`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct World {
    /// Objects that matched, before `max` cut the list.
    pub total: u32,
    /// Objects walked.
    pub walked: u32,
    /// Whether the time budget of the walk ran out (the list is partial).
    pub partial: bool,
    /// Class names of the objects: hash, name.
    pub classes: Vec<(u32, String)>,
    /// Loaded streams by group: id, name (`gf2\cuba\local.str`).
    pub streams: Vec<(u32, String)>,
    pub objects: Vec<WorldObject>,
}

impl World {
    /// Name of a class hash, when the module sent it.
    pub fn class_name(&self, class: u32) -> Option<&str> {
        self.classes
            .iter()
            .find(|(h, _)| *h == class)
            .map(|(_, n)| n.as_str())
    }

    /// Name of the stream of a group.
    pub fn stream_name(&self, group: u32) -> Option<&str> {
        self.streams
            .iter()
            .find(|(g, _)| *g == group)
            .map(|(_, n)| n.as_str())
    }
}

/// Most objects one answer carries (the payload stays under 1 MB).
pub const MAX_WORLD_OBJECTS: u32 = 16_000;
