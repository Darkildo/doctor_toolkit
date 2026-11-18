//! Stage 109: delta resources on the files of the game.
//!
//! The mod build stores a delta instead of a resource only when the delta,
//! applied to the base it was made from, gives the resource back byte for
//! byte. These tests make the edits an author makes, in the ways the
//! programs make them (every edit written and read again, or all in one
//! write), and count how often that holds.

use std::collections::{BTreeMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use doctor_toolkit::resource::delta::{self, Delta, Kind};
use doctor_toolkit::resource::simgroup::{Guid, Hash32, Kind as AttrKind, SimGroup, Value};
use doctor_toolkit::resource::simgroup_edit::{Transform, new_object_guid};

use super::{XorShift, check_every, simgroup_view};

/// One edit of a group as the editor makes it, the object named by GUID.
#[derive(Debug, Clone)]
pub(super) enum Edit {
    Move(Guid, [f32; 3]),
    Set(Guid, u32, u16, Option<Value>),
    /// Source, GUID of the copy, offset of the copy.
    Duplicate(Guid, Guid, [f32; 3]),
    /// An object of another group (number in the donor), the GUID of the
    /// copy, its position.
    Copy(usize, Guid, [f32; 3]),
    Remove(Guid),
}

impl Edit {
    /// GUIDs of the objects the edit reads or writes.
    pub(super) fn touches(&self) -> Vec<Guid> {
        match self {
            Self::Move(g, _) | Self::Set(g, ..) | Self::Remove(g) => vec![*g],
            Self::Duplicate(a, b, _) => vec![*a, *b],
            Self::Copy(_, g, _) => vec![*g],
        }
    }
}

fn find(group: &SimGroup, guid: &Guid) -> Option<usize> {
    group.objects.iter().position(|o| o.guid == *guid)
}

/// Makes one edit the way `level::edit` and the logic editor do. `false`
/// when the object is not in the group.
fn play_one(group: &mut SimGroup, donor: &SimGroup, edit: &Edit) -> Result<bool, String> {
    let e = |error: doctor_toolkit::Error| format!("{edit:?}: {error}");
    match edit {
        Edit::Move(guid, position) => {
            let Some(o) = find(group, guid) else {
                return Ok(false);
            };
            group.set_position(o, *position).map_err(e)?;
        }
        Edit::Set(guid, class, index, value) => {
            let Some(o) = find(group, guid) else {
                return Ok(false);
            };
            group
                .set_class_attr(o, *class, *index, value.clone())
                .map_err(e)?;
        }
        Edit::Duplicate(source, guid, offset) => {
            let Some(o) = find(group, source) else {
                return Ok(false);
            };
            let copy = group.duplicate_object(o, *guid).map_err(e)?;
            if let Some(mut m) = group.transform(&group.objects[copy]) {
                for (k, d) in offset.iter().enumerate() {
                    m[3][k] += d;
                }
                group.set_transform(copy, m).map_err(e)?;
            }
        }
        Edit::Copy(source, guid, position) => {
            let copy = group.copy_object_from(donor, *source, *guid).map_err(e)?;
            if group.transform(&group.objects[copy]).is_some() {
                group.set_position(copy, *position).map_err(e)?;
            }
        }
        Edit::Remove(guid) => {
            let Some(o) = find(group, guid) else {
                return Ok(false);
            };
            group.remove_object(o).map_err(e)?;
        }
    }
    Ok(true)
}

/// Makes `edits` on `base`. With `each_written` every edit is written and
/// the group read again before the next, as the editor does; otherwise all
/// edits are made on one parsed group, as a project build does.
pub(super) fn play(
    base: &[u8],
    donor: &SimGroup,
    edits: &[Edit],
    each_written: bool,
) -> Result<Vec<u8>, String> {
    let mut group = SimGroup::parse(base).map_err(|e| e.to_string())?;
    for edit in edits {
        play_one(&mut group, donor, edit)?;
        if each_written {
            group = SimGroup::parse(&group.encode()).map_err(|e| format!("reparse: {e}"))?;
        }
    }
    Ok(group.encode())
}

fn position(rng: &mut XorShift) -> [f32; 3] {
    [rng.float(), rng.float(), rng.float()]
}

/// Makes edits one after another, each chosen for the group as the edits
/// before it left it.
pub(super) struct Author<'a> {
    rng: XorShift,
    group: SimGroup,
    donor: &'a SimGroup,
    pub(super) edits: Vec<Edit>,
    new_objects: usize,
    /// What the GUIDs of its new objects are made from: two authors with
    /// other names make other GUIDs.
    name: &'static str,
}

impl<'a> Author<'a> {
    pub(super) fn new(donor: &'a SimGroup, seed: u64) -> Self {
        Self::named(donor, seed, "delta")
    }

    pub(super) fn named(donor: &'a SimGroup, seed: u64, name: &'static str) -> Self {
        Self {
            rng: XorShift(seed | 1),
            group: donor.clone(),
            donor,
            edits: Vec::new(),
            new_objects: 0,
            name,
        }
    }

    fn make(&mut self, edit: Edit) -> Result<(), String> {
        play_one(&mut self.group, self.donor, &edit)?;
        self.edits.push(edit);
        Ok(())
    }

    fn new_guid(&mut self) -> Guid {
        self.new_objects += 1;
        let (group, donor) = (&self.group, self.donor);
        new_object_guid(&format!("{}{}", self.name, self.new_objects), |g| {
            find(group, g).is_some() || find(donor, g).is_some()
        })
    }

    /// An object with a transform, if the group has one.
    fn placed(&mut self) -> Option<Guid> {
        let n = self.group.objects.len();
        let start = self.rng.below(n);
        (0..n)
            .map(|k| &self.group.objects[(start + k) % n])
            .find(|o| self.group.transform(o).is_some())
            .map(|o| o.guid)
    }

    fn any(&mut self) -> Option<Guid> {
        let n = self.group.objects.len();
        (n > 0).then(|| self.group.objects[self.rng.below(n)].guid)
    }

    fn move_one(&mut self) -> Result<Option<Guid>, String> {
        let Some(guid) = self.placed() else {
            return Ok(None);
        };
        let to = position(&mut self.rng);
        self.make(Edit::Move(guid, to))?;
        Ok(Some(guid))
    }

    pub(super) fn move_again(&mut self, guid: Guid) -> Result<(), String> {
        let to = position(&mut self.rng);
        self.make(Edit::Move(guid, to))
    }

    /// A random value for a random attribute of a random object.
    pub(super) fn set_one(&mut self) -> Result<bool, String> {
        for _ in 0..50 {
            let n = self.group.objects.len();
            if n == 0 {
                return Ok(false);
            }
            let object = &self.group.objects[self.rng.below(n)];
            if object.packets.is_empty() {
                continue;
            }
            let packet = &self.group.packets[object.packets[self.rng.below(object.packets.len())]];
            if packet.attr_count == 0 {
                continue;
            }
            let (guid, class) = (object.guid, packet.class.0);
            let index = self.rng.below(packet.attr_count as usize) as u16;
            let rng = &mut self.rng;
            let value = if rng.below(8) == 0 {
                None
            } else {
                Some(match self.group.attr_kind(class, index) {
                    Some(AttrKind::Int) => Value::Int(rng.below(2000) as i32 - 1000),
                    Some(AttrKind::Float) => Value::Float(rng.float()),
                    Some(AttrKind::Hash) => Value::Hash(Hash32(rng.next() as u32)),
                    Some(AttrKind::String) => Value::String(format!("edit_{}", rng.below(5))),
                    Some(AttrKind::Guid) => {
                        Value::Guid(Guid::from_resource_id(rng.next() as u32 | 1))
                    }
                    Some(AttrKind::Vec3) => Value::Vec3(position(rng)),
                    Some(AttrKind::Matrix) => Value::Matrix(
                        Transform {
                            position: position(rng),
                            rotation: position(rng),
                            scale: [1.0, 2.0, 0.5],
                        }
                        .to_matrix(),
                    ),
                    None => continue,
                })
            };
            self.make(Edit::Set(guid, class, index, value))?;
            return Ok(true);
        }
        Ok(false)
    }

    pub(super) fn duplicate_one(&mut self) -> Result<Option<Guid>, String> {
        let Some(source) = self.any() else {
            return Ok(None);
        };
        let guid = self.new_guid();
        self.make(Edit::Duplicate(source, guid, [1.0, 0.0, 2.5]))?;
        Ok(Some(guid))
    }

    pub(super) fn copy_one(&mut self) -> Result<(), String> {
        let source = self.rng.below(self.donor.objects.len());
        let guid = self.new_guid();
        let to = position(&mut self.rng);
        self.make(Edit::Copy(source, guid, to))
    }

    fn remove_one(&mut self) -> Result<(), String> {
        match self.any() {
            Some(guid) => self.make(Edit::Remove(guid)),
            None => Ok(()),
        }
    }

    pub(super) fn mixed(&mut self, count: usize) -> Result<(), String> {
        for _ in 0..count {
            match self.rng.below(10) {
                0..=2 => {
                    self.move_one()?;
                }
                3..=5 => {
                    self.set_one()?;
                }
                6 => {
                    self.duplicate_one()?;
                }
                7 => self.copy_one()?,
                8 => self.remove_one()?,
                _ => {
                    // Something made before is edited again.
                    let again = self.edits.iter().rev().find_map(|e| match e {
                        Edit::Move(g, _) | Edit::Duplicate(_, g, _) | Edit::Copy(_, g, _) => {
                            Some(*g)
                        }
                        _ => None,
                    });
                    match again {
                        Some(guid)
                            if find(&self.group, &guid).is_some_and(|o| {
                                self.group.transform(&self.group.objects[o]).is_some()
                            }) =>
                        {
                            self.move_again(guid)?;
                        }
                        _ => {
                            self.set_one()?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// The scenarios: a name, whether every edit is written before the next,
/// and what the author does.
type Scenario = (&'static str, bool, fn(&mut Author) -> Result<(), String>);

const SCENARIOS: &[Scenario] = &[
    ("move one object", true, |a| a.move_one().map(drop)),
    ("move A, move B, move A again", true, |a| {
        let first = a.move_one()?;
        a.move_one()?;
        match first {
            Some(guid) => a.move_again(guid),
            None => Ok(()),
        }
    }),
    ("set one attribute", true, |a| a.set_one().map(drop)),
    ("set four attributes", true, |a| {
        for _ in 0..4 {
            a.set_one()?;
        }
        Ok(())
    }),
    ("duplicate an object", true, |a| a.duplicate_one().map(drop)),
    ("duplicate an object, move the copy", true, |a| {
        match a.duplicate_one()? {
            Some(guid) if a.group.transform(a.group.objects.last().unwrap()).is_some() => {
                a.move_again(guid)
            }
            _ => Ok(()),
        }
    }),
    ("copy an object of another group", true, |a| a.copy_one()),
    ("remove an object", true, |a| a.remove_one()),
    ("twelve mixed edits, each written", true, |a| a.mixed(12)),
    ("twelve mixed edits, one write", false, |a| a.mixed(12)),
];

/// How many times every scenario is played on every group, each time with
/// other objects and values.
const ROUNDS: u64 = 4;

#[derive(Debug, Default, Clone, Copy)]
struct Stat {
    /// Groups, in every round, the author found something to edit in.
    groups: usize,
    /// `exact` gave a delta.
    exact: usize,
    /// Of those, deltas that needed a layout.
    with_layout: usize,
    /// The edits are not operations of a delta.
    no_operations: usize,
    resource_bytes: usize,
    delta_bytes: usize,
}

#[test]
fn simgroup_deltas_give_the_authors_bytes_back() {
    let stats: Mutex<BTreeMap<usize, Stat>> = Mutex::new(BTreeMap::new());
    let type_id = Kind::SimGroup.type_id();
    let count = check_every("simgroup", |data| {
        let donor = SimGroup::parse(data).map_err(|e| e.to_string())?;
        if donor.objects.is_empty() {
            return Ok(());
        }
        let rounds =
            (0..ROUNDS).flat_map(|round| SCENARIOS.iter().enumerate().map(move |s| (round, s)));
        for (round, (number, (name, each_written, script))) in rounds {
            let seed = (0x9E37_79B9_7F4A_7C15 ^ (data.len() as u64) << 8 ^ number as u64)
                .wrapping_mul(2 * round + 1);
            let mut author = Author::new(&donor, seed);
            script(&mut author).map_err(|e| format!("{name}: {e}"))?;
            if author.edits.is_empty() {
                // Nothing to move in a group without placed objects.
                continue;
            }
            let mine = play(data, &donor, &author.edits, *each_written)
                .map_err(|e| format!("{name}: {e}"))?;
            let mut stat = Stat {
                groups: 1,
                ..Stat::default()
            };
            match delta::exact(type_id, data, &mine).map_err(|e| format!("{name}: {e}"))? {
                Some(bytes) => {
                    // The rule itself, once more from the outside.
                    let applied = delta::apply_resource(type_id, &bytes, data)
                        .map_err(|e| format!("{name}: {e}"))?;
                    if applied.data != mine
                        || !applied.skipped.is_empty()
                        || !applied.conflicts.is_empty()
                    {
                        return Err(format!("{name}: an exact delta does not give the bytes"));
                    }
                    let document = Delta::parse(&bytes).map_err(|e| e.to_string())?;
                    if document.to_bytes() != bytes {
                        return Err(format!("{name}: the document is not canonical"));
                    }
                    let Delta::SimGroup(document) = document else {
                        return Err("not a SimGroup delta".into());
                    };
                    stat.exact = 1;
                    stat.with_layout = usize::from(document.layout.is_some());
                    stat.resource_bytes = mine.len();
                    stat.delta_bytes = bytes.len();
                }
                None => {
                    let operations = delta::simgroup::operations(data, &mine)
                        .map_err(|e| format!("{name}: {e}"))?;
                    stat.no_operations = usize::from(operations.is_none());
                }
            }
            let mut stats = stats.lock().unwrap();
            let total = stats.entry(number).or_default();
            total.groups += stat.groups;
            total.exact += stat.exact;
            total.with_layout += stat.with_layout;
            total.no_operations += stat.no_operations;
            total.resource_bytes += stat.resource_bytes;
            total.delta_bytes += stat.delta_bytes;
        }
        Ok(())
    });
    if count == 0 {
        return;
    }
    assert_eq!(count, 2006);
    let stats = stats.into_inner().unwrap();
    eprintln!("SimGroup deltas, {count} groups, {ROUNDS} rounds:");
    for (number, stat) in &stats {
        let exact = stat.exact.max(1);
        eprintln!(
            "  {:<36} cases {:>4}  exact {:>4} ({:6.2}%)  by the operations alone {:>4} \
             ({:5.1}%)  not operations {:>2}  delta {:>5} B of {:>6} B",
            SCENARIOS[*number].0,
            stat.groups,
            stat.exact,
            100.0 * stat.exact as f64 / stat.groups as f64,
            stat.exact - stat.with_layout,
            100.0 * (stat.exact - stat.with_layout) as f64 / stat.groups as f64,
            stat.no_operations,
            stat.delta_bytes / exact,
            stat.resource_bytes / exact,
        );
    }
    // The target of stage 109: every edit of every group comes back byte
    // for byte.
    for (number, stat) in &stats {
        assert!(stat.groups > 5_000, "{}: {stat:?}", SCENARIOS[*number].0);
        assert_eq!(stat.exact, stat.groups, "{}", SCENARIOS[*number].0);
    }
}

#[test]
fn simgroup_deltas_apply_to_a_changed_base_as_the_edits_by_hand() {
    let done = AtomicUsize::new(0);
    let removed = AtomicUsize::new(0);
    let type_id = Kind::SimGroup.type_id();
    let count = check_every("simgroup", |data| {
        let donor = SimGroup::parse(data).map_err(|e| e.to_string())?;
        if donor.objects.is_empty() {
            return Ok(());
        }
        let mut author = Author::new(&donor, 0xD1B5_4A32_D192_ED03 ^ data.len() as u64);
        author.mixed(6)?;
        let mine = play(data, &donor, &author.edits, true)?;
        let bytes = delta::exact(type_id, data, &mine)
            .map_err(|e| e.to_string())?
            .ok_or("the edits are not an exact delta")?;
        // Another mod changed the base: it added an object of its own, and
        // copied, removed and moved objects the author did not touch, as
        // many of the three as the group has.
        let touched: HashSet<Guid> = author.edits.iter().flat_map(Edit::touches).collect();
        let free: Vec<Guid> = donor
            .objects
            .iter()
            .map(|o| o.guid)
            .filter(|g| !touched.contains(g))
            .collect();
        let taken = |g: &Guid| find(&donor, g).is_some() || touched.contains(g);
        let mut other = vec![Edit::Copy(
            0,
            new_object_guid("other copy", taken),
            [9.0, 8.0, 7.0],
        )];
        if let Some(&source) = free.first() {
            let guid = new_object_guid("other duplicate", taken);
            other.push(Edit::Duplicate(source, guid, [0.0, 3.0, 0.0]));
        }
        if let Some(&victim) = free.get(1) {
            other.push(Edit::Remove(victim));
            removed.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(third) = free.get(2)
            && let Some(at) = find(&donor, third)
            && donor.transform(&donor.objects[at]).is_some()
        {
            other.push(Edit::Move(*third, [5.0, 6.0, 7.0]));
        }
        let changed = play(data, &donor, &other, true)?;
        let applied =
            delta::apply_resource(type_id, &bytes, &changed).map_err(|e| e.to_string())?;
        if !applied.skipped.is_empty() || !applied.conflicts.is_empty() {
            return Err(format!(
                "skipped {:?}, conflicts {:?}",
                applied.skipped, applied.conflicts
            ));
        }
        let got = SimGroup::parse(&applied.data).map_err(|e| format!("the result: {e}"))?;
        let by_hand = play(&changed, &donor, &author.edits, true)?;
        let expected = SimGroup::parse(&by_hand).map_err(|e| e.to_string())?;
        if simgroup_view(&got) != simgroup_view(&expected) {
            return Err(format!(
                "the delta on a changed base differs from the edits by hand: {:?} over {other:?}",
                author.edits
            ));
        }
        if got.encode() != applied.data {
            return Err("the result is not written as it is read".into());
        }
        done.fetch_add(1, Ordering::Relaxed);
        Ok(())
    });
    if count == 0 {
        return;
    }
    let (done, removed) = (done.into_inner(), removed.into_inner());
    eprintln!(
        "SimGroup deltas applied to a changed base as the edits by hand: {done} of {count} \
         groups; in {removed} of them the other mod also removed an object"
    );
    assert_eq!(done, count);
}

/// Counts of the tests of the other three kinds.
#[derive(Default)]
struct Counts {
    exact: AtomicUsize,
    not_exact: AtomicUsize,
    rebased: AtomicUsize,
}

impl Counts {
    fn add(counter: &AtomicUsize) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    fn report(self, what: &str, count: usize) -> (usize, usize, usize) {
        let (exact, not_exact, rebased) = (
            self.exact.into_inner(),
            self.not_exact.into_inner(),
            self.rebased.into_inner(),
        );
        eprintln!(
            "{what} deltas, {count} resources: exact {exact}, kept whole {not_exact}, applied \
             to a changed base as by hand {rebased}"
        );
        (exact, not_exact, rebased)
    }
}

/// `exact` for one pair, with the rule checked from the outside. `None`
/// when the change is kept whole.
fn exact_checked(type_id: u32, base: &[u8], mine: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let Some(bytes) = delta::exact(type_id, base, mine).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let applied = delta::apply_resource(type_id, &bytes, base).map_err(|e| e.to_string())?;
    if applied.data != mine || !applied.skipped.is_empty() || !applied.conflicts.is_empty() {
        return Err("an exact delta does not give the bytes".into());
    }
    let document = Delta::parse(&bytes).map_err(|e| e.to_string())?;
    if document.to_bytes() != bytes {
        return Err("the document is not canonical".into());
    }
    Ok(Some(bytes))
}

#[test]
fn string_table_deltas_give_the_authors_bytes_back() {
    use doctor_toolkit::resource::lh2::StringTable;

    let counts = Counts::default();
    let type_id = Kind::Lh2.type_id();
    let count = check_every("lh2", |data| {
        let table = StringTable::parse(data).map_err(|e| e.to_string())?;
        if table.hashes.is_empty() || table.strings.is_empty() {
            return Ok(());
        }
        let e = |e: doctor_toolkit::Error| e.to_string();
        let mut rng = XorShift(0x2545_F491_4F6C_DD1D ^ data.len() as u64);
        // The author: three texts and two new strings. The strings are
        // found by hash, as they have to be on a changed table.
        let edited: Vec<(u32, usize)> = (0..3)
            .map(|_| {
                (
                    table.hashes[rng.below(table.hashes.len())],
                    rng.below(table.strings.len()),
                )
            })
            .collect();
        let mut new = Vec::new();
        while new.len() < 3 {
            let hash = rng.next() as u32;
            if table.find(hash).is_none() && !new.contains(&hash) {
                new.push(hash);
            }
        }
        let other_new = new.pop().expect("three hashes");
        let author = |table: &mut StringTable| -> Result<(), String> {
            for (k, &(hash, column)) in edited.iter().enumerate() {
                let row = table.find(hash).expect("the string is in the table");
                table.strings[column][row] = format!("Edited {k} «ё»");
            }
            for &hash in &new {
                let texts: Vec<String> = (0..table.strings.len())
                    .map(|c| format!("New {hash:08x} in column {c}"))
                    .collect();
                table.insert(hash, &texts).map_err(e)?;
            }
            Ok(())
        };
        let mut mine = table.clone();
        author(&mut mine)?;
        let mine = mine.to_bytes().map_err(e)?;
        let Some(bytes) = exact_checked(type_id, data, &mine)? else {
            Counts::add(&counts.not_exact);
            return Ok(());
        };
        Counts::add(&counts.exact);

        // Another mod: another text and a string of its own.
        let mut changed = table.clone();
        let free = (0..table.hashes.len())
            .find(|&row| edited.iter().all(|&(hash, _)| hash != table.hashes[row]));
        if let Some(row) = free {
            changed.strings[0][row] = "Changed by another mod".into();
        }
        let texts = vec!["Another".to_owned(); changed.strings.len()];
        changed.insert(other_new, &texts).map_err(e)?;
        let mut by_hand = changed.clone();
        author(&mut by_hand)?;
        let changed = changed.to_bytes().map_err(e)?;
        let applied = delta::apply_resource(type_id, &bytes, &changed).map_err(e)?;
        if applied.data != by_hand.to_bytes().map_err(e)?
            || !applied.skipped.is_empty()
            || !applied.conflicts.is_empty()
        {
            return Err("the delta on a changed table differs from the edits by hand".into());
        }
        Counts::add(&counts.rebased);
        Ok(())
    });
    if count == 0 {
        return;
    }
    let (exact, not_exact, rebased) = counts.report("string table", count);
    assert_eq!(count, 21);
    assert_eq!((exact, not_exact, rebased), (count, 0, count));
}

#[test]
fn parts_database_deltas_give_the_authors_bytes_back() {
    use doctor_toolkit::resource::partsdb::{Part, PartsDb};

    let counts = Counts::default();
    let type_id = Kind::PartsDb.type_id();
    let count = check_every("partsdb", |data| {
        let db = PartsDb::parse(data).map_err(|e| e.to_string())?;
        let e = |e: doctor_toolkit::Error| e.to_string();
        let mut rng = XorShift(0x9E37_79B9_7F4A_7C15 ^ data.len() as u64);
        // A new part in the slot of a part of the database, a variant of it
        // and of an old part, and the variant allowed in a set.
        let old_part = (!db.parts.is_empty()).then(|| rng.below(db.parts.len()));
        let slot = old_part.and_then(|p| db.parts[p].slot);
        let set = (!db.sets.is_empty()).then(|| rng.below(db.sets.len()));
        let add = |db: &mut PartsDb, name: &str, model: u32| -> Result<(), String> {
            let part = db
                .add_part(Part {
                    name: format!("{name}_part"),
                    slot,
                    models: [model, model + 1, 0, 0, 0].map(Hash32),
                    label: String::new(),
                    tags: vec!["doctorCulled".into()],
                })
                .map_err(e)?;
            let mut parts = vec![part as u16];
            parts.extend(old_part.map(|p| p as u16));
            let variant = db
                .add_variant(&format!("{name}_variant"), parts)
                .map_err(e)?;
            if let (Some(set), Some(slot)) = (set, slot) {
                db.allow_variant(set, slot, variant, false).map_err(e)?;
            }
            Ok(())
        };
        let mut mine = db.clone();
        add(&mut mine, "doctor_delta", 0x0D0C_0001)?;
        let mine = mine.encode().map_err(e)?;
        let Some(bytes) = exact_checked(type_id, data, &mine)? else {
            Counts::add(&counts.not_exact);
            return Ok(());
        };
        Counts::add(&counts.exact);

        // Another mod added its part first: the numbers move, the names do not.
        let mut changed = db.clone();
        add(&mut changed, "another_mod", 0x0A0B_0001)?;
        let mut by_hand = changed.clone();
        add(&mut by_hand, "doctor_delta", 0x0D0C_0001)?;
        let changed = changed.encode().map_err(e)?;
        let applied = delta::apply_resource(type_id, &bytes, &changed).map_err(e)?;
        if applied.data != by_hand.encode().map_err(e)? || !applied.skipped.is_empty() {
            return Err(format!(
                "the delta on a changed database differs from the edits by hand: {:?}",
                applied.skipped
            ));
        }
        Counts::add(&counts.rebased);
        Ok(())
    });
    if count == 0 {
        return;
    }
    let (exact, not_exact, rebased) = counts.report("parts database", count);
    assert_eq!(count, 68);
    assert_eq!((exact, not_exact, rebased), (count, 0, count));
}

#[test]
fn world_grid_deltas_give_the_authors_bytes_back() {
    use doctor_toolkit::resource::wgi::{self, NewCell, WorldGrid};

    let counts = Counts::default();
    let removed = AtomicUsize::new(0);
    let type_id = Kind::Wgi.type_id();
    let count = check_every("wgi", |data| {
        let grid = WorldGrid::parse(data).map_err(|e| e.to_string())?;
        let mut rng = XorShift(0xD1B5_4A32_D192_ED03 ^ data.len() as u64);
        let e = |e: doctor_toolkit::Error| e.to_string();
        let mut zone = |seed: u8| {
            let mut guid = *b"\0\0\0X\0\0\0\0DOCTORTK";
            guid[0] = seed;
            let at = [rng.float(), 0.0, rng.float()];
            NewCell {
                guid,
                box_min: at,
                box_max: [at[0] + 40.0, 12.0, at[2] + 25.0],
                kind: wgi::KIND_WORLD,
                flags: 0,
                lod_mask: grid.main_lod_mask().unwrap_or(wgi::lod::HIGH),
                name: Some(format!("Doctor_Zone_{seed}")),
                // The parent: a stream of the table, as a new zone has.
                needs: (!grid.streams.is_empty())
                    .then(|| grid.streams[rng.below(grid.streams.len())].hash()),
            }
        };
        // The author adds two zones and takes a cell of the game out.
        let (first, second, other) = (zone(1), zone(2), zone(3));
        let victim = (!grid.cells.is_empty()).then(|| grid.cells[rng.below(grid.cells.len())].guid);
        let author = |grid: &mut WorldGrid| -> Result<(), String> {
            grid.add_cell(&first).map_err(e)?;
            grid.add_cell(&second).map_err(e)?;
            if let Some(victim) = victim
                && let Some(cell) = grid.cells.iter().position(|c| c.guid == victim)
            {
                grid.remove_cell(cell).map_err(e)?;
                grid.trim_pool();
            }
            Ok(())
        };
        let mut mine = grid.clone();
        author(&mut mine)?;
        let mine = mine.to_bytes().map_err(e)?;
        let Some(bytes) = exact_checked(type_id, data, &mine)? else {
            Counts::add(&counts.not_exact);
            return Ok(());
        };
        Counts::add(&counts.exact);
        removed.fetch_add(usize::from(victim.is_some()), Ordering::Relaxed);

        // Another mod added its zone to the same grid.
        let mut changed = grid.clone();
        changed.add_cell(&other).map_err(e)?;
        let mut by_hand = changed.clone();
        author(&mut by_hand)?;
        let changed = changed.to_bytes().map_err(e)?;
        let applied = delta::apply_resource(type_id, &bytes, &changed).map_err(e)?;
        if applied.data != by_hand.to_bytes().map_err(e)? || !applied.skipped.is_empty() {
            return Err("the delta on a changed grid differs from the edits by hand".into());
        }
        Counts::add(&counts.rebased);
        Ok(())
    });
    if count == 0 {
        return;
    }
    let (exact, not_exact, rebased) = counts.report("world grid", count);
    eprintln!("  cells of the game removed: {}", removed.into_inner());
    assert_eq!(count, 28);
    assert_eq!((exact, not_exact, rebased), (count, 0, count));
}
